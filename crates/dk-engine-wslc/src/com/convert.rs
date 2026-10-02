//! Pure conversions from WSLC COM outputs to DTOs (platform-neutral, unit-tested everywhere).
//! The `docker_json` mappers handle Docker-shaped JSON (inspect, volumes, networks, events);
//! this module covers the WSLC-specific bits: `ListContainers` entries (labels/networks/mounts
//! are `k=v,…` strings, F-6), image rows, events JSON adaptation and log line splitting.

use std::collections::BTreeMap;
use std::net::IpAddr;

use bytes::Bytes;
use dk_core::{
    ContainerState, ContainerSummary, Health, ImageSummary, LogChunk, LogStream, MountKind,
    MountSummary, PortMapping, Proto, PullProgress,
};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Plain-data copy of one `WSLCContainerEntry` (decoupled from the FFI struct for tests).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawContainerEntry {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: Option<String>,
    pub status: Option<String>,
    pub labels: Option<String>,
    pub networks: Option<String>,
    pub mounts: Option<String>,
    pub state_changed_at: i64,
    pub created_at: i64,
    pub size_rw: i64,
    pub size_root_fs: i64,
    pub local_volumes: u32,
    pub state: i32,
}

/// Plain-data copy of one `WSLCPortMapping` (+ owning container id).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawPort {
    pub container_id: String,
    pub host_port: u16,
    pub container_port: u16,
    pub family: i32,
    pub protocol: i32,
    pub binding_address: String,
}

/// `WSLCContainerState` → DTO state. `Deleted` reads as "stopped" in wslc (ContainerService.cpp).
pub fn container_state(state: i32) -> ContainerState {
    match state {
        1 => ContainerState::Created,
        2 => ContainerState::Running,
        3 => ContainerState::Exited,
        4 => ContainerState::Dead,
        _ => ContainerState::Unknown,
    }
}

/// Splits the server's `Join(labels, ',')` of `k=v` items. Values may contain commas
/// (e.g. `com.docker.compose.depends_on=a:service_started:false,b:…` or a ports list in the
/// `com.microsoft.wsl.container.metadata` JSON); a segment without `=` that does not look like
/// a key is therefore appended to the previous value.
pub fn parse_labels(s: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for seg in s.split(',') {
        if seg.is_empty() && last.is_none() {
            continue;
        }
        match seg.split_once('=') {
            Some((k, v)) if looks_like_label_key(k) => {
                out.insert(k.to_owned(), v.to_owned());
                last = Some(k.to_owned());
            }
            _ => {
                if let Some(v) = last.as_ref().and_then(|k| out.get_mut(k)) {
                    v.push(',');
                    v.push_str(seg);
                }
            }
        }
    }
    out
}

/// Label keys are reverse-DNS-ish: `[A-Za-z0-9._/-]+`, never containing JSON/space punctuation.
fn looks_like_label_key(k: &str) -> bool {
    !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn split_list(s: Option<&str>) -> Vec<String> {
    s.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Mount list: named volumes report the volume name, binds the host path (WSLCSession.cpp).
pub fn parse_mounts(s: Option<&str>) -> Vec<MountSummary> {
    split_list(s)
        .into_iter()
        .map(|src| {
            let kind = if src.starts_with('/') || src.contains('\\') || src.contains(':') {
                MountKind::Bind
            } else {
                MountKind::Volume
            };
            MountSummary {
                kind,
                source: src,
                destination: String::new(),
                rw: true,
            }
        })
        .collect()
}

fn proto(p: i32) -> Proto {
    match p {
        17 => Proto::Udp,
        132 => Proto::Sctp,
        _ => Proto::Tcp,
    }
}

/// Health from a Docker status string: `Up 3 minutes (healthy)` / `(health: starting)`.
pub fn health_from_status(status: &str) -> Option<Health> {
    let open = status.rfind('(')?;
    let inner = status[open + 1..].strip_suffix(')')?;
    Health::parse(inner)
}

/// Exit code from `Exited (137) 2 minutes ago`.
pub fn exit_code_from_status(status: &str) -> Option<i64> {
    let rest = status.trim().strip_prefix("Exited (")?;
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

fn unix(ts: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(ts).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// Builds a `ContainerSummary` from one entry + all port mappings of the list call.
pub fn container_summary(e: &RawContainerEntry, ports: &[RawPort]) -> ContainerSummary {
    let labels = e.labels.as_deref().map(parse_labels).unwrap_or_default();
    let status_text = e.status.clone().unwrap_or_default();
    let state = container_state(e.state);
    ContainerSummary {
        id: e.id.clone(),
        name: e.name.trim_start_matches('/').to_owned(),
        image: e.image.clone(),
        image_id: String::new(),
        command: e.command.clone().unwrap_or_default(),
        created: unix(e.created_at),
        state,
        health: health_from_status(&status_text),
        exit_code: if state == ContainerState::Exited {
            exit_code_from_status(&status_text)
        } else {
            None
        },
        status_text,
        ports: ports
            .iter()
            .filter(|p| p.container_id == e.id)
            .map(|p| PortMapping {
                ip: p.binding_address.parse::<IpAddr>().ok(),
                private: p.container_port,
                public: Some(p.host_port).filter(|&h| h != 0),
                proto: proto(p.protocol),
            })
            .collect(),
        networks: split_list(e.networks.as_deref()),
        ip_addresses: Vec::new(),
        mounts: parse_mounts(e.mounts.as_deref()),
        size_rw: u64::try_from(e.size_rw).ok().filter(|&s| s > 0),
        size_root_fs: u64::try_from(e.size_root_fs).ok().filter(|&s| s > 0),
        // Grouping derives compose info from `labels` when this is None (dk-core grouping);
        // left None like the CLI transport so both transports produce identical DTOs.
        compose: None,
        labels,
    }
}

/// Plain-data copy of one `WSLCImageInformation` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawImage {
    pub image: String,
    pub hash: String,
    pub digest: String,
    pub size: i64,
    pub created: i64,
    pub containers: i64,
}

/// Groups `ListImages` rows (one per repo tag / digest, WSLCSession.cpp) by image id.
pub fn image_summaries(rows: &[RawImage]) -> Vec<ImageSummary> {
    let mut by_id: Vec<ImageSummary> = Vec::new();
    for r in rows {
        let idx = match by_id.iter().position(|s| s.id == r.hash) {
            Some(i) => i,
            None => {
                by_id.push(ImageSummary {
                    id: r.hash.clone(),
                    repo_tags: Vec::new(),
                    repo_digests: Vec::new(),
                    created: unix(r.created),
                    size: u64::try_from(r.size).unwrap_or(0),
                    shared_size: None,
                    containers: u32::try_from(r.containers).ok(),
                    labels: BTreeMap::new(),
                    dangling: false,
                });
                by_id.len() - 1
            }
        };
        let s = &mut by_id[idx];
        let tagged =
            r.image.contains(':') && r.image != "<none>:<none>" && !r.image.ends_with(":<none>");
        if tagged && !s.repo_tags.contains(&r.image) {
            s.repo_tags.push(r.image.clone());
        }
        if !r.digest.is_empty() && !s.repo_digests.contains(&r.digest) {
            s.repo_digests.push(r.digest.clone());
        }
    }
    for s in &mut by_id {
        s.dangling = s.repo_tags.is_empty();
    }
    by_id
}

/// `wslc_schema::Event` (`{Type, Action, Actor:{ID, Attributes}, time}`) → the Docker events
/// shape `docker_json::engine_event` understands (adds lowercase `id`, `status`, `from`).
pub fn adapt_event_json(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        let id = obj
            .get("Actor")
            .and_then(|a| a.get("ID"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(id) = id {
            obj.entry("id").or_insert(Value::String(id));
        }
        if let Some(action) = obj.get("Action").cloned() {
            obj.entry("status").or_insert(action);
        }
        if let Some(t) = obj.get("time").cloned() {
            obj.entry("timeNano").or_insert_with(|| {
                Value::from(t.as_i64().unwrap_or(0).saturating_mul(1_000_000_000))
            });
        }
    }
    v
}

/// One log line → `LogChunk`, stripping the leading RFC 3339 timestamp when timestamps were
/// requested (same rules as the Docker backend).
pub fn log_line(stream: LogStream, line: Bytes, timestamps: bool) -> LogChunk {
    if timestamps {
        let token_end = line.iter().position(|b| *b == b' ' || *b == b'\n');
        let token = token_end.map_or(&line[..], |i| &line[..i]);
        if let Some(ts) = std::str::from_utf8(token)
            .ok()
            .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        {
            let rest = match token_end {
                Some(i) if line[i] == b' ' => line.slice(i + 1..),
                Some(i) => line.slice(i..),
                None => Bytes::new(),
            };
            return LogChunk {
                stream,
                ts: Some(ts),
                bytes: rest,
            };
        }
    }
    LogChunk {
        stream,
        ts: None,
        bytes: line,
    }
}

/// `IProgressCallback::OnProgress(status, id, current, total)` → `PullProgress`.
pub fn pull_progress(status: &str, id: &str, current: u64, total: u64) -> PullProgress {
    if id.is_empty() {
        PullProgress::Status(status.to_owned())
    } else {
        PullProgress::Layer {
            id: id.to_owned(),
            status: status.to_owned(),
            current: (current > 0 || total > 0).then_some(current),
            total: (total > 0).then_some(total),
        }
    }
}

/// `Digest: sha256:…` status line → digest.
pub fn digest_from_status(status: &str) -> Option<String> {
    status
        .strip_prefix("Digest: ")
        .map(|d| d.trim().to_owned())
        .filter(|d| d.contains(':'))
}

/// Docker registry auth header (`X-Registry-Auth`): base64(JSON). Same encoding as
/// `wslutil::BuildRegistryAuthHeader` (standard alphabet, padded).
pub fn registry_auth_header(auth: &dk_core::RegistryAuth) -> String {
    let mut m = serde_json::Map::new();
    if let Some(tok) = auth.identity_token.as_ref().filter(|t| !t.is_empty()) {
        m.insert(
            "identitytoken".into(),
            Value::String(tok.expose().to_owned()),
        );
    } else {
        m.insert(
            "username".into(),
            Value::String(auth.username.clone().unwrap_or_default()),
        );
        m.insert(
            "password".into(),
            Value::String(
                auth.password
                    .as_ref()
                    .map(|p| p.expose().to_owned())
                    .unwrap_or_default(),
            ),
        );
    }
    if !auth.server.is_empty() {
        m.insert("serveraddress".into(), Value::String(auth.server.clone()));
    }
    base64_std(Value::Object(m).to_string().as_bytes())
}

fn base64_std(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Signal name/number → `WSLCSignal` value (Linux numbering, WSLCShared.idl).
pub fn signal_number(s: &str) -> Option<i32> {
    if let Ok(n) = s.parse::<i32>() {
        return (1..=31).contains(&n).then_some(n);
    }
    let name = s.trim().to_ascii_uppercase();
    let name = name.strip_prefix("SIG").unwrap_or(&name);
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" | "IOT" => 6,
        "BUS" => 7,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        "STKFLT" => 16,
        "CHLD" => 17,
        "CONT" => 18,
        "STOP" => 19,
        "TSTP" => 20,
        "TTIN" => 21,
        "TTOU" => 22,
        "URG" => 23,
        "XCPU" => 24,
        "XFSZ" => 25,
        "VTALRM" => 26,
        "PROF" => 27,
        "WINCH" => 28,
        "IO" | "POLL" => 29,
        "PWR" => 30,
        "SYS" => 31,
        _ => return None,
    })
}

/// Splits an image reference into `(repo, tag)` for `TagImage` (`tag` defaults to `latest`).
pub fn split_tag_target(repo: &str, tag: &str) -> (String, String) {
    let tag = if tag.is_empty() { "latest" } else { tag };
    (repo.to_owned(), tag.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> RawContainerEntry {
        RawContainerEntry {
            id: "2e4fac884218aa".into(),
            name: "dk-fixture-nginx".into(),
            image: "nginx:alpine".into(),
            command: Some("/docker-entrypoint.sh nginx -g 'daemon off;'".into()),
            status: Some("Up 28 minutes (healthy)".into()),
            labels: Some(
                "com.docker.compose.project=web,com.docker.compose.service=nginx,\
                 com.docker.compose.depends_on=db:service_started:false,cache:service_started:false,\
                 maintainer=NGINX <docker-maint@nginx.com>"
                    .into(),
            ),
            networks: Some("bridge,web_default".into()),
            mounts: Some("webdata,/mnt/c/Users/x/site".into()),
            state_changed_at: 1_759_400_000,
            created_at: 1_759_399_000,
            size_rw: -1,
            size_root_fs: 0,
            local_volumes: 1,
            state: 2,
        }
    }

    #[test]
    fn labels_with_commas_in_values() {
        let l = parse_labels(entry().labels.as_deref().unwrap_or_default());
        assert_eq!(l["com.docker.compose.project"], "web");
        assert_eq!(
            l["com.docker.compose.depends_on"],
            "db:service_started:false,cache:service_started:false"
        );
        assert_eq!(l["maintainer"], "NGINX <docker-maint@nginx.com>");
        assert_eq!(l.len(), 4);
        assert!(parse_labels("").is_empty());
        let json =
            parse_labels(r#"a=1,com.microsoft.wsl.container.metadata={"Ports":[{"x":1},{"y":2}]}"#);
        assert_eq!(
            json["com.microsoft.wsl.container.metadata"],
            r#"{"Ports":[{"x":1},{"y":2}]}"#
        );
    }

    #[test]
    fn summary_from_entry_and_ports() {
        let ports = vec![
            RawPort {
                container_id: "2e4fac884218aa".into(),
                host_port: 18081,
                container_port: 80,
                family: 2,
                protocol: 6,
                binding_address: "127.0.0.1".into(),
            },
            RawPort {
                container_id: "other".into(),
                host_port: 1,
                container_port: 1,
                family: 2,
                protocol: 17,
                binding_address: "0.0.0.0".into(),
            },
        ];
        let s = container_summary(&entry(), &ports);
        assert_eq!(s.state, ContainerState::Running);
        assert_eq!(s.health, Some(Health::Healthy));
        assert_eq!(s.ports.len(), 1);
        assert_eq!(s.ports[0].public, Some(18081));
        assert_eq!(s.ports[0].ip, Some("127.0.0.1".parse().expect("ip")));
        assert_eq!(s.networks, vec!["bridge", "web_default"]);
        assert_eq!(s.mounts.len(), 2);
        assert_eq!(s.mounts[0].kind, MountKind::Volume);
        assert_eq!(s.mounts[1].kind, MountKind::Bind);
        assert_eq!(s.created.unix_timestamp(), 1_759_399_000);
        assert_eq!(s.size_rw, None);
        assert_eq!(s.labels["com.docker.compose.project"], "web");
    }

    #[test]
    fn exited_status() {
        let mut e = entry();
        e.state = 3;
        e.status = Some("Exited (137) 2 minutes ago".into());
        let s = container_summary(&e, &[]);
        assert_eq!(s.exit_code, Some(137));
        assert_eq!(s.health, None);
        assert_eq!(container_state(4), ContainerState::Dead);
        assert_eq!(container_state(99), ContainerState::Unknown);
    }

    #[test]
    fn images_grouped_by_id() {
        let rows = vec![
            RawImage {
                image: "nginx:alpine".into(),
                hash: "sha256:a".into(),
                size: 10,
                created: 5,
                containers: -1,
                ..Default::default()
            },
            RawImage {
                image: "nginx:latest".into(),
                hash: "sha256:a".into(),
                size: 10,
                created: 5,
                containers: -1,
                ..Default::default()
            },
            RawImage {
                image: "<none>:<none>".into(),
                hash: "sha256:b".into(),
                size: 3,
                created: 1,
                containers: 2,
                ..Default::default()
            },
        ];
        let s = image_summaries(&rows);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].repo_tags, vec!["nginx:alpine", "nginx:latest"]);
        assert!(!s[0].dangling);
        assert_eq!(s[0].containers, None);
        assert!(s[1].dangling);
        assert_eq!(s[1].containers, Some(2));
    }

    #[test]
    fn event_adaptation() {
        let v: Value = serde_json::from_str(
            r#"{"Type":"container","Action":"start","Actor":{"ID":"abc","Attributes":{"name":"x"}},"time":1759400000}"#,
        )
        .expect("json");
        let a = adapt_event_json(v);
        assert_eq!(a["id"], "abc");
        assert_eq!(a["status"], "start");
        assert_eq!(a["timeNano"], 1_759_400_000_000_000_000i64);
        assert_eq!(a["Actor"]["Attributes"]["name"], "x");
    }

    #[test]
    fn log_lines() {
        let c = log_line(
            LogStream::Stdout,
            Bytes::from_static(b"2026-10-02T10:00:00.123456789Z hello\n"),
            true,
        );
        assert_eq!(c.bytes, Bytes::from_static(b"hello\n"));
        assert!(c.ts.is_some());
        let c = log_line(LogStream::Stderr, Bytes::from_static(b"no ts\n"), true);
        assert_eq!(c.ts, None);
        assert_eq!(c.bytes, Bytes::from_static(b"no ts\n"));
    }

    #[test]
    fn progress_and_auth() {
        assert_eq!(
            pull_progress("Pulling from library/nginx", "", 0, 0),
            PullProgress::Status("Pulling from library/nginx".into())
        );
        assert_eq!(
            pull_progress("Downloading", "abc", 5, 10),
            PullProgress::Layer {
                id: "abc".into(),
                status: "Downloading".into(),
                current: Some(5),
                total: Some(10)
            }
        );
        assert_eq!(
            digest_from_status("Digest: sha256:abc"),
            Some("sha256:abc".into())
        );
        assert_eq!(base64_std(b"Man"), "TWFu");
        assert_eq!(base64_std(b"Ma"), "TWE=");
        assert_eq!(base64_std(b"M"), "TQ==");
        let auth = dk_core::RegistryAuth {
            server: "ghcr.io".into(),
            username: Some("u".into()),
            password: Some(dk_core::SecretString::new("p")),
            identity_token: None,
        };
        let h = registry_auth_header(&auth);
        assert!(!h.contains("p\""), "must be encoded");
        assert_eq!(
            h,
            base64_std(br#"{"password":"p","serveraddress":"ghcr.io","username":"u"}"#)
        );
    }

    #[test]
    fn signals() {
        assert_eq!(signal_number("SIGKILL"), Some(9));
        assert_eq!(signal_number("TERM"), Some(15));
        assert_eq!(signal_number("2"), Some(2));
        assert_eq!(signal_number("SIGRTMIN+3"), None);
        assert_eq!(signal_number("0"), None);
    }
}
