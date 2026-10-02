//! dk-engine-wslc: `Engine` for WSL containers (spec 20 §5, ADR-0003).
//!
//! Two transports, each a complete `dk_core::Engine` implementation, private to this crate:
//! - `com::WslcComEngine` — primary, native COM (`IWSLC*`) through a version-gated ABI module
//!   (owner: `windows-platform` for `com/abi`, `com/ffi`, threading; ops may be shared).
//! - `cli::WslcCliEngine` — fallback, `wslc.exe … --format json` (owner: `engine-integrator`).
//!
//! `WslcFactory` (ENG-008/013/109/110) detects WSL/WSLC without COM, picks the ABI module from
//! the `wslservice.exe` version, self-checks, and falls back to the CLI.
//! Windows-only code is behind `#[cfg(windows)]`; on other OSes discovery returns nothing.
//!
//! `unsafe` is allowed only under `src/com/` (ADR-0003).

#![deny(unsafe_code)]

pub mod cli;
// ADR-0003: the COM transport (version-gated vtables, FFI, threading) is crate-private. Only
// the in-process fake server and the contract/live tests see it, through feature
// `test-support` (enabled by this crate's self dev-dependency).
// In that private build the complete ABI tables (every IDL constant/slot), the fake-server
// allocators and the raw-JSON test accessors are intentionally unused, hence `dead_code`.
#[cfg(not(any(test, feature = "test-support")))]
#[allow(unsafe_code, dead_code)]
mod com;
#[cfg(any(test, feature = "test-support"))]
#[allow(unsafe_code)]
pub mod com;
mod factory;
pub mod version;

pub use factory::WslcFactory;

/// Process-wide COM/Winsock setup that MUST run in `main()` before GPUI starts (spec 10 §7,
/// spike F-7/F-8): `WSAStartup(2.2)`, then on a short-lived MTA thread `CoInitializeEx(MTA)` +
/// `CoInitializeSecurity(IMPERSONATE, EOAC_STATIC_CLOAKING)`. No-op (returns false) on
/// non-Windows. Returns whether `CoInitializeSecurity` succeeded (else WSLC uses per-proxy
/// blankets only and the self-check decides).
pub fn init_process_com_security() -> bool {
    com::init_process_com_security()
}
