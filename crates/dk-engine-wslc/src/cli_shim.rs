//! TEMPORARY merge shim — DELETE when `wip/wslc-cli` is merged.
//!
//! The fixed CLI API says `cli::WslcCliEngine` implements `dk_core::Engine`, but on this branch
//! `src/cli/` is still the skeleton (owned by `engine-integrator`). This impl only exists so
//! the factory compiles here; the skeleton's `connect()` always errors, so none of these
//! methods is reachable. After the merge the compiler reports a conflicting `impl Engine`;
//! resolve it by deleting this file and the `mod cli_shim;` line in `lib.rs`.

use async_trait::async_trait;
use dk_core::*;

use crate::cli::WslcCliEngine;

fn not_impl() -> EngineError {
    EngineError::unreachable("WSLC CLI transport not implemented on this branch")
}

#[async_trait]
impl Engine for WslcCliEngine {
    fn id(&self) -> &EngineId {
        static ID: std::sync::OnceLock<EngineId> = std::sync::OnceLock::new();
        ID.get_or_init(|| EngineId::new("wslc-cli-shim"))
    }
    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
    async fn ping(&self) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn info(&self) -> EngineResult<EngineInfo> {
        Err(not_impl())
    }
    fn events(&self, _f: EventFilter) -> EngineStream<EngineEvent> {
        error_stream(not_impl())
    }
    async fn list_containers(&self, _q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        Err(not_impl())
    }
    async fn inspect_container(&self, _id: &str) -> EngineResult<ContainerDetails> {
        Err(not_impl())
    }
    async fn container_action(&self, _id: &str, _a: ContainerAction) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn remove_container(&self, _id: &str, _o: RemoveContainerOpts) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        Err(not_impl())
    }
    fn logs(&self, _id: &str, _o: LogOpts) -> EngineStream<LogChunk> {
        error_stream(not_impl())
    }
    fn stats(&self, _id: &str) -> EngineStream<StatsSample> {
        error_stream(not_impl())
    }
    async fn top(&self, _id: &str) -> EngineResult<ProcessList> {
        Err(not_impl())
    }
    async fn exec(&self, _id: &str, _r: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        Err(not_impl())
    }
    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        Err(not_impl())
    }
    async fn inspect_image(&self, _id: &str) -> EngineResult<ImageDetails> {
        Err(not_impl())
    }
    async fn image_history(&self, _id: &str) -> EngineResult<Vec<ImageLayer>> {
        Err(not_impl())
    }
    fn pull_image(&self, _r: &str, _a: Option<RegistryAuth>) -> EngineStream<PullProgress> {
        error_stream(not_impl())
    }
    async fn remove_image(&self, _id: &str, _f: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        Err(not_impl())
    }
    async fn prune_images(&self, _d: bool) -> EngineResult<PruneReport> {
        Err(not_impl())
    }
    async fn tag_image(&self, _id: &str, _r: &str, _t: &str) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn run_image(&self, _s: RunSpec) -> EngineResult<String> {
        Err(not_impl())
    }
    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        Err(not_impl())
    }
    async fn inspect_volume(&self, _n: &str) -> EngineResult<VolumeDetails> {
        Err(not_impl())
    }
    async fn create_volume(&self, _s: VolumeSpec) -> EngineResult<VolumeSummary> {
        Err(not_impl())
    }
    async fn remove_volume(&self, _n: &str, _f: bool) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        Err(not_impl())
    }
    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        Err(not_impl())
    }
    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        Err(not_impl())
    }
    async fn inspect_network(&self, _id: &str) -> EngineResult<NetworkDetails> {
        Err(not_impl())
    }
    async fn remove_network(&self, _id: &str) -> EngineResult<()> {
        Err(not_impl())
    }
    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        Err(not_impl())
    }
}
