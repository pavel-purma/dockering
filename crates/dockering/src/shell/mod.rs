//! The window shell (spec 30 §1–3): `AppShell` with title bar, sidebar, page area, status
//! bar, focus regions (KBD-005), navigation (spec 30 §2), engine UX (ENG-100…112),
//! the command palette (KBD-020), the shortcut reference (KBD-022), and native menus
//! (SHL-020/021).

pub mod app_shell;
mod engine_views;
pub mod menus;
pub mod palette;
pub mod regions;
pub mod shortcuts;
mod sidebar_nav;
pub mod switcher;

pub use app_shell::{AppShell, ShellPage};
pub use regions::{Region, Regions};

use dk_core::EngineState;
use gpui_kit::{App, Global};

/// Shared shell facts views need without holding the shell (read-only state, SHL-013).
#[derive(Default)]
pub struct ShellState {
    pub engine_state: Option<EngineState>,
}

impl Global for ShellState {}

/// While Degraded (or not connected) data is read-only: actions disabled (SHL-013).
pub fn engine_read_only(cx: &App) -> bool {
    cx.try_global::<ShellState>()
        .and_then(|s| s.engine_state.as_ref())
        .is_some_and(|s| !s.is_connected())
}
