//! dk-hub: the `EngineHub` — owns the dedicated tokio runtime and every engine connection
//! (spec 10 §3, ADR-0002). The public API exposes only `futures`-based types.

pub mod bridge;
pub mod config;
pub mod console;
mod e2e;
mod events;
mod handle;
mod hub;
pub mod logging;
pub mod paths;
mod registry;
mod setup;
pub mod single_instance;
mod stats_service;
mod supervisor;
mod terminal;
pub mod updates;
mod version;

#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "updater"))]
mod updates_apply_tests;
#[cfg(all(test, feature = "updater"))]
mod updates_offer_tests;
#[cfg(all(test, feature = "updater"))]
mod updates_tests;

pub use bridge::{Feed, HubCall, HubEvent, HubStream, STREAM_CAPACITY, TermCmd, TerminalHandle};
pub use config::{Config, ThemeMode, UiState, UpdateSettings};
pub use e2e::e2e_quit_delay;
pub use handle::{ConfigHandle, HubHandle, HubOptions};
pub use hub::EngineHub;
pub use paths::Paths;
pub use supervisor::backoff_delay;
pub use updates::{
    ALREADY_APPLYING, ApplyOutcome, DisabledReason, OfferMode, OfferStage, UpdateCheck,
    UpdateOffer, UpdateSnapshot, UpdateState, UpdateStatus,
};
pub use version::app_version;

/// The backend factories for this OS (spec 21 §8): Docker everywhere; WSL distro + WSLC on
/// Windows (their crates compile everywhere and discover nothing elsewhere).
pub fn default_factories() -> Vec<std::sync::Arc<dyn dk_core::EngineFactory>> {
    vec![
        std::sync::Arc::new(dk_engine_docker::DockerFactory::new()),
        std::sync::Arc::new(dk_wsl::WslDistroFactory::new()),
        std::sync::Arc::new(dk_engine_wslc::WslcFactory::new()),
    ]
}

/// Process-wide platform setup that MUST run in `main()` before GPUI starts (spec 10 §7):
/// on Windows `WSAStartup` + `CoInitializeSecurity` (via `dk-engine-wslc`); no-op elsewhere.
/// Returns whether COM security was initialised.
pub fn init_platform() -> bool {
    dk_engine_wslc::init_process_com_security()
}

/// Load `config.toml` + `state.json` synchronously (startup only, before the first window).
/// Missing/corrupt files → defaults (a corrupt file is renamed to `*.bak` and logged).
pub fn load_config(paths: &Paths) -> (Config, UiState) {
    config::load(paths)
}
