//! `DockerFactory`: discovery (ENG-001…006, ENG-009/010), connect, probe, and the
//! "Add engine" schema (spec 20 §2, 21 §8).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use dk_core::engine::preference;
use dk_core::{
    ConfigField, ConfigFieldKind, DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema,
    EngineEndpoint, EngineError, EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult,
    EngineState, ProbeResult, TlsFiles,
};
use futures::future::join_all;
use serde_json::Value;

use crate::connect::{self, PROBE_TIMEOUT};
use crate::errors::{ErrCtx, map_err};
use crate::registry_auth::docker_config_dir;
use crate::{DockerEngine, DockerEngineOptions, DockerTarget};

const SSH_UNSUPPORTED: &str = "ssh contexts are not supported in v1";

pub struct DockerFactory {
    _private: (),
}

impl DockerFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for DockerFactory {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────── pure parsing helpers ─────────────────────────────

/// `\\.\pipe\x` form from `npipe:////./pipe/x`, `//./pipe/x`, or `\\.\pipe\x`.
pub(crate) fn normalize_pipe_path(p: &str) -> String {
    let p = p.trim();
    let p = p.strip_prefix("npipe://").unwrap_or(p);
    p.replace('/', "\\")
}

/// `host:port`, `[v6]:port`, `host` → `(host, port)`.
fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    let s = s.split('/').next().unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host.to_owned(), port.parse().ok()?)),
        Some(_) => Some((s.to_owned(), default_port)), // bare IPv6
        None => Some((s.to_owned(), default_port)),
    }
}

/// TLS files under a Docker cert directory (`ca.pem`, `cert.pem`, `key.pem`).
pub(crate) fn tls_files_in(dir: &Path, verify: bool) -> TlsFiles {
    TlsFiles {
        ca: dir.join("ca.pem"),
        cert: dir.join("cert.pem"),
        key: dir.join("key.pem"),
        verify,
    }
}

/// Parse a Docker host URL (ENG-001, ENG-002). `tls` is used for `tcp://` hosts; `https://`
/// forces TLS. `None` for unsupported schemes (`fd://`, garbage).
pub(crate) fn parse_docker_host(host: &str, tls: Option<TlsFiles>) -> Option<EngineEndpoint> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let (scheme, rest) = host.split_once("://").unwrap_or(("tcp", host));
    match scheme.to_ascii_lowercase().as_str() {
        "unix" => {
            let path = rest.trim();
            (!path.is_empty()).then(|| EngineEndpoint::UnixSocket {
                path: PathBuf::from(path),
            })
        }
        "npipe" => {
            let path = normalize_pipe_path(rest);
            (!path.is_empty()).then_some(EngineEndpoint::NamedPipe { path })
        }
        "tcp" | "http" | "https" => {
            let tls = if scheme.eq_ignore_ascii_case("https") && tls.is_none() {
                docker_config_dir().map(|d| tls_files_in(&d, false))
            } else {
                tls
            };
            let default_port = if tls.is_some() { 2376 } else { 2375 };
            let (h, port) = split_host_port(rest, default_port)?;
            Some(EngineEndpoint::Tcp { host: h, port, tls })
        }
        "ssh" => Some(EngineEndpoint::Ssh {
            url: host.to_owned(),
        }),
        _ => None,
    }
}

/// TLS from `DOCKER_TLS_VERIFY` / `DOCKER_CERT_PATH` (Docker CLI semantics: any non-empty
/// `DOCKER_TLS_VERIFY` enables verified TLS; certs default to the Docker config dir).
pub(crate) fn tls_from_env(
    tls_verify: Option<&str>,
    cert_path: Option<&Path>,
    default_dir: Option<&Path>,
) -> Option<TlsFiles> {
    let verify = tls_verify.map(str::trim).filter(|v| !v.is_empty())?;
    let verify = !matches!(verify, "0" | "false" | "FALSE" | "False");
    let dir = cert_path.or(default_dir)?;
    Some(tls_files_in(dir, verify))
}

/// `context-<slug>` id.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unnamed".into()
    } else {
        out
    }
}

/// One context from `contexts/meta/<hash>/meta.json` → `(name, endpoint)`.
/// `tls_dir` = `contexts/tls/<hash>/docker` if it exists.
pub(crate) fn parse_context_meta(
    meta: &Value,
    tls_dir: Option<&Path>,
) -> Option<(String, EngineEndpoint)> {
    let name = meta.get("Name")?.as_str()?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let docker = meta.get("Endpoints")?.get("docker")?;
    let host = docker.get("Host")?.as_str()?;
    let skip_verify = docker
        .get("SkipTLSVerify")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tls = tls_dir.map(|d| tls_files_in(d, !skip_verify));
    let endpoint = parse_docker_host(host, tls)?;
    Some((name, endpoint))
}

/// Friendly engine name from `/info` (ENG-005).
pub(crate) fn name_from_info(info: &Value) -> Option<String> {
    let s = |k: &str| {
        info.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let os = s("OperatingSystem");
    let name = s("Name");
    let labels = info
        .get("Labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let podman = info
        .pointer("/Components")
        .map(|c| c.to_string().to_ascii_lowercase().contains("podman"))
        .unwrap_or(false)
        || labels.contains("podman")
        || s("ServerVersion").contains("podman");
    if os.contains("docker desktop") || name == "docker-desktop" {
        Some("Docker Desktop".into())
    } else if os.contains("orbstack") || name == "orbstack" {
        Some("OrbStack".into())
    } else if name.contains("colima") || labels.contains("colima") {
        Some("Colima".into())
    } else if name.contains("rancher-desktop") || os.contains("rancher desktop") {
        Some("Rancher Desktop".into())
    } else if podman {
        Some("Podman".into())
    } else {
        None
    }
}

// ───────────────────────────── candidates ─────────────────────────────

#[derive(Debug, Clone)]
struct Candidate {
    id: &'static str,
    name: &'static str,
    endpoint: EngineEndpoint,
    /// Use the `/info` heuristics for the name.
    sniff_name: bool,
}

fn socket_candidates() -> Vec<Candidate> {
    let mut out = Vec::new();
    #[cfg(unix)]
    {
        let home = crate::registry_auth::home_dir();
        let xdg = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|x| !x.is_empty())
            .map(PathBuf::from);
        let mut push = |id, name, path: Option<PathBuf>, sniff| {
            if let Some(path) = path {
                out.push(Candidate {
                    id,
                    name,
                    endpoint: EngineEndpoint::UnixSocket { path },
                    sniff_name: sniff,
                });
            }
        };
        push(
            "docker-local",
            "Docker (local)",
            Some(PathBuf::from("/var/run/docker.sock")),
            true,
        );
        if cfg!(target_os = "linux") {
            push(
                "docker-rootless",
                "Docker (rootless)",
                xdg.as_ref().map(|x| x.join("docker.sock")),
                false,
            );
        }
        push(
            "docker-desktop",
            "Docker Desktop",
            home.as_ref().map(|h| h.join(".docker/run/docker.sock")),
            false,
        );
        push(
            "colima",
            "Colima",
            home.as_ref().map(|h| h.join(".colima/default/docker.sock")),
            false,
        );
        push(
            "orbstack",
            "OrbStack",
            home.as_ref().map(|h| h.join(".orbstack/run/docker.sock")),
            false,
        );
        push(
            "rancher-desktop",
            "Rancher Desktop",
            home.as_ref().map(|h| h.join(".rd/docker.sock")),
            false,
        );
        if cfg!(target_os = "linux") {
            push(
                "podman",
                "Podman",
                xdg.as_ref().map(|x| x.join("podman/podman.sock")),
                false,
            );
        }
    }
    #[cfg(windows)]
    {
        out.push(Candidate {
            id: "docker-desktop",
            name: "Docker Desktop",
            endpoint: EngineEndpoint::NamedPipe {
                path: r"\\.\pipe\docker_engine".into(),
            },
            sniff_name: true,
        });
        out.push(Candidate {
            id: "docker-desktop-linux",
            name: "Docker Desktop (Linux engine)",
            endpoint: EngineEndpoint::NamedPipe {
                path: r"\\.\pipe\dockerDesktopLinuxEngine".into(),
            },
            sniff_name: false,
        });
    }
    out
}

fn endpoint_path(e: &EngineEndpoint) -> Option<PathBuf> {
    match e {
        EngineEndpoint::UnixSocket { path } => Some(path.clone()),
        EngineEndpoint::NamedPipe { path } => Some(PathBuf::from(path)),
        _ => None,
    }
}

fn discovered(id: String, name: String, endpoint: EngineEndpoint, pref: u8) -> DiscoveredEngine {
    let unsupported = matches!(endpoint, EngineEndpoint::Ssh { .. });
    let mut d = DiscoveredEngine::new(
        EngineConfig {
            id: EngineId::new(id),
            name,
            endpoint,
            origin: EngineOrigin::Discovered,
            enabled: true,
            hidden: false,
        },
        pref,
    );
    if unsupported {
        d.initial_state = Some(EngineState::Unsupported {
            reason: SSH_UNSUPPORTED.into(),
        });
    }
    d
}

/// ENG-001.
fn docker_host_engine() -> Option<DiscoveredEngine> {
    let host = std::env::var("DOCKER_HOST").ok()?;
    let verify = std::env::var("DOCKER_TLS_VERIFY").ok();
    let cert = std::env::var_os("DOCKER_CERT_PATH")
        .filter(|c| !c.is_empty())
        .map(PathBuf::from);
    let tls = tls_from_env(
        verify.as_deref(),
        cert.as_deref(),
        docker_config_dir().as_deref(),
    );
    let endpoint = parse_docker_host(&host, tls)?;
    Some(discovered(
        "docker-host".into(),
        "DOCKER_HOST".into(),
        endpoint,
        preference::DOCKER_HOST,
    ))
}

/// ENG-002: contexts (skipping `default`, which is the default socket/pipe).
async fn context_engines() -> Vec<DiscoveredEngine> {
    let Some(dir) = docker_config_dir() else {
        return Vec::new();
    };
    let current = match tokio::fs::read_to_string(dir.join("config.json")).await {
        Ok(text) => serde_json::from_str::<Value>(&text).ok().and_then(|v| {
            v.get("currentContext")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        }),
        Err(_) => None,
    };
    let current = std::env::var("DOCKER_CONTEXT")
        .ok()
        .filter(|c| !c.is_empty())
        .or(current);
    let meta_root = dir.join("contexts").join("meta");
    let Ok(mut entries) = tokio::fs::read_dir(&meta_root).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let hash = entry.file_name();
        let Ok(text) = tokio::fs::read_to_string(entry.path().join("meta.json")).await else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let tls_dir = dir.join("contexts").join("tls").join(&hash).join("docker");
        let tls_dir = tokio::fs::try_exists(tls_dir.join("ca.pem"))
            .await
            .unwrap_or(false)
            .then_some(tls_dir);
        let Some((name, endpoint)) = parse_context_meta(&meta, tls_dir.as_deref()) else {
            continue;
        };
        if name == "default" {
            continue;
        }
        let pref = if current.as_deref() == Some(name.as_str()) {
            preference::DOCKER_CONTEXT
        } else {
            preference::DOCKER_CONTEXT + 5
        };
        out.push(discovered(
            format!("context-{}", slug(&name)),
            name,
            endpoint,
            pref,
        ));
    }
    out
}

/// Ping (any HTTP answer counts) and sniff the name, within `PROBE_TIMEOUT`.
async fn probe_socket(c: &Candidate) -> (bool, Option<String>) {
    let Some(target) = target_for(&c.endpoint) else {
        return (false, None);
    };
    let fut = async {
        let ctx = ErrCtx {
            hint: connect::hint_ctx(&target, false),
            timeout: PROBE_TIMEOUT,
        };
        let Ok(docker) =
            connect::build_client(&target, PROBE_TIMEOUT, &connect::min_version(), &ctx)
        else {
            return (false, None);
        };
        let alive = match docker.ping().await {
            Ok(_) => true,
            Err(bollard::errors::Error::DockerResponseServerError { .. }) => true,
            Err(_) => false,
        };
        if !alive || !c.sniff_name {
            return (alive, None);
        }
        let name = match docker.info().await {
            Ok(info) => serde_json::to_value(&info)
                .ok()
                .and_then(|v| name_from_info(&v)),
            Err(_) => None,
        };
        (true, name)
    };
    tokio::time::timeout(PROBE_TIMEOUT, fut)
        .await
        .unwrap_or((false, None))
}

async fn local_engines() -> Vec<DiscoveredEngine> {
    let candidates = socket_candidates();
    let probes = candidates.into_iter().map(|c| async move {
        let path = endpoint_path(&c.endpoint)?;
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return None;
        }
        let (alive, sniffed) = probe_socket(&c).await;
        tracing::debug!(id = c.id, alive, "docker socket probe");
        let name = sniffed.unwrap_or_else(|| c.name.to_owned());
        Some(discovered(
            c.id.to_owned(),
            name,
            c.endpoint,
            preference::LOCAL_SOCKET,
        ))
    });
    join_all(probes).await.into_iter().flatten().collect()
}

/// Dedupe key (ENG-009): canonical endpoint, with Unix sockets resolved through symlinks.
async fn dedupe_key(e: &EngineEndpoint) -> String {
    if let EngineEndpoint::UnixSocket { path } = e
        && let Ok(real) = tokio::fs::canonicalize(path).await
    {
        return EngineEndpoint::UnixSocket { path: real }.canonical();
    }
    e.canonical()
}

/// Keep the most preferred engine per key (stable for equal preference).
pub(crate) fn dedupe(mut found: Vec<(String, DiscoveredEngine)>) -> Vec<DiscoveredEngine> {
    found.sort_by_key(|(_, d)| d.preference);
    let mut seen_keys = HashSet::new();
    let mut seen_ids = HashSet::new();
    found
        .into_iter()
        .filter(|(key, d)| seen_keys.insert(key.clone()) && seen_ids.insert(d.config.id.clone()))
        .map(|(_, d)| d)
        .collect()
}

pub(crate) fn target_for(endpoint: &EngineEndpoint) -> Option<DockerTarget> {
    match endpoint {
        EngineEndpoint::UnixSocket { path } => Some(DockerTarget::Unix(path.clone())),
        EngineEndpoint::NamedPipe { path } => Some(DockerTarget::NamedPipe(path.clone())),
        EngineEndpoint::Tcp { host, port, tls } => Some(DockerTarget::Tcp {
            host: host.clone(),
            port: *port,
            tls: tls.clone(),
        }),
        _ => None,
    }
}

fn unsupported_endpoint(endpoint: &EngineEndpoint) -> EngineError {
    match endpoint {
        EngineEndpoint::Ssh { .. } => EngineError::Unreachable {
            reason: "unsupported endpoint".into(),
            hint: Some(SSH_UNSUPPORTED.into()),
        },
        _ => EngineError::unreachable("unsupported endpoint"),
    }
}

fn field(
    key: &str,
    label: &str,
    kind: ConfigFieldKind,
    required: bool,
    ph: Option<&str>,
) -> ConfigField {
    ConfigField {
        key: key.into(),
        label: label.into(),
        kind,
        required,
        placeholder: ph.map(Into::into),
    }
}

#[async_trait]
impl EngineFactory for DockerFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        matches!(
            endpoint,
            EngineEndpoint::UnixSocket { .. }
                | EngineEndpoint::NamedPipe { .. }
                | EngineEndpoint::Tcp { .. }
                | EngineEndpoint::Ssh { .. }
        )
    }

    async fn discover(&self) -> Vec<DiscoveredEngine> {
        let (contexts, locals) = futures::join!(context_engines(), local_engines());
        let mut all: Vec<DiscoveredEngine> = docker_host_engine().into_iter().collect();
        all.extend(contexts);
        all.extend(locals);
        let mut keyed = Vec::with_capacity(all.len());
        for d in all {
            keyed.push((dedupe_key(&d.config.endpoint).await, d));
        }
        dedupe(keyed)
    }

    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        let target =
            target_for(&cfg.endpoint).ok_or_else(|| unsupported_endpoint(&cfg.endpoint))?;
        let engine =
            DockerEngine::connect(cfg.id.clone(), target, DockerEngineOptions::default()).await?;
        Ok(Arc::new(engine))
    }

    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        let path_kind = if cfg!(windows) {
            ConfigFieldKind::Text
        } else {
            ConfigFieldKind::Path
        };
        vec![
            EngineConfigSchema {
                kind: EngineKind::Docker,
                label: "Unix socket".into(),
                fields: vec![field(
                    "path",
                    "Socket path",
                    ConfigFieldKind::Path,
                    true,
                    Some("/var/run/docker.sock"),
                )],
            },
            EngineConfigSchema {
                kind: EngineKind::Docker,
                label: "Named pipe".into(),
                fields: vec![field(
                    "path",
                    "Pipe path",
                    path_kind,
                    true,
                    Some(r"\\.\pipe\docker_engine"),
                )],
            },
            EngineConfigSchema {
                kind: EngineKind::Docker,
                label: "TCP".into(),
                fields: vec![
                    field(
                        "host",
                        "Host",
                        ConfigFieldKind::Text,
                        true,
                        Some("127.0.0.1"),
                    ),
                    field("port", "Port", ConfigFieldKind::Port, true, Some("2375")),
                ],
            },
            EngineConfigSchema {
                kind: EngineKind::Docker,
                label: "TCP + TLS".into(),
                fields: vec![
                    field(
                        "host",
                        "Host",
                        ConfigFieldKind::Text,
                        true,
                        Some("docker.example.com"),
                    ),
                    field("port", "Port", ConfigFieldKind::Port, true, Some("2376")),
                    field(
                        "ca",
                        "CA certificate",
                        ConfigFieldKind::Path,
                        true,
                        Some("ca.pem"),
                    ),
                    field(
                        "cert",
                        "Client certificate",
                        ConfigFieldKind::Path,
                        true,
                        Some("cert.pem"),
                    ),
                    field(
                        "key",
                        "Client key",
                        ConfigFieldKind::Path,
                        true,
                        Some("key.pem"),
                    ),
                    field(
                        "verify",
                        "Verify server certificate",
                        ConfigFieldKind::Bool,
                        false,
                        None,
                    ),
                ],
            },
        ]
    }

    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        let Some(target) = target_for(&cfg.endpoint) else {
            return ProbeResult::Unreachable(unsupported_endpoint(&cfg.endpoint));
        };
        let ctx = ErrCtx {
            hint: connect::hint_ctx(&target, false),
            timeout: PROBE_TIMEOUT,
        };
        let docker =
            match connect::build_client(&target, PROBE_TIMEOUT, &connect::min_version(), &ctx) {
                Ok(d) => d,
                Err(e) => return ProbeResult::Unreachable(e),
            };
        match tokio::time::timeout(PROBE_TIMEOUT, docker.ping()).await {
            Ok(Ok(_)) => ProbeResult::Reachable,
            // Any HTTP answer means the daemon is up (e.g. a version mismatch on `_ping`).
            Ok(Err(bollard::errors::Error::DockerResponseServerError { .. })) => {
                ProbeResult::Reachable
            }
            Ok(Err(e)) => ProbeResult::Unreachable(map_err(e, &ctx, None)),
            Err(_) => ProbeResult::Unreachable(EngineError::Timeout(PROBE_TIMEOUT)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eng_001_parse_docker_host() {
        assert_eq!(
            parse_docker_host("unix:///var/run/docker.sock", None),
            Some(EngineEndpoint::UnixSocket {
                path: "/var/run/docker.sock".into()
            })
        );
        assert_eq!(
            parse_docker_host("npipe:////./pipe/docker_engine", None),
            Some(EngineEndpoint::NamedPipe {
                path: r"\\.\pipe\docker_engine".into()
            })
        );
        assert_eq!(
            parse_docker_host("tcp://10.0.0.5:2375", None),
            Some(EngineEndpoint::Tcp {
                host: "10.0.0.5".into(),
                port: 2375,
                tls: None
            })
        );
        assert_eq!(
            parse_docker_host("tcp://[::1]:2375", None),
            Some(EngineEndpoint::Tcp {
                host: "::1".into(),
                port: 2375,
                tls: None
            })
        );
        assert_eq!(
            parse_docker_host("tcp://docker.local", None),
            Some(EngineEndpoint::Tcp {
                host: "docker.local".into(),
                port: 2375,
                tls: None
            })
        );
        let tls = tls_files_in(Path::new("/certs"), true);
        assert_eq!(
            parse_docker_host("tcp://docker.local", Some(tls.clone())),
            Some(EngineEndpoint::Tcp {
                host: "docker.local".into(),
                port: 2376,
                tls: Some(tls)
            })
        );
        assert_eq!(
            parse_docker_host("ssh://me@host", None),
            Some(EngineEndpoint::Ssh {
                url: "ssh://me@host".into()
            })
        );
        assert_eq!(parse_docker_host("fd://", None), None);
        assert_eq!(parse_docker_host("", None), None);
        assert_eq!(parse_docker_host("tcp://host:notaport", None), None);
    }

    #[test]
    fn eng_001_tls_from_env() {
        let dir = Path::new("/home/me/.docker");
        assert_eq!(tls_from_env(None, None, Some(dir)), None);
        assert_eq!(tls_from_env(Some(""), None, Some(dir)), None);
        let t = tls_from_env(Some("1"), Some(Path::new("/certs")), Some(dir)).unwrap();
        assert_eq!(t.ca, Path::new("/certs").join("ca.pem"));
        assert!(t.verify);
        let t = tls_from_env(Some("0"), None, Some(dir)).unwrap();
        assert!(!t.verify);
        assert_eq!(t.key, dir.join("key.pem"));
    }

    #[test]
    fn eng_002_parse_context_meta() {
        let meta = json!({
            "Name": "desktop-linux",
            "Metadata": {"Description": "Docker Desktop"},
            "Endpoints": {"docker": {"Host": "npipe:////./pipe/dockerDesktopLinuxEngine", "SkipTLSVerify": false}}
        });
        let (name, ep) = parse_context_meta(&meta, None).unwrap();
        assert_eq!(name, "desktop-linux");
        assert_eq!(
            ep.canonical(),
            EngineEndpoint::NamedPipe {
                path: r"\\.\pipe\dockerDesktopLinuxEngine".into()
            }
            .canonical()
        );

        let remote = json!({"Name": "prod", "Endpoints": {"docker": {"Host": "tcp://prod:2376", "SkipTLSVerify": true}}});
        let (_, ep) = parse_context_meta(&remote, Some(Path::new("/tls/abc/docker"))).unwrap();
        match ep {
            EngineEndpoint::Tcp {
                port, tls: Some(t), ..
            } => {
                assert_eq!(port, 2376);
                assert!(!t.verify);
            }
            other => panic!("unexpected {other:?}"),
        }

        let ssh = json!({"Name": "box", "Endpoints": {"docker": {"Host": "ssh://me@box"}}});
        assert!(matches!(
            parse_context_meta(&ssh, None),
            Some((_, EngineEndpoint::Ssh { .. }))
        ));
        assert!(parse_context_meta(&json!({"Name": "x"}), None).is_none());
        assert!(parse_context_meta(&json!([]), None).is_none());
    }

    #[test]
    fn eng_002_slug_and_ssh_unsupported() {
        assert_eq!(slug("desktop-linux"), "desktop-linux");
        assert_eq!(slug("My Prod (EU)"), "my-prod-eu");
        assert_eq!(slug("--"), "unnamed");
        let d = discovered(
            "context-box".into(),
            "box".into(),
            EngineEndpoint::Ssh {
                url: "ssh://box".into(),
            },
            preference::DOCKER_CONTEXT,
        );
        assert_eq!(
            d.initial_state,
            Some(EngineState::Unsupported {
                reason: SSH_UNSUPPORTED.into()
            })
        );
    }

    #[test]
    fn eng_005_name_from_info() {
        assert_eq!(
            name_from_info(&json!({"OperatingSystem": "Docker Desktop", "Name": "docker-desktop"}))
                .as_deref(),
            Some("Docker Desktop")
        );
        assert_eq!(
            name_from_info(&json!({"OperatingSystem": "OrbStack", "Name": "orbstack"})).as_deref(),
            Some("OrbStack")
        );
        assert_eq!(
            name_from_info(&json!({"Name": "colima", "OperatingSystem": "Ubuntu 24.04"}))
                .as_deref(),
            Some("Colima")
        );
        assert_eq!(
            name_from_info(&json!({"Name": "lima-rancher-desktop"})).as_deref(),
            Some("Rancher Desktop")
        );
        assert_eq!(
            name_from_info(&json!({"Name": "fedora", "Components": [{"Name": "Podman Engine"}]}))
                .as_deref(),
            Some("Podman")
        );
        assert_eq!(
            name_from_info(&json!({"Name": "myhost", "OperatingSystem": "Ubuntu 24.04"})),
            None
        );
    }

    #[test]
    fn eng_009_dedupe_keeps_most_preferred() {
        let pipe = EngineEndpoint::NamedPipe {
            path: r"\\.\pipe\docker_engine".into(),
        };
        let a = discovered(
            "docker-desktop".into(),
            "Docker Desktop".into(),
            pipe.clone(),
            preference::LOCAL_SOCKET,
        );
        let b = discovered(
            "docker-host".into(),
            "DOCKER_HOST".into(),
            pipe.clone(),
            preference::DOCKER_HOST,
        );
        let c = discovered(
            "docker-desktop-linux".into(),
            "x".into(),
            EngineEndpoint::NamedPipe {
                path: r"\\.\pipe\dockerDesktopLinuxEngine".into(),
            },
            preference::LOCAL_SOCKET,
        );
        let out = dedupe(vec![
            (pipe.canonical(), a),
            (pipe.canonical(), b),
            (c.config.endpoint.canonical(), c),
        ]);
        let ids: Vec<_> = out.iter().map(|d| d.config.id.as_str()).collect();
        assert_eq!(ids, vec!["docker-host", "docker-desktop-linux"]);
    }

    #[test]
    fn eng_105_config_schema_keys() {
        let f = DockerFactory::new();
        let schema = f.config_schema();
        let labels: Vec<_> = schema.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Unix socket", "Named pipe", "TCP", "TCP + TLS"]
        );
        let keys: Vec<_> = schema[3].fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, vec!["host", "port", "ca", "cert", "key", "verify"]);
        assert!(f.handles(&EngineEndpoint::Ssh {
            url: "ssh://x".into()
        }));
        assert!(!f.handles(&EngineEndpoint::Unknown));
        assert!(target_for(&EngineEndpoint::Unknown).is_none());
    }

    #[test]
    fn eng_010_connect_rejects_unsupported_endpoints() {
        let f = DockerFactory::new();
        let cfg = |endpoint| EngineConfig {
            id: EngineId::new("x"),
            name: "x".into(),
            endpoint,
            origin: EngineOrigin::Manual,
            enabled: true,
            hidden: false,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(f.connect(&cfg(EngineEndpoint::Ssh {
                url: "ssh://box".into(),
            })))
            .err()
            .unwrap();
        assert!(
            matches!(err, EngineError::Unreachable { ref reason, .. } if reason == "unsupported endpoint")
        );
        let p = rt.block_on(f.probe(&cfg(EngineEndpoint::Unknown)));
        assert!(matches!(p, ProbeResult::Unreachable(_)));
    }
}
