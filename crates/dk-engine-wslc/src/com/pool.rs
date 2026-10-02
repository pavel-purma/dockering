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
/// order. Dropping the pool closes the queue; threads exit after their current job.
pub struct RpcPool {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// Default size: 3 (spec: 2–4). One slow call (e.g. `Stats()` blocks ~1 s server-side, or a
/// `PullImage`) can't starve list/inspect calls.
pub const DEFAULT_THREADS: usize = 3;

impl RpcPool {
    pub fn new(threads: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let n = threads.clamp(1, 8);
        let mut handles = Vec::with_capacity(n);
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
            match spawned {
                Ok(h) => handles.push(h),
                Err(e) => tracing::error!(%e, "failed to spawn WSLC RPC thread"),
            }
        }
        Arc::new(Self {
            tx: Mutex::new(Some(tx)),
            threads: Mutex::new(handles),
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

    /// Closes the queue and joins the threads (blocking). Called from `Drop`.
    fn shutdown(&self) {
        if let Ok(mut g) = self.tx.lock() {
            g.take();
        }
        let handles = self
            .threads
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        let me = std::thread::current().id();
        for h in handles {
            // A job that drops the last engine reference runs *on* a pool thread; never
            // join ourselves.
            if h.thread().id() != me {
                let _ = h.join();
            }
        }
    }
}

impl Drop for RpcPool {
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
