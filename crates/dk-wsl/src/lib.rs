//! dk-wsl: WSL distro discovery, `wsl.exe` runner, and the named-pipe ⇄ `wsl.exe` stdio
//! bridge (spec 20 §4, ADR-0004). Windows-only code sits behind `#[cfg(windows)]`; on other
//! OSes discovery returns nothing and the crate still builds.
//!
//! PUBLIC API FIXED — implemented by `windows-platform`.

mod factory;

pub use factory::WslDistroFactory;
