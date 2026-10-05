//! T15 cross-adapter inspect and missing payload/fault cases, without installed WSL.
use dk_core::{ContainerState, Engine, EngineError, EngineId, Proto, RunSpec};
use dk_engine_wslc::cli::WslcCliEngine;

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: this binary's tests all call init before accessing the environment;
        // values are never modified after Once completes. No real engine is used here.
        unsafe {
            std::env::set_var("DOCKERING_WSLC_EXE", env!("CARGO_BIN_EXE_fake-wslc"));
            std::env::set_var(
                "FAKE_WSLC_FIXTURES",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/repair"),
            );
        }
    });
}

async fn cli() -> WslcCliEngine {
    init();
    WslcCliEngine::connect(
        EngineId::new("repair-fixture"),
        Some("repair-session".into()),
        None,
        None,
    )
    .await
    .expect("fake CLI")
}

#[tokio::test]
async fn eng_133_cdt_030_040_published_ports_both_delegate_paths() {
    let cli = cli().await;
    let details = cli
        .inspect_container("repair-ports")
        .await
        .expect("CLI inspect");
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/repair/inspect_ports.json")).expect("fixture");
    assert_eq!(details.raw, raw, "original JSON, including unknown fields");
    assert!(
        !details
            .raw
            .as_object()
            .expect("object")
            .contains_key("NetworkSettings")
    );
    assert_eq!(details.cmd, ["/web"]);
    assert_eq!(details.summary.exit_code, Some(0));
    assert_eq!(details.summary.labels["fixture"], "sanitised");
    assert_eq!(details.summary.ports, details.port_bindings);
    assert_eq!(details.port_bindings.len(), 4);
    for (private, public, ip, proto) in [
        (80, Some(18080), Some("127.0.0.1"), Proto::Tcp),
        (80, Some(18081), Some("::1"), Proto::Tcp),
        (53, Some(15353), Some("0.0.0.0"), Proto::Udp),
        (9000, None, None, Proto::Tcp),
    ] {
        assert!(details.port_bindings.iter().any(|p| p.private == private
            && p.public == public
            && p.proto == proto
            && p.ip.map(|ip| ip.to_string()).as_deref() == ip));
    }
    #[cfg(windows)]
    {
        use dk_engine_wslc::com::{
            WslcComEngine,
            fake::{self, FakeContainerData, FakeManager},
        };
        let state = fake::sample_state();
        state.lock().expect("state").containers = vec![FakeContainerData {
            id: "a".repeat(64),
            name: "repair-ports".into(),
            inspect_json: raw.to_string(),
            ..Default::default()
        }];
        let native = WslcComEngine::from_manager_for_tests(
            EngineId::new("repair-fixture"),
            FakeManager::new_interface(state),
            None,
        );
        let com = native
            .inspect_container(&"a".repeat(64))
            .await
            .expect("COM inspect");
        assert_eq!(com.raw, raw);
        assert_eq!(com.summary.ports, details.summary.ports);
        assert_eq!(com.port_bindings, details.port_bindings);
        assert_eq!(com.cmd, details.cmd);
        assert_eq!(com.summary.state, details.summary.state);
        assert_eq!(com.summary.labels, details.summary.labels);
        assert_eq!(com.mounts, details.mounts);
    }
}

#[tokio::test]
async fn eng_129_cli_malformed_inspect_must_remain_protocol_error() {
    let e = cli().await;
    let malformed = e.inspect_container("repair-malformed").await;
    assert!(
        matches!(malformed, Err(EngineError::Protocol(_))),
        "malformed: {malformed:?}"
    );
    for id in [
        "repair-blank",
        "repair-null",
        "repair-scalar",
        "repair-nonobject-array",
        "repair-mixed-array",
    ] {
        assert!(
            matches!(e.inspect_container(id).await, Err(EngineError::Protocol(_))),
            "{id}"
        );
    }
    assert!(matches!(
        e.inspect_container("repair-empty").await,
        Err(EngineError::NotFound { .. })
    ));
    for result in [
        e.inspect_image("repair-malformed").await.map(|_| ()),
        e.inspect_volume("repair-malformed").await.map(|_| ()),
        e.inspect_network("repair-malformed").await.map(|_| ()),
    ] {
        assert!(matches!(result, Err(EngineError::Protocol(_))));
    }
}

#[tokio::test]
async fn nfr_031_cli_unknown_fields_and_state_safe() {
    let e = cli().await;
    let d = e
        .inspect_container("repair-unknown")
        .await
        .expect("future fields safe");
    assert_eq!(d.summary.state, ContainerState::Unknown);
    assert_eq!(d.raw["FutureField"], true);
}

#[tokio::test]
async fn eng_130_img_005_cli_lost_run_id_not_fabricated() {
    let e = cli().await;
    let result = e
        .run_image(RunSpec {
            image: "hello-world:latest".into(),
            name: Some("repair-lost-id".into()),
            ..Default::default()
        })
        .await;
    assert!(
        matches!(result, Err(EngineError::Unreachable { .. })),
        "lost ID must be unknown, not guessed: {result:?}"
    );
    let err = result.expect_err("lost ID");
    assert!(
        err.hint()
            .expect("unknown outcome guidance")
            .contains("Refresh")
    );
}

#[tokio::test]
async fn eng_129_cli_elevation_run_rejected_without_success_id() {
    let e = cli().await;
    let result = e
        .run_image(RunSpec {
            image: "hello-world:latest".into(),
            name: Some("repair-elevation".into()),
            ..Default::default()
        })
        .await;
    let err = result.expect_err("elevation must not turn into success");
    assert!(
        err.hint()
            .is_some_and(|h| h.to_lowercase().contains("elevat")),
        "{err:?}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn eng_134_token_failure_blocks_remove_logs_and_exec() {
    use dk_core::{ExecRequest, LogOpts, RemoveContainerOpts};
    use dk_engine_wslc::com::{
        WslcComEngine,
        fake::{self, FakeManager},
        ffi::hr,
    };
    use futures::StreamExt;
    for protected in ["Delete", "Logs", "Exec"] {
        let state = fake::sample_state();
        let id = state.lock().expect("state").containers[0].id.clone();
        state
            .lock()
            .expect("state")
            .fail_next
            .insert("BeginContainerOperation".into(), hr::RPC_E_DISCONNECTED);
        let e = WslcComEngine::from_manager_for_tests(
            EngineId::new("repair-fixture"),
            FakeManager::new_interface(state.clone()),
            None,
        );
        match protected {
            "Delete" => assert!(
                e.remove_container(
                    &id,
                    RemoveContainerOpts {
                        force: true,
                        ..Default::default()
                    }
                )
                .await
                .is_err()
            ),
            "Logs" => assert!(
                e.logs(&id, LogOpts::default())
                    .next()
                    .await
                    .expect("token error")
                    .is_err()
            ),
            "Exec" => assert!(
                e.exec(
                    &id,
                    ExecRequest {
                        cmd: vec!["/bin/true".into()],
                        tty: true,
                        env: vec![],
                        user: None,
                        working_dir: None,
                        cols: 80,
                        rows: 24
                    }
                )
                .await
                .is_err()
            ),
            _ => unreachable!(),
        }
        let s = state.lock().expect("state");
        assert!(
            !s.calls.iter().any(|c| c == protected),
            "protected call {protected} dispatched: {:?}",
            s.calls
        );
        assert_eq!(
            s.calls
                .iter()
                .filter(|c| *c == "BeginContainerOperation")
                .count(),
            1
        );
        assert!(s.deleted.is_empty());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn eng_129_com_malformed_payload_no_implicit_retry() {
    use dk_engine_wslc::com::{
        WslcComEngine,
        fake::{self, FakeManager},
    };
    let state = fake::sample_state();
    let id = state.lock().expect("state").containers[0].id.clone();
    state.lock().expect("state").containers[0].inspect_json = "{broken-json".into();
    let e = WslcComEngine::from_manager_for_tests(
        EngineId::new("repair-fixture"),
        FakeManager::new_interface(state.clone()),
        None,
    );
    assert!(matches!(
        e.inspect_container(&id).await,
        Err(EngineError::Protocol(_))
    ));
    // The fake's Inspect slot does not record calls; OpenContainer does and proves a
    // single protected inspect attempt (no delegate reopen/retry after payload error).
    assert_eq!(
        state
            .lock()
            .expect("state")
            .calls
            .iter()
            .filter(|c| *c == "OpenContainer")
            .count(),
        1
    );
}
