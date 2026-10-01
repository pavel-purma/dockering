//! Human formatting helpers (SHL-007, SHL-008).

use time::{Duration, OffsetDateTime};

use crate::model::PortMapping;

/// SI units like Docker Desktop: `0 B`, `999 B`, `1.2 kB`, `12.3 MB`, `1.2 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["kB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = "B";
    for u in UNITS {
        v /= 1000.0;
        unit = u;
        if v < 999.95 {
            break;
        }
    }
    if v < 10.0 {
        format!("{v:.2} {unit}")
    } else if v < 100.0 {
        format!("{v:.1} {unit}")
    } else {
        format!("{v:.0} {unit}")
    }
}

/// `1.2 MB/s`
pub fn format_rate(bytes_per_s: f64) -> String {
    let b = if bytes_per_s.is_finite() && bytes_per_s > 0.0 {
        bytes_per_s.round() as u64
    } else {
        0
    };
    format!("{}/s", format_size(b))
}

/// `3.7%` (one decimal; `0%` for 0).
pub fn format_percent(p: f64) -> String {
    if !p.is_finite() || p <= 0.0 {
        "0%".into()
    } else if p < 0.05 {
        "<0.1%".into()
    } else {
        format!("{p:.1}%")
    }
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

/// `just now`, `12 seconds ago`, `2 minutes ago`, `2 hours ago`, `3 days ago`,
/// `2 months ago`, `1 year ago`. Future timestamps → `just now`.
pub fn format_relative(then: OffsetDateTime, now: OffsetDateTime) -> String {
    let d = now - then;
    if d < Duration::seconds(5) {
        return "just now".into();
    }
    format!("{} ago", format_duration(d))
}

/// `2 hours`, `3 minutes`, `5 seconds` (docker-style, largest unit).
pub fn format_duration(d: Duration) -> String {
    let s = d.whole_seconds().max(0);
    match s {
        0..60 => plural(s, "second"),
        60..3600 => plural(s / 60, "minute"),
        3600..86_400 => plural(s / 3600, "hour"),
        86_400..2_592_000 => plural(s / 86_400, "day"),
        2_592_000..31_536_000 => plural(s / 2_592_000, "month"),
        _ => plural(s / 31_536_000, "year"),
    }
}

/// `2026-10-02 14:03:11 UTC`
pub fn format_timestamp(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// First 12 chars, stripping a `sha256:` prefix.
pub fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    match id.char_indices().nth(12) {
        Some((i, _)) => &id[..i],
        None => id,
    }
}

/// `8080:80/tcp` when published (with `ip:` prefix unless unspecified), `80/tcp` otherwise.
pub fn format_port(p: &PortMapping) -> String {
    match p.public {
        Some(public) => match p.ip {
            Some(ip) if !ip.is_unspecified() => {
                if ip.is_ipv6() {
                    format!("[{ip}]:{public}:{}/{}", p.private, p.proto)
                } else {
                    format!("{ip}:{public}:{}/{}", p.private, p.proto)
                }
            }
            _ => format!("{public}:{}/{}", p.private, p.proto),
        },
        None => format!("{}/{}", p.private, p.proto),
    }
}

/// Split `repo:tag` (handles registry ports and digests): `("localhost:5000/app", "1.0")`.
/// `<none>` for missing parts.
pub fn split_repo_tag(reference: &str) -> (String, String) {
    const NONE: &str = "<none>";
    if reference.is_empty() || reference.starts_with("<none>") {
        return (NONE.into(), NONE.into());
    }
    let without_digest = reference.split('@').next().unwrap_or(reference);
    let last_slash = without_digest.rfind('/').map_or(0, |i| i + 1);
    match without_digest[last_slash..].rfind(':') {
        Some(i) => {
            let split = last_slash + i;
            (
                without_digest[..split].to_owned(),
                without_digest[split + 1..].to_owned(),
            )
        }
        None => (without_digest.to_owned(), NONE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Proto;

    #[test]
    fn shl_008_sizes_are_si() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_200), "1.20 kB");
        assert_eq!(format_size(12_300_000), "12.3 MB");
        assert_eq!(format_size(187_000_000), "187 MB");
        assert_eq!(format_size(1_234_000_000), "1.23 GB");
        assert_eq!(format_size(999_960), "1.00 MB");
        assert_eq!(format_rate(1_500.0), "1.50 kB/s");
    }

    #[test]
    fn shl_007_relative_times() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(20_000);
        assert_eq!(format_relative(now, now), "just now");
        assert_eq!(format_relative(now + Duration::hours(1), now), "just now");
        assert_eq!(
            format_relative(now - Duration::seconds(12), now),
            "12 seconds ago"
        );
        assert_eq!(
            format_relative(now - Duration::minutes(1), now),
            "1 minute ago"
        );
        assert_eq!(
            format_relative(now - Duration::hours(2), now),
            "2 hours ago"
        );
        assert_eq!(format_relative(now - Duration::days(3), now), "3 days ago");
        assert_eq!(
            format_relative(now - Duration::days(400), now),
            "1 year ago"
        );
    }

    #[test]
    fn shl_008_short_ids_and_ports() {
        assert_eq!(short_id("sha256:0123456789abcdef"), "0123456789ab");
        assert_eq!(short_id("abc"), "abc");
        let p = PortMapping {
            ip: Some("0.0.0.0".parse().unwrap()),
            private: 80,
            public: Some(8080),
            proto: Proto::Tcp,
        };
        assert_eq!(format_port(&p), "8080:80/tcp");
        let p = PortMapping {
            ip: None,
            private: 5432,
            public: None,
            proto: Proto::Tcp,
        };
        assert_eq!(format_port(&p), "5432/tcp");
    }

    #[test]
    fn img_001_split_repo_tag() {
        assert_eq!(
            split_repo_tag("nginx:1.27"),
            ("nginx".into(), "1.27".into())
        );
        assert_eq!(
            split_repo_tag("localhost:5000/app:dev"),
            ("localhost:5000/app".into(), "dev".into())
        );
        assert_eq!(
            split_repo_tag("localhost:5000/app"),
            ("localhost:5000/app".into(), "<none>".into())
        );
        assert_eq!(
            split_repo_tag("<none>:<none>"),
            ("<none>".into(), "<none>".into())
        );
        assert_eq!(format_percent(3.7), "3.7%");
        assert_eq!(format_percent(0.0), "0%");
    }
}
