//! Native COM transport (ADR-0003, spec 20 §5.3–5.4). Owner: `windows-platform`.
//!
//! - `ffi`: CoTaskMem RAII, proxy blankets, process COM security, HRESULT mapping.
//! - `win32`: non-COM Win32 helpers (version resources, registry, Toolhelp, handle I/O).
//! - `abi`: version-gated vtable declarations; `abi::select` is the only way in.
//!
//! `unsafe` is allowed in this module tree only; every block carries a `// SAFETY:` note.
//!
//! Visibility (ADR-0003): this module is crate-private in normal builds (`lib.rs`); with
//! feature `test-support` it is public for the fake server and the contract/live tests. Even
//! then the vtable declarations (`abi`) stay crate-private: only the ABI selection metadata and
//! the one interface the fake hands out are re-exported, under that cfg.

pub(crate) mod abi;
pub mod convert;
#[cfg(windows)]
pub(crate) mod dispatch;
#[cfg(windows)]
pub mod engine;
#[cfg(all(windows, any(test, feature = "test-support")))]
pub mod fake;
pub mod ffi;
#[cfg(windows)]
pub(crate) mod pool;
#[cfg(windows)]
pub(crate) mod stream;
#[cfg(windows)]
pub(crate) mod win32;

#[cfg(all(windows, any(test, feature = "test-support")))]
pub use engine::{SelfCheck, SessionEntry};
#[cfg(windows)]
pub use engine::{WslcComEngine, list_sessions};

/// ABI selection for tests (`WslcComEngine::connect` takes the selected module). No vtables.
#[cfg(any(test, feature = "test-support"))]
pub use abi::{AbiInfo, AbiModule, MODULES, select};

/// See [`crate::init_process_com_security`].
pub fn init_process_com_security() -> bool {
    ffi::init_process_com_security()
}
