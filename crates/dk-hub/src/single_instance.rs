//! Single instance per user (SHL-022). A second launch asks the running instance to focus its
//! window and exits. Windows: named pipe `\\.\pipe\dockering-<user-sid-or-name>`;
//! Unix: socket in the runtime/data dir. SKELETON — implemented by `rust-core`.

use crate::paths::Paths;

/// Held by the primary instance for the app's lifetime.
pub struct InstanceGuard {
    _private: (),
}

impl InstanceGuard {
    /// Focus requests from later launches. Can be taken once; later calls return `None`.
    pub fn take_focus_requests(&mut self) -> Option<futures::channel::mpsc::UnboundedReceiver<()>> {
        unimplemented!("rust-core")
    }
}

pub enum Instance {
    Primary(InstanceGuard),
    /// Another instance is running and was asked to focus; the caller should exit.
    Secondary,
}

/// Startup only (before GPUI). Never fails hard: on any error behaves as `Primary`.
pub fn acquire(paths: &Paths) -> Instance {
    let _ = paths;
    unimplemented!("rust-core")
}
