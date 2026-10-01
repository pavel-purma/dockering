//! `WslDistroFactory`: WSL distros running Docker Engine (spec 20 §4, ENG-007/011/012/020/106/107).
//!
//! Engines are `DockerEngine`s reached through a [`crate::PipeBridge`] (default, ENG-011) or
//! over TCP on `127.0.0.1` (opt-in, ENG-012). Background checks never boot a stopped distro
//! (ENG-106): only [`EngineFactory::start`] (the user's *Start & connect*) runs a command in a
//! stopped distro.

use std::sync::Arc;

use async_trait::async_trait;
use dk_core::engine::preference;
use dk_core::{
    ConfigField, ConfigFieldKind, DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema,
    EngineEndpoint, EngineError, EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult,
    EngineState, ProbeResult, WslMode,
};

use crate::discovery::{self, ProbeClassification};

/// Hint for a stopped distro (ENG-106).
pub(crate) const STOPPED_HINT: &str = "Distro stopped — use Start & connect";
const NOT_WINDOWS: &str = "WSL is only available on Windows";

/// Discovers WSL distros and connects to the Docker Engine inside them.
#[derive(Debug, Default)]
pub struct WslDistroFactory {
    _private: (),
}

impl WslDistroFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

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
        let distros = match discovery::enumerate().await {
            Ok(distros) => distros,
            Err(error) => {
                tracing::debug!(%error, "WSL distro discovery failed");
                return Vec::new();
            }
        };
        // Probe running distros concurrently; stopped ones are never touched (ENG-106).
        let probes = distros.iter().map(|distro| async move {
            if distro.running {
                Some(
                    discovery::probe(&distro.name)
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
            .map(|(distro, probe)| discovered_engine(&distro.name, distro.running, probe))
            .collect()
    }

    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        let (distro, mode) = distro_endpoint(cfg)?;
        if !cfg!(windows) {
            return Err(EngineError::unreachable(NOT_WINDOWS));
        }
        let running = discovery::is_running(distro)
            .await
            .map_err(|error| io_unreachable("couldn't check the WSL distro state", error))?;
        if !running {
            // Never boot implicitly: the user starts it with *Start & connect* (ENG-106).
            return Err(stopped_error(distro));
        }
        match mode {
            WslMode::DialStdio => connect_bridge(&cfg.id, distro).await,
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

    /// Running-state check only — no Docker ping, so an idle distro can still shut down and a
    /// stopped one is never booted (ENG-020, ENG-106).
    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        let (distro, _) = match distro_endpoint(cfg) {
            Ok(endpoint) => endpoint,
            Err(error) => return ProbeResult::Unreachable(error),
        };
        if !cfg!(windows) {
            return ProbeResult::Unreachable(EngineError::unreachable(NOT_WINDOWS));
        }
        match discovery::is_running(distro).await {
            Ok(true) => ProbeResult::Reachable,
            Ok(false) => ProbeResult::Stopped,
            Err(error) => ProbeResult::Unreachable(io_unreachable(
                "couldn't check the WSL distro state",
                error,
            )),
        }
    }

    /// *Start & connect* (ENG-106): boots the distro with `wsl.exe -d <distro> --exec true`.
    async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        let (distro, _) = distro_endpoint(cfg)?;
        start_distro(distro).await
    }
}

#[cfg(windows)]
async fn start_distro(distro: &str) -> EngineResult<()> {
    let output = crate::runner::run(["-d", distro, "--exec", "true"])
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

#[cfg(not(windows))]
async fn start_distro(_distro: &str) -> EngineResult<()> {
    Err(EngineError::unreachable(NOT_WINDOWS))
}

#[cfg(windows)]
async fn connect_bridge(id: &EngineId, distro: &str) -> EngineResult<Arc<dyn Engine>> {
    use dk_engine_docker::{DockerEngine, DockerEngineOptions, DockerTarget};

    let tool = match discovery::probe(distro)
        .await
        .map_err(|error| io_unreachable(&format!("couldn't probe {distro}"), error))?
    {
        ProbeClassification::Available(tool) => tool,
        ProbeClassification::PermissionDenied => return Err(permission_error(distro)),
        ProbeClassification::NoDocker => return Err(no_docker_error(distro)),
        ProbeClassification::SocketNoClient => return Err(no_client_error(distro)),
    };
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
    // MERGE-FIXUP: replace with Ok(Arc::new(engine))
    // (needs `impl dk_core::Engine for DockerEngine` from the Docker backend branch).
    // Real code: `Ok(Arc::new(engine) as Arc<dyn Engine>)`
    let _unconverted = engine;
    Err(EngineError::unreachable("docker backend not merged"))
}

#[cfg(not(windows))]
async fn connect_bridge(_id: &EngineId, _distro: &str) -> EngineResult<Arc<dyn Engine>> {
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
}
