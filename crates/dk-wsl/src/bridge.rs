//! ACL-restricted named-pipe ⇄ `wsl.exe` stdio bridge (ENG-011, NFR-021).

#[cfg(windows)]
mod platform {
    use std::io;
    use std::process::Stdio;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use tokio::task::JoinHandle;
    use tokio::time::{Instant, sleep};
    use tokio_util::sync::CancellationToken;

    use crate::discovery::{BridgeTool, validate_distro_name};

    const IDLE_SHUTDOWN: Duration = Duration::from_secs(60);

    /// One per connected Docker HTTP transport; decrements the count even when a task is aborted.
    struct ConnectionGuard(Arc<AtomicUsize>);

    impl Drop for ConnectionGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub struct PipeBridge {
        path: String,
        active_connections: Arc<AtomicUsize>,
        active: Arc<AtomicBool>,
        cancel: CancellationToken,
        accept_task: JoinHandle<()>,
    }

    impl PipeBridge {
        pub async fn start(distro: impl Into<String>, tool: BridgeTool) -> io::Result<Arc<Self>> {
            let distro = distro.into();
            if !validate_distro_name(&distro) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid WSL distro name",
                ));
            }
            let path = random_pipe_path(&distro);
            let server = crate::win32::create_pipe(&path, true)?;
            let active_connections = Arc::new(AtomicUsize::new(0));
            let active = Arc::new(AtomicBool::new(true));
            let cancel = CancellationToken::new();
            let accept_task = tokio::spawn(accept_loop(
                server,
                path.clone(),
                distro,
                tool,
                Arc::clone(&active_connections),
                Arc::clone(&active),
                cancel.clone(),
            ));
            Ok(Arc::new(Self {
                path,
                active_connections,
                active,
                cancel,
                accept_task,
            }))
        }

        pub fn path(&self) -> &str {
            &self.path
        }

        pub fn active_connections(&self) -> usize {
            self.active_connections.load(Ordering::Acquire)
        }

        /// Mark whether this bridge belongs to the active engine. An inactive, connection-free
        /// bridge stops accepting after 60 seconds (spec 20 §4.4).
        pub fn set_active(&self, active: bool) {
            self.active.store(active, Ordering::Release);
        }

        pub fn shutdown(&self) {
            self.cancel.cancel();
        }
    }

    impl Drop for PipeBridge {
        fn drop(&mut self) {
            self.cancel.cancel();
            self.accept_task.abort();
        }
    }

    async fn accept_loop(
        mut server: NamedPipeServer,
        path: String,
        distro: String,
        tool: BridgeTool,
        active_connections: Arc<AtomicUsize>,
        active: Arc<AtomicBool>,
        cancel: CancellationToken,
    ) {
        let mut idle_since = None;
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = sleep(Duration::from_secs(1)) => {
                    if active.load(Ordering::Acquire)
                        || active_connections.load(Ordering::Acquire) != 0
                    {
                        idle_since = None;
                    } else {
                        let since = idle_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= IDLE_SHUTDOWN {
                            cancel.cancel();
                            break;
                        }
                    }
                }
                connected = server.connect() => {
                    if let Err(error) = connected {
                        if !cancel.is_cancelled() {
                            tracing::debug!(%error, "WSL bridge pipe accept failed");
                        }
                        break;
                    }

                    // Create the next listening instance before serving this connection so clients
                    // never race a gap in the accept loop.
                    let next = match crate::win32::create_pipe(&path, false) {
                        Ok(next) => next,
                        Err(error) => {
                            tracing::debug!(%error, "WSL bridge couldn't create its next pipe instance");
                            break;
                        }
                    };
                    let connected = std::mem::replace(&mut server, next);
                    active_connections.fetch_add(1, Ordering::AcqRel);
                    let guard = ConnectionGuard(Arc::clone(&active_connections));
                    let distro = distro.clone();
                    let child_cancel = cancel.clone();
                    tokio::spawn(async move {
                        if let Err(error) = serve_connection(connected, &distro, tool, child_cancel).await {
                            tracing::debug!(%error, distro = %distro, "WSL bridge connection ended with an error");
                        }
                        drop(guard);
                    });
                }
            }
        }
    }

    async fn serve_connection(
        pipe: NamedPipeServer,
        distro: &str,
        tool: BridgeTool,
        cancel: CancellationToken,
    ) -> io::Result<()> {
        let mut command = crate::runner::command();
        command.args(["-d", distro, "--exec"]);
        match tool {
            BridgeTool::Docker => {
                command.args(["docker", "system", "dial-stdio"]);
            }
            BridgeTool::Socat => {
                command.args(["socat", "-", "UNIX-CONNECT:/var/run/docker.sock"]);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("wsl.exe child stdin unavailable"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("wsl.exe child stdout unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("wsl.exe child stderr unavailable"))?;
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            bytes
        });
        let (mut pipe_read, mut pipe_write) = tokio::io::split(pipe);

        let pump_result = tokio::select! {
            result = tokio::io::copy(&mut pipe_read, &mut stdin) => result.map(|_| ()),
            result = tokio::io::copy(&mut stdout, &mut pipe_write) => result.map(|_| ()),
            () = cancel.cancelled() => Ok(()),
        };

        let _ = stdin.shutdown().await;
        let _ = pipe_write.shutdown().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        if let Ok(stderr) = stderr_task.await {
            let stderr = String::from_utf8_lossy(&stderr);
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                tracing::debug!(stderr, distro, "WSL bridge child stderr");
            }
        }
        pump_result
    }

    pub(super) fn sanitized_distro(distro: &str) -> String {
        let sanitized = distro
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        if sanitized.is_empty() {
            "distro".into()
        } else {
            sanitized
        }
    }

    fn random_pipe_path(distro: &str) -> String {
        format!(
            r"\\.\pipe\dockering-wsl-{}-{:016x}",
            sanitized_distro(distro),
            rand::random::<u64>()
        )
    }

    #[cfg(test)]
    mod tests {
        use super::sanitized_distro;

        #[test]
        fn nfr_021_pipe_name_component_is_sanitized() {
            assert_eq!(sanitized_distro("Ubuntu-22.04"), "ubuntu-22.04");
            assert_eq!(sanitized_distro("bad name/pipe"), "bad-name-pipe");
        }
    }
}

#[cfg(windows)]
pub use platform::PipeBridge;

#[cfg(not(windows))]
mod platform_stub {
    use std::io;
    use std::sync::Arc;

    use crate::discovery::BridgeTool;

    pub struct PipeBridge {
        path: String,
    }

    impl PipeBridge {
        pub async fn start(_distro: impl Into<String>, _tool: BridgeTool) -> io::Result<Arc<Self>> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "WSL is only available on Windows",
            ))
        }
        pub fn path(&self) -> &str {
            &self.path
        }
        pub fn active_connections(&self) -> usize {
            0
        }
        pub fn set_active(&self, _active: bool) {}
        pub fn shutdown(&self) {}
    }
}

#[cfg(not(windows))]
pub use platform_stub::PipeBridge;
