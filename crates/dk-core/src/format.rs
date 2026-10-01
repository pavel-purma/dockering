//! Human formatting helpers (SHL-007, SHL-008). SIGNATURES FIXED — bodies by `rust-core`.

use time::{Duration, OffsetDateTime};

use crate::model::PortMapping;

/// SI units like Docker Desktop: `0 B`, `999 B`, `1.2 kB`, `12.3 MB`, `1.2 GB`.
pub fn format_size(bytes: u64) -> String {
    let _ = bytes;
    unimplemented!("rust-core")
}

/// `1.2 MB/s`
pub fn format_rate(bytes_per_s: f64) -> String {
    let _ = bytes_per_s;
    unimplemented!("rust-core")
}

/// `3.7%` (one decimal; `0%` for 0).
pub fn format_percent(p: f64) -> String {
    let _ = p;
    unimplemented!("rust-core")
}

/// `just now`, `12 seconds ago`, `2 minutes ago`, `2 hours ago`, `3 days ago`,
/// `2 months ago`, `1 year ago`. Future timestamps → `just now`.
pub fn format_relative(then: OffsetDateTime, now: OffsetDateTime) -> String {
    let _ = (then, now);
    unimplemented!("rust-core")
}

/// `2 hours`, `3 minutes`, `5 seconds` (docker-style, largest unit).
pub fn format_duration(d: Duration) -> String {
    let _ = d;
    unimplemented!("rust-core")
}

/// `2026-10-02 14:03:11 UTC`
pub fn format_timestamp(t: OffsetDateTime) -> String {
    let _ = t;
    unimplemented!("rust-core")
}

/// First 12 chars, stripping a `sha256:` prefix.
pub fn short_id(id: &str) -> &str {
    let _ = id;
    unimplemented!("rust-core")
}

/// `8080:80/tcp` when published (with `ip:` prefix unless unspecified), `80/tcp` otherwise.
pub fn format_port(p: &PortMapping) -> String {
    let _ = p;
    unimplemented!("rust-core")
}

/// Split `repo:tag` (handles registry ports and digests): `("localhost:5000/app", "1.0")`.
/// `<none>` for missing parts.
pub fn split_repo_tag(reference: &str) -> (String, String) {
    let _ = reference;
    unimplemented!("rust-core")
}
