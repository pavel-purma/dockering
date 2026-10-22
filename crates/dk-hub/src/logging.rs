//! Tracing setup and panic hook (spec 10 §7–8).

use std::backtrace::Backtrace;
use std::io::Write as _;
use std::path::PathBuf;

use tracing::field::{Field, Visit};
use tracing::{Event, Metadata};
use tracing_log::NormalizeEvent as _;
use tracing_subscriber::filter::FilterExt as _;
use tracing_subscriber::layer::{Context, Filter, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::LogLevel;
use crate::paths::Paths;

/// Keeps the non-blocking log writer alive; drop at exit to flush.
pub struct LoggingGuard {
    _private: (),
    _file: Option<tracing_appender::non_blocking::WorkerGuard>,
}

fn filter<S>(level: LogLevel) -> impl Filter<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_directive(level)))
        .and(KnownNoise)
}

fn default_directive(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
    }
}

/// Error records from GPUI's Windows backend that need no attention (NFR-050): the optional
/// DirectX debug layer being absent (debug builds) and window handles already gone while the
/// last window closes. Matched on the HRESULT because the OS message text is localised.
const KNOWN_NOISE_HRESULTS: [&str; 3] = ["(0x887A002D)", "(0x80040102)", "(0x80070578)"];

fn is_known_noise(message: &str) -> bool {
    message == "window not found"
        || KNOWN_NOISE_HRESULTS
            .iter()
            .any(|code| message.ends_with(code))
}

#[derive(Default)]
struct MessageText(String);

impl Visit for MessageText {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// Drops [`is_known_noise`] records that arrive through the `log` crate; our own `tracing`
/// events always pass.
struct KnownNoise;

impl<S> Filter<S> for KnownNoise {
    fn enabled(&self, _: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        true
    }

    fn event_enabled(&self, event: &Event<'_>, _: &Context<'_, S>) -> bool {
        if !event.is_log() {
            return true;
        }
        let mut message = MessageText::default();
        event.record(&mut message);
        !is_known_noise(&message.0)
    }
}

/// Rotating daily logs in `paths.log_dir` (keep 7) + stderr in debug builds.
/// `RUST_LOG` overrides `level`. Safe to call once.
pub fn init(paths: &Paths, level: LogLevel) -> LoggingGuard {
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
                .with_filter(filter(level));
            (Some(layer), Some(guard), None)
        }
        Err(e) => (None, None, Some(e)),
    };

    let stderr_layer = cfg!(debug_assertions).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(filter(level))
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
    writeln!(f, "Dockering {} crashed", crate::app_version())?;
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
    use std::io::Write;

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

    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn captured(emit: impl FnOnce()) -> String {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(KnownNoise),
        );
        tracing::subscriber::with_default(subscriber, emit);
        let bytes = capture.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    fn log_error(message: &str) {
        tracing_log::format_trace(
            &tracing_log::log::Record::builder()
                .level(tracing_log::log::Level::Error)
                .target("")
                .args(format_args!("{message}"))
                .build(),
        )
        .unwrap();
    }

    #[test]
    fn nfr_050_known_gpui_noise_is_dropped() {
        let out = captured(|| {
            log_error("window not found");
            log_error("Invalid window handle (0x80040102)");
            log_error("Invalid window handle. (0x80070578)");
            log_error(
                "The application requested an operation that depends on an SDK component that is missing or mismatched. (0x887A002D)",
            );
            log_error("Ungültiges Fensterhandle. (0x80070578)");
        });
        assert_eq!(out, "");
    }

    #[test]
    fn nfr_050_other_records_still_pass() {
        let out = captured(|| {
            log_error("device lost (0x887A0005)");
            log_error("something broke");
            tracing::error!("window not found");
            tracing::error!("Invalid window handle (0x80040102)");
        });
        assert_eq!(out.lines().count(), 4, "{out}");
        assert!(out.contains("device lost (0x887A0005)"), "{out}");
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
