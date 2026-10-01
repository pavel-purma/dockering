//! dk-engine-wslc: `Engine` for WSL containers (spec 20 §5, ADR-0003).
//! Primary transport: native COM (`IWSLC*`, version-gated ABI modules). Fallback: `wslc.exe` CLI.
//! Windows-only code is behind `#[cfg(windows)]`; elsewhere discovery returns nothing.
//!
//! PUBLIC API FIXED — implemented by `windows-platform` (COM plumbing) + `engine-integrator` (ops).

#![deny(unsafe_code)]

mod factory;

pub use factory::WslcFactory;

/// Process-wide COM/Winsock setup that MUST run in `main()` before GPUI starts (spec 10 §7,
/// spike F-7/F-8): `WSAStartup(2.2)`, then on a short-lived MTA thread `CoInitializeEx(MTA)` +
/// `CoInitializeSecurity(IMPERSONATE, EOAC_STATIC_CLOAKING)`. No-op on non-Windows.
/// Returns whether `CoInitializeSecurity` succeeded (else WSLC uses per-proxy blankets only).
pub fn init_process_com_security() -> bool {
    unimplemented!("windows-platform")
}
