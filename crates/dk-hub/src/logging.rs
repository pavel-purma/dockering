//! Tracing setup and panic hook (spec 10 §7–8).

use std::backtrace::Backtrace;
use std::io::Write as _;
use std::path::PathBuf;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::LogLevel;
use crate::paths::Paths;

/// Keeps the non-blocking log writer alive; drop at exit to flush.
pub struct LoggingGuard {
    _private: (),
    _file: Option<tracing_appender::non_blocking::WorkerGuard>,
}

fn default_directive(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
    }
}

/// Rotating daily logs in `paths.log_dir` (keep 7) + stderr in debug builds.
/// `RUST_LOG` overrides `level`. Safe to call once.
pub fn init(paths: &Paths, level: LogLevel) -> LoggingGuard {
    let filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(default_directive(level)))
    };

    let file = std::fs::create_dir_all(&paths.log_dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("dockering")
                .filename_suffix("log")
                .max_log_files(7)
                .build(&paths.log_dir)
                .map_err(|e| e.to_string())
        });
    let (file_layer, guard, file_err) = match file {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(filter());
            (Some(layer), Some(guard), None)
        }
        Err(e) => (None, None, Some(e)),
    };

    let stderr_layer = cfg!(debug_assertions).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(filter())
    });

    let _ = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    if let Some(e) = file_err {
        tracing::warn!(error = %e, dir = %paths.log_dir.display(), "file logging disabled");
    }
    LoggingGuard {
        _private: (),
        _file: guard,
    }
}

/// Logs panics via `tracing` and writes `crash-<timestamp>.txt` to `paths.data_dir`.
pub fn install_panic_hook(paths: &Paths) {
    let dir = paths.data_dir.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = crate::hub::panic_message(info.payload());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        tracing::error!(panic = %msg, %location, %thread, "panic");
        // Panics on hub threads are caught and mapped to errors (NFR-030): not a crash.
        if !thread.starts_with("dk-hub-")
            && let Err(e) = write_crash_file(&dir, &msg, &location, &thread)
        {
            tracing::error!(error = %e, "couldn't write crash file");
        }
        previous(info);
    }));
}

fn write_crash_file(
    dir: &std::path::Path,
    msg: &str,
    location: &str,
    thread: &str,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{ts}.txt"));
    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "Dockering {} crashed", env!("CARGO_PKG_VERSION"))?;
    writeln!(f, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH)?;
    writeln!(f, "thread: {thread}")?;
    writeln!(f, "message: {msg}")?;
    writeln!(f, "location: {location}")?;
    writeln!(f, "\nbacktrace:\n{}", Backtrace::force_capture())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_writes_rotating_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let guard = init(&paths, LogLevel::Info);
        tracing::info!("hello from the log test");
        drop(guard); // flushes the non-blocking writer
        let files: Vec<_> = std::fs::read_dir(&paths.log_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].starts_with("dockering") && files[0].ends_with(".log"));
        // A second init is harmless (try_init).
        let _g = init(&paths, LogLevel::Debug);
    }

    #[test]
    fn crash_file_contains_message_and_location() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_crash_file(dir.path(), "boom", "src/x.rs:1:2", "main").unwrap();
        let text = std::fs::read_to_string(p).unwrap();
        assert!(text.contains("boom") && text.contains("src/x.rs:1:2"));
        assert!(text.contains("backtrace:"));
    }
}
