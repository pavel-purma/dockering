//! UI-side state (spec 10 §4): `AppState` global, `Resource<T>`, `EngineListStore`,
//! `EngineStore`, and the app-wide relative-time ticker (SHL-007).

mod engine_list;
mod engine_store;
mod resource;
mod terminals;
mod ticker;
mod updates;

pub use engine_list::{EngineListEvent, EngineListStore};
pub use engine_store::{Collection, EngineStore, EngineStoreEvent, LiveMode};
pub use resource::Resource;
pub use terminals::{ParkedSessions, TerminalRegistry};
pub use ticker::Ticker;
pub use updates::{ManualCheck, UpdateStore};

use dk_hub::{Config, HubHandle, UiState};
use gpui_kit::{App, Global};

/// App-wide state (spec 10 §4.1). The `HubHandle` is also a GPUI global on its own via
/// [`Hub`], so views can grab it cheaply.
pub struct AppState {
    pub hub: HubHandle,
    /// Config snapshot at the last change made through the UI (the hub owns persistence).
    pub config: Config,
    pub demo: bool,
}

impl Global for AppState {}

/// The hub handle as a GPUI global (spec 10 §3.2).
#[derive(Clone)]
pub struct Hub(pub HubHandle);

impl Global for Hub {}

impl AppState {
    pub fn hub(cx: &App) -> HubHandle {
        cx.global::<Hub>().0.clone()
    }

    pub fn config(cx: &App) -> &Config {
        &cx.global::<AppState>().config
    }

    /// Mutates the config, persists it through the hub (debounced), and refreshes the snapshot.
    ///
    /// The hub's config is the source of truth: it also changes behind the UI's back
    /// (`add_engine`, `update_engine`, `remove_engine` store engine entries), so `f` is applied
    /// to the hub's copy and the snapshot is re-read, never written over the hub's copy.
    pub fn update_config(cx: &mut App, f: impl FnOnce(&mut Config)) {
        let handle = cx.global::<AppState>().hub.config();
        handle.update(f);
        let fresh = handle.get();
        cx.global_mut::<AppState>().config = fresh;
    }

    pub fn ui_state(cx: &App) -> UiState {
        cx.global::<AppState>().hub.config().ui_state()
    }

    pub fn update_ui_state(cx: &App, f: impl FnOnce(&mut UiState)) {
        cx.global::<AppState>().hub.config().update_ui_state(f);
    }
}

/// Registers the globals. Used by `app::run` and by view tests.
pub fn install(hub: HubHandle, config: Config, demo: bool, cx: &mut App) {
    cx.set_global(Hub(hub.clone()));
    UpdateStore::install(hub.clone(), cx);
    cx.set_global(AppState { hub, config, demo });
}
