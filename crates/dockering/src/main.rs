//! Dockering app binary: process bootstrap (spec 10 §7). Everything here runs before the first
//! window opens, so the blocking calls below are allowed (NFR-001 startup exception).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::io::Write;
use std::process::ExitCode; // nfr-001-allow: exit-code type only, no process I/O
use std::sync::Arc;

use dk_hub::single_instance::{Instance, WaitOutcome, WaitResult};
use dk_hub::{EngineHub, HubOptions, Paths};
use dockering::app::{Boot, run};

struct Args {
    version: bool,
    demo: bool,
    /// `--update-relaunch=1` (UPD-017): wait for the previous instance before anything else.
    /// Never set together with `--version` or `--demo`, which never relaunch.
    relaunch: bool,
}

/// `argv` is the command line without the program name. Unknown arguments are ignored (frozen:
/// an old binary starts a new one with the relaunch flag, UPD-017).
fn parse_args(argv: &[String]) -> Args {
    let mut args = Args {
        version: false,
        demo: false,
        relaunch: false,
    };
    for arg in argv {
        match arg.as_str() {
            "--version" | "-V" => args.version = true,
            "--demo" => args.demo = true,
            _ => {}
        }
    }
    args.relaunch = dk_hub::single_instance::is_relaunch(argv) && !args.version && !args.demo;
    args
}

fn say(out: &mut impl Write, line: &str) {
    dk_hub::console::attach_parent_console();
    let _ = writeln!(out, "{line}");
}

/// UPD-017: one log line per relaunch wait, once logging is up. A plain launch logs nothing.
fn log_relaunch_wait(wait: &WaitResult) {
    match wait.outcome {
        WaitOutcome::NotRequested => {}
        WaitOutcome::TimedOut => tracing::warn!(
            waited_ms = wait.waited_ms,
            outcome = ?wait.outcome,
            "update relaunch: the previous instance did not exit in time; continuing as a normal launch"
        ),
        _ => tracing::info!(
            waited_ms = wait.waited_ms,
            outcome = ?wait.outcome,
            "update relaunch: wait finished"
        ),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv);
    if args.version {
        say(
            &mut std::io::stdout(),
            &format!("dockering {}", dk_hub::app_version()),
        );
        return ExitCode::SUCCESS;
    }
    if args.demo && !cfg!(feature = "demo") {
        say(
            &mut std::io::stderr(),
            "dockering: this build has no demo support (feature `demo` disabled)",
        );
        return ExitCode::FAILURE;
    }

    let paths = if args.demo {
        let pid = std::process::id(); // nfr-001-allow: startup before first window
        let root = std::env::temp_dir().join(format!("dockering-demo-{pid}"));
        Paths::in_dir(root)
    } else {
        match Paths::for_user() {
            Some(paths) => paths,
            None => {
                say(
                    &mut std::io::stderr(),
                    "dockering: cannot determine the user's home directory",
                );
                return ExitCode::FAILURE;
            }
        }
    };

    // Panic hook first, so a crash anywhere below still leaves a crash file.
    dk_hub::logging::install_panic_hook(&paths);
    // UPD-017, SHL-022: an update relaunch (`--update-relaunch=1`, stdin on a pipe) waits here,
    // before the config and state.json are read (so the old instance's final save is never
    // overwritten by a stale copy) and before the single-instance check below (so it never asks
    // the instance that is quitting to focus). The wait is bounded (30 s) and a plain launch,
    // whose stdin is often /dev/null, never waits. The outcome is logged once logging is up.
    let wait_args: &[String] = if args.relaunch { &argv } else { &[] };
    let relaunch_wait = dk_hub::single_instance::wait_for_predecessor_from_args(wait_args);
    // nfr-001-allow: startup before first window (sync config load, spec 10 §6)
    let (config, ui_state) = dk_hub::load_config(&paths);
    let _logging = dk_hub::logging::init(&paths, config.diagnostics.log_level);
    log_relaunch_wait(&relaunch_wait);
    tracing::info!(
        version = dk_hub::app_version(),
        demo = args.demo,
        "starting"
    );

    // SHL-022: a second launch focuses the running window and exits. Demo runs are isolated.
    // After a relaunch wait that timed out, this is the ordinary rule (the wait never sent a
    // focus request).
    let instance = if args.demo {
        None
    } else {
        match dk_hub::single_instance::acquire(&paths) {
            Instance::Primary(guard) => Some(guard),
            Instance::Secondary => {
                tracing::info!("another instance is running; asked it to focus");
                return ExitCode::SUCCESS;
            }
        }
    };

    // Windows: WSAStartup + CoInitializeSecurity must happen before GPUI (spec 10 §7, F-8).
    let com_security = dk_hub::init_platform();
    tracing::debug!(com_security, "platform initialised");

    let factories = if args.demo { demo_factories() } else { None };
    let hub = match EngineHub::start(HubOptions {
        paths,
        config: config.clone(),
        ui_state: ui_state.clone(),
        factories,
        discover_on_start: true,
        worker_threads: 3,
        demo: args.demo,
    }) {
        Ok(hub) => hub,
        Err(err) => {
            tracing::error!(%err, "failed to start the engine hub");
            say(
                &mut std::io::stderr(),
                &format!("dockering: failed to start the engine hub: {err}"),
            );
            return ExitCode::FAILURE;
        }
    };

    run(Boot {
        hub,
        config,
        ui_state,
        instance,
        demo: args.demo,
    });
    // REL-086 test seam, a no-op without the `update-e2e` feature and its variable: linger
    // here, once `run()` has returned (the hub and the single-instance guard were moved into
    // it), so a relaunch that only works when this process exits quickly fails the
    // end-to-end test. The sleep itself lives in dk-hub (NFR-001).
    dk_hub::e2e_quit_delay();
    ExitCode::SUCCESS
}

#[cfg(feature = "demo")]
fn demo_factories() -> Option<Vec<Arc<dyn dk_core::EngineFactory>>> {
    let (factories, engine) = dockering::demo::factories();
    // Live demo feed (log lines + stats samples) on a plain thread outside the UI; started
    // before the first window (NFR-001 startup exception), it only pushes into the fake.
    std::thread::Builder::new() // nfr-001-allow: demo-only feed thread, never on the UI thread
        .name("demo-feed".into())
        .spawn(move || {
            for n in 0..=u64::MAX {
                dockering::demo::tick(&engine, n);
                std::thread::sleep(std::time::Duration::from_secs(1)); // nfr-001-allow: demo feed thread
            }
        })
        .ok();
    Some(factories)
}

#[cfg(not(feature = "demo"))]
fn demo_factories() -> Option<Vec<Arc<dyn dk_core::EngineFactory>>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn upd_017_parse_args_reads_the_relaunch_flag_exactly() {
        assert!(parse_args(&argv(&["--update-relaunch=1"])).relaunch);
        assert!(parse_args(&argv(&["--update-relaunch=1", "unknown"])).relaunch);
        for other in [
            &[][..],
            &["--update-relaunch=2"],
            &["--update-relaunch"],
            &["--update-relaunch=1x"],
            &["unknown", "--also-unknown"],
        ] {
            let args = parse_args(&argv(other));
            assert!(!args.relaunch, "{other:?}");
            assert!(!args.version && !args.demo, "{other:?}");
        }
    }

    #[test]
    fn shl_022_version_and_demo_never_relaunch() {
        let args = parse_args(&argv(&["--version", "--update-relaunch=1"]));
        assert!(args.version && !args.relaunch);
        let args = parse_args(&argv(&["-V", "--update-relaunch=1"]));
        assert!(args.version && !args.relaunch);
        let args = parse_args(&argv(&["--update-relaunch=1", "--demo"]));
        assert!(args.demo && !args.relaunch);
    }
}
