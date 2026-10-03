//! Live integration tests against a real Docker daemon (`--features it`).
//!
//! Target: `DOCKERING_IT_HOST` (`npipe:////./pipe/docker_engine`, `unix:///…`, `tcp://…`),
//! else the first engine `DockerFactory::discover()` finds. Creates and removes its own
//! throwaway resources (`dk-it-*`).

#![cfg(feature = "it")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dk_core::*;
use dk_engine_docker::{DockerEngine, DockerFactory, DockerTarget};
use futures::StreamExt;

const IMAGE: &str = "alpine:3.20";
const CONTAINER: &str = "dk-it-alpine";
const VOLUME: &str = "dk-it-vol";

fn target_from_env() -> Option<DockerTarget> {
    let host = std::env::var("DOCKERING_IT_HOST").ok()?;
    let (scheme, rest) = host.split_once("://")?;
    Some(match scheme {
        "unix" => DockerTarget::Unix(rest.into()),
        "npipe" => DockerTarget::NamedPipe(rest.replace('/', "\\")),
        _ => {
            let (h, p) = rest.rsplit_once(':')?;
            DockerTarget::Tcp {
                host: h.into(),
                port: p.parse().ok()?,
                tls: None,
            }
        }
    })
}

async fn engine() -> Arc<dyn Engine> {
    if let Some(t) = target_from_env() {
        let e = DockerEngine::connect("it".into(), t, Default::default())
            .await
            .expect("connect DOCKERING_IT_HOST");
        return Arc::new(e);
    }
    let f = DockerFactory::new();
    let found = f.discover().await;
    for d in &found {
        if d.initial_state.is_some() {
            continue;
        }
        if let Ok(e) = f.connect(&d.config).await {
            return e;
        }
    }
    panic!("no reachable Docker engine discovered: {found:#?}");
}

/// Pull alpine and (re)create the throwaway container; returns its id.
async fn ensure_container(e: &Arc<dyn Engine>) -> String {
    let mut pull = e.pull_image(IMAGE, None);
    let mut done = false;
    while let Some(p) = pull.next().await {
        if matches!(p.expect("pull progress"), PullProgress::Done { .. }) {
            done = true;
        }
    }
    assert!(done, "pull ended without Done");
    let _ = e
        .remove_container(
            CONTAINER,
            RemoveContainerOpts {
                force: true,
                volumes: true,
            },
        )
        .await;
    e.run_image(RunSpec {
        image: IMAGE.into(),
        name: Some(CONTAINER.into()),
        cmd: Some(vec![
            "sh".into(),
            "-c".into(),
            "echo ready; echo oops >&2; sleep 300".into(),
        ]),
        labels: [(
            "com.docker.compose.project".to_string(),
            "dk-it".to_string(),
        )]
        .into(),
        ..Default::default()
    })
    .await
    .expect("run_image")
}

async fn remove_container(e: &Arc<dyn Engine>) {
    let _ = e
        .remove_container(
            CONTAINER,
            RemoveContainerOpts {
                force: true,
                volumes: true,
            },
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_eng_006_discovery_finds_local_engine() {
    let found = DockerFactory::new().discover().await;
    assert!(!found.is_empty(), "nothing discovered");
    let f = DockerFactory::new();
    let mut reachable = 0;
    for d in &found {
        if matches!(f.probe(&d.config).await, ProbeResult::Reachable) {
            reachable += 1;
        }
    }
    assert!(
        reachable > 0,
        "no discovered engine is reachable: {found:#?}"
    );
    if cfg!(windows) {
        assert!(
            found
                .iter()
                .any(|d| matches!(&d.config.endpoint, EngineEndpoint::NamedPipe { .. }))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_eng_108_connect_and_info() {
    let e = engine().await;
    e.ping().await.unwrap();
    let info = e.info().await.unwrap();
    assert!(!info.server_version.is_empty());
    assert!(info.api_version.is_some());
    assert!(!info.arch.is_empty());
    assert_eq!(info.capabilities, e.capabilities());
    assert!(
        info.capabilities
            .contains(Capabilities::DOCKER - Capabilities::DISK_USAGE)
    );
    assert!(info.daemon_id.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn it_eng_wsl_options_reflected_in_kind_and_info() {
    let Some(target) = target_from_env().or_else(|| {
        cfg!(windows).then(|| DockerTarget::NamedPipe(r"\\.\pipe\docker_engine".into()))
    }) else {
        return;
    };
    let keep: Arc<dyn std::any::Any + Send + Sync> = Arc::new(42u8);
    let e = DockerEngine::connect(
        "wsl-test".into(),
        target,
        dk_engine_docker::DockerEngineOptions {
            kind: Some(EngineKind::WslDistro),
            transport: Some("bridge".into()),
            keepalive: Some(keep.clone()),
        },
    )
    .await
    .unwrap();
    assert_eq!(e.kind(), EngineKind::WslDistro);
    let info = e.info().await.unwrap();
    assert_eq!(info.kind, EngineKind::WslDistro);
    assert_eq!(info.transport.as_deref(), Some("bridge"));
    assert_eq!(info.list_stats_limit, 8);
    assert_eq!(Arc::strong_count(&keep), 2);
    drop(e);
    assert_eq!(Arc::strong_count(&keep), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn it_container_lifecycle_logs_stats_events_exec() {
    let e = engine().await;
    // Streams are lazy (the request is sent on first poll), so subscribe with `since`.
    let since = time::OffsetDateTime::now_utc() - time::Duration::seconds(2);
    let id = ensure_container(&e).await;
    let mut events = e.events(EventFilter {
        kinds: vec![ResourceKind::Container],
        since: Some(since),
    });

    // List / inspect.
    let list = e.list_containers(ContainerQuery::default()).await.unwrap();
    let c = list.iter().find(|c| c.id == id).expect("listed");
    assert_eq!(c.name, CONTAINER);
    assert_eq!(c.compose.as_ref().unwrap().project, "dk-it");
    let d = e.inspect_container(CONTAINER).await.unwrap();
    assert_eq!(d.summary.id, id);
    assert_eq!(d.summary.state, ContainerState::Running);
    assert!(matches!(
        e.inspect_container("dk-it-missing").await,
        Err(EngineError::NotFound {
            kind: ResourceKind::Container,
            ..
        })
    ));

    // Events: at least the start of our container.
    let start = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = events.next().await {
            let ev = ev.unwrap();
            if ev.id == id && ev.action == "start" {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(start, "no start event for {id}");

    // Logs (follow=false), both streams, timestamps parsed.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let logs: Vec<LogChunk> = e
        .logs(
            CONTAINER,
            LogOpts {
                follow: false,
                tail: Some(10),
                since: None,
                timestamps: true,
            },
        )
        .map(|c| c.unwrap())
        .collect()
        .await;
    assert!(
        logs.iter()
            .any(|l| l.stream == LogStream::Stdout && l.bytes.starts_with(b"ready")),
        "{logs:?}"
    );
    assert!(
        logs.iter()
            .any(|l| l.stream == LogStream::Stderr && l.bytes.starts_with(b"oops"))
    );
    assert!(logs.iter().all(|l| l.ts.is_some()));

    // Stats: two samples, monotonic.
    let samples: Vec<StatsSample> = e
        .stats(CONTAINER)
        .take(2)
        .map(|s| s.unwrap())
        .collect()
        .await;
    assert_eq!(samples.len(), 2);
    assert!(samples[1].at >= samples[0].at);
    assert!(samples[1].mem_used > 0);

    // Top.
    let top = e.top(CONTAINER).await.unwrap();
    assert!(!top.processes.is_empty());

    // Exec `echo hi` (TTY) → output + exit code.
    let mut sess = e
        .exec(
            CONTAINER,
            ExecRequest {
                cmd: vec!["echo".into(), "hi".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut out = sess.output();
    let mut buf = Vec::new();
    while let Some(b) = out.next().await {
        buf.extend_from_slice(&b.unwrap());
    }
    assert!(String::from_utf8_lossy(&buf).contains("hi"), "{buf:?}");
    assert_eq!(sess.wait().await.unwrap(), Some(0));

    // Interactive exec: write + resize + exit code.
    let mut sh = e
        .exec(
            CONTAINER,
            ExecRequest {
                cmd: vec!["sh".into()],
                cols: 100,
                rows: 30,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut out = sh.output();
    sh.resize(120, 40).await.unwrap();
    sh.write(Bytes::from_static(b"stty size; exit 7\n"))
        .await
        .unwrap();
    let mut buf = Vec::new();
    while let Some(b) = out.next().await {
        buf.extend_from_slice(&b.unwrap());
    }
    assert!(String::from_utf8_lossy(&buf).contains("40 120"), "{buf:?}");
    assert_eq!(sh.wait().await.unwrap(), Some(7));
    sh.close().await.unwrap();

    // Actions: pause/unpause, 304 start, stop.
    e.container_action(CONTAINER, ContainerAction::Pause)
        .await
        .unwrap();
    e.container_action(CONTAINER, ContainerAction::Unpause)
        .await
        .unwrap();
    e.container_action(CONTAINER, ContainerAction::Start)
        .await
        .unwrap();
    e.container_action(CONTAINER, ContainerAction::Stop { timeout_s: Some(1) })
        .await
        .unwrap();
    e.container_action(CONTAINER, ContainerAction::Stop { timeout_s: Some(1) })
        .await
        .unwrap();
    let d = e.inspect_container(CONTAINER).await.unwrap();
    assert_eq!(d.summary.state, ContainerState::Exited);

    remove_container(&e).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_vol_create_inspect_remove() {
    let e = engine().await;
    let _ = e.remove_volume(VOLUME, true).await;
    let v = e
        .create_volume(VolumeSpec {
            name: Some(VOLUME.into()),
            labels: [(
                "com.docker.compose.project".to_string(),
                "dk-it".to_string(),
            )]
            .into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(v.name, VOLUME);
    assert_eq!(v.compose.as_ref().unwrap().project, "dk-it");
    let d = e.inspect_volume(VOLUME).await.unwrap();
    assert_eq!(d.summary.name, VOLUME);
    assert!(d.used_by.is_empty());
    assert!(
        e.list_volumes()
            .await
            .unwrap()
            .iter()
            .any(|x| x.name == VOLUME)
    );
    e.remove_volume(VOLUME, false).await.unwrap();
    assert!(matches!(
        e.inspect_volume(VOLUME).await,
        Err(EngineError::NotFound {
            kind: ResourceKind::Volume,
            ..
        })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn it_img_net_and_disk_usage() {
    let e = engine().await;
    let imgs = e.list_images().await.unwrap();
    assert!(!imgs.is_empty());
    let nets = e.list_networks().await.unwrap();
    assert!(nets.iter().any(|n| n.name == "bridge" && n.is_builtin()));
    let bridge = e.inspect_network("bridge").await.unwrap();
    assert_eq!(bridge.summary.name, "bridge");
    // DISK_USAGE needs API ≥ 1.52 (bollard 0.21 drops the older `/system/df` shape).
    if e.capabilities().contains(Capabilities::DISK_USAGE) {
        let du = e.disk_usage().await.unwrap();
        assert!(du.images_size > 0);
    } else {
        assert!(matches!(
            e.disk_usage().await,
            Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
        ));
    }
    assert!(e.remove_network("-bad").await.is_err());
}
