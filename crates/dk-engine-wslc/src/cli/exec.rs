//! Interactive exec through ConPTY (spec 21 §5, WSLC CLI fallback): `portable-pty` runs
//! `wslc.exe [--session s] container exec -i -t [-e K=V] [-u U] [-w D] <id> <cmd…>` in a
//! pseudo console. Resize goes through `MasterPty::resize`; the exit code is the child's.
//!
//! Blocking PTY I/O never runs on the async runtime: the reader is a dedicated std thread,
//! writes and resizes go through `spawn_blocking`, and `wait` blocks on its own thread.

use dk_core::{EngineResult, ExecRequest};

/// argv after `wslc.exe` (and the global `--session`): `container exec -i -t … <id> <cmd…>`.
/// Values were validated by the caller.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn exec_args(id: &str, req: &ExecRequest) -> Vec<String> {
    let mut a: Vec<String> = vec!["container".into(), "exec".into(), "-i".into()];
    if req.tty {
        a.push("-t".into());
    }
    for e in &req.env {
        a.push("-e".into());
        a.push(e.clone());
    }
    if let Some(u) = &req.user {
        a.push("-u".into());
        a.push(u.clone());
    }
    if let Some(w) = &req.working_dir {
        a.push("-w".into());
        a.push(w.clone());
    }
    a.push(id.to_owned());
    let cmd = if req.cmd.is_empty() {
        ExecRequest::default_shell_cmd()
    } else {
        req.cmd.clone()
    };
    a.extend(cmd);
    a
}

/// ConPTY's start-up cursor query (Device Status Report 6).
const DSR_CPR: &[u8] = b"\x1b[6n";

/// If `chunk` contains the cursor query, the chunk without its first occurrence.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn strip_first_dsr(chunk: &bytes::Bytes) -> Option<bytes::Bytes> {
    let pos = chunk.windows(DSR_CPR.len()).position(|w| w == DSR_CPR)?;
    let mut out = Vec::with_capacity(chunk.len() - DSR_CPR.len());
    out.extend_from_slice(&chunk[..pos]);
    out.extend_from_slice(&chunk[pos + DSR_CPR.len()..]);
    Some(out.into())
}

/// Validate the free-form exec fields that become argv values (NFR-022): no option-looking
/// values, no NULs. `cmd` elements go after the container id and are passed verbatim (wslc
/// stops option parsing at the first positional), but must not contain NUL.
pub(crate) fn validate_exec(req: &ExecRequest) -> EngineResult<()> {
    use dk_core::validate::validate_env_key;
    let bad = |what: &str, v: &str| dk_core::EngineError::Api {
        status: 400,
        message: format!(
            "invalid {what}: \"{}\"",
            v.chars().take(80).collect::<String>()
        ),
    };
    for e in &req.env {
        let key = e.split_once('=').map_or(e.as_str(), |(k, _)| k);
        validate_env_key(key)?;
        if e.contains('\0') {
            return Err(bad("env", key));
        }
    }
    for (what, v) in [("user", &req.user), ("working dir", &req.working_dir)] {
        if let Some(v) = v
            && (v.is_empty() || v.starts_with('-') || v.contains('\0') || v.contains('\n'))
        {
            return Err(bad(what, v));
        }
    }
    if req.cmd.iter().any(|c| c.contains('\0')) {
        return Err(bad("command", "<NUL>"));
    }
    if req.cmd.first().is_some_and(|c| c.starts_with('-')) {
        return Err(bad("command", &req.cmd[0]));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) use imp::spawn;

#[cfg(windows)]
pub(crate) struct SpawnAdmission {
    state: std::sync::atomic::AtomicU8,
    cancel: tokio_util::sync::CancellationToken,
}
#[cfg(windows)]
impl SpawnAdmission {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::atomic::AtomicU8::new(0),
            cancel: tokio_util::sync::CancellationToken::new(),
        })
    }
    pub(crate) fn admit(&self) -> EngineResult<()> {
        self.state
            .compare_exchange(
                0,
                1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| dk_core::EngineError::Cancelled)
    }
    pub(crate) fn cancel(&self) {
        let _ = self.state.compare_exchange(
            0,
            2,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
        self.cancel.cancel();
    }
    fn check_cancelled(&self) -> EngineResult<()> {
        if self.cancel.is_cancelled() {
            Err(dk_core::EngineError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(crate) async fn validate(
        &self,
        guard: impl Future<Output = EngineResult<()>>,
    ) -> EngineResult<()> {
        tokio::select! { biased; _ = self.cancel.cancelled() => Err(dk_core::EngineError::Cancelled), result = guard => result }
    }
}
#[cfg(windows)]
pub(crate) struct CancelSpawnOnDrop(pub(crate) std::sync::Arc<SpawnAdmission>);
#[cfg(windows)]
impl Drop for CancelSpawnOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(windows)]
pub(crate) async fn spawn_checked(
    exe: std::path::PathBuf,
    argv: Vec<String>,
    cols: u16,
    rows: u16,
    guard: impl Future<Output = EngineResult<()>> + Send + 'static,
) -> EngineResult<Box<dyn dk_core::TerminalSession>> {
    let runtime = tokio::runtime::Handle::current();
    let admission = SpawnAdmission::new();
    let _cancel_on_drop = CancelSpawnOnDrop(admission.clone());
    tokio::task::spawn_blocking(move || {
        admission.check_cancelled()?;
        spawn(&exe, argv, cols, rows, move || {
            runtime.block_on(admission.validate(guard))?;
            admission.admit()
        })
    })
    .await
    .map_err(|e| dk_core::EngineError::protocol(format!("exec spawn task failed: {e}")))?
}

#[cfg(windows)]
mod imp {
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bytes::Bytes;
    use dk_core::{EngineError, EngineResult, EngineStream, TerminalSession};
    use futures::channel::mpsc;
    use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    fn pty_err(what: &str, e: impl std::fmt::Display) -> EngineError {
        EngineError::protocol(format!("ConPTY {what} failed: {e}"))
    }

    fn join_err(e: tokio::task::JoinError) -> EngineError {
        EngineError::protocol(format!("blocking PTY task failed: {e}"))
    }

    fn size(cols: u16, rows: u16) -> PtySize {
        PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;
    type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;
    type IoThreads = Arc<Mutex<Vec<Arc<crate::com::win32::OwnedHandle>>>>;
    struct IoRegistration {
        threads: IoThreads,
        thread: Arc<crate::com::win32::OwnedHandle>,
    }
    impl IoRegistration {
        fn new(threads: IoThreads) -> EngineResult<Self> {
            let thread = Arc::new(
                crate::com::win32::current_io_thread().map_err(|e| pty_err("writer thread", e))?,
            );
            threads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(thread.clone());
            Ok(Self { threads, thread })
        }
    }
    impl Drop for IoRegistration {
        fn drop(&mut self) {
            self.threads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|t| !Arc::ptr_eq(t, &self.thread));
        }
    }

    struct SpawnCleanup(Option<Box<dyn ChildKiller + Send + Sync>>);
    impl Drop for SpawnCleanup {
        fn drop(&mut self) {
            if let Some(killer) = &mut self.0 {
                let _ = killer.kill();
            }
        }
    }

    pub(crate) struct ConPtySession {
        master: SharedMaster,
        writer: SharedWriter,
        control: std::sync::mpsc::Sender<Control>,
        cancel: CancellationToken,
        io_threads: IoThreads,
        startup: watch::Receiver<bool>,
        output: Option<mpsc::Receiver<EngineResult<Bytes>>>,
        exit: watch::Receiver<Option<Option<i64>>>,
    }
    enum Control {
        Stop,
        Exited(Option<i64>),
    }

    /// Spawn `exe argv…` inside a new pseudo console of `cols`×`rows`.
    pub(crate) fn spawn(
        exe: &Path,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
        before_spawn: impl FnOnce() -> EngineResult<()>,
    ) -> EngineResult<Box<dyn TerminalSession>> {
        spawn_session(exe, argv, cols, rows, before_spawn)
            .map(|s| Box::new(s) as Box<dyn TerminalSession>)
    }
    fn spawn_session(
        exe: &Path,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
        before_spawn: impl FnOnce() -> EngineResult<()>,
    ) -> EngineResult<ConPtySession> {
        spawn_session_startup(exe, argv, cols, rows, before_spawn, || {})
    }
    fn spawn_session_startup(
        exe: &Path,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
        before_spawn: impl FnOnce() -> EngineResult<()>,
        before_ack: impl FnOnce() + Send + 'static,
    ) -> EngineResult<ConPtySession> {
        let pty = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| pty_err("open", e))?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.args(&argv);
        cmd.env("NO_COLOR", "1");
        before_spawn()?;
        let mut child = pty.slave.spawn_command(cmd).map_err(|e| {
            crate::cli::runner::spawn_error(exe, &std::io::Error::other(e.to_string()))
        })?;
        let mut cleanup = SpawnCleanup(Some(child.clone_killer()));
        // The slave handle must be closed in our process so EOF propagates when the child
        // exits.
        drop(pty.slave);
        let master = pty.master;
        let mut reader = master
            .try_clone_reader()
            .map_err(|_| crate::cli::runner::unknown_outcome())?;
        let writer = master
            .take_writer()
            .map_err(|_| crate::cli::runner::unknown_outcome())?;
        let killer = child.clone_killer();
        let writer: SharedWriter = Arc::new(Mutex::new(Some(writer)));
        let cancel = CancellationToken::new();
        let io_threads: IoThreads = Arc::new(Mutex::new(Vec::new()));
        let cancel_r = cancel.clone();
        let (startup_tx, startup) = watch::channel(false);
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let threads_r = io_threads.clone();

        let (mut tx, rx) = mpsc::channel::<EngineResult<Bytes>>(64);
        let writer_r = writer.clone();
        let reader_thread = std::thread::Builder::new()
            .name("wslc-exec-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                // portable-pty creates the pseudo console with INHERIT_CURSOR, so ConPTY
                // starts by asking for the cursor position (`ESC[6n`) and blocks until it
                // gets a report. Answer that first query here so the session never depends on
                // the UI's terminal emulator replying in time.
                let mut answered_dsr = false;
                let mut seen = 0usize;
                let mut before_ack = Some(before_ack);
                let mut ack_tx = Some(ack_tx);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // ClosePseudoConsole requires output drain, even after Stop.
                            let mut chunk = Bytes::copy_from_slice(&buf[..n]);
                            if !answered_dsr && seen < 256 {
                                seen += n;
                                if let Some(rest) = super::strip_first_dsr(&chunk) {
                                    answered_dsr = true;
                                    if let Some(before_ack) = before_ack.take() { before_ack(); }
                                    // User input is gated until this short response completes:
                                    // it cannot fill the pipe while ConPTY awaits its cursor.
                                    // Register even this write; teardown never writes to stdin.
                                    if let Ok(_registration) = IoRegistration::new(threads_r.clone()) {
                                        let mut g = writer_r.lock().unwrap_or_else(|e| e.into_inner());
                                        if let Some(w) = g.as_mut()
                                            && w.write_all(b"\x1b[1;1R").and_then(|_| w.flush()).is_ok() {
                                                let _ = startup_tx.send(true);
                                        }
                                    }
                                    if let Some(tx) = ack_tx.take() { let _ = tx.send(()); }
                                    // Answered here: don't let the UI emulator answer it too
                                    // (its report would reach the shell as input).
                                    chunk = rest;
                                }
                            }
                            if cancel_r.is_cancelled() { continue; }
                            if chunk.is_empty() {
                                continue;
                            }
                            let sent = futures::executor::block_on(async {
                                tokio::select! { biased;
                                    _ = cancel_r.cancelled() => false,
                                    result = futures::SinkExt::send(&mut tx, Ok(chunk)) => result.is_ok(),
                                }
                            });
                            if !sent {
                                cancel_r.cancel(); // discard/drain until console closes
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break, // pipe closed (process exited / closed)
                    }
                }
            })
            .map_err(|_| crate::cli::runner::unknown_outcome())?;

        let master: SharedMaster = Arc::new(Mutex::new(Some(master)));
        let (exit_tx, exit_rx) = watch::channel(None);
        let (control, commands) = std::sync::mpsc::channel();
        let wait_control = control.clone();
        let (master_w, writer_w) = (master.clone(), writer.clone());
        let wait_thread = std::thread::Builder::new()
            .name("wslc-exec-wait".into())
            .spawn(move || {
                let code = child.wait().ok().map(|s| i64::from(s.exit_code()));
                let _ = wait_control.send(Control::Exited(code));
            })
            .map_err(|_| crate::cli::runner::unknown_outcome())?;
        let cancel_w = cancel.clone();
        let threads_w = io_threads.clone();
        // An owned lifecycle worker exists before handing the session to an async caller.
        // Drop only sends Stop; kill, PTY destruction, reaping and joins never run on hub.
        std::thread::Builder::new()
            .name("wslc-exec-cleanup".into())
            .spawn(move || {
                let mut killer = killer;
                let mut code = None;
                match commands.recv() {
                    Ok(Control::Exited(exit)) => code = exit,
                    Ok(Control::Stop) | Err(_) => {
                        cancel_w.cancel();
                        let _ = killer.kill();
                    }
                }
                cancel_w.cancel();
                // User writes never entered before ack. Let the reader release ConPTY's
                // startup handshake (registered, tiny empty-pipe write) before teardown.
                let _ = ack_rx.recv();
                // Cancellation can race WriteFile entry: repeat until every registered
                // synchronous writer has returned, without holding the writer lock.
                loop {
                    let threads = threads_w.lock().unwrap_or_else(|e| e.into_inner());
                    if threads.is_empty() {
                        break;
                    }
                    for thread in threads.iter() {
                        crate::com::win32::cancel_sync_thread(thread);
                    }
                    // Registration cannot finish/reuse its blocking-pool thread until the
                    // cancellation call has returned; no stale handle cancels another job.
                    drop(threads);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                // No teardown writes: all input (including cursor replies) belongs to
                // registered writers. Input flooding cannot precede startup acknowledgement.
                let mut writer = writer_w.lock().unwrap_or_else(|e| e.into_inner());
                writer.take();
                drop(writer);
                master_w.lock().unwrap_or_else(|e| e.into_inner()).take();
                let _ = wait_thread.join();
                if code.is_none() {
                    while let Ok(command) = commands.try_recv() {
                        if let Control::Exited(exit) = command {
                            code = exit;
                        }
                    }
                }
                let _ = reader_thread.join();
                let _ = exit_tx.send(Some(code));
            })
            .map_err(|_| crate::cli::runner::unknown_outcome())?;
        cleanup.0.take();

        Ok(ConPtySession {
            master,
            writer,
            control,
            cancel,
            io_threads,
            startup,
            output: Some(rx),
            exit: exit_rx,
        })
    }

    #[async_trait]
    impl TerminalSession for ConPtySession {
        fn output(&mut self) -> EngineStream<Bytes> {
            match self.output.take() {
                Some(rx) => Box::pin(rx),
                None => Box::pin(futures::stream::empty()),
            }
        }

        async fn write(&self, data: Bytes) -> EngineResult<()> {
            let mut startup = self.startup.clone();
            while !*startup.borrow_and_update() {
                tokio::select! { biased;
                    _ = self.cancel.cancelled() => return Err(EngineError::Cancelled),
                    result = startup.changed() => if result.is_err() { return Err(EngineError::Cancelled); },
                }
            }
            let writer = self.writer.clone();
            let cancel = self.cancel.clone();
            let threads = self.io_threads.clone();
            tokio::task::spawn_blocking(move || {
                let _registration = IoRegistration::new(threads)?;
                if cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                let mut g = writer
                    .lock()
                    .map_err(|_| EngineError::protocol("PTY writer poisoned"))?;
                let w = g.as_mut().ok_or(EngineError::Cancelled)?;
                if cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                w.write_all(&data)
                    .and_then(|()| w.flush())
                    .map_err(|e| pty_err("write", e))
            })
            .await
            .map_err(join_err)?
        }

        async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
            let master = self.master.clone();
            tokio::task::spawn_blocking(move || {
                let g = master
                    .lock()
                    .map_err(|_| EngineError::protocol("PTY master poisoned"))?;
                match g.as_ref() {
                    Some(m) => m.resize(size(cols, rows)).map_err(|e| pty_err("resize", e)),
                    None => Ok(()), // process already exited: no-op
                }
            })
            .await
            .map_err(join_err)?
        }

        async fn wait(&self) -> EngineResult<Option<i64>> {
            let mut rx = self.exit.clone();
            loop {
                if let Some(code) = *rx.borrow_and_update() {
                    return Ok(code);
                }
                if rx.changed().await.is_err() {
                    return Ok(None);
                }
            }
        }

        async fn close(&self) -> EngineResult<()> {
            self.cancel.cancel();
            let _ = self.control.send(Control::Stop);
            self.wait().await.map(|_| ())
        }
    }

    impl Drop for ConPtySession {
        fn drop(&mut self) {
            self.cancel.cancel();
            let _ = self.control.send(Control::Stop);
        }
    }

    // Keep the trait objects' auto traits honest: the session crosses threads.
    const _: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<ConPtySession>();
    };

    #[cfg(test)]
    mod lifecycle_tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        #[tokio::test]
        async fn conpty_flood_before_startup_ack_close_and_drop_reap() {
            for drop_session in [false, true] {
                let (release, blocked) = std::sync::mpsc::channel();
                let (at_ack, ack_seen) = tokio::sync::oneshot::channel();
                let exe =
                    std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("Windows"))
                        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
                let session = Arc::new(
                    tokio::task::spawn_blocking(move || {
                        spawn_session_startup(
                            &exe,
                            vec![
                                "-NoProfile".into(),
                                "-Command".into(),
                                "Start-Sleep -Seconds 30".into(),
                            ],
                            80,
                            24,
                            || Ok(()),
                            move || {
                                let _ = at_ack.send(());
                                blocked.recv().expect("release ack");
                            },
                        )
                    })
                    .await
                    .expect("worker")
                    .expect("ConPTY"),
                );
                tokio::time::timeout(Duration::from_secs(3), ack_seen)
                    .await
                    .expect("startup query deadline")
                    .expect("startup query");
                let mut exit = session.exit.clone();
                let flood = tokio::spawn({
                    let s = session.clone();
                    async move { s.write(Bytes::from(vec![b'x'; 16 * 1024 * 1024])).await }
                });
                tokio::task::yield_now().await;
                assert!(!*session.startup.borrow(), "ack deliberately withheld");
                assert!(
                    session.io_threads.lock().expect("threads").is_empty(),
                    "user flood has not entered synchronous I/O before ack"
                );
                if drop_session {
                    flood.abort();
                    let _ = flood.await;
                    drop(session);
                    release.send(()).expect("release ack");
                } else {
                    let close = tokio::spawn({
                        let s = session.clone();
                        async move { s.close().await }
                    });
                    tokio::task::yield_now().await;
                    release.send(()).expect("release ack");
                    tokio::time::timeout(Duration::from_secs(3), close)
                        .await
                        .expect("close bounded")
                        .expect("close task")
                        .expect("close");
                    assert_eq!(
                        flood.await.expect("flood waiter"),
                        Err(EngineError::Cancelled)
                    );
                    assert!(session.writer.lock().expect("writer").is_none());
                    assert!(session.io_threads.lock().expect("threads").is_empty());
                    drop(session);
                }
                tokio::time::timeout(Duration::from_secs(3), async {
                    while exit.borrow().is_none() {
                        exit.changed().await.expect("worker completion");
                    }
                })
                .await
                .expect("child reaped and reader joined before startup flood");
            }
        }

        #[test]
        fn conpty_stalled_writer_close_and_switch_keep_runtime_responsive() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                // A real ConPTY child that never consumes input. No installed WSL required.
                let exe =
                    std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("Windows"))
                        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
                let session = tokio::task::spawn_blocking(move || {
                    spawn_session(
                        &exe,
                        vec![
                            "-NoProfile".into(),
                            "-Command".into(),
                            "Start-Sleep -Seconds 30".into(),
                        ],
                        80,
                        24,
                        || Ok(()),
                    )
                })
                .await
                .expect("spawn worker")
                .expect("ConPTY");
                let session = Arc::new(session);
                // Allow ConPTY's startup cursor query to be answered before flooding its
                // input; otherwise the fixture stalls startup, not the user writer.
                tokio::time::sleep(Duration::from_millis(500)).await;
                let writer = session.writer.clone();
                let threads = session.io_threads.clone();
                let entered = Arc::new(AtomicBool::new(false));
                let flag = entered.clone();
                let write = tokio::task::spawn_blocking(move || {
                    let _registration = IoRegistration::new(threads).expect("thread registration");
                    let mut lock = writer.lock().expect("writer");
                    flag.store(true, Ordering::SeqCst);
                    lock.as_mut()
                        .expect("writer")
                        .write_all(&vec![b'x'; 16 * 1024 * 1024])
                });
                tokio::time::timeout(Duration::from_secs(3), async {
                    while !entered.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                })
                .await
                .expect("write started");
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert!(
                    !write.is_finished(),
                    "fixture must actually stall ConPTY WriteFile"
                );
                // Simulate a dropped write waiter; blocking work still owns writer lock.
                let heartbeat = tokio::spawn(async {
                    let start = std::time::Instant::now();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    assert!(
                        start.elapsed() < Duration::from_millis(250),
                        "hub heartbeat stalled behind PTY cleanup"
                    );
                });
                let close = tokio::time::timeout(Duration::from_secs(5), session.close());
                let (close, heartbeat) = tokio::join!(close, heartbeat);
                close.expect("close bounded").expect("close");
                heartbeat.expect("heartbeat");
                tokio::time::timeout(Duration::from_secs(2), write)
                    .await
                    .expect("write drained")
                    .expect("worker")
                    .expect_err("pipe interrupted");
                assert!(
                    session.exit.borrow().is_some(),
                    "child reaped and readers joined"
                );
                assert!(session.writer.lock().expect("writer").is_none());
                assert!(session.master.lock().expect("master").is_none());
                // Engine switch/drop cannot destroy ConPTY or wait for a lock on hub.
                let start = std::time::Instant::now();
                drop(session);
                assert!(start.elapsed() < Duration::from_millis(100));
            });
        }

        #[tokio::test]
        async fn conpty_dispatch_wins_dropped_session_reaped_offhub() {
            let exe = std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("Windows"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let session = tokio::task::spawn_blocking(move || {
                spawn_session(
                    &exe,
                    vec![
                        "-NoProfile".into(),
                        "-Command".into(),
                        "Start-Sleep -Seconds 30".into(),
                    ],
                    80,
                    24,
                    || Ok(()),
                )
            })
            .await
            .expect("spawn")
            .expect("ConPTY");
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut exit = session.exit.clone();
            let writer = session.writer.clone();
            let threads = session.io_threads.clone();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let write = tokio::task::spawn_blocking(move || {
                let _registration = IoRegistration::new(threads).expect("thread");
                let mut w = writer.lock().expect("writer");
                let _ = entered.send(());
                w.as_mut()
                    .expect("writer")
                    .write_all(&vec![b'x'; 16 * 1024 * 1024])
            });
            ready.await.expect("writer entered");
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(!write.is_finished(), "ConPTY writer stalled");
            // A dropped join handle does not stop blocking WriteFile. Session Drop must.
            drop(write);
            let start = std::time::Instant::now();
            drop(session);
            assert!(
                start.elapsed() < Duration::from_millis(100),
                "drop performs no blocking cleanup"
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while exit.borrow().is_none() {
                    exit.changed().await.expect("cleanup worker completion");
                }
            })
            .await
            .expect("child reaped, PTY and reader cleanup completed after drop");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn eng_134_cancel_queued_blocking_spawn_zero_admissions() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let (release, blocked) = std::sync::mpsc::channel();
            let (ready, entered) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = ready.send(());
                blocked.recv().expect("release");
            });
            entered.await.expect("blocking pool occupied");
            let calls = Arc::new(AtomicUsize::new(0));
            let c = calls.clone();
            let exe = std::path::PathBuf::from(std::env::var_os("ComSpec").expect("cmd"));
            let task = tokio::spawn(spawn_checked(
                exe,
                vec!["/c".into(), "exit".into()],
                80,
                24,
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            ));
            tokio::task::yield_now().await;
            task.abort();
            let _ = task.await;
            release.send(()).expect("release");
            blocker.await.expect("blocker");
            // Barrier runs after the cancelled queued closure on the one-worker pool.
            tokio::task::spawn_blocking(|| ()).await.expect("barrier");
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        });
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn eng_134_cancel_pending_targetguard_zero_spawn_admissions() {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (dropped, finished) = tokio::sync::oneshot::channel();
        struct GuardDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for GuardDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let exe = std::path::PathBuf::from(std::env::var_os("ComSpec").expect("cmd"));
        let task = tokio::spawn(spawn_checked(
            exe,
            vec!["/c".into(), "exit".into()],
            80,
            24,
            async move {
                let _guard = GuardDrop(Some(dropped));
                let _ = entered.send(());
                futures::future::pending::<EngineResult<()>>().await
            },
        ));
        ready.await.expect("guard entered");
        task.abort();
        let _ = task.await;
        tokio::time::timeout(std::time::Duration::from_secs(2), finished)
            .await
            .expect("cancel wakes guard worker")
            .expect("guard dropped");
    }

    #[cfg(windows)]
    #[test]
    fn eng_134_admission_cancel_race_single_winner() {
        for _ in 0..100 {
            let admission = SpawnAdmission::new();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let (a, b) = (admission.clone(), barrier.clone());
            let cancel = std::thread::spawn(move || {
                b.wait();
                a.cancel();
            });
            barrier.wait();
            let admitted = admission.admit();
            cancel.join().expect("cancel");
            assert_eq!(
                admission.state.load(std::sync::atomic::Ordering::Acquire),
                if admitted.is_ok() { 1 } else { 2 }
            );
            assert_eq!(admission.admit(), Err(dk_core::EngineError::Cancelled));
        }
    }

    #[test]
    fn exec_argv() {
        let req = ExecRequest {
            cmd: vec!["echo".into(), "hi".into(), "--x".into()],
            tty: true,
            env: vec!["TERM=xterm-256color".into()],
            user: Some("root".into()),
            working_dir: Some("/tmp".into()),
            cols: 80,
            rows: 24,
        };
        assert_eq!(
            exec_args("web", &req),
            [
                "container",
                "exec",
                "-i",
                "-t",
                "-e",
                "TERM=xterm-256color",
                "-u",
                "root",
                "-w",
                "/tmp",
                "web",
                "echo",
                "hi",
                "--x"
            ]
        );
        let req = ExecRequest {
            cmd: vec![],
            env: vec![],
            ..ExecRequest::default()
        };
        let a = exec_args("web", &req);
        assert_eq!(&a[..5], ["container", "exec", "-i", "-t", "web"]);
        assert_eq!(a[5], "/bin/sh");
    }

    #[test]
    fn dsr_query_is_stripped_once() {
        let c = bytes::Bytes::from_static(b"\x1b[?9001h\x1b[6n\x1b[mX\x1b[6n");
        let r = strip_first_dsr(&c).expect("found");
        assert_eq!(&r[..], b"\x1b[?9001h\x1b[mX\x1b[6n");
        assert!(strip_first_dsr(&bytes::Bytes::from_static(b"hi")).is_none());
    }

    #[test]
    fn exec_validation() {
        assert!(validate_exec(&ExecRequest::default()).is_ok());
        for bad in [
            ExecRequest {
                env: vec!["-x=1".into()],
                ..ExecRequest::default()
            },
            ExecRequest {
                user: Some("--privileged".into()),
                ..ExecRequest::default()
            },
            ExecRequest {
                working_dir: Some(String::new()),
                ..ExecRequest::default()
            },
            ExecRequest {
                cmd: vec!["--rm".into()],
                ..ExecRequest::default()
            },
            ExecRequest {
                cmd: vec!["sh".into(), "a\0b".into()],
                ..ExecRequest::default()
            },
        ] {
            assert!(validate_exec(&bad).is_err(), "{bad:?}");
        }
    }
}
