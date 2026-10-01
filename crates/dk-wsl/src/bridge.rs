//! ACL-restricted named-pipe ⇄ `wsl.exe` stdio bridge (spec 20 §4.3–4.4, ADR-0004, ENG-011,
//! NFR-021).
//!
//! `\\.\pipe\dockering-wsl-<slug>-<rand16hex>` is created with a protected DACL that grants
//! `GENERIC_ALL` to the current user's SID only, rejects remote clients, and claims the name with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`. Each accepted connection spawns one
//! `wsl.exe -d <distro> --exec docker system dial-stdio` (or `socat - UNIX-CONNECT:…`) child and
//! pumps bytes both ways. The child is killed as soon as either side ends.
//!
//! Lifecycle: the bridge is owned by the `DockerEngine` built on it (`keepalive`). Dropping the
//! last `Arc<PipeBridge>` (or calling [`PipeBridge::shutdown`]) stops the listener and kills every
//! child. While the bridge is marked inactive ([`PipeBridge::set_active`]), connections without
//! traffic for 60 s are closed so WSL can idle the distro out (spec 20 §4.4).

use std::time::Duration;

/// Idle timeout for connections of an inactive bridge (spec 20 §4.4).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The distro part of the pipe name: lowercase, `[a-z0-9._-]`, anything else becomes `-`.
pub(crate) fn pipe_slug(distro: &str) -> String {
    let slug = distro
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    if slug.is_empty() {
        "distro".into()
    } else {
        slug
    }
}

/// `\\.\pipe\dockering-wsl-<slug>-<16 hex digits>`; randomised per bridge (NFR-021).
pub(crate) fn pipe_path(distro: &str, random: u64) -> String {
    format!(
        r"\\.\pipe\dockering-wsl-{}-{random:016x}",
        pipe_slug(distro)
    )
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::process::Stdio;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use tokio::sync::Notify;
    use tokio::task::JoinHandle;
    use tokio::time::{Instant, sleep};
    use tokio_util::sync::CancellationToken;

    use super::IDLE_TIMEOUT;
    use crate::discovery::{BridgeTool, validate_distro_name};
    use crate::win32::PipeSecurity;

    const PUMP_BUFFER: usize = 64 * 1024;

    /// State shared by the accept loop and the connection tasks.
    struct Shared {
        distro: String,
        tool: BridgeTool,
        security: PipeSecurity,
        path: String,
        active: AtomicBool,
        /// Wakes a parked accept loop when the bridge becomes active again.
        activated: Notify,
        parked: AtomicBool,
        connections: AtomicUsize,
        /// Last accept, connection end, or `set_active` change (ms since `epoch`).
        last_change_ms: AtomicU64,
        idle_timeout: Duration,
        epoch: Instant,
        cancel: CancellationToken,
    }

    impl Shared {
        fn now_ms(&self) -> u64 {
            u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
        }

        fn touch(&self) {
            self.last_change_ms.store(self.now_ms(), Ordering::Release);
        }

        fn idle_timeout_ms(&self) -> u64 {
            u64::try_from(self.idle_timeout.as_millis()).unwrap_or(u64::MAX)
        }

        fn tick(&self) -> Duration {
            (self.idle_timeout / 4).clamp(Duration::from_millis(50), Duration::from_secs(5))
        }
    }

    /// Decrements the connection count even when a connection task is aborted.
    struct ConnectionGuard(Arc<Shared>);

    impl Drop for ConnectionGuard {
        fn drop(&mut self) {
            self.0.connections.fetch_sub(1, Ordering::AcqRel);
            self.0.touch();
        }
    }

    /// Named-pipe server that bridges to Docker inside a WSL distro. See the module docs.
    pub struct PipeBridge {
        shared: Arc<Shared>,
        accept_task: JoinHandle<()>,
    }

    impl std::fmt::Debug for PipeBridge {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PipeBridge")
                .field("distro", &self.shared.distro)
                .field("path", &self.shared.path)
                .field("tool", &self.shared.tool)
                .finish_non_exhaustive()
        }
    }

    impl PipeBridge {
        /// Creates the pipe and starts accepting. Must be called inside a tokio runtime. The
        /// distro must be running (ENG-106); the bridge never checks or boots it.
        pub async fn start(distro: impl Into<String>, tool: BridgeTool) -> io::Result<Arc<Self>> {
            Self::start_with_idle_timeout(distro.into(), tool, IDLE_TIMEOUT)
        }

        pub(crate) fn start_with_idle_timeout(
            distro: String,
            tool: BridgeTool,
            idle_timeout: Duration,
        ) -> io::Result<Arc<Self>> {
            if !validate_distro_name(&distro) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid WSL distro name",
                ));
            }
            let security = PipeSecurity::current_user()?;
            let path = super::pipe_path(&distro, rand::random::<u64>());
            let first = security.create_pipe(&path, true)?;
            let shared = Arc::new(Shared {
                distro,
                tool,
                security,
                path,
                active: AtomicBool::new(true),
                activated: Notify::new(),
                parked: AtomicBool::new(false),
                connections: AtomicUsize::new(0),
                last_change_ms: AtomicU64::new(0),
                idle_timeout,
                epoch: Instant::now(),
                cancel: CancellationToken::new(),
            });
            let accept_task = tokio::spawn(accept_loop(first, Arc::clone(&shared)));
            tracing::debug!(path = %shared.path, distro = %shared.distro, tool = ?tool, "WSL bridge listening");
            Ok(Arc::new(Self {
                shared,
                accept_task,
            }))
        }

        /// The pipe path to hand to the Docker client.
        pub fn path(&self) -> &str {
            &self.shared.path
        }

        pub fn distro(&self) -> &str {
            &self.shared.distro
        }

        pub fn tool(&self) -> BridgeTool {
            self.shared.tool
        }

        /// Open client connections, one `wsl.exe` child each.
        pub fn active_connections(&self) -> usize {
            self.shared.connections.load(Ordering::Acquire)
        }

        /// Marks whether this bridge belongs to the active engine (default: active). While
        /// inactive, connections with no traffic for 60 s are closed, and after 60 s without any
        /// connection the listener is parked (pipe name released) until the bridge is marked
        /// active again (spec 20 §4.4).
        pub fn set_active(&self, active: bool) {
            let was = self.shared.active.swap(active, Ordering::AcqRel);
            if was != active {
                self.shared.touch();
            }
            if active {
                self.shared.activated.notify_one();
            }
        }

        /// Whether the listener is currently parked by the idle policy.
        pub fn is_parked(&self) -> bool {
            self.shared.parked.load(Ordering::Acquire)
        }

        pub fn is_active(&self) -> bool {
            self.shared.active.load(Ordering::Acquire)
        }

        /// Stops accepting and kills every child. Idempotent.
        pub fn shutdown(&self) {
            self.shared.cancel.cancel();
        }

        pub fn is_shut_down(&self) -> bool {
            self.shared.cancel.is_cancelled()
        }
    }

    impl Drop for PipeBridge {
        fn drop(&mut self) {
            self.shared.cancel.cancel();
            self.accept_task.abort();
        }
    }

    async fn accept_loop(first: NamedPipeServer, shared: Arc<Shared>) {
        let mut server = Some(first);
        loop {
            let Some(listening) = server.as_mut() else {
                // Parked: wait until the bridge is active again, then reclaim the same name.
                tokio::select! {
                    () = shared.cancel.cancelled() => break,
                    () = shared.activated.notified() => {}
                }
                if !shared.active.load(Ordering::Acquire) {
                    continue;
                }
                match shared.security.create_pipe(&shared.path, true) {
                    Ok(next) => {
                        tracing::debug!(path = %shared.path, "WSL bridge listener resumed");
                        shared.parked.store(false, Ordering::Release);
                        shared.touch();
                        server = Some(next);
                    }
                    Err(error) => {
                        tracing::warn!(%error, path = %shared.path, "WSL bridge can't reclaim its pipe; stopping");
                        shared.cancel.cancel();
                        break;
                    }
                }
                continue;
            };

            let connected = tokio::select! {
                () = shared.cancel.cancelled() => break,
                () = listener_idle(&shared) => {
                    tracing::debug!(path = %shared.path, "WSL bridge inactive and unused; parking listener");
                    server = None;
                    shared.parked.store(true, Ordering::Release);
                    continue;
                }
                connected = listening.connect() => connected,
            };

            // Create the next listening instance first so clients never see a gap.
            let next = match shared.security.create_pipe(&shared.path, false) {
                Ok(next) => next,
                Err(error) => {
                    tracing::warn!(%error, path = %shared.path, "WSL bridge can't create a pipe instance; stopping");
                    shared.cancel.cancel();
                    break;
                }
            };
            let Some(instance) = server.replace(next) else {
                break;
            };

            if let Err(error) = connected {
                // e.g. ERROR_NO_DATA: the client went away before the connect completed.
                tracing::debug!(%error, "WSL bridge pipe accept failed; continuing");
                continue;
            }

            shared.connections.fetch_add(1, Ordering::AcqRel);
            shared.touch();
            let guard = ConnectionGuard(Arc::clone(&shared));
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                if let Err(error) = serve_connection(instance, &shared).await {
                    tracing::debug!(%error, distro = %shared.distro, "WSL bridge connection ended with an error");
                }
                drop(guard);
            });
        }
    }

    /// Completes once the bridge is inactive with no connections and nothing changed for the
    /// idle timeout. Never completes while active.
    async fn listener_idle(shared: &Shared) {
        loop {
            sleep(shared.tick()).await;
            if shared.active.load(Ordering::Acquire)
                || shared.connections.load(Ordering::Acquire) != 0
            {
                continue;
            }
            let idle = shared
                .now_ms()
                .saturating_sub(shared.last_change_ms.load(Ordering::Acquire));
            if idle >= shared.idle_timeout_ms() {
                return;
            }
        }
    }

    fn child_command(distro: &str, tool: BridgeTool) -> tokio::process::Command {
        let mut command = crate::runner::command();
        command.args(["-d", distro, "--exec"]);
        match tool {
            BridgeTool::Docker => command.args(["docker", "system", "dial-stdio"]),
            BridgeTool::Socat => command.args(["socat", "-", "UNIX-CONNECT:/var/run/docker.sock"]),
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    async fn serve_connection(pipe: NamedPipeServer, shared: &Shared) -> io::Result<()> {
        let mut child = child_command(&shared.distro, shared.tool).spawn()?;
        let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill().await;
            return Err(io::Error::other("wsl.exe child stdio unavailable"));
        };
        // Bounded stderr capture for diagnostics only; never forwarded to the client.
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = (&mut stderr).take(4096).read_to_end(&mut bytes).await;
            bytes
        });

        let last_activity = AtomicU64::new(shared.now_ms());
        let (mut pipe_read, mut pipe_write) = tokio::io::split(pipe);

        let result = tokio::select! {
            result = pump(&mut pipe_read, &mut stdin, &last_activity, shared) => result,
            result = pump(&mut stdout, &mut pipe_write, &last_activity, shared) => result,
            () = idle_watch(&last_activity, shared) => {
                tracing::debug!(distro = %shared.distro, "closing idle WSL bridge connection (engine inactive)");
                Ok(())
            }
            () = shared.cancel.cancelled() => Ok(()),
        };

        // Either side ended: kill the child first (this also closes its stdout), then the pipe.
        let _ = child.kill().await;
        drop(stdin);
        let _ = pipe_write.shutdown().await;
        drop((pipe_read, pipe_write));
        if let Ok(stderr) = stderr_task.await {
            let stderr = String::from_utf8_lossy(&stderr);
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                tracing::debug!(stderr, distro = %shared.distro, "WSL bridge child stderr");
            }
        }
        result
    }

    /// Copies until EOF, flushing after each chunk and recording activity.
    async fn pump<R, W>(
        reader: &mut R,
        writer: &mut W,
        last_activity: &AtomicU64,
        shared: &Shared,
    ) -> io::Result<()>
    where
        R: AsyncRead + Unpin + ?Sized,
        W: AsyncWrite + Unpin + ?Sized,
    {
        let mut buffer = vec![0u8; PUMP_BUFFER];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(());
            }
            last_activity.store(shared.now_ms(), Ordering::Release);
            writer.write_all(&buffer[..read]).await?;
            writer.flush().await?;
        }
    }

    /// Completes once the bridge is inactive and this connection had no traffic for the idle
    /// timeout. Never completes while the bridge is active.
    async fn idle_watch(last_activity: &AtomicU64, shared: &Shared) {
        let timeout_ms = shared.idle_timeout_ms();
        loop {
            sleep(shared.tick()).await;
            if shared.active.load(Ordering::Acquire) {
                continue;
            }
            let idle_ms = shared
                .now_ms()
                .saturating_sub(last_activity.load(Ordering::Acquire));
            if idle_ms >= timeout_ms {
                return;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::os::windows::io::AsRawHandle;
        use std::time::Duration;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::ClientOptions;

        use super::PipeBridge;
        use crate::discovery::BridgeTool;

        const LIVE_DISTRO: &str = "Ubuntu-22.04";

        async fn ping(path: &str) -> String {
            let mut client = ClientOptions::new().open(path).expect("open bridge pipe");
            client
                .write_all(b"GET /_ping HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n")
                .await
                .expect("write request");
            let mut response = Vec::new();
            tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut response))
                .await
                .expect("response within 15 s")
                .expect("read response");
            String::from_utf8_lossy(&response).into_owned()
        }

        #[test]
        fn nfr_021_rejects_invalid_distro_names() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let error = PipeBridge::start("-d", BridgeTool::Docker)
                    .await
                    .expect_err("invalid name");
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            });
        }

        /// Starting a bridge spawns nothing (no `wsl.exe`), claims a random name per bridge.
        #[tokio::test]
        async fn nfr_021_bridge_names_are_random_per_bridge() {
            let a = PipeBridge::start(LIVE_DISTRO, BridgeTool::Docker)
                .await
                .expect("start bridge");
            let b = PipeBridge::start(LIVE_DISTRO, BridgeTool::Docker)
                .await
                .expect("second bridge");
            assert!(
                a.path()
                    .starts_with(r"\\.\pipe\dockering-wsl-ubuntu-22.04-")
            );
            assert_ne!(a.path(), b.path());
            assert_eq!(a.active_connections(), 0);
        }

        /// The live bridge pipe's DACL is protected and grants access to the current user only.
        /// Opening a client spawns a `wsl.exe` child, hence `#[ignore]` (would boot a stopped
        /// distro).
        #[tokio::test]
        #[ignore = "live: needs running WSL distro Ubuntu-22.04"]
        async fn nfr_021_live_bridge_pipe_dacl_is_current_user_only() {
            let bridge = PipeBridge::start(LIVE_DISTRO, BridgeTool::Docker)
                .await
                .expect("start bridge");
            // GENERIC_READ includes READ_CONTROL, so the client handle can read the DACL.
            let client = ClientOptions::new().open(bridge.path()).expect("open pipe");
            let dacl = crate::win32::object_dacl(client.as_raw_handle()).expect("read DACL");
            let me = crate::win32::current_user_sid().expect("current SID");
            eprintln!("bridge {} DACL: {dacl:?} (me = {me})", bridge.path());
            assert!(dacl.protected, "DACL must be protected (no inheritance)");
            assert_eq!(dacl.allowed_sids, vec![me]);
            assert_eq!(dacl.other_aces, 0);
        }

        /// Raw `GET /_ping` through the bridge into the running distro's Docker (ENG-011).
        #[tokio::test]
        #[ignore = "live: needs running WSL distro Ubuntu-22.04 with Docker"]
        async fn eng_011_live_bridge_ping_ubuntu() {
            let bridge = PipeBridge::start(LIVE_DISTRO, BridgeTool::Docker)
                .await
                .expect("start bridge");
            let started = std::time::Instant::now();
            let response = ping(bridge.path()).await;
            eprintln!(
                "bridge {} → {:?} in {:?}",
                bridge.path(),
                response.lines().next(),
                started.elapsed()
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert!(response.ends_with("OK"), "{response}");

            // Two concurrent connections → two children; both closed afterwards.
            let (a, b) = tokio::join!(ping(bridge.path()), ping(bridge.path()));
            assert!(a.ends_with("OK") && b.ends_with("OK"));
            for _ in 0..100 {
                if bridge.active_connections() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(bridge.active_connections(), 0);
        }

        /// The socat fallback, when the distro has socat installed.
        #[tokio::test]
        #[ignore = "live: needs running WSL distro Ubuntu-22.04 with socat"]
        async fn eng_011_live_bridge_ping_ubuntu_socat() {
            let bridge = PipeBridge::start(LIVE_DISTRO, BridgeTool::Socat)
                .await
                .expect("start bridge");
            let response = ping(bridge.path()).await;
            assert!(response.ends_with("OK"), "{response}");
        }

        /// An inactive bridge closes connections without traffic after the idle timeout; an
        /// active one keeps them (spec 20 §4.4).
        #[tokio::test]
        #[ignore = "live: needs running WSL distro Ubuntu-22.04 with Docker"]
        async fn eng_011_live_idle_connection_closed_only_while_inactive() {
            let bridge = PipeBridge::start_with_idle_timeout(
                LIVE_DISTRO.into(),
                BridgeTool::Docker,
                Duration::from_millis(400),
            )
            .expect("start bridge");
            let mut client = ClientOptions::new().open(bridge.path()).expect("open");
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert_eq!(
                bridge.active_connections(),
                1,
                "active bridge keeps idle connections"
            );

            bridge.set_active(false);
            let mut buffer = [0u8; 16];
            let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut buffer))
                .await
                .expect("closed within 5 s");
            assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
            for _ in 0..100 {
                if bridge.active_connections() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(bridge.active_connections(), 0);
        }

        /// Dropping the last `Arc` stops the listener: the name disappears.
        #[tokio::test]
        async fn eng_011_drop_stops_the_listener() {
            let bridge = PipeBridge::start(LIVE_DISTRO, BridgeTool::Docker)
                .await
                .expect("start bridge");
            let path = bridge.path().to_owned();
            drop(bridge);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let error = ClientOptions::new().open(&path).expect_err("pipe gone");
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
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

    /// Non-Windows stand-in: `start` always fails.
    #[derive(Debug)]
    pub struct PipeBridge {
        path: String,
        distro: String,
        tool: BridgeTool,
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
        pub fn distro(&self) -> &str {
            &self.distro
        }
        pub fn tool(&self) -> BridgeTool {
            self.tool
        }
        pub fn active_connections(&self) -> usize {
            0
        }
        pub fn set_active(&self, _active: bool) {}
        pub fn is_active(&self) -> bool {
            false
        }
        pub fn shutdown(&self) {}
        pub fn is_shut_down(&self) -> bool {
            true
        }
    }
}

#[cfg(not(windows))]
pub use platform_stub::PipeBridge;

#[cfg(test)]
mod tests {
    use super::{pipe_path, pipe_slug};

    #[test]
    fn nfr_021_pipe_name_component_is_sanitized() {
        assert_eq!(pipe_slug("Ubuntu-22.04"), "ubuntu-22.04");
        assert_eq!(pipe_slug("bad name/pipe\\x"), "bad-name-pipe-x");
        assert_eq!(pipe_slug(""), "distro");
        assert_eq!(pipe_slug(&"A".repeat(300)).len(), 64);
    }

    #[test]
    fn nfr_021_pipe_path_is_randomised_and_well_formed() {
        assert_eq!(
            pipe_path("Ubuntu-22.04", 0xab),
            r"\\.\pipe\dockering-wsl-ubuntu-22.04-00000000000000ab"
        );
        let a = pipe_path("Debian", rand::random());
        let b = pipe_path("Debian", rand::random());
        assert_ne!(a, b);
        let suffix = a.rsplit('-').next().expect("suffix");
        assert_eq!(suffix.len(), 16);
        assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
