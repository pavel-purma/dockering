//! Interactive exec through ConPTY (spec 21 §5, WSLC CLI fallback): `portable-pty` runs
//! `wslc.exe [--session s] container exec -i -t [-e K=V] [-u U] [-w D] <id> <cmd…>` in a
//! pseudo console. Resize goes through `MasterPty::resize`; the exit code is the child's.
//!
//! Blocking PTY I/O never runs on the async runtime: the reader is a dedicated std thread,
//! writes and resizes go through `spawn_blocking`, and `wait` blocks on its own thread.

use dk_core::{EngineResult, ExecRequest};

/// argv after `wslc.exe` (and the global `--session`): `container exec -i -t … <id> <cmd…>`.
/// Values were validated by the caller.
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

    pub(crate) struct ConPtySession {
        master: SharedMaster,
        writer: SharedWriter,
        killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
        output: Option<mpsc::Receiver<EngineResult<Bytes>>>,
        exit: watch::Receiver<Option<Option<i64>>>,
    }

    /// Spawn `exe argv…` inside a new pseudo console of `cols`×`rows`.
    pub(crate) fn spawn(
        exe: &Path,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> EngineResult<Box<dyn TerminalSession>> {
        let pty = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| pty_err("open", e))?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.args(&argv);
        cmd.env("NO_COLOR", "1");
        let mut child = pty.slave.spawn_command(cmd).map_err(|e| {
            crate::cli::runner::spawn_error(exe, &std::io::Error::other(e.to_string()))
        })?;
        // The slave handle must be closed in our process so EOF propagates when the child
        // exits.
        drop(pty.slave);
        let master = pty.master;
        let mut reader = master
            .try_clone_reader()
            .map_err(|e| pty_err("reader", e))?;
        let writer = master.take_writer().map_err(|e| pty_err("writer", e))?;
        let killer = child.clone_killer();

        let (mut tx, rx) = mpsc::channel::<EngineResult<Bytes>>(64);
        std::thread::Builder::new()
            .name("wslc-exec-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = Bytes::copy_from_slice(&buf[..n]);
                            if futures::executor::block_on(futures::SinkExt::send(
                                &mut tx,
                                Ok(chunk),
                            ))
                            .is_err()
                            {
                                break; // output stream dropped
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break, // pipe closed (process exited / closed)
                    }
                }
            })
            .map_err(|e| pty_err("reader thread", e))?;

        let master: SharedMaster = Arc::new(Mutex::new(Some(master)));
        let writer: SharedWriter = Arc::new(Mutex::new(Some(writer)));
        let (exit_tx, exit_rx) = watch::channel(None);
        let (master_w, writer_w) = (master.clone(), writer.clone());
        std::thread::Builder::new()
            .name("wslc-exec-wait".into())
            .spawn(move || {
                let code = child.wait().ok().map(|s| i64::from(s.exit_code()));
                // ConPTY keeps the output pipe open until the pseudo console is closed:
                // close it (writer + master) so the reader thread sees EOF.
                if let Ok(mut w) = writer_w.lock() {
                    w.take();
                }
                if let Ok(mut m) = master_w.lock() {
                    m.take();
                }
                let _ = exit_tx.send(Some(code));
            })
            .map_err(|e| pty_err("wait thread", e))?;

        Ok(Box::new(ConPtySession {
            master,
            writer,
            killer: Mutex::new(killer),
            output: Some(rx),
            exit: exit_rx,
        }))
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
            let writer = self.writer.clone();
            tokio::task::spawn_blocking(move || {
                let mut g = writer
                    .lock()
                    .map_err(|_| EngineError::protocol("PTY writer poisoned"))?;
                let w = g.as_mut().ok_or(EngineError::Cancelled)?;
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
            if let Ok(mut w) = self.writer.lock() {
                w.take();
            }
            if self.exit.borrow().is_none()
                && let Ok(mut k) = self.killer.lock()
            {
                let _ = k.kill();
            }
            Ok(())
        }
    }

    impl Drop for ConPtySession {
        fn drop(&mut self) {
            if self.exit.borrow().is_none()
                && let Ok(mut k) = self.killer.lock()
            {
                let _ = k.kill();
            }
        }
    }

    // Keep the trait objects' auto traits honest: the session crosses threads.
    const _: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<ConPtySession>();
    };
}

#[cfg(test)]
mod tests {
    use super::*;

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
