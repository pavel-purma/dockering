//! `WslcComEngine` against the in-process fake WSLC COM server (spec 21 §7): exercises the v3_0
//! vtable/struct declarations, CoTaskMem freeing, handle reading and HRESULT mapping without
//! WSL. Assertions avoid `dk_core::docker_json` (implemented on another branch); inspect is
//! checked as raw JSON.

#![cfg(windows)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use dk_core::{
    Capabilities, ContainerAction, ContainerQuery, ContainerState, Engine, EngineError, EngineId,
    LogOpts, LogStream, Proto, RemoveContainerOpts, ResourceKind, VolumeSpec,
};
use dk_engine_wslc::com::WslcComEngine;
use dk_engine_wslc::com::fake::{self, FakeManager, OPEN_OPERATIONS, Shared};
use dk_engine_wslc::com::ffi::hr;
use futures::StreamExt;
use futures::executor::block_on;

/// `OPEN_OPERATIONS` is process-global; tests in this file run serially so the count is theirs.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn engine() -> (WslcComEngine, Shared) {
    let state = fake::sample_state();
    let mgr = FakeManager::new_interface(state.clone());
    (
        WslcComEngine::from_manager_for_tests(EngineId::new("wslc-fake"), mgr, None),
        state,
    )
}

fn calls(state: &Shared) -> Vec<String> {
    state.lock().map(|g| g.calls.clone()).unwrap_or_default()
}

fn with_timeout<T: Send + 'static>(
    d: Duration,
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(block_on(fut));
    });
    rx.recv_timeout(d).expect("future timed out")
}

#[test]
fn self_check_through_fake() {
    let _serial = serial();
    let (e, _) = engine();
    let check = block_on(e.self_check(Some(dk_engine_wslc::version::WslVersion::new(3, 0, 1, 0))))
        .expect("self-check");
    assert_eq!(check.com_version, (3, 0, 1));
    assert_eq!(check.sessions, 2);
    assert_eq!(check.default_session.as_deref(), Some("wslc-cli-tester"));
    // Version mismatch fails the self-check (→ factory falls back to CLI).
    let bad = block_on(e.self_check(Some(dk_engine_wslc::version::WslVersion::new(3, 0, 2, 0))));
    assert!(matches!(bad, Err(EngineError::Protocol(_))), "{bad:?}");
    let names = block_on(e.list_session_names()).expect("sessions");
    assert_eq!(names, vec!["wslc-cli-admin-tester", "wslc-cli-tester"]);
}

#[test]
fn list_containers_maps_entries_ports_and_strings() {
    let _serial = serial();
    let (e, state) = engine();
    let cs = block_on(e.list_containers(ContainerQuery::default())).expect("list");
    assert_eq!(cs.len(), 2);
    let web = &cs[0];
    assert!(web.id.starts_with("2e4fac884218"));
    assert_eq!(web.name, "dk-fixture-nginx");
    assert_eq!(web.image, "nginx:alpine");
    assert_eq!(web.state, ContainerState::Running);
    assert_eq!(web.status_text, "Up 2 hours");
    assert_eq!(web.labels["com.docker.compose.project"], "web");
    assert_eq!(web.networks, vec!["bridge"]);
    assert_eq!(web.mounts.len(), 1);
    assert_eq!(web.ports.len(), 1);
    assert_eq!(web.ports[0].public, Some(18081));
    assert_eq!(web.ports[0].private, 80);
    assert_eq!(web.ports[0].proto, Proto::Tcp);
    assert_eq!(web.created.unix_timestamp(), 1_759_358_512);
    let stopped = &cs[1];
    assert_eq!(stopped.state, ContainerState::Exited);
    assert_eq!(stopped.exit_code, Some(137));
    assert!(stopped.mounts.is_empty(), "null Mounts pointer handled");
    assert!(calls(&state).contains(&"ListContainers".to_owned()));
}

#[test]
fn inspect_raw_json_and_not_found() {
    let _serial = serial();
    let (e, _) = engine();
    let v = block_on(e.inspect_container_json("2e4fac884218")).expect("inspect");
    assert_eq!(v["Name"], "/dk-fixture-nginx");
    assert_eq!(v["State"]["Status"], "running");
    let missing = block_on(e.inspect_container_json("ffffffffffff"));
    assert_eq!(
        missing,
        Err(EngineError::not_found(
            ResourceKind::Container,
            "ffffffffffff"
        ))
    );
    // Container ops hold a BeginContainerOperation token only for their duration.
    assert_eq!(OPEN_OPERATIONS.load(Ordering::SeqCst), 0);
}

#[test]
fn stats_json_to_sample() {
    let _serial = serial();
    let (e, _) = engine();
    let v = block_on(e.stats_json("2e4fac884218")).expect("stats");
    let mut norm = dk_core::stats::StatsNormalizer::new();
    let s =
        dk_engine_wslc::com::engine::stats_sample(&mut norm, &v, time::OffsetDateTime::now_utc())
            .expect("sample");
    assert_eq!(s.online_cpus, 20);
    // (200M-100M)/(1e12-0.999e12) * 20 * 100 = 200 %
    assert!((s.cpu_percent - 200.0).abs() < 1e-6, "{}", s.cpu_percent);
    assert_eq!(s.mem_used, 20_000_000 - 1_903_872);
    assert_eq!(s.net_rx_total, 1436);
    assert_eq!(s.blk_read_total, 8192);
    assert_eq!(s.pids, Some(21));

    // Stream: first sample arrives, dropping the stream stops polling.
    let (e2, _) = engine();
    let first = with_timeout(Duration::from_secs(10), async move {
        let mut st = e2.stats("2e4fac884218");
        st.next().await
    });
    assert!(matches!(first, Some(Ok(_))), "{first:?}");
}

#[test]
fn hresults_map_to_engine_errors() {
    let _serial = serial();
    let (e, state) = engine();
    state
        .lock()
        .expect("lock")
        .fail_next
        .insert("ListContainers".into(), hr::WSLC_E_VM_NOT_RUNNING);
    let r = block_on(e.list_containers(ContainerQuery::default()));
    assert!(matches!(r, Err(EngineError::Unreachable { .. })), "{r:?}");

    state
        .lock()
        .expect("lock")
        .fail_next
        .insert("Stats".into(), hr::WSLC_E_CONTAINER_DISABLED);
    let r = block_on(e.stats_json("2e4fac884218"));
    let err = r.expect_err("policy");
    assert_eq!(err.hint(), Some(dk_engine_wslc::com::ffi::POLICY_HINT));

    // Disconnect → reopen session once and retry → succeeds.
    state
        .lock()
        .expect("lock")
        .fail_next
        .insert("ListContainers".into(), hr::RPC_E_DISCONNECTED);
    let before = calls(&state)
        .iter()
        .filter(|c| *c == "OpenSessionByName")
        .count();
    let r = block_on(e.list_containers(ContainerQuery::default()));
    assert!(r.is_ok(), "{r:?}");
    let after = calls(&state)
        .iter()
        .filter(|c| *c == "OpenSessionByName")
        .count();
    assert_eq!(after, before + 1, "session reopened exactly once");

    let r = block_on(e.inspect_volume_json("nope"));
    assert_eq!(r, Err(EngineError::not_found(ResourceKind::Volume, "nope")));
}

#[test]
fn actions_idempotence_and_flags() {
    let _serial = serial();
    let (e, state) = engine();
    let web = "2e4fac884218";
    let stopped = "da29ef747f13";
    // Start on a running container is a no-op (WSLC_E_CONTAINER_IS_RUNNING swallowed).
    block_on(e.container_action(web, ContainerAction::Start)).expect("start running");
    // Stop on a stopped container is a no-op.
    block_on(e.container_action(stopped, ContainerAction::Stop { timeout_s: None }))
        .expect("stop stopped");
    block_on(e.container_action(web, ContainerAction::Stop { timeout_s: Some(5) })).expect("stop");
    block_on(e.container_action(
        web,
        ContainerAction::Kill {
            signal: Some("SIGTERM".into()),
        },
    ))
    .expect("kill");
    block_on(e.container_action(web, ContainerAction::Restart { timeout_s: None }))
        .expect("restart");
    let pause = block_on(e.container_action(web, ContainerAction::Pause));
    assert_eq!(pause, Err(EngineError::Unsupported(Capabilities::PAUSE)));
    let bad_sig = block_on(e.container_action(
        web,
        ContainerAction::Kill {
            signal: Some("kill; rm".into()),
        },
    ));
    assert!(matches!(bad_sig, Err(EngineError::Api { status: 400, .. })));

    let busy = block_on(e.remove_container(web, RemoveContainerOpts::default()));
    assert!(matches!(busy, Err(EngineError::Conflict(_))), "{busy:?}");
    block_on(e.remove_container(
        web,
        RemoveContainerOpts {
            force: true,
            volumes: true,
        },
    ))
    .expect("rm -f");

    let g = state.lock().expect("lock");
    assert!(g.stopped.iter().any(|(_, sig, t)| *sig == 0 && *t == 5));
    assert!(
        g.stopped.iter().any(|(_, _, t)| *t == i32::MIN),
        "default timeout = LONG_MIN"
    );
    assert_eq!(g.killed.last().map(|k| k.1), Some(15));
    assert_eq!(g.deleted.last().map(|d| d.1), Some(3));
    assert!(
        g.calls
            .iter()
            .filter(|c| *c == "BeginContainerOperation")
            .count()
            >= 6
    );
    drop(g);
    assert_eq!(OPEN_OPERATIONS.load(Ordering::SeqCst), 0, "tokens released");
}

#[test]
fn logs_read_from_pipe_handles() {
    let _serial = serial();
    let (e, _) = engine();
    let chunks = with_timeout(Duration::from_secs(10), async move {
        e.logs(
            "2e4fac884218",
            LogOpts {
                follow: false,
                tail: Some(10),
                since: None,
                timestamps: true,
            },
        )
        .collect::<Vec<_>>()
        .await
    });
    let chunks: Vec<_> = chunks.into_iter().map(|c| c.expect("chunk")).collect();
    let out: Vec<_> = chunks
        .iter()
        .filter(|c| c.stream == LogStream::Stdout)
        .collect();
    let err: Vec<_> = chunks
        .iter()
        .filter(|c| c.stream == LogStream::Stderr)
        .collect();
    assert_eq!(out.len(), 2, "{chunks:?}");
    assert_eq!(err.len(), 1, "{chunks:?}");
    assert_eq!(&out[0].bytes[..], b"hello\n");
    assert!(out[0].ts.is_some());
    assert_eq!(&err[0].bytes[..], b"warn\n");
}

#[test]
fn events_stream_and_cancel_on_drop() {
    let _serial = serial();
    let (e, _) = engine();
    let first = with_timeout(Duration::from_secs(5), async move {
        let mut s = e.events_json(0);
        let first = s.next().await;
        // The fake now blocks in GetNext until the cancel event fires; dropping must unblock it.
        drop(s);
        first
    });
    let v = first.expect("one event").expect("ok");
    assert_eq!(v["Action"], "start");
    assert_eq!(v["Actor"]["ID"], "2e4fac884218");
}

/// Spec 20 §5.4 / review finding 3: `WSLC_E_EVENTS_LOST` surfaces as exactly
/// `Protocol("events lost")` (what the hub maps to `Feed::Lagged`) and the stream continues.
#[test]
fn events_lost_reports_gap_and_stream_continues() {
    let _serial = serial();
    let (e, state) = engine();
    {
        let mut g = state.lock().expect("state");
        let first = g.events[0].clone();
        let second = first.replace("\"start\"", "\"die\"");
        g.events = vec![first, fake::EVENTS_LOST_MARKER.into(), second];
    }
    let items = with_timeout(Duration::from_secs(5), async move {
        let s = e.events(dk_core::EventFilter {
            since: Some(time::OffsetDateTime::UNIX_EPOCH),
            ..Default::default()
        });
        s.take(3).collect::<Vec<_>>().await
    });
    assert_eq!(items.len(), 3, "{items:?}");
    assert_eq!(items[0].as_ref().map(|e| e.action.as_str()), Ok("start"));
    assert_eq!(
        items[1],
        Err(EngineError::Protocol("events lost".into())),
        "exact message the hub maps to Feed::Lagged"
    );
    assert!(
        items[1]
            .as_ref()
            .is_err_and(dk_core::EngineError::is_events_lost),
        "{:?}",
        items[1]
    );
    assert_eq!(items[2].as_ref().map(|e| e.action.as_str()), Ok("die"));
}

#[test]
fn images_volumes_networks_prune() {
    let _serial = serial();
    let (e, _) = engine();
    let imgs = block_on(e.list_images()).expect("images");
    assert_eq!(imgs.len(), 2, "rows grouped by image id");
    assert_eq!(imgs[0].repo_tags, vec!["nginx:alpine", "nginx:latest"]);
    assert_eq!(imgs[0].size, 62_941_990);
    let vols = block_on(e.list_volumes_json()).expect("volumes");
    assert_eq!(vols[0]["Name"], "webdata");
    let nets = block_on(e.list_networks_json()).expect("networks");
    assert_eq!(nets[0]["Name"], "bridge");
    let pr = block_on(e.prune_containers()).expect("prune");
    assert_eq!(pr.deleted, vec!["da29ef747f13"]);
    assert_eq!(pr.space_reclaimed, 4096);
    let name = block_on(e.create_volume_raw(VolumeSpec {
        name: Some("dk-test".into()),
        ..Default::default()
    }))
    .expect("create");
    assert_eq!(name, "dk-test");
    let bad = block_on(e.create_volume_raw(VolumeSpec {
        name: Some("bad name".into()),
        ..Default::default()
    }));
    assert!(matches!(bad, Err(EngineError::Api { status: 400, .. })));
}

#[test]
fn unsupported_and_capabilities() {
    let _serial = serial();
    let (e, _) = engine();
    assert!(
        e.capabilities()
            .contains(Capabilities::PULL_PROGRESS | Capabilities::EXEC_RESIZE)
    );
    assert!(!e.capabilities().contains(Capabilities::PAUSE));
    assert_eq!(
        block_on(e.top("x")),
        Err(EngineError::Unsupported(Capabilities::TOP))
    );
    assert_eq!(
        block_on(e.disk_usage()),
        Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
    );
    assert!(matches!(
        block_on(e.run_image(dk_core::RunSpec::default())),
        Err(EngineError::Api { status: 501, .. })
    ));
    block_on(e.ping()).expect("ping");
}

/// Recorded real outputs (WSL 3.0.1, `com_live::live_record_fixtures`) through the pure
/// converters (no docker_json dependency).
#[test]
fn recorded_fixtures_parse() {
    let _serial = serial();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/com-3.0.1");
    let read = |n: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(n)).expect(n)).expect(n)
    };
    let stats = read("stats.json");
    let mut norm = dk_core::stats::StatsNormalizer::new();
    let s = dk_engine_wslc::com::engine::stats_sample(
        &mut norm,
        &stats,
        time::OffsetDateTime::now_utc(),
    )
    .expect("real Stats() JSON parses");
    assert_eq!(s.online_cpus, 20);
    assert!(s.mem_limit > 0);
    let events = read("events.json");
    for ev in events.as_array().expect("array") {
        let a = dk_engine_wslc::com::convert::adapt_event_json(ev.clone());
        assert!(a["id"].as_str().is_some_and(|s| s.len() == 64), "{a}");
        assert!(a["status"].is_string());
    }
    assert!(
        read("list_networks.json")
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
    assert!(read("inspect_container.json")["State"]["Status"].is_string());
}

fn with_timeout_on_thread<T: Send + 'static>(
    d: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(d).expect("timed out")
}

/// Resolves after `d` (no tokio in this crate's tests).
async fn sleep(d: Duration) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// Review finding 2 (TRM-008): a write to a TTY whose peer never reads blocks; `close()` must
/// interrupt it promptly (not queue behind it) and the write must fail.
#[test]
fn exec_close_interrupts_a_stuck_write() {
    let _serial = serial();
    let (e, state) = engine();
    // `TerminalSession` isn't `Sync`, so the future (borrowing it twice) is built and polled
    // on the watchdog's worker thread.
    let (write_res, close_res, close_took, write_pending_before_close) =
        with_timeout_on_thread(Duration::from_secs(20), move || {
            block_on(async move {
                let req = dk_core::ExecRequest {
                    cmd: vec!["sh".into()],
                    tty: true,
                    env: vec![],
                    user: None,
                    working_dir: None,
                    cols: 80,
                    rows: 24,
                };
                let term = e.exec("2e4fac884218", req).await.expect("exec");
                let pending = std::sync::atomic::AtomicBool::new(true);
                // Far more than the 1 KiB pipe buffer: the write pends until someone reads.
                let write = async {
                    let r = term.write(bytes::Bytes::from(vec![b'x'; 256 * 1024])).await;
                    pending.store(false, Ordering::SeqCst);
                    r
                };
                let close = async {
                    sleep(Duration::from_millis(300)).await;
                    let still_pending = pending.load(Ordering::SeqCst);
                    let t = std::time::Instant::now();
                    let r = term.close().await;
                    (r, t.elapsed(), still_pending)
                };
                let (w, (c, took, still_pending)) = futures::join!(write, close);
                // A write after close fails right away too.
                let after = term.write(bytes::Bytes::from_static(b"y")).await;
                assert!(after.is_err(), "write after close: {after:?}");
                let exit = term.wait().await;
                assert_eq!(exit, Ok(Some(129)), "SIGHUP → signalled exit");
                (w, c, took, still_pending)
            })
        });
    assert!(
        write_pending_before_close,
        "the write should block on the stalled pipe"
    );
    assert_eq!(close_res, Ok(()));
    assert!(
        close_took < Duration::from_secs(1),
        "close took {close_took:?}"
    );
    assert!(
        matches!(&write_res, Err(EngineError::Protocol(m)) if m == "TTY closed"),
        "{write_res:?}"
    );
    let g = state.lock().expect("state");
    assert!(g.calls.iter().any(|c| c == "Exec"));
    assert_eq!(g.signalled, vec![1], "close sends SIGHUP");
}
