//! Mappers from Docker-Engine-API-shaped JSON (`serde_json::Value`) to DTOs. Shared by
//! `dk-engine-docker` and `dk-engine-wslc` (WSLC COM/CLI return Docker-shaped JSON for
//! inspect, stats, volumes, and networks). Tolerant (NFR-031): never panic; missing or
//! unknown fields map to defaults / `Unknown`.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{EngineError, EngineResult};
use crate::format::format_duration;
use crate::grouping::compose_info_from_labels;
use crate::model::*;

// ───────────────────────────── small tolerant accessors ─────────────────────────────

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| !x.is_null())
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    get(v, key).and_then(Value::as_str)
}

/// String field, `""` when missing.
fn string(v: &Value, key: &str) -> String {
    str_of(v, key).unwrap_or_default().to_owned()
}

/// Non-empty string field.
fn opt_string(v: &Value, key: &str) -> Option<String> {
    str_of(v, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Integer from a number (int or float) or a numeric string.
fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX)))
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn i64_of(v: &Value, key: &str) -> Option<i64> {
    get(v, key).and_then(as_i64)
}

/// Non-negative size; `-1` (not computed) and other negatives → `None`.
fn size_of(v: &Value, key: &str) -> Option<u64> {
    i64_of(v, key).and_then(|n| u64::try_from(n).ok())
}

fn bool_of(v: &Value, key: &str) -> bool {
    match get(v, key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    }
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                other => Some(other.to_string()),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `Cmd` / `Entrypoint`: a string, an array of strings, or null.
fn str_or_array(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        other => str_array(other),
    }
}

/// Keys of an object (e.g. `ExposedPorts`, `Volumes`), or the items of an array.
fn keys_of(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        other => str_array(other),
    }
}

/// String map (labels, options). Non-string values are stringified.
fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    match v {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, val)| {
                let s = match val {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s.trim();
    // Inspect-style `172.17.0.2/16` or bracketed v6.
    let s = s.split('/').next().unwrap_or(s);
    let s = s.trim_start_matches('[').trim_end_matches(']');
    if s.is_empty() { None } else { s.parse().ok() }
}

fn parse_proto(s: &str) -> Proto {
    match s.trim().to_ascii_lowercase().as_str() {
        "udp" => Proto::Udp,
        "sctp" => Proto::Sctp,
        _ => Proto::Tcp,
    }
}

fn parse_mount_kind(s: &str) -> MountKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "volume" => MountKind::Volume,
        "bind" => MountKind::Bind,
        "tmpfs" => MountKind::Tmpfs,
        "npipe" => MountKind::Npipe,
        _ => MountKind::Unknown,
    }
}

fn epoch() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

/// Docker's zero time (`0001-01-01T00:00:00Z`) and anything before 1970 → `None`.
fn not_zero(t: OffsetDateTime) -> Option<OffsetDateTime> {
    (t.year() > 1).then_some(t)
}

/// RFC 3339 with nanoseconds (`2026-10-02T12:00:00.123456789Z`), unix seconds as number,
/// or the zero time `0001-01-01T00:00:00Z` (→ `None`).
pub fn parse_time(v: &Value) -> Option<OffsetDateTime> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
                return not_zero(t.to_offset(time::UtcOffset::UTC));
            }
            // Some engines print a space instead of `T`, or a numeric string.
            if let Ok(t) = OffsetDateTime::parse(&s.replacen(' ', "T", 1), &Rfc3339) {
                return not_zero(t.to_offset(time::UtcOffset::UTC));
            }
            s.parse::<f64>().ok().and_then(unix_float)
        }
        Value::Number(n) => match n.as_i64() {
            Some(i) => unix_int(i),
            None => n.as_f64().and_then(unix_float),
        },
        _ => None,
    }
}

fn unix_int(i: i64) -> Option<OffsetDateTime> {
    if i <= 0 {
        return None;
    }
    // Seconds; values too large for seconds are treated as nanoseconds.
    OffsetDateTime::from_unix_timestamp(i)
        .ok()
        .or_else(|| OffsetDateTime::from_unix_timestamp_nanos(i128::from(i)).ok())
}

fn unix_float(f: f64) -> Option<OffsetDateTime> {
    if !f.is_finite() || f <= 0.0 {
        return None;
    }
    if f.fract() == 0.0 && f < i64::MAX as f64 {
        return unix_int(f as i64);
    }
    OffsetDateTime::from_unix_timestamp_nanos((f * 1e9) as i128).ok()
}

fn time_of(v: &Value, key: &str) -> Option<OffsetDateTime> {
    get(v, key).and_then(parse_time)
}

fn strip_slash(s: &str) -> String {
    s.strip_prefix('/').unwrap_or(s).to_owned()
}

fn expect_object(v: &Value, what: &str) -> EngineResult<()> {
    if v.is_object() {
        Ok(())
    } else {
        Err(EngineError::protocol(format!(
            "{what}: expected a JSON object"
        )))
    }
}

// ───────────────────────────── containers ─────────────────────────────

/// Health from the list `Status` text: `Up 3 minutes (healthy)`, `(health: starting)`.
fn health_from_status(status: &str) -> Option<Health> {
    let lower = status.to_ascii_lowercase();
    if lower.contains("(unhealthy)") {
        Some(Health::Unhealthy)
    } else if lower.contains("(healthy)") {
        Some(Health::Healthy)
    } else if lower.contains("(health: starting)") {
        Some(Health::Starting)
    } else {
        None
    }
}

/// Exit code from `Exited (137) 2 hours ago`.
fn exit_code_from_status(status: &str) -> Option<i64> {
    let lower = status.to_ascii_lowercase();
    let start = lower.find("exited (")? + "exited (".len();
    let rest = &status[start..];
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

fn health_from_object(v: Option<&Value>) -> Option<Health> {
    v.and_then(|h| str_of(h, "Status")).and_then(Health::parse)
}

fn mount_summary(m: &Value) -> MountSummary {
    let kind = parse_mount_kind(str_of(m, "Type").unwrap_or_default());
    let source = match kind {
        MountKind::Volume => opt_string(m, "Name").unwrap_or_else(|| string(m, "Source")),
        _ => opt_string(m, "Source").unwrap_or_else(|| string(m, "Name")),
    };
    MountSummary {
        kind,
        source,
        destination: string(m, "Destination"),
        rw: get(m, "RW").and_then(Value::as_bool).unwrap_or(true),
    }
}

fn mounts_summary(v: Option<&Value>) -> Vec<MountSummary> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|m| m.is_object())
                .map(mount_summary)
                .collect()
        })
        .unwrap_or_default()
}

/// `(network names, ip addresses)` from `NetworkSettings.Networks`.
fn networks_and_ips(network_settings: Option<&Value>) -> (Vec<String>, Vec<IpAddr>) {
    let mut names = Vec::new();
    let mut ips = Vec::new();
    if let Some(nets) = network_settings
        .and_then(|ns| get(ns, "Networks"))
        .and_then(Value::as_object)
    {
        for (name, n) in nets {
            names.push(name.clone());
            for key in ["IPAddress", "GlobalIPv6Address"] {
                if let Some(ip) = str_of(n, key).and_then(parse_ip)
                    && !ips.contains(&ip)
                {
                    ips.push(ip);
                }
            }
        }
    }
    (names, ips)
}

fn list_port(p: &Value) -> Option<PortMapping> {
    let private = u16::try_from(i64_of(p, "PrivatePort")?).ok()?;
    let public = i64_of(p, "PublicPort")
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| *n != 0);
    Some(PortMapping {
        ip: str_of(p, "IP").and_then(parse_ip),
        private,
        public,
        proto: parse_proto(str_of(p, "Type").unwrap_or("tcp")),
    })
}

/// `"80/tcp"` → `(80, Tcp)`.
fn parse_port_key(key: &str) -> Option<(u16, Proto)> {
    let (port, proto) = key.split_once('/').unwrap_or((key, "tcp"));
    Some((port.trim().parse().ok()?, parse_proto(proto)))
}

/// `NetworkSettings.Ports` / `HostConfig.PortBindings`:
/// `{"80/tcp": [{"HostIp": "0.0.0.0", "HostPort": "8080"}], "443/tcp": null}`.
fn port_map(v: Option<&Value>) -> Vec<PortMapping> {
    let mut out = Vec::new();
    let Some(map) = v.and_then(Value::as_object) else {
        return out;
    };
    for (key, bindings) in map {
        let Some((private, proto)) = parse_port_key(key) else {
            continue;
        };
        match bindings.as_array() {
            Some(list) if !list.is_empty() => {
                for b in list {
                    out.push(PortMapping {
                        ip: str_of(b, "HostIp").and_then(parse_ip),
                        private,
                        public: str_of(b, "HostPort")
                            .and_then(|s| s.trim().parse::<u16>().ok())
                            .filter(|p| *p != 0),
                        proto,
                    });
                }
            }
            _ => out.push(PortMapping {
                ip: None,
                private,
                public: None,
                proto,
            }),
        }
    }
    out
}

/// One item of `GET /containers/json`.
pub fn container_summary(v: &Value) -> ContainerSummary {
    let id = string(v, "Id");
    let name = get(v, "Names")
        .and_then(Value::as_array)
        .and_then(|names| names.iter().find_map(Value::as_str))
        .map(strip_slash)
        .or_else(|| str_of(v, "Name").map(strip_slash))
        .unwrap_or_else(|| crate::format::short_id(&id).to_owned());
    let status_text = string(v, "Status");
    let state = match get(v, "State") {
        Some(Value::String(s)) => ContainerState::parse(s),
        Some(obj @ Value::Object(_)) => ContainerState::parse(str_of(obj, "Status").unwrap_or("")),
        _ => ContainerState::parse(status_text.split_whitespace().next().unwrap_or("")),
    };
    let health = health_from_object(get(v, "Health")).or_else(|| health_from_status(&status_text));
    let exit_code = exit_code_from_status(&status_text);
    let labels = str_map(get(v, "Labels"));
    let (networks, ip_addresses) = networks_and_ips(get(v, "NetworkSettings"));
    let ports = get(v, "Ports")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(list_port).collect())
        .unwrap_or_default();
    let compose = compose_info_from_labels(&labels);
    ContainerSummary {
        id,
        name,
        image: string(v, "Image"),
        image_id: string(v, "ImageID"),
        command: string(v, "Command"),
        created: time_of(v, "Created").unwrap_or_else(epoch),
        state,
        status_text,
        health,
        exit_code,
        ports,
        labels,
        networks,
        ip_addresses,
        mounts: mounts_summary(get(v, "Mounts")),
        size_rw: size_of(v, "SizeRw"),
        size_root_fs: size_of(v, "SizeRootFs"),
        compose,
    }
}

/// Docker-like status text from an inspect `State` (`Up 2 hours (healthy)`,
/// `Exited (0) 3 minutes ago`, `Created`).
fn status_text_from_state(state: &Value, now: OffsetDateTime) -> String {
    let status = ContainerState::parse(str_of(state, "Status").unwrap_or_default());
    let started = time_of(state, "StartedAt");
    let finished = time_of(state, "FinishedAt");
    let exit_code = i64_of(state, "ExitCode").unwrap_or(0);
    let since = |t: Option<OffsetDateTime>| t.map(|t| format_duration(now - t));
    let health = get(state, "Health")
        .and_then(|h| str_of(h, "Status"))
        .and_then(Health::parse);
    match status {
        ContainerState::Running | ContainerState::Paused => {
            let mut s = match since(started) {
                Some(d) => format!("Up {d}"),
                None => "Up".to_owned(),
            };
            if status == ContainerState::Paused {
                s.push_str(" (Paused)");
            } else if let Some(h) = health {
                match h {
                    Health::Starting => s.push_str(" (health: starting)"),
                    other => {
                        s.push_str(" (");
                        s.push_str(other.label());
                        s.push(')');
                    }
                }
            }
            s
        }
        ContainerState::Restarting => match since(finished) {
            Some(d) => format!("Restarting ({exit_code}) {d} ago"),
            None => format!("Restarting ({exit_code})"),
        },
        ContainerState::Removing => "Removal In Progress".to_owned(),
        ContainerState::Dead => "Dead".to_owned(),
        ContainerState::Created => "Created".to_owned(),
        ContainerState::Exited => match (started, since(finished)) {
            (None, _) => "Created".to_owned(),
            (Some(_), Some(d)) => format!("Exited ({exit_code}) {d} ago"),
            (Some(_), None) => format!("Exited ({exit_code})"),
        },
        ContainerState::Unknown => string(state, "Status"),
    }
}

fn mount_detail(m: &Value) -> MountDetail {
    MountDetail {
        kind: parse_mount_kind(str_of(m, "Type").unwrap_or_default()),
        source: string(m, "Source"),
        destination: string(m, "Destination"),
        mode: string(m, "Mode"),
        rw: get(m, "RW").and_then(Value::as_bool).unwrap_or(true),
        propagation: opt_string(m, "Propagation"),
        volume_name: opt_string(m, "Name"),
    }
}

fn network_attachment(name: &str, n: &Value) -> NetworkAttachment {
    NetworkAttachment {
        network: name.to_owned(),
        network_id: opt_string(n, "NetworkID"),
        ipv4: opt_string(n, "IPAddress"),
        ipv6: opt_string(n, "GlobalIPv6Address"),
        gateway: opt_string(n, "Gateway"),
        mac: opt_string(n, "MacAddress"),
        aliases: str_array(get(n, "Aliases")),
    }
}

fn non_zero(v: &Value, key: &str) -> Option<i64> {
    i64_of(v, key).filter(|n| *n != 0)
}

fn env_var(s: &str) -> EnvVar {
    match s.split_once('=') {
        Some((k, val)) => EnvVar {
            key: k.to_owned(),
            value: val.to_owned(),
        },
        None => EnvVar {
            key: s.to_owned(),
            value: String::new(),
        },
    }
}

/// `GET /containers/{id}/json`. `raw` = `v` itself.
pub fn container_details(v: &Value) -> EngineResult<ContainerDetails> {
    container_details_at(v, OffsetDateTime::now_utc())
}

fn container_details_at(v: &Value, now: OffsetDateTime) -> EngineResult<ContainerDetails> {
    expect_object(v, "container inspect")?;
    let null = Value::Null;
    let state = get(v, "State").unwrap_or(&null);
    let config = get(v, "Config").unwrap_or(&null);
    let host_config = get(v, "HostConfig").unwrap_or(&null);
    let network_settings = get(v, "NetworkSettings");

    let id = string(v, "Id");
    let labels = str_map(get(config, "Labels"));
    let (networks, ip_addresses) = networks_and_ips(network_settings);
    let mut port_bindings = port_map(network_settings.and_then(|ns| get(ns, "Ports")));
    if port_bindings.is_empty() {
        port_bindings = port_map(get(host_config, "PortBindings"));
    }
    let container_state = ContainerState::parse(str_of(state, "Status").unwrap_or_default());
    let exit_code = matches!(
        container_state,
        ContainerState::Exited | ContainerState::Dead
    )
    .then(|| i64_of(state, "ExitCode"))
    .flatten();
    let path = string(v, "Path");
    let args = str_array(get(v, "Args"));
    let command = std::iter::once(path)
        .chain(args)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let compose = compose_info_from_labels(&labels);

    let summary = ContainerSummary {
        name: str_of(v, "Name")
            .map(strip_slash)
            .unwrap_or_else(|| crate::format::short_id(&id).to_owned()),
        id,
        image: opt_string(config, "Image").unwrap_or_else(|| string(v, "Image")),
        image_id: string(v, "Image"),
        command,
        created: time_of(v, "Created").unwrap_or_else(epoch),
        state: container_state,
        status_text: status_text_from_state(state, now),
        health: health_from_object(get(state, "Health")),
        exit_code,
        ports: port_bindings.clone(),
        labels,
        networks,
        ip_addresses,
        mounts: mounts_summary(get(v, "Mounts")),
        size_rw: size_of(v, "SizeRw"),
        size_root_fs: size_of(v, "SizeRootFs"),
        compose,
    };

    let network_attachments = network_settings
        .and_then(|ns| get(ns, "Networks"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(name, n)| network_attachment(name, n))
                .collect()
        })
        .unwrap_or_default();

    let health_log = get(state, "Health")
        .and_then(|h| get(h, "Log"))
        .and_then(Value::as_array)
        .map(|log| {
            log.iter()
                .map(|e| HealthCheckResult {
                    start: time_of(e, "Start"),
                    end: time_of(e, "End"),
                    exit_code: i64_of(e, "ExitCode").unwrap_or(0),
                    output: string(e, "Output"),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ContainerDetails {
        summary,
        started_at: time_of(state, "StartedAt"),
        finished_at: time_of(state, "FinishedAt"),
        restart_count: i64_of(v, "RestartCount")
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        restart_policy: get(host_config, "RestartPolicy").and_then(|p| opt_string(p, "Name")),
        pid: i64_of(state, "Pid")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n != 0),
        platform: opt_string(v, "Platform"),
        entrypoint: str_or_array(get(config, "Entrypoint")),
        cmd: str_or_array(get(config, "Cmd")),
        working_dir: opt_string(config, "WorkingDir"),
        user: opt_string(config, "User"),
        hostname: opt_string(config, "Hostname"),
        tty: bool_of(config, "Tty"),
        env: str_array(get(config, "Env"))
            .iter()
            .map(|s| env_var(s))
            .collect(),
        mounts: get(v, "Mounts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|m| m.is_object())
                    .map(mount_detail)
                    .collect()
            })
            .unwrap_or_default(),
        network_settings: ContainerNetworking {
            network_mode: opt_string(host_config, "NetworkMode"),
            dns: str_array(get(host_config, "Dns")),
            networks: network_attachments,
        },
        port_bindings,
        resources: ResourceLimits {
            nano_cpus: non_zero(host_config, "NanoCpus"),
            memory: non_zero(host_config, "Memory"),
            memory_swap: non_zero(host_config, "MemorySwap"),
            pids_limit: non_zero(host_config, "PidsLimit"),
            cpu_shares: non_zero(host_config, "CpuShares"),
        },
        health_log,
        raw: v.clone(),
    })
}

// ───────────────────────────── images ─────────────────────────────

fn is_none_ref(s: &str) -> bool {
    s.is_empty() || s == "<none>:<none>" || s == "<none>@<none>"
}

/// One item of `GET /images/json`.
pub fn image_summary(v: &Value) -> ImageSummary {
    let raw_tags = str_array(get(v, "RepoTags"));
    let dangling = raw_tags.iter().all(|t| is_none_ref(t));
    ImageSummary {
        id: string(v, "Id"),
        repo_tags: raw_tags.into_iter().filter(|t| !is_none_ref(t)).collect(),
        repo_digests: str_array(get(v, "RepoDigests"))
            .into_iter()
            .filter(|d| !is_none_ref(d))
            .collect(),
        created: time_of(v, "Created").unwrap_or_else(epoch),
        size: size_of(v, "Size").unwrap_or(0),
        shared_size: size_of(v, "SharedSize"),
        containers: i64_of(v, "Containers").and_then(|n| u32::try_from(n).ok()),
        labels: str_map(get(v, "Labels")),
        dangling,
    }
}

/// `GET /images/{id}/json`.
pub fn image_details(v: &Value) -> EngineResult<ImageDetails> {
    expect_object(v, "image inspect")?;
    let null = Value::Null;
    let config = get(v, "Config").unwrap_or(&null);
    let raw_tags = str_array(get(v, "RepoTags"));
    let dangling = raw_tags.iter().all(|t| is_none_ref(t));
    let labels = str_map(get(config, "Labels"));
    let summary = ImageSummary {
        id: string(v, "Id"),
        repo_tags: raw_tags.into_iter().filter(|t| !is_none_ref(t)).collect(),
        repo_digests: str_array(get(v, "RepoDigests"))
            .into_iter()
            .filter(|d| !is_none_ref(d))
            .collect(),
        created: time_of(v, "Created").unwrap_or_else(epoch),
        size: size_of(v, "Size").unwrap_or(0),
        shared_size: None,
        containers: None,
        labels: labels.clone(),
        dangling,
    };
    Ok(ImageDetails {
        summary,
        architecture: string(v, "Architecture"),
        os: string(v, "Os"),
        variant: opt_string(v, "Variant"),
        author: opt_string(v, "Author"),
        config: ImageConfig {
            env: str_array(get(config, "Env")),
            cmd: str_or_array(get(config, "Cmd")),
            entrypoint: str_or_array(get(config, "Entrypoint")),
            exposed_ports: keys_of(get(config, "ExposedPorts")),
            working_dir: opt_string(config, "WorkingDir"),
            user: opt_string(config, "User"),
            volumes: keys_of(get(config, "Volumes")),
            labels,
        },
        root_fs_layers: str_array(get(v, "RootFS").and_then(|r| get(r, "Layers"))),
        raw: v.clone(),
    })
}

/// One item of `GET /images/{id}/history`.
pub fn image_layer(v: &Value) -> ImageLayer {
    ImageLayer {
        id: opt_string(v, "Id").filter(|id| id != "<missing>"),
        created: time_of(v, "Created").unwrap_or_else(epoch),
        created_by: string(v, "CreatedBy"),
        size: size_of(v, "Size").unwrap_or(0),
        comment: string(v, "Comment"),
    }
}

// ───────────────────────────── volumes / networks ─────────────────────────────

/// One item of `GET /volumes` `.Volumes[]` (also `GET /volumes/{name}`).
pub fn volume_summary(v: &Value) -> VolumeSummary {
    let labels = str_map(get(v, "Labels"));
    let usage = get(v, "UsageData");
    VolumeSummary {
        name: string(v, "Name"),
        driver: string(v, "Driver"),
        mountpoint: string(v, "Mountpoint"),
        created: time_of(v, "CreatedAt"),
        scope: string(v, "Scope"),
        size: usage.and_then(|u| size_of(u, "Size")),
        ref_count: usage
            .and_then(|u| i64_of(u, "RefCount"))
            .filter(|n| *n >= 0),
        compose: compose_info_from_labels(&labels),
        labels,
    }
}

/// One item of `GET /networks` (also `GET /networks/{id}`).
pub fn network_summary(v: &Value) -> NetworkSummary {
    let labels = str_map(get(v, "Labels"));
    let subnets = get(v, "IPAM")
        .and_then(|ipam| get(ipam, "Config"))
        .and_then(Value::as_array)
        .map(|cfgs| {
            cfgs.iter()
                .map(|c| IpamConfig {
                    subnet: opt_string(c, "Subnet"),
                    gateway: opt_string(c, "Gateway"),
                    ip_range: opt_string(c, "IPRange"),
                })
                .collect()
        })
        .unwrap_or_default();
    NetworkSummary {
        id: string(v, "Id"),
        name: string(v, "Name"),
        driver: string(v, "Driver"),
        scope: string(v, "Scope"),
        internal: bool_of(v, "Internal"),
        attachable: bool_of(v, "Attachable"),
        ipv6: bool_of(v, "EnableIPv6"),
        created: time_of(v, "Created").unwrap_or_else(epoch),
        subnets,
        compose: compose_info_from_labels(&labels),
        labels,
        containers: get(v, "Containers")
            .and_then(Value::as_object)
            .map(|m| u32::try_from(m.len()).unwrap_or(u32::MAX)),
    }
}

/// `GET /networks/{id}`.
pub fn network_details(v: &Value) -> EngineResult<NetworkDetails> {
    expect_object(v, "network inspect")?;
    let containers = get(v, "Containers")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(id, c)| NetworkEndpoint {
                    name: string(c, "Name"),
                    id: id.clone(),
                    ipv4: opt_string(c, "IPv4Address"),
                    ipv6: opt_string(c, "IPv6Address"),
                    mac: opt_string(c, "MacAddress"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(NetworkDetails {
        summary: network_summary(v),
        containers,
        options: str_map(get(v, "Options")),
        raw: v.clone(),
    })
}

// ───────────────────────────── events / disk usage ─────────────────────────────

fn resource_kind(s: &str) -> Option<ResourceKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "container" => Some(ResourceKind::Container),
        "image" => Some(ResourceKind::Image),
        "volume" => Some(ResourceKind::Volume),
        "network" => Some(ResourceKind::Network),
        "daemon" => Some(ResourceKind::Daemon),
        _ => None,
    }
}

/// One message of `GET /events`. Unknown `Type` → `None`.
pub fn engine_event(v: &Value) -> Option<EngineEvent> {
    if !v.is_object() {
        return None;
    }
    // Pre-1.22 events have no `Type`; they are container events with `status`/`id`.
    let kind = match str_of(v, "Type") {
        Some(t) => resource_kind(t)?,
        None if get(v, "status").is_some() => ResourceKind::Container,
        None => return None,
    };
    let actor = get(v, "Actor");
    let at = i64_of(v, "timeNano")
        .filter(|n| *n > 0)
        .and_then(|n| OffsetDateTime::from_unix_timestamp_nanos(i128::from(n)).ok())
        .or_else(|| time_of(v, "time"))
        .unwrap_or_else(OffsetDateTime::now_utc);
    Some(EngineEvent {
        at,
        kind,
        action: opt_string(v, "Action")
            .or_else(|| opt_string(v, "status"))
            .unwrap_or_default(),
        id: actor
            .and_then(|a| opt_string(a, "ID"))
            .or_else(|| opt_string(v, "id"))
            .unwrap_or_default(),
        attributes: str_map(actor.and_then(|a| get(a, "Attributes"))),
    })
}

fn sum_sizes<'a>(items: impl Iterator<Item = &'a Value>, key: &str) -> u64 {
    items
        .filter_map(|i| size_of(i, key))
        .fold(0u64, u64::saturating_add)
}

fn array<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    get(v, key).and_then(Value::as_array)
}

/// `GET /system/df`. Handles the legacy shape (`LayersSize`, `Images`, `Containers`,
/// `Volumes`, `BuildCache`) and the API ≥ 1.52 shape (`ImageUsage`, `ContainerUsage`,
/// `VolumeUsage`, `BuildCacheUsage` with `TotalSize`/`Reclaimable`/`Items`).
pub fn disk_usage(v: &Value) -> DiskUsage {
    let mut du = DiskUsage::default();
    let image_usage = get(v, "ImageUsage");
    let container_usage = get(v, "ContainerUsage");
    let volume_usage = get(v, "VolumeUsage");
    let build_usage = get(v, "BuildCacheUsage");

    // Images: `LayersSize` counts shared layers once (what `docker system df` shows).
    let images = array(v, "Images").or_else(|| image_usage.and_then(|u| array(u, "Items")));
    du.images_size = size_of(v, "LayersSize")
        .or_else(|| image_usage.and_then(|u| size_of(u, "TotalSize")))
        .or_else(|| images.map(|items| sum_sizes(items.iter(), "Size")))
        .unwrap_or(0);
    du.images_reclaimable = image_usage
        .and_then(|u| size_of(u, "Reclaimable"))
        .or_else(|| {
            images.map(|items| {
                sum_sizes(
                    items.iter().filter(|i| i64_of(i, "Containers") == Some(0)),
                    "Size",
                )
            })
        });

    let containers =
        array(v, "Containers").or_else(|| container_usage.and_then(|u| array(u, "Items")));
    du.containers_size = containers
        .map(|items| sum_sizes(items.iter(), "SizeRw"))
        .or_else(|| container_usage.and_then(|u| size_of(u, "TotalSize")))
        .unwrap_or(0);

    let volumes = array(v, "Volumes").or_else(|| volume_usage.and_then(|u| array(u, "Items")));
    for vol in volumes.into_iter().flatten() {
        let name = string(vol, "Name");
        if name.is_empty() {
            continue;
        }
        let usage = get(vol, "UsageData");
        if let Some(size) = usage.and_then(|u| size_of(u, "Size")) {
            du.volumes.push((name.clone(), size));
        }
        if let Some(refs) = usage
            .and_then(|u| i64_of(u, "RefCount"))
            .filter(|n| *n >= 0)
        {
            du.volume_refs.push((name, refs));
        }
    }

    du.build_cache = array(v, "BuildCache")
        .map(|items| sum_sizes(items.iter(), "Size"))
        .or_else(|| build_usage.and_then(|u| size_of(u, "TotalSize")))
        .or_else(|| {
            build_usage
                .and_then(|u| array(u, "Items"))
                .map(|items| sum_sizes(items.iter(), "Size"))
        })
        .unwrap_or(0);
    du
}

/// Containers whose mounts reference `volume` (VOL-010 "Used by").
pub fn volume_used_by(volume: &str, containers: &[ContainerSummary]) -> Vec<ContainerRef> {
    let mut out = Vec::new();
    for c in containers {
        for m in &c.mounts {
            if m.kind == MountKind::Volume && m.source == volume {
                out.push(ContainerRef {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    detail: Some(m.destination.clone()),
                    rw: Some(m.rw),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn nfr_031_parse_time_variants() {
        assert_eq!(
            parse_time(&json!("2026-10-02T12:00:00.123456789Z")),
            Some(datetime!(2026-10-02 12:00:00.123456789 UTC))
        );
        assert_eq!(
            parse_time(&json!("2026-10-02T14:00:00+02:00")),
            Some(datetime!(2026-10-02 12:00:00 UTC))
        );
        assert_eq!(
            parse_time(&json!(1_759_406_400)),
            Some(datetime!(2025-10-02 12:00:00 UTC))
        );
        assert_eq!(
            parse_time(&json!(1_759_406_400_000_000_000_i64)),
            Some(datetime!(2025-10-02 12:00:00 UTC))
        );
        assert_eq!(parse_time(&json!("0001-01-01T00:00:00Z")), None);
        assert_eq!(parse_time(&json!("")), None);
        assert_eq!(parse_time(&json!(0)), None);
        assert_eq!(parse_time(&json!("garbage")), None);
        assert_eq!(parse_time(&json!(null)), None);
        assert_eq!(parse_time(&json!({"a": 1})), None);
    }

    fn list_item() -> Value {
        json!({
            "Id": "8dfafdbc3a40",
            "Names": ["/shop-web-1"],
            "Image": "nginx:1.27",
            "ImageID": "sha256:abc",
            "Command": "nginx -g 'daemon off;'",
            "Created": 1_759_406_400,
            "State": "running",
            "Status": "Up 3 minutes (healthy)",
            "Ports": [
                {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"IP": "::", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"PrivatePort": 53, "Type": "udp"}
            ],
            "Labels": {
                "com.docker.compose.project": "shop",
                "com.docker.compose.service": "web",
                "com.docker.compose.container-number": "1",
                "com.docker.compose.project.config_files": "/srv/shop/compose.yaml,/srv/shop/override.yaml",
                "com.docker.compose.depends_on": "db:service_started:false,cache:service_healthy:true"
            },
            "SizeRw": 12,
            "SizeRootFs": -1,
            "NetworkSettings": {"Networks": {"shop_default": {"IPAddress": "172.18.0.3", "GlobalIPv6Address": ""}}},
            "Mounts": [
                {"Type": "volume", "Name": "shop_data", "Source": "/var/lib/docker/volumes/shop_data/_data", "Destination": "/data", "RW": true},
                {"Type": "bind", "Source": "/srv/conf", "Destination": "/etc/nginx/conf.d", "RW": false}
            ],
            "SomethingNew": {"x": 1}
        })
    }

    #[test]
    fn con_container_summary_maps_list_item() {
        let c = container_summary(&list_item());
        assert_eq!(c.name, "shop-web-1");
        assert_eq!(c.state, ContainerState::Running);
        assert_eq!(c.health, Some(Health::Healthy));
        assert_eq!(c.exit_code, None);
        assert_eq!(c.ports.len(), 3);
        assert_eq!(c.ports[0].ip, Some("0.0.0.0".parse().unwrap()));
        assert_eq!(c.ports[0].public, Some(8080));
        assert_eq!(c.ports[2].proto, Proto::Udp);
        assert_eq!(c.ports[2].public, None);
        assert_eq!(c.networks, vec!["shop_default".to_string()]);
        assert_eq!(
            c.ip_addresses,
            vec!["172.18.0.3".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(c.mounts[0].source, "shop_data");
        assert!(!c.mounts[1].rw);
        assert_eq!(c.size_rw, Some(12));
        assert_eq!(c.size_root_fs, None);
        assert_eq!(c.created, datetime!(2025-10-02 12:00:00 UTC));
        let compose = c.compose.unwrap();
        assert_eq!(compose.project, "shop");
        assert_eq!(compose.service, "web");
        assert_eq!(compose.number, Some(1));
        assert_eq!(compose.config_files.len(), 2);
        assert_eq!(compose.depends_on, vec!["db", "cache"]);
    }

    #[test]
    fn con_status_text_health_and_exit_code() {
        assert_eq!(exit_code_from_status("Exited (137) 2 hours ago"), Some(137));
        assert_eq!(exit_code_from_status("Exited (-1) 2 hours ago"), Some(-1));
        assert_eq!(exit_code_from_status("Up 2 hours"), None);
        assert_eq!(exit_code_from_status("Exited (oops"), None);
        assert_eq!(
            health_from_status("Up 1 second (health: starting)"),
            Some(Health::Starting)
        );
        assert_eq!(
            health_from_status("Up 1 second (unhealthy)"),
            Some(Health::Unhealthy)
        );
        assert_eq!(health_from_status("Up 1 second"), None);
        let c = container_summary(
            &json!({"Id": "1", "Names": ["/x"], "State": "exited", "Status": "Exited (2) 1 minute ago"}),
        );
        assert_eq!(c.state, ContainerState::Exited);
        assert_eq!(c.exit_code, Some(2));
    }

    #[test]
    fn nfr_031_container_summary_tolerates_garbage() {
        for v in [
            json!(null),
            json!([]),
            json!("x"),
            json!({}),
            json!({"Names": null, "Ports": "nope", "Labels": [1], "State": 7, "Created": "x", "Mounts": [null, 1, {"Type": 5}]}),
            json!({"Ports": [{"PrivatePort": 99999}, {"PrivatePort": "80"}, {}], "NetworkSettings": {"Networks": []}}),
        ] {
            let c = container_summary(&v);
            assert_eq!(c.created, OffsetDateTime::UNIX_EPOCH);
        }
        let c = container_summary(&json!({"Id": "abc", "State": "weird-new-state"}));
        assert_eq!(c.state, ContainerState::Unknown);
        assert_eq!(c.name, "abc");
    }

    fn inspect() -> Value {
        json!({
            "Id": "8dfafdbc3a40",
            "Created": "2026-10-01T10:00:00.5Z",
            "Path": "nginx",
            "Args": ["-g", "daemon off;"],
            "State": {
                "Status": "running", "Running": true, "Pid": 4242, "ExitCode": 0,
                "StartedAt": "2026-10-02T10:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z",
                "Health": {"Status": "healthy", "FailingStreak": 0, "Log": [
                    {"Start": "2026-10-02T10:00:01Z", "End": "2026-10-02T10:00:02Z", "ExitCode": 0, "Output": "ok"}
                ]}
            },
            "Image": "sha256:abc",
            "Name": "/shop-web-1",
            "RestartCount": 2,
            "Platform": "linux",
            "Mounts": [{"Type": "volume", "Name": "shop_data", "Source": "/var/lib/docker/volumes/shop_data/_data", "Destination": "/data", "Driver": "local", "Mode": "z", "RW": true, "Propagation": ""}],
            "Config": {
                "Hostname": "8dfafdbc3a40", "User": "", "Tty": true,
                "Env": ["PATH=/usr/bin", "DB_PASSWORD=a=b", "EMPTY"],
                "Cmd": ["nginx", "-g", "daemon off;"], "Entrypoint": "/docker-entrypoint.sh",
                "Image": "nginx:1.27", "WorkingDir": "/app",
                "Labels": {"com.docker.compose.project": "shop", "com.docker.compose.service": "web"}
            },
            "HostConfig": {
                "NetworkMode": "shop_default", "Dns": ["1.1.1.1"],
                "RestartPolicy": {"Name": "unless-stopped", "MaximumRetryCount": 0},
                "NanoCpus": 500000000, "Memory": 0, "MemorySwap": -1, "PidsLimit": null, "CpuShares": 0,
                "PortBindings": {"80/tcp": [{"HostIp": "", "HostPort": "8080"}]}
            },
            "NetworkSettings": {
                "Ports": {"80/tcp": [{"HostIp": "0.0.0.0", "HostPort": "8080"}], "443/tcp": null},
                "Networks": {"shop_default": {
                    "Aliases": ["web"], "NetworkID": "net1", "Gateway": "172.18.0.1",
                    "IPAddress": "172.18.0.3", "GlobalIPv6Address": "", "MacAddress": "02:42:ac:12:00:03"
                }}
            }
        })
    }

    #[test]
    fn cdt_container_details_maps_inspect() {
        let now = datetime!(2026-10-02 12:00:00 UTC);
        let d = container_details_at(&inspect(), now).unwrap();
        assert_eq!(d.summary.name, "shop-web-1");
        assert_eq!(d.summary.image, "nginx:1.27");
        assert_eq!(d.summary.image_id, "sha256:abc");
        assert_eq!(d.summary.command, "nginx -g daemon off;");
        assert_eq!(d.summary.status_text, "Up 2 hours (healthy)");
        assert_eq!(d.summary.health, Some(Health::Healthy));
        assert_eq!(d.summary.compose.as_ref().unwrap().project, "shop");
        assert_eq!(d.started_at, Some(datetime!(2026-10-02 10:00:00 UTC)));
        assert_eq!(d.finished_at, None);
        assert_eq!(d.restart_count, 2);
        assert_eq!(d.restart_policy.as_deref(), Some("unless-stopped"));
        assert_eq!(d.pid, Some(4242));
        assert_eq!(d.entrypoint, vec!["/docker-entrypoint.sh"]);
        assert_eq!(d.cmd.len(), 3);
        assert_eq!(d.user, None);
        assert_eq!(d.working_dir.as_deref(), Some("/app"));
        assert!(d.tty);
        assert_eq!(d.env[1].key, "DB_PASSWORD");
        assert_eq!(d.env[1].value, "a=b");
        assert_eq!(d.env[2].value, "");
        assert_eq!(d.mounts[0].volume_name.as_deref(), Some("shop_data"));
        assert_eq!(d.mounts[0].propagation, None);
        assert_eq!(
            d.network_settings.network_mode.as_deref(),
            Some("shop_default")
        );
        assert_eq!(d.network_settings.dns, vec!["1.1.1.1"]);
        let n = &d.network_settings.networks[0];
        assert_eq!(n.ipv4.as_deref(), Some("172.18.0.3"));
        assert_eq!(n.ipv6, None);
        assert_eq!(n.aliases, vec!["web"]);
        assert_eq!(d.port_bindings.len(), 2);
        assert_eq!(d.resources.nano_cpus, Some(500_000_000));
        assert_eq!(d.resources.memory, None);
        assert_eq!(d.resources.memory_swap, Some(-1));
        assert_eq!(d.resources.pids_limit, None);
        assert_eq!(d.health_log.len(), 1);
        assert_eq!(d.health_log[0].output, "ok");
        assert_eq!(d.raw, inspect());
    }

    #[test]
    fn cdt_status_text_for_exited_and_created() {
        let now = datetime!(2026-10-02 12:00:00 UTC);
        let exited = json!({"Status": "exited", "ExitCode": 3, "StartedAt": "2026-10-02T11:00:00Z", "FinishedAt": "2026-10-02T11:57:00Z"});
        assert_eq!(
            status_text_from_state(&exited, now),
            "Exited (3) 3 minutes ago"
        );
        let created = json!({"Status": "created", "StartedAt": "0001-01-01T00:00:00Z"});
        assert_eq!(status_text_from_state(&created, now), "Created");
        let paused = json!({"Status": "paused", "StartedAt": "2026-10-02T11:59:00Z"});
        assert_eq!(status_text_from_state(&paused, now), "Up 1 minute (Paused)");
        let d =
            container_details_at(&json!({"Id": "1", "Name": "/x", "State": exited}), now).unwrap();
        assert_eq!(d.summary.exit_code, Some(3));
    }

    #[test]
    fn nfr_031_container_details_tolerates_garbage() {
        assert!(container_details(&json!([])).is_err());
        assert!(container_details(&json!(null)).is_err());
        let d = container_details(&json!({"State": "x", "Config": [], "HostConfig": 1, "NetworkSettings": {"Ports": [], "Networks": 3}})).unwrap();
        assert_eq!(d.summary.state, ContainerState::Unknown);
        assert!(d.env.is_empty());
    }

    #[test]
    fn img_image_summary_and_dangling() {
        let i = image_summary(&json!({
            "Id": "sha256:1", "RepoTags": ["nginx:1.27", "nginx:latest"], "RepoDigests": ["nginx@sha256:aa"],
            "Created": 1_759_406_400, "Size": 100, "SharedSize": -1, "Containers": -1, "Labels": null
        }));
        assert!(!i.dangling);
        assert_eq!(i.repo_tags.len(), 2);
        assert_eq!(i.shared_size, None);
        assert_eq!(i.containers, None);
        let d = image_summary(
            &json!({"Id": "sha256:2", "RepoTags": ["<none>:<none>"], "RepoDigests": ["<none>@<none>"], "Containers": 2}),
        );
        assert!(d.dangling);
        assert!(d.repo_tags.is_empty());
        assert!(d.repo_digests.is_empty());
        assert_eq!(d.containers, Some(2));
        assert!(image_summary(&json!({"Id": "sha256:3", "RepoTags": null})).dangling);
    }

    #[test]
    fn img_image_details_and_layer() {
        let d = image_details(&json!({
            "Id": "sha256:1", "RepoTags": ["alpine:3"], "Created": "2026-01-01T00:00:00Z", "Size": 5,
            "Architecture": "arm64", "Variant": "v8", "Os": "linux", "Author": "",
            "Config": {"Env": ["A=1"], "Cmd": ["/bin/sh"], "Entrypoint": null, "ExposedPorts": {"80/tcp": {}},
                       "WorkingDir": "", "Volumes": {"/data": {}}, "Labels": {"a": "b"}},
            "RootFS": {"Type": "layers", "Layers": ["sha256:l1", "sha256:l2"]}
        }))
        .unwrap();
        assert_eq!(d.architecture, "arm64");
        assert_eq!(d.variant.as_deref(), Some("v8"));
        assert_eq!(d.author, None);
        assert_eq!(d.config.exposed_ports, vec!["80/tcp"]);
        assert_eq!(d.config.volumes, vec!["/data"]);
        assert!(d.config.entrypoint.is_empty());
        assert_eq!(d.root_fs_layers.len(), 2);
        assert_eq!(d.summary.labels["a"], "b");
        assert!(image_details(&json!("x")).is_err());

        let l = image_layer(
            &json!({"Id": "<missing>", "Created": 1_759_406_400, "CreatedBy": "/bin/sh -c #(nop) CMD", "Size": 0, "Comment": ""}),
        );
        assert_eq!(l.id, None);
        assert_eq!(l.created_by, "/bin/sh -c #(nop) CMD");
        assert_eq!(
            image_layer(&json!({"Id": "sha256:x"})).id.as_deref(),
            Some("sha256:x")
        );
    }

    #[test]
    fn vol_volume_summary() {
        let v = volume_summary(&json!({
            "Name": "shop_data", "Driver": "local", "Mountpoint": "/var/lib/docker/volumes/shop_data/_data",
            "CreatedAt": "2026-10-01T10:00:00Z", "Scope": "local",
            "Labels": {"com.docker.compose.project": "shop", "com.docker.compose.volume": "data"},
            "UsageData": {"Size": -1, "RefCount": 2}
        }));
        assert_eq!(v.size, None);
        assert_eq!(v.ref_count, Some(2));
        assert_eq!(v.compose.unwrap().project, "shop");
        let v = volume_summary(&json!({"Name": "x", "Labels": null, "UsageData": null}));
        assert_eq!(v.created, None);
        assert!(v.labels.is_empty());
    }

    #[test]
    fn net_network_summary_and_details() {
        let raw = json!({
            "Name": "shop_default", "Id": "net1", "Created": "2026-10-01T10:00:00.123Z", "Scope": "local",
            "Driver": "bridge", "EnableIPv6": false, "Internal": false, "Attachable": true,
            "IPAM": {"Driver": "default", "Config": [{"Subnet": "172.18.0.0/16", "Gateway": "172.18.0.1"}]},
            "Containers": {"c1": {"Name": "shop-web-1", "MacAddress": "02:42", "IPv4Address": "172.18.0.3/16", "IPv6Address": ""}},
            "Options": {"com.docker.network.bridge.name": "br-1"},
            "Labels": {"com.docker.compose.project": "shop"}
        });
        let s = network_summary(&raw);
        assert_eq!(s.subnets[0].subnet.as_deref(), Some("172.18.0.0/16"));
        assert_eq!(s.subnets[0].ip_range, None);
        assert!(s.attachable);
        assert_eq!(s.containers, Some(1));
        assert_eq!(s.compose.unwrap().project, "shop");
        let d = network_details(&raw).unwrap();
        assert_eq!(d.containers[0].id, "c1");
        assert_eq!(d.containers[0].ipv4.as_deref(), Some("172.18.0.3/16"));
        assert_eq!(d.containers[0].ipv6, None);
        assert_eq!(d.options.len(), 1);
        assert_eq!(network_summary(&json!({"Name": "host"})).containers, None);
        assert!(network_details(&json!(1)).is_err());
    }

    #[test]
    fn eng_engine_event() {
        let e = engine_event(&json!({
            "Type": "container", "Action": "start", "Actor": {"ID": "c1", "Attributes": {"name": "web", "exitCode": 0}},
            "scope": "local", "time": 1_759_406_400, "timeNano": 1_759_406_400_500_000_000_i64
        }))
        .unwrap();
        assert_eq!(e.kind, ResourceKind::Container);
        assert_eq!(e.action, "start");
        assert_eq!(e.id, "c1");
        assert_eq!(e.attributes["exitCode"], "0");
        assert_eq!(e.at, datetime!(2025-10-02 12:00:00.5 UTC));
        assert!(engine_event(&json!({"Type": "plugin", "Action": "enable"})).is_none());
        assert!(engine_event(&json!([])).is_none());
        let legacy =
            engine_event(&json!({"status": "die", "id": "c2", "time": 1_759_406_400})).unwrap();
        assert_eq!(legacy.action, "die");
        assert_eq!(legacy.id, "c2");
        assert_eq!(legacy.at, datetime!(2025-10-02 12:00:00 UTC));
    }

    #[test]
    fn vol_disk_usage_legacy_shape() {
        let du = disk_usage(&json!({
            "LayersSize": 1000,
            "Images": [{"Size": 600, "Containers": 1}, {"Size": 500, "Containers": 0}],
            "Containers": [{"SizeRw": 10}, {"SizeRw": -1}, {}],
            "Volumes": [
                {"Name": "a", "UsageData": {"Size": 5, "RefCount": 1}},
                {"Name": "b", "UsageData": {"Size": -1, "RefCount": -1}}
            ],
            "BuildCache": [{"Size": 7}, {"Size": 8}]
        }));
        assert_eq!(du.images_size, 1000);
        assert_eq!(du.images_reclaimable, Some(500));
        assert_eq!(du.containers_size, 10);
        assert_eq!(du.volumes, vec![("a".to_string(), 5)]);
        assert_eq!(du.volume_refs, vec![("a".to_string(), 1)]);
        assert_eq!(du.build_cache, 15);
    }

    #[test]
    fn vol_disk_usage_new_shape_and_garbage() {
        let du = disk_usage(&json!({
            "ImageUsage": {"TotalSize": 2000, "Reclaimable": 300, "Items": null},
            "ContainerUsage": {"TotalSize": 40},
            "VolumeUsage": {"TotalSize": 9, "Items": [{"Name": "v", "UsageData": {"Size": 9, "RefCount": 0}}]},
            "BuildCacheUsage": {"TotalSize": 11}
        }));
        assert_eq!(du.images_size, 2000);
        assert_eq!(du.images_reclaimable, Some(300));
        assert_eq!(du.containers_size, 40);
        assert_eq!(du.volumes, vec![("v".to_string(), 9)]);
        assert_eq!(du.volume_refs, vec![("v".to_string(), 0)]);
        assert_eq!(du.build_cache, 11);
        assert_eq!(disk_usage(&json!(null)), DiskUsage::default());
        assert_eq!(
            disk_usage(&json!({"Images": 3, "Volumes": {}})).images_size,
            0
        );
    }

    #[test]
    fn vol_used_by() {
        let c = container_summary(&list_item());
        let refs = volume_used_by("shop_data", std::slice::from_ref(&c));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "shop-web-1");
        assert_eq!(refs[0].detail.as_deref(), Some("/data"));
        assert_eq!(refs[0].rw, Some(true));
        assert!(volume_used_by("other", &[c]).is_empty());
    }
}
