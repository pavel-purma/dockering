//! `--demo` data: a populated `FakeEngine` behind a `FakeFactory`, so the whole UI works
//! without Docker. Also used by view tests.

use std::collections::BTreeMap;
use std::sync::Arc;

use dk_core::fake::{FakeEngine, FakeFactory, fixtures};
use dk_core::grouping::{COMPOSE_DEPENDS_ON_LABEL, COMPOSE_ONEOFF_LABEL};
use dk_core::{ContainerState, EngineFactory, PortMapping, Proto};

/// Containers for the demo engine: two Compose projects, a label-grouped pair, and singles.
pub fn containers() -> Vec<dk_core::ContainerSummary> {
    use ContainerState::*;
    let mut web = fixtures::compose_container("myshop", "web", Running);
    web.image = "nginx:1.27".into();
    web.ports = vec![PortMapping {
        ip: None,
        private: 80,
        public: Some(8080),
        proto: Proto::Tcp,
    }];
    set_depends(&mut web, "api:service_started:false");
    let mut api = fixtures::compose_container("myshop", "api", Running);
    api.image = "myshop/api:latest".into();
    api.ports = vec![PortMapping {
        ip: None,
        private: 3000,
        public: Some(3000),
        proto: Proto::Tcp,
    }];
    set_depends(&mut api, "db:service_healthy:false");
    let mut db = fixtures::compose_container("myshop", "db", Running);
    db.image = "postgres:16".into();
    db.health = Some(dk_core::Health::Healthy);
    db.status_text = "Up 2 hours (healthy)".into();
    let mut migrate = fixtures::compose_container("myshop", "migrate", Exited);
    migrate.name = "myshop-migrate-run-1a2b".into();
    migrate.image = "myshop/api:latest".into();
    migrate
        .labels
        .insert(COMPOSE_ONEOFF_LABEL.into(), "True".into());
    if let Some(c) = migrate.compose.as_mut() {
        c.oneoff = true;
    }

    let mut grafana = fixtures::compose_container("monitoring", "grafana", Exited);
    grafana.image = "grafana/grafana:11".into();
    grafana.exit_code = Some(1);
    grafana.status_text = "Exited (1) 3 hours ago".into();
    let mut prom = fixtures::compose_container("monitoring", "prometheus", Running);
    prom.image = "prom/prometheus:v2.53".into();
    prom.ports = vec![PortMapping {
        ip: None,
        private: 9090,
        public: Some(9090),
        proto: Proto::Tcp,
    }];

    let mut redis = fixtures::container("redis", Running);
    redis.image = "redis:7".into();
    redis.ports = vec![PortMapping {
        ip: None,
        private: 6379,
        public: None,
        proto: Proto::Tcp,
    }];
    let mut scratch = fixtures::container("scratchpad", Exited);
    scratch.image = "alpine:3.20".into();
    let mut paused = fixtures::container("batch-worker", Paused);
    paused.image = "python:3.12-slim".into();
    let mut restarting = fixtures::container("flaky-job", Restarting);
    restarting.image = "busybox:1.36".into();
    let mut created = fixtures::container("fresh", Created);
    created.image = "hello-world:latest".into();
    let mut labelled = fixtures::container("billing-svc", Running);
    labelled.labels = BTreeMap::from([("app".into(), "billing".into())]);

    let mut all = vec![
        web, api, db, migrate, grafana, prom, redis, scratch, paused, restarting, created, labelled,
    ];
    link_resources(&mut all);
    all
}

/// M6: container image ids match the demo images, Compose members sit on their project
/// network, and a few containers mount the demo volumes (so "In use" shows everywhere).
fn link_resources(cs: &mut [dk_core::ContainerSummary]) {
    use dk_core::{MountKind, MountSummary};
    for c in cs.iter_mut() {
        c.image_id = image_id(&c.image);
        if let Some(project) = c.compose.as_ref().map(|ci| ci.project.clone()) {
            c.networks = vec![format!("{project}_default")];
        }
        let mount = match c.name.as_str() {
            "myshop-db-1" => Some(("myshop_pgdata", "/var/lib/postgresql/data", true)),
            "monitoring-grafana-1" => Some(("monitoring_grafana", "/var/lib/grafana", true)),
            "scratchpad" => Some(("scratch", "/scratch", false)),
            _ => None,
        };
        if let Some((source, destination, rw)) = mount {
            c.mounts = vec![MountSummary {
                kind: MountKind::Volume,
                source: source.into(),
                destination: destination.into(),
                rw,
            }];
        }
    }
}

/// The demo image id for a reference (stable across runs).
pub fn image_id(reference: &str) -> String {
    fixtures::image(reference, reference).id
}

fn set_depends(c: &mut dk_core::ContainerSummary, v: &str) {
    c.labels.insert(COMPOSE_DEPENDS_ON_LABEL.into(), v.into());
    if let Some(ci) = c.compose.as_mut() {
        ci.depends_on = v
            .split(',')
            .filter_map(|e| e.split(':').next())
            .map(str::to_owned)
            .collect();
    }
}

/// A fully populated fake engine with id `demo`.
pub fn engine() -> Arc<FakeEngine> {
    let e = FakeEngine::new("demo");
    e.set_containers(containers());
    seed_mount_details(&e);
    let image = |r: &str, size: u64| {
        let mut i = fixtures::image(r, r);
        i.size = size;
        i
    };
    let mut nginx = image("nginx:1.27", 192_000_000);
    nginx.repo_tags.push("nginx:latest".into());
    nginx.repo_digests = vec![format!("nginx:{}", "a".repeat(64))];
    e.set_images(vec![
        nginx,
        image("postgres:16", 438_000_000),
        image("redis:7", 117_000_000),
        image("myshop/api:latest", 245_000_000),
        image("grafana/grafana:11", 471_000_000),
        image("prom/prometheus:v2.53", 279_000_000),
        image("alpine:3.20", 7_800_000),
        image("python:3.12-slim", 130_000_000),
        image("busybox:1.36", 4_300_000),
        image("hello-world:latest", 13_000),
        image("node:22-alpine", 158_000_000),
        fixtures::image("", "dangling"),
    ]);
    let volume = |name: &str, size: u64, project: Option<&str>| {
        let mut v = fixtures::volume(name);
        v.size = Some(size);
        if let Some(p) = project {
            v.labels
                .insert(dk_core::grouping::COMPOSE_PROJECT_LABEL.into(), p.into());
            v.compose = dk_core::grouping::compose_info_from_labels(&v.labels);
        }
        v
    };
    e.set_volumes(vec![
        volume("myshop_pgdata", 1_240_000_000, Some("myshop")),
        volume("monitoring_grafana", 58_000_000, Some("monitoring")),
        volume("scratch", 4_000, None),
        volume("old-cache", 312_000_000, None),
    ]);
    let network = |name: &str, driver: &str, subnet: &str, project: Option<&str>| {
        let mut n = fixtures::network(name, driver);
        if let Some(s) = n.subnets.first_mut() {
            s.subnet = Some(format!("{subnet}.0/16"));
            s.gateway = Some(format!("{subnet}.1"));
        }
        if driver == "host" || driver == "null" {
            n.subnets.clear();
        }
        if let Some(p) = project {
            n.labels
                .insert(dk_core::grouping::COMPOSE_PROJECT_LABEL.into(), p.into());
            n.compose = dk_core::grouping::compose_info_from_labels(&n.labels);
        }
        n
    };
    e.set_networks(vec![
        network("bridge", "bridge", "172.17.0", None),
        network("host", "host", "", None),
        network("none", "null", "", None),
        network("myshop_default", "bridge", "172.20.0", Some("myshop")),
        network(
            "monitoring_default",
            "bridge",
            "172.21.0",
            Some("monitoring"),
        ),
        network("legacy-net", "bridge", "172.30.0", None),
    ]);
    e
}

/// Mounts tab data (CDT-020): the summary mounts as volumes, plus a few binds with long
/// host paths and mode options.
fn seed_mount_details(e: &FakeEngine) {
    use dk_core::{MountDetail, MountKind};
    let bind = |source: &str, destination: &str, mode: &str| MountDetail {
        kind: MountKind::Bind,
        source: source.into(),
        destination: destination.into(),
        mode: mode.into(),
        rw: !mode.split(',').any(|o| o == "ro"),
        propagation: Some("rprivate".into()),
        volume_name: None,
    };
    for c in containers() {
        let mut mounts: Vec<MountDetail> = c
            .mounts
            .iter()
            .map(|m| MountDetail {
                kind: MountKind::Volume,
                source: format!("/var/lib/docker/volumes/{}/_data", m.source),
                destination: m.destination.clone(),
                mode: "z".into(),
                rw: m.rw,
                propagation: None,
                volume_name: Some(m.source.clone()),
            })
            .collect();
        match c.name.as_str() {
            "myshop-db-1" => mounts.push(bind(
                r"C:\Users\dev\projects\myshop\docker\postgres\init-scripts",
                "/docker-entrypoint-initdb.d",
                "ro",
            )),
            "myshop-api-1" => {
                mounts.push(bind(
                    r"C:\Users\dev\.local\share\copilot-proxy-api",
                    "/root/.local/share/copilot-proxy-api",
                    "rw",
                ));
                mounts.push(bind("/run/secrets/api-token", "/run/secrets/token", "ro,Z"));
            }
            _ => {}
        }
        if mounts.is_empty() {
            continue;
        }
        let mut d = fixtures::details_for(c);
        d.mounts = mounts;
        e.set_container_details(d);
    }
}

/// Factories for `HubOptions.factories` in `--demo` mode, plus the engine (so `main` can
/// drive the live feed, [`tick`]).
pub fn factories() -> (Vec<Arc<dyn EngineFactory>>, Arc<FakeEngine>) {
    let factory = FakeFactory::new();
    let e = engine();
    seed_logs(&e);
    factory.add(e.clone());
    (vec![factory as Arc<dyn EngineFactory>], e)
}

/// Log history for the running demo containers (Logs tab, LOG-001).
fn seed_logs(e: &FakeEngine) {
    use dk_core::LogStream::{Stderr, Stdout};
    let now = time::OffsetDateTime::now_utc();
    for c in containers().iter().filter(|c| c.state.is_running()) {
        let lines: Vec<dk_core::LogChunk> = (0..40)
            .map(|i| {
                let (stream, text) = demo_log_line(&c.name, i);
                let stream = if stream { Stderr } else { Stdout };
                dk_core::LogChunk {
                    stream,
                    ts: Some(now - time::Duration::seconds(400 - i as i64 * 10)),
                    bytes: bytes::Bytes::from(format!(
                        "{text}
"
                    )),
                }
            })
            .collect();
        e.set_logs(&c.id, lines);
    }
}

/// One plausible log line (`stderr`, text) with some ANSI colour (LOG-002).
fn demo_log_line(name: &str, i: u64) -> (bool, String) {
    match i % 9 {
        0 => (
            false,
            format!(
                "[32mINFO[0m  {name}: request GET /api/items 200 in {}ms",
                3 + i % 40
            ),
        ),
        1 => (
            false,
            format!("[36mDEBUG[0m {name}: cache hit ratio {}%", 80 + i % 19),
        ),
        2 => (
            true,
            format!("[33mWARN[0m  {name}: slow query took {}ms", 200 + i % 300),
        ),
        3 => (
            false,
            format!("[32mINFO[0m  {name}: worker {} heartbeat", i % 4),
        ),
        4 => (
            true,
            format!(
                "[1;31mERROR[0m {name}: upstream timed out (attempt {})",
                1 + i % 3
            ),
        ),
        5 => (false, format!("{name}: [2mGET /healthz 200[0m")),
        6 => (
            false,
            format!("[32mINFO[0m  {name}: [1mready[0m to accept connections"),
        ),
        7 => (
            false,
            format!("{name}: processed batch #{i} ({} items)", 10 + i % 90),
        ),
        _ => (
            false,
            format!(
                "[35mTRACE[0m {name}: span id={:08x}",
                i * 2_654_435_761 % 4_294_967_296
            ),
        ),
    }
}

/// Pushes one live tick (a log line + a stats sample per running container); called about
/// once a second by `main` in `--demo` mode so Logs and Stats show live data.
pub fn tick(e: &FakeEngine, n: u64) {
    let now = time::OffsetDateTime::now_utc();
    for (k, c) in containers()
        .iter()
        .filter(|c| c.state.is_running())
        .enumerate()
    {
        let k = k as u64;
        let (stderr, text) = demo_log_line(&c.name, n + k * 7);
        let stream = if stderr {
            dk_core::LogStream::Stderr
        } else {
            dk_core::LogStream::Stdout
        };
        e.push_log(
            &c.id,
            dk_core::LogChunk {
                stream,
                ts: Some(now),
                bytes: bytes::Bytes::from(format!(
                    "{text}
"
                )),
            },
        );
        let phase = (n as f64 + k as f64 * 3.0) / 8.0;
        let mut s = fixtures::stats_sample(
            now,
            (12.0 + 10.0 * phase.sin() + (n % 5) as f64).max(0.5),
            ((180.0 + 40.0 * (phase / 2.0).cos()) * 1_000_000.0) as u64,
        );
        s.net_rx_bps = 20_000.0 + 15_000.0 * (phase * 1.3).sin().abs();
        s.net_tx_bps = 6_000.0 + 4_000.0 * (phase * 0.7).cos().abs();
        s.blk_read_bps = if n.is_multiple_of(7) { 120_000.0 } else { 0.0 };
        s.blk_write_bps = 30_000.0 + 25_000.0 * (phase * 0.9).sin().abs();
        s.net_rx_total = 340_000_000 + n * 20_000;
        s.net_tx_total = 12_000_000 + n * 6_000;
        e.push_stats(&c.id, s);
    }
}
