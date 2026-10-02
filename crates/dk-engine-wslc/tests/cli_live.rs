//! Live tests against the real `wslc.exe` (spec 21 §7 "real-WSLC run"). Windows only and
//! `#[ignore]`d; run manually:
//!
//! ```sh
//! pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test cli_live -- --ignored --test-threads=1
//! ```
//!
//! Read-only except `exec` (runs `echo hi` in an already running container). Needs the
//! `dk_core::grouping` bodies (compose labels) — merged from `wip/core`.
#![cfg(windows)]

use std::time::Duration;

use bytes::Bytes;
use dk_core::{
    Capabilities, ContainerQuery, ContainerState, ContainerSummary, Engine, EngineId, EngineKind,
    EventFilter, ExecRequest, LogOpts,
};
use dk_engine_wslc::cli::{WslcCliEngine, list_sessions, wslc_exe};
use futures::StreamExt;

async fn engine() -> WslcCliEngine {
    assert!(wslc_exe().is_some(), "wslc.exe not installed");
    WslcCliEngine::connect(
        EngineId::new("wslc-live"),
        None,
        None,
        Some("live test".into()),
    )
    .await
    .expect("connect to the default WSLC session")
}

/// Running containers.
async fn running(e: &WslcCliEngine) -> Vec<ContainerSummary> {
    e.list_containers(ContainerQuery {
        all: false,
        ..ContainerQuery::default()
    })
    .await
    .expect("list running")
}

#[tokio::test]
#[ignore = "live: needs WSLC"]
async fn live_connect_info_and_sessions() {
    let e = engine().await;
    e.ping().await.expect("ping");
    let i = e.info().await.expect("info");
    println!("info: {i:#?}");
    assert_eq!(i.kind, EngineKind::Wslc);
    assert_eq!(i.transport.as_deref(), Some("cli"));
    assert!(!i.server_version.is_empty());
    let s = list_sessions().await.expect("sessions");
    println!("sessions: {s:?}");
    assert!(!s.is_empty());
}

#[tokio::test]
#[ignore = "live: needs WSLC + dk_core::grouping bodies (merge)"]
async fn live_lists() {
    let e = engine().await;
    let c = e
        .list_containers(ContainerQuery::default())
        .await
        .expect("containers");
    println!("{} containers", c.len());
    for x in &c {
        assert!(
            !x.id.is_empty() && x.state != ContainerState::Unknown,
            "{x:?}"
        );
    }
    let imgs = e.list_images().await.expect("images");
    println!("{} images", imgs.len());
    assert!(imgs.iter().all(|i| !i.id.is_empty()));
    let vols = e.list_volumes().await.expect("volumes");
    println!("{} volumes", vols.len());
    let nets = e.list_networks().await.expect("networks");
    println!("{} networks", nets.len());
    assert!(nets.iter().any(|n| n.name == "bridge"));
}

#[tokio::test]
#[ignore = "live: needs WSLC"]
async fn live_events_open() {
    let e = engine().await;
    let mut s = e.events(EventFilter::default());
    // No activity is required: the stream must stay open (no error) for a moment.
    match tokio::time::timeout(Duration::from_secs(2), s.next()).await {
        Err(_) => {}
        Ok(Some(Ok(ev))) => println!("event: {ev:?}"),
        Ok(other) => panic!("events ended early: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "live: needs WSLC, a running container and dk_core::grouping bodies (merge)"]
async fn live_running_container_logs_stats_exec() {
    let e = engine().await;
    let Some(c) = running(&e).await.into_iter().next() else {
        println!("no running container — skipped");
        return;
    };
    println!("using {} ({})", c.name, &c.id[..12]);

    let logs: Vec<_> = e
        .logs(
            &c.id,
            LogOpts {
                follow: false,
                tail: Some(5),
                since: None,
                timestamps: true,
            },
        )
        .collect()
        .await;
    println!("{} log chunks", logs.len());
    assert!(logs.iter().all(Result::is_ok), "{logs:?}");
    assert!(logs.iter().flatten().all(|l| l.ts.is_some()));

    let mut stats = e.stats(&c.id);
    let s = tokio::time::timeout(Duration::from_secs(10), stats.next())
        .await
        .expect("stats in time")
        .expect("item")
        .expect("sample");
    println!("stats: {s:?}");
    assert!(s.mem_limit > 0);
    drop(stats);

    assert!(e.capabilities().contains(Capabilities::EXEC_TTY));
    let mut term = e
        .exec(
            &c.id,
            ExecRequest {
                cmd: vec!["echo".into(), "hi".into()],
                cols: 80,
                rows: 24,
                ..ExecRequest::default()
            },
        )
        .await
        .expect("exec");
    term.resize(100, 30).await.expect("resize");
    let out = term.output();
    let collected: Vec<Bytes> =
        tokio::time::timeout(Duration::from_secs(15), out.collect::<Vec<_>>())
            .await
            .expect("exec output ends")
            .into_iter()
            .collect::<Result<_, _>>()
            .expect("output");
    let text = String::from_utf8_lossy(&collected.concat()).into_owned();
    println!("exec output: {text:?}");
    assert!(text.contains("hi"), "{text:?}");
    let code = tokio::time::timeout(Duration::from_secs(10), term.wait())
        .await
        .expect("exit in time")
        .expect("wait");
    assert_eq!(code, Some(0));
    term.close().await.expect("close");
}

#[tokio::test]
#[ignore = "live: needs WSLC, a running container and dk_core::grouping bodies (merge)"]
async fn live_exec_interactive_shell_resize_and_exit_code() {
    let e = engine().await;
    let Some(c) = running(&e).await.into_iter().next() else {
        println!("no running container — skipped");
        return;
    };
    let mut term = e
        .exec(
            &c.id,
            ExecRequest {
                cmd: vec!["/bin/sh".into()],
                cols: 80,
                rows: 24,
                ..ExecRequest::default()
            },
        )
        .await
        .expect("exec");
    let mut out = term.output();
    term.resize(120, 40).await.expect("resize");
    term.write(Bytes::from_static(b"stty size; exit 7\r"))
        .await
        .expect("write");
    let mut text = String::new();
    let read = async {
        while let Some(chunk) = out.next().await {
            text.push_str(&String::from_utf8_lossy(&chunk.expect("chunk")));
        }
    };
    tokio::time::timeout(Duration::from_secs(15), read)
        .await
        .expect("output ends");
    println!("shell output: {text:?}");
    assert!(
        text.contains("40 120"),
        "resize visible in the TTY: {text:?}"
    );
    let code = tokio::time::timeout(Duration::from_secs(10), term.wait())
        .await
        .expect("exit in time")
        .expect("wait");
    assert_eq!(code, Some(7));
}

#[tokio::test]
#[ignore = "live: needs WSLC + dk_core::docker_json bodies (merge)"]
async fn live_inspect_first_of_each() {
    let e = engine().await;
    if let Some(c) = e
        .list_containers(ContainerQuery::default())
        .await
        .expect("containers")
        .first()
    {
        let d = e.inspect_container(&c.id).await.expect("inspect container");
        assert_eq!(d.summary.id, c.id);
    }
    if let Some(i) = e.list_images().await.expect("images").first() {
        e.inspect_image(&i.id).await.expect("inspect image");
    }
    if let Some(v) = e.list_volumes().await.expect("volumes").first() {
        e.inspect_volume(&v.name).await.expect("inspect volume");
    }
    let d = e.inspect_network("bridge").await.expect("inspect network");
    assert_eq!(d.summary.name, "bridge");
}
