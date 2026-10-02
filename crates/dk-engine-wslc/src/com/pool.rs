//! COM threading (spec 20 §5.4): a small MTA **RPC pool** for short blocking calls, and a
//! dedicated MTA thread per long-lived stream ([`spawn_stream_thread`]).
//!
//! No tokio here: callers `await` a `futures` oneshot, so the pool works from any executor and
//! never blocks an async worker or the UI thread. COM calls must only happen on these threads.

#![cfg(windows)]

use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use dk_core::{EngineError, EngineResult};
use futures::channel::oneshot;

use super::ffi::MtaGuard;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed set of OS threads, each in the process MTA, executing submitted closures in FIFO
/// order.
///
/// Dropping the pool closes the queue and **never blocks**: the threads are detached, finish
/// the jobs already queued (each job owns everything it touches, e.g. an `Arc` of the engine
/// state, so it stays valid without the pool) and then exit on their own. Pools are often
/// dropped on hub tokio workers while a COM call is in flight (a discovery or connect timed out
/// against a cold or hung `wslservice`); joining there would block the worker.
pub struct RpcPool {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
}

/// Default size: 3 (spec: 2–4). One slow call (e.g. `Stats()` blocks ~1 s server-side, or a
/// `PullImage`) can't starve list/inspect calls.
pub const DEFAULT_THREADS: usize = 3;

impl RpcPool {
    pub fn new(threads: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let n = threads.clamp(1, 8);
        for i in 0..n {
            let rx = rx.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("dk-wslc-rpc-{i}"))
                .spawn(move || {
                    let _mta = MtaGuard::enter();
                    loop {
                        // Hold the lock only while dequeuing.
                        let job = match rx.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return, // queue closed
                        }
                    }
                });
            // Detached: the thread ends when the queue closes (see the type docs).
            if let Err(e) = spawned {
                tracing::error!(%e, "failed to spawn WSLC RPC thread");
            }
        }
        Arc::new(Self {
            tx: Mutex::new(Some(tx)),
        })
    }

    /// Runs `f` on a pool thread; resolves with its result. A panic inside `f` becomes
    /// `Protocol` (NFR-030) and the thread keeps serving.
    pub fn run<T, F>(&self, f: F) -> impl Future<Output = EngineResult<T>> + Send + 'static
    where
        T: Send + 'static,
        F: FnOnce() -> EngineResult<T> + Send + 'static,
    {
        let (otx, orx) = oneshot::channel::<EngineResult<T>>();
        let job: Job = Box::new(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .unwrap_or_else(|_| Err(EngineError::protocol("WSLC COM call panicked")));
            let _ = otx.send(r);
        });
        let sent = self
            .tx
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|tx| tx.send(job).is_ok()))
            .unwrap_or(false);
        async move {
            if !sent {
                return Err(EngineError::unreachable("WSLC COM pool is shut down"));
            }
            orx.await
                .unwrap_or_else(|_| Err(EngineError::protocol("WSLC COM job dropped")))
        }
    }

    /// Closes the queue without waiting for the threads (they exit after the queued jobs).
    fn shutdown(&self) {
        let tx = match self.tx.lock() {
            Ok(mut g) => g.take(),
            Err(e) => e.into_inner().take(),
        };
        drop(tx);
    }
}

impl Drop for RpcPool {
    /// Non-blocking (never joins): safe on async workers and on the pool's own threads.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Spawns a dedicated MTA thread for a long-lived blocking stream (events `GetNext`, a logs
/// reader, an exec TTY reader). `body` owns its COM pointers and handles; it must return once
/// its cancel signal fires (cancel event / `CancelIoEx`).
pub fn spawn_stream_thread<F>(name: &str, body: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(format!("dk-wslc-{name}"))
        .spawn(move || {
            let _mta = MtaGuard::enter();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
                tracing::error!("WSLC stream thread panicked");
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_and_survives_panics() {
        let pool = RpcPool::new(2);
        let r = futures::executor::block_on(pool.run(|| Ok(21 * 2)));
        assert_eq!(r, Ok(42));
        let p = futures::executor::block_on(pool.run(|| -> EngineResult<()> { panic!("boom") }));
        assert!(matches!(p, Err(EngineError::Protocol(_))));
        // Still serving after the panic.
        let r = futures::executor::block_on(pool.run(|| Ok("ok")));
        assert_eq!(r, Ok("ok"));
    }

    #[test]
    fn jobs_run_in_the_mta() {
        use windows::Win32::System::Com::{APTTYPE, APTTYPEQUALIFIER, CoGetApartmentType};
        let pool = RpcPool::new(1);
        let apt = futures::executor::block_on(pool.run(|| {
            let mut t = APTTYPE::default();
            let mut q = APTTYPEQUALIFIER::default();
            // SAFETY: plain out-params on the current thread.
            unsafe { CoGetApartmentType(&mut t, &mut q) }
                .map_err(|e| EngineError::protocol(e.to_string()))?;
            Ok(t.0)
        }));
        // APTTYPE_MTA = 1
        assert_eq!(apt, Ok(1));
    }

    #[test]
    fn concurrent_jobs_use_several_threads() {
        let pool = RpcPool::new(3);
        let futs: Vec<_> = (0..3)
            .map(|_| {
                pool.run(|| {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    Ok(std::thread::current().name().map(str::to_owned))
                })
            })
            .collect();
        let t = std::time::Instant::now();
        let names = futures::executor::block_on(futures::future::join_all(futs));
        assert!(
            t.elapsed() < std::time::Duration::from_millis(400),
            "{:?}",
            t.elapsed()
        );
        let mut names: Vec<_> = names.into_iter().map(|r| r.ok().flatten()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 3);
    }

    /// Review finding 1: dropping a pool while a job is in flight returns immediately; the job
    /// still completes on its detached thread and delivers its result.
    #[test]
    fn drop_does_not_wait_for_in_flight_job() {
        let pool = RpcPool::new(1);
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let state = Arc::new(Mutex::new(Vec::<u32>::new()));
        let job_state = state.clone();
        let fut = pool.run(move || {
            let _ = started_tx.send(());
            std::thread::sleep(std::time::Duration::from_secs(2));
            job_state
                .lock()
                .map_err(|_| EngineError::protocol("poisoned"))?
                .push(7);
            Ok(7)
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("job started");
        let t = std::time::Instant::now();
        drop(pool);
        let took = t.elapsed();
        assert!(
            took < std::time::Duration::from_millis(100),
            "drop took {took:?}"
        );
        // The job owns what it needs, so it finishes and its result is still delivered.
        assert_eq!(futures::executor::block_on(fut), Ok(7));
        assert_eq!(*state.lock().expect("lock"), vec![7]);
    }

    #[test]
    fn dropping_pool_fails_pending_submissions_cleanly() {
        let pool = RpcPool::new(1);
        let fut = pool.run(|| Ok(1));
        drop(pool);
        // Already-queued job either ran or was dropped; never hangs.
        let r = futures::executor::block_on(fut);
        assert!(r == Ok(1) || matches!(r, Err(EngineError::Protocol(_))));
    }
}
