//! `WslDistroFactory`: WSL distros running Docker Engine (spec 20 §4, ENG-007/011/012/020/106/107).
//!
//! Engines are `DockerEngine`s reached through a [`crate::PipeBridge`] (default, ENG-011) or
//! over TCP on `127.0.0.1` (opt-in, ENG-012). Background checks never boot a stopped distro
//! (ENG-106): only [`EngineFactory::start`] (the user's *Start & connect* / *Start Docker*) runs
//! a command in a stopped distro or starts `dockerd` (ENG-149).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dk_core::engine::preference;
use dk_core::{
    ConfigField, ConfigFieldKind, DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema,
    EngineEndpoint, EngineError, EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult,
    EngineState, ProbeResult, StartPrompt, WslMode,
};

use crate::discovery::{self, BridgeTool, ProbeClassification};
use crate::runner::{self, RealWsl, Wsl};
use crate::setup::{self, SetupEnv};

/// Hint for a stopped distro (ENG-106).
pub(crate) const STOPPED_HINT: &str = "Distro stopped — use Start & connect";
const NOT_WINDOWS: &str = "WSL is only available on Windows";

/// Reason and hint of the error that tells the hub "Docker isn't started yet" (ENG-146/148).
const NOT_RUNNING_HINT: &str = "Use Start Docker to start it";
/// Poll interval and overall limit for `dockerd` to answer after a start (ENG-149).
const READY_POLL: Duration = Duration::from_millis(250);
const READY_TIMEOUT: Duration = Duration::from_secs(30);

const START_DOCKER_BODY: &str = "Start Docker to see its containers and images.";
const START_DOCKER_LABEL: &str = "Start Docker";
const START_FAILED_HINT: &str = "Check the Docker service inside the distro (`sudo systemctl status docker` or `sudo service docker status`)";

/// Discovers WSL distros and connects to the Docker Engine inside them.
#[derive(Debug)]
pub struct WslDistroFactory {
    wsl: Arc<dyn Wsl>,
    /// Lowercase distros last seen as installed-not-running: `start_prompt` is synchronous.
    not_running: Mutex<HashSet<String>>,
    ready_poll: Duration,
    ready_timeout: Duration,
    /// ENG-150: `None` where setup can't exist (not Windows, no `%LOCALAPPDATA%`).
    setup: Option<Arc<SetupEnv>>,
}

impl Default for WslDistroFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl WslDistroFactory {
    pub fn new() -> Self {
        let wsl: Arc<dyn Wsl> = Arc::new(RealWsl);
        let mut factory = Self::with_wsl(wsl.clone());
        factory.setup = SetupEnv::production(wsl).map(Arc::new);
        factory
    }

    pub(crate) fn with_wsl(wsl: Arc<dyn Wsl>) -> Self {
        Self {
            wsl,
            not_running: Mutex::new(HashSet::new()),
            ready_poll: READY_POLL,
            ready_timeout: READY_TIMEOUT,
            setup: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_setup(mut self, env: SetupEnv) -> Self {
        self.setup = Some(Arc::new(env));
        self
    }

    fn remember(&self, distro: &str, not_running: bool) {
        let mut set = self.not_running.lock().unwrap_or_else(|e| e.into_inner());
        let key = distro.to_ascii_lowercase();
        if not_running {
            set.insert(key);
        } else {
            set.remove(&key);
        }
    }

    fn remembered_not_running(&self, distro: &str) -> bool {
        self.not_running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&distro.to_ascii_lowercase())
    }

    /// Updates the cache from a probe result; a failed probe changes nothing.
    fn note(&self, distro: &str, probe: &Option<Result<ProbeClassification, String>>) {
        match probe {
            Some(Ok(class)) => {
                self.remember(distro, *class == ProbeClassification::InstalledNotRunning);
            }
            Some(Err(_)) => {}
            None => self.remember(distro, false),
        }
    }

    /// `Ready` once Docker answers `GET /_ping` (or is up but not usable by this user, which
    /// `connect` reports with its own hint), `Missing` when there's no `dockerd` to start.
    /// Runs the probe script, so callers check the distro is running first (ENG-106).
    async fn readiness(&self, distro: &str) -> EngineResult<Readiness> {
        let class = discovery::probe(&*self.wsl, distro)
            .await
            .map_err(|error| io_unreachable(&format!("couldn't probe {distro}"), error))?;
        self.remember(distro, class == ProbeClassification::InstalledNotRunning);
        Ok(match class {
            ProbeClassification::Available(tool) => {
                let reply = self.wsl.ping(distro, tool).await.unwrap_or_default();
                if runner::ping_ok(&reply) {
                    Readiness::Ready
                } else {
                    Readiness::NotYet
                }
            }
            ProbeClassification::PermissionDenied | ProbeClassification::SocketNoClient => {
                Readiness::Ready
            }
            ProbeClassification::InstalledNotRunning => Readiness::NotYet,
            ProbeClassification::NoDocker => Readiness::Missing,
        })
    }

    /// ENG-149: start `dockerd` in a running distro, then wait for `GET /_ping`.
    async fn start_docker(&self, distro: &str) -> EngineResult<()> {
        match self.readiness(distro).await? {
            Readiness::Ready => return Ok(()),
            Readiness::Missing => return Err(no_docker_error(distro)),
            Readiness::NotYet => {}
        }
        let systemd = self
            .wsl
            .run(&["-d", distro, "--exec", "test", "-d", SYSTEMD_DIR])
            .await
            .is_ok_and(|output| output.success());
        let service: &[&str] = if systemd {
            &SYSTEMCTL_START
        } else {
            &SERVICE_START
        };
        let mut argv = vec!["-d", distro, "-u", "root", "--exec"];
        argv.extend_from_slice(service);
        let output = self
            .wsl
            .run(&argv)
            .await
            .map_err(|error| start_failed_error(distro, error))?;
        if !output.success() {
            return Err(start_failed_error(distro, output.error_message()));
        }
        let deadline = tokio::time::Instant::now() + self.ready_timeout;
        loop {
            if self.readiness(distro).await? == Readiness::Ready {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(start_failed_error(
                    distro,
                    format!(
                        "Docker didn't answer within {} s",
                        self.ready_timeout.as_secs()
                    ),
                ));
            }
            tokio::time::sleep(self.ready_poll).await;
        }
    }

    async fn discover_from(&self, distros: Vec<discovery::Distro>) -> Vec<DiscoveredEngine> {
        // Probe running distros concurrently; stopped ones are never touched (ENG-106).
        let probes = distros.iter().map(|distro| async move {
            if distro.running {
                Some(
                    discovery::probe(&*self.wsl, &distro.name)
                        .await
                        .map_err(|error| error.to_string()),
                )
            } else {
                None
            }
        });
        let probes = futures::future::join_all(probes).await;
        distros
            .iter()
            .zip(probes)
            .map(|(distro, probe)| {
                self.note(&distro.name, &probe);
                discovered_engine(&distro.name, distro.running, probe)
            })
            .collect()
    }

    async fn bridge_tool(&self, distro: &str) -> EngineResult<BridgeTool> {
        let class = discovery::probe(&*self.wsl, distro)
            .await
            .map_err(|error| io_unreachable(&format!("couldn't probe {distro}"), error))?;
        self.remember(distro, class == ProbeClassification::InstalledNotRunning);
        match class {
            ProbeClassification::Available(tool) => Ok(tool),
            ProbeClassification::PermissionDenied => Err(permission_error(distro)),
            ProbeClassification::NoDocker => Err(no_docker_error(distro)),
            ProbeClassification::SocketNoClient => Err(no_client_error(distro)),
            ProbeClassification::InstalledNotRunning => Err(not_running_error(distro)),
        }
    }

    async fn boot(&self, distro: &str) -> EngineResult<()> {
        let output = self
            .wsl
            .run(&["-d", distro, "--exec", "true"])
            .await
            .map_err(|error| io_unreachable(&format!("couldn't start {distro}"), error))?;
        if output.success() {
            Ok(())
        } else {
            Err(io_unreachable(
                &format!("couldn't start {distro}"),
                output.error_message(),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Ready,
    NotYet,
    Missing,
}

/// Present when systemd is PID 1 (spec 20 §4.2).
const SYSTEMD_DIR: &str = "/run/systemd/system";
/// Fixed in-distro commands, run as root through `--exec` (never a shell). `service` needs its
/// absolute path: the relay's `PATH` lacks `/usr/sbin` (spike W-2.2).
const SYSTEMCTL_START: [&str; 3] = ["systemctl", "start", "docker"];
const SERVICE_START: [&str; 3] = ["/usr/sbin/service", "docker", "start"];

fn invalid_config(message: impl Into<String>) -> EngineError {
    EngineError::Api {
        status: 400,
        message: message.into(),
    }
}

fn permission_error(distro: &str) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("permission denied opening /var/run/docker.sock in {distro}"),
        format!(
            "Add yourself to the docker group inside the distro: `sudo usermod -aG docker $USER`, \
             then run `wsl --terminate {distro}` and reconnect"
        ),
    )
}

fn no_client_error(distro: &str) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("Docker socket found in {distro}, but neither the docker CLI nor socat"),
        "Install socat inside the distro (`sudo apt install socat`) or switch this engine to TCP mode",
    )
}

fn no_docker_error(distro: &str) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("no Docker socket (/var/run/docker.sock) in {distro}"),
        "Start Docker Engine inside the distro (`sudo service docker start`)",
    )
}

/// `connect` against a running distro with Docker installed but not running (ENG-148).
fn not_running_error(distro: &str) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("Docker isn't running in {distro}"),
        NOT_RUNNING_HINT,
    )
}

/// Whether `error` is the one [`not_running_error`] builds. `EngineError` has no variant for it
/// and can't gain one here, so the constant hint is the marker; permission, no-docker and socat
/// errors carry other hints.
pub(crate) fn is_not_running_error(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Unreachable { reason, hint: Some(hint) }
            if hint == NOT_RUNNING_HINT && reason.starts_with("Docker isn't running in ")
    )
}

fn start_failed_error(distro: &str, message: impl std::fmt::Display) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("couldn't start Docker in {distro}: {message}"),
        START_FAILED_HINT,
    )
}

fn stopped_error(distro: &str) -> EngineError {
    EngineError::unreachable_with_hint(format!("WSL distro {distro} is stopped"), STOPPED_HINT)
}

fn io_unreachable(operation: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::unreachable(format!("{operation}: {error}"))
}

/// Validated `(distro, mode)` of a WSL-distro config (NFR-022).
pub(crate) fn distro_endpoint(cfg: &EngineConfig) -> EngineResult<(&str, WslMode)> {
    match &cfg.endpoint {
        EngineEndpoint::WslDistro { distro, mode } => {
            if !discovery::validate_distro_name(distro) {
                return Err(invalid_config(format!(
                    "invalid WSL distro name: \"{}\"",
                    distro.chars().take(80).collect::<String>().escape_default()
                )));
            }
            if matches!(mode, WslMode::Tcp { port: 0 }) {
                return Err(invalid_config("WSL TCP port must be between 1 and 65535"));
            }
            Ok((distro, *mode))
        }
        _ => Err(invalid_config("expected a WSL distro endpoint")),
    }
}

/// `wsl-<lowercase distro>` — stable across runs (ENG-007).
pub(crate) fn engine_id(distro: &str) -> EngineId {
    EngineId::new(format!("wsl-{}", distro.to_ascii_lowercase()))
}

pub(crate) fn discovered_config(distro: &str) -> EngineConfig {
    EngineConfig {
        id: engine_id(distro),
        name: format!("{distro} (WSL)"),
        endpoint: EngineEndpoint::WslDistro {
            distro: distro.to_owned(),
            mode: WslMode::DialStdio,
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
}

/// Maps a discovery probe result onto a `DiscoveredEngine` (spec 20 §4.2 step 6).
pub(crate) fn discovered_engine(
    distro: &str,
    running: bool,
    probe: Option<Result<ProbeClassification, String>>,
) -> DiscoveredEngine {
    let mut discovered = DiscoveredEngine::new(discovered_config(distro), preference::WSL_DISTRO);
    let failed = |error| EngineState::Failed {
        error,
        retry_in_ms: None,
    };
    if !running {
        discovered.initial_state = Some(EngineState::Stopped);
        return discovered;
    }
    match probe {
        None | Some(Ok(ProbeClassification::Available(_))) => {}
        Some(Ok(ProbeClassification::InstalledNotRunning)) => {
            discovered.initial_state = Some(EngineState::Stopped);
        }
        Some(Ok(ProbeClassification::NoDocker)) => discovered.show_only_when_all = true,
        Some(Ok(ProbeClassification::SocketNoClient)) => {
            discovered.initial_state = Some(failed(no_client_error(distro)));
        }
        Some(Ok(ProbeClassification::PermissionDenied)) => {
            discovered.initial_state = Some(failed(permission_error(distro)));
        }
        Some(Err(message)) => {
            discovered.initial_state = Some(failed(io_unreachable(
                &format!("couldn't probe {distro}"),
                message,
            )));
        }
    }
    discovered
}

#[async_trait]
impl EngineFactory for WslDistroFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::WslDistro
    }

    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        matches!(endpoint, EngineEndpoint::WslDistro { .. })
    }

    async fn discover(&self) -> Vec<DiscoveredEngine> {
        match discovery::enumerate().await {
            Ok(distros) => self.discover_from(distros).await,
            Err(error) => {
                tracing::debug!(%error, "WSL distro discovery failed");
                Vec::new()
            }
        }
    }

    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        let (distro, mode) = distro_endpoint(cfg)?;
        if !self.wsl.supported() {
            return Err(EngineError::unreachable(NOT_WINDOWS));
        }
        let running = discovery::is_running(&*self.wsl, distro)
            .await
            .map_err(|error| io_unreachable("couldn't check the WSL distro state", error))?;
        if !running {
            // Never boot implicitly: the user starts it with *Start & connect* (ENG-106).
            return Err(stopped_error(distro));
        }
        match mode {
            WslMode::DialStdio => {
                let tool = self.bridge_tool(distro).await?;
                connect_bridge(&cfg.id, distro, tool).await
            }
            WslMode::Tcp { port } => connect_tcp(&cfg.id, port).await,
        }
    }

    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        vec![EngineConfigSchema {
            kind: EngineKind::WslDistro,
            label: "WSL distro".into(),
            fields: vec![
                ConfigField {
                    key: "distro".into(),
                    label: "Distro".into(),
                    // Schemas are synchronous; discovered distros are listed by the engine list.
                    kind: ConfigFieldKind::Text,
                    required: true,
                    placeholder: Some("Ubuntu-22.04".into()),
                },
                ConfigField {
                    key: "mode".into(),
                    label: "Mode".into(),
                    kind: ConfigFieldKind::Choice {
                        options: vec!["bridge".into(), "tcp".into()],
                    },
                    required: true,
                    placeholder: None,
                },
                ConfigField {
                    key: "port".into(),
                    label: "TCP port".into(),
                    kind: ConfigFieldKind::Port,
                    required: false,
                    placeholder: Some("2375".into()),
                },
            ],
        }]
    }

    /// Running-state check; a stopped distro is never booted (ENG-020, ENG-106) and Docker is
    /// never pinged. Only a running distro last seen as installed-not-running is re-probed
    /// (one `--exec` into the already running distro), so it stays `Stopped` until Docker is
    /// up (ENG-148).
    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        let (distro, _) = match distro_endpoint(cfg) {
            Ok(endpoint) => endpoint,
            Err(error) => return ProbeResult::Unreachable(error),
        };
        if !self.wsl.supported() {
            return ProbeResult::Unreachable(EngineError::unreachable(NOT_WINDOWS));
        }
        match discovery::is_running(&*self.wsl, distro).await {
            Ok(true) if self.remembered_not_running(distro) => {
                match discovery::probe(&*self.wsl, distro).await {
                    Ok(class) => {
                        let stopped = class == ProbeClassification::InstalledNotRunning;
                        self.remember(distro, stopped);
                        if stopped {
                            ProbeResult::Stopped
                        } else {
                            ProbeResult::Reachable
                        }
                    }
                    Err(_) => ProbeResult::Stopped,
                }
            }
            Ok(true) => ProbeResult::Reachable,
            Ok(false) => {
                self.remember(distro, false);
                ProbeResult::Stopped
            }
            Err(error) => ProbeResult::Unreachable(io_unreachable(
                "couldn't check the WSL distro state",
                error,
            )),
        }
    }

    /// *Start Docker* only for a distro last seen as installed-not-running; any other distro
    /// keeps the generic *Start & connect* (ENG-106, ENG-148).
    fn start_prompt(&self, cfg: &EngineConfig) -> Option<StartPrompt> {
        let (distro, _) = distro_endpoint(cfg).ok()?;
        self.remembered_not_running(distro).then(|| StartPrompt {
            title: format!("Docker isn't running in {distro}"),
            body: START_DOCKER_BODY.into(),
            action_label: START_DOCKER_LABEL.into(),
        })
    }

    /// Explicit user action. A stopped distro is booted with `wsl.exe -d <distro> --exec true`
    /// (ENG-106); a running one gets `dockerd` started (ENG-149).
    async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        let (distro, _) = distro_endpoint(cfg)?;
        if !self.wsl.supported() {
            return Err(EngineError::unreachable(NOT_WINDOWS));
        }
        let running = discovery::is_running(&*self.wsl, distro)
            .await
            .map_err(|error| io_unreachable("couldn't check the WSL distro state", error))?;
        if running {
            self.start_docker(distro).await
        } else {
            self.remember(distro, false);
            self.boot(distro).await
        }
    }

    /// ENG-146: true only for the error `connect` returns for installed-not-running Docker in
    /// this config's distro.
    async fn stopped_after(&self, cfg: &EngineConfig, error: &EngineError) -> bool {
        let Ok((distro, _)) = distro_endpoint(cfg) else {
            return false;
        };
        let ours = is_not_running_error(error) && *error == not_running_error(distro);
        if ours {
            self.remember(distro, true);
        }
        ours
    }

    async fn setup_offers(&self) -> Vec<dk_core::SetupOffer> {
        setup::offers(self.setup.as_deref()).await
    }

    fn setup(
        &self,
        offer_id: &str,
        choice_id: &str,
    ) -> dk_core::EngineStream<dk_core::SetupProgress> {
        setup::start(self.setup.clone(), offer_id, choice_id)
    }
}

#[cfg(windows)]
async fn connect_bridge(
    id: &EngineId,
    distro: &str,
    tool: BridgeTool,
) -> EngineResult<Arc<dyn Engine>> {
    use dk_engine_docker::{DockerEngine, DockerEngineOptions, DockerTarget};

    let bridge = crate::PipeBridge::start(distro.to_owned(), tool)
        .await
        .map_err(|error| io_unreachable("couldn't start the WSL bridge pipe", error))?;
    let keepalive: Arc<dyn std::any::Any + Send + Sync> = bridge.clone();
    let engine = DockerEngine::connect(
        id.clone(),
        DockerTarget::NamedPipe(bridge.path().to_owned()),
        DockerEngineOptions {
            kind: Some(EngineKind::WslDistro),
            transport: Some("bridge".into()),
            keepalive: Some(keepalive),
        },
    )
    .await
    .map_err(|error| {
        if error
            .to_string()
            .to_ascii_lowercase()
            .contains("permission denied")
        {
            permission_error(distro)
        } else {
            error
        }
    })?;
    into_engine(engine)
}

#[cfg(windows)]
async fn connect_tcp(id: &EngineId, port: u16) -> EngineResult<Arc<dyn Engine>> {
    use dk_engine_docker::{DockerEngine, DockerEngineOptions, DockerTarget};

    let engine = DockerEngine::connect(
        id.clone(),
        DockerTarget::Tcp {
            host: "127.0.0.1".into(),
            port,
            tls: None,
        },
        DockerEngineOptions {
            kind: Some(EngineKind::WslDistro),
            transport: Some("tcp".into()),
            keepalive: None,
        },
    )
    .await?;
    into_engine(engine)
}

/// The single conversion point from the Docker backend to `dyn Engine`.
#[cfg(windows)]
fn into_engine(engine: dk_engine_docker::DockerEngine) -> EngineResult<Arc<dyn Engine>> {
    Ok(Arc::new(engine))
}

#[cfg(not(windows))]
async fn connect_bridge(
    _id: &EngineId,
    _distro: &str,
    _tool: BridgeTool,
) -> EngineResult<Arc<dyn Engine>> {
    Err(EngineError::unreachable(NOT_WINDOWS))
}

#[cfg(not(windows))]
async fn connect_tcp(_id: &EngineId, _port: u16) -> EngineResult<Arc<dyn Engine>> {
    Err(EngineError::unreachable(NOT_WINDOWS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::BridgeTool;

    fn wsl_config(distro: &str, mode: WslMode) -> EngineConfig {
        let mut cfg = discovered_config(distro);
        cfg.endpoint = EngineEndpoint::WslDistro {
            distro: distro.into(),
            mode,
        };
        cfg
    }

    #[test]
    fn eng_007_discovered_engine_identity() {
        let cfg = discovered_config("Ubuntu-22.04");
        assert_eq!(cfg.id.as_str(), "wsl-ubuntu-22.04");
        assert_eq!(cfg.name, "Ubuntu-22.04 (WSL)");
        assert_eq!(cfg.origin, EngineOrigin::Discovered);
        assert_eq!(
            cfg.endpoint,
            EngineEndpoint::WslDistro {
                distro: "Ubuntu-22.04".into(),
                mode: WslMode::DialStdio
            }
        );
    }

    #[test]
    fn eng_007_maps_probe_results_to_discovery_state() {
        let available = discovered_engine(
            "Ubuntu",
            true,
            Some(Ok(ProbeClassification::Available(BridgeTool::Docker))),
        );
        assert_eq!(available.preference, preference::WSL_DISTRO);
        assert_eq!(available.initial_state, None);
        assert!(!available.show_only_when_all);

        let no_docker = discovered_engine("Ubuntu", true, Some(Ok(ProbeClassification::NoDocker)));
        assert!(no_docker.show_only_when_all);
        assert_eq!(no_docker.initial_state, None);

        let no_client = discovered_engine(
            "Ubuntu",
            true,
            Some(Ok(ProbeClassification::SocketNoClient)),
        );
        let Some(EngineState::Failed { error, .. }) = no_client.initial_state else {
            panic!("expected Failed");
        };
        assert!(error.hint().is_some_and(|hint| hint.contains("socat")));

        let denied = discovered_engine(
            "Ubuntu",
            true,
            Some(Ok(ProbeClassification::PermissionDenied)),
        );
        let Some(EngineState::Failed { error, .. }) = denied.initial_state else {
            panic!("expected Failed");
        };
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("sudo usermod -aG docker $USER"), "{hint}");
        assert!(hint.contains("wsl --terminate Ubuntu"), "{hint}");
    }

    #[test]
    fn eng_106_stopped_distro_is_listed_stopped_and_not_probed() {
        let stopped = discovered_engine("Debian", false, None);
        assert_eq!(stopped.initial_state, Some(EngineState::Stopped));
        assert!(!stopped.show_only_when_all);
        assert_eq!(stopped_error("Debian").hint(), Some(STOPPED_HINT));
    }

    #[test]
    fn nfr_022_rejects_invalid_endpoints() {
        for distro in ["-d", "a b", "x;y", ""] {
            let cfg = wsl_config(distro, WslMode::DialStdio);
            assert!(
                matches!(
                    distro_endpoint(&cfg),
                    Err(EngineError::Api { status: 400, .. })
                ),
                "{distro}"
            );
        }
        let cfg = wsl_config("Ubuntu", WslMode::Tcp { port: 0 });
        assert!(distro_endpoint(&cfg).is_err());
        let mut other = discovered_config("Ubuntu");
        other.endpoint = EngineEndpoint::NamedPipe {
            path: r"\\.\pipe\docker_engine".into(),
        };
        assert!(distro_endpoint(&other).is_err());
        let ok = wsl_config("Ubuntu", WslMode::Tcp { port: 2375 });
        assert_eq!(
            distro_endpoint(&ok).ok(),
            Some(("Ubuntu", WslMode::Tcp { port: 2375 }))
        );
    }

    #[test]
    fn eng_007_handles_only_wsl_endpoints() {
        let factory = WslDistroFactory::new();
        assert_eq!(factory.kind(), EngineKind::WslDistro);
        assert!(factory.handles(&discovered_config("Ubuntu").endpoint));
    }

    #[test]
    fn schema_has_distro_mode_and_port() {
        let schema = WslDistroFactory::new().config_schema();
        assert_eq!(schema.len(), 1);
        assert_eq!(schema[0].kind, EngineKind::WslDistro);
        assert_eq!(schema[0].label, "WSL distro");
        let keys = schema[0]
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["distro", "mode", "port"]);
        assert_eq!(schema[0].fields[0].kind, ConfigFieldKind::Text);
        assert_eq!(schema[0].fields[2].kind, ConfigFieldKind::Port);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn non_windows_is_empty_and_unreachable() {
        let factory = WslDistroFactory::new();
        assert!(factory.discover().await.is_empty());
        let error = factory
            .connect(&discovered_config("Ubuntu"))
            .await
            .err()
            .expect("unreachable");
        assert_eq!(error, EngineError::unreachable(NOT_WINDOWS));
    }

    /// Live: discovery on this machine lists the running Ubuntu-22.04 as available and skips
    /// Docker Desktop's distros; `probe` reports it running without a Docker ping.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "live: needs running WSL distro Ubuntu-22.04 with Docker"]
    async fn eng_007_live_discover_and_probe_ubuntu() {
        let factory = WslDistroFactory::new();
        let found = factory.discover().await;
        eprintln!("{found:#?}");
        assert!(
            found
                .iter()
                .all(|engine| !engine.config.id.as_str().starts_with("wsl-docker-desktop"))
        );
        let ubuntu = found
            .iter()
            .find(|engine| engine.config.id.as_str() == "wsl-ubuntu-22.04")
            .expect("Ubuntu-22.04 discovered");
        assert_eq!(ubuntu.initial_state, None);
        assert!(!ubuntu.show_only_when_all);
        assert_eq!(factory.probe(&ubuntu.config).await, ProbeResult::Reachable);
    }

    // ── ENG-146/148/149/155 through a fake `wsl.exe` (no real distro is ever touched) ──

    use crate::discovery::PROBE_SCRIPT;
    use crate::runner::fake::FakeWsl;

    fn factory(fake: &Arc<FakeWsl>) -> WslDistroFactory {
        let mut factory = WslDistroFactory::with_wsl(fake.clone());
        factory.ready_poll = Duration::from_millis(1);
        factory.ready_timeout = Duration::from_millis(150);
        factory
    }

    fn distro(name: &str, running: bool) -> discovery::Distro {
        discovery::Distro {
            name: name.into(),
            running,
        }
    }

    /// Every argv Dockering may issue: the list call, the probe, the systemd check, the two
    /// fixed start commands and the boot.
    fn allowed(argv: &[String]) -> bool {
        let argv = argv.iter().map(String::as_str).collect::<Vec<_>>();
        match argv.as_slice() {
            ["--list", "--running", "--quiet"] => true,
            ["-d", _, "--exec", "sh", "-c", script] => *script == PROBE_SCRIPT,
            ["-d", _, "--exec", "test", "-d", "/run/systemd/system"] => true,
            [
                "-d",
                _,
                "-u",
                "root",
                "--exec",
                "systemctl",
                "start",
                "docker",
            ] => true,
            [
                "-d",
                _,
                "-u",
                "root",
                "--exec",
                "/usr/sbin/service",
                "docker",
                "start",
            ] => true,
            ["-d", _, "--exec", "true"] => true,
            _ => false,
        }
    }

    fn only_targets(calls: &[Vec<String>], name: &str) {
        for call in calls {
            if call.first().map(String::as_str) == Some("-d") {
                assert_eq!(call[1], name, "{call:?}");
            }
        }
    }

    #[test]
    fn eng_148_exit_14_listed_stopped() {
        let found = discovered_engine(
            "Debian",
            true,
            Some(Ok(ProbeClassification::InstalledNotRunning)),
        );
        assert_eq!(found.initial_state, Some(EngineState::Stopped));
        assert!(!found.show_only_when_all);
        assert_eq!(found.config.id.as_str(), "wsl-debian");
    }

    #[test]
    fn eng_148_exit_11_still_hidden() {
        let found = discovered_engine("Debian", true, Some(Ok(ProbeClassification::NoDocker)));
        assert!(found.show_only_when_all);
        assert_eq!(found.initial_state, None);
    }

    #[tokio::test]
    async fn eng_148_discovery_lists_not_running_distro_with_start_docker_prompt() {
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14]);
        let factory = factory(&fake);
        let found = factory
            .discover_from(vec![distro("Debian", true), distro("Idle", false)])
            .await;
        assert_eq!(found[0].initial_state, Some(EngineState::Stopped));
        assert_eq!(found[1].initial_state, Some(EngineState::Stopped));
        let prompt = factory.start_prompt(&found[0].config).expect("prompt");
        assert_eq!(prompt.title, "Docker isn't running in Debian");
        assert_eq!(
            prompt.body,
            "Start Docker to see its containers and images."
        );
        assert_eq!(prompt.action_label, "Start Docker");
        // A stopped distro keeps the generic *Start & connect* (no prompt of its own).
        assert_eq!(factory.start_prompt(&found[1].config), None);
        // Only the running distro was probed (ENG-106).
        assert_eq!(fake.calls().len(), 1);
    }

    #[tokio::test]
    async fn eng_148_connect_reports_not_running_through_stopped_after() {
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14]);
        let factory = factory(&fake);
        let cfg = discovered_config("Debian");
        let error = factory.connect(&cfg).await.err().expect("not running");
        assert!(is_not_running_error(&error), "{error}");
        assert!(factory.stopped_after(&cfg, &error).await);
        assert!(factory.start_prompt(&cfg).is_some());

        // A stopped distro never reaches the probe: connect says "stopped", not "not running".
        let fake = FakeWsl::new(&[]);
        let stopped = self::factory(&fake);
        let error = stopped.connect(&cfg).await.err().expect("stopped");
        assert!(!stopped.stopped_after(&cfg, &error).await);
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn eng_146_is_not_running_error_both_ways() {
        assert!(is_not_running_error(&not_running_error("Debian")));
        for error in [
            permission_error("Debian"),
            no_docker_error("Debian"),
            no_client_error("Debian"),
            stopped_error("Debian"),
            start_failed_error("Debian", "x"),
            EngineError::unreachable("Docker isn't running in Debian"),
            EngineError::unreachable_with_hint("socat", NOT_RUNNING_HINT),
            EngineError::Timeout(Duration::from_secs(1)),
        ] {
            assert!(!is_not_running_error(&error), "{error}");
        }
    }

    #[tokio::test]
    async fn eng_146_distro_stopped_after_only_for_not_running() {
        let fake = FakeWsl::new(&["Debian", "Other"]);
        let factory = factory(&fake);
        let cfg = discovered_config("Debian");
        assert!(
            factory
                .stopped_after(&cfg, &not_running_error("Debian"))
                .await
        );
        for error in [
            permission_error("Debian"),
            no_docker_error("Debian"),
            no_client_error("Debian"),
            stopped_error("Debian"),
            // Another distro's error is not this engine's "not started yet".
            not_running_error("Other"),
        ] {
            assert!(!factory.stopped_after(&cfg, &error).await, "{error}");
        }
        let mut other = discovered_config("Debian");
        other.endpoint = EngineEndpoint::NamedPipe {
            path: r"\\.\pipe\docker_engine".into(),
        };
        assert!(
            !factory
                .stopped_after(&other, &not_running_error("Debian"))
                .await
        );
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn eng_149_systemd_and_service_argv() {
        for (systemd, expected) in [
            (true, ["systemctl", "start", "docker"]),
            (false, ["/usr/sbin/service", "docker", "start"]),
        ] {
            let fake = FakeWsl::new(&["Debian"]);
            fake.probes(&[14, 0]).pings(&[true]).systemd(systemd);
            let factory = factory(&fake);
            factory
                .start(&discovered_config("Debian"))
                .await
                .expect("started");
            let start = fake
                .calls()
                .into_iter()
                .find(|call| call.contains(&"root".to_owned()))
                .expect("start argv");
            let mut want = vec!["-d", "Debian", "-u", "root", "--exec"];
            want.extend(expected);
            assert_eq!(start, want);
            // Docker answered, so it is no longer an installed-not-running distro.
            assert_eq!(factory.start_prompt(&discovered_config("Debian")), None);
        }
    }

    #[tokio::test]
    async fn eng_149_waits_for_ping_not_socket_then_errors() {
        // The socket is readable (probe 0) right away but Docker answers only on the 3rd ping.
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14, 0])
            .pings(&[false, false, true])
            .systemd(true);
        let factory = factory(&fake);
        factory
            .start(&discovered_config("Debian"))
            .await
            .expect("ready after the ping");
        assert_eq!(fake.pinged().len(), 3);

        // A stale socket that never answers: error after the timeout, with the reason.
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14, 0]).pings(&[false]).systemd(true);
        let timed_out = self::factory(&fake);
        let error = timed_out
            .start(&discovered_config("Debian"))
            .await
            .expect_err("times out");
        assert!(error.to_string().contains("didn't answer"), "{error}");
        assert!(!is_not_running_error(&error));
        assert!(error.hint().is_some());
        assert!(fake.pinged().len() > 1);

        // The start command failing reports its decoded message and never polls.
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14]).start_exit(1);
        let failing = self::factory(&fake);
        let error = failing
            .start(&discovered_config("Debian"))
            .await
            .expect_err("fails");
        assert!(error.to_string().contains("boom: unit failed"), "{error}");
        assert!(fake.pinged().is_empty());
    }

    #[tokio::test]
    async fn eng_149_never_edits_the_distro() {
        for systemd in [true, false] {
            let fake = FakeWsl::new(&["Debian"]);
            fake.probes(&[14, 0]).pings(&[false, true]).systemd(systemd);
            let factory = factory(&fake);
            factory
                .start(&discovered_config("Debian"))
                .await
                .expect("started");
            let calls = fake.calls();
            assert!(calls.len() >= 4, "{calls:?}");
            for call in &calls {
                assert!(allowed(call), "{call:?}");
            }
        }
        // A stopped distro is only booted with `--exec true`.
        let fake = FakeWsl::new(&[]);
        factory(&fake)
            .start(&discovered_config("Debian"))
            .await
            .expect("booted");
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], ["-d", "Debian", "--exec", "true"]);
        assert!(calls.iter().all(|call| allowed(call)));
    }

    #[tokio::test]
    async fn eng_149_not_offered_without_dockerd() {
        // Exit 11 (no dockerd, e.g. a Docker Desktop integration distro): no prompt, and a start
        // issues nothing beyond the probe.
        let fake = FakeWsl::new(&["Ubuntu"]);
        fake.probes(&[11]);
        let factory = factory(&fake);
        let found = factory.discover_from(vec![distro("Ubuntu", true)]).await;
        assert!(found[0].show_only_when_all);
        assert_eq!(factory.start_prompt(&found[0].config), None);
        let error = factory
            .start(&found[0].config)
            .await
            .expect_err("nothing to start");
        assert_eq!(error, no_docker_error("Ubuntu"));
        assert!(!is_not_running_error(&error));
        assert!(
            !fake
                .calls()
                .iter()
                .any(|call| call.contains(&"root".to_owned()))
        );
    }

    #[tokio::test]
    async fn eng_155_never_touches_other_distros() {
        let fake = FakeWsl::new(&["Debian", "Other"]);
        fake.probes(&[14, 0]).pings(&[true]).systemd(true);
        let factory = factory(&fake);
        factory
            .start(&discovered_config("Debian"))
            .await
            .expect("started");
        let calls = fake.calls();
        only_targets(&calls, "Debian");
        assert!(fake.pinged().iter().all(|name| name == "Debian"));
        let forbidden = ["--terminate", "--unregister", "--shutdown", "-t"];
        assert!(
            calls
                .iter()
                .all(|call| call.iter().all(|arg| !forbidden.contains(&arg.as_str())))
        );

        let fake = FakeWsl::new(&["Other"]);
        let booted = self::factory(&fake);
        booted
            .start(&discovered_config("Debian"))
            .await
            .expect("booted");
        only_targets(&fake.calls(), "Debian");
    }

    #[tokio::test]
    async fn eng_106_probe_never_boots_and_flags_not_running_only_when_known() {
        let cfg = discovered_config("Debian");
        // Stopped: just the list call, `Stopped`.
        let fake = FakeWsl::new(&[]);
        assert_eq!(factory(&fake).probe(&cfg).await, ProbeResult::Stopped);
        assert_eq!(fake.calls(), [["--list", "--running", "--quiet"]]);
        // Running and unknown: no script either.
        let fake = FakeWsl::new(&["Debian"]);
        assert_eq!(factory(&fake).probe(&cfg).await, ProbeResult::Reachable);
        assert_eq!(fake.calls().len(), 1);
        // Running and known installed-not-running: re-probed, `Stopped` until Docker is up.
        let fake = FakeWsl::new(&["Debian"]);
        fake.probes(&[14, 14, 0]);
        let known = self::factory(&fake);
        known.discover_from(vec![distro("Debian", true)]).await;
        assert_eq!(known.probe(&cfg).await, ProbeResult::Stopped);
        assert_eq!(known.probe(&cfg).await, ProbeResult::Reachable);
        assert!(known.start_prompt(&cfg).is_none());
        assert!(fake.pinged().is_empty());
    }

    /// Live and READ-ONLY: runs only the probe script in the user's Docker Desktop-integrated
    /// distro (no start, no stop). It must stay `available`, never exit 14.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "live: needs running WSL distro Ubuntu-22.04 (read-only probe)"]
    async fn eng_148_live_probe_reports_available_on_ubuntu() {
        let class = discovery::probe(&RealWsl, "Ubuntu-22.04")
            .await
            .expect("probe");
        eprintln!("{class:?}");
        assert_eq!(class, ProbeClassification::Available(BridgeTool::Docker));
    }
}
