//! Owned DTOs that cross every boundary (spec 21 §3). No protocol types leak through here.
//!
//! Every type is `Clone + Send + 'static` and serde-serialisable. All timestamps are UTC
//! `OffsetDateTime`, all sizes are `u64` bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::capabilities::Capabilities;
use crate::error::EngineError;
use crate::secret::SecretString;

// ───────────────────────────── identity ─────────────────────────────

/// Stable engine slug, e.g. `docker-desktop`, `wsl-ubuntu-22.04`, `wslc-default`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EngineId(pub String);

impl EngineId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EngineId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// ENG-030: open enum; `AppleContainer` is reserved. UI never branches behaviour on it.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineKind {
    Docker,
    WslDistro,
    Wslc,
    AppleContainer,
}

impl EngineKind {
    pub fn label(self) -> &'static str {
        match self {
            EngineKind::Docker => "Docker",
            EngineKind::WslDistro => "WSL distro",
            EngineKind::Wslc => "WSL containers",
            EngineKind::AppleContainer => "macOS containers",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Container,
    Image,
    Volume,
    Network,
    Daemon,
    Engine,
    Session,
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ResourceKind::Container => "container",
            ResourceKind::Image => "image",
            ResourceKind::Volume => "volume",
            ResourceKind::Network => "network",
            ResourceKind::Daemon => "daemon",
            ResourceKind::Engine => "engine",
            ResourceKind::Session => "session",
        })
    }
}

// ───────────────────────────── engine config (spec 20 §1) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsFiles {
    pub ca: PathBuf,
    pub cert: PathBuf,
    pub key: PathBuf,
    #[serde(default = "default_true")]
    pub verify: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WslMode {
    /// Named-pipe ⇄ `wsl.exe … docker system dial-stdio` bridge (ENG-011).
    #[default]
    DialStdio,
    /// dockerd exposed on TCP inside the distro (ENG-012).
    Tcp { port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WslcTransportPref {
    #[default]
    Auto,
    Com,
    Cli,
}

/// Where an engine lives. Stored configs with an unknown variant (written by a newer app)
/// deserialize to `Unknown` and are kept, shown as unsupported (spec 21 §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEndpoint {
    UnixSocket {
        path: PathBuf,
    },
    /// e.g. `\\.\pipe\docker_engine`
    NamedPipe {
        path: String,
    },
    Tcp {
        host: String,
        port: u16,
        #[serde(default)]
        tls: Option<TlsFiles>,
    },
    WslDistro {
        distro: String,
        #[serde(default)]
        mode: WslMode,
    },
    Wslc {
        #[serde(default)]
        session: Option<String>,
        #[serde(default)]
        transport: WslcTransportPref,
    },
    /// Docker context with an `ssh://` host — listed, never contacted in v1 (ENG-010).
    Ssh {
        url: String,
    },
    #[serde(other)]
    Unknown,
}

impl EngineEndpoint {
    /// Canonical string used for discovery de-duplication (ENG-009).
    pub fn canonical(&self) -> String {
        match self {
            EngineEndpoint::UnixSocket { path } => format!("unix://{}", path.display()),
            EngineEndpoint::NamedPipe { path } => {
                format!("npipe://{}", path.replace('/', "\\").to_ascii_lowercase())
            }
            EngineEndpoint::Tcp { host, port, tls } => format!(
                "{}://{}:{}",
                if tls.is_some() { "https" } else { "tcp" },
                host.to_ascii_lowercase(),
                port
            ),
            EngineEndpoint::WslDistro { distro, .. } => {
                format!("wsl://{}", distro.to_ascii_lowercase())
            }
            EngineEndpoint::Wslc { session, .. } => {
                format!("wslc://{}", session.as_deref().unwrap_or("default"))
            }
            EngineEndpoint::Ssh { url } => url.clone(),
            EngineEndpoint::Unknown => "unknown://".into(),
        }
    }

    /// Short user-facing description.
    pub fn display(&self) -> String {
        match self {
            EngineEndpoint::UnixSocket { path } => format!("unix://{}", path.display()),
            EngineEndpoint::NamedPipe { path } => format!("npipe://{path}"),
            EngineEndpoint::Tcp { host, port, tls } => format!(
                "{}://{host}:{port}",
                if tls.is_some() { "https" } else { "tcp" }
            ),
            EngineEndpoint::WslDistro { distro, mode } => match mode {
                WslMode::DialStdio => format!("WSL {distro} (bridge)"),
                WslMode::Tcp { port } => format!("WSL {distro} (tcp :{port})"),
            },
            EngineEndpoint::Wslc { session, .. } => {
                format!("WSLC {}", session.as_deref().unwrap_or("default session"))
            }
            EngineEndpoint::Ssh { url } => url.clone(),
            EngineEndpoint::Unknown => "unknown endpoint".into(),
        }
    }

    /// The engine kind this endpoint is served by, if known.
    pub fn kind(&self) -> Option<EngineKind> {
        match self {
            EngineEndpoint::UnixSocket { .. }
            | EngineEndpoint::NamedPipe { .. }
            | EngineEndpoint::Tcp { .. }
            | EngineEndpoint::Ssh { .. } => Some(EngineKind::Docker),
            EngineEndpoint::WslDistro { .. } => Some(EngineKind::WslDistro),
            EngineEndpoint::Wslc { .. } => Some(EngineKind::Wslc),
            EngineEndpoint::Unknown => None,
        }
    }

    /// Plain TCP without TLS shows a persistent warning (NFR-021).
    pub fn is_insecure_tcp(&self) -> bool {
        matches!(
            self,
            EngineEndpoint::Tcp { tls: None, .. }
                | EngineEndpoint::WslDistro {
                    mode: WslMode::Tcp { .. },
                    ..
                }
        )
    }
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineOrigin {
    #[default]
    Discovered,
    Manual,
}

/// Grouping in the engine switcher (ENG-101 UI): Local / WSL distros / WSL containers / Remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineGroup {
    Local,
    WslDistros,
    WslContainers,
    Remote,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    pub id: EngineId,
    /// User-visible name.
    pub name: String,
    pub endpoint: EngineEndpoint,
    #[serde(default)]
    pub origin: EngineOrigin,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hidden discovered engine (ENG-104 "Hide").
    #[serde(default)]
    pub hidden: bool,
}

impl EngineConfig {
    pub fn group(&self) -> EngineGroup {
        match &self.endpoint {
            EngineEndpoint::UnixSocket { .. } | EngineEndpoint::NamedPipe { .. } => {
                EngineGroup::Local
            }
            EngineEndpoint::Tcp { host, .. } if is_loopback_host(host) => EngineGroup::Local,
            EngineEndpoint::Tcp { .. } | EngineEndpoint::Ssh { .. } => EngineGroup::Remote,
            EngineEndpoint::WslDistro { .. } => EngineGroup::WslDistros,
            EngineEndpoint::Wslc { .. } => EngineGroup::WslContainers,
            EngineEndpoint::Unknown => EngineGroup::Other,
        }
    }
}

/// Fields for the generic "Add engine" dialog (spec 21 §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfigSchema {
    pub kind: EngineKind,
    pub label: String,
    pub fields: Vec<ConfigField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub kind: ConfigFieldKind,
    pub required: bool,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigFieldKind {
    Text,
    Path,
    Port,
    Bool,
    Choice { options: Vec<String> },
}

// ───────────────────────────── engine state & info (spec 21 §3.1) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContainerCounts {
    pub running: u32,
    pub paused: u32,
    pub stopped: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub kind: EngineKind,
    /// e.g. "com", "cli", "xpc", "bridge"; display/diagnostics only.
    pub transport: Option<String>,
    /// Why a fallback transport is in use (ENG-110 info chip).
    #[serde(default)]
    pub transport_note: Option<String>,
    pub server_version: String,
    pub api_version: Option<String>,
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub cpus: Option<u32>,
    pub mem_total: Option<u64>,
    pub containers: ContainerCounts,
    pub images: u32,
    pub storage_driver: Option<String>,
    pub root_dir: Option<String>,
    /// Daemon identity (`/info.ID`) for de-duplication (ENG-009).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// STA-006: max concurrent stats streams for list CPU/memory columns on this engine
    /// (Docker/WSLC-COM 20, WSL-distro bridge 8, WSLC-CLI 0 = columns hidden).
    #[serde(default = "default_list_stats_limit")]
    pub list_stats_limit: u32,
    pub capabilities: Capabilities,
}

fn default_list_stats_limit() -> u32 {
    20
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineState {
    Disconnected,
    Connecting,
    Connected,
    /// Ping failing, reconnecting; data stays visible read-only (SHL-013).
    Degraded,
    Failed {
        error: EngineError,
        /// Milliseconds until the next automatic retry; `None` = no auto retry.
        retry_in_ms: Option<u64>,
    },
    Disabled,
    /// WSL distro not running — offer *Start & connect* (ENG-106).
    Stopped,
    /// ssh context, API < 1.41, unknown endpoint (ENG-010/112).
    Unsupported {
        reason: String,
    },
}

impl EngineState {
    pub fn is_connected(&self) -> bool {
        matches!(self, EngineState::Connected)
    }
    pub fn is_usable(&self) -> bool {
        matches!(self, EngineState::Connected | EngineState::Degraded)
    }
}

/// Broadcast on `hub_events()` (ENG-023).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub config: EngineConfig,
    pub state: EngineState,
    /// Present once connected at least once.
    pub info: Option<EngineInfo>,
    pub active: bool,
    /// Other endpoints reaching the same daemon (ENG-009 tooltip).
    #[serde(default)]
    pub also_reachable_via: Vec<String>,
}

impl EngineStatus {
    pub fn id(&self) -> &EngineId {
        &self.config.id
    }
    pub fn capabilities(&self) -> Capabilities {
        self.info
            .as_ref()
            .map(|i| i.capabilities)
            .unwrap_or_default()
    }
}

// ───────────────────────────── request / option types (spec 21 §3.0) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerQuery {
    pub all: bool,
    pub size: bool,
    pub label_filter: Vec<(String, Option<String>)>,
}

impl Default for ContainerQuery {
    fn default() -> Self {
        Self {
            all: true,
            size: false,
            label_filter: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub kinds: Vec<ResourceKind>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub since: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    #[default]
    Tcp,
    Udp,
    Sctp,
}

impl fmt::Display for Proto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
            Proto::Sctp => "sctp",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortMapping {
    pub ip: Option<IpAddr>,
    pub private: u16,
    pub public: Option<u16>,
    pub proto: Proto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MountKind {
    #[default]
    Volume,
    Bind,
    Tmpfs,
    Npipe,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRequest {
    pub kind: MountKind,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RunSpec {
    pub image: String,
    pub name: Option<String>,
    pub ports: Vec<PortMapping>,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<MountRequest>,
    pub auto_remove: bool,
    pub cmd: Option<Vec<String>>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub name: Option<String>,
    pub driver: Option<String>,
    pub driver_opts: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RegistryAuth {
    pub server: String,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub identity_token: Option<SecretString>,
}

impl fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NFR-020: never print credentials, not even the username.
        f.debug_struct("RegistryAuth")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessList {
    pub titles: Vec<String>,
    pub processes: Vec<Vec<String>>,
}

// ───────────────────────────── containers (spec 21 §3.2) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    #[default]
    #[serde(other)]
    Unknown,
}

impl ContainerState {
    /// Tolerant parse (NFR-031): unknown values map to `Unknown`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "running" | "up" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" | "stopped" => Self::Exited,
            "dead" => Self::Dead,
            _ => Self::Unknown,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Restarting)
    }

    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Created | Self::Exited | Self::Dead)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Created => "Created",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Restarting => "Restarting",
            Self::Removing => "Removing",
            Self::Exited => "Exited",
            Self::Dead => "Dead",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Starting,
    Healthy,
    Unhealthy,
}

impl Health {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "starting" | "health: starting" => Some(Self::Starting),
            "healthy" => Some(Self::Healthy),
            "unhealthy" => Some(Self::Unhealthy),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeInfo {
    pub project: String,
    pub service: String,
    pub number: Option<u32>,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    /// `com.docker.compose.oneoff=True` (CON-013).
    #[serde(default)]
    pub oneoff: bool,
    /// Services this one depends on, from `com.docker.compose.depends_on` (CON-013).
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountSummary {
    pub kind: MountKind,
    /// Volume name for volumes, host path for binds.
    pub source: String,
    pub destination: String,
    pub rw: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerSummary {
    pub id: String,
    /// Without leading '/'.
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub command: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub state: ContainerState,
    /// "Up 3 minutes (healthy)"
    pub status_text: String,
    pub health: Option<Health>,
    pub exit_code: Option<i64>,
    pub ports: Vec<PortMapping>,
    pub labels: BTreeMap<String, String>,
    pub networks: Vec<String>,
    pub ip_addresses: Vec<IpAddr>,
    pub mounts: Vec<MountSummary>,
    pub size_rw: Option<u64>,
    pub size_root_fs: Option<u64>,
    pub compose: Option<ComposeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl EnvVar {
    /// CDT-010: values masked when the key matches `/(?i)pass|secret|token|key/`.
    pub fn is_sensitive(&self) -> bool {
        is_sensitive_key(&self.key)
    }
}

pub fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["pass", "secret", "token", "key"]
        .iter()
        .any(|needle| k.contains(needle))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountDetail {
    pub kind: MountKind,
    pub source: String,
    pub destination: String,
    pub mode: String,
    pub rw: bool,
    pub propagation: Option<String>,
    pub volume_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetworkAttachment {
    pub network: String,
    pub network_id: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub gateway: Option<String>,
    pub mac: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContainerNetworking {
    pub network_mode: Option<String>,
    pub dns: Vec<String>,
    pub networks: Vec<NetworkAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub nano_cpus: Option<i64>,
    pub memory: Option<i64>,
    pub memory_swap: Option<i64>,
    pub pids_limit: Option<i64>,
    pub cpu_shares: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthCheckResult {
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub end: Option<OffsetDateTime>,
    pub exit_code: i64,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerDetails {
    pub summary: ContainerSummary,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub finished_at: Option<OffsetDateTime>,
    pub restart_count: u32,
    pub restart_policy: Option<String>,
    pub pid: Option<u32>,
    pub platform: Option<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub hostname: Option<String>,
    pub tty: bool,
    pub env: Vec<EnvVar>,
    pub mounts: Vec<MountDetail>,
    pub network_settings: ContainerNetworking,
    pub port_bindings: Vec<PortMapping>,
    pub resources: ResourceLimits,
    pub health_log: Vec<HealthCheckResult>,
    /// Full inspect JSON for the Inspect tab (unmasked, CDT-040).
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ContainerAction {
    Start,
    Stop { timeout_s: Option<u32> },
    Restart { timeout_s: Option<u32> },
    Kill { signal: Option<String> },
    Pause,
    Unpause,
}

impl ContainerAction {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop { .. } => "stop",
            Self::Restart { .. } => "restart",
            Self::Kill { .. } => "kill",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
        }
    }

    /// Capability required, if the op isn't universal.
    pub fn required_capability(&self) -> Option<Capabilities> {
        match self {
            Self::Pause | Self::Unpause => Some(Capabilities::PAUSE),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemoveContainerOpts {
    pub force: bool,
    pub volumes: bool,
}

// ───────────────────────────── logs / stats / exec (spec 21 §3.3) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogOpts {
    pub follow: bool,
    /// Default 1000 (LOG-001).
    pub tail: Option<u32>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub since: Option<OffsetDateTime>,
    pub timestamps: bool,
}

impl Default for LogOpts {
    fn default() -> Self {
        Self {
            follow: true,
            tail: Some(1000),
            since: None,
            timestamps: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// TTY containers: raw, not multiplexed (LOG-008).
    Console,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogChunk {
    pub stream: LogStream,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub ts: Option<OffsetDateTime>,
    pub bytes: bytes::Bytes,
}

/// One normalised sample: rates, never cumulative counters (spec 21 §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSample {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// 0..=100*online_cpus (Docker CLI semantics).
    pub cpu_percent: f64,
    pub online_cpus: u32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub net_rx_total: u64,
    pub net_tx_total: u64,
    pub blk_read_bps: f64,
    pub blk_write_bps: f64,
    pub blk_read_total: u64,
    pub blk_write_total: u64,
    pub pids: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequest {
    pub cmd: Vec<String>,
    pub tty: bool,
    pub env: Vec<String>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl ExecRequest {
    /// TRM-004 auto-detect: bash if present, else sh.
    pub fn default_shell_cmd() -> Vec<String> {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "if command -v bash >/dev/null; then exec bash; else exec sh; fi".into(),
        ]
    }
}

impl Default for ExecRequest {
    fn default() -> Self {
        Self {
            cmd: Self::default_shell_cmd(),
            tty: true,
            env: vec!["TERM=xterm-256color".into()],
            user: None,
            working_dir: None,
            cols: 80,
            rows: 24,
        }
    }
}

// ───────────────────────────── images / volumes / networks (spec 21 §3.4) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSummary {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub size: u64,
    pub shared_size: Option<u64>,
    /// Number of containers using it, if the engine reports it.
    pub containers: Option<u32>,
    pub labels: BTreeMap<String, String>,
    pub dangling: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageConfig {
    pub env: Vec<String>,
    pub cmd: Vec<String>,
    pub entrypoint: Vec<String>,
    pub exposed_ports: Vec<String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub volumes: Vec<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageDetails {
    pub summary: ImageSummary,
    pub architecture: String,
    pub os: String,
    pub variant: Option<String>,
    pub author: Option<String>,
    pub config: ImageConfig,
    pub root_fs_layers: Vec<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageLayer {
    pub id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub created_by: String,
    pub size: u64,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PullProgress {
    Layer {
        id: String,
        status: String,
        current: Option<u64>,
        total: Option<u64>,
    },
    Status(String),
    Done {
        digest: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageDeleteItem {
    Untagged(String),
    Deleted(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerRef {
    pub id: String,
    pub name: String,
    /// Mount destination (volumes) or other context.
    pub detail: Option<String>,
    pub rw: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeSummary {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub created: Option<OffsetDateTime>,
    pub scope: String,
    pub labels: BTreeMap<String, String>,
    pub size: Option<u64>,
    pub ref_count: Option<i64>,
    pub compose: Option<ComposeInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeDetails {
    pub summary: VolumeSummary,
    pub options: BTreeMap<String, String>,
    pub status: Option<serde_json::Value>,
    /// Computed from container mounts.
    pub used_by: Vec<ContainerRef>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IpamConfig {
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub ip_range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkSummary {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub attachable: bool,
    pub ipv6: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    pub subnets: Vec<IpamConfig>,
    pub labels: BTreeMap<String, String>,
    pub compose: Option<ComposeInfo>,
    /// Number of attached containers, if known from the list call.
    #[serde(default)]
    pub containers: Option<u32>,
}

impl NetworkSummary {
    /// NET-002: built-in networks can't be deleted.
    pub fn is_builtin(&self) -> bool {
        matches!(self.name.as_str(), "bridge" | "host" | "none")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkEndpoint {
    pub name: String,
    pub id: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkDetails {
    pub summary: NetworkSummary,
    pub containers: Vec<NetworkEndpoint>,
    pub options: BTreeMap<String, String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiskUsage {
    pub images_size: u64,
    pub containers_size: u64,
    /// (volume name, size bytes)
    pub volumes: Vec<(String, u64)>,
    /// (volume name, ref count) — in-use counts for VOL-002.
    #[serde(default)]
    pub volume_refs: Vec<(String, i64)>,
    pub build_cache: u64,
    /// Reclaimable bytes from unused images, if known (IMG-003).
    #[serde(default)]
    pub images_reclaimable: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PruneReport {
    pub deleted: Vec<String>,
    pub space_reclaimed: u64,
}

// ───────────────────────────── events (spec 21 §3.5) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineEvent {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub kind: ResourceKind,
    /// start, die, destroy, pull, create, ...
    pub action: String,
    pub id: String,
    pub attributes: BTreeMap<String, String>,
}
