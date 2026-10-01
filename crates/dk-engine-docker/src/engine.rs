//! `DockerEngine` (bollard). Spec 20 §3, 21 §6.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::{ClientVersion, Docker};
use dk_core::{
    Capabilities, ContainerAction, ContainerCounts, ContainerDetails, ContainerQuery,
    ContainerSummary, DiskUsage, Engine, EngineError, EngineEvent, EngineId, EngineInfo,
    EngineKind, EngineResult, EngineStream, EventFilter, ExecRequest, ImageDeleteItem,
    ImageDetails, ImageLayer, ImageSummary, LogChunk, LogOpts, NetworkDetails, NetworkSummary,
    ProcessList, PruneReport, PullProgress, RegistryAuth, RemoveContainerOpts, ResourceKind,
    RunSpec, StatsSample, TerminalSession, VolumeDetails, VolumeSpec, VolumeSummary,
};
use serde_json::Value;

use crate::connect::{self, Negotiated};
use crate::errors::{ErrCtx, map_err};
use crate::{DockerEngineOptions, DockerTarget};

/// STA-006: concurrent list stats streams per engine.
const LIST_STATS_LIMIT: u32 = 20;
const LIST_STATS_LIMIT_WSL: u32 = 8;

pub struct DockerEngine {
    pub(crate) id: EngineId,
    pub(crate) kind: EngineKind,
    pub(crate) docker: Docker,
    pub(crate) api: ClientVersion,
    pub(crate) server_version: Option<String>,
    pub(crate) transport: String,
    pub(crate) ctx: ErrCtx,
    _keepalive: Option<Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for DockerEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerEngine")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("api", &connect::version_string(&self.api))
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl DockerEngine {
    /// Connect, negotiate the API version (`min(server, client)`, require ≥ 1.41), and
    /// compute capabilities. Fails with `Unreachable{hint}` when the daemon can't be reached,
    /// `Api{status: 0, ..}`-style "unsupported version" error for API < 1.41.
    pub async fn connect(
        id: EngineId,
        target: DockerTarget,
        opts: DockerEngineOptions,
    ) -> EngineResult<DockerEngine> {
        Self::connect_with_timeout(id, target, opts, connect::REQUEST_TIMEOUT).await
    }

    pub(crate) async fn connect_with_timeout(
        id: EngineId,
        target: DockerTarget,
        opts: DockerEngineOptions,
        timeout: Duration,
    ) -> EngineResult<DockerEngine> {
        let kind = opts.kind.unwrap_or(EngineKind::Docker);
        let ctx = ErrCtx {
            hint: connect::hint_ctx(&target, kind == EngineKind::WslDistro),
            timeout,
        };
        let max = connect::client_max();
        let probe = connect::build_client(&target, timeout, &max, &ctx)?;
        let Negotiated {
            api,
            server_version,
        } = connect::negotiate(&probe, &ctx).await?;
        let docker = if api == max {
            probe
        } else {
            connect::build_client(&target, timeout, &api, &ctx)?
        };
        tracing::debug!(
            engine = %id,
            api = %connect::version_string(&api),
            "docker engine connected"
        );
        Ok(DockerEngine {
            id,
            kind,
            docker,
            api,
            server_version,
            transport: opts
                .transport
                .unwrap_or_else(|| connect::transport_label(&target).to_owned()),
            ctx,
            _keepalive: opts.keepalive,
        })
    }

    /// Map a bollard error; `nf` names what a 404 refers to.
    pub(crate) fn err(&self, e: bollard::errors::Error) -> EngineError {
        map_err(e, &self.ctx, None)
    }

    pub(crate) fn err_nf(
        &self,
        e: bollard::errors::Error,
        kind: ResourceKind,
        id: &str,
    ) -> EngineError {
        map_err(e, &self.ctx, Some((kind, id)))
    }

    /// Negotiated API version ≥ `major.minor`.
    pub(crate) fn api_at_least(&self, major: usize, minor: usize) -> bool {
        self.api
            >= ClientVersion {
                major_version: major,
                minor_version: minor,
            }
    }

    /// Bollard model → JSON (for the shared `docker_json` mappers).
    pub(crate) fn to_value<T: serde::Serialize>(v: &T) -> EngineResult<Value> {
        serde_json::to_value(v).map_err(EngineError::from)
    }
}

/// `x86_64` → `amd64`, `aarch64` → `arm64` (Docker naming).
pub(crate) fn normalize_arch(arch: &str) -> String {
    match arch.trim() {
        "x86_64" | "x86-64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        "armv7l" | "armhf" => "arm".into(),
        other => other.to_owned(),
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn u32_field(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(Value::as_i64)
        .and_then(|n| u32::try_from(n).ok())
}

/// `/info` JSON → `EngineInfo` (pure; tested on fixtures).
pub(crate) fn engine_info_from_json(
    info: &Value,
    kind: EngineKind,
    transport: &str,
    api_version: &str,
    server_version: Option<&str>,
) -> EngineInfo {
    let name = str_field(info, "Name")
        .or_else(|| str_field(info, "OperatingSystem"))
        .unwrap_or_else(|| "Docker".into());
    EngineInfo {
        name,
        kind,
        transport: Some(transport.to_owned()),
        transport_note: None,
        server_version: str_field(info, "ServerVersion")
            .or_else(|| server_version.map(ToOwned::to_owned))
            .unwrap_or_default(),
        api_version: Some(api_version.to_owned()),
        os: str_field(info, "OSType").unwrap_or_else(|| "linux".into()),
        arch: normalize_arch(&str_field(info, "Architecture").unwrap_or_default()),
        kernel: str_field(info, "KernelVersion"),
        cpus: u32_field(info, "NCPU").filter(|n| *n > 0),
        mem_total: info
            .get("MemTotal")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0),
        containers: ContainerCounts {
            running: u32_field(info, "ContainersRunning").unwrap_or(0),
            paused: u32_field(info, "ContainersPaused").unwrap_or(0),
            stopped: u32_field(info, "ContainersStopped").unwrap_or(0),
        },
        images: u32_field(info, "Images").unwrap_or(0),
        storage_driver: str_field(info, "Driver"),
        root_dir: str_field(info, "DockerRootDir"),
        daemon_id: str_field(info, "ID"),
        list_stats_limit: if kind == EngineKind::WslDistro {
            LIST_STATS_LIMIT_WSL
        } else {
            LIST_STATS_LIMIT
        },
        capabilities: Capabilities::DOCKER,
    }
}

#[async_trait]
impl Engine for DockerEngine {
    fn id(&self) -> &EngineId {
        &self.id
    }

    fn kind(&self) -> EngineKind {
        self.kind
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::DOCKER
    }

    async fn ping(&self) -> EngineResult<()> {
        self.docker
            .ping()
            .await
            .map(|_| ())
            .map_err(|e| self.err(e))
    }

    async fn info(&self) -> EngineResult<EngineInfo> {
        let info = self.docker.info().await.map_err(|e| self.err(e))?;
        let v = Self::to_value(&info)?;
        Ok(engine_info_from_json(
            &v,
            self.kind,
            &self.transport,
            &connect::version_string(&self.api),
            self.server_version.as_deref(),
        ))
    }

    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent> {
        self.events_stream(filter)
    }

    // ── containers ─────────────────────────────────────────────────────
    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        self.list_containers_impl(q).await
    }

    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails> {
        self.inspect_container_impl(id).await
    }

    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        self.container_action_impl(id, action).await
    }

    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        self.remove_container_impl(id, opts).await
    }

    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        self.prune_containers_impl().await
    }

    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        self.logs_stream(id, opts)
    }

    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        self.stats_stream(id)
    }

    async fn top(&self, id: &str) -> EngineResult<ProcessList> {
        self.top_impl(id).await
    }

    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        self.exec_impl(id, req).await
    }

    // ── images ─────────────────────────────────────────────────────────
    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        self.list_images_impl().await
    }

    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails> {
        self.inspect_image_impl(id).await
    }

    async fn image_history(&self, id: &str) -> EngineResult<Vec<ImageLayer>> {
        self.image_history_impl(id).await
    }

    fn pull_image(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        self.pull_image_stream(reference, auth)
    }

    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        self.remove_image_impl(id, force).await
    }

    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        self.prune_images_impl(dangling_only).await
    }

    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        self.tag_image_impl(id, repo, tag).await
    }

    async fn run_image(&self, spec: RunSpec) -> EngineResult<String> {
        self.run_image_impl(spec).await
    }

    // ── volumes ────────────────────────────────────────────────────────
    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        self.list_volumes_impl().await
    }

    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails> {
        self.inspect_volume_impl(name).await
    }

    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        self.create_volume_impl(spec).await
    }

    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        self.remove_volume_impl(name, force).await
    }

    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        self.prune_volumes_impl().await
    }

    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        self.disk_usage_impl().await
    }

    // ── networks ───────────────────────────────────────────────────────
    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        self.list_networks_impl().await
    }

    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails> {
        self.inspect_network_impl(id).await
    }

    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        self.remove_network_impl(id).await
    }

    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        self.prune_networks_impl().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eng_108_engine_info_from_json() {
        let v = json!({
            "ID": "abcd", "Name": "docker-desktop", "OperatingSystem": "Docker Desktop",
            "OSType": "linux", "Architecture": "x86_64", "KernelVersion": "6.6.87",
            "NCPU": 16, "MemTotal": 33_000_000_000_u64, "ContainersRunning": 5,
            "ContainersPaused": 0, "ContainersStopped": 1, "Images": 8, "Driver": "overlayfs",
            "DockerRootDir": "/var/lib/docker", "ServerVersion": "29.8.1"
        });
        let i = engine_info_from_json(&v, EngineKind::Docker, "npipe", "1.53", None);
        assert_eq!(i.name, "docker-desktop");
        assert_eq!(i.arch, "amd64");
        assert_eq!(i.cpus, Some(16));
        assert_eq!(i.containers.running, 5);
        assert_eq!(i.daemon_id.as_deref(), Some("abcd"));
        assert_eq!(i.transport.as_deref(), Some("npipe"));
        assert_eq!(i.list_stats_limit, 20);
        assert_eq!(i.capabilities, Capabilities::DOCKER);

        let w = engine_info_from_json(
            &json!({"OperatingSystem": "Ubuntu 24.04", "Architecture": "aarch64"}),
            EngineKind::WslDistro,
            "bridge",
            "1.47",
            Some("27.5.1"),
        );
        assert_eq!(w.name, "Ubuntu 24.04");
        assert_eq!(w.arch, "arm64");
        assert_eq!(w.server_version, "27.5.1");
        assert_eq!(w.kind, EngineKind::WslDistro);
        assert_eq!(w.list_stats_limit, 8);
        assert_eq!(w.cpus, None);
    }
}
