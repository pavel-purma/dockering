//! ENG-137/138: missing default session on the CLI path and the fixed start argv, against the
//! fake `wslc.exe` with `FAKE_WSLC_SESSION_FILE` simulating "no session until started".
#![cfg(windows)]
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, Once};

use dk_core::{
    EngineConfig, EngineEndpoint, EngineError, EngineFactory, EngineId, EngineOrigin, ResourceKind,
    WslcTransportPref,
};
use dk_engine_wslc::WslcFactory;
use dk_engine_wslc::cli::WslcCliEngine;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-wslc");

static LOCK: Mutex<()> = Mutex::new(());

fn state_file() -> PathBuf {
    std::env::temp_dir().join(format!("dk-fake-wslc-{}.state", std::process::id()))
}

#[allow(unsafe_code)]
fn setup() -> MutexGuard<'static, ()> {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: runs once before any test in this binary reads the environment.
        unsafe {
            std::env::set_var("DOCKERING_WSLC_EXE", FAKE);
            std::env::set_var(
                "FAKE_WSLC_FIXTURES",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cli"),
            );
            std::env::set_var("FAKE_WSLC_SESSION_FILE", state_file());
        }
    });
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let f = state_file();
    for p in [
        f.clone(),
        f.with_extension("argv"),
        f.with_extension("fail"),
    ] {
        let _ = std::fs::remove_file(p);
    }
    guard
}

fn argv_log() -> Vec<String> {
    std::fs::read_to_string(state_file().with_extension("argv"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn cfg(session: Option<&str>) -> EngineConfig {
    EngineConfig {
        id: EngineId::new("wslc-test"),
        name: "WSL containers".into(),
        endpoint: EngineEndpoint::Wslc {
            session: session.map(str::to_owned),
            transport: WslcTransportPref::Cli,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    }
}

#[tokio::test]
async fn eng_137_cli_missing_default_session_is_not_found() {
    let _g = setup();
    let err = WslcCliEngine::connect(EngineId::new("x"), None, None, None)
        .await
        .expect_err("no session");
    assert_eq!(
        err,
        EngineError::not_found(ResourceKind::Session, "default")
    );
    let err = WslcFactory::new()
        .connect(&cfg(None))
        .await
        .map(|_| ())
        .expect_err("no session");
    assert!(
        matches!(
            err,
            EngineError::NotFound {
                kind: ResourceKind::Session,
                ..
            }
        ),
        "{err:?}"
    );
    let log = argv_log();
    assert!(!log.is_empty());
    assert!(
        log.iter().all(|l| !l.starts_with("container ")),
        "no session-bound argv may run: {log:?}"
    );
    assert!(!state_file().exists(), "connect must not create a session");
}

#[tokio::test]
async fn eng_141_ping_never_creates_session() {
    let _g = setup();
    WslcFactory::new().start(&cfg(None)).await.expect("start");
    let engine = WslcFactory::new()
        .connect(&cfg(None))
        .await
        .expect("connects");
    engine.ping().await.expect("ping with session");
    std::fs::remove_file(state_file()).expect("session lost");
    std::fs::remove_file(state_file().with_extension("argv")).expect("clear log");
    let err = engine.ping().await.expect_err("session lost");
    assert!(
        matches!(
            err,
            EngineError::NotFound {
                kind: ResourceKind::Session,
                ..
            }
        ),
        "{err:?}"
    );
    let log = argv_log();
    assert!(
        log.iter().all(|l| !l.starts_with("container ")),
        "ping must not run a session-bound argv: {log:?}"
    );
    assert!(!state_file().exists(), "ping must not create a session");
}

#[tokio::test]
async fn eng_140_probe_never_spawns_cli() {
    let _g = setup();
    let _ = WslcFactory::new().probe(&cfg(None)).await;
    assert!(argv_log().is_empty(), "{:?}", argv_log());
    assert!(!state_file().exists());
}

#[tokio::test]
async fn eng_137_named_missing_session_has_no_start_prompt() {
    let _g = setup();
    let f = WslcFactory::new();
    assert!(f.start_prompt(&cfg(None)).is_some());
    assert!(f.start_prompt(&cfg(Some("custom"))).is_none());
    let err = WslcCliEngine::connect(EngineId::new("x"), Some("nosuch".into()), None, None)
        .await
        .expect_err("missing named session");
    assert_eq!(err, EngineError::not_found(ResourceKind::Session, "nosuch"));
}

#[tokio::test]
async fn eng_138_start_runs_one_fixed_argv() {
    let _g = setup();
    let f = WslcFactory::new();
    f.start(&cfg(None)).await.expect("start");
    assert_eq!(argv_log(), ["container list --all --format json"]);
    WslcCliEngine::connect(EngineId::new("x"), None, None, None)
        .await
        .expect("connects once the session exists");
}

#[tokio::test]
async fn eng_138_start_refused_for_named_session() {
    let _g = setup();
    let err = WslcFactory::new()
        .start(&cfg(Some("custom")))
        .await
        .expect_err("named session is not startable");
    assert!(
        matches!(err, EngineError::Api { status: 400, .. }),
        "{err:?}"
    );
    assert!(argv_log().is_empty(), "no child may run");
}

#[tokio::test]
async fn eng_138_start_failure_maps_with_hint() {
    let _g = setup();
    std::fs::write(state_file().with_extension("fail"), "").expect("marker");
    let err = WslcFactory::new()
        .start(&cfg(None))
        .await
        .expect_err("start fails");
    assert!(err.hint().is_some(), "{err:?}");
    assert_eq!(argv_log(), ["container list --all --format json"]);
}

#[tokio::test]
async fn eng_139_start_prompt_has_no_transport_words() {
    let _g = setup();
    let p = WslcFactory::new().start_prompt(&cfg(None)).expect("prompt");
    assert_eq!(p.action_label, "Start WSL containers");
    let text = format!("{} {} {}", p.title, p.body, p.action_label).to_lowercase();
    for w in ["com", "cli", "process", "wslc"] {
        assert!(
            !text.split(|c: char| !c.is_alphanumeric()).any(|t| t == w),
            "{w}"
        );
    }
}
