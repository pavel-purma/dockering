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

    vec![
        web, api, db, migrate, grafana, prom, redis, scratch, paused, restarting, created,
        labelled,
    ]
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
    e.set_images(vec![
        fixtures::image("nginx:1.27", "nginx"),
        fixtures::image("postgres:16", "postgres"),
        fixtures::image("redis:7", "redis"),
        fixtures::image("myshop/api:latest", "api"),
        fixtures::image("grafana/grafana:11", "grafana"),
        fixtures::image("", "dangling"),
    ]);
    e.set_volumes(vec![
        fixtures::volume("myshop_pgdata"),
        fixtures::volume("monitoring_grafana"),
        fixtures::volume("scratch"),
    ]);
    e.set_networks(vec![
        fixtures::network("bridge", "bridge"),
        fixtures::network("host", "host"),
        fixtures::network("none", "null"),
        fixtures::network("myshop_default", "bridge"),
    ]);
    e
}

/// Factories for `HubOptions.factories` in `--demo` mode.
pub fn factories() -> Vec<Arc<dyn EngineFactory>> {
    let factory = FakeFactory::new();
    factory.add(engine());
    vec![factory as Arc<dyn EngineFactory>]
}
