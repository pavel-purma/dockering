//! dk-wsl: WSL distro discovery, the `wsl.exe` runner, and the named-pipe ⇄ `wsl.exe` stdio
//! bridge (spec 20 §4, ADR-0004).
//!
//! Windows-only code sits behind `#[cfg(windows)]`. On other OSes discovery returns nothing,
//! `connect` fails with `Unreachable("WSL is only available on Windows")`, and the crate still
//! builds. All `unsafe` lives in `win32` (Windows only).

// The parsers/classifiers are platform-neutral (and unit-tested everywhere) but only called from
// the Windows code paths.
#![cfg_attr(not(windows), allow(dead_code))]

mod bridge;
mod discovery;
mod factory;
mod runner;
#[cfg(windows)]
mod win32;

pub use bridge::{IDLE_TIMEOUT as BRIDGE_IDLE_TIMEOUT, PipeBridge};
pub use discovery::BridgeTool;
pub use factory::WslDistroFactory;
