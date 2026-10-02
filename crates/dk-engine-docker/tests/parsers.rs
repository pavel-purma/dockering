//! Parser snapshot tests: recorded Docker Engine API fixtures → DTOs via
//! `dk_core::docker_json` (spec 60 "Parser" layer, NFR-031).
//!
//! Fixtures are produced by `tests/record_fixtures.rs`. Time-relative fields (`status_text`
//! of inspected containers) are normalised so snapshots are stable.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use dk_core::docker_json;
use dk_core::stats::{RawStats, StatsNormalizer};
use dk_core::{ContainerState, Health, LogStream, MountKind, ResourceKind};
use serde_json::Value;

fn fixture(op: &str, name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(op)
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn items(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// Replace relative-time text (`Up 3 seconds`) with a stable token.
fn stable_status(v: &mut Value) {
    if let Some(Value::String(s)) = v.get_mut("status_text") {
        let state_word = s.split_whitespace().next().unwrap_or("").to_owned();
        let suffix = s
            .find('(')
            .filter(|_| state_word == "Up")
            .map(|i| s[i..].to_owned());
        let exit = s
            .strip_prefix("Exited ")
            .and_then(|r| r.split_whitespace().next())
            .map(ToOwned::to_owned);
        *s = match (state_word.as_str(), exit, suffix) {
            ("Exited", Some(code), _) => format!("Exited {code} <ago>"),
            ("Up", _, Some(sfx)) => format!("Up <duration> {sfx}"),
            ("Up", _, None) => "Up <duration>".into(),
            _ => s.clone(),
        };
    }
}

#[test]
fn con_list_containers_snapshot() {
    let list: Vec<_> = items(&fixture("containers", "list"))
        .iter()
        .map(docker_json::container_summary)
        .collect();
    assert_eq!(list.len(), 3);
    let web = list.iter().find(|c| c.name == "dk-fx-web").unwrap();
    assert_eq!(web.state, ContainerState::Running);
    assert_eq!(web.health, Some(Health::Healthy));
    assert_eq!(web.compose.as_ref().unwrap().project, "dk-fx");
    assert_eq!(web.compose.as_ref().unwrap().depends_on, vec!["db"]);
    assert!(
        web.ports
            .iter()
            .any(|p| p.private == 80 && p.public.is_some())
    );
    assert!(web.networks.contains(&"dk-fx-net".to_string()));
    assert!(!web.ip_addresses.is_empty());
    assert_eq!(web.mounts[0].kind, MountKind::Volume);
    assert_eq!(web.mounts[0].source, "dk-fx-data");
    assert!(web.size_rw.is_some());
    let exited = list.iter().find(|c| c.name == "dk-fx-exited").unwrap();
    assert_eq!(exited.state, ContainerState::Exited);
    assert_eq!(exited.exit_code, Some(3));

    let mut v = serde_json::to_value(&list).unwrap();
    for c in v.as_array_mut().unwrap() {
        stable_status(c);
    }
    insta::assert_json_snapshot!("containers_list", v);
}

#[test]
fn cdt_inspect_containers_snapshot() {
    for name in ["web", "tty", "exited"] {
        let raw = fixture("containers", &format!("inspect_{name}"));
        let d = docker_json::container_details(&raw).unwrap();
        assert_eq!(d.raw, raw);
        match name {
            "web" => {
                assert_eq!(d.summary.health, Some(Health::Healthy));
                assert_eq!(d.restart_policy.as_deref(), Some("unless-stopped"));
                assert_eq!(d.resources.nano_cpus, Some(500_000_000));
                assert_eq!(d.resources.memory, Some(256 * 1024 * 1024));
                assert_eq!(d.working_dir.as_deref(), Some("/srv"));
                assert!(
                    d.env
                        .iter()
                        .any(|e| e.key == "DB_PASSWORD" && e.is_sensitive())
                );
                assert!(!d.health_log.is_empty());
                assert!(d.pid.is_some());
                let net = d
                    .network_settings
                    .networks
                    .iter()
                    .find(|n| n.network == "dk-fx-net")
                    .unwrap();
                assert!(net.aliases.contains(&"web".to_string()));
                assert!(net.ipv4.is_some());
            }
            "tty" => assert!(d.tty),
            "exited" => {
                assert_eq!(d.summary.exit_code, Some(3));
                assert!(d.finished_at.is_some());
                assert_eq!(d.pid, None);
                assert!(d.summary.status_text.starts_with("Exited (3)"));
            }
            _ => unreachable!(),
        }
        let mut v = serde_json::to_value(&d).unwrap();
        v.as_object_mut().unwrap().remove("raw");
        stable_status(v.get_mut("summary").unwrap());
        insta::assert_json_snapshot!(format!("containers_inspect_{name}"), v);
    }
}

#[test]
fn con_top_fixture_shape() {
    let top = fixture("containers", "top_web");
    assert!(top["Titles"].as_array().unwrap().iter().any(|t| t == "PID"));
    assert!(!top["Processes"].as_array().unwrap().is_empty());
}

#[test]
fn sta_002_stats_stream_normalises() {
    let samples = items(&fixture("stats", "web_stream"));
    assert_eq!(samples.len(), 2);
    let mut norm = StatsNormalizer::new();
    let now = time::OffsetDateTime::now_utc();
    let out: Vec<_> = samples
        .iter()
        .map(|s| norm.push(RawStats::from_docker_json(s).unwrap(), now))
        .collect();
    assert!(out[1].at > out[0].at);
    assert!(out[0].online_cpus >= 1);
    assert!(out[1].mem_used > 0);
    assert!(out[1].mem_limit > 0);
    assert!(out[1].cpu_percent >= 0.0);
    assert!(out[1].pids.is_some());
}

#[test]
fn img_images_snapshot() {
    let list: Vec<_> = items(&fixture("images", "list"))
        .iter()
        .map(docker_json::image_summary)
        .collect();
    assert_eq!(list.len(), 1);
    assert!(!list[0].dangling);
    assert!(list[0].repo_tags.contains(&"alpine:3.20".to_string()));
    insta::assert_json_snapshot!("images_list", list);

    let d = docker_json::image_details(&fixture("images", "inspect_alpine")).unwrap();
    assert!(!d.architecture.is_empty());
    assert!(!d.root_fs_layers.is_empty());
    let mut v = serde_json::to_value(&d).unwrap();
    v.as_object_mut().unwrap().remove("raw");
    insta::assert_json_snapshot!("images_inspect_alpine", v);

    let history: Vec<_> = items(&fixture("images", "history_alpine"))
        .iter()
        .map(docker_json::image_layer)
        .collect();
    assert!(!history.is_empty());
    insta::assert_json_snapshot!("images_history_alpine", history);
}

#[test]
fn img_004_pull_progress_fixture_shape() {
    let msgs = items(&fixture("images", "pull_progress"));
    assert!(msgs.iter().any(|m| {
        m["status"]
            .as_str()
            .is_some_and(|s| s.starts_with("Digest: sha256:"))
    }));
}

#[test]
fn vol_volumes_snapshot() {
    let list: Vec<_> = items(&fixture("volumes", "list")["Volumes"])
        .iter()
        .map(docker_json::volume_summary)
        .collect();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].compose.as_ref().unwrap().project, "dk-fx");
    insta::assert_json_snapshot!("volumes_list", list);

    let v = docker_json::volume_summary(&fixture("volumes", "inspect_data"));
    assert_eq!(v.name, "dk-fx-data");
    assert!(v.created.is_some());

    let containers: Vec<_> = items(&fixture("containers", "list"))
        .iter()
        .map(docker_json::container_summary)
        .collect();
    let used_by = docker_json::volume_used_by("dk-fx-data", &containers);
    assert_eq!(used_by.len(), 1);
    assert_eq!(used_by[0].name, "dk-fx-web");
    assert_eq!(used_by[0].detail.as_deref(), Some("/data"));
}

#[test]
fn net_networks_snapshot() {
    let list: Vec<_> = items(&fixture("networks", "list"))
        .iter()
        .map(docker_json::network_summary)
        .collect();
    assert_eq!(list.len(), 1);
    assert!(list[0].attachable);
    assert!(!list[0].subnets.is_empty());
    insta::assert_json_snapshot!("networks_list", list);

    let d = docker_json::network_details(&fixture("networks", "inspect_net")).unwrap();
    assert!(d.containers.iter().any(|c| c.name == "dk-fx-web"));
    let mut v = serde_json::to_value(&d).unwrap();
    v.as_object_mut().unwrap().remove("raw");
    insta::assert_json_snapshot!("networks_inspect_net", v);
}

#[test]
fn eng_events_snapshot() {
    let raw = items(&fixture("events", "recorded"));
    let events: Vec<_> = raw.iter().filter_map(docker_json::engine_event).collect();
    assert!(!events.is_empty());
    assert!(
        events
            .iter()
            .any(|e| e.kind == ResourceKind::Container && e.action == "start")
    );
    assert!(events.iter().any(|e| e.kind == ResourceKind::Network));
    assert!(events.iter().all(|e| !e.id.is_empty()));
    let summary: Vec<_> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "kind": e.kind,
                "action": e.action,
                "name": e.attributes.get("name"),
            })
        })
        .collect();
    insta::assert_json_snapshot!("events_recorded", summary);
}

#[test]
fn vol_002_disk_usage_snapshot() {
    let du = docker_json::disk_usage(&fixture("system", "df"));
    assert!(du.images_size > 0);
    assert!(
        du.volume_refs
            .iter()
            .any(|(n, r)| n == "dk-fx-data" && *r == 1)
    );
    insta::assert_json_snapshot!("system_df", du);
}

#[test]
fn log_008_console_stream_label() {
    // TTY containers map to `LogStream::Console` (the engine decides from the frame type).
    assert_eq!(
        serde_json::to_value(LogStream::Console).unwrap(),
        Value::String("console".into())
    );
}

#[test]
fn nfr_031_fixtures_survive_field_mangling() {
    // Drop or corrupt every top-level field of each inspect fixture: must never panic.
    for (op, name) in [
        ("containers", "inspect_web"),
        ("images", "inspect_alpine"),
        ("networks", "inspect_net"),
    ] {
        let raw = fixture(op, name);
        let obj = raw.as_object().unwrap();
        for key in obj.keys() {
            for replacement in [
                Value::Null,
                Value::from(42),
                Value::from("x"),
                Value::Array(vec![]),
            ] {
                let mut v = raw.clone();
                v[key] = replacement;
                let _ = docker_json::container_details(&v);
                let _ = docker_json::image_details(&v);
                let _ = docker_json::network_details(&v);
                let _ = docker_json::container_summary(&v);
                let _ = docker_json::volume_summary(&v);
            }
        }
    }
}
