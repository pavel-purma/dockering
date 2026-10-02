//! Live WSLC COM tests (spec 20 §5.7 contract run on a real machine). `#[ignore]`d: they need
//! WSL ≥ 3.0 with WSLC enabled. Run:
//! `pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test com_live -- --ignored --nocapture --test-threads=1`

#![cfg(windows)]

use std::sync::{Arc, Once};
use std::time::Duration;

use dk_core::{
    ContainerQuery, Engine, EngineConfig, EngineEndpoint, EngineFactory, EngineId, EngineOrigin,
    EventFilter, ExecRequest, LogOpts, VolumeSpec, WslcTransportPref,
};
use dk_engine_wslc::com::WslcComEngine;
use dk_engine_wslc::com::abi;
use dk_engine_wslc::version;
use futures::StreamExt;
use futures::executor::block_on;

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| {
        let ok = dk_engine_wslc::init_process_com_security();
        eprintln!("init_process_com_security -> {ok}");
    });
}

/// Polls `fut` with a wall-clock limit (no tokio in this crate's tests).
fn with_timeout<T: Send + 'static>(
    d: Duration,
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(block_on(fut));
    });
    rx.recv_timeout(d).ok()
}

async fn com_engine() -> WslcComEngine {
    let v = version::detect().expect("WSL installed");
    let m = abi::select(&v).expect("verified ABI module for this WSL");
    let (e, check) = WslcComEngine::connect(EngineId::new("wslc-live"), None, v, m)
        .await
        .expect("COM connect + self-check");
    eprintln!("self-check: {check:?}");
    e
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_version_detect() {
    let v = version::detect().expect("WSL installed");
    eprintln!("WSL {v}, wslc registered: {}", version::wslc_registered());
    assert!(v.supports_wslc());
    assert!(abi::select(&v).is_some(), "no verified ABI module for {v}");
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_factory_discover_connect_auto_uses_com() {
    init();
    let f = dk_engine_wslc::WslcFactory::new();
    let found = block_on(f.discover());
    eprintln!(
        "discovered: {:#?}",
        found
            .iter()
            .map(|d| (&d.config.id, &d.config.name, d.show_only_when_all))
            .collect::<Vec<_>>()
    );
    let default = found
        .iter()
        .find(|d| d.config.id.as_str() == "wslc-default")
        .expect("wslc-default discovered");
    let probe = block_on(f.probe(&default.config));
    eprintln!("probe: {probe:?}");
    let engine = block_on(f.connect(&default.config)).expect("connect");
    let info = block_on(engine.info()).expect("info");
    eprintln!("info: {info:#?}");
    assert_eq!(info.transport.as_deref(), Some("com"));
    assert_eq!(info.transport_note, None);
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_lists() {
    init();
    block_on(async {
        let e = com_engine().await;
        let cs = e
            .list_containers(ContainerQuery::default())
            .await
            .expect("containers");
        eprintln!("containers: {}", cs.len());
        for c in &cs {
            eprintln!(
                "  {} {} {} {:?} ports={:?} nets={:?}",
                &c.id[..12.min(c.id.len())],
                c.name,
                c.image,
                c.state,
                c.ports,
                c.networks
            );
        }
        let imgs = e.list_images().await.expect("images");
        eprintln!("images: {}", imgs.len());
        for i in &imgs {
            eprintln!("  {} {:?} {} bytes", i.id, i.repo_tags, i.size);
        }
        let vols = e.list_volumes_json().await.expect("volumes json");
        eprintln!("volumes json: {vols}");
        let nets = e.list_networks_json().await.expect("networks json");
        eprintln!("networks json: {}", nets.as_array().map_or(0, Vec::len));
        e.ping().await.expect("ping");
    });
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_events_open_and_drop() {
    init();
    let e = Arc::new(block_on(com_engine()));
    let e2 = e.clone();
    let r = with_timeout(Duration::from_secs(10), async move {
        let mut s = e2.events_json(0);
        // Replays the buffered ring (since = 0); may be empty on a fresh session.
        let first = futures::future::select(
            Box::pin(s.next()),
            Box::pin(futures_timer(Duration::from_millis(800))),
        )
        .await;
        let got = match first {
            futures::future::Either::Left((item, _)) => format!("{item:?}"),
            futures::future::Either::Right(_) => "no event within 800 ms".into(),
        };
        drop(s); // fires the cancel event → GetNext returns E_ABORT
        got
    });
    let got = r.expect("events open/drop must not hang");
    eprintln!("first event: {got:.200}");
    // Typed stream too.
    let e3 = e.clone();
    with_timeout(Duration::from_secs(5), async move {
        let s = e3.events(EventFilter::default());
        drop(s);
    })
    .expect("typed events drop must not hang");
}

/// Executor-agnostic sleep.
async fn futures_timer(d: Duration) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let _ = tx.send(());
    });
    let _ = rx.await;
}

fn running_container(e: &WslcComEngine) -> Option<String> {
    let cs = block_on(e.list_containers(ContainerQuery::default())).ok()?;
    cs.into_iter().find(|c| c.state.is_running()).map(|c| c.id)
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC and a running container"]
fn live_running_container_stats_logs_exec() {
    init();
    let e = Arc::new(block_on(com_engine()));
    let Some(id) = running_container(&e) else {
        eprintln!("no running container; skipping");
        return;
    };
    eprintln!("using container {id}");

    let raw = block_on(e.inspect_container_json(&id)).expect("inspect json");
    eprintln!(
        "inspect: Id={} Name={} State={}",
        raw["Id"], raw["Name"], raw["State"]["Status"]
    );
    assert!(raw["Id"].as_str().is_some_and(|s| s.starts_with(&id[..12])));

    let stats = block_on(e.stats_json(&id)).expect("stats json");
    assert!(stats.get("cpu_stats").is_some(), "{stats}");
    let e2 = e.clone();
    let id2 = id.clone();
    let sample = with_timeout(Duration::from_secs(15), async move {
        let mut s = e2.stats(&id2);
        s.next().await
    })
    .expect("stats sample in time");
    eprintln!("stats sample: {sample:?}");
    assert!(matches!(sample, Some(Ok(_))));

    let e3 = e.clone();
    let id3 = id.clone();
    let lines = with_timeout(Duration::from_secs(15), async move {
        let s = e3.logs(
            &id3,
            LogOpts {
                follow: false,
                tail: Some(5),
                since: None,
                timestamps: true,
            },
        );
        s.collect::<Vec<_>>().await
    })
    .expect("logs (follow=false) must end");
    eprintln!("logs: {} chunks", lines.len());
    for l in &lines {
        match l {
            Ok(c) => eprintln!(
                "  {:?} {:?} {:?}",
                c.stream,
                c.ts,
                String::from_utf8_lossy(&c.bytes)
            ),
            Err(err) => panic!("log error: {err}"),
        }
    }

    // Follow + drop must not hang.
    let e4 = e.clone();
    let id4 = id.clone();
    with_timeout(Duration::from_secs(5), async move {
        let s = e4.logs(
            &id4,
            LogOpts {
                follow: true,
                tail: Some(1),
                since: None,
                timestamps: false,
            },
        );
        drop(s);
    })
    .expect("logs follow drop must not hang");

    let e5 = e.clone();
    let out = with_timeout(Duration::from_secs(20), async move {
        let req = ExecRequest {
            cmd: vec![
                "sh".into(),
                "-c".into(),
                "stty size; sleep 0.5; stty size; echo hi".into(),
            ],
            tty: true,
            env: vec!["TERM=xterm-256color".into()],
            user: None,
            working_dir: None,
            cols: 80,
            rows: 24,
        };
        let mut t = e5.exec(&id, req).await.expect("exec");
        let mut out = t.output();
        futures_timer(Duration::from_millis(200)).await;
        t.resize(132, 40).await.expect("resize");
        let mut buf = Vec::new();
        while let Some(chunk) = out.next().await {
            buf.extend_from_slice(&chunk.expect("tty chunk"));
        }
        let code = t.wait().await.expect("wait");
        (String::from_utf8_lossy(&buf).into_owned(), code)
    })
    .expect("exec finishes");
    eprintln!("exec output: {:?} exit={:?}", out.0, out.1);
    assert!(out.0.contains("hi"), "{:?}", out.0);
    assert!(out.0.contains("40 132"), "resize not observed: {:?}", out.0);
    assert_eq!(out.1, Some(0));
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_volume_create_remove() {
    init();
    block_on(async {
        let e = com_engine().await;
        let name = format!("dk-com-test-{:08x}", rand_u32());
        let spec = VolumeSpec {
            name: Some(name.clone()),
            ..Default::default()
        };
        // Raw path: `create_volume` additionally maps the inspect JSON via dk_core::docker_json.
        let created = e.create_volume_raw(spec).await.expect("CreateVolume");
        eprintln!("created volume: {created}");
        assert_eq!(created, name);
        let inspected = e
            .inspect_volume_json(&name)
            .await
            .expect("inspect created volume");
        assert_eq!(inspected["Name"], name.as_str());
        e.remove_volume(&name, false).await.expect("remove");
        let gone = e.inspect_volume_json(&name).await;
        eprintln!("after remove: {gone:?}");
        assert!(
            matches!(gone, Err(dk_core::EngineError::NotFound { .. })),
            "{gone:?}"
        );
    });
}

fn rand_u32() -> u32 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    t ^ std::process::id().rotate_left(16)
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC"]
fn live_cli_preference_and_unknown_config() {
    init();
    let f = dk_engine_wslc::WslcFactory::new();
    let cfg = EngineConfig {
        id: EngineId::new("wslc-com-forced"),
        name: "WSLC (COM)".into(),
        endpoint: EngineEndpoint::Wslc {
            session: None,
            transport: WslcTransportPref::Com,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    };
    let e = block_on(f.connect(&cfg)).expect("COM forced connect");
    let info = block_on(e.info()).expect("info");
    assert_eq!(info.transport.as_deref(), Some("com"));
}
