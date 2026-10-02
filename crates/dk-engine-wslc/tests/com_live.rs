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

/// PullImage with a Rust `#[implement(IProgressCallback)]` (spec 20 §5.4, S-3). Pulls a small
/// image (`busybox:1.37`, ~4 MB) and removes it again unless it was already present.
#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC and network access"]
fn live_pull_with_progress_callback() {
    init();
    let e = Arc::new(block_on(com_engine()));
    let reference = "busybox:1.37";
    let had = block_on(e.list_images())
        .expect("images")
        .iter()
        .any(|i| i.repo_tags.iter().any(|t| t == reference));
    let e2 = e.clone();
    let events = with_timeout(Duration::from_secs(180), async move {
        e2.pull_image(reference, None).collect::<Vec<_>>().await
    })
    .expect("pull finishes");
    let layers = events
        .iter()
        .filter(|p| matches!(p, Ok(dk_core::PullProgress::Layer { .. })))
        .count();
    eprintln!(
        "pull: {} events, {layers} layer events, last = {:?}",
        events.len(),
        events.last()
    );
    for ev in events.iter().take(8) {
        eprintln!("  {ev:?}");
    }
    assert!(
        matches!(events.last(), Some(Ok(dk_core::PullProgress::Done { .. }))),
        "{:?}",
        events.last()
    );
    let tagged = block_on(e.list_images())
        .expect("images")
        .iter()
        .any(|i| i.repo_tags.iter().any(|t| t == reference));
    assert!(tagged, "pulled image listed");
    if !had {
        let removed = block_on(e.remove_image(reference, false)).expect("remove pulled image");
        eprintln!("removed: {removed:?}");
    }
}

/// Records real COM outputs (raw JSON) as fixtures under `tests/fixtures/com-3.0.1/` when
/// `DK_RECORD_FIXTURES=1`. Volatile fields are kept verbatim (the fake replays them).
#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC; set DK_RECORD_FIXTURES=1 to write files"]
fn live_record_fixtures() {
    init();
    let e = block_on(com_engine());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/com-3.0.1");
    let write = std::env::var("DK_RECORD_FIXTURES").is_ok_and(|v| v == "1");
    let save = |name: &str, v: &serde_json::Value| {
        let text = serde_json::to_string_pretty(v).expect("json");
        eprintln!("{name}: {} bytes", text.len());
        if write {
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join(name), text + "\n").expect("write");
        }
    };
    save(
        "list_networks.json",
        &block_on(e.list_networks_json()).expect("networks"),
    );
    save(
        "list_volumes.json",
        &block_on(e.list_volumes_json()).expect("volumes"),
    );
    if let Some(id) = running_container(&e) {
        save(
            "inspect_container.json",
            &block_on(e.inspect_container_json(&id)).expect("inspect"),
        );
        save("stats.json", &block_on(e.stats_json(&id)).expect("stats"));
        let image = block_on(e.list_containers(ContainerQuery::default()))
            .expect("list")
            .into_iter()
            .find(|c| c.id == id)
            .map(|c| c.image)
            .expect("image");
        save(
            "inspect_image.json",
            &block_on(e.inspect_image_json(&image)).expect("inspect image"),
        );
        let nets = block_on(e.list_networks_json()).expect("networks");
        if let Some(n) = nets[0]["Name"].as_str() {
            save(
                "inspect_network.json",
                &block_on(e.inspect_network_json(n)).expect("inspect network"),
            );
        }
    }
    let ev = with_timeout(Duration::from_secs(5), {
        let s = e.events_json(0);
        async move { s.take(3).collect::<Vec<_>>().await }
    });
    if let Some(ev) = ev {
        let arr: Vec<serde_json::Value> = ev.into_iter().filter_map(Result::ok).collect();
        save("events.json", &serde_json::Value::Array(arr));
    }
}

#[test]
#[ignore = "needs WSL ≥ 3.0 with WSLC and the merged docker_json/CLI"]
fn live_typed_ops_and_cli_pref() {
    init();
    let e = block_on(com_engine());
    if let Some(id) = running_container(&e) {
        let d = block_on(e.inspect_container(&id)).expect("typed inspect");
        eprintln!(
            "typed inspect: {} {:?} env={}",
            d.summary.name,
            d.summary.state,
            d.env.len()
        );
    }
    let vols = block_on(e.list_volumes()).expect("typed volumes");
    let nets = block_on(e.list_networks()).expect("typed networks");
    eprintln!(
        "typed: {} volumes, {} networks: {:?}",
        vols.len(),
        nets.len(),
        nets.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
    let name = format!("dk-com-typed-{:08x}", rand_u32());
    let v = block_on(e.create_volume(VolumeSpec {
        name: Some(name.clone()),
        ..Default::default()
    }))
    .expect("typed create");
    assert_eq!(v.name, name);
    let det = block_on(e.inspect_volume(&name)).expect("typed inspect volume");
    eprintln!(
        "volume: {} driver={} used_by={:?}",
        det.summary.name, det.summary.driver, det.used_by
    );
    block_on(e.remove_volume(&name, false)).expect("remove");
    let s = with_timeout(Duration::from_secs(5), {
        let s = e.events(EventFilter::default());
        async move { drop(s) }
    });
    assert!(s.is_some());
    // CLI preference → CLI transport (implemented on main).
    let f = dk_engine_wslc::WslcFactory::new();
    let cfg = EngineConfig {
        id: EngineId::new("wslc-cli-forced"),
        name: "WSLC (CLI)".into(),
        endpoint: EngineEndpoint::Wslc {
            session: None,
            transport: WslcTransportPref::Cli,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    };
    // The CLI transport spawns tokio processes: drive it on a tokio runtime like the hub does.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let cli = rt.block_on(f.connect(&cfg)).expect("CLI connect");
    let info = rt.block_on(cli.info()).expect("cli info");
    eprintln!(
        "cli info transport={:?} note={:?}",
        info.transport, info.transport_note
    );
    assert_eq!(info.transport.as_deref(), Some("cli"));
}
