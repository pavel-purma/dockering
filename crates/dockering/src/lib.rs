//! Dockering UI (GPUI Kit). The binary in `main.rs` only bootstraps the process; everything
//! else lives in this library so view tests can drive it with `TestAppContext`.
//!
//! Module map (see also the final report in the PR):
//! - [`app`]: GPUI bootstrap (theme, keymap, globals, main window, quit handling).
//! - [`actions`], [`keymap`], [`commands`]: every user command (KBD-002), the per-OS binding
//!   table (KBD-080), and the command-palette registry (KBD-020).
//! - [`state`]: `AppState`, `Resource<T>`, `EngineListStore`, `EngineStore`, the relative-time
//!   ticker (spec 10 §4).
//! - [`nav`]: `Route` + `Navigator` (spec 30 §2).
//! - [`shell`]: `AppShell` (title bar, sidebar, status bar, regions, switcher, palette,
//!   shortcut reference, menus).
//! - [`ui`]: reusable pieces for every page (ListTable, confirm, notify, chips, states…).
//! - [`pages`]: Containers (M2) and the phase-2 placeholders.

pub mod actions;
pub mod app;
pub mod assets;
pub mod commands;
#[cfg(any(test, feature = "demo"))]
pub mod demo;
pub mod keymap;
pub mod nav;
pub mod pages;
pub mod shell;
pub mod state;
pub mod strings;
pub mod theme;
pub mod ui;

#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod view_tests;
#[cfg(test)]
mod view_tests_detail;
#[cfg(test)]
mod view_tests_m6;
#[cfg(test)]
mod view_tests_settings;
#[cfg(test)]
mod view_tests_updates;
