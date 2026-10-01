//! `WslDistroFactory` (ENG-007/011/012/020/106).

use std::sync::Arc;

use async_trait::async_trait;
use dk_core::{
    ConfigField, ConfigFieldKind, DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema,
    EngineEndpoint, EngineError, EngineFactory, EngineKind, EngineOrigin, EngineResult, EngineState,
    ProbeResult, WslMode,
};
use dk_core::engine::preference;
use dk_engine_docker::{DockerEngine, DockerEngineOptions, DockerTarget};

#[cfg(windows)]
use crate::PipeBridge;
use crate::discovery::{self, ProbeClassification};

pub struct WslDistroFactory {
    _private: (),
}

impl WslDistroFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WslDistroFactory {
    fn default() -> Self {
        Self::new()
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
        format!("permission denied opening Docker in {distro}"),
        format!(
            "Run `sudo usermod -aG docker $USER`, then `wsl --terminate {distro}` and reconnect"
        ),
    )
}

fn io_unreachable(operation: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::unreachable(format!("{operation}: {error}"))
}

fn distro_endpoint(cfg: &EngineConfig) -> EngineResult<(&str, WslMode)> {
    match &cfg.endpoint {
        EngineEndpoint::WslDistro { distro, mode } => {
            if !discovery::validate_distro_name(distro) {
                return Err(invalid_config("invalid WSL distro name"));
            }
            Ok((distro, *mode))
        }
        _ => Err(invalid_config("expected a WSL distro endpoint")),
    }
}

fn engine_id(name: &str) -> dk_core::EngineId {
    dk_core::EngineId::new(format!("wsl-{}", name.to_ascii_lowercase()))
}

fn discovered_config(name: &str) -> EngineConfig {
    EngineConfig {
        id: engine_id(name),
        name: format!("{name} (WSL)"),
        endpoint: EngineEndpoint::WslDistro {
            distro: name.to_owned(),
            mode: WslMode::DialStdio,
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
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
        #[cfg(not(windows))]
        {
            Vec::new()
        }

        #[cfg(windows)]
        {
            let distros = match discovery::enumerate().await {
                Ok(distros) => distros,
                Err(error) => {
                    tracing::debug!(%error, "WSL distro discovery failed");
                    return Vec::new();
                }
            };
            let mut engines = Vec::with_capacity(distros.len());
            for distro in distros {
                let mut discovered =
                    DiscoveredEngine::new(discovered_config(&distro.name), preference::WSL_DISTRO);
                if !distro.running {
                    discovered.initial_state = Some(EngineState::Stopped);
                    engines.push(discovered);
                    continue;
                }

                match discovery::probe(&distro.name).await {
                    Ok(ProbeClassification::Available(_)) => {}
                    Ok(ProbeClassification::NoDocker) => {
                        discovered.show_only_when_all = true;
                    }
                    Ok(ProbeClassification::SocketNoClient) => {
                        discovered.initial_state = Some(EngineState::Failed {
                            error: EngineError::Unreachable {
                                reason: format!(
                                    "docker CLI and socat not found in {}",
                                    distro.name
                                ),
                                hint: Some(
                                    "Install socat (`sudo apt install socat`) or enable TCP mode"
                                        .into(),
                                ),
                            },
                            retry_in_ms: None,
                        });
                    }
                    Ok(ProbeClassification::PermissionDenied) => {
                        discovered.initial_state = Some(EngineState::Failed {
                            error: permission_error(&distro.name),
                            retry_in_ms: None,
                        });
                    }
                    Err(error) => {
                        discovered.initial_state = Some(EngineState::Failed {
                            error: io_unreachable(
                                &format!("failed to probe {}", distro.name),
                                error,
                            ),
                            retry_in_ms: None,
                        });
                    }
                }
                engines.push(discovered);
            }
            engines
        }
    }

    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        #[cfg(not(windows))]
        {
            let _ = cfg;
            Err(EngineError::unreachable("WSL is only available on Windows"))
        }

        #[cfg(windows)]
        {
            let (distro, mode) = distro_endpoint(cfg)?;
            match mode {
                WslMode::DialStdio => {
                    let running = discovery::is_running(distro).await.map_err(|error| {
                        io_unreachable("failed to check WSL distro state", error)
                    })?;
                    if !running {
                        return Err(EngineError::unreachable_with_hint(
                            format!("WSL distro {distro} is stopped"),
                            "Distro stopped — use Start & connect",
                        ));
                    }
                    let tool = match discovery::probe(distro)
                        .await
                        .map_err(|error| io_unreachable("failed to probe WSL distro", error))?
                    {
                        ProbeClassification::Available(tool) => tool,
                        ProbeClassification::PermissionDenied => {
                            return Err(permission_error(distro));
                        }
                        ProbeClassification::NoDocker => {
                            return Err(EngineError::unreachable_with_hint(
                                format!("Docker socket not found in {distro}"),
                                "Start Docker Engine inside the distro",
                            ));
                        }
                        ProbeClassification::SocketNoClient => {
                            return Err(EngineError::unreachable_with_hint(
                                format!("docker CLI and socat not found in {distro}"),
                                "Install socat (`sudo apt install socat`) or enable TCP mode",
                            ));
                        }
                    };
                    let bridge = PipeBridge::start(distro.to_owned(), tool)
                        .await
                        .map_err(|error| io_unreachable("failed to start WSL bridge", error))?;
                    let keepalive: Arc<dyn std::any::Any + Send + Sync> = bridge.clone();
                    let result = DockerEngine::connect(
                        cfg.id.clone(),
                        DockerTarget::NamedPipe(bridge.path().to_owned()),
                        DockerEngineOptions {
                            kind: Some(EngineKind::WslDistro),
                            transport: Some("bridge".into()),
                            keepalive: Some(keepalive),
                        },
                    )
                    .await;
                    match result {
                        Ok(engine) => Ok(Arc::new(engine)),
                        Err(error)
                            if format!("{error}")
                                .to_ascii_lowercase()
                                .contains("permission denied") =>
                        {
                            Err(permission_error(distro))
                        }
                        Err(error) => Err(error),
                    }
                }
                WslMode::Tcp { port } => {
                    if port == 0 {
                        return Err(invalid_config("WSL TCP port must be non-zero"));
                    }
                    let engine = DockerEngine::connect(
                        cfg.id.clone(),
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
                    Ok(Arc::new(engine))
                }
            }
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
                    // Discovery is async while schemas are synchronous, so the generic UI accepts
                    // validated text here. Discovered distros are still presented by the engine list.
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

    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        #[cfg(not(windows))]
        {
            let _ = cfg;
            ProbeResult::Unreachable(EngineError::unreachable("WSL is only available on Windows"))
        }

        #[cfg(windows)]
        {
            let (distro, _) = match distro_endpoint(cfg) {
                Ok(endpoint) => endpoint,
                Err(error) => return ProbeResult::Unreachable(error),
            };
            match discovery::is_running(distro).await {
                Ok(true) => ProbeResult::Reachable,
                Ok(false) => ProbeResult::Stopped,
                Err(error) => ProbeResult::Unreachable(io_unreachable(
                    "failed to check WSL distro state",
                    error,
                )),
            }
        }
    }

    async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        #[cfg(not(windows))]
        {
            let _ = cfg;
            Err(EngineError::unreachable("WSL is only available on Windows"))
        }

        #[cfg(windows)]
        {
            let (distro, _) = distro_endpoint(cfg)?;
            let output = crate::runner::run(["-d", distro, "--exec", "true"])
                .await
                .map_err(|error| io_unreachable("failed to start WSL distro", error))?;
            if output.success {
                Ok(())
            } else {
                Err(io_unreachable(
                    "failed to start WSL distro",
                    output.stderr_utf8().trim(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_id_is_stable_lowercase_slug() {
        assert_eq!(engine_id("Ubuntu-22.04").as_str(), "wsl-ubuntu-22.04");
    }

    #[test]
    fn schema_has_distro_mode_and_port() {
        let schema = WslDistroFactory::new().config_schema();
        let keys = schema[0]
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["distro", "mode", "port"]);
    }
}
