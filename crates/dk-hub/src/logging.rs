//! Tracing setup and panic hook (spec 10 §7–8). SKELETON — implemented by `rust-core`.

use crate::config::LogLevel;
use crate::paths::Paths;

/// Keeps the non-blocking log writer alive; drop at exit to flush.
pub struct LoggingGuard {
    _private: (),
}

/// Rotating daily logs in `paths.log_dir` (keep 7) + stderr in debug builds.
/// `RUST_LOG` overrides `level`. Safe to call once.
pub fn init(paths: &Paths, level: LogLevel) -> LoggingGuard {
    let _ = (paths, level);
    unimplemented!("rust-core")
}

/// Logs panics via `tracing` and writes `crash-<timestamp>.txt` to `paths.data_dir`.
pub fn install_panic_hook(paths: &Paths) {
    let _ = paths;
    unimplemented!("rust-core")
}
