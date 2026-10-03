//! `HubHandle`: the UI's only door into the hub (spec 10 §3.3). Signatures are fixed (other
//! crates build against them); bodies live in `hub.rs` and the service modules.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use dk_core::{
    Engine, EngineConfig, EngineConfigSchema, EngineError, EngineEvent, EngineId, EngineInfo,
    EngineResult, EngineStatus, ExecRequest, StatsSample,
};
use futures::Stream;

use crate::bridge::{Feed, HubCall, HubEvent, HubStream, TerminalHandle};
use crate::config::{Config, UiState};
use crate::paths::Paths;
use crate::updates::{UpdateCheck, UpdateStatus};

/// Options for `EngineHub::start`.
pub struct HubOptions {
    pub paths: Paths,
    /// Config loaded synchronously before the first window (spec 10 §7).
    pub config: Config,
    pub ui_state: UiState,
    /// Backend factories. `None` = the default set for this OS (`crate::default_factories()`).
    pub factories: Option<Vec<Arc<dyn dk_core::EngineFactory>>>,
    /// Run discovery at start (tests disable it).
    pub discover_on_start: bool,
    /// Worker threads (2–4).
    pub worker_threads: usize,
    /// `--demo`: the updater stays off (UPD-005).
    pub demo: bool,
}

/// Cheap to clone (`Arc` inside); stored as a GPUI `Global` by the app.
#[derive(Clone)]
pub struct HubHandle {
    pub(crate) inner: Arc<crate::hub::HubInner>,
}

impl HubHandle {
    // ── generic engine access (closures run on the hub runtime) ───────────────────────────

    /// If the engine isn't Connected/Degraded: fails fast with `EngineError::Unreachable`.
    pub fn call<T, F, C>(&self, engine: &EngineId, f: C) -> HubCall<T>
    where
        C: FnOnce(Arc<dyn Engine>) -> F + Send + 'static,
        F: Future<Output = EngineResult<T>> + Send + 'static,
        T: Send + 'static,
    {
        crate::hub::call(self, engine, f)
    }

    /// If the engine isn't connected: yields one `Err(Unreachable)` and ends.
    pub fn subscribe<T, S, C>(&self, engine: &EngineId, f: C) -> HubStream<T>
    where
        C: FnOnce(Arc<dyn Engine>) -> S + Send + 'static,
        S: Stream<Item = EngineResult<T>> + Send + 'static,
        T: Send + 'static,
    {
        crate::hub::subscribe(self, engine, f)
    }

    // ── engines (ENG-*) ────────────────────────────────────────────────────────────────────

    /// First item is always `HubEvent::Snapshot`.
    pub fn hub_events(&self) -> HubStream<Feed<HubEvent>> {
        crate::hub::hub_events(self)
    }
    pub fn engines(&self) -> HubCall<Vec<EngineStatus>> {
        crate::hub::engines(self)
    }
    /// Current active engine id (synchronous snapshot, lock-free read).
    pub fn active_engine(&self) -> Option<EngineId> {
        crate::hub::active_engine(self)
    }
    pub fn set_active(&self, id: &EngineId) -> HubCall<()> {
        crate::hub::set_active(self, id)
    }
    pub fn add_engine(&self, cfg: EngineConfig) -> HubCall<EngineId> {
        crate::hub::add_engine(self, cfg)
    }
    pub fn update_engine(&self, cfg: EngineConfig) -> HubCall<()> {
        crate::hub::update_engine(self, cfg)
    }
    pub fn remove_engine(&self, id: &EngineId) -> HubCall<()> {
        crate::hub::remove_engine(self, id)
    }
    pub fn test_engine(&self, cfg: EngineConfig) -> HubCall<EngineInfo> {
        crate::hub::test_engine(self, cfg)
    }
    pub fn rescan(&self) -> HubCall<()> {
        crate::hub::rescan(self)
    }
    /// Manual retry: resets backoff (ENG-021).
    pub fn retry(&self, id: &EngineId) -> HubCall<()> {
        crate::hub::retry(self, id)
    }
    /// ENG-106: explicit user action only.
    pub fn start_wsl_distro(&self, id: &EngineId) -> HubCall<()> {
        crate::hub::start_engine(self, id)
    }
    /// Add-engine dialog schemas from all registered factories.
    pub fn engine_schemas(&self) -> Vec<EngineConfigSchema> {
        crate::hub::engine_schemas(self)
    }

    // ── services owned by the hub ─────────────────────────────────────────────────────────

    /// Shared, deduped per engine.
    pub fn events(&self, engine: &EngineId) -> HubStream<Feed<EngineEvent>> {
        crate::hub::events(self, engine)
    }
    /// StatsService: replays buffered history first (STA-006).
    pub fn stats(&self, engine: &EngineId, id: &str) -> HubStream<Feed<StatsSample>> {
        crate::stats_service::subscribe(self, engine, id)
    }
    pub fn open_terminal(
        &self,
        engine: &EngineId,
        id: &str,
        req: ExecRequest,
    ) -> HubCall<TerminalHandle> {
        crate::terminal::open(self, engine, id, req)
    }
    /// LOG-004 export etc. Atomic write on the hub runtime.
    pub fn save_file(&self, path: PathBuf, bytes: Bytes) -> HubCall<()> {
        crate::hub::save_file(self, path, bytes)
    }
    /// Spawn a detached child process with an argv vector (TRM-009 external terminal).
    pub fn launch(&self, argv: Vec<String>) -> HubCall<()> {
        crate::hub::launch(self, argv)
    }
    /// Multi-line diagnostics text (SET-060 "Copy diagnostics").
    pub fn diagnostics(&self) -> HubCall<String> {
        crate::hub::diagnostics(self)
    }
    pub fn config(&self) -> ConfigHandle {
        crate::hub::config_handle(self)
    }

    // ── updates (UPD-*) ───────────────────────────────────────────────────────────────────

    /// Current updater status first, then every change (UPD-008).
    pub fn update_status(&self) -> HubStream<UpdateStatus> {
        crate::hub::update_status(self)
    }
    /// Manual check (+ download when the install supports it). Errors surface (UPD-004).
    pub fn check_for_updates(&self) -> HubCall<UpdateCheck> {
        crate::hub::check_for_updates(self)
    }
    /// Starts the verified installer; the UI quits right after (UPD-007).
    pub fn apply_update(&self) -> HubCall<()> {
        crate::hub::apply_update(self)
    }
    /// The version that ran before this one, if older (once per upgrade; "Updated to X.Y.Z").
    /// Records the current version as the last run.
    pub fn take_previous_version(&self) -> Option<String> {
        crate::hub::take_previous_version(self)
    }
    /// Marks the "ready to install" notification for `version` as shown; `true` the first time.
    pub fn mark_update_notified(&self, version: &str) -> bool {
        crate::hub::mark_update_notified(self, version)
    }
    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }
    /// Stops the runtime; flushes pending config saves (best effort, bounded).
    pub fn shutdown(&self) {
        crate::hub::shutdown(self)
    }
}

/// Read snapshots / write (debounced 500 ms save) of `config.toml` and `state.json`.
/// Reads are a brief uncontended lock + clone; never held across awaits.
#[derive(Clone)]
pub struct ConfigHandle {
    pub(crate) inner: Arc<crate::hub::HubInner>,
}

impl ConfigHandle {
    pub fn get(&self) -> Config {
        crate::hub::config_get(&self.inner)
    }
    /// Mutate and schedule a debounced save.
    pub fn update(&self, f: impl FnOnce(&mut Config)) {
        crate::hub::config_update(&self.inner, f)
    }
    pub fn ui_state(&self) -> UiState {
        crate::hub::ui_state_get(&self.inner)
    }
    pub fn update_ui_state(&self, f: impl FnOnce(&mut UiState)) {
        crate::hub::ui_state_update(&self.inner, f)
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn is<T: Send + Sync + Clone>() {}
    is::<HubHandle>();
    let _ = EngineError::Cancelled;
}
