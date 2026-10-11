//! ENG-138 opt-in live check: terminates the caller's default WSLC session, then starts it
//! through the factory. Run:
//! `pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test start_live -- --ignored --nocapture`
#![cfg(windows)]

use std::time::Duration;

use dk_core::{
    EngineConfig, EngineEndpoint, EngineError, EngineFactory, EngineId, EngineOrigin, ProbeResult,
    ResourceKind, WslcTransportPref,
};
use dk_engine_wslc::WslcFactory;

fn config(transport: WslcTransportPref) -> EngineConfig {
    EngineConfig {
        id: EngineId::new("start-live"),
        name: "WSLC start".into(),
        endpoint: EngineEndpoint::Wslc {
            session: None,
            transport,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    }
}

fn session_missing(r: &Result<impl Sized, EngineError>) -> bool {
    matches!(
        r,
        Err(EngineError::NotFound {
            kind: ResourceKind::Session,
            ..
        })
    )
}

#[tokio::test]
#[ignore = "terminates the caller's default WSLC session"]
async fn eng_138_live_terminate_then_start_session() {
    assert!(dk_engine_wslc::init_process_com_security());
    let exe = dk_engine_wslc::cli::wslc_exe().expect("installed wslc.exe");
    let f = WslcFactory::new();
    let auto = config(WslcTransportPref::Auto);

    let out = tokio::process::Command::new(&exe)
        .args(["system", "session", "terminate"])
        .kill_on_drop(true)
        .output()
        .await
        .expect("terminate spawn");
    eprintln!("terminate exit={:?}", out.status.code());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = f.connect(&auto).await;
        if session_missing(&r) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "session did not disappear: {:?}",
            r.err()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("connect (Auto) -> NotFound{{Session}}");
    let cli = config(WslcTransportPref::Cli);
    let r = f.connect(&cli).await;
    assert!(session_missing(&r), "{:?}", r.err());
    let listed = tokio::process::Command::new(&exe)
        .args(["system", "session", "list"])
        .kill_on_drop(true)
        .output()
        .await
        .expect("list spawn");
    let table = String::from_utf8_lossy(&listed.stdout);
    eprintln!(
        "session list after CLI connect:
{table}"
    );
    assert!(
        table.lines().all(|l| l.trim().is_empty()
            || l.contains("Display Name")
            || l.contains("No sessions")),
        "CLI connect created a session: {table}"
    );
    let probe = f.probe(&auto).await;
    eprintln!("probe while absent: {probe:?}");
    assert!(matches!(probe, ProbeResult::Stopped), "{probe:?}");

    let t = std::time::Instant::now();
    f.start(&auto).await.expect("start");
    eprintln!("start ok in {:?}", t.elapsed());

    let engine = f.connect(&auto).await.expect("connect after start");
    engine.ping().await.expect("ping");
    let probe = f.probe(&auto).await;
    eprintln!("probe after start: {probe:?}");
    assert!(matches!(probe, ProbeResult::Reachable), "{probe:?}");
}
