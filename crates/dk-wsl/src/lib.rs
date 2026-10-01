//! dk-wsl: WSL distro discovery, `wsl.exe` runner, and the named-pipe ⇄ `wsl.exe` stdio
//! bridge (spec 20 §4, ADR-0004). Windows-only code sits behind `#[cfg(windows)]`; on other
//! OSes discovery returns nothing and the crate still builds.

mod bridge;
mod discovery;
mod factory;
#[cfg(windows)]
mod runner;
#[cfg(windows)]
mod win32;

pub use bridge::PipeBridge;
pub use discovery::BridgeTool;
pub use factory::WslDistroFactory;
