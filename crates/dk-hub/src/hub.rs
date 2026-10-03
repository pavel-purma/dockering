//! EngineHub internals (spec 10 §3, ADR-0002): the dedicated tokio runtime, the generic
//! `call`/`subscribe` bridge, engine registry operations, config persistence, and misc ops.
//!
//! Locking rule: every `std::sync::Mutex` here is held only for short, synchronous sections
//! and never across an `.await`.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use dk_core::{
    Engine, EngineConfig, EngineConfigSchema, EngineError, EngineEvent, EngineId, EngineInfo,
    EngineOrigin, EngineResult, EngineState, EngineStatus, ResourceKind,
};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::bridge::{Feed, HubCall, HubEvent, HubStream, STREAM_CAPACITY};
use crate::config::{self, Config, UiState};
use crate::handle::{ConfigHandle, HubHandle, HubOptions};
use crate::paths::Paths;
use crate::registry::{self, Conn, Registry, View};
use crate::{events, stats_service, supervisor, terminal};

/// Capacity of the `hub_events` broadcast ring.
const HUB_EVENTS_CAPACITY: usize = 256;
/// Debounce of config/state saves (spec 10 §6).
pub(crate) const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// Bound on the runtime shutdown (never block exit for long).
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Debounced, serialized config/state persistence.
#[derive(Default)]
pub(crate) struct Saver {
    config_dirty: AtomicBool,
    state_dirty: AtomicBool,
    notify: Notify,
    /// Serialises snapshot + write so an older snapshot can never overwrite a newer one.
    io_lock: Mutex<()>,
}

pub struct HubInner {
    pub(crate) paths: Paths,
    /// The owned runtime (`None` when running on a borrowed handle in tests, or after shutdown).
    rt: Mutex<Option<Runtime>>,
    pub(crate) handle: Handle,
    pub(crate) factories: Vec<Arc<dyn dk_core::EngineFactory>>,
    pub(crate) discover_on_start: bool,
    pub(crate) reg: Mutex<Registry>,
    /// Lock-free-ish snapshot for `active_engine()` (brief read lock).
    pub(crate) active: RwLock<Option<EngineId>>,
    pub(crate) config: Mutex<Config>,
    pub(crate) ui_state: Mutex<UiState>,
    saver: Saver,
    pub(crate) engine_events: Mutex<HashMap<EngineId, events::Shared>>,
    pub(crate) stats: stats_service::State,
    pub(crate) terminals: terminal::Registry,
    /// Cancelled by `shutdown()`; parent of every long-lived hub task.
    pub(crate) shutdown: CancellationToken,
    /// Generation counter for shared upstream entries.
    pub(crate) generation: AtomicU64,
    #[cfg_attr(not(feature = "updater"), allow(dead_code))]
    pub(crate) demo: bool,
    /// `None` without the `updater` feature or when the HTTP client can't start.
    #[cfg(feature = "updater")]
    pub(crate) updates: Option<crate::updates::State>,
}

impl Drop for HubInner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(rt) = lock(&self.rt).take() {
            rt.shutdown_background();
        }
    }
}

pub struct EngineHub;

impl EngineHub {
    /// Creates the dedicated multi-thread tokio runtime (`dk-hub-N` threads) and starts the
    /// registry, supervisor, and discovery. Never blocks for engine I/O.
    pub fn start(opts: HubOptions) -> std::io::Result<HubHandle> {
        let n = AtomicUsize::new(0);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(opts.worker_threads.clamp(2, 4))
            .thread_name_fn(move || format!("dk-hub-{}", n.fetch_add(1, Ordering::Relaxed)))
            .enable_all()
            .build()?;
        let handle = rt.handle().clone();
        Ok(HubInner::start_on(opts, handle, Some(rt)))
    }
}

/// Locks a std mutex, ignoring poisoning (a panicking engine op must not wedge the hub).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl HubInner {
    /// Builds the hub on `handle` (the owned runtime, or the test runtime) and spawns the
    /// bootstrap task (spec 10 §7 step 5).
    pub(crate) fn start_on(opts: HubOptions, handle: Handle, rt: Option<Runtime>) -> HubHandle {
        Self::start_with(
            opts,
            handle,
            rt,
            #[cfg(feature = "updater")]
            None,
        )
    }

    /// `start_on` with an injected updater (tests use a static source).
    pub(crate) fn start_with(
        opts: HubOptions,
        handle: Handle,
        rt: Option<Runtime>,
        #[cfg(feature = "updater")] update_state: Option<crate::updates::State>,
    ) -> HubHandle {
        let factories = opts.factories.unwrap_or_else(crate::default_factories);
        let (events_tx, _) = broadcast::channel(HUB_EVENTS_CAPACITY);
        let mut reg = Registry::new(View::from_settings(&opts.config.engines), events_tx);
        for cfg in &opts.config.engines.entries {
            if reg.idx(&cfg.id).is_none() {
                let f = registry::factory_for(&factories, &cfg.endpoint);
                reg.insert_config(cfg.clone(), f);
            }
        }
        let inner = Arc::new(HubInner {
            paths: opts.paths,
            rt: Mutex::new(rt),
            handle,
            factories,
            discover_on_start: opts.discover_on_start,
            reg: Mutex::new(reg),
            active: RwLock::new(None),
            config: Mutex::new(opts.config),
            ui_state: Mutex::new(opts.ui_state),
            saver: Saver::default(),
            engine_events: Mutex::new(HashMap::new()),
            stats: stats_service::State::default(),
            terminals: terminal::Registry::default(),
            shutdown: CancellationToken::new(),
            generation: AtomicU64::new(1),
            demo: opts.demo,
            #[cfg(feature = "updater")]
            updates: if opts.demo {
                None
            } else {
                update_state.or_else(crate::updates::State::production)
            },
        });
        inner.handle.spawn(saver_loop(inner.clone()));
        inner.handle.spawn(supervisor::bootstrap(inner.clone()));
        #[cfg(feature = "updater")]
        crate::updates::start(&inner);
        HubHandle { inner }
    }

    pub(crate) fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed)
    }

    /// The live connection of a usable (Connected/Degraded) engine; else fail fast (NFR-002).
    pub(crate) fn conn(&self, id: &EngineId) -> EngineResult<Conn> {
        let reg = lock(&self.reg);
        let Some(e) = reg.get(id) else {
            return Err(EngineError::unreachable(format!("unknown engine '{id}'")));
        };
        match (&e.conn, e.state.is_usable()) {
            (Some(c), true) if !c.token.is_cancelled() => Ok(c.clone()),
            _ => Err(EngineError::unreachable(format!(
                "engine '{id}' is not connected"
            ))),
        }
    }

    pub(crate) fn set_state(&self, id: &EngineId, state: EngineState) {
        lock(&self.reg).set_state(id, state);
    }

    pub(crate) fn active_id(&self) -> Option<EngineId> {
        self.active
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_active_id(&self, id: Option<EngineId>) {
        *self.active.write().unwrap_or_else(|e| e.into_inner()) = id;
    }

    /// Makes `id` the active engine (ENG-020): stops the previous supervisor, drops its
    /// connection and closes its terminal actors (TRM-008), then supervises `id`.
    /// `pre` is an already established connection (auto-select, ENG-103).
    pub(crate) fn activate(
        self: &Arc<Self>,
        id: &EngineId,
        pre: Option<(Arc<dyn Engine>, EngineInfo)>,
        only_if_none: bool,
    ) -> EngineResult<()> {
        let (old, token, retry) = {
            let mut reg = lock(&self.reg);
            if reg.idx(id).is_none() {
                return Err(EngineError::not_found(ResourceKind::Engine, id.as_str()));
            }
            if only_if_none && reg.active.is_some() {
                return Ok(());
            }
            if reg.active.as_ref() == Some(id) && reg.supervisor.is_some() {
                return Ok(());
            }
            let old = reg.active.replace(id.clone());
            self.set_active_id(Some(id.clone()));
            if let Some(sup) = reg.supervisor.take() {
                sup.token.cancel();
            }
            reg.emit(HubEvent::ActiveChanged(Some(id.clone())));
            if let Some(old) = old.as_ref().filter(|o| *o != id) {
                disconnect_locked(&mut reg, old);
                match reg.get(old) {
                    Some(e) if reg.visible(e) => reg.emit_status(old),
                    // e.g. merged into another engine (ENG-009) while it was active.
                    Some(_) => reg.emit(HubEvent::Removed(old.clone())),
                    None => {}
                }
            }
            reg.emit_status(id);
            let token = self.shutdown.child_token();
            let retry = Arc::new(Notify::new());
            reg.supervisor = Some(registry::Supervisor {
                id: id.clone(),
                token: token.clone(),
                retry: retry.clone(),
            });
            (old, token, retry)
        };
        if let Some(old) = old.filter(|o| o != id) {
            self.terminals.close_engine(&old);
        }
        self.update_ui_state(|s| s.last_engine = Some(id.clone()));
        self.handle.spawn(supervisor::supervise(
            self.clone(),
            id.clone(),
            token,
            retry,
            pre,
        ));
        Ok(())
    }

    /// Restarts the active engine's supervisor (endpoint changed / re-enabled).
    pub(crate) fn restart_supervisor(self: &Arc<Self>, id: &EngineId) {
        let started = {
            let mut reg = lock(&self.reg);
            if reg.active.as_ref() != Some(id) {
                return;
            }
            if let Some(sup) = reg.supervisor.take() {
                sup.token.cancel();
            }
            disconnect_locked(&mut reg, id);
            let token = self.shutdown.child_token();
            let retry = Arc::new(Notify::new());
            reg.supervisor = Some(registry::Supervisor {
                id: id.clone(),
                token: token.clone(),
                retry: retry.clone(),
            });
            (token, retry)
        };
        self.handle.spawn(supervisor::supervise(
            self.clone(),
            id.clone(),
            started.0,
            started.1,
            None,
        ));
    }

    // ── config ────────────────────────────────────────────────────────────────────────

    pub(crate) fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let engines = {
            let mut c = lock(&self.config);
            let before = c.engines.clone();
            f(&mut c);
            (before != c.engines).then(|| c.engines.clone())
        };
        if let Some(e) = engines {
            let mut reg = lock(&self.reg);
            let view = View::from_settings(&e);
            let changed = view.show_all_wsl_distros != reg.view.show_all_wsl_distros
                || view.show_all_wslc_sessions != reg.view.show_all_wslc_sessions
                || view.unmerged != reg.view.unmerged;
            reg.view = view;
            if changed {
                reg.emit_snapshot();
            }
        }
        self.saver.config_dirty.store(true, Ordering::SeqCst);
        self.saver.notify.notify_one();
    }

    pub(crate) fn update_ui_state(&self, f: impl FnOnce(&mut UiState)) {
        {
            let mut s = lock(&self.ui_state);
            f(&mut s);
        }
        self.saver.state_dirty.store(true, Ordering::SeqCst);
        self.saver.notify.notify_one();
    }

    /// Writes dirty files now (synchronous; called on a blocking thread or at shutdown).
    pub(crate) fn flush_now(&self) {
        let _io = lock(&self.saver.io_lock);
        if self.saver.config_dirty.swap(false, Ordering::SeqCst) {
            let c = lock(&self.config).clone();
            if let Err(e) = config::save_config(&self.paths, &c) {
                tracing::warn!(error = %e, "failed to save config.toml");
            }
        }
        if self.saver.state_dirty.swap(false, Ordering::SeqCst) {
            let s = lock(&self.ui_state).clone();
            if let Err(e) = config::save_ui_state(&self.paths, &s) {
                tracing::warn!(error = %e, "failed to save state.json");
            }
        }
    }

    /// Inserts or replaces the stored entry for `cfg.id` (manual engine or override).
    fn store_entry(&self, cfg: EngineConfig) {
        self.update_config(
            |c| match c.engines.entries.iter_mut().find(|e| e.id == cfg.id) {
                Some(e) => *e = cfg,
                None => c.engines.entries.push(cfg),
            },
        );
    }
}

/// Drops the live connection of `id` (cancelling its streams) and resets its state.
pub(crate) fn disconnect_locked(reg: &mut Registry, id: &EngineId) {
    let Some(e) = reg.get_mut(id) else { return };
    if let Some(c) = e.conn.take() {
        c.token.cancel();
    }
    e.state = match &e.state {
        s @ (EngineState::Disabled | EngineState::Unsupported { .. } | EngineState::Stopped) => {
            s.clone()
        }
        _ if !e.config.enabled => EngineState::Disabled,
        _ => EngineState::Disconnected,
    };
}

async fn saver_loop(inner: Arc<HubInner>) {
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = inner.saver.notify.notified() => {}
        }
        // Debounce: wait for SAVE_DEBOUNCE of quiet.
        loop {
            tokio::select! {
                _ = inner.shutdown.cancelled() => return,
                _ = tokio::time::sleep(SAVE_DEBOUNCE) => break,
                _ = inner.saver.notify.notified() => {}
            }
        }
        let i = inner.clone();
        let _ = tokio::task::spawn_blocking(move || i.flush_now()).await;
    }
}

// ── panic safety (NFR-030) ─────────────────────────────────────────────────────────────

pub(crate) fn panic_message(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

pub(crate) fn panic_error(p: &(dyn Any + Send)) -> EngineError {
    let msg = panic_message(p);
    tracing::error!(panic = %msg, "engine future panicked");
    EngineError::Protocol(format!("engine panicked: {msg}"))
}

/// Runs an engine future, mapping a panic to `EngineError::Protocol` (NFR-030).
pub(crate) async fn guarded<T>(fut: impl Future<Output = EngineResult<T>>) -> EngineResult<T> {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(p) => Err(panic_error(&*p)),
    }
}

/// Wraps an engine stream so a panic while polling becomes one `Protocol` error item.
pub(crate) fn guarded_stream<T, S>(s: S) -> impl Stream<Item = EngineResult<T>> + Send
where
    S: Stream<Item = EngineResult<T>> + Send,
    T: Send,
{
    AssertUnwindSafe(s)
        .catch_unwind()
        .map(|r| r.unwrap_or_else(|p| Err(panic_error(&*p))))
}

pub(crate) fn disconnected(id: &EngineId) -> EngineError {
    EngineError::unreachable(format!("engine '{id}' disconnected"))
}

// ── generic engine access ──────────────────────────────────────────────────────────────

pub(crate) fn call<T, F, C>(h: &HubHandle, engine: &EngineId, f: C) -> HubCall<T>
where
    C: FnOnce(Arc<dyn Engine>) -> F + Send + 'static,
    F: Future<Output = EngineResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let conn = match h.inner.conn(engine) {
        Ok(c) => c,
        Err(e) => return HubCall::ready(Err(e)),
    };
    let (mut tx, call) = HubCall::channel();
    let span = tracing::debug_span!("engine.call", engine = %engine);
    let id = engine.clone();
    h.inner.handle.spawn(
        async move {
            let started = Instant::now();
            let conn_token = conn.token;
            let engine = conn.engine;
            let fut = guarded(async move { f(engine).await });
            let res = tokio::select! {
                r = fut => r,
                // The caller dropped the HubCall: cancel the engine future.
                _ = tx.cancellation() => return,
                // The connection was dropped (engine switch, disable, ping failure, shutdown):
                // the engine future is cancelled and the caller sees `Unreachable`.
                _ = conn_token.cancelled() => Err(disconnected(&id)),
            };
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                ok = res.is_ok(),
                "engine call finished"
            );
            let _ = tx.send(res);
        }
        .instrument(span),
    );
    call
}

pub(crate) fn subscribe<T, S, C>(h: &HubHandle, engine: &EngineId, f: C) -> HubStream<T>
where
    C: FnOnce(Arc<dyn Engine>) -> S + Send + 'static,
    S: Stream<Item = EngineResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let conn = match h.inner.conn(engine) {
        Ok(c) => c,
        Err(e) => return HubStream::failed(e),
    };
    let (mut tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
    let id = engine.clone();
    h.inner.handle.spawn(async move {
        let s = match std::panic::catch_unwind(AssertUnwindSafe(|| f(conn.engine))) {
            Ok(s) => s,
            Err(p) => {
                let _ = tx.try_send(Err(panic_error(&*p)));
                return;
            }
        };
        pump(guarded_stream(s), tx, token, conn.token, id).await;
    });
    stream
}

/// Forwards `s` into a bounded channel with backpressure until the consumer drops the
/// stream (`token`), the connection drops (`conn_token`), or the upstream ends.
pub(crate) async fn pump<T, S>(
    s: S,
    mut tx: mpsc::Sender<EngineResult<T>>,
    token: CancellationToken,
    conn_token: CancellationToken,
    id: EngineId,
) where
    S: Stream<Item = EngineResult<T>>,
{
    let mut s = pin!(s);
    loop {
        let item = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = conn_token.cancelled() => {
                let _ = tx.try_send(Err(disconnected(&id)));
                return;
            }
            item = s.next() => item,
        };
        let Some(item) = item else { return };
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = tx.send(item) => if r.is_err() { return },
        }
    }
}

// ── hub events (ENG-023) ───────────────────────────────────────────────────────────────

pub(crate) fn hub_events(h: &HubHandle) -> HubStream<Feed<HubEvent>> {
    let (snapshot, rx) = {
        let reg = lock(&h.inner.reg);
        // Subscribing under the registry lock: no event is missed or duplicated.
        (reg.snapshot(), reg.events.subscribe())
    };
    let (tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
    let shutdown = h.inner.shutdown.clone();
    h.inner
        .handle
        .spawn(forward_hub_events(snapshot, rx, tx, token, shutdown));
    stream
}

pub(crate) async fn forward_hub_events(
    snapshot: Vec<EngineStatus>,
    mut rx: broadcast::Receiver<HubEvent>,
    mut tx: mpsc::Sender<EngineResult<Feed<HubEvent>>>,
    token: CancellationToken,
    shutdown: CancellationToken,
) {
    let mut next = Some(Feed::Item(HubEvent::Snapshot(snapshot)));
    loop {
        if let Some(item) = next.take() {
            tokio::select! {
                biased;
                _ = token.cancelled() => return,
                r = tx.send(Ok(item)) => if r.is_err() { return },
            }
        }
        next = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = shutdown.cancelled() => return,
            r = rx.recv() => match r {
                Ok(ev) => Some(Feed::Item(ev)),
                Err(broadcast::error::RecvError::Lagged(n)) => Some(Feed::Lagged { dropped: n }),
                Err(broadcast::error::RecvError::Closed) => return,
            },
        };
    }
}

// ── engine registry ops ────────────────────────────────────────────────────────────────

pub(crate) fn engines(h: &HubHandle) -> HubCall<Vec<EngineStatus>> {
    HubCall::ready(Ok(lock(&h.inner.reg).snapshot()))
}

pub(crate) fn active_engine(h: &HubHandle) -> Option<EngineId> {
    h.inner.active_id()
}

pub(crate) fn set_active(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    HubCall::ready(h.inner.activate(id, None, false))
}

pub(crate) fn add_engine(h: &HubHandle, mut cfg: EngineConfig) -> HubCall<EngineId> {
    let inner = &h.inner;
    let status = {
        let mut reg = lock(&inner.reg);
        if cfg.id.as_str().is_empty() || reg.idx(&cfg.id).is_some() {
            let name = if cfg.name.trim().is_empty() {
                cfg.endpoint.display()
            } else {
                cfg.name.clone()
            };
            cfg.id = registry::slug_id(&name, |s| reg.idx(&EngineId::new(s)).is_some());
        }
        if cfg.name.trim().is_empty() {
            cfg.name = cfg.endpoint.display();
        }
        cfg.origin = EngineOrigin::Manual;
        let f = registry::factory_for(&inner.factories, &cfg.endpoint);
        reg.insert_config(cfg.clone(), f);
        let status = reg.status(&reg.entries[reg.entries.len() - 1]);
        reg.emit(HubEvent::Added(status.clone()));
        status
    };
    let id = status.config.id.clone();
    inner.store_entry(status.config);
    HubCall::ready(Ok(id))
}

pub(crate) fn update_engine(h: &HubHandle, cfg: EngineConfig) -> HubCall<()> {
    let inner = &h.inner;
    let res = (|| {
        let (stored, restart) = {
            let mut reg = lock(&inner.reg);
            let is_active = reg.is_active(&cfg.id);
            let Some(e) = reg.get_mut(&cfg.id) else {
                return Err(EngineError::not_found(
                    ResourceKind::Engine,
                    cfg.id.as_str(),
                ));
            };
            let was_enabled = e.config.enabled;
            let mut endpoint_changed = false;
            if e.config.origin == EngineOrigin::Manual {
                let mut new = cfg.clone();
                new.origin = EngineOrigin::Manual;
                endpoint_changed = e.config.endpoint != new.endpoint;
                e.config = new;
                e.factory = registry::factory_for(&inner.factories, &e.config.endpoint);
            } else {
                e.config.name = cfg.name.clone();
                e.config.enabled = cfg.enabled;
                e.config.hidden = cfg.hidden;
            }
            let toggled = was_enabled != e.config.enabled;
            if endpoint_changed || toggled {
                if let Some(c) = e.conn.take() {
                    c.token.cancel();
                }
                e.state = registry::initial_state(&e.config, e.factory, None);
            }
            let stored = e.config.clone();
            reg.emit_status(&cfg.id);
            (stored, is_active && (endpoint_changed || toggled))
        };
        inner.store_entry(stored);
        if restart {
            inner.restart_supervisor(&cfg.id);
        }
        Ok(())
    })();
    HubCall::ready(res)
}

pub(crate) fn remove_engine(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let inner = &h.inner;
    let res = (|| {
        let manual = {
            let mut reg = lock(&inner.reg);
            let Some(i) = reg.idx(id) else {
                return Err(EngineError::not_found(ResourceKind::Engine, id.as_str()));
            };
            if reg.entries[i].config.origin == EngineOrigin::Manual {
                if reg.is_active(id) {
                    if let Some(sup) = reg.supervisor.take() {
                        sup.token.cancel();
                    }
                    reg.active = None;
                    inner.set_active_id(None);
                    reg.emit(HubEvent::ActiveChanged(None));
                }
                let e = reg.entries.remove(i);
                if let Some(c) = e.conn {
                    c.token.cancel();
                }
                reg.emit(HubEvent::Removed(id.clone()));
                None
            } else {
                reg.entries[i].config.hidden = true;
                let cfg = reg.entries[i].config.clone();
                reg.emit_status(id);
                Some(cfg)
            }
        };
        match manual {
            None => {
                inner.terminals.close_engine(id);
                inner.update_config(|c| c.engines.entries.retain(|e| &e.id != id));
                if inner.ui_state().last_engine.as_ref() == Some(id) {
                    inner.update_ui_state(|s| s.last_engine = None);
                }
            }
            Some(cfg) => inner.store_entry(cfg),
        }
        Ok(())
    })();
    HubCall::ready(res)
}

pub(crate) fn test_engine(h: &HubHandle, cfg: EngineConfig) -> HubCall<EngineInfo> {
    let inner = h.inner.clone();
    let (mut tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        let res = async {
            if let EngineState::Unsupported { reason } =
                registry::initial_state(&cfg, Some(0), None)
            {
                return Err(EngineError::unreachable(reason));
            }
            let Some(fi) = registry::factory_for(&inner.factories, &cfg.endpoint) else {
                return Err(EngineError::unreachable("no backend for this endpoint"));
            };
            let factory = inner.factories[fi].clone();
            let t = supervisor::TEST_TIMEOUT;
            match tokio::time::timeout(t, supervisor::connect_info(&factory, &cfg)).await {
                Ok(r) => r.map(|(_engine, info)| info),
                Err(_) => Err(EngineError::Timeout(t)),
            }
        };
        tokio::select! {
            r = res => { let _ = tx.send(r); }
            _ = tx.cancellation() => {}
        }
    });
    call
}

pub(crate) fn rescan(h: &HubHandle) -> HubCall<()> {
    let inner = h.inner.clone();
    let (tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        supervisor::discover(&inner).await;
        if inner.active_id().is_none() {
            supervisor::autoselect(&inner).await;
        }
        let _ = tx.send(Ok(()));
    });
    call
}

pub(crate) fn retry(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let inner = &h.inner;
    let sup = {
        let reg = lock(&inner.reg);
        if reg.idx(id).is_none() {
            return HubCall::ready(Err(EngineError::not_found(
                ResourceKind::Engine,
                id.as_str(),
            )));
        }
        reg.supervisor
            .as_ref()
            .filter(|s| &s.id == id)
            .map(|s| s.retry.clone())
    };
    match sup {
        Some(retry) => retry.notify_one(),
        None => {
            let inner = inner.clone();
            let id = id.clone();
            h.inner
                .handle
                .spawn(async move { supervisor::probe_one(&inner, &id).await });
        }
    }
    HubCall::ready(Ok(()))
}

pub(crate) fn start_engine(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let inner = h.inner.clone();
    let id = id.clone();
    let (tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        let res = async {
            let (cfg, fi) = {
                let reg = lock(&inner.reg);
                let e = reg
                    .get(&id)
                    .ok_or_else(|| EngineError::not_found(ResourceKind::Engine, id.as_str()))?;
                if !e.contactable() {
                    return Err(EngineError::unreachable(format!(
                        "engine '{id}' can't be started"
                    )));
                }
                (e.config.clone(), e.factory)
            };
            let factory = fi
                .and_then(|i| inner.factories.get(i).cloned())
                .ok_or_else(|| EngineError::unreachable("no backend for this endpoint"))?;
            inner.set_state(&id, EngineState::Connecting);
            match guarded(factory.start(&cfg)).await {
                Ok(()) => {
                    let sup = {
                        let mut reg = lock(&inner.reg);
                        reg.set_state(&id, EngineState::Disconnected);
                        reg.supervisor
                            .as_ref()
                            .filter(|s| s.id == id)
                            .map(|s| s.retry.clone())
                    };
                    match sup {
                        // Already supervised: wake it so it connects now.
                        Some(retry) => retry.notify_one(),
                        // "Start & connect" (ENG-106): make it the active engine, exactly
                        // like `set_active` (emits ActiveChanged, supervisor connects).
                        None => inner.activate(&id, None, false)?,
                    }
                    Ok(())
                }
                Err(e) => {
                    inner.set_state(
                        &id,
                        EngineState::Failed {
                            error: e.clone(),
                            retry_in_ms: None,
                        },
                    );
                    Err(e)
                }
            }
        };
        let _ = tx.send(res.await);
    });
    call
}

pub(crate) fn engine_schemas(h: &HubHandle) -> Vec<EngineConfigSchema> {
    h.inner
        .factories
        .iter()
        .flat_map(|f| f.config_schema())
        .collect()
}

pub(crate) fn events(h: &HubHandle, engine: &EngineId) -> HubStream<Feed<EngineEvent>> {
    events::subscribe(&h.inner, engine)
}

// ── misc ops ───────────────────────────────────────────────────────────────────────────

pub(crate) fn save_file(h: &HubHandle, path: PathBuf, bytes: Bytes) -> HubCall<()> {
    let (tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        let r = write_atomic_async(&path, &bytes)
            .await
            .map_err(|e| EngineError::Api {
                status: 0,
                message: format!("couldn't save {}: {e}", path.display()),
            });
        let _ = tx.send(r);
    });
    call
}

/// Atomic, durable save: unique temp file in the same directory, `sync_all` (data on disk
/// before it becomes visible under `path`), then rename. The temp file is removed on error.
async fn write_atomic_async(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = config::temp_path(path);
    let res = async {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await?;
        f.write_all(bytes).await?;
        f.sync_all().await?;
        drop(f);
        tokio::fs::rename(&tmp, path).await
    }
    .await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    res
}

pub(crate) fn launch(h: &HubHandle, argv: Vec<String>) -> HubCall<()> {
    if argv.first().is_none_or(|p| p.trim().is_empty()) {
        return HubCall::ready(Err(EngineError::Api {
            status: 0,
            message: "empty command line".into(),
        }));
    }
    let (tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        // argv vector only, never a shell (NFR-022).
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(mut child) => {
                let _ = tx.send(Ok(()));
                // Reap the detached child.
                let _ = child.wait().await;
            }
            Err(e) => {
                let _ = tx.send(Err(EngineError::Api {
                    status: 0,
                    message: format!("couldn't start '{}': {e}", argv[0]),
                }));
            }
        }
    });
    call
}

fn state_label(s: &EngineState) -> String {
    match s {
        EngineState::Disconnected => "disconnected".into(),
        EngineState::Connecting => "connecting".into(),
        EngineState::Connected => "connected".into(),
        EngineState::Degraded => "degraded".into(),
        EngineState::Failed { error, .. } => format!("failed ({error})"),
        EngineState::Disabled => "disabled".into(),
        EngineState::Stopped => "stopped".into(),
        EngineState::Unsupported { reason } => format!("unsupported ({reason})"),
    }
}

/// Diagnostics text (SET-060). Never includes secrets or env values (NFR-020).
pub(crate) fn diagnostics_text(inner: &HubInner) -> String {
    use std::fmt::Write as _;
    let reg = lock(&inner.reg);
    let mut out = String::new();
    let _ = writeln!(out, "Dockering {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "OS: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        out,
        "Active engine: {}",
        reg.active.as_ref().map(|i| i.as_str()).unwrap_or("none")
    );
    let _ = writeln!(out, "Engines:");
    for e in &reg.entries {
        let kind = e.kind().map(|k| k.label()).unwrap_or("unknown");
        let _ = writeln!(
            out,
            "- {} [{}] {}: {}",
            e.config.id,
            kind,
            e.config.endpoint.canonical_kind(),
            state_label(&e.state)
        );
        if let Some(info) = &e.info {
            let _ = writeln!(
                out,
                "    server {} · API {} · {}/{}",
                info.server_version,
                info.api_version.as_deref().unwrap_or("-"),
                info.os,
                info.arch
            );
            if let Some(t) = &info.transport {
                let _ = write!(out, "    transport {t}");
                if let Some(n) = &info.transport_note {
                    let _ = write!(out, " ({n})");
                }
                let _ = writeln!(out);
            }
            let _ = writeln!(
                out,
                "    capabilities: {}",
                info.capabilities.names().join(", ")
            );
        }
        if !e.also_reachable_via.is_empty() {
            let _ = writeln!(
                out,
                "    also reachable via: {}",
                e.also_reachable_via.join(", ")
            );
        }
    }
    out
}

pub(crate) fn diagnostics(h: &HubHandle) -> HubCall<String> {
    HubCall::ready(Ok(diagnostics_text(&h.inner)))
}

/// Endpoint kind label for diagnostics (no hosts/paths: they can be sensitive).
trait CanonicalKind {
    fn canonical_kind(&self) -> &'static str;
}

impl CanonicalKind for dk_core::EngineEndpoint {
    fn canonical_kind(&self) -> &'static str {
        use dk_core::EngineEndpoint as E;
        match self {
            E::UnixSocket { .. } => "unix socket",
            E::NamedPipe { .. } => "named pipe",
            E::Tcp { tls: Some(_), .. } => "tcp+tls",
            E::Tcp { .. } => "tcp",
            E::WslDistro { .. } => "wsl distro",
            E::Wslc { .. } => "wslc",
            E::Ssh { .. } => "ssh",
            E::Unknown => "unknown",
        }
    }
}

pub(crate) fn config_handle(h: &HubHandle) -> ConfigHandle {
    ConfigHandle {
        inner: h.inner.clone(),
    }
}

pub(crate) fn shutdown(h: &HubHandle) {
    let inner = &h.inner;
    inner.shutdown.cancel();
    inner.terminals.close_all();
    // Pending saves are flushed synchronously (two small files), bounded by the disk.
    inner.flush_now();
    if let Some(rt) = lock(&inner.rt).take() {
        let spawned = std::thread::Builder::new()
            .name("dk-hub-shutdown".into())
            .spawn(move || rt.shutdown_timeout(SHUTDOWN_TIMEOUT));
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "couldn't spawn the hub shutdown thread");
        }
    }
}

impl HubInner {
    pub(crate) fn ui_state(&self) -> UiState {
        lock(&self.ui_state).clone()
    }
}

pub(crate) fn config_get(inner: &Arc<HubInner>) -> Config {
    lock(&inner.config).clone()
}
pub(crate) fn config_update(inner: &Arc<HubInner>, f: impl FnOnce(&mut Config)) {
    #[cfg(feature = "updater")]
    let before = lock(&inner.config).updates.check;
    inner.update_config(f);
    #[cfg(feature = "updater")]
    if lock(&inner.config).updates.check != before {
        crate::updates::settings_changed(inner);
    }
}

// ── updates (UPD-*) ─────────────────────────────────────────────────────────────────────────

pub(crate) fn update_status(h: &HubHandle) -> HubStream<crate::updates::UpdateStatus> {
    use crate::updates::UpdateStatus;
    #[cfg(feature = "updater")]
    if let Some(u) = h.inner.updates.as_ref() {
        let mut rx = u.status.subscribe();
        let (mut tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
        h.inner.handle.spawn(async move {
            loop {
                let item = rx.borrow_and_update().clone();
                tokio::select! {
                    biased;
                    _ = token.cancelled() => return,
                    r = tx.send(Ok(item)) => if r.is_err() { return },
                }
                tokio::select! {
                    biased;
                    _ = token.cancelled() => return,
                    r = rx.changed() => if r.is_err() { return },
                }
            }
        });
        return stream;
    }
    let _ = h;
    let (mut tx, _token, stream) = HubStream::channel(1);
    let _ = tx.try_send(Ok(UpdateStatus::Disabled { by_policy: false }));
    stream
}

pub(crate) fn check_for_updates(h: &HubHandle) -> HubCall<crate::updates::UpdateCheck> {
    #[cfg(feature = "updater")]
    if h.inner.updates.is_some() {
        let (tx, call) = HubCall::channel();
        let inner = h.inner.clone();
        h.inner.handle.spawn(async move {
            let _ = tx.send(crate::updates::check(&inner, true).await);
        });
        return call;
    }
    let _ = h;
    HubCall::ready(Ok(crate::updates::UpdateCheck::Disabled))
}

pub(crate) fn apply_update(h: &HubHandle) -> HubCall<()> {
    #[cfg(feature = "updater")]
    {
        let (tx, call) = HubCall::channel();
        let inner = h.inner.clone();
        h.inner.handle.spawn_blocking(move || {
            let _ = tx.send(crate::updates::apply(&inner));
        });
        call
    }
    #[cfg(not(feature = "updater"))]
    {
        let _ = h;
        HubCall::ready(Err(EngineError::protocol(
            "updates are not available in this build",
        )))
    }
}

pub(crate) fn take_previous_version(h: &HubHandle) -> Option<String> {
    let current = env!("CARGO_PKG_VERSION");
    let previous = h.inner.ui_state().updates.last_run_version;
    if previous.as_deref() == Some(current) {
        return None;
    }
    h.inner
        .update_ui_state(|s| s.updates.last_run_version = Some(current.to_owned()));
    previous.filter(|p| crate::updates::older(p, current))
}

pub(crate) fn mark_update_notified(h: &HubHandle, version: &str) -> bool {
    if h.inner.ui_state().updates.notified_version.as_deref() == Some(version) {
        return false;
    }
    h.inner
        .update_ui_state(|s| s.updates.notified_version = Some(version.to_owned()));
    true
}
pub(crate) fn ui_state_get(inner: &Arc<HubInner>) -> UiState {
    inner.ui_state()
}
pub(crate) fn ui_state_update(inner: &Arc<HubInner>, f: impl FnOnce(&mut UiState)) {
    inner.update_ui_state(f)
}
