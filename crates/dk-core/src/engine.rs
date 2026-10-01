//! The `Engine` contract (spec 21 §1, §5, §8). Implementations live on the hub runtime only.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::capabilities::Capabilities;
use crate::error::{EngineError, EngineResult};
use crate::model::*;

pub type EngineStream<T> = Pin<Box<dyn Stream<Item = EngineResult<T>> + Send + 'static>>;

/// A one-item stream that yields `err` and ends.
pub fn error_stream<T: Send + 'static>(err: EngineError) -> EngineStream<T> {
    Box::pin(futures::stream::once(async move { Err(err) }))
}

/// Shorthand for an `Unsupported(cap)` stream.
pub fn unsupported_stream<T: Send + 'static>(cap: Capabilities) -> EngineStream<T> {
    error_stream(EngineError::Unsupported(cap))
}

#[async_trait]
pub trait Engine: Send + Sync + 'static {
    // ── identity & health ──────────────────────────────────────────────
    fn id(&self) -> &EngineId;
    fn kind(&self) -> EngineKind;
    /// May change after a transport switch → `HubEvent::CapabilitiesChanged`.
    fn capabilities(&self) -> Capabilities;
    async fn ping(&self) -> EngineResult<()>;
    async fn info(&self) -> EngineResult<EngineInfo>;
    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent>;

    // ── containers ─────────────────────────────────────────────────────
    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>>;
    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails>;
    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()>;
    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()>;
    async fn prune_containers(&self) -> EngineResult<PruneReport>;
    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk>;
    /// Live, ~1 sample / 1–2 s, already normalised to rates.
    fn stats(&self, id: &str) -> EngineStream<StatsSample>;
    /// `Capabilities::TOP`
    async fn top(&self, id: &str) -> EngineResult<ProcessList>;
    /// Consumed by the hub terminal actor, never handed to the UI (spec 10 §3.3).
    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>>;

    // ── images ─────────────────────────────────────────────────────────
    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>>;
    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails>;
    /// `Capabilities::IMAGE_HISTORY`
    async fn image_history(&self, id: &str) -> EngineResult<Vec<ImageLayer>>;
    fn pull_image(&self, reference: &str, auth: Option<RegistryAuth>)
    -> EngineStream<PullProgress>;
    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>>;
    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport>;
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()>;
    /// Returns the new container id.
    async fn run_image(&self, spec: RunSpec) -> EngineResult<String>;

    // ── volumes ────────────────────────────────────────────────────────
    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>>;
    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails>;
    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary>;
    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()>;
    async fn prune_volumes(&self) -> EngineResult<PruneReport>;
    /// `Capabilities::DISK_USAGE`
    async fn disk_usage(&self) -> EngineResult<DiskUsage>;

    // ── networks ───────────────────────────────────────────────────────
    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>>;
    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails>;
    async fn remove_network(&self, id: &str) -> EngineResult<()>;
    async fn prune_networks(&self) -> EngineResult<PruneReport>;
}

/// Interactive exec session (spec 21 §5).
#[async_trait]
pub trait TerminalSession: Send + 'static {
    /// Bytes from the process (PTY output). Ends when the process exits.
    /// Called once; subsequent calls may return an empty stream.
    fn output(&mut self) -> EngineStream<Bytes>;
    /// Send keystrokes / pasted text.
    async fn write(&self, data: Bytes) -> EngineResult<()>;
    /// No-op if the engine lacks `EXEC_RESIZE`.
    async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()>;
    async fn wait(&self) -> EngineResult<Option<i64>>;
    async fn close(&self) -> EngineResult<()>;
}

/// Implemented once per backend crate; registered with the hub at startup (spec 21 §8).
#[async_trait]
pub trait EngineFactory: Send + Sync + 'static {
    fn kind(&self) -> EngineKind;
    /// Whether this factory serves the given endpoint.
    fn handles(&self, endpoint: &EngineEndpoint) -> bool;
    /// Find engines of this kind on the current machine (ENG-001…008, ENG-033).
    async fn discover(&self) -> Vec<DiscoveredEngine>;
    /// Validate a manual/stored config and build a connected engine.
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>>;
    /// Settings UI schema for the "Add engine" dialog.
    fn config_schema(&self) -> Vec<EngineConfigSchema>;
    /// Lightweight background status check for non-active engines (ENG-020).
    /// Must never boot a stopped WSL distro (ENG-106). Default: `Ok(Reachable)`
    /// determined by connect+ping is done by the hub; factories override for cheap checks.
    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        let _ = cfg;
        ProbeResult::Unknown
    }
    /// Explicit user action: start a stopped backend (e.g. boot a WSL distro, ENG-106).
    async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        let _ = cfg;
        Err(EngineError::Api {
            status: 0,
            message: "this engine can't be started from Dockering".into(),
        })
    }
}

/// Result of a discovery probe.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredEngine {
    pub config: EngineConfig,
    /// Discovery-time state hint: e.g. `Stopped` for a stopped WSL distro, `Unsupported` for ssh.
    pub initial_state: Option<EngineState>,
    /// Preference rank for first-launch auto-select (ENG-103): lower = preferred.
    pub preference: u8,
    /// Hidden unless "Show all WSL distros"/"Show all WSLC sessions" (ENG-007 `no-docker`, ENG-109).
    pub show_only_when_all: bool,
}

impl DiscoveredEngine {
    pub fn new(config: EngineConfig, preference: u8) -> Self {
        Self {
            config,
            initial_state: None,
            preference,
            show_only_when_all: false,
        }
    }
}

/// ENG-103 preference ranks.
pub mod preference {
    pub const DOCKER_HOST: u8 = 0;
    pub const DOCKER_CONTEXT: u8 = 10;
    pub const LOCAL_SOCKET: u8 = 20;
    pub const WSL_DISTRO: u8 = 30;
    pub const WSLC: u8 = 40;
    pub const MANUAL: u8 = 50;
}

/// Lightweight status for non-active engines (ENG-020).
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeResult {
    Reachable,
    Unreachable(EngineError),
    /// WSL distro not running.
    Stopped,
    /// Factory has no cheap probe; the hub decides (connect + ping).
    Unknown,
}
