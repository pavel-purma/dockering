//! Dockering app binary: process bootstrap (spec 10 §7). Everything here runs before the first
//! window opens, so the blocking calls below are allowed (NFR-001 startup exception).

use std::process::ExitCode;
use std::sync::Arc;

use dk_hub::single_instance::Instance;
use dk_hub::{EngineHub, HubOptions, Paths};
use dockering::app::{Boot, run};

struct Args {
    version: bool,
    demo: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        version: false,
        demo: false,
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => args.version = true,
            "--demo" => args.demo = true,
            _ => {}
        }
    }
    args
}

fn main() -> ExitCode {
    let args = parse_args();
    if args.version {
        println!("dockering {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.demo && !cfg!(feature = "demo") {
        eprintln!("dockering: this build has no demo support (feature `demo` disabled)");
        return ExitCode::FAILURE;
    }

    let paths = if args.demo {
        // nfr-001-allow: startup before first window
        let root = std::env::temp_dir().join(format!("dockering-demo-{}", std::process::id()));
        Paths::in_dir(root)
    } else {
        match Paths::for_user() {
            Some(paths) => paths,
            None => {
                eprintln!("dockering: cannot determine the user's home directory");
                return ExitCode::FAILURE;
            }
        }
    };

    // Panic hook first, so a crash anywhere below still leaves a crash file.
    dk_hub::logging::install_panic_hook(&paths);
    // nfr-001-allow: startup before first window (sync config load, spec 10 §6)
    let (config, ui_state) = dk_hub::load_config(&paths);
    let _logging = dk_hub::logging::init(&paths, config.diagnostics.log_level);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), demo = args.demo, "starting");

    // SHL-022: a second launch focuses the running window and exits. Demo runs are isolated.
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

    let factories = if args.demo {
        demo_factories()
    } else {
        None
    };
    let hub = match EngineHub::start(HubOptions {
        paths,
        config: config.clone(),
        ui_state: ui_state.clone(),
        factories,
        discover_on_start: true,
        worker_threads: 3,
    }) {
        Ok(hub) => hub,
        Err(err) => {
            tracing::error!(%err, "failed to start the engine hub");
            eprintln!("dockering: failed to start the engine hub: {err}");
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
    ExitCode::SUCCESS
}

#[cfg(feature = "demo")]
fn demo_factories() -> Option<Vec<Arc<dyn dk_core::EngineFactory>>> {
    Some(dockering::demo::factories())
}

#[cfg(not(feature = "demo"))]
fn demo_factories() -> Option<Vec<Arc<dyn dk_core::EngineFactory>>> {
    None
}
