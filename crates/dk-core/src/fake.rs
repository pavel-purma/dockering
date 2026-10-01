//! `FakeEngine`: a scriptable in-memory engine for hub and view tests (spec 60). View tests
//! MUST NOT need Docker. No tokio here: latency uses a helper thread.
//!
//! Behaves like a small engine: actions mutate state and emit events; logs/stats/events are
//! live streams fed by `push_*`/`emit_event`; exec is a loopback TTY (echoes input).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use time::OffsetDateTime;

use crate::capabilities::Capabilities;
use crate::engine::{
    DiscoveredEngine, Engine, EngineFactory, EngineStream, ProbeResult, TerminalSession,
};
use crate::error::{EngineError, EngineResult};
use crate::model::*;

/// One recorded engine call, for assertions (`fake.calls()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCall {
    pub op: &'static str,
    /// Primary argument (id, name, reference) or empty.
    pub arg: String,
}

#[derive(Default)]
struct State {
    caps: Capabilities,
    latency: Duration,
    containers: Vec<ContainerSummary>,
    details: HashMap<String, ContainerDetails>,
    images: Vec<ImageSummary>,
    volumes: Vec<VolumeSummary>,
    networks: Vec<NetworkSummary>,
    logs: HashMap<String, Vec<LogChunk>>,
    top: HashMap<String, ProcessList>,
    disk_usage: Option<DiskUsage>,
    errors: HashMap<&'static str, EngineError>,
    errors_once: HashMap<&'static str, EngineError>,
    panics: HashSet<&'static str>,
    calls: Vec<FakeCall>,
    event_subs: Vec<mpsc::UnboundedSender<EngineResult<EngineEvent>>>,
    log_subs: Vec<(String, mpsc::UnboundedSender<EngineResult<LogChunk>>)>,
    stats_subs: Vec<(String, mpsc::UnboundedSender<EngineResult<StatsSample>>)>,
    exec_count: usize,
    next_id: u64,
    info_name: String,
}

/// Scriptable in-memory engine.
pub struct FakeEngine {
    id: EngineId,
    kind: EngineKind,
    state: Mutex<State>,
}

impl FakeEngine {
    pub fn new(id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self::build(id.into()))
    }

    fn build(id: String) -> Self {
        Self {
            kind: EngineKind::Docker,
            state: Mutex::new(State {
                caps: Capabilities::all(),
                info_name: format!("fake {id}"),
                next_id: 1,
                ..Default::default()
            }),
            id: EngineId(id),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        // A panicking test op must not poison the fake for later assertions.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── scripting ────────────────────────────────────────────────────────────

    pub fn set_capabilities(&self, caps: Capabilities) {
        self.st().caps = caps;
    }
    /// Every async op waits this long first (helper thread; no tokio needed).
    pub fn set_latency(&self, d: Duration) {
        self.st().latency = d;
    }
    pub fn set_containers(&self, v: Vec<ContainerSummary>) {
        self.st().containers = v;
    }
    pub fn set_container_details(&self, d: ContainerDetails) {
        self.st().details.insert(d.summary.id.clone(), d);
    }
    pub fn set_images(&self, v: Vec<ImageSummary>) {
        self.st().images = v;
    }
    pub fn set_volumes(&self, v: Vec<VolumeSummary>) {
        self.st().volumes = v;
    }
    pub fn set_networks(&self, v: Vec<NetworkSummary>) {
        self.st().networks = v;
    }
    pub fn set_logs(&self, container_id: &str, chunks: Vec<LogChunk>) {
        self.st().logs.insert(container_id.to_owned(), chunks);
    }
    pub fn set_top(&self, container_id: &str, p: ProcessList) {
        self.st().top.insert(container_id.to_owned(), p);
    }
    pub fn set_disk_usage(&self, d: DiskUsage) {
        self.st().disk_usage = Some(d);
    }
    /// Persistent failure of `op` (method name, e.g. `"list_containers"`); `None` clears it.
    pub fn set_error(&self, op: &'static str, err: Option<EngineError>) {
        match err {
            Some(e) => self.st().errors.insert(op, e),
            None => self.st().errors.remove(op),
        };
    }
    /// The next call of `op` fails with `err`.
    pub fn fail_once(&self, op: &'static str, err: EngineError) {
        self.st().errors_once.insert(op, err);
    }
    /// `op` panics (NFR-030 tests).
    pub fn panic_on(&self, op: &'static str) {
        self.st().panics.insert(op);
    }
    pub fn calls(&self) -> Vec<FakeCall> {
        self.st().calls.clone()
    }
    pub fn calls_to(&self, op: &str) -> Vec<FakeCall> {
        self.st().calls.iter().filter(|c| c.op == op).cloned().collect()
    }
    pub fn clear_calls(&self) {
        self.st().calls.clear();
    }
    /// Number of currently open subscriber streams per kind: (events, logs, stats).
    pub fn open_streams(&self) -> (usize, usize, usize) {
        let mut st = self.st();
        st.event_subs.retain(|s| !s.is_closed());
        st.log_subs.retain(|(_, s)| !s.is_closed());
        st.stats_subs.retain(|(_, s)| !s.is_closed());
        (st.event_subs.len(), st.log_subs.len(), st.stats_subs.len())
    }
    pub fn exec_count(&self) -> usize {
        self.st().exec_count
    }

    // ── live feeds ───────────────────────────────────────────────────────────

    pub fn emit_event(&self, e: EngineEvent) {
        self.st()
            .event_subs
            .retain(|s| s.unbounded_send(Ok(e.clone())).is_ok());
    }
    /// Ends all event streams with `err` (simulates a dropped connection).
    pub fn fail_event_streams(&self, err: EngineError) {
        for s in self.st().event_subs.drain(..) {
            let _ = s.unbounded_send(Err(err.clone()));
        }
    }
    pub fn push_log(&self, container_id: &str, chunk: LogChunk) {
        let mut st = self.st();
        st.logs
            .entry(container_id.to_owned())
            .or_default()
            .push(chunk.clone());
        st.log_subs
            .retain(|(id, s)| id != container_id || s.unbounded_send(Ok(chunk.clone())).is_ok());
    }
    /// Ends the log streams of a container (as when it stops).
    pub fn end_logs(&self, container_id: &str) {
        self.st().log_subs.retain(|(id, _)| id != container_id);
    }
    pub fn push_stats(&self, container_id: &str, s: StatsSample) {
        self.st()
            .stats_subs
            .retain(|(id, tx)| id != container_id || tx.unbounded_send(Ok(s.clone())).is_ok());
    }

    // ── internals ────────────────────────────────────────────────────────────

    async fn enter(&self, op: &'static str, arg: &str) -> EngineResult<()> {
        let (latency, panic) = {
            let mut st = self.st();
            st.calls.push(FakeCall {
                op,
                arg: arg.to_owned(),
            });
            (st.latency, st.panics.contains(op))
        };
        if !latency.is_zero() {
            sleep(latency).await;
        }
        if panic {
            panic!("FakeEngine: scripted panic in {op}");
        }
        let mut st = self.st();
        if let Some(e) = st.errors_once.remove(op) {
            return Err(e);
        }
        if let Some(e) = st.errors.get(op) {
            return Err(e.clone());
        }
        Ok(())
    }

    fn require(&self, cap: Capabilities) -> EngineResult<()> {
        if self.st().caps.contains(cap) {
            Ok(())
        } else {
            Err(EngineError::Unsupported(cap))
        }
    }

    fn find_container(st: &State, id: &str) -> Option<usize> {
        st.containers
            .iter()
            .position(|c| c.id == id || c.name == id || (id.len() >= 4 && c.id.starts_with(id)))
    }

    fn emit_locked(st: &mut State, kind: ResourceKind, action: &str, id: &str) {
        let e = EngineEvent {
            at: OffsetDateTime::now_utc(),
            kind,
            action: action.to_owned(),
            id: id.to_owned(),
            attributes: BTreeMap::new(),
        };
        st.event_subs
            .retain(|s| s.unbounded_send(Ok(e.clone())).is_ok());
    }

    fn new_id(st: &mut State) -> String {
        st.next_id += 1;
        format!("{:064x}", st.next_id * 0x9e37_79b9_7f4a_7c15)
    }
}

/// Executor-agnostic sleep for the fake (helper thread + oneshot).
async fn sleep(d: Duration) {
    let (tx, rx) = oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let _ = tx.send(());
    });
    let _ = rx.await;
}

#[async_trait]
impl Engine for FakeEngine {
    fn id(&self) -> &EngineId {
        &self.id
    }
    fn kind(&self) -> EngineKind {
        self.kind
    }
    fn capabilities(&self) -> Capabilities {
        self.st().caps
    }
    async fn ping(&self) -> EngineResult<()> {
        self.enter("ping", "").await
    }
    async fn info(&self) -> EngineResult<EngineInfo> {
        self.enter("info", "").await?;
        let st = self.st();
        let mut counts = ContainerCounts::default();
        for c in &st.containers {
            match c.state {
                ContainerState::Running | ContainerState::Restarting => counts.running += 1,
                ContainerState::Paused => counts.paused += 1,
                _ => counts.stopped += 1,
            }
        }
        Ok(EngineInfo {
            name: st.info_name.clone(),
            kind: self.kind,
            transport: Some("fake".into()),
            transport_note: None,
            server_version: "27.3.1".into(),
            api_version: Some("1.47".into()),
            os: "linux".into(),
            arch: "amd64".into(),
            kernel: Some("6.6.0-fake".into()),
            cpus: Some(8),
            mem_total: Some(16 * 1024 * 1024 * 1024),
            containers: counts,
            images: st.images.len() as u32,
            storage_driver: Some("overlay2".into()),
            root_dir: Some("/var/lib/docker".into()),
            daemon_id: Some(format!("FAKE-{}", self.id)),
            capabilities: st.caps,
        })
    }
    fn events(&self, _filter: EventFilter) -> EngineStream<EngineEvent> {
        let (tx, rx) = mpsc::unbounded();
        let mut st = self.st();
        st.calls.push(FakeCall {
            op: "events",
            arg: String::new(),
        });
        if let Some(e) = st.errors.get("events").cloned() {
            return crate::engine::error_stream(e);
        }
        if !st.caps.contains(Capabilities::EVENTS) {
            return crate::engine::unsupported_stream(Capabilities::EVENTS);
        }
        st.event_subs.push(tx);
        Box::pin(rx)
    }

    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        self.enter("list_containers", "").await?;
        let st = self.st();
        Ok(st
            .containers
            .iter()
            .filter(|c| q.all || c.state.is_running())
            .cloned()
            .collect())
    }
    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails> {
        self.enter("inspect_container", id).await?;
        let st = self.st();
        let idx = Self::find_container(&st, id)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Container, id))?;
        let summary = st.containers[idx].clone();
        Ok(match st.details.get(&summary.id) {
            Some(d) => ContainerDetails {
                summary,
                ..d.clone()
            },
            None => fixtures::details_for(summary),
        })
    }
    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        self.enter(
            match action {
                ContainerAction::Start => "start",
                ContainerAction::Stop { .. } => "stop",
                ContainerAction::Restart { .. } => "restart",
                ContainerAction::Kill { .. } => "kill",
                ContainerAction::Pause => "pause",
                ContainerAction::Unpause => "unpause",
            },
            id,
        )
        .await?;
        if let Some(cap) = action.required_capability() {
            self.require(cap)?;
        }
        let mut st = self.st();
        let idx = Self::find_container(&st, id)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Container, id))?;
        let (new_state, status, ev): (ContainerState, String, &str) = match action {
            ContainerAction::Start | ContainerAction::Restart { .. } => {
                (ContainerState::Running, "Up Less than a second".into(), "start")
            }
            ContainerAction::Stop { .. } => {
                (ContainerState::Exited, "Exited (0) Less than a second ago".into(), "die")
            }
            ContainerAction::Kill { .. } => (
                ContainerState::Exited,
                "Exited (137) Less than a second ago".into(),
                "kill",
            ),
            ContainerAction::Pause => (ContainerState::Paused, "Up (Paused)".into(), "pause"),
            ContainerAction::Unpause => (ContainerState::Running, "Up".into(), "unpause"),
        };
        let c = &mut st.containers[idx];
        c.state = new_state;
        c.status_text = status;
        c.exit_code = match action {
            ContainerAction::Stop { .. } => Some(0),
            ContainerAction::Kill { .. } => Some(137),
            _ => None,
        };
        let cid = c.id.clone();
        if new_state == ContainerState::Exited {
            st.log_subs.retain(|(id, _)| *id != cid);
        }
        Self::emit_locked(&mut st, ResourceKind::Container, ev, &cid);
        Ok(())
    }
    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        self.enter("remove_container", id).await?;
        let mut st = self.st();
        let idx = Self::find_container(&st, id)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Container, id))?;
        if st.containers[idx].state.is_running() && !opts.force {
            return Err(EngineError::Conflict(format!(
                "You cannot remove a running container {id}. Stop the container before attempting removal or force remove"
            )));
        }
        let c = st.containers.remove(idx);
        Self::emit_locked(&mut st, ResourceKind::Container, "destroy", &c.id);
        Ok(())
    }
    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        self.enter("prune_containers", "").await?;
        let mut st = self.st();
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.containers)
            .into_iter()
            .partition(|c| c.state.is_stopped());
        st.containers = keep;
        let deleted: Vec<String> = gone.iter().map(|c| c.id.clone()).collect();
        for id in &deleted {
            Self::emit_locked(&mut st, ResourceKind::Container, "destroy", id);
        }
        Ok(PruneReport {
            space_reclaimed: 1000 * deleted.len() as u64,
            deleted,
        })
    }
    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        let mut st = self.st();
        st.calls.push(FakeCall {
            op: "logs",
            arg: id.to_owned(),
        });
        if let Some(e) = st.errors.get("logs").cloned() {
            return crate::engine::error_stream(e);
        }
        let Some(idx) = Self::find_container(&st, id) else {
            return crate::engine::error_stream(EngineError::not_found(
                ResourceKind::Container,
                id,
            ));
        };
        let cid = st.containers[idx].id.clone();
        let running = st.containers[idx].state.is_running();
        let mut history: Vec<LogChunk> = st.logs.get(&cid).cloned().unwrap_or_default();
        if let Some(since) = opts.since {
            history.retain(|c| c.ts.is_none_or(|t| t > since));
        }
        if let Some(tail) = opts.tail {
            let skip = history.len().saturating_sub(tail as usize);
            history.drain(..skip);
        }
        let replay = futures::stream::iter(history.into_iter().map(Ok));
        if opts.follow && running {
            let (tx, rx) = mpsc::unbounded();
            st.log_subs.push((cid, tx));
            Box::pin(replay.chain(rx))
        } else {
            Box::pin(replay)
        }
    }
    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        let mut st = self.st();
        st.calls.push(FakeCall {
            op: "stats",
            arg: id.to_owned(),
        });
        if let Some(e) = st.errors.get("stats").cloned() {
            return crate::engine::error_stream(e);
        }
        let Some(idx) = Self::find_container(&st, id) else {
            return crate::engine::error_stream(EngineError::not_found(
                ResourceKind::Container,
                id,
            ));
        };
        let cid = st.containers[idx].id.clone();
        let (tx, rx) = mpsc::unbounded();
        st.stats_subs.push((cid, tx));
        Box::pin(rx)
    }
    async fn top(&self, id: &str) -> EngineResult<ProcessList> {
        self.enter("top", id).await?;
        self.require(Capabilities::TOP)?;
        let st = self.st();
        Ok(st.top.get(id).cloned().unwrap_or_else(|| ProcessList {
            titles: vec!["PID".into(), "USER".into(), "COMMAND".into()],
            processes: vec![vec!["1".into(), "root".into(), "/bin/sh".into()]],
        }))
    }
    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        self.enter("exec", id).await?;
        self.require(Capabilities::EXEC_TTY)?;
        let mut st = self.st();
        let idx = Self::find_container(&st, id)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Container, id))?;
        if !st.containers[idx].state.is_running() {
            return Err(EngineError::Conflict(format!("container {id} is not running")));
        }
        st.exec_count += 1;
        Ok(Box::new(FakeTerminal::new(req.cols, req.rows)))
    }

    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        self.enter("list_images", "").await?;
        let st = self.st();
        let mut images = st.images.clone();
        for img in &mut images {
            let n = st.containers.iter().filter(|c| c.image_id == img.id).count() as u32;
            img.containers = Some(n);
        }
        Ok(images)
    }
    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails> {
        self.enter("inspect_image", id).await?;
        let st = self.st();
        let img = st
            .images
            .iter()
            .find(|i| i.id == id || i.repo_tags.iter().any(|t| t == id) || i.id.contains(id))
            .cloned()
            .ok_or_else(|| EngineError::not_found(ResourceKind::Image, id))?;
        Ok(ImageDetails {
            raw: serde_json::json!({ "Id": img.id, "RepoTags": img.repo_tags }),
            architecture: "amd64".into(),
            os: "linux".into(),
            variant: None,
            author: None,
            config: ImageConfig {
                cmd: vec!["/bin/sh".into()],
                ..Default::default()
            },
            root_fs_layers: vec![format!("sha256:{:064x}", 1)],
            summary: img,
        })
    }
    async fn image_history(&self, id: &str) -> EngineResult<Vec<ImageLayer>> {
        self.enter("image_history", id).await?;
        self.require(Capabilities::IMAGE_HISTORY)?;
        Ok(vec![ImageLayer {
            id: Some(id.to_owned()),
            created: OffsetDateTime::UNIX_EPOCH,
            created_by: "/bin/sh -c #(nop) CMD [\"sh\"]".into(),
            size: 0,
            comment: String::new(),
        }])
    }
    fn pull_image(&self, reference: &str, _auth: Option<RegistryAuth>) -> EngineStream<PullProgress> {
        let mut st = self.st();
        st.calls.push(FakeCall {
            op: "pull_image",
            arg: reference.to_owned(),
        });
        if let Some(e) = st.errors.get("pull_image").cloned() {
            return crate::engine::error_stream(e);
        }
        let id = Self::new_id(&mut st);
        st.images.push(fixtures::image(reference, &id));
        Self::emit_locked(&mut st, ResourceKind::Image, "pull", reference);
        let structured = st.caps.contains(Capabilities::PULL_PROGRESS);
        let items = if structured {
            vec![
                PullProgress::Status(format!("Pulling from {reference}")),
                PullProgress::Layer {
                    id: "a1b2c3".into(),
                    status: "Downloading".into(),
                    current: Some(50),
                    total: Some(100),
                },
                PullProgress::Layer {
                    id: "a1b2c3".into(),
                    status: "Pull complete".into(),
                    current: Some(100),
                    total: Some(100),
                },
                PullProgress::Done {
                    digest: Some(format!("sha256:{id}")),
                },
            ]
        } else {
            vec![
                PullProgress::Status(format!("Pulling {reference}")),
                PullProgress::Done { digest: None },
            ]
        };
        Box::pin(futures::stream::iter(items.into_iter().map(Ok)))
    }
    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        self.enter("remove_image", id).await?;
        let mut st = self.st();
        let idx = st
            .images
            .iter()
            .position(|i| i.id == id || i.repo_tags.iter().any(|t| t == id))
            .ok_or_else(|| EngineError::not_found(ResourceKind::Image, id))?;
        let image_id = st.images[idx].id.clone();
        let users: Vec<String> = st
            .containers
            .iter()
            .filter(|c| c.image_id == image_id)
            .map(|c| c.name.clone())
            .collect();
        if !users.is_empty() && !force {
            return Err(EngineError::Conflict(format!(
                "unable to delete {id} (must be forced) - image is being used by container(s) {}",
                users.join(", ")
            )));
        }
        let img = st.images.remove(idx);
        Self::emit_locked(&mut st, ResourceKind::Image, "delete", &img.id);
        let mut out: Vec<ImageDeleteItem> = img
            .repo_tags
            .iter()
            .map(|t| ImageDeleteItem::Untagged(t.clone()))
            .collect();
        out.push(ImageDeleteItem::Deleted(img.id));
        Ok(out)
    }
    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        self.enter("prune_images", "").await?;
        let mut st = self.st();
        let used: HashSet<String> = st.containers.iter().map(|c| c.image_id.clone()).collect();
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.images)
            .into_iter()
            .partition(|i| !used.contains(&i.id) && (i.dangling || !dangling_only));
        st.images = keep;
        Ok(PruneReport {
            space_reclaimed: gone.iter().map(|i| i.size).sum(),
            deleted: gone.into_iter().map(|i| i.id).collect(),
        })
    }
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        self.enter("tag_image", id).await?;
        let mut st = self.st();
        let img = st
            .images
            .iter_mut()
            .find(|i| i.id == id || i.repo_tags.iter().any(|t| t == id))
            .ok_or_else(|| EngineError::not_found(ResourceKind::Image, id))?;
        img.repo_tags.push(format!("{repo}:{tag}"));
        img.dangling = false;
        Ok(())
    }
    async fn run_image(&self, spec: RunSpec) -> EngineResult<String> {
        self.enter("run_image", &spec.image).await?;
        let mut st = self.st();
        let id = Self::new_id(&mut st);
        let name = spec.name.clone().unwrap_or_else(|| format!("fake_{}", st.next_id));
        let mut c = fixtures::container(&name, ContainerState::Running);
        c.id = id.clone();
        c.image = spec.image.clone();
        c.ports = spec.ports.clone();
        c.labels = spec.labels.clone();
        st.containers.push(c);
        Self::emit_locked(&mut st, ResourceKind::Container, "start", &id);
        Ok(id)
    }

    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        self.enter("list_volumes", "").await?;
        Ok(self.st().volumes.clone())
    }
    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails> {
        self.enter("inspect_volume", name).await?;
        let st = self.st();
        let v = st
            .volumes
            .iter()
            .find(|v| v.name == name)
            .cloned()
            .ok_or_else(|| EngineError::not_found(ResourceKind::Volume, name))?;
        let used_by = st
            .containers
            .iter()
            .filter_map(|c| {
                c.mounts
                    .iter()
                    .find(|m| m.kind == MountKind::Volume && m.source == name)
                    .map(|m| ContainerRef {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        detail: Some(m.destination.clone()),
                        rw: Some(m.rw),
                    })
            })
            .collect();
        Ok(VolumeDetails {
            raw: serde_json::json!({ "Name": v.name, "Driver": v.driver }),
            options: BTreeMap::new(),
            status: None,
            used_by,
            summary: v,
        })
    }
    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        let name = spec.name.clone().unwrap_or_default();
        self.enter("create_volume", &name).await?;
        let mut st = self.st();
        let name = if name.is_empty() {
            Self::new_id(&mut st)
        } else {
            name
        };
        let mut v = fixtures::volume(&name);
        v.driver = spec.driver.clone().unwrap_or_else(|| "local".into());
        v.labels = spec.labels.clone();
        st.volumes.push(v.clone());
        Self::emit_locked(&mut st, ResourceKind::Volume, "create", &name);
        Ok(v)
    }
    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        self.enter("remove_volume", name).await?;
        let mut st = self.st();
        let users: Vec<String> = st
            .containers
            .iter()
            .filter(|c| c.mounts.iter().any(|m| m.source == name))
            .map(|c| c.name.clone())
            .collect();
        if !users.is_empty() && !force {
            return Err(EngineError::Conflict(format!(
                "remove {name}: volume is in use - [{}]",
                users.join(", ")
            )));
        }
        let idx = st
            .volumes
            .iter()
            .position(|v| v.name == name)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Volume, name))?;
        st.volumes.remove(idx);
        Self::emit_locked(&mut st, ResourceKind::Volume, "destroy", name);
        Ok(())
    }
    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        self.enter("prune_volumes", "").await?;
        let mut st = self.st();
        let used: HashSet<String> = st
            .containers
            .iter()
            .flat_map(|c| c.mounts.iter().map(|m| m.source.clone()))
            .collect();
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.volumes)
            .into_iter()
            .partition(|v| !used.contains(&v.name));
        st.volumes = keep;
        Ok(PruneReport {
            space_reclaimed: gone.iter().filter_map(|v| v.size).sum(),
            deleted: gone.into_iter().map(|v| v.name).collect(),
        })
    }
    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        self.enter("disk_usage", "").await?;
        self.require(Capabilities::DISK_USAGE)?;
        let st = self.st();
        Ok(st.disk_usage.clone().unwrap_or_else(|| DiskUsage {
            images_size: st.images.iter().map(|i| i.size).sum(),
            containers_size: 0,
            volumes: st
                .volumes
                .iter()
                .map(|v| (v.name.clone(), v.size.unwrap_or(0)))
                .collect(),
            volume_refs: st
                .volumes
                .iter()
                .map(|v| {
                    let n = st
                        .containers
                        .iter()
                        .filter(|c| c.mounts.iter().any(|m| m.source == v.name))
                        .count();
                    (v.name.clone(), n as i64)
                })
                .collect(),
            build_cache: 0,
            images_reclaimable: None,
        }))
    }

    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        self.enter("list_networks", "").await?;
        Ok(self.st().networks.clone())
    }
    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails> {
        self.enter("inspect_network", id).await?;
        let st = self.st();
        let n = st
            .networks
            .iter()
            .find(|n| n.id == id || n.name == id)
            .cloned()
            .ok_or_else(|| EngineError::not_found(ResourceKind::Network, id))?;
        let containers = st
            .containers
            .iter()
            .filter(|c| c.networks.contains(&n.name))
            .map(|c| NetworkEndpoint {
                name: c.name.clone(),
                id: c.id.clone(),
                ipv4: Some("172.17.0.2/16".into()),
                ipv6: None,
                mac: Some("02:42:ac:11:00:02".into()),
            })
            .collect();
        Ok(NetworkDetails {
            raw: serde_json::json!({ "Id": n.id, "Name": n.name }),
            containers,
            options: BTreeMap::new(),
            summary: n,
        })
    }
    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        self.enter("remove_network", id).await?;
        self.require(Capabilities::NETWORK_MGMT)?;
        let mut st = self.st();
        let idx = st
            .networks
            .iter()
            .position(|n| n.id == id || n.name == id)
            .ok_or_else(|| EngineError::not_found(ResourceKind::Network, id))?;
        if st.networks[idx].is_builtin() {
            return Err(EngineError::Api {
                status: 403,
                message: format!("{id} is a pre-defined network and cannot be removed"),
            });
        }
        let n = st.networks.remove(idx);
        Self::emit_locked(&mut st, ResourceKind::Network, "destroy", &n.id);
        Ok(())
    }
    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        self.enter("prune_networks", "").await?;
        let mut st = self.st();
        let used: HashSet<String> = st
            .containers
            .iter()
            .flat_map(|c| c.networks.iter().cloned())
            .collect();
        let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.networks)
            .into_iter()
            .partition(|n| !n.is_builtin() && !used.contains(&n.name));
        st.networks = keep;
        Ok(PruneReport {
            space_reclaimed: 0,
            deleted: gone.into_iter().map(|n| n.name).collect(),
        })
    }
}

// ───────────────────────────── loopback terminal ─────────────────────────────

/// Echoes every write back to the output. `close()` ends the output with exit code 0;
/// writing `exit\r` ends it too.
pub struct FakeTerminal {
    out_tx: Arc<Mutex<Option<mpsc::UnboundedSender<EngineResult<Bytes>>>>>,
    out_rx: Option<mpsc::UnboundedReceiver<EngineResult<Bytes>>>,
    exit_tx: Arc<Mutex<Option<oneshot::Sender<Option<i64>>>>>,
    exit_rx: Mutex<Option<oneshot::Receiver<Option<i64>>>>,
    pub size: Arc<Mutex<(u16, u16)>>,
}

impl FakeTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        let (out_tx, out_rx) = mpsc::unbounded();
        let (exit_tx, exit_rx) = oneshot::channel();
        let _ = out_tx.unbounded_send(Ok(Bytes::from_static(b"fake$ ")));
        Self {
            out_tx: Arc::new(Mutex::new(Some(out_tx))),
            out_rx: Some(out_rx),
            exit_tx: Arc::new(Mutex::new(Some(exit_tx))),
            exit_rx: Mutex::new(Some(exit_rx)),
            size: Arc::new(Mutex::new((cols, rows))),
        }
    }

    fn finish(&self, code: i64) {
        self.out_tx.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(tx) = self.exit_tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Some(code));
        }
    }
}

#[async_trait]
impl TerminalSession for FakeTerminal {
    fn output(&mut self) -> EngineStream<Bytes> {
        match self.out_rx.take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::empty()),
        }
    }
    async fn write(&self, data: Bytes) -> EngineResult<()> {
        let exit = data.as_ref() == b"exit\r" || data.as_ref() == b"exit\n";
        if let Some(tx) = self.out_tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.unbounded_send(Ok(data));
        }
        if exit {
            self.finish(0);
        }
        Ok(())
    }
    async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = (cols, rows);
        Ok(())
    }
    async fn wait(&self) -> EngineResult<Option<i64>> {
        let rx = self.exit_rx.lock().unwrap_or_else(|e| e.into_inner()).take();
        match rx {
            Some(rx) => Ok(rx.await.unwrap_or(None)),
            None => Err(EngineError::protocol("wait() called twice")),
        }
    }
    async fn close(&self) -> EngineResult<()> {
        self.finish(0);
        Ok(())
    }
}

// ───────────────────────────── factory ─────────────────────────────

/// Serves pre-built `FakeEngine`s by engine id; discovers one config per engine.
/// `handles()` returns true for every endpoint, so register it as the only factory in tests.
pub struct FakeFactory {
    engines: Mutex<Vec<(EngineConfig, Arc<FakeEngine>)>>,
    connect_errors: Mutex<HashMap<EngineId, EngineError>>,
    connects: Mutex<Vec<EngineId>>,
}

impl FakeFactory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            engines: Mutex::new(Vec::new()),
            connect_errors: Mutex::new(HashMap::new()),
            connects: Mutex::new(Vec::new()),
        })
    }
    /// Add an engine discovered as a local socket `/fake/<id>.sock`.
    pub fn add(&self, engine: Arc<FakeEngine>) -> EngineConfig {
        let id = engine.id().clone();
        let cfg = EngineConfig {
            name: format!("Fake {id}"),
            endpoint: EngineEndpoint::UnixSocket {
                path: format!("/fake/{id}.sock").into(),
            },
            origin: EngineOrigin::Discovered,
            enabled: true,
            hidden: false,
            id,
        };
        self.engines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((cfg.clone(), engine));
        cfg
    }
    /// Make `connect` fail for `id` (`None` clears).
    pub fn set_connect_error(&self, id: &EngineId, err: Option<EngineError>) {
        let mut m = self.connect_errors.lock().unwrap_or_else(|e| e.into_inner());
        match err {
            Some(e) => m.insert(id.clone(), e),
            None => m.remove(id),
        };
    }
    /// Ids passed to `connect`, in order.
    pub fn connects(&self) -> Vec<EngineId> {
        self.connects.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait]
impl EngineFactory for FakeFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }
    fn handles(&self, _endpoint: &EngineEndpoint) -> bool {
        true
    }
    async fn discover(&self) -> Vec<DiscoveredEngine> {
        self.engines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(c, _)| DiscoveredEngine::new(c.clone(), crate::engine::preference::LOCAL_SOCKET))
            .collect()
    }
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        self.connects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(cfg.id.clone());
        if let Some(e) = self
            .connect_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&cfg.id)
        {
            return Err(e.clone());
        }
        let engines = self.engines.lock().unwrap_or_else(|e| e.into_inner());
        let found = engines.iter().find(|(c, _)| c.id == cfg.id).map(|(_, e)| e.clone());
        match found {
            Some(e) => Ok(e),
            None => Err(EngineError::unreachable(format!("no fake engine '{}'", cfg.id))),
        }
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        vec![EngineConfigSchema {
            kind: EngineKind::Docker,
            label: "Fake".into(),
            fields: vec![],
        }]
    }
    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        if self
            .connect_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&cfg.id)
        {
            ProbeResult::Unreachable(EngineError::unreachable("fake"))
        } else {
            ProbeResult::Reachable
        }
    }
}

// ───────────────────────────── fixtures ─────────────────────────────

/// Builders for test DTOs.
pub mod fixtures {
    use super::*;

    fn hex_id(seed: &str) -> String {
        // Deterministic 64-hex id from a name (FNV-1a, repeated).
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in seed.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}{:016x}{h:016x}{:016x}", h.rotate_left(17), h.rotate_left(33))
    }

    pub fn container(name: &str, state: ContainerState) -> ContainerSummary {
        let id = hex_id(name);
        let status_text = match state {
            ContainerState::Running => "Up 2 hours".to_owned(),
            ContainerState::Paused => "Up 2 hours (Paused)".to_owned(),
            ContainerState::Exited => "Exited (0) 3 minutes ago".to_owned(),
            ContainerState::Created => "Created".to_owned(),
            ContainerState::Restarting => "Restarting (1) 1 second ago".to_owned(),
            ContainerState::Dead => "Dead".to_owned(),
            _ => String::new(),
        };
        ContainerSummary {
            image_id: format!("sha256:{}", hex_id("nginx:1.27")),
            id,
            name: name.to_owned(),
            image: "nginx:1.27".to_owned(),
            command: "nginx -g 'daemon off;'".to_owned(),
            created: OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000),
            state,
            status_text,
            health: None,
            exit_code: (state == ContainerState::Exited).then_some(0),
            ports: vec![],
            labels: BTreeMap::new(),
            networks: vec!["bridge".to_owned()],
            ip_addresses: vec![],
            mounts: vec![],
            size_rw: None,
            size_root_fs: None,
            compose: None,
        }
    }

    /// A Compose member: labels `com.docker.compose.{project,service,container-number}`.
    pub fn compose_container(project: &str, service: &str, state: ContainerState) -> ContainerSummary {
        let name = format!("{project}-{service}-1");
        let mut c = container(&name, state);
        for (k, v) in [
            (crate::grouping::COMPOSE_PROJECT_LABEL, project),
            (crate::grouping::COMPOSE_SERVICE_LABEL, service),
            (crate::grouping::COMPOSE_NUMBER_LABEL, "1"),
            (crate::grouping::COMPOSE_WORKING_DIR_LABEL, "/home/dev/proj"),
        ] {
            c.labels.insert(k.to_owned(), v.to_owned());
        }
        c.compose = Some(ComposeInfo {
            project: project.to_owned(),
            service: service.to_owned(),
            number: Some(1),
            working_dir: Some("/home/dev/proj".to_owned()),
            config_files: vec![],
            oneoff: false,
            depends_on: vec![],
        });
        c
    }

    pub fn details_for(summary: ContainerSummary) -> ContainerDetails {
        ContainerDetails {
            raw: serde_json::json!({ "Id": summary.id, "Name": format!("/{}", summary.name),
                "Config": { "Env": ["PATH=/usr/bin", "DB_PASSWORD=secret"] } }),
            started_at: Some(summary.created),
            finished_at: None,
            restart_count: 0,
            restart_policy: Some("no".into()),
            pid: summary.state.is_running().then_some(4242),
            platform: Some("linux".into()),
            entrypoint: vec!["/docker-entrypoint.sh".into()],
            cmd: vec!["nginx".into(), "-g".into(), "daemon off;".into()],
            working_dir: None,
            user: None,
            hostname: Some(super::format_short(&summary.id)),
            tty: false,
            env: vec![
                EnvVar {
                    key: "PATH".into(),
                    value: "/usr/bin".into(),
                },
                EnvVar {
                    key: "DB_PASSWORD".into(),
                    value: "secret".into(),
                },
            ],
            mounts: vec![],
            network_settings: ContainerNetworking {
                network_mode: Some("bridge".into()),
                dns: vec![],
                networks: vec![NetworkAttachment {
                    network: "bridge".into(),
                    ipv4: Some("172.17.0.2".into()),
                    ..Default::default()
                }],
            },
            port_bindings: summary.ports.clone(),
            resources: ResourceLimits::default(),
            health_log: vec![],
            summary,
        }
    }

    pub fn image(reference: &str, id_seed: &str) -> ImageSummary {
        ImageSummary {
            id: format!("sha256:{}", hex_id(id_seed)),
            repo_tags: if reference.is_empty() {
                vec![]
            } else {
                vec![reference.to_owned()]
            },
            repo_digests: vec![],
            created: OffsetDateTime::UNIX_EPOCH + time::Duration::days(19_900),
            size: 187_000_000,
            shared_size: None,
            containers: None,
            labels: BTreeMap::new(),
            dangling: reference.is_empty(),
        }
    }

    pub fn volume(name: &str) -> VolumeSummary {
        VolumeSummary {
            name: name.to_owned(),
            driver: "local".to_owned(),
            mountpoint: format!("/var/lib/docker/volumes/{name}/_data"),
            created: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::days(19_950)),
            scope: "local".to_owned(),
            labels: BTreeMap::new(),
            size: None,
            ref_count: None,
            compose: None,
        }
    }

    pub fn network(name: &str, driver: &str) -> NetworkSummary {
        NetworkSummary {
            id: hex_id(name),
            name: name.to_owned(),
            driver: driver.to_owned(),
            scope: "local".to_owned(),
            internal: false,
            attachable: false,
            ipv6: false,
            created: OffsetDateTime::UNIX_EPOCH + time::Duration::days(19_000),
            subnets: vec![IpamConfig {
                subnet: Some("172.17.0.0/16".into()),
                gateway: Some("172.17.0.1".into()),
                ip_range: None,
            }],
            labels: BTreeMap::new(),
            compose: None,
            containers: None,
        }
    }

    pub fn stats_sample(at: OffsetDateTime, cpu: f64, mem: u64) -> StatsSample {
        StatsSample {
            at,
            cpu_percent: cpu,
            online_cpus: 8,
            mem_used: mem,
            mem_limit: 2 * 1024 * 1024 * 1024,
            net_rx_bps: 1000.0,
            net_tx_bps: 500.0,
            net_rx_total: 1_000_000,
            net_tx_total: 500_000,
            blk_read_bps: 0.0,
            blk_write_bps: 2000.0,
            blk_read_total: 10_000_000,
            blk_write_total: 2_000_000,
            pids: Some(12),
        }
    }

    pub fn event(kind: ResourceKind, action: &str, id: &str) -> EngineEvent {
        EngineEvent {
            at: OffsetDateTime::now_utc(),
            kind,
            action: action.to_owned(),
            id: id.to_owned(),
            attributes: BTreeMap::new(),
        }
    }

    pub fn log_line(stream: LogStream, text: &str) -> LogChunk {
        LogChunk {
            stream,
            ts: Some(OffsetDateTime::now_utc()),
            bytes: Bytes::from(format!("{text}\n")),
        }
    }
}

fn format_short(id: &str) -> String {
    id.chars().take(12).collect()
}
