//! Hub tests (spec 60): FakeEngine + a scriptable test factory; no Docker.
//!
//! Public-API tests start the real runtime (`EngineHub::start`) from a plain `#[test]` and use
//! short real waits. Timing logic (ping/backoff/linger/debounce) runs on a paused tokio clock
//! via `HubInner::start_on` with the test runtime's handle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use dk_core::engine::preference;
use dk_core::fake::{FakeEngine, fixtures};
use dk_core::*;
use futures::{SinkExt, StreamExt};
use tempfile::TempDir;

use crate::bridge::{Feed, HubEvent, HubStream, TermCmd};
use crate::config::{Config, UiState};
use crate::handle::{HubHandle, HubOptions};
use crate::hub::{EngineHub, HubInner, lock};
use crate::paths::Paths;

// ───────────────────────────── test doubles ─────────────────────────────

/// Scriptable factory: fixed discovery results, engines by id, connect errors, call logs.
#[derive(Default)]
struct TestFactory {
    discovered: Mutex<Vec<DiscoveredEngine>>,
    engines: Mutex<HashMap<EngineId, Arc<dyn Engine>>>,
    connect_errors: Mutex<HashMap<EngineId, EngineError>>,
    connects: Mutex<Vec<EngineId>>,
    probes: Mutex<Vec<EngineId>>,
    starts: Mutex<Vec<EngineId>>,
}

fn sock_cfg(id: &str) -> EngineConfig {
    EngineConfig {
        id: EngineId::new(id),
        name: format!("Engine {id}"),
        endpoint: EngineEndpoint::UnixSocket {
            path: format!("/fake/{id}.sock").into(),
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
}

impl TestFactory {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Discovers `engine` as a local socket with preference `pref`.
    fn add(&self, engine: Arc<dyn Engine>, pref: u8) -> EngineConfig {
        let cfg = sock_cfg(engine.id().as_str());
        self.add_discovered(DiscoveredEngine::new(cfg.clone(), pref), Some(engine));
        cfg
    }
    fn add_discovered(&self, d: DiscoveredEngine, engine: Option<Arc<dyn Engine>>) {
        if let Some(e) = engine {
            lock(&self.engines).insert(d.config.id.clone(), e);
        }
        lock(&self.discovered).push(d);
    }
    fn set_connect_error(&self, id: &str, err: Option<EngineError>) {
        let mut m = lock(&self.connect_errors);
        match err {
            Some(e) => m.insert(EngineId::new(id), e),
            None => m.remove(&EngineId::new(id)),
        };
    }
    fn connects(&self) -> Vec<String> {
        lock(&self.connects).iter().map(|i| i.0.clone()).collect()
    }
    fn probes(&self) -> Vec<String> {
        lock(&self.probes).iter().map(|i| i.0.clone()).collect()
    }
}

#[async_trait]
impl EngineFactory for TestFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }
    fn handles(&self, _endpoint: &EngineEndpoint) -> bool {
        true
    }
    async fn discover(&self) -> Vec<DiscoveredEngine> {
        lock(&self.discovered).clone()
    }
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        lock(&self.connects).push(cfg.id.clone());
        if let Some(e) = lock(&self.connect_errors).get(&cfg.id) {
            return Err(e.clone());
        }
        lock(&self.engines)
            .get(&cfg.id)
            .cloned()
            .ok_or_else(|| EngineError::unreachable(format!("no engine '{}'", cfg.id)))
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        vec![EngineConfigSchema {
            kind: EngineKind::Docker,
            label: "Test".into(),
            fields: vec![],
        }]
    }
    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        lock(&self.probes).push(cfg.id.clone());
        match lock(&self.connect_errors).get(&cfg.id) {
            Some(e) => ProbeResult::Unreachable(e.clone()),
            None => ProbeResult::Reachable,
        }
    }
    async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        lock(&self.starts).push(cfg.id.clone());
        Ok(())
    }
}

/// Delegates to a FakeEngine but reports a chosen daemon id (ENG-009 tests).
struct SameDaemon {
    fake: Arc<FakeEngine>,
    daemon: String,
}

#[async_trait]
impl Engine for SameDaemon {
    fn id(&self) -> &EngineId {
        self.fake.id()
    }
    fn kind(&self) -> EngineKind {
        self.fake.kind()
    }
    fn capabilities(&self) -> Capabilities {
        self.fake.capabilities()
    }
    async fn ping(&self) -> EngineResult<()> {
        self.fake.ping().await
    }
    async fn info(&self) -> EngineResult<EngineInfo> {
        let mut i = self.fake.info().await?;
        i.daemon_id = Some(self.daemon.clone());
        Ok(i)
    }
    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent> {
        self.fake.events(filter)
    }
    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        self.fake.list_containers(q).await
    }
    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails> {
        self.fake.inspect_container(id).await
    }
    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        self.fake.container_action(id, action).await
    }
    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        self.fake.remove_container(id, opts).await
    }
    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        self.fake.prune_containers().await
    }
    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        self.fake.logs(id, opts)
    }
    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        self.fake.stats(id)
    }
    async fn top(&self, id: &str) -> EngineResult<ProcessList> {
        self.fake.top(id).await
    }
    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        self.fake.exec(id, req).await
    }
    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        self.fake.list_images().await
    }
    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails> {
        self.fake.inspect_image(id).await
    }
    async fn image_history(&self, id: &str) -> EngineResult<Vec<ImageLayer>> {
        self.fake.image_history(id).await
    }
    fn pull_image(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        self.fake.pull_image(reference, auth)
    }
    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        self.fake.remove_image(id, force).await
    }
    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        self.fake.prune_images(dangling_only).await
    }
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        self.fake.tag_image(id, repo, tag).await
    }
    async fn run_image(&self, spec: RunSpec) -> EngineResult<String> {
        self.fake.run_image(spec).await
    }
    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        self.fake.list_volumes().await
    }
    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails> {
        self.fake.inspect_volume(name).await
    }
    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        self.fake.create_volume(spec).await
    }
    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        self.fake.remove_volume(name, force).await
    }
    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        self.fake.prune_volumes().await
    }
    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        self.fake.disk_usage().await
    }
    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        self.fake.list_networks().await
    }
    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails> {
        self.fake.inspect_network(id).await
    }
    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        self.fake.remove_network(id).await
    }
    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        self.fake.prune_networks().await
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn opts(dir: &TempDir, f: Arc<TestFactory>, config: Config, ui_state: UiState) -> HubOptions {
    HubOptions {
        paths: Paths::in_dir(dir.path()),
        config,
        ui_state,
        factories: Some(vec![f as Arc<dyn EngineFactory>]),
        discover_on_start: true,
        worker_threads: 2,
    }
}

/// Hub on the current (paused) test runtime.
fn start_paused(f: Arc<TestFactory>, config: Config, ui_state: UiState) -> (HubHandle, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let h = HubInner::start_on(
        opts(&dir, f, config, ui_state),
        tokio::runtime::Handle::current(),
        None,
    );
    (h, dir)
}

/// Waits (virtual time) until `f` holds; panics after `max`.
async fn until(max: Duration, mut f: impl FnMut() -> bool) {
    let step = Duration::from_millis(10);
    let mut waited = Duration::ZERO;
    while !f() {
        assert!(waited < max, "condition not met within {max:?}");
        tokio::time::sleep(step).await;
        waited += step;
    }
}

/// Next stream item within `max` (virtual time on paused tests).
async fn next<T>(s: &mut HubStream<T>, max: Duration) -> Option<EngineResult<T>> {
    tokio::time::timeout(max, s.next())
        .await
        .expect("stream item timed out")
}

fn state_of(h: &HubHandle, id: &str) -> Option<EngineState> {
    lock(&h.inner.reg)
        .get(&EngineId::new(id))
        .map(|e| e.state.clone())
}

fn listed(h: &HubHandle) -> Vec<String> {
    lock(&h.inner.reg)
        .snapshot()
        .into_iter()
        .map(|s| s.config.id.0)
        .collect()
}

fn running_fake(id: &str) -> (Arc<FakeEngine>, String) {
    let fake = FakeEngine::new(id);
    let c = fixtures::container("web", ContainerState::Running);
    let cid = c.id.clone();
    fake.set_containers(vec![c]);
    (fake, cid)
}

/// Real runtime (public API) with short real waits.
fn start_real(f: Arc<TestFactory>) -> (HubHandle, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let h = EngineHub::start(opts(&dir, f, Config::default(), UiState::default())).unwrap();
    (h, dir)
}

fn wait_real(mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !f() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition not met in time"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn block_on_timeout<T: Send + 'static>(
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(futures::executor::block_on(fut));
    });
    rx.recv_timeout(Duration::from_secs(3))
        .expect("future timed out")
}

// ───────────────────────────── public API (real runtime) ─────────────────────────────

#[test]
fn nfr_030_engine_panic_becomes_protocol_error() {
    let f = TestFactory::new();
    let fake = FakeEngine::new("a");
    fake.panic_on("list_containers");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_real(f);
    let id = EngineId::new("a");
    wait_real(|| {
        hub.active_engine().is_some() && state_of(&hub, "a") == Some(EngineState::Connected)
    });

    let r = block_on_timeout(hub.call(&id, |e| async move {
        e.list_containers(Default::default()).await
    }));
    match r {
        Err(EngineError::Protocol(m)) => assert!(m.contains("engine panicked"), "{m}"),
        other => panic!("expected protocol error, got {other:?}"),
    }
    // A panic from caller-supplied code is caught too.
    async fn boom() -> EngineResult<()> {
        panic!("closure boom")
    }
    let r = block_on_timeout(hub.call(&id, |_e| boom()));
    assert!(matches!(r, Err(EngineError::Protocol(m)) if m.contains("closure boom")));
    // The hub keeps working; calls run on the hub runtime threads.
    let thread = block_on_timeout(hub.call(&id, |e| async move {
        e.ping().await?;
        Ok(std::thread::current().name().unwrap_or("").to_owned())
    }))
    .unwrap();
    assert!(thread.starts_with("dk-hub-"), "{thread}");
    hub.shutdown();
}

#[test]
fn nfr_002_call_fails_fast_when_not_connected() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    f.set_connect_error("a", Some(EngineError::unreachable("down")));
    let (hub, _dir) = start_real(f);
    wait_real(|| matches!(state_of(&hub, "a"), Some(EngineState::Failed { .. })));

    for id in [EngineId::new("a"), EngineId::new("nope")] {
        let started = std::time::Instant::now();
        let r = futures::executor::block_on(hub.call(&id, |e| async move { e.ping().await }));
        assert!(matches!(r, Err(EngineError::Unreachable { .. })), "{r:?}");
        assert!(started.elapsed() < Duration::from_millis(100));

        let items: Vec<_> = futures::executor::block_on(
            hub.subscribe(&id, |e| e.events(EventFilter::default()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(EngineError::Unreachable { .. })));
    }
    hub.shutdown();
}

#[test]
fn adr_0002_drop_stream_cancels_producer() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_real(f);
    let id = EngineId::new("a");
    wait_real(|| state_of(&hub, "a") == Some(EngineState::Connected));

    let c = cid.clone();
    let mut logs = hub.subscribe(&id, move |e| e.logs(&c, LogOpts::default()));
    wait_real(|| fake.open_streams().1 == 1);
    fake.push_log(&cid, fixtures::log_line(LogStream::Stdout, "hello"));
    let first = block_on_timeout(async move {
        let item = logs.next().await;
        (item, logs)
    });
    let (item, logs) = first;
    assert_eq!(item.unwrap().unwrap().bytes, Bytes::from("hello\n"));
    drop(logs);
    wait_real(|| fake.open_streams().1 == 0);
    hub.shutdown();
}

#[test]
fn hub_shutdown_flushes_and_returns_quickly() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    let (hub, dir) = start_real(f);
    wait_real(|| hub.active_engine().is_some());
    hub.config().update(|c| c.stats.history_minutes = 7);
    let t = std::time::Instant::now();
    hub.shutdown();
    assert!(t.elapsed() < Duration::from_millis(500));
    let (c, s) = crate::load_config(&Paths::in_dir(dir.path()));
    assert_eq!(c.stats.history_minutes, 7);
    assert_eq!(s.last_engine, Some(EngineId::new("a")));
}

// ───────────────────────────── supervisor & registry (paused clock) ─────────────────────────────

#[tokio::test(start_paused = true)]
async fn eng_021_failed_connect_retries_with_backoff_and_retry_resets() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    f.set_connect_error("a", Some(EngineError::unreachable("down")));
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    // Autoselect tries once; nothing becomes active → pick it explicitly.
    until(Duration::from_secs(5), || f.connects().len() == 1).await;
    hub.set_active(&EngineId::new("a")).await.unwrap();
    until(Duration::from_secs(5), || f.connects().len() == 2).await;
    until(Duration::from_secs(1), || {
        matches!(state_of(&hub, "a"), Some(EngineState::Failed { retry_in_ms: Some(ms), .. }) if (1000..=1100).contains(&ms))
    })
    .await;
    let t0 = tokio::time::Instant::now();
    until(Duration::from_secs(5), || f.connects().len() == 3).await;
    let d1 = t0.elapsed();
    assert!(
        d1 >= Duration::from_millis(950) && d1 < Duration::from_millis(1200),
        "{d1:?}"
    );
    until(Duration::from_secs(5), || f.connects().len() == 4).await;
    let d2 = t0.elapsed() - d1;
    assert!(
        d2 >= Duration::from_millis(1950) && d2 < Duration::from_millis(2300),
        "{d2:?}"
    );
    // Manual retry: immediate, and resets the backoff to 1 s.
    let n = f.connects().len();
    hub.retry(&EngineId::new("a")).await.unwrap();
    until(Duration::from_millis(50), || f.connects().len() == n + 1).await;
    until(Duration::from_millis(100), || {
        matches!(state_of(&hub, "a"), Some(EngineState::Failed { retry_in_ms: Some(ms), .. }) if ms < 1200)
    })
    .await;
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_022_reconnect_emits_event() {
    let f = TestFactory::new();
    let fake = FakeEngine::new("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut ev = hub.hub_events();
    assert!(matches!(
        next(&mut ev, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(HubEvent::Snapshot(_))))
    ));

    // An open stream ends with an error when the connection drops.
    let mut events = hub.events(&id);
    until(Duration::from_secs(1), || fake.open_streams().0 == 1).await;

    fake.set_error("ping", Some(EngineError::unreachable("daemon gone")));
    // Degraded is transient (immediate re-ping); the event log below checks it was reported.
    until(Duration::from_secs(15), || {
        matches!(state_of(&hub, "a"), Some(EngineState::Failed { .. }))
    })
    .await;
    assert!(matches!(
        next(&mut events, Duration::from_secs(1)).await,
        Some(Err(EngineError::Unreachable { .. }))
    ));
    assert!(next(&mut events, Duration::from_secs(1)).await.is_none());
    fake.set_error("ping", None);

    let mut seen = Vec::new();
    loop {
        match next(&mut ev, Duration::from_secs(5)).await {
            Some(Ok(Feed::Item(HubEvent::Reconnected(r)))) => {
                assert_eq!(r, id);
                break;
            }
            Some(Ok(Feed::Item(HubEvent::StatusChanged(s)))) => seen.push(s.state),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(seen.contains(&EngineState::Degraded), "{seen:?}");
    assert!(
        seen.iter().any(|s| matches!(s, EngineState::Failed { .. })),
        "{seen:?}"
    );
    assert_eq!(state_of(&hub, "a"), Some(EngineState::Connected));
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_021_wsl_distro_stops_auto_retry_after_three_failures() {
    let f = TestFactory::new();
    let mut d = DiscoveredEngine::new(
        EngineConfig {
            endpoint: EngineEndpoint::WslDistro {
                distro: "Ubuntu".into(),
                mode: WslMode::DialStdio,
            },
            ..sock_cfg("wsl-ubuntu")
        },
        preference::WSL_DISTRO,
    );
    d.initial_state = None;
    f.add_discovered(d, Some(FakeEngine::new("wsl-ubuntu")));
    f.set_connect_error(
        "wsl-ubuntu",
        Some(EngineError::unreachable("distro stopped")),
    );
    let ui = UiState {
        last_engine: Some(EngineId::new("wsl-ubuntu")),
        ..Default::default()
    };
    let (hub, _dir) = start_paused(f.clone(), Config::default(), ui);
    until(Duration::from_secs(60), || {
        matches!(
            state_of(&hub, "wsl-ubuntu"),
            Some(EngineState::Failed {
                retry_in_ms: None,
                ..
            })
        )
    })
    .await;
    assert_eq!(f.connects().len(), 3);
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(f.connects().len(), 3);
    // ENG-106: explicit start, then connect.
    f.set_connect_error("wsl-ubuntu", None);
    hub.start_wsl_distro(&EngineId::new("wsl-ubuntu"))
        .await
        .unwrap();
    until(Duration::from_secs(1), || {
        state_of(&hub, "wsl-ubuntu") == Some(EngineState::Connected)
    })
    .await;
    assert_eq!(lock(&f.starts).len(), 1);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_020_only_active_engine_connected() {
    let f = TestFactory::new();
    let a = FakeEngine::new("a");
    let b = FakeEngine::new("b");
    f.add(a.clone(), 20);
    f.add(b.clone(), 30);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    // Several probe periods: the non-active engine is probed, never connected.
    tokio::time::sleep(Duration::from_secs(95)).await;
    assert_eq!(f.connects(), vec!["a"]);
    assert!(f.probes().iter().filter(|p| *p == "b").count() >= 3);
    assert!(!f.probes().contains(&"a".to_string()));
    assert!(a.calls_to("ping").len() >= 9);
    assert!(b.calls().is_empty());
    assert_eq!(state_of(&hub, "b"), Some(EngineState::Disconnected));
    let r = hub
        .call(&EngineId::new("b"), |e| async move { e.ping().await })
        .await;
    assert!(matches!(r, Err(EngineError::Unreachable { .. })));

    // Switch: b connects, a is dropped.
    let mut ev = hub.hub_events();
    let _ = next(&mut ev, Duration::from_secs(1)).await;
    hub.set_active(&EngineId::new("b")).await.unwrap();
    assert_eq!(hub.active_engine(), Some(EngineId::new("b")));
    assert!(matches!(
        next(&mut ev, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(HubEvent::ActiveChanged(Some(id))))) if id.as_str() == "b"
    ));
    until(Duration::from_secs(5), || {
        state_of(&hub, "b") == Some(EngineState::Connected)
    })
    .await;
    assert_eq!(state_of(&hub, "a"), Some(EngineState::Disconnected));
    assert_eq!(f.connects(), vec!["a", "b"]);
    let r = hub
        .call(&EngineId::new("a"), |e| async move { e.ping().await })
        .await;
    assert!(matches!(r, Err(EngineError::Unreachable { .. })));
    assert_eq!(
        hub.config().ui_state().last_engine,
        Some(EngineId::new("b"))
    );
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_009_dedupe_by_daemon_id() {
    let f = TestFactory::new();
    let pipe: Arc<dyn Engine> = Arc::new(SameDaemon {
        fake: FakeEngine::new("pipe"),
        daemon: "DAEMON-1".into(),
    });
    let wsl: Arc<dyn Engine> = Arc::new(SameDaemon {
        fake: FakeEngine::new("wsl"),
        daemon: "DAEMON-1".into(),
    });
    f.add(pipe, preference::LOCAL_SOCKET);
    let wsl_cfg = f.add(wsl, preference::WSL_DISTRO);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "pipe") == Some(EngineState::Connected)
    })
    .await;
    assert_eq!(listed(&hub), vec!["pipe", "wsl"]);

    // Connecting the WSL engine reveals the same daemon: it's merged into `pipe`.
    hub.set_active(&EngineId::new("wsl")).await.unwrap();
    until(Duration::from_secs(5), || {
        state_of(&hub, "wsl") == Some(EngineState::Connected)
    })
    .await;
    hub.set_active(&EngineId::new("pipe")).await.unwrap();
    until(Duration::from_secs(5), || {
        state_of(&hub, "pipe") == Some(EngineState::Connected)
    })
    .await;
    let list = hub.engines().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].config.id.as_str(), "pipe");
    assert_eq!(list[0].also_reachable_via, vec![wsl_cfg.name.clone()]);

    // Un-merge in Settings brings it back.
    hub.config()
        .update(|c| c.engines.unmerged.push(EngineId::new("wsl")));
    assert_eq!(listed(&hub), vec!["pipe", "wsl"]);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_009_dedupe_by_canonical_endpoint() {
    let f = TestFactory::new();
    let same = EngineEndpoint::UnixSocket {
        path: "/var/run/docker.sock".into(),
    };
    let ctx = EngineConfig {
        endpoint: same.clone(),
        ..sock_cfg("ctx-default")
    };
    let local = EngineConfig {
        endpoint: same,
        ..sock_cfg("local-socket")
    };
    f.add_discovered(
        DiscoveredEngine::new(local, preference::LOCAL_SOCKET),
        Some(FakeEngine::new("local-socket")),
    );
    f.add_discovered(
        DiscoveredEngine::new(ctx, preference::DOCKER_CONTEXT),
        Some(FakeEngine::new("ctx-default")),
    );
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || hub.active_engine().is_some()).await;
    let list = hub.engines().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].config.id.as_str(), "ctx-default");
    // Rescan is idempotent.
    hub.rescan().await.unwrap();
    assert_eq!(hub.engines().await.unwrap().len(), 1);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_010_ssh_endpoint_unsupported_never_contacted() {
    let f = TestFactory::new();
    let ssh = EngineConfig {
        endpoint: EngineEndpoint::Ssh {
            url: "ssh://me@remote".into(),
        },
        ..sock_cfg("remote")
    };
    f.add_discovered(
        DiscoveredEngine::new(ssh, preference::DOCKER_HOST),
        Some(FakeEngine::new("remote")),
    );
    f.add(FakeEngine::new("local"), preference::LOCAL_SOCKET);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || hub.active_engine().is_some()).await;
    assert_eq!(hub.active_engine(), Some(EngineId::new("local")));
    assert!(matches!(
        state_of(&hub, "remote"),
        Some(EngineState::Unsupported { .. })
    ));
    // Explicit activation and retry never contact it either.
    hub.set_active(&EngineId::new("remote")).await.unwrap();
    hub.retry(&EngineId::new("remote")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(70)).await;
    assert!(!f.connects().contains(&"remote".to_string()));
    assert!(!f.probes().contains(&"remote".to_string()));
    assert!(matches!(
        state_of(&hub, "remote"),
        Some(EngineState::Unsupported { .. })
    ));
    assert!(listed(&hub).contains(&"remote".to_string()));
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_025_disabled_never_contacted() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("off"), preference::DOCKER_HOST);
    f.add(FakeEngine::new("on"), preference::LOCAL_SOCKET);
    let mut config = Config::default();
    config.engines.entries.push(EngineConfig {
        enabled: false,
        ..sock_cfg("off")
    });
    config.engines.entries.push(EngineConfig {
        origin: EngineOrigin::Manual,
        enabled: false,
        ..sock_cfg("manual-off")
    });
    let ui = UiState {
        last_engine: Some(EngineId::new("off")),
        ..Default::default()
    };
    let (hub, _dir) = start_paused(f.clone(), config, ui);
    until(Duration::from_secs(5), || hub.active_engine().is_some()).await;
    assert_eq!(hub.active_engine(), Some(EngineId::new("on")));
    hub.set_active(&EngineId::new("off")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(70)).await;
    for id in ["off", "manual-off"] {
        assert_eq!(state_of(&hub, id), Some(EngineState::Disabled), "{id}");
        assert!(!f.connects().contains(&id.to_string()), "{id}");
        assert!(!f.probes().contains(&id.to_string()), "{id}");
    }
    // Enabling the active engine connects it.
    let mut cfg = hub
        .engines()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.config.id.as_str() == "off")
        .unwrap()
        .config;
    cfg.enabled = true;
    hub.update_engine(cfg).await.unwrap();
    until(Duration::from_secs(5), || {
        state_of(&hub, "off") == Some(EngineState::Connected)
    })
    .await;
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_103_autoselect_prefers_lower_preference() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("wsl"), preference::WSL_DISTRO);
    f.add(FakeEngine::new("socket"), preference::LOCAL_SOCKET);
    f.add(FakeEngine::new("host"), preference::DOCKER_HOST);
    f.set_connect_error("host", Some(EngineError::unreachable("nope")));
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(10), || hub.active_engine().is_some()).await;
    assert_eq!(hub.active_engine(), Some(EngineId::new("socket")));
    until(Duration::from_secs(5), || {
        state_of(&hub, "socket") == Some(EngineState::Connected)
    })
    .await;
    // `host` was tried first, `wsl` never.
    assert_eq!(f.connects(), vec!["host", "socket"]);
    assert!(matches!(
        state_of(&hub, "host"),
        Some(EngineState::Failed { .. })
    ));
    assert_eq!(
        hub.config().ui_state().last_engine,
        Some(EngineId::new("socket"))
    );
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_103_last_engine_connects_first() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("host"), preference::DOCKER_HOST);
    f.add(FakeEngine::new("wsl"), preference::WSL_DISTRO);
    let ui = UiState {
        last_engine: Some(EngineId::new("wsl")),
        ..Default::default()
    };
    let (hub, _dir) = start_paused(f.clone(), Config::default(), ui);
    until(Duration::from_secs(5), || {
        state_of(&hub, "wsl") == Some(EngineState::Connected)
    })
    .await;
    assert_eq!(f.connects(), vec!["wsl"]);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_104_add_update_remove_engines() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("disc"), preference::LOCAL_SOCKET);
    let (hub, dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || hub.active_engine().is_some()).await;
    let mut ev = hub.hub_events();
    let _ = next(&mut ev, Duration::from_secs(1)).await;

    let manual = EngineConfig {
        id: EngineId::new(""),
        name: "My Remote".into(),
        endpoint: EngineEndpoint::Tcp {
            host: "10.0.0.5".into(),
            port: 2375,
            tls: None,
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    };
    let id = hub.add_engine(manual.clone()).await.unwrap();
    assert_eq!(id.as_str(), "my-remote");
    let id2 = hub
        .add_engine(EngineConfig {
            id: id.clone(),
            ..manual.clone()
        })
        .await
        .unwrap();
    assert_eq!(id2.as_str(), "my-remote-2");
    assert!(matches!(
        next(&mut ev, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(HubEvent::Added(s)))) if s.config.origin == EngineOrigin::Manual
    ));
    let _ = next(&mut ev, Duration::from_secs(1)).await;

    // Remove: manual → gone; discovered → hidden (kept, persisted).
    hub.remove_engine(&id2).await.unwrap();
    assert!(matches!(
        next(&mut ev, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(HubEvent::Removed(r)))) if r == id2
    ));
    hub.remove_engine(&EngineId::new("disc")).await.unwrap();
    let disc = hub
        .engines()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.config.id.as_str() == "disc")
        .unwrap();
    assert!(disc.config.hidden);
    assert!(hub.remove_engine(&EngineId::new("nope")).await.is_err());

    hub.shutdown();
    let (c, _) = crate::load_config(&Paths::in_dir(dir.path()));
    let ids: Vec<_> = c.engines.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["my-remote", "disc"]);
    assert!(c.engines.entries[1].hidden);
    assert_eq!(hub.engine_schemas().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn eng_104_test_engine_connects_without_registering() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("x"), 20);
    let mut cfg = Config::default();
    cfg.engines.entries.clear();
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(&dir, f.clone(), cfg, UiState::default());
    o.discover_on_start = false;
    let hub = HubInner::start_on(o, tokio::runtime::Handle::current(), None);
    let info = hub.test_engine(sock_cfg("x")).await.unwrap();
    assert_eq!(info.server_version, "27.3.1");
    assert!(hub.engines().await.unwrap().is_empty());
    let r = hub
        .test_engine(EngineConfig {
            endpoint: EngineEndpoint::Ssh {
                url: "ssh://h".into(),
            },
            ..sock_cfg("y")
        })
        .await;
    assert!(r.is_err());
    assert_eq!(f.connects(), vec!["x"]);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_020_capability_change_detected_on_ping() {
    let f = TestFactory::new();
    let fake = FakeEngine::new("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut ev = hub.hub_events();
    let _ = next(&mut ev, Duration::from_secs(1)).await;
    let caps = Capabilities::EVENTS | Capabilities::LOGS_FOLLOW;
    fake.set_capabilities(caps);
    loop {
        match next(&mut ev, Duration::from_secs(15)).await {
            Some(Ok(Feed::Item(HubEvent::CapabilitiesChanged { id, capabilities }))) => {
                assert_eq!(id.as_str(), "a");
                assert_eq!(capabilities, caps);
                break;
            }
            Some(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    }
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn eng_020_probe_marks_unreachable_engine_failed() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    f.add(FakeEngine::new("b"), 30);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    f.set_connect_error("b", Some(EngineError::unreachable("gone")));
    until(Duration::from_secs(35), || {
        matches!(
            state_of(&hub, "b"),
            Some(EngineState::Failed {
                retry_in_ms: None,
                ..
            })
        )
    })
    .await;
    hub.shutdown();
}

// ───────────────────────────── hub events / engine events ─────────────────────────────

#[tokio::test(start_paused = true)]
async fn eng_023_hub_events_start_with_snapshot() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut ev = hub.hub_events();
    match next(&mut ev, Duration::from_secs(1)).await {
        Some(Ok(Feed::Item(HubEvent::Snapshot(list)))) => {
            assert_eq!(list.len(), 1);
            assert!(list[0].active);
            assert_eq!(list[0].state, EngineState::Connected);
            assert!(list[0].info.is_some());
        }
        other => panic!("{other:?}"),
    }
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn events_shared_upstream_per_engine() {
    let f = TestFactory::new();
    let fake = FakeEngine::new("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut s1 = hub.events(&id);
    let mut s2 = hub.events(&id);
    until(Duration::from_secs(1), || fake.open_streams().0 == 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fake.calls_to("events").len(), 1);
    fake.emit_event(fixtures::event(ResourceKind::Container, "start", "c1"));
    for s in [&mut s1, &mut s2] {
        match next(s, Duration::from_secs(1)).await {
            Some(Ok(Feed::Item(e))) => assert_eq!(e.action, "start"),
            other => panic!("{other:?}"),
        }
    }
    drop(s1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fake.open_streams().0, 1);
    drop(s2);
    until(Duration::from_secs(1), || fake.open_streams().0 == 0).await;
    // A new subscriber restarts the upstream.
    let _s3 = hub.events(&id);
    until(Duration::from_secs(1), || fake.open_streams().0 == 1).await;
    // Upstream error → forwarded, then end.
    let mut s4 = hub.events(&id);
    fake.fail_event_streams(EngineError::protocol("boom"));
    assert!(matches!(
        next(&mut s4, Duration::from_secs(1)).await,
        Some(Err(EngineError::Protocol(_)))
    ));
    assert!(next(&mut s4, Duration::from_secs(1)).await.is_none());
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn events_lagged_yields_feed_lagged() {
    let f = TestFactory::new();
    let fake = FakeEngine::new("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut s = hub.events(&id);
    until(Duration::from_secs(1), || fake.open_streams().0 == 1).await;
    const N: usize = 2000;
    for i in 0..N {
        fake.emit_event(fixtures::event(
            ResourceKind::Container,
            "start",
            &format!("c{i}"),
        ));
    }
    // Let the upstream and the forwarder run until idle while the consumer reads nothing.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut items = 0usize;
    let mut dropped = 0u64;
    let mut last_id = String::new();
    while items + dropped as usize != N {
        match next(&mut s, Duration::from_secs(1)).await {
            Some(Ok(Feed::Item(e))) => {
                items += 1;
                last_id = e.id;
            }
            Some(Ok(Feed::Lagged { dropped: n })) => dropped += n,
            other => panic!("{other:?}"),
        }
    }
    assert!(dropped > 0, "expected a lag");
    assert_eq!(last_id, format!("c{}", N - 1));
    hub.shutdown();
}

// ───────────────────────────── stats service ─────────────────────────────

fn sample(i: i64) -> StatsSample {
    fixtures::stats_sample(
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(i),
        i as f64,
        1024,
    )
}

#[tokio::test(start_paused = true)]
async fn sta_006_stats_service_dedups_subscribers() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut s1 = hub.stats(&id, &cid);
    let mut s2 = hub.stats(&id, &cid);
    until(Duration::from_secs(1), || fake.open_streams().2 == 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fake.calls_to("stats").len(), 1);
    fake.push_stats(&cid, sample(1));
    for s in [&mut s1, &mut s2] {
        assert_eq!(
            next(s, Duration::from_secs(1)).await,
            Some(Ok(Feed::Item(sample(1))))
        );
    }
    drop(s1);
    drop(s2);
    // Linger 5 s, then stop the upstream; the buffer stays for `history_minutes`.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(fake.open_streams().2, 1);
    until(Duration::from_secs(2), || fake.open_streams().2 == 0).await;
    let inner = &hub.inner;
    assert_eq!(
        crate::stats_service::debug_entry(inner, &id, &cid),
        Some((0, 1, false))
    );
    // Re-subscribing within the window replays the buffer and restarts the upstream.
    let mut s3 = hub.stats(&id, &cid);
    assert_eq!(
        next(&mut s3, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(sample(1))))
    );
    until(Duration::from_secs(1), || fake.open_streams().2 == 1).await;
    drop(s3);
    // Evicted after `history_minutes` (15) without subscribers.
    tokio::time::sleep(Duration::from_secs(15 * 60 + 1)).await;
    assert_eq!(crate::stats_service::debug_entry(inner, &id, &cid), None);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn sta_006_history_replayed_to_new_subscriber() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    let mut config = Config::default();
    config.stats.history_minutes = 1; // ring of 60
    let (hub, _dir) = start_paused(f.clone(), config, UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut s1 = hub.stats(&id, &cid);
    until(Duration::from_secs(1), || fake.open_streams().2 == 1).await;
    for i in 0..70 {
        fake.push_stats(&cid, sample(i));
        assert_eq!(
            next(&mut s1, Duration::from_secs(1)).await,
            Some(Ok(Feed::Item(sample(i))))
        );
    }
    let mut s2 = hub.stats(&id, &cid);
    for i in 10..70 {
        assert_eq!(
            next(&mut s2, Duration::from_secs(1)).await,
            Some(Ok(Feed::Item(sample(i))))
        );
    }
    fake.push_stats(&cid, sample(70));
    assert_eq!(
        next(&mut s2, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(sample(70))))
    );
    assert_eq!(
        next(&mut s1, Duration::from_secs(1)).await,
        Some(Ok(Feed::Item(sample(70))))
    );
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn sta_006_stats_error_forwarded_and_buffer_kept() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let mut s = hub.stats(&id, "missing");
    assert!(matches!(
        next(&mut s, Duration::from_secs(1)).await,
        Some(Err(EngineError::NotFound { .. }))
    ));
    assert!(next(&mut s, Duration::from_secs(1)).await.is_none());

    let mut s = hub.stats(&id, &cid);
    until(Duration::from_secs(1), || fake.open_streams().2 == 1).await;
    fake.push_stats(&cid, sample(1));
    let _ = next(&mut s, Duration::from_secs(1)).await;
    fake.set_error("ping", Some(EngineError::unreachable("x")));
    // Connection lost → stream error + end; buffer kept.
    assert!(matches!(
        next(&mut s, Duration::from_secs(30)).await,
        Some(Err(EngineError::Unreachable { .. }))
    ));
    assert!(next(&mut s, Duration::from_secs(1)).await.is_none());
    assert_eq!(
        crate::stats_service::debug_entry(&hub.inner, &id, &cid).map(|e| e.1),
        Some(1)
    );
    hub.shutdown();
}

// ───────────────────────────── terminals ─────────────────────────────

#[tokio::test(start_paused = true)]
async fn trm_008_terminal_survives_output_drop_and_closes_on_engine_switch() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    f.add(FakeEngine::new("b"), 30);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;

    let t = hub
        .open_terminal(&id, &cid, ExecRequest::default())
        .await
        .unwrap();
    let crate::bridge::TerminalHandle {
        session_id,
        mut output,
        mut input,
        mut exit,
    } = t;
    assert!(session_id > 0);
    assert_eq!(
        next(&mut output, Duration::from_secs(1)).await,
        Some(Ok(Bytes::from_static(b"fake$ ")))
    );
    input
        .send(TermCmd::Data(Bytes::from_static(b"ls")))
        .await
        .unwrap();
    assert_eq!(
        next(&mut output, Duration::from_secs(1)).await,
        Some(Ok(Bytes::from_static(b"ls")))
    );
    input
        .send(TermCmd::Resize {
            cols: 100,
            rows: 30,
        })
        .await
        .unwrap();

    // The UI drops the output stream (tab switch): the session lives on.
    drop(output);
    for _ in 0..300 {
        input
            .send(TermCmd::Data(Bytes::from_static(b"x")))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(futures::FutureExt::now_or_never(&mut exit).is_none());
    assert_eq!(hub.inner.terminals.count(&id), 1);

    // Engine switch closes it (TRM-008).
    hub.set_active(&EngineId::new("b")).await.unwrap();
    let code = tokio::time::timeout(Duration::from_secs(5), &mut exit)
        .await
        .unwrap();
    assert_eq!(code, Ok(Some(0)));
    until(Duration::from_secs(1), || {
        hub.inner.terminals.count(&id) == 0
    })
    .await;
    assert!(input.send(TermCmd::Close).await.is_err() || input.is_closed());
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn trm_008_terminal_close_and_input_drop_end_session() {
    let f = TestFactory::new();
    let (fake, cid) = running_fake("a");
    f.add(fake.clone(), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;

    // Close command.
    let mut t = hub
        .open_terminal(&id, &cid, ExecRequest::default())
        .await
        .unwrap();
    t.input.send(TermCmd::Close).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), t.exit)
            .await
            .unwrap(),
        Ok(Some(0))
    );
    let rest: Vec<_> = t.output.collect().await;
    assert!(rest.len() <= 1);

    // Dropping every input sender.
    let t = hub
        .open_terminal(&id, &cid, ExecRequest::default())
        .await
        .unwrap();
    let input2 = t.input.clone();
    drop(t.input);
    drop(input2);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), t.exit)
            .await
            .unwrap(),
        Ok(Some(0))
    );

    // Process exit (`exit\r` on the fake) resolves `exit` and ends output.
    let mut t = hub
        .open_terminal(&id, &cid, ExecRequest::default())
        .await
        .unwrap();
    t.input
        .send(TermCmd::Data(Bytes::from_static(b"exit\r")))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), t.exit)
            .await
            .unwrap(),
        Ok(Some(0))
    );
    until(Duration::from_secs(1), || {
        hub.inner.terminals.count(&id) == 0
    })
    .await;

    // Exec errors are returned (stopped container).
    let err = hub
        .open_terminal(&id, "missing", ExecRequest::default())
        .await;
    assert!(matches!(err, Err(EngineError::NotFound { .. })));
    hub.shutdown();
}

// ───────────────────────────── config & misc ─────────────────────────────

#[tokio::test(start_paused = true)]
async fn config_debounced_atomic_save() {
    let f = TestFactory::new();
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(&dir, f, Config::default(), UiState::default());
    o.discover_on_start = false;
    let paths = o.paths.clone();
    let hub = HubInner::start_on(o, tokio::runtime::Handle::current(), None);
    let cfgh = hub.config();
    for i in 0..5u32 {
        cfgh.update(|c| c.logs.initial_tail = 100 + i);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // Each update restarted the 500 ms debounce: nothing written yet.
    assert!(!paths.config_file().exists());
    cfgh.update_ui_state(|s| s.sidebar_collapsed = true);
    assert_eq!(cfgh.get().logs.initial_tail, 104);
    assert!(cfgh.ui_state().sidebar_collapsed);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!paths.config_file().exists());
    until(Duration::from_secs(2), || {
        paths.config_file().exists() && paths.state_file().exists()
    })
    .await;
    let (c, s) = crate::load_config(&paths);
    assert_eq!(c.logs.initial_tail, 104);
    assert!(s.sidebar_collapsed);
    // Atomic: only the final files, no temp leftovers.
    let names: Vec<String> = std::fs::read_dir(&paths.config_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["config.toml".to_string()]);
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn save_file_launch_and_diagnostics() {
    let f = TestFactory::new();
    let (fake, _) = running_fake("a");
    f.add(fake, 20);
    let (hub, dir) = start_paused(f, Config::default(), UiState::default());
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;

    let p = dir.path().join("export/sub/logs.txt");
    hub.save_file(p.clone(), Bytes::from_static(b"line\n"))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"line\n");

    assert!(hub.launch(vec![]).await.is_err());
    assert!(hub.launch(vec!["".into()]).await.is_err());
    assert!(
        hub.launch(vec!["dockering-definitely-not-a-binary-xyz".into()])
            .await
            .is_err()
    );

    let d = hub.diagnostics().await.unwrap();
    assert!(d.contains(env!("CARGO_PKG_VERSION")));
    assert!(d.contains(std::env::consts::OS));
    assert!(d.contains("- a [Docker]"), "{d}");
    assert!(d.contains("27.3.1") && d.contains("EVENTS"), "{d}");
    assert!(!d.contains("/fake/a.sock"), "endpoint paths stay out: {d}");
    hub.shutdown();
}

// ───────────────────────────── review fixes ─────────────────────────────

/// Enabled-flag edit of the stored engine config, through the public API.
async fn set_enabled(hub: &HubHandle, id: &str, enabled: bool) {
    let mut cfg = hub
        .engines()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.config.id.as_str() == id)
        .unwrap()
        .config;
    cfg.enabled = enabled;
    hub.update_engine(cfg).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn eng_020_call_cancelled_when_engine_disabled_or_deactivated() {
    let f = TestFactory::new();
    let a = FakeEngine::new("a");
    f.add(a.clone(), 20);
    f.add(FakeEngine::new("b"), 30);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    // Every engine op now hangs (real helper-thread sleep; virtual time never reaches it).
    a.set_latency(Duration::from_secs(3600));

    // Disabling the engine drops its connection: the in-flight call resolves promptly.
    let call = hub.call(&id, |e| async move {
        e.list_containers(Default::default()).await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    set_enabled(&hub, "a", false).await;
    let r = tokio::time::timeout(Duration::from_millis(100), call)
        .await
        .expect("call must resolve once the connection is dropped");
    match r {
        Err(EngineError::Unreachable { reason, .. }) => assert!(reason.contains("disconnected")),
        other => panic!("expected Unreachable, got {other:?}"),
    }

    // Re-enable, then switch the active engine away while a call is in flight.
    a.set_latency(Duration::ZERO);
    set_enabled(&hub, "a", true).await;
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    a.set_latency(Duration::from_secs(3600));
    let call = hub.call(&id, |e| async move { e.ping().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    hub.set_active(&EngineId::new("b")).await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(100), call)
        .await
        .expect("call must resolve once the engine is deactivated");
    assert!(matches!(r, Err(EngineError::Unreachable { .. })), "{r:?}");
    hub.shutdown();
}

/// A terminal whose `write` never resolves (wedged PTY / transport). `close()` or `exit()`
/// resolves `wait()`.
struct StuckTerminal {
    exit_tx: Mutex<Option<futures::channel::oneshot::Sender<Option<i64>>>>,
    exit_rx: Mutex<Option<futures::channel::oneshot::Receiver<Option<i64>>>>,
    writes: Arc<std::sync::atomic::AtomicUsize>,
}

impl StuckTerminal {
    fn new() -> (Self, Arc<std::sync::atomic::AtomicUsize>) {
        let (tx, rx) = futures::channel::oneshot::channel();
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let t = Self {
            exit_tx: Mutex::new(Some(tx)),
            exit_rx: Mutex::new(Some(rx)),
            writes: writes.clone(),
        };
        (t, writes)
    }
    fn finish(&self, code: i64) {
        if let Some(tx) = lock(&self.exit_tx).take() {
            let _ = tx.send(Some(code));
        }
    }
}

#[async_trait]
impl TerminalSession for StuckTerminal {
    fn output(&mut self) -> EngineStream<Bytes> {
        Box::pin(futures::stream::pending())
    }
    async fn write(&self, _data: Bytes) -> EngineResult<()> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        futures::future::pending().await
    }
    async fn resize(&self, _cols: u16, _rows: u16) -> EngineResult<()> {
        futures::future::pending().await
    }
    async fn wait(&self) -> EngineResult<Option<i64>> {
        let rx = lock(&self.exit_rx).take();
        match rx {
            Some(rx) => Ok(rx.await.unwrap_or(None)),
            None => Ok(None),
        }
    }
    async fn close(&self) -> EngineResult<()> {
        self.finish(130);
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn trm_008_stuck_write_does_not_block_close_or_engine_switch() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    f.add(FakeEngine::new("b"), 30);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    // Same wiring as `open_terminal` after a successful exec.
    let open = |session: StuckTerminal| {
        let conn = hub.inner.conn(&id).unwrap();
        crate::terminal::start_actor(&hub.inner, &id, Box::new(session), conn.token)
    };

    // 1. `Close` while a write is stuck: the actor closes the session and resolves `exit`.
    let (session, writes) = StuckTerminal::new();
    let mut t = open(session);
    t.input
        .send(TermCmd::Data(Bytes::from_static(b"ls\r")))
        .await
        .unwrap();
    until(Duration::from_secs(1), || {
        writes.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    // Input queued behind the stuck write (more than the channel holds) and the final
    // `Close` are still accepted: the actor keeps reading its input while the write hangs.
    let input = &mut t.input;
    tokio::time::timeout(Duration::from_secs(1), async move {
        for _ in 0..100 {
            input
                .send(TermCmd::Data(Bytes::from_static(b"x")))
                .await
                .unwrap();
        }
        input.send(TermCmd::Close).await.unwrap();
    })
    .await
    .expect("input must not block behind a stuck write");
    let code = tokio::time::timeout(Duration::from_secs(5), t.exit)
        .await
        .expect("Close must end a stuck session");
    assert_eq!(code, Ok(Some(130)));
    until(Duration::from_secs(1), || {
        hub.inner.terminals.count(&id) == 0
    })
    .await;

    // 2. Process exit while a write is stuck.
    let (session, writes) = StuckTerminal::new();
    let exit_tx = lock(&session.exit_tx).take().unwrap();
    let mut t = open(session);
    t.input
        .send(TermCmd::Data(Bytes::from_static(b"x")))
        .await
        .unwrap();
    until(Duration::from_secs(1), || {
        writes.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    let _ = exit_tx.send(Some(3));
    let code = tokio::time::timeout(Duration::from_secs(5), t.exit)
        .await
        .expect("process exit must end a stuck session");
    assert_eq!(code, Ok(Some(3)));
    // After a natural exit the actor drains trailing output for up to EXIT_GRACE.
    until(Duration::from_secs(3), || {
        hub.inner.terminals.count(&id) == 0
    })
    .await;

    // 3. Engine switch (TRM-008) while a resize is stuck.
    let (session, _) = StuckTerminal::new();
    let mut t = open(session);
    t.input
        .send(TermCmd::Resize { cols: 80, rows: 24 })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(hub.inner.terminals.count(&id), 1);
    hub.set_active(&EngineId::new("b")).await.unwrap();
    let code = tokio::time::timeout(Duration::from_secs(5), t.exit)
        .await
        .expect("engine switch must end a stuck session");
    assert_eq!(code, Ok(Some(130)));
    until(Duration::from_secs(1), || {
        hub.inner.terminals.count(&id) == 0
    })
    .await;
    hub.shutdown();
}

#[tokio::test(start_paused = true)]
async fn trm_008_stuck_write_does_not_block_hub_shutdown() {
    let f = TestFactory::new();
    f.add(FakeEngine::new("a"), 20);
    let (hub, _dir) = start_paused(f.clone(), Config::default(), UiState::default());
    let id = EngineId::new("a");
    until(Duration::from_secs(5), || {
        state_of(&hub, "a") == Some(EngineState::Connected)
    })
    .await;
    let (session, writes) = StuckTerminal::new();
    let conn = hub.inner.conn(&id).unwrap();
    let mut t = crate::terminal::start_actor(&hub.inner, &id, Box::new(session), conn.token);
    t.input
        .send(TermCmd::Data(Bytes::from_static(b"x")))
        .await
        .unwrap();
    until(Duration::from_secs(1), || {
        writes.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    hub.shutdown();
    let code = tokio::time::timeout(Duration::from_secs(5), t.exit)
        .await
        .expect("hub shutdown must end a stuck session");
    assert_eq!(code, Ok(Some(130)));
}
