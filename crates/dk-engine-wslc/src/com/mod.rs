//! Native COM transport (ADR-0003, spec 20 §5.3–5.4). Owner: `windows-platform`.
//!
//! - `ffi`: CoTaskMem RAII, proxy blankets, process COM security, HRESULT mapping.
//! - `win32`: non-COM Win32 helpers (version resources, registry, Toolhelp, handle I/O).
//! - `abi`: version-gated vtable declarations; `abi::select` is the only way in.
//!
//! `unsafe` is allowed in this module tree only; every block carries a `// SAFETY:` note.

pub mod abi;
pub mod convert;
#[cfg(windows)]
pub mod engine;
pub mod ffi;
#[cfg(windows)]
pub mod pool;
#[cfg(windows)]
pub mod stream;
#[cfg(windows)]
pub mod win32;

#[cfg(windows)]
pub use engine::{SelfCheck, WslcComEngine};

/// See [`crate::init_process_com_security`].
pub fn init_process_com_security() -> bool {
    ffi::init_process_com_security()
}
