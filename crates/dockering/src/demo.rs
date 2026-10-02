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

/// Factories for `HubOptions.factories` in `--demo` mode.
pub fn factories() -> Vec<Arc<dyn EngineFactory>> {
    let factory = FakeFactory::new();
    factory.add(engine());
    vec![factory as Arc<dyn EngineFactory>]
}
