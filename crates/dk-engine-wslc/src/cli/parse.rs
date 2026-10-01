//! Pure, tolerant parsers for `wslc.exe` 3.0.1 output (spec 20 §5.5, spike F-10).
//!
//! Unknown fields are ignored, missing fields default, human-formatted sizes and times are
//! parsed with fallbacks (NFR-031). Nothing here panics on bad input. Every output line may end
//! in `\r\n` (most commands) or `\n` (`logs`), so lines are always trimmed.
//!
//! Docker-inspect-shaped outputs (`container|image|volume|network inspect`) are not parsed
//! here; they go through `dk_core::docker_json`.

use std::collections::BTreeMap;
use std::net::IpAddr;

use dk_core::{
    ContainerState, ContainerSummary, EngineError, EngineEvent, EngineResult, Health,
    ImageDeleteItem, ImageSummary, IpamConfig, MountKind, MountSummary, NetworkSummary,
    PortMapping, Proto, PruneReport, PullProgress, ResourceKind, VolumeSummary,
};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// Internal label WSLC adds to every container (port/volume metadata as JSON). Hidden from
/// DTOs: `inspect` doesn't report it either.
pub(crate) const WSL_METADATA_LABEL: &str = "com.microsoft.wsl.container.metadata";

// ───────────────────────────── JSON helpers ─────────────────────────────

/// `--format json` lists: NDJSON (one object per line, wslc 3.0.1) or a JSON array.
/// Blank lines are skipped; a malformed line is skipped with a warning (tolerant).
pub(crate) fn json_list(s: &str) -> EngineResult<Vec<Value>> {
    let t = s.trim_start_matches('\u{feff}').trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    if t.starts_with('[') {
        return match serde_json::from_str::<Value>(t)? {
            Value::Array(a) => Ok(a),
            other => Ok(vec![other]),
        };
    }
    let mut out = Vec::new();
    let mut bad = 0usize;
    for line in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(a)) => out.extend(a),
            Ok(v) => out.push(v),
            Err(_) => bad += 1,
        }
    }
    if out.is_empty() && bad > 0 {
        return Err(EngineError::protocol("wslc printed no parseable JSON"));
    }
    if bad > 0 {
        tracing::warn!(target: "dk_engine_wslc::cli", bad, "skipped malformed wslc JSON lines");
    }
    Ok(out)
}

/// The first object of an `inspect` JSON array (or a bare object).
pub(crate) fn first_object(s: &str) -> EngineResult<Value> {
    let v: Value = serde_json::from_str(s.trim_start_matches('\u{feff}').trim())?;
    match v {
        Value::Array(mut a) if !a.is_empty() => Ok(a.swap_remove(0)),
        Value::Object(_) => Ok(v),
        _ => Err(EngineError::protocol("wslc inspect returned no object")),
    }
}

/// Field lookup: exact key first, then case-insensitive (`ID` vs `Id`).
pub(crate) fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let obj = v.as_object()?;
    obj.get(key).or_else(|| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// String value of a field; numbers and bools are stringified, null/missing → "".
pub(crate) fn text(v: &Value, key: &str) -> String {
    match field(v, key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// `"N/A"`, `"<none>"`, `""` → None.
fn meaningful(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty() && s != "N/A" && s != "<none>").then_some(s)
}

fn bool_field(v: &Value, key: &str) -> bool {
    match field(v, key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    }
}

// ───────────────────────────── scalars ─────────────────────────────

/// Human sizes as printed by the Docker CLI / wslc: `0B`, `4.1kB`, `62.9MB`, `17.02MiB`,
/// `3.4GiB`, `1.2 GB`. Decimal units are powers of 1000, `*iB` powers of 1024. Trailing
/// text (`0B (virtual 4.42MB)`) is ignored. `N/A` → None.
pub(crate) fn parse_size(s: &str) -> Option<u64> {
    let s = meaningful(s)?;
    let s = s.split(" (").next().unwrap_or(s).trim();
    let num_end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
        .map_or(s.len(), |(i, _)| i);
    let n: f64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim();
    let mult: f64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" | "k" => 1e3,
        "mb" | "m" => 1e6,
        "gb" | "g" => 1e9,
        "tb" | "t" => 1e12,
        "pb" | "p" => 1e15,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024f64.powi(4),
        "pib" => 1024f64.powi(5),
        _ => return None,
    };
    let bytes = (n * mult).round();
    (bytes.is_finite() && bytes >= 0.0).then_some(bytes as u64)
}

/// `"0.00%"` → 0.0. None when not a number.
pub(crate) fn parse_percent(s: &str) -> Option<f64> {
    meaningful(s)?.trim_end_matches('%').trim().parse().ok()
}

/// `"17.02MiB / 15.53GiB"` → (used, limit). Missing halves → None.
pub(crate) fn parse_pair(s: &str) -> (Option<u64>, Option<u64>) {
    match s.split_once('/') {
        Some((a, b)) => (parse_size(a), parse_size(b)),
        None => (parse_size(s), None),
    }
}

fn parse_offset(s: &str) -> Option<UtcOffset> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("z") || s.eq_ignore_ascii_case("utc") {
        return Some(UtcOffset::UTC);
    }
    UtcOffset::parse(
        s,
        format_description!("[offset_hour sign:mandatory][offset_minute]"),
    )
    .or_else(|_| {
        UtcOffset::parse(
            s,
            format_description!("[offset_hour sign:mandatory]:[offset_minute]"),
        )
    })
    .ok()
}

/// Timestamps from wslc lists, normalised to UTC:
/// - `2026-10-01 14:03:11 +0200 SELČ` (container/image `CreatedAt`; the localised TZ name
///   is ignored, the numeric offset is authoritative),
/// - `2026-10-01 23:09:11.462461977 +0000 UTC` (network `CreatedAt`),
/// - RFC 3339 (`inspect`, `events`, `logs --timestamps`),
/// - unix seconds.
///
/// Zero times (`0001-01-01…`) → None.
pub(crate) fn parse_time(s: &str) -> Option<OffsetDateTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let t = if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        t
    } else if let Some(t) = parse_spaced_time(s) {
        t
    } else if let Ok(secs) = s.parse::<i64>() {
        OffsetDateTime::from_unix_timestamp(secs).ok()?
    } else {
        return None;
    };
    (t.year() > 1).then(|| t.to_offset(UtcOffset::UTC))
}

fn parse_spaced_time(s: &str) -> Option<OffsetDateTime> {
    let mut it = s.split_whitespace();
    let date = Date::parse(it.next()?, format_description!("[year]-[month]-[day]")).ok()?;
    let time = Time::parse(
        it.next()?,
        format_description!("[hour]:[minute]:[second][optional [.[subsecond]]]"),
    )
    .ok()?;
    // No offset → assume UTC.
    let offset = it.next().and_then(parse_offset).unwrap_or(UtcOffset::UTC);
    Some(PrimitiveDateTime::new(date, time).assume_offset(offset))
}

// ───────────────────────────── labels / k=v lists ─────────────────────────────

/// Does `s` start with `key=` where key is label-like (`[A-Za-z0-9_][A-Za-z0-9._/-]*`)?
fn starts_with_key(s: &str) -> bool {
    let Some(eq) = s.find('=') else {
        return false;
    };
    let key = &s[..eq];
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// Split `k=v,k2=v2` (labels) or `k=v, k2=v2` (event attributes). A comma only separates
/// pairs when it's outside `{}`/`[]` and the next segment starts with `key=`, so values with
/// commas or embedded JSON (the WSL metadata label) survive.
pub(crate) fn parse_kv_list(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let s = s.trim();
    if s.is_empty() {
        return out;
    }
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0usize;
    let mut parts: Vec<&str> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\\' if in_str => i += 1,
            b'"' if depth > 0 => in_str = !in_str,
            b'{' | b'[' if !in_str => depth += 1,
            b'}' | b']' if !in_str => depth = (depth - 1).max(0),
            b',' if depth == 0 && !in_str => {
                if starts_with_key(s[i + 1..].trim_start()) {
                    parts.push(&s[start..i]);
                    start = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        match p.split_once('=') {
            Some((k, v)) => out.insert(k.trim().to_owned(), v.to_owned()),
            None => out.insert(p.to_owned(), String::new()),
        };
    }
    out
}

/// Labels as a `k=v,…` string or a JSON object; drops the internal WSL metadata label.
pub(crate) fn labels(v: Option<&Value>) -> BTreeMap<String, String> {
    let mut m = match v {
        Some(Value::String(s)) => parse_kv_list(s),
        Some(Value::Object(o)) => o
            .iter()
            .map(|(k, v)| {
                let v = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                (k.clone(), v)
            })
            .collect(),
        _ => BTreeMap::new(),
    };
    m.remove(WSL_METADATA_LABEL);
    m
}

// ───────────────────────────── ports ─────────────────────────────

fn parse_proto(s: &str) -> Proto {
    match s.trim().to_ascii_lowercase().as_str() {
        "udp" => Proto::Udp,
        "sctp" => Proto::Sctp,
        _ => Proto::Tcp,
    }
}

fn port_range(s: &str) -> Option<(u16, u16)> {
    let s = s.trim();
    match s.split_once('-') {
        Some((a, b)) => {
            let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (a <= b).then_some((a, b))
        }
        None => {
            let p = s.parse().ok()?;
            Some((p, p))
        }
    }
}

/// Docker-CLI port strings: `127.0.0.1:18081->80/tcp, [::]:8080->80/tcp, 53/udp,
/// 0.0.0.0:8000-8001->80-81/tcp`. Unparseable entries are skipped.
pub(crate) fn parse_ports(s: &str) -> Vec<PortMapping> {
    const MAX_EXPANDED: usize = 1024;
    let mut out = Vec::new();
    for entry in s.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (mapping, proto) = entry.rsplit_once('/').unwrap_or((entry, "tcp"));
        let proto = parse_proto(proto);
        let (host, private) = match mapping.split_once("->") {
            Some((h, p)) => (Some(h.trim()), p.trim()),
            None => (None, mapping.trim()),
        };
        let Some((p_lo, p_hi)) = port_range(private) else {
            continue;
        };
        let (ip, public) = match host {
            None => (None, None),
            Some(h) => {
                let (ip, port) = match h.rsplit_once(':') {
                    Some((ip, port)) => (Some(ip), port),
                    None => (None, h),
                };
                let ip = ip
                    .map(|i| i.trim_start_matches('[').trim_end_matches(']'))
                    .filter(|i| !i.is_empty())
                    .and_then(|i| i.parse::<IpAddr>().ok());
                (ip, port_range(port))
            }
        };
        for (n, private) in (p_lo..=p_hi).enumerate() {
            if out.len() >= MAX_EXPANDED {
                break;
            }
            let public = public.and_then(|(lo, hi)| {
                let p = u32::from(lo) + n as u32;
                (p <= u32::from(hi)).then_some(p as u16)
            });
            out.push(PortMapping {
                ip,
                private,
                public,
                proto,
            });
        }
    }
    out
}

// ───────────────────────────── containers ─────────────────────────────

/// `"Exited (3) 2 seconds ago"` → 3.
pub(crate) fn exit_code_from_status(status: &str) -> Option<i64> {
    let rest = status.trim().strip_prefix("Exited")?.trim_start();
    let inner = rest.strip_prefix('(')?;
    inner[..inner.find(')')?].trim().parse().ok()
}

/// `(healthy)`, `(unhealthy)`, `(health: starting)` inside a status string.
fn health_from_status(status: &str) -> Option<Health> {
    let open = status.rfind('(')?;
    let close = status[open..].find(')')? + open;
    Health::parse(&status[open + 1..close])
}

/// `Command` is JSON-quoted by the CLI (`"\"/entry.sh nginx\""`): strip one pair of quotes.
fn unquote(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(t)
        .to_owned()
}

fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with('\\') || s.as_bytes().get(1) == Some(&b':') && s.len() > 2
}

/// `container list --all --no-trunc --format json` entry → summary. `compose` is left `None`
/// (the engine derives it from labels).
pub(crate) fn container_summary(v: &Value) -> ContainerSummary {
    let status_text = text(v, "Status");
    let state_s = text(v, "State");
    let mut state = ContainerState::parse(&state_s);
    if state == ContainerState::Unknown {
        // Older formats: derive from the status text.
        let st = status_text.to_ascii_lowercase();
        state = if st.starts_with("up") {
            if st.contains("(paused)") {
                ContainerState::Paused
            } else {
                ContainerState::Running
            }
        } else if st.starts_with("exited") {
            ContainerState::Exited
        } else if st.starts_with("created") {
            ContainerState::Created
        } else if st.starts_with("restarting") {
            ContainerState::Restarting
        } else {
            ContainerState::Unknown
        };
    }
    let health = meaningful(&text(v, "HealthStatus"))
        .and_then(Health::parse)
        .or_else(|| health_from_status(&status_text));
    let exit_code = exit_code_from_status(&status_text);
    let name = text(v, "Names")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('/')
        .to_owned();
    let networks = text(v, "Networks")
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    let mounts = text(v, "Mounts")
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| MountSummary {
            kind: if looks_like_path(m) {
                MountKind::Bind
            } else {
                MountKind::Volume
            },
            source: m.to_owned(),
            destination: String::new(),
            rw: true,
        })
        .collect();
    let size_s = text(v, "Size");
    let size_rw = parse_size(&size_s);
    let size_root_fs = size_s
        .split_once("(virtual")
        .and_then(|(_, r)| parse_size(r.trim().trim_end_matches(')')));
    ContainerSummary {
        id: text(v, "ID"),
        name,
        image: text(v, "Image"),
        image_id: text(v, "ImageID"),
        command: unquote(&text(v, "Command")),
        created: parse_time(&text(v, "CreatedAt")).unwrap_or(OffsetDateTime::UNIX_EPOCH),
        state,
        status_text,
        health,
        exit_code,
        ports: parse_ports(&text(v, "Ports")),
        labels: labels(field(v, "Labels")),
        networks,
        ip_addresses: Vec::new(),
        mounts,
        size_rw,
        size_root_fs,
        compose: None,
    }
}

// ───────────────────────────── images ─────────────────────────────

/// `image list --format json --no-trunc` rows → summaries, merging rows of the same image id
/// (wslc prints one row per tag, like `docker images`). Order of first appearance is kept.
pub(crate) fn image_summaries(rows: &[Value]) -> Vec<ImageSummary> {
    let mut out: Vec<ImageSummary> = Vec::new();
    for v in rows {
        let id = text(v, "ID");
        if id.is_empty() {
            continue;
        }
        let repo = text(v, "Repository");
        let tag = text(v, "Tag");
        let digest = text(v, "Digest");
        let repo_tag = match (meaningful(&repo), meaningful(&tag)) {
            (Some(r), Some(t)) => Some(format!("{r}:{t}")),
            _ => None,
        };
        let repo_digest = match (meaningful(&repo), meaningful(&digest)) {
            (Some(r), Some(d)) => Some(format!("{r}@{d}")),
            _ => None,
        };
        if let Some(existing) = out.iter_mut().find(|i| i.id == id) {
            if let Some(rt) = repo_tag {
                if !existing.repo_tags.contains(&rt) {
                    existing.repo_tags.push(rt);
                }
            }
            if let Some(rd) = repo_digest {
                if !existing.repo_digests.contains(&rd) {
                    existing.repo_digests.push(rd);
                }
            }
            existing.dangling = existing.repo_tags.is_empty();
            continue;
        }
        let repo_tags: Vec<String> = repo_tag.into_iter().collect();
        out.push(ImageSummary {
            dangling: repo_tags.is_empty(),
            id,
            repo_tags,
            repo_digests: repo_digest.into_iter().collect(),
            created: parse_time(&text(v, "CreatedAt")).unwrap_or(OffsetDateTime::UNIX_EPOCH),
            size: parse_size(&text(v, "Size"))
                .or_else(|| parse_size(&text(v, "VirtualSize")))
                .unwrap_or(0),
            shared_size: parse_size(&text(v, "SharedSize")),
            containers: meaningful(&text(v, "Containers")).and_then(|c| c.parse().ok()),
            labels: labels(field(v, "Labels")),
        });
    }
    out
}

/// `image remove` stdout: `Untagged: ref` / `Deleted: sha256:…` lines.
pub(crate) fn image_delete_items(stdout: &str) -> Vec<ImageDeleteItem> {
    stdout
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let v = v.trim().to_owned();
            match k.trim().to_ascii_lowercase().as_str() {
                "untagged" => Some(ImageDeleteItem::Untagged(v)),
                "deleted" => Some(ImageDeleteItem::Deleted(v)),
                _ => None,
            }
        })
        .collect()
}

/// One line of `image pull` stdout. Lines are text only on the CLI (no PULL_PROGRESS).
/// Returns `(progress, digest)` — the digest from `Digest: sha256:…` lines.
pub(crate) fn pull_line(line: &str) -> Option<(PullProgress, Option<String>)> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    let digest = l
        .strip_prefix("Digest:")
        .map(str::trim)
        .filter(|d| d.contains(':'))
        .map(str::to_owned);
    Some((PullProgress::Status(l.to_owned()), digest))
}

// ───────────────────────────── volumes / networks ─────────────────────────────

/// `volume list --format json` entry. `created` isn't in the list (filled from `inspect`).
pub(crate) fn volume_summary(v: &Value) -> VolumeSummary {
    VolumeSummary {
        name: text(v, "Name"),
        driver: text(v, "Driver"),
        mountpoint: text(v, "Mountpoint"),
        created: parse_time(&text(v, "CreatedAt")),
        scope: text(v, "Scope"),
        labels: labels(field(v, "Labels")),
        size: parse_size(&text(v, "Size")),
        ref_count: meaningful(&text(v, "Links")).and_then(|l| l.parse().ok()),
        compose: None,
    }
}

/// Fill fields missing from `volume list` with a Docker-shaped `volume inspect` object.
pub(crate) fn enrich_volume(s: &mut VolumeSummary, inspect: &Value) {
    if s.created.is_none() {
        s.created = parse_time(&text(inspect, "CreatedAt"));
    }
    if s.labels.is_empty() {
        s.labels = labels(field(inspect, "Labels"));
    }
    if s.mountpoint.is_empty() {
        s.mountpoint = text(inspect, "Mountpoint");
    }
}

/// `network list --format json --no-trunc` entry. Subnets/attachable/containers come from
/// `inspect` ([`enrich_network`]).
pub(crate) fn network_summary(v: &Value) -> NetworkSummary {
    NetworkSummary {
        id: text(v, "ID"),
        name: text(v, "Name"),
        driver: text(v, "Driver"),
        scope: text(v, "Scope"),
        internal: bool_field(v, "Internal"),
        attachable: bool_field(v, "Attachable"),
        ipv6: bool_field(v, "IPv6"),
        created: parse_time(&text(v, "CreatedAt")).unwrap_or(OffsetDateTime::UNIX_EPOCH),
        subnets: Vec::new(),
        labels: labels(field(v, "Labels")),
        compose: None,
        containers: None,
    }
}

/// Fill subnets, attachable, created and the container count from a Docker-shaped
/// `network inspect` object.
pub(crate) fn enrich_network(s: &mut NetworkSummary, inspect: &Value) {
    if let Some(cfgs) = field(inspect, "IPAM")
        .and_then(|i| field(i, "Config"))
        .and_then(Value::as_array)
    {
        s.subnets = cfgs
            .iter()
            .map(|c| IpamConfig {
                subnet: meaningful(&text(c, "Subnet")).map(str::to_owned),
                gateway: meaningful(&text(c, "Gateway")).map(str::to_owned),
                ip_range: meaningful(&text(c, "IPRange")).map(str::to_owned),
            })
            .collect();
    }
    s.attachable = bool_field(inspect, "Attachable");
    if let Some(t) = parse_time(&text(inspect, "Created")) {
        s.created = t;
    }
    s.containers = field(inspect, "Containers")
        .and_then(Value::as_object)
        .map(|o| o.len() as u32)
        .or(Some(0));
    if s.labels.is_empty() {
        s.labels = labels(field(inspect, "Labels"));
    }
}

// ───────────────────────────── system ─────────────────────────────

/// `version --format json` (`{"Client":{"Version":"3.0.1.0"}}`) or text (`wslc 3.0.1.0`).
pub(crate) fn version(s: &str) -> Option<String> {
    let t = s.trim();
    if t.starts_with('{') {
        let v: Value = serde_json::from_str(t).ok()?;
        let ver = field(&v, "Client")
            .map(|c| text(c, "Version"))
            .unwrap_or_else(|| text(&v, "Version"));
        return meaningful(&ver).map(str::to_owned);
    }
    t.lines()
        .flat_map(str::split_whitespace)
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
        .map(str::to_owned)
}

/// Facts from `system info --format json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SystemInfo {
    pub client_version: Option<String>,
    pub kernel: Option<String>,
    pub windows_version: Option<String>,
    pub session_manager_version: Option<String>,
    pub sessions: Vec<SessionRow>,
}

pub(crate) fn system_info(s: &str) -> EngineResult<SystemInfo> {
    let v: Value = serde_json::from_str(s.trim())?;
    let client = field(&v, "Client").cloned().unwrap_or(Value::Null);
    let server = field(&v, "Server").cloned().unwrap_or(Value::Null);
    let opt = |v: &Value, k: &str| meaningful(&text(v, k)).map(str::to_owned);
    let sessions = field(&server, "Sessions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| SessionRow {
                    id: meaningful(&text(s, "ID")).and_then(|i| i.parse().ok()),
                    creator_pid: meaningful(&text(s, "CreatorPid")).and_then(|i| i.parse().ok()),
                    name: text(s, "Name"),
                })
                .filter(|s| !s.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(SystemInfo {
        client_version: opt(&client, "Version"),
        kernel: opt(&client, "KernelVersion"),
        windows_version: opt(&client, "WindowsVersion"),
        session_manager_version: opt(&server, "SessionManagerVersion"),
        sessions,
    })
}

/// One row of `system session list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRow {
    pub id: Option<u32>,
    pub creator_pid: Option<u32>,
    pub name: String,
}

/// `system session list` table (no JSON in 3.0.1): columns `ID`, `Creator PID`,
/// `Display Name`, fixed width. Parsed by header column offsets; lines before the header
/// (`[wslc] Found 2 sessions` with `--verbose`) are skipped. Without a recognisable header,
/// falls back to whitespace splitting (`id pid name…`).
pub(crate) fn session_table(s: &str) -> Vec<SessionRow> {
    let lines: Vec<Vec<char>> = s.lines().map(|l| l.trim_end().chars().collect()).collect();
    let find = |hay: &[char], needle: &str| -> Option<usize> {
        let n: Vec<char> = needle.chars().collect();
        hay.windows(n.len()).position(|w| w == n.as_slice())
    };
    let header = lines
        .iter()
        .position(|l| find(l, "Display Name").is_some() && find(l, "ID").is_some());
    let slice = |l: &[char], a: usize, b: usize| -> String {
        let a = a.min(l.len());
        let b = b.min(l.len()).max(a);
        l[a..b].iter().collect::<String>().trim().to_owned()
    };
    let mut out = Vec::new();
    match header {
        Some(h) => {
            let hl = &lines[h];
            let id_off = find(hl, "ID").unwrap_or(0);
            let name_off = find(hl, "Display Name").unwrap_or(hl.len());
            let pid_off = find(hl, "Creator PID").unwrap_or(name_off);
            for l in lines.iter().skip(h + 1) {
                if l.iter().all(|c| c.is_whitespace()) {
                    continue;
                }
                let name = slice(l, name_off, l.len());
                if name.is_empty() {
                    continue;
                }
                out.push(SessionRow {
                    id: slice(l, id_off, pid_off).parse().ok(),
                    creator_pid: slice(l, pid_off, name_off).parse().ok(),
                    name,
                });
            }
        }
        None => {
            for l in s.lines() {
                let mut it = l.split_whitespace();
                let (Some(id), Some(pid)) = (it.next(), it.next()) else {
                    continue;
                };
                let name = it.collect::<Vec<_>>().join(" ");
                if let (Ok(id), false) = (id.parse(), name.is_empty()) {
                    out.push(SessionRow {
                        id: Some(id),
                        creator_pid: pid.parse().ok(),
                        name,
                    });
                }
            }
        }
    }
    out
}

/// `container|network|volume|image prune -f` stdout:
/// ```text
/// Deleted Containers:
/// <id>
///
/// Total reclaimed space: 0B
/// ```
/// Image prune lines may be prefixed `untagged:` / `deleted:`; both are reported.
pub(crate) fn prune_report(stdout: &str) -> PruneReport {
    let mut r = PruneReport::default();
    let mut in_list = false;
    for l in stdout.lines().map(str::trim) {
        if l.is_empty() {
            in_list = false;
            continue;
        }
        if let Some(rest) = l.strip_prefix("Total reclaimed space:") {
            r.space_reclaimed = parse_size(rest).unwrap_or(0);
            in_list = false;
            continue;
        }
        if l.starts_with("Deleted") && l.ends_with(':') {
            in_list = true;
            continue;
        }
        if in_list {
            let item = l
                .split_once(':')
                .filter(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "deleted" | "untagged"))
                .map_or(l, |(_, v)| v.trim());
            r.deleted.push(item.to_owned());
        }
    }
    r
}

// ───────────────────────────── events ─────────────────────────────

fn resource_kind(s: &str) -> Option<ResourceKind> {
    match s.to_ascii_lowercase().as_str() {
        "container" => Some(ResourceKind::Container),
        "image" => Some(ResourceKind::Image),
        "volume" => Some(ResourceKind::Volume),
        "network" => Some(ResourceKind::Network),
        "daemon" => Some(ResourceKind::Daemon),
        _ => None,
    }
}

/// One `system events` line (no JSON in 3.0.1, F-10):
/// `2026-10-02T01:15:51.000000000+02:00 container start <id> (k=v, k2=v2)`.
/// The id may be empty (`network prune  (reclaimed=0)`). Unknown types/garbage → None.
pub(crate) fn event_line(line: &str) -> Option<EngineEvent> {
    let l = line.trim();
    let (ts, rest) = l.split_once(char::is_whitespace)?;
    let at = parse_time(ts)?;
    let rest = rest.trim_start();
    let (typ, rest) = rest.split_once(char::is_whitespace)?;
    let kind = resource_kind(typ)?;
    let rest = rest.trim_start();
    let (action, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if action.is_empty() {
        return None;
    }
    let rest = rest.trim_start();
    let (id, attrs) = if rest.starts_with('(') {
        ("", rest)
    } else {
        rest.split_once(char::is_whitespace).unwrap_or((rest, ""))
    };
    let attrs = attrs.trim();
    let attributes = attrs
        .strip_prefix('(')
        .and_then(|a| a.strip_suffix(')'))
        .map(parse_kv_list)
        .unwrap_or_default();
    Some(EngineEvent {
        at,
        kind,
        action: action.to_owned(),
        id: id.to_owned(),
        attributes,
    })
}

// ───────────────────────────── logs ─────────────────────────────

/// Split a leading RFC 3339 timestamp (`logs --timestamps`) from a log line.
/// Returns `(None, line)` when the line doesn't start with one.
pub(crate) fn split_log_timestamp(line: &[u8]) -> (Option<OffsetDateTime>, &[u8]) {
    let Some(sp) = line.iter().position(|b| *b == b' ') else {
        // A timestamp-only line (empty message).
        let ts = std::str::from_utf8(line).ok().map(str::trim_end);
        return match ts.and_then(parse_rfc3339) {
            Some(t) => (Some(t), &line[line.len()..]),
            None => (None, line),
        };
    };
    match std::str::from_utf8(&line[..sp])
        .ok()
        .and_then(parse_rfc3339)
    {
        Some(t) => (Some(t), &line[sp + 1..]),
        None => (None, line),
    }
}

fn parse_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.to_offset(UtcOffset::UTC))
}

// ───────────────────────────── stats ─────────────────────────────

/// `container stats <id> --format json` in the Docker CLI string format (wslc 3.0.1):
/// `{"BlockIO":"0B / 4.1kB","CPUPerc":"0.00%","MemUsage":"17.02MiB / 15.53GiB",
///   "NetIO":"1.16kB / 0B","PIDs":21,…}`. Totals are cumulative.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CliStats {
    pub cpu_percent: f64,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub blk_read: u64,
    pub blk_write: u64,
    pub pids: Option<u64>,
}

pub(crate) fn cli_stats(v: &Value) -> CliStats {
    let (mem_used, mem_limit) = parse_pair(&text(v, "MemUsage"));
    let (net_rx, net_tx) = parse_pair(&text(v, "NetIO"));
    let (blk_read, blk_write) = parse_pair(&text(v, "BlockIO"));
    CliStats {
        cpu_percent: parse_percent(&text(v, "CPUPerc")).unwrap_or(0.0).max(0.0),
        mem_used: mem_used.unwrap_or(0),
        mem_limit: mem_limit.unwrap_or(0),
        net_rx: net_rx.unwrap_or(0),
        net_tx: net_tx.unwrap_or(0),
        blk_read: blk_read.unwrap_or(0),
        blk_write: blk_write.unwrap_or(0),
        pids: meaningful(&text(v, "PIDs")).and_then(|p| p.parse().ok()),
    }
}

/// Docker-API-shaped stats (`cpu_stats`, …) vs the CLI string format.
pub(crate) fn is_docker_stats(v: &Value) -> bool {
    v.get("cpu_stats").is_some() || v.get("memory_stats").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    macro_rules! fx {
        ($p:literal) => {
            include_str!(concat!("../../tests/fixtures/cli/", $p))
        };
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("4.1kB"), Some(4100));
        assert_eq!(parse_size("62.9MB"), Some(62_900_000));
        assert_eq!(parse_size("1.2MB"), Some(1_200_000));
        assert_eq!(parse_size("17.02MiB"), Some(17_846_764));
        assert_eq!(parse_size("3.4GiB"), Some(3_650_722_202));
        assert_eq!(parse_size("1.5 GB"), Some(1_500_000_000));
        assert_eq!(parse_size("0B (virtual 4.42MB)"), Some(0));
        assert_eq!(parse_size("N/A"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size("12XB"), None);
        assert_eq!(parse_pair("17.02MiB / 15.53GiB").1, Some(16_675_210_527));
        assert_eq!(parse_percent("0.15%"), Some(0.15));
        assert_eq!(parse_percent("--"), None);
    }

    #[test]
    fn times() {
        assert_eq!(
            parse_time("2026-10-02 01:09:38 +0200 SELČ"),
            Some(datetime!(2026-10-01 23:09:38 UTC))
        );
        assert_eq!(
            parse_time("2026-10-01 23:09:11.462461977 +0000 UTC"),
            Some(datetime!(2026-10-01 23:09:11.462461977 UTC))
        );
        assert_eq!(
            parse_time("2026-10-02T01:15:51.000000000+02:00"),
            Some(datetime!(2026-10-01 23:15:51 UTC))
        );
        assert_eq!(
            parse_time("2026-10-01 10:00:00"),
            Some(datetime!(2026-10-01 10:00 UTC))
        );
        assert_eq!(
            parse_time("2026-10-01 10:00:00 -05:30 IST"),
            Some(datetime!(2026-10-01 15:30 UTC))
        );
        assert_eq!(
            parse_time("1759360000"),
            OffsetDateTime::from_unix_timestamp(1759360000).ok()
        );
        assert_eq!(parse_time("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn kv_lists_keep_json_and_commas() {
        let m = parse_kv_list(
            r#"a=1,com.microsoft.wsl.container.metadata={"V1":{"Flags":0,"Ports":[{"x":1,"y":"a=b"}]}},files=/a.yml,/b.yml,m=NGINX <x@y.z>"#,
        );
        assert_eq!(m.len(), 4, "{m:?}");
        assert_eq!(m["a"], "1");
        assert!(m[WSL_METADATA_LABEL].ends_with("}]}}"));
        assert_eq!(m["files"], "/a.yml,/b.yml");
        assert_eq!(m["m"], "NGINX <x@y.z>");
        let m = parse_kv_list("container=abc, name=bridge, type=bridge");
        assert_eq!(m.len(), 3);
        assert_eq!(m["name"], "bridge");
        assert!(parse_kv_list("").is_empty());
        assert_eq!(parse_kv_list("flag")["flag"], "");
    }

    #[test]
    fn ports() {
        let p =
            parse_ports("127.0.0.1:18095->80/tcp, 127.0.0.1:18097->53/udp, 0.0.0.0:18098->443/tcp");
        assert_eq!(p.len(), 3);
        assert_eq!(
            p[1],
            PortMapping {
                ip: Some("127.0.0.1".parse().unwrap_or(IpAddr::from([0, 0, 0, 0]))),
                private: 53,
                public: Some(18097),
                proto: Proto::Udp
            }
        );
        let p =
            parse_ports("[::]:8080->80/tcp, :::8081->81/tcp, 9000/tcp, 8000-8001->80-81/tcp, junk");
        assert_eq!(p.len(), 5, "{p:?}");
        assert_eq!(p[0].ip, "::".parse().ok());
        assert_eq!(p[1].public, Some(8081));
        assert_eq!((p[2].private, p[2].public, p[2].ip), (9000, None, None));
        assert_eq!((p[3].private, p[3].public), (80, Some(8000)));
        assert_eq!((p[4].private, p[4].public), (81, Some(8001)));
        assert!(parse_ports("").is_empty());
    }

    #[test]
    fn container_list_fixture() {
        let rows = json_list(fx!("container_list/list.ndjson")).expect("ndjson");
        assert_eq!(rows.len(), 2);
        let s: Vec<_> = rows.iter().map(container_summary).collect();
        let exited = s
            .iter()
            .find(|c| c.name == "dk-cli-exited")
            .expect("exited");
        assert_eq!(exited.state, ContainerState::Exited);
        assert_eq!(exited.exit_code, Some(3));
        assert_eq!(exited.image, "nginx:alpine");
        assert!(
            exited.command.starts_with("/docker-entrypoint.sh sh -c"),
            "{}",
            exited.command
        );
        assert_eq!(exited.id.len(), 64);
        let web = s.iter().find(|c| c.name == "dk-cli-fixture").expect("web");
        assert_eq!(web.state, ContainerState::Running);
        assert_eq!(web.created, datetime!(2026-10-01 23:15:51 UTC));
        assert_eq!(web.ports.len(), 1);
        assert_eq!(
            (web.ports[0].private, web.ports[0].public),
            (80, Some(18090))
        );
        assert_eq!(web.labels["com.docker.compose.project"], "dkcli");
        assert_eq!(
            web.labels["maintainer"],
            "NGINX Docker Maintainers <docker-maint@nginx.com>"
        );
        assert!(!web.labels.contains_key(WSL_METADATA_LABEL));
        assert_eq!(web.networks, ["bridge"]);
        assert_eq!(web.mounts.len(), 1);
        assert_eq!(
            (web.mounts[0].kind, web.mounts[0].source.as_str()),
            (MountKind::Volume, "dk-cli-vol")
        );
        assert_eq!(web.size_rw, Some(0));
        assert_eq!(web.health, None);
        assert_eq!(web.compose, None);
    }

    #[test]
    fn container_entry_variants() {
        let v: Value = serde_json::json!({
            "ID": "abc", "Names": "/web,/other", "Status": "Up 3 minutes (health: starting)",
            "Size": "1.2MB (virtual 4.42MB)", "Labels": {"k": "v"}, "Mounts": "C:\\data,/srv"
        });
        let c = container_summary(&v);
        assert_eq!(c.name, "web");
        assert_eq!(c.state, ContainerState::Running);
        assert_eq!(c.health, Some(Health::Starting));
        assert_eq!(
            (c.size_rw, c.size_root_fs),
            (Some(1_200_000), Some(4_420_000))
        );
        assert_eq!(c.labels["k"], "v");
        assert!(c.mounts.iter().all(|m| m.kind == MountKind::Bind));
        assert_eq!(c.created, OffsetDateTime::UNIX_EPOCH);
        let c = container_summary(
            &serde_json::json!({"State": "running", "Status": "Up 1 second (unhealthy)", "HealthStatus": "unhealthy"}),
        );
        assert_eq!(c.health, Some(Health::Unhealthy));
        assert_eq!(
            container_summary(&Value::Null).state,
            ContainerState::Unknown
        );
    }

    #[test]
    fn json_list_shapes() {
        assert!(json_list("").expect("empty").is_empty());
        assert!(json_list("\r\n").expect("blank").is_empty());
        assert_eq!(json_list(r#"[{"a":1},{"a":2}]"#).expect("array").len(), 2);
        assert_eq!(
            json_list("{\"a\":1}\r\n\r\n{\"a\":2}\r\nnot json\r\n")
                .expect("nd")
                .len(),
            2
        );
        assert!(json_list("garbage").is_err());
        assert_eq!(
            text(
                &first_object(fx!("container_inspect/inspect.json")).expect("obj"),
                "Name"
            ),
            "/dk-cli-fixture"
        );
        assert!(first_object("[]").is_err());
    }

    #[test]
    fn image_list_fixture_and_merge() {
        let rows = json_list(fx!("image_list/list.ndjson")).expect("ndjson");
        let imgs = image_summaries(&rows);
        assert_eq!(imgs.len(), 1);
        let i = &imgs[0];
        assert_eq!(i.repo_tags, ["nginx:alpine"]);
        assert!(i.repo_digests.is_empty());
        assert_eq!(i.size, 62_900_000);
        assert_eq!(i.shared_size, None);
        assert_eq!(i.containers, Some(3));
        assert_eq!(i.created, datetime!(2026-09-22 22:10:17 UTC));
        assert!(!i.dangling);
        let rows = json_list(concat!(
            r#"{"ID":"sha256:aa","Repository":"a/b","Tag":"t","Digest":"<none>","Size":"4.42MB","Containers":"0"}"#, "\n",
            r#"{"ID":"sha256:aa","Repository":"busybox","Tag":"1.37","Digest":"sha256:dd","Size":"4.42MB","Containers":"0"}"#, "\n",
            r#"{"ID":"sha256:bb","Repository":"<none>","Tag":"<none>","Digest":"<none>","Size":"1kB","Containers":"N/A"}"#
        )).expect("rows");
        let imgs = image_summaries(&rows);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].repo_tags, ["a/b:t", "busybox:1.37"]);
        assert_eq!(imgs[0].repo_digests, ["busybox@sha256:dd"]);
        assert!(imgs[1].dangling && imgs[1].containers.is_none());
    }

    #[test]
    fn image_remove_and_pull() {
        let items = image_delete_items(fx!("image_remove/delete.stdout"));
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], ImageDeleteItem::Untagged("busybox:1.37".into()));
        assert!(matches!(&items[2], ImageDeleteItem::Deleted(d) if d.starts_with("sha256:30ec")));
        let lines: Vec<_> = fx!("pull/pull.stdout")
            .lines()
            .filter_map(pull_line)
            .collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(
            lines[0].0,
            PullProgress::Status("1.37: Pulling from library/busybox".into())
        );
        let digests: Vec<_> = lines.iter().filter_map(|(_, d)| d.clone()).collect();
        assert_eq!(
            digests,
            ["sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e"]
        );
        assert!(pull_line("\r").is_none());
    }

    #[test]
    fn volumes_and_networks() {
        let rows = json_list(fx!("volume_list/list.ndjson")).expect("ndjson");
        let mut v = volume_summary(&rows[0]);
        assert_eq!(
            (v.name.as_str(), v.driver.as_str(), v.scope.as_str()),
            ("dk-cli-vol", "guest", "local")
        );
        assert_eq!(v.labels["com.example.fixture"], "dk-cli");
        assert_eq!((v.size, v.ref_count, v.created), (None, None, None));
        enrich_volume(
            &mut v,
            &first_object(fx!("volume_inspect/inspect.json")).expect("obj"),
        );
        assert_eq!(v.created, Some(datetime!(2026-10-01 23:15:51 UTC)));

        let rows = json_list(fx!("network_list/list.ndjson")).expect("ndjson");
        let nets: Vec<_> = rows.iter().map(network_summary).collect();
        let mut n = nets
            .iter()
            .find(|n| n.name == "dk-cli-net")
            .cloned()
            .expect("net");
        assert_eq!(
            (n.driver.as_str(), n.internal, n.ipv6),
            ("bridge", false, false)
        );
        assert_eq!(n.labels["com.docker.compose.project"], "dkcli");
        assert!(nets.iter().any(|n| n.is_builtin()));
        let bridge = nets.iter().find(|n| n.name == "bridge").expect("bridge");
        assert_eq!(bridge.created, datetime!(2026-10-01 23:09:11.462461977 UTC));
        enrich_network(
            &mut n,
            &first_object(fx!("network_inspect/inspect.json")).expect("obj"),
        );
        assert_eq!(
            n.subnets,
            [IpamConfig {
                subnet: Some("172.19.0.0/16".into()),
                gateway: Some("172.19.0.1".into()),
                ip_range: None
            }]
        );
        assert_eq!(n.containers, Some(0));
        let mut b = bridge.clone();
        enrich_network(
            &mut b,
            &first_object(fx!("network_inspect/inspect_bridge.json")).expect("obj"),
        );
        assert!(b.containers.unwrap_or(0) >= 1);
    }

    #[test]
    fn system_fixtures() {
        assert_eq!(
            version(fx!("version/version.json")).as_deref(),
            Some("3.0.1.0")
        );
        assert_eq!(
            version(fx!("version/version.txt")).as_deref(),
            Some("3.0.1.0")
        );
        assert_eq!(version("garbage"), None);
        let i = system_info(fx!("system_info/info.json")).expect("info");
        assert_eq!(i.client_version.as_deref(), Some("3.0.1.0"));
        assert_eq!(i.kernel.as_deref(), Some("6.18.40.1-1"));
        assert_eq!(i.session_manager_version.as_deref(), Some("3.0.1"));
        assert_eq!(i.sessions.len(), 2);
        assert_eq!(
            i.sessions[1],
            SessionRow {
                id: Some(2),
                creator_pid: Some(34112),
                name: "wslc-cli-user".into()
            }
        );
        assert!(system_info("nope").is_err());
    }

    #[test]
    fn session_tables() {
        let rows = session_table(fx!("session_list/sessions.txt"));
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            SessionRow {
                id: Some(1),
                creator_pid: Some(19636),
                name: "wslc-cli-admin-user".into()
            }
        );
        let verbose = "[wslc] Found 1 sessions\r\nID   Creator PID   Display Name\r\n7    1             My Session Ü\r\n\r\n";
        assert_eq!(
            session_table(verbose),
            [SessionRow {
                id: Some(7),
                creator_pid: Some(1),
                name: "My Session Ü".into()
            }]
        );
        assert_eq!(
            session_table("3 44 name with space")[0].name,
            "name with space"
        );
        assert!(session_table("").is_empty());
    }

    #[test]
    fn prune_reports() {
        let r = prune_report(fx!("prune/containers.stdout"));
        assert_eq!(
            r.deleted,
            ["e2bdd5746101d366e60c0f19d4a5703d2a0898176ff47ce044ea626dd3534ada"]
        );
        assert_eq!(r.space_reclaimed, 0);
        assert_eq!(
            prune_report(fx!("prune/networks.stdout")).deleted,
            ["dk-cli-net"]
        );
        assert!(prune_report(fx!("prune/volumes.stdout")).deleted.is_empty());
        let r = prune_report(
            "Deleted Images:\nuntagged: a:1\ndeleted: sha256:abc\n\nTotal reclaimed space: 1.5MB\n",
        );
        assert_eq!(r.deleted, ["a:1", "sha256:abc"]);
        assert_eq!(r.space_reclaimed, 1_500_000);
    }

    #[test]
    fn events_fixture() {
        let text = fx!("events/events.txt");
        let evs: Vec<_> = text.lines().filter_map(event_line).collect();
        assert_eq!(
            evs.len(),
            text.lines().filter(|l| !l.trim().is_empty()).count()
        );
        let e = &evs[0];
        assert_eq!(
            (e.kind, e.action.as_str()),
            (ResourceKind::Container, "create")
        );
        assert_eq!(e.at, datetime!(2026-10-01 23:15:51 UTC));
        assert_eq!(e.id.len(), 64);
        assert_eq!(e.attributes["name"], "dk-cli-fixture");
        assert_eq!(e.attributes["image"], "nginx:alpine");
        assert_eq!(
            e.attributes["maintainer"],
            "NGINX Docker Maintainers <docker-maint@nginx.com>"
        );
        let prune = evs.iter().find(|e| e.action == "prune").expect("prune");
        assert_eq!((prune.kind, prune.id.as_str()), (ResourceKind::Network, ""));
        assert_eq!(prune.attributes["reclaimed"], "0");
        let stop = evs.iter().find(|e| e.action == "stop").expect("stop");
        assert_eq!(stop.attributes["exitCode"], "0");
        let conn = evs.iter().find(|e| e.action == "connect").expect("connect");
        assert_eq!(conn.attributes["name"], "bridge");
        assert!(event_line("").is_none());
        assert!(event_line("not a timestamp container start x").is_none());
        assert!(event_line("2026-10-02T01:15:51Z plugin enable x").is_none());
        let bare = event_line("2026-10-02T01:15:51Z image pull nginx:alpine").expect("bare");
        assert_eq!(
            (bare.id.as_str(), bare.attributes.len()),
            ("nginx:alpine", 0)
        );
    }

    #[test]
    fn log_timestamps() {
        let (ts, rest) = split_log_timestamp(b"2026-10-01T23:16:32.926967408Z out-line");
        assert_eq!(ts, Some(datetime!(2026-10-01 23:16:32.926967408 UTC)));
        assert_eq!(rest, b"out-line");
        let (ts, rest) = split_log_timestamp(b"2026/10/01 23:15:59 [notice] x");
        assert_eq!((ts, rest), (None, &b"2026/10/01 23:15:59 [notice] x"[..]));
        let (ts, rest) = split_log_timestamp(b"2026-10-01T23:16:32Z");
        assert!(ts.is_some() && rest.is_empty());
        assert_eq!(split_log_timestamp(b"").0, None);
    }

    #[test]
    fn stats_fixture() {
        let v = first_object(fx!("container_stats/stats.json")).expect("obj");
        assert!(!is_docker_stats(&v));
        let s = cli_stats(&v);
        assert_eq!(s.pids, Some(21));
        assert!(
            s.mem_used == 16_368_271 && s.mem_limit > 16_000_000_000,
            "{s:?}"
        );
        assert_eq!(s.net_tx, 0);
        assert!(s.net_rx > 1000);
        assert_eq!(s.blk_write, 4100);
        let v = first_object(fx!("container_stats/stats_stopped.json")).expect("obj");
        assert_eq!(
            cli_stats(&v),
            CliStats {
                pids: Some(0),
                ..CliStats::default()
            }
        );
        assert!(is_docker_stats(&serde_json::json!({"cpu_stats": {}})));
    }
}
