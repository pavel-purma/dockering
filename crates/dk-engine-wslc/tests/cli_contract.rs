//! CLI transport against the fake `wslc.exe` (spec 21 §7): read-only ops and error mapping
//! on recorded wslc 3.0.1 fixtures. Ops that go through `dk_core::docker_json` or
//! Read-only ops against the fake `wslc.exe` replaying recorded fixtures.

use std::sync::Once;
use std::time::Duration;

use dk_core::{
    Capabilities, ContainerAction, ContainerQuery, ContainerState, Engine, EngineError, EngineId,
    EngineKind, EventFilter, LogOpts, LogStream, PullProgress, RemoveContainerOpts, ResourceKind,
};
use dk_engine_wslc::cli::{WslcCliEngine, list_sessions, wslc_exe};
use futures::StreamExt;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-wslc");

/// Point the transport at the fake. Every test calls this first; `Once` blocks concurrent
/// callers until the vars are set, and they never change afterwards, so no test reads the
/// environment while it's being written.
fn env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(set_env);
}

#[allow(unsafe_code)]
fn set_env() {
    // SAFETY: runs exactly once, before any test in this binary reads the environment
    // (see `env`).
    unsafe {
        std::env::set_var("DOCKERING_WSLC_EXE", FAKE);
        std::env::set_var(
            "FAKE_WSLC_FIXTURES",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cli"),
        );
    }
}

async fn engine(session: Option<&str>) -> WslcCliEngine {
    WslcCliEngine::connect(
        EngineId::new("wslc-test"),
        session.map(str::to_owned),
        Some("3.0.1".into()),
        Some("test: CLI forced".into()),
    )
    .await
    .expect("connect to fake wslc")
}

#[tokio::test]
async fn wslc_exe_honours_env_override() {
    env();
    assert_eq!(wslc_exe().as_deref(), Some(std::path::Path::new(FAKE)));
}

#[tokio::test]
async fn eng_008_list_sessions_parses_table() {
    env();
    let s = list_sessions().await.expect("sessions");
    assert_eq!(s, ["wslc-cli-admin-user", "wslc-cli-user"]);
}

#[tokio::test]
async fn eng_013_connect_default_and_named_session() {
    env();
    let e = engine(None).await;
    assert_eq!(e.kind(), EngineKind::Wslc);
    assert_eq!(e.id().as_str(), "wslc-test");
    let caps = e.capabilities();
    assert!(
        caps.contains(Capabilities::EVENTS | Capabilities::EXEC_TTY | Capabilities::LOGS_FOLLOW)
    );
    assert!(
        !caps.intersects(
            Capabilities::PAUSE | Capabilities::PULL_PROGRESS | Capabilities::DISK_USAGE
        )
    );
    e.ping().await.expect("ping");
    let _named = engine(Some("wslc-cli-user")).await;
}

#[tokio::test]
async fn eng_013_connect_unknown_session_is_unreachable() {
    env();
    let err = WslcCliEngine::connect(EngineId::new("x"), Some("nosuch".into()), None, None)
        .await
        .expect_err("must fail");
    assert!(err.is_unreachable(), "{err:?}");
    assert!(err.hint().is_some());
    let err = WslcCliEngine::connect(EngineId::new("x"), Some("--format".into()), None, None)
        .await
        .expect_err("must fail");
    assert!(
        matches!(err, EngineError::Api { status: 400, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn eng_024_info() {
    env();
    let i = engine(None).await.info().await.expect("info");
    assert_eq!(i.kind, EngineKind::Wslc);
    assert_eq!(i.transport.as_deref(), Some("cli"));
    assert_eq!(i.transport_note.as_deref(), Some("test: CLI forced"));
    assert_eq!(i.server_version, "3.0.1");
    assert_eq!(i.os, "linux");
    assert_eq!(i.kernel.as_deref(), Some("6.18.40.1-1"));
    assert_eq!(i.daemon_id, None);
    assert_eq!(i.list_stats_limit, 0);
    assert_eq!((i.containers.running, i.containers.stopped), (1, 1));
    assert_eq!(i.images, 2);
    assert_eq!(i.capabilities, engine(None).await.capabilities());
}

#[tokio::test]
async fn con_010_list_containers() {
    env();
    let e = engine(None).await;
    let all = e
        .list_containers(ContainerQuery::default())
        .await
        .expect("list");
    assert_eq!(all.len(), 2);
    let web = all
        .iter()
        .find(|c| c.name == "dk-cli-fixture")
        .expect("web");
    assert_eq!(web.state, ContainerState::Running);
    assert_eq!(
        web.compose.as_ref().map(|c| c.project.as_str()),
        Some("dkcli")
    );
    assert_eq!(web.size_rw, None, "size only when q.size");
    let running = e
        .list_containers(ContainerQuery {
            all: false,
            ..ContainerQuery::default()
        })
        .await
        .expect("running");
    assert_eq!(running.len(), 1);
}

#[tokio::test]
async fn cdt_inspect_container() {
    env();
    let d = engine(None)
        .await
        .inspect_container("dk-cli-fixture")
        .await
        .expect("inspect");
    assert_eq!(d.summary.name, "dk-cli-fixture");
    assert_eq!(d.port_bindings.len(), 1);
}

#[tokio::test]
async fn errors_map_to_engine_errors() {
    env();
    let e = engine(None).await;
    let err = e
        .inspect_container("deadbeefdeadbeef")
        .await
        .expect_err("err");
    assert_eq!(
        err,
        EngineError::not_found(ResourceKind::Container, "deadbeefdeadbeef")
    );
    let err = e
        .container_action("dk-cli-nosuch", ContainerAction::Stop { timeout_s: None })
        .await
        .expect_err("err");
    assert_eq!(
        err,
        EngineError::not_found(ResourceKind::Container, "dk-cli-nosuch")
    );
    let err = e
        .remove_container("dk-cli-fixture", RemoveContainerOpts::default())
        .await
        .expect_err("err");
    assert!(matches!(err, EngineError::Conflict(_)), "{err:?}");
    let err = e
        .container_action("dk-cli-exited", ContainerAction::Kill { signal: None })
        .await
        .expect_err("err");
    assert!(matches!(err, EngineError::Conflict(_)), "{err:?}");
    let err = e.inspect_volume("nosuchvol").await.expect_err("err");
    assert_eq!(
        err,
        EngineError::not_found(ResourceKind::Volume, "nosuchvol")
    );
    let err = e.inspect_image("nosuch:img").await.expect_err("err");
    assert_eq!(
        err,
        EngineError::not_found(ResourceKind::Image, "nosuch:img")
    );
    let err = e.remove_volume("dk-cli-vol", false).await.expect_err("err");
    assert!(matches!(err, EngineError::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn actions_and_unsupported() {
    env();
    let e = engine(None).await;
    e.container_action(
        "dk-cli-fixture",
        ContainerAction::Stop { timeout_s: Some(1) },
    )
    .await
    .expect("stop");
    e.container_action("dk-cli-fixture", ContainerAction::Start)
        .await
        .expect("start");
    assert_eq!(
        e.container_action("dk-cli-fixture", ContainerAction::Pause)
            .await,
        Err(EngineError::Unsupported(Capabilities::PAUSE))
    );
    assert_eq!(
        e.top("x").await,
        Err(EngineError::Unsupported(Capabilities::TOP))
    );
    assert_eq!(
        e.image_history("nginx").await,
        Err(EngineError::Unsupported(Capabilities::IMAGE_HISTORY))
    );
    assert_eq!(
        e.disk_usage().await,
        Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
    );
    let r = e.prune_containers().await.expect("prune");
    assert_eq!(r.deleted.len(), 1);
    assert_eq!(
        e.prune_networks().await.expect("prune").deleted,
        ["dk-cli-net"]
    );
    let id = e
        .run_image(dk_core::RunSpec {
            image: "nginx:alpine".into(),
            name: Some("dk-cli-run".into()),
            ..dk_core::RunSpec::default()
        })
        .await
        .expect("run");
    assert_eq!(id.len(), 64);
    let items = e.remove_image("busybox:1.37", false).await.expect("rmi");
    assert_eq!(items.len(), 4);
    e.tag_image("busybox:1.37", "dk-cli-test/busybox", "t3")
        .await
        .expect("tag");
}

#[tokio::test]
async fn nfr_022_rejects_option_like_arguments_without_spawning() {
    env();
    let e = engine(None).await;
    for bad in ["--all", "-f", "a b", ""] {
        let err = e.inspect_container(bad).await.expect_err("err");
        assert!(
            matches!(err, EngineError::Api { status: 400, .. }),
            "{bad}: {err:?}"
        );
    }
    let err = e.remove_image("--force", false).await.expect_err("err");
    assert!(matches!(err, EngineError::Api { status: 400, .. }));
    let err = e.remove_network("-x").await.expect_err("err");
    assert!(matches!(err, EngineError::Api { status: 400, .. }));
    let mut s = e.logs("--follow", LogOpts::default());
    assert!(matches!(
        s.next().await,
        Some(Err(EngineError::Api { status: 400, .. }))
    ));
}

#[tokio::test]
async fn img_list_images() {
    env();
    let imgs = engine(None).await.list_images().await.expect("images");
    assert_eq!(imgs.len(), 2);
    let nginx = imgs
        .iter()
        .find(|i| i.repo_tags.iter().any(|t| t == "nginx:alpine"))
        .expect("nginx");
    assert_eq!(nginx.repo_digests.len(), 1);
    assert!(nginx.repo_digests[0].starts_with("nginx@sha256:"));
    assert_eq!(nginx.size, 62_900_000);
    assert!(imgs.iter().all(|i| i.id.starts_with("sha256:")));
}

#[tokio::test]
async fn vol_list_volumes_enriched_by_inspect() {
    env();
    let vols = engine(None).await.list_volumes().await.expect("volumes");
    assert_eq!(vols.len(), 1);
    assert_eq!(vols[0].name, "dk-cli-vol");
    assert_eq!(vols[0].driver, "guest");
    assert!(vols[0].created.is_some(), "created from inspect");
    assert_eq!(vols[0].size, None, "WSLC reports N/A (VOL-002)");
    assert_eq!(vols[0].compose, None);
}

#[tokio::test]
async fn net_list_networks_enriched_by_inspect() {
    env();
    let nets = engine(None).await.list_networks().await.expect("networks");
    assert_eq!(nets.len(), 4);
    let n = nets.iter().find(|n| n.name == "dk-cli-net").expect("net");
    assert_eq!(n.subnets.len(), 1);
    assert_eq!(n.containers, Some(0));
    assert_eq!(
        n.compose.as_ref().map(|c| c.project.as_str()),
        Some("dkcli")
    );
    let bridge = nets.iter().find(|n| n.name == "bridge").expect("bridge");
    assert!(bridge.is_builtin());
    assert!(bridge.containers.unwrap_or(0) >= 1);
}

#[tokio::test]
async fn log_001_logs_non_follow_split_streams() {
    env();
    let e = engine(None).await;
    let opts = LogOpts {
        follow: false,
        tail: None,
        since: None,
        timestamps: false,
    };
    let chunks: Vec<_> = e
        .logs("dk-cli-exited", opts)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("logs");
    assert_eq!(chunks.len(), 2);
    let out = chunks
        .iter()
        .find(|c| c.stream == LogStream::Stdout)
        .expect("stdout");
    assert_eq!(&out.bytes[..], b"out-line\n");
    let err = chunks
        .iter()
        .find(|c| c.stream == LogStream::Stderr)
        .expect("stderr");
    assert_eq!(&err.bytes[..], b"err-line\n");
    assert!(chunks.iter().all(|c| c.ts.is_none()));

    let opts = LogOpts {
        follow: false,
        ..LogOpts::default()
    };
    let chunks: Vec<_> = e
        .logs("dk-cli-exited", opts)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("logs");
    assert_eq!(chunks.len(), 2);
    assert!(chunks.iter().all(|c| c.ts.is_some()));
    assert!(chunks.iter().any(|c| &c.bytes[..] == b"out-line\n"));
}

#[tokio::test]
async fn log_follow_stream_kills_child_on_drop() {
    env();
    let e = engine(None).await;
    let mut s = e.logs("dk-cli-exited", LogOpts::default());
    let first = tokio::time::timeout(Duration::from_secs(10), s.next())
        .await
        .expect("chunk in time")
        .expect("item")
        .expect("ok");
    assert!(first.ts.is_some());
    // The fake hangs for 30 s after printing; dropping must not wait for it.
    let t = std::time::Instant::now();
    drop(s);
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn sta_stats_from_cli_strings() {
    env();
    let e = engine(None).await;
    let mut s = e.stats("dk-cli-fixture");
    let a = s.next().await.expect("item").expect("sample");
    assert_eq!(a.pids, Some(21));
    assert!(a.mem_limit > 16_000_000_000);
    assert_eq!((a.net_rx_bps, a.blk_write_total), (0.0, 4100));
    let b = s.next().await.expect("item").expect("sample");
    assert!(b.at >= a.at, "monotonic timestamps");
    assert_eq!(b.net_rx_bps, 0.0, "fixture totals don't change");
    let mut s = e.stats("nosuch");
    assert_eq!(
        s.next().await.expect("item"),
        Err(EngineError::not_found(ResourceKind::Container, "nosuch"))
    );
    assert!(s.next().await.is_none());
}

#[tokio::test]
async fn eng_events_parse_and_filter() {
    env();
    let e = engine(None).await;
    let evs: Vec<_> = e
        .events(EventFilter::default())
        .take(5)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("events");
    assert_eq!(evs.len(), 5);
    assert_eq!(evs[0].kind, ResourceKind::Container);
    assert_eq!(evs[0].action, "create");
    assert_eq!(evs[0].attributes["name"], "dk-cli-fixture");

    let nets: Vec<_> = e
        .events(EventFilter {
            kinds: vec![ResourceKind::Network],
            since: None,
        })
        .take(3)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("events");
    assert!(nets.iter().all(|e| e.kind == ResourceKind::Network));
    assert_eq!(nets[0].action, "create");
}

#[tokio::test]
async fn img_004_pull_is_text_then_done() {
    env();
    let e = engine(None).await;
    let items: Vec<_> = e
        .pull_image("busybox:1.37", None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("pull");
    assert_eq!(items.len(), 11);
    assert!(
        items[..10]
            .iter()
            .all(|p| matches!(p, PullProgress::Status(_)))
    );
    assert_eq!(
        items[10],
        PullProgress::Done {
            digest: Some(
                "sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e".into()
            )
        }
    );
    let mut bad = e.pull_image("-q", None);
    assert!(matches!(
        bad.next().await,
        Some(Err(EngineError::Api { status: 400, .. }))
    ));
}

#[cfg(not(windows))]
#[tokio::test]
async fn exec_unsupported_off_windows() {
    env();
    let e = engine(None).await;
    let r = e
        .exec("dk-cli-fixture", dk_core::ExecRequest::default())
        .await;
    assert!(matches!(r, Err(EngineError::Unsupported(c)) if c == Capabilities::EXEC_TTY));
}
