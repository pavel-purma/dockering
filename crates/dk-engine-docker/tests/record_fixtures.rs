//! Records sanitised Docker Engine API fixtures from a live daemon (spec 60 "Fixtures").
//!
//! ```sh
//! DOCKERING_RECORD_FIXTURES=1 cargo test -p dk-engine-docker --test record_fixtures -- --ignored
//! ```
//!
//! Optional `DOCKERING_FIXTURE_HOST` (`npipe:////./pipe/docker_engine`, `unix:///…`,
//! `tcp://…`); default is the platform's local socket/pipe.
//!
//! Only resources this recorder creates (`dk-fx-*`) are recorded, plus allow-listed
//! `/info` and `/version` fields, so nothing from the user's own containers is committed.
//! Env values, auth-like keys, and host names are redacted (NFR-020).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HealthConfig, HostConfig, Mount, MountType,
    NetworkCreateRequest, NetworkingConfig, PortBinding, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, EventsOptionsBuilder,
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListNetworksOptionsBuilder,
    ListVolumesOptionsBuilder, RemoveContainerOptionsBuilder, StatsOptionsBuilder,
    WaitContainerOptionsBuilder,
};
use futures::StreamExt;
use serde_json::{Value, json};

const IMAGE: &str = "alpine:3.20";
const PREFIX: &str = "dk-fx-";
const REDACTED: &str = "<redacted>";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn connect() -> Docker {
    match std::env::var("DOCKERING_FIXTURE_HOST") {
        Ok(host) => Docker::connect_with_host(&host).expect("connect DOCKERING_FIXTURE_HOST"),
        Err(_) => Docker::connect_with_local_defaults().expect("connect local daemon"),
    }
}

// ───────────────────────────── sanitising ─────────────────────────────

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "auth",
        "password",
        "secret",
        "token",
        "identitytoken",
        "proxy",
    ]
    .iter()
    .any(|n| k.contains(n))
}

/// Recursively redacts env values and auth-like keys.
fn sanitize(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if k == "Env" {
                    if let Value::Array(items) = val {
                        for item in items {
                            if let Value::String(s) = item {
                                let key = s.split('=').next().unwrap_or_default().to_owned();
                                *s = format!("{key}={REDACTED}");
                            }
                        }
                    }
                } else if is_secret_key(k) && val.is_string() {
                    *val = Value::String(REDACTED.into());
                } else {
                    sanitize(val);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(sanitize),
        _ => {}
    }
}

/// Keep only `keys` of an object.
fn allow(v: &Value, keys: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for k in keys {
        if let Some(x) = v.get(*k) {
            out.insert((*k).to_owned(), x.clone());
        }
    }
    Value::Object(out)
}

fn write(op: &str, name: &str, mut v: Value) {
    sanitize(&mut v);
    let dir = fixtures_dir().join(op);
    std::fs::create_dir_all(&dir).unwrap();
    let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
    std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
}

fn to_value<T: serde::Serialize>(t: &T) -> Value {
    serde_json::to_value(t).unwrap()
}

fn names_of(v: &Value) -> Vec<String> {
    v.get("Names")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn is_fixture_container(v: &Value) -> bool {
    names_of(v)
        .iter()
        .any(|n| n.trim_start_matches('/').starts_with(PREFIX))
}

// ───────────────────────────── resources ─────────────────────────────

fn compose_labels(service: &str, number: &str) -> HashMap<String, String> {
    [
        ("com.docker.compose.project", "dk-fx"),
        ("com.docker.compose.service", service),
        ("com.docker.compose.container-number", number),
        ("com.docker.compose.project.working_dir", "/srv/dk-fx"),
        (
            "com.docker.compose.project.config_files",
            "/srv/dk-fx/compose.yaml",
        ),
        ("com.docker.compose.oneoff", "False"),
        ("com.docker.compose.depends_on", "db:service_started:false"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

async fn cleanup(d: &Docker) {
    for name in ["dk-fx-web", "dk-fx-tty", "dk-fx-exited"] {
        let _ = d
            .remove_container(
                name,
                Some(RemoveContainerOptionsBuilder::new().force(true).build()),
            )
            .await;
    }
    let _ = d
        .remove_volume(
            "dk-fx-data",
            None::<bollard::query_parameters::RemoveVolumeOptions>,
        )
        .await;
    let _ = d.remove_network("dk-fx-net").await;
}

async fn create(d: &Docker, name: &str, body: ContainerCreateBody) -> String {
    let opts = CreateContainerOptionsBuilder::new().name(name).build();
    d.create_container(Some(opts), body).await.unwrap().id
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live Docker daemon; set DOCKERING_RECORD_FIXTURES=1"]
async fn record_fixtures() {
    if std::env::var("DOCKERING_RECORD_FIXTURES").as_deref() != Ok("1") {
        eprintln!("DOCKERING_RECORD_FIXTURES != 1, skipping");
        return;
    }
    let d = connect();
    cleanup(&d).await;
    let started = time::OffsetDateTime::now_utc().unix_timestamp();

    // Pull (record the progress stream too).
    let (from, tag) = IMAGE.split_once(':').unwrap();
    let pull: Vec<Value> = d
        .create_image(
            Some(
                CreateImageOptionsBuilder::new()
                    .from_image(from)
                    .tag(tag)
                    .build(),
            ),
            None,
            None,
        )
        .map(|r| to_value(&r.unwrap()))
        .collect()
        .await;
    write("images", "pull_progress", Value::Array(pull));

    // Network + volume with compose labels.
    let mut net_labels = HashMap::new();
    net_labels.insert("com.docker.compose.project".to_owned(), "dk-fx".to_owned());
    net_labels.insert(
        "com.docker.compose.network".to_owned(),
        "default".to_owned(),
    );
    d.create_network(NetworkCreateRequest {
        name: "dk-fx-net".into(),
        driver: Some("bridge".into()),
        attachable: Some(true),
        labels: Some(net_labels.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let mut vol_labels = net_labels.clone();
    vol_labels.remove("com.docker.compose.network");
    vol_labels.insert("com.docker.compose.volume".to_owned(), "data".to_owned());
    d.create_volume(VolumeCreateRequest {
        name: Some("dk-fx-data".into()),
        labels: Some(vol_labels),
        ..Default::default()
    })
    .await
    .unwrap();

    // Web: compose labels, healthcheck, port, volume, env (with a secret), custom network.
    let mut bindings = HashMap::new();
    bindings.insert(
        "80/tcp".to_owned(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: Some(String::new()), // ephemeral
        }]),
    );
    let mut endpoints = HashMap::new();
    endpoints.insert(
        "dk-fx-net".to_owned(),
        EndpointSettings {
            aliases: Some(vec!["web".into()]),
            ..Default::default()
        },
    );
    let web = create(
        &d,
        "dk-fx-web",
        ContainerCreateBody {
            image: Some(IMAGE.into()),
            cmd: Some(vec![
                "sh".into(),
                "-c".into(),
                "echo started; while true; do echo tick; sleep 1; done".into(),
            ]),
            env: Some(vec![
                "APP_MODE=fixture".into(),
                "DB_PASSWORD=hunter2".into(),
            ]),
            labels: Some(compose_labels("web", "1")),
            exposed_ports: Some(vec!["80/tcp".into()]),
            working_dir: Some("/srv".into()),
            healthcheck: Some(HealthConfig {
                test: Some(vec!["CMD".into(), "true".into()]),
                interval: Some(1_000_000_000),
                timeout: Some(1_000_000_000),
                retries: Some(1),
                ..Default::default()
            }),
            host_config: Some(HostConfig {
                port_bindings: Some(bindings),
                mounts: Some(vec![Mount {
                    target: Some("/data".into()),
                    source: Some("dk-fx-data".into()),
                    typ: Some(MountType::VOLUME),
                    read_only: Some(false),
                    ..Default::default()
                }]),
                memory: Some(256 * 1024 * 1024),
                nano_cpus: Some(500_000_000),
                restart_policy: Some(bollard::models::RestartPolicy {
                    name: Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED),
                    maximum_retry_count: None,
                }),
                ..Default::default()
            }),
            networking_config: Some(NetworkingConfig {
                endpoints_config: Some(endpoints),
            }),
            ..Default::default()
        },
    )
    .await;
    d.start_container(
        &web,
        None::<bollard::query_parameters::StartContainerOptions>,
    )
    .await
    .unwrap();

    // TTY container (LOG-008).
    let tty = create(
        &d,
        "dk-fx-tty",
        ContainerCreateBody {
            image: Some(IMAGE.into()),
            tty: Some(true),
            cmd: Some(vec![
                "sh".into(),
                "-c".into(),
                "echo hello-tty; sleep 600".into(),
            ]),
            labels: Some(compose_labels("tty", "1")),
            ..Default::default()
        },
    )
    .await;
    d.start_container(
        &tty,
        None::<bollard::query_parameters::StartContainerOptions>,
    )
    .await
    .unwrap();

    // Exited container with a non-zero exit code.
    let exited = create(
        &d,
        "dk-fx-exited",
        ContainerCreateBody {
            image: Some(IMAGE.into()),
            cmd: Some(vec!["sh".into(), "-c".into(), "echo bye; exit 3".into()]),
            ..Default::default()
        },
    )
    .await;
    d.start_container(
        &exited,
        None::<bollard::query_parameters::StartContainerOptions>,
    )
    .await
    .unwrap();
    let _ = d
        .wait_container(
            &exited,
            Some(
                WaitContainerOptionsBuilder::new()
                    .condition("not-running")
                    .build(),
            ),
        )
        .collect::<Vec<_>>()
        .await;

    // Let the healthcheck report healthy.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ── containers ──
    let mut name_filter = HashMap::new();
    name_filter.insert("name".to_owned(), vec![PREFIX.to_owned()]);
    let list = d
        .list_containers(Some(
            ListContainersOptionsBuilder::new()
                .all(true)
                .size(true)
                .filters(&name_filter)
                .build(),
        ))
        .await
        .unwrap();
    let list: Vec<Value> = list
        .iter()
        .map(to_value)
        .filter(is_fixture_container)
        .collect();
    write("containers", "list", Value::Array(list));
    for (name, id) in [("web", &web), ("tty", &tty), ("exited", &exited)] {
        let v = to_value(&d.inspect_container(id, None).await.unwrap());
        write("containers", &format!("inspect_{name}"), v);
    }
    write(
        "containers",
        "top_web",
        to_value(&d.top_processes(&web, None).await.unwrap()),
    );

    // ── stats (two samples) ──
    let stats: Vec<Value> = d
        .stats(&web, Some(StatsOptionsBuilder::new().stream(true).build()))
        .take(2)
        .map(|r| to_value(&r.unwrap()))
        .collect()
        .await;
    write("stats", "web_stream", Value::Array(stats));

    // ── images ──
    let mut ref_filter = HashMap::new();
    ref_filter.insert("reference".to_owned(), vec![IMAGE.to_owned()]);
    let images = d
        .list_images(Some(
            ListImagesOptionsBuilder::new().filters(&ref_filter).build(),
        ))
        .await
        .unwrap();
    write("images", "list", to_value(&images));
    write(
        "images",
        "inspect_alpine",
        to_value(&d.inspect_image(IMAGE).await.unwrap()),
    );
    write(
        "images",
        "history_alpine",
        to_value(&d.image_history(IMAGE).await.unwrap()),
    );

    // ── volumes ──
    let vols = d
        .list_volumes(Some(
            ListVolumesOptionsBuilder::new()
                .filters(&name_filter)
                .build(),
        ))
        .await
        .unwrap();
    write("volumes", "list", to_value(&vols));
    write(
        "volumes",
        "inspect_data",
        to_value(&d.inspect_volume("dk-fx-data").await.unwrap()),
    );

    // ── networks ──
    let mut net_filter = HashMap::new();
    net_filter.insert("name".to_owned(), vec!["dk-fx-net".to_owned()]);
    let nets = d
        .list_networks(Some(
            ListNetworksOptionsBuilder::new()
                .filters(&net_filter)
                .build(),
        ))
        .await
        .unwrap();
    write("networks", "list", to_value(&nets));
    let mut net = to_value(&d.inspect_network("dk-fx-net", None).await.unwrap());
    if let Some(Value::Object(containers)) = net.get_mut("Containers") {
        containers.retain(|_, c| {
            c.get("Name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.starts_with(PREFIX))
        });
    }
    write("networks", "inspect_net", net);

    // ── system ──
    let info = to_value(&d.info().await.unwrap());
    let mut info = allow(
        &info,
        &[
            "ID",
            "Name",
            "OperatingSystem",
            "OSType",
            "Architecture",
            "KernelVersion",
            "NCPU",
            "MemTotal",
            "Containers",
            "ContainersRunning",
            "ContainersPaused",
            "ContainersStopped",
            "Images",
            "Driver",
            "DockerRootDir",
            "ServerVersion",
        ],
    );
    info["ID"] = json!("00000000-0000-0000-0000-000000000000");
    info["Name"] = json!("dk-fixture-host");
    write("system", "info", info);
    let version = to_value(&d.version().await.unwrap());
    write(
        "system",
        "version",
        allow(
            &version,
            &[
                "Version",
                "ApiVersion",
                "MinAPIVersion",
                "Os",
                "Arch",
                "KernelVersion",
            ],
        ),
    );
    let mut df = to_value(&d.df(None).await.unwrap());
    let keep_item = |section: &str, item: &Value| -> bool {
        match section {
            "ImageUsage" => item
                .get("RepoTags")
                .and_then(Value::as_array)
                .is_some_and(|t| t.iter().any(|x| x.as_str() == Some(IMAGE))),
            "ContainerUsage" => is_fixture_container(item),
            "VolumeUsage" => item
                .get("Name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.starts_with(PREFIX)),
            _ => false,
        }
    };
    for section in [
        "ImageUsage",
        "ContainerUsage",
        "VolumeUsage",
        "BuildCacheUsage",
    ] {
        if let Some(Value::Array(items)) = df.get_mut(section).and_then(|s| s.get_mut("Items")) {
            items.retain(|i| keep_item(section, i));
        }
    }
    // Legacy (< 1.52) shape.
    for (section, mapped) in [
        ("Images", "ImageUsage"),
        ("Containers", "ContainerUsage"),
        ("Volumes", "VolumeUsage"),
        ("BuildCache", "BuildCacheUsage"),
    ] {
        if let Some(Value::Array(items)) = df.get_mut(section) {
            items.retain(|i| keep_item(mapped, i));
        }
    }
    write("system", "df", df);

    // ── events (bounded by `until`) ──
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let events: Vec<Value> = d
        .events(Some(
            EventsOptionsBuilder::new()
                .since(&started.to_string())
                .until(&now.to_string())
                .build(),
        ))
        .filter_map(|r| async move { r.ok().map(|m| to_value(&m)) })
        .filter(|v| {
            let attrs = v
                .pointer("/Actor/Attributes")
                .cloned()
                .unwrap_or(Value::Null);
            let name = attrs.get("name").and_then(Value::as_str).unwrap_or("");
            let keep = name.starts_with(PREFIX) || name == IMAGE || name == "alpine";
            futures::future::ready(keep)
        })
        .collect()
        .await;
    write("events", "recorded", Value::Array(events));

    cleanup(&d).await;
}

#[test]
fn nfr_020_sanitizer_redacts_env_and_auth() {
    let mut v = json!({
        "Config": {"Env": ["A=1", "DB_PASSWORD=hunter2", "EMPTY"]},
        "auths": {"x": {"auth": "dXNlcjpwYXNz"}},
        "HttpProxy": "http://user:pw@proxy",
        "Nested": [{"IdentityToken": "t"}]
    });
    sanitize(&mut v);
    let s = v.to_string();
    assert!(!s.contains("hunter2"));
    assert!(!s.contains("dXNlcjpwYXNz"));
    assert!(!s.contains("user:pw"));
    assert!(s.contains("DB_PASSWORD=<redacted>"));
}
