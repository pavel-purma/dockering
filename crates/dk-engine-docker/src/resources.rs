//! Image, volume, and network ops (spec 21 §6). Filled in by item 4.

use dk_core::{
    DiskUsage, EngineError, EngineResult, EngineStream, ImageDeleteItem, ImageDetails, ImageLayer,
    ImageSummary, NetworkDetails, NetworkSummary, PruneReport, PullProgress, RegistryAuth, RunSpec,
    VolumeDetails, VolumeSpec, VolumeSummary, error_stream,
};

use crate::DockerEngine;

fn todo_err() -> EngineError {
    EngineError::protocol("not implemented yet")
}

impl DockerEngine {
    pub(crate) async fn list_images_impl(&self) -> EngineResult<Vec<ImageSummary>> {
        Err(todo_err())
    }
    pub(crate) async fn inspect_image_impl(&self, _id: &str) -> EngineResult<ImageDetails> {
        Err(todo_err())
    }
    pub(crate) async fn image_history_impl(&self, _id: &str) -> EngineResult<Vec<ImageLayer>> {
        Err(todo_err())
    }
    pub(crate) fn pull_image_stream(
        &self,
        _r: &str,
        _a: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        error_stream(todo_err())
    }
    pub(crate) async fn remove_image_impl(
        &self,
        _id: &str,
        _force: bool,
    ) -> EngineResult<Vec<ImageDeleteItem>> {
        Err(todo_err())
    }
    pub(crate) async fn prune_images_impl(&self, _d: bool) -> EngineResult<PruneReport> {
        Err(todo_err())
    }
    pub(crate) async fn tag_image_impl(&self, _id: &str, _r: &str, _t: &str) -> EngineResult<()> {
        Err(todo_err())
    }
    pub(crate) async fn run_image_impl(&self, _s: RunSpec) -> EngineResult<String> {
        Err(todo_err())
    }
    pub(crate) async fn list_volumes_impl(&self) -> EngineResult<Vec<VolumeSummary>> {
        Err(todo_err())
    }
    pub(crate) async fn inspect_volume_impl(&self, _n: &str) -> EngineResult<VolumeDetails> {
        Err(todo_err())
    }
    pub(crate) async fn create_volume_impl(&self, _s: VolumeSpec) -> EngineResult<VolumeSummary> {
        Err(todo_err())
    }
    pub(crate) async fn remove_volume_impl(&self, _n: &str, _f: bool) -> EngineResult<()> {
        Err(todo_err())
    }
    pub(crate) async fn prune_volumes_impl(&self) -> EngineResult<PruneReport> {
        Err(todo_err())
    }
    pub(crate) async fn disk_usage_impl(&self) -> EngineResult<DiskUsage> {
        Err(todo_err())
    }
    pub(crate) async fn list_networks_impl(&self) -> EngineResult<Vec<NetworkSummary>> {
        Err(todo_err())
    }
    pub(crate) async fn inspect_network_impl(&self, _id: &str) -> EngineResult<NetworkDetails> {
        Err(todo_err())
    }
    pub(crate) async fn remove_network_impl(&self, _id: &str) -> EngineResult<()> {
        Err(todo_err())
    }
    pub(crate) async fn prune_networks_impl(&self) -> EngineResult<PruneReport> {
        Err(todo_err())
    }
}
