//! Stats math (spec 21 §3.3, STA-003).

use std::collections::VecDeque;

use serde_json::Value;
use time::OffsetDateTime;

use crate::error::EngineResult;
use crate::model::StatsSample;

/// Cumulative counters as reported by the engine (Docker stats JSON shape).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawStats {
    pub read: Option<OffsetDateTime>,
    pub cpu_total_usage: u64,
    pub system_cpu_usage: Option<u64>,
    pub precpu_total_usage: Option<u64>,
    pub precpu_system_cpu_usage: Option<u64>,
    pub online_cpus: u32,
    pub mem_usage: u64,
    /// `inactive_file` (cgroup v2) or `cache` (v1), subtracted from usage.
    pub mem_cache: u64,
    pub mem_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub blk_read: u64,
    pub blk_write: u64,
    pub pids: Option<u64>,
}

fn u(v: &Value, path: &[&str]) -> Option<u64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_u64()
        .or_else(|| cur.as_f64().map(|f| f.max(0.0) as u64))
}

impl RawStats {
    /// Parse one Docker `/containers/{id}/stats` JSON object (also WSLC COM `Stats()`).
    /// Tolerant: missing fields → 0/None (NFR-031).
    pub fn from_docker_json(v: &Value) -> EngineResult<RawStats> {
        if !v.is_object() {
            return Err(crate::error::EngineError::protocol(
                "stats: expected a JSON object",
            ));
        }
        let read = v
            .get("read")
            .and_then(Value::as_str)
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .filter(|t| t.year() > 1);
        let online_cpus = u(v, &["cpu_stats", "online_cpus"])
            .filter(|n| *n > 0)
            .or_else(|| {
                v.pointer("/cpu_stats/cpu_usage/percpu_usage")
                    .and_then(Value::as_array)
                    .map(|a| a.len() as u64)
                    .filter(|n| *n > 0)
            })
            .or_else(|| u(v, &["num_procs"]).filter(|n| *n > 0))
            .unwrap_or(1) as u32;
        let mem_usage = u(v, &["memory_stats", "usage"])
            .or_else(|| u(v, &["memory_stats", "privateworkingset"]))
            .unwrap_or(0);
        // cgroup v2: inactive_file; v1: total_inactive_file, then cache.
        let mem_cache = u(v, &["memory_stats", "stats", "inactive_file"])
            .or_else(|| u(v, &["memory_stats", "stats", "total_inactive_file"]))
            .or_else(|| u(v, &["memory_stats", "stats", "cache"]))
            .unwrap_or(0);
        let mem_limit = u(v, &["memory_stats", "limit"]).unwrap_or(0);

        let (mut net_rx, mut net_tx) = (0u64, 0u64);
        if let Some(nets) = v.get("networks").and_then(Value::as_object) {
            for n in nets.values() {
                net_rx = net_rx.saturating_add(u(n, &["rx_bytes"]).unwrap_or(0));
                net_tx = net_tx.saturating_add(u(n, &["tx_bytes"]).unwrap_or(0));
            }
        }

        let (mut blk_read, mut blk_write) = (0u64, 0u64);
        if let Some(entries) = v
            .pointer("/blkio_stats/io_service_bytes_recursive")
            .and_then(Value::as_array)
        {
            for e in entries {
                let val = u(e, &["value"]).unwrap_or(0);
                match e
                    .get("op")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("read") => blk_read = blk_read.saturating_add(val),
                    Some("write") => blk_write = blk_write.saturating_add(val),
                    _ => {}
                }
            }
        } else if v.get("storage_stats").is_some() {
            // Windows containers shape.
            blk_read = u(v, &["storage_stats", "read_size_bytes"]).unwrap_or(0);
            blk_write = u(v, &["storage_stats", "write_size_bytes"]).unwrap_or(0);
        }

        Ok(RawStats {
            read,
            cpu_total_usage: u(v, &["cpu_stats", "cpu_usage", "total_usage"]).unwrap_or(0),
            system_cpu_usage: u(v, &["cpu_stats", "system_cpu_usage"]),
            precpu_total_usage: u(v, &["precpu_stats", "cpu_usage", "total_usage"]),
            precpu_system_cpu_usage: u(v, &["precpu_stats", "system_cpu_usage"]),
            online_cpus,
            mem_usage,
            mem_cache,
            mem_limit,
            net_rx,
            net_tx,
            blk_read,
            blk_write,
            pids: u(v, &["pids_stats", "current"]),
        })
    }
}

/// Converts consecutive raw samples to rates. The first sample yields rates of 0. CPU % uses
/// the sample's own `precpu` when present and non-zero, else the previous sample (polled
/// engines).
#[derive(Debug, Default)]
pub struct StatsNormalizer {
    prev: Option<(RawStats, OffsetDateTime)>,
}

impl StatsNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `now` is used when `raw.read` is missing.
    pub fn push(&mut self, raw: RawStats, now: OffsetDateTime) -> StatsSample {
        let at = raw.read.unwrap_or(now);
        let prev = self.prev.as_ref();

        let (pre_cpu, pre_sys) = match (raw.precpu_total_usage, raw.precpu_system_cpu_usage) {
            (Some(c), Some(s)) if c > 0 && s > 0 => (Some(c), Some(s)),
            _ => prev
                .map(|(p, _)| (Some(p.cpu_total_usage), p.system_cpu_usage))
                .unwrap_or((None, None)),
        };
        let cpu = match (pre_cpu, raw.system_cpu_usage, pre_sys) {
            (Some(pc), Some(sys), Some(ps)) => cpu_percent(
                raw.cpu_total_usage.saturating_sub(pc),
                sys.saturating_sub(ps),
                raw.online_cpus,
            ),
            _ => 0.0,
        };

        let rate = |cur: u64, prev_val: Option<u64>, dt: f64| -> f64 {
            match prev_val {
                Some(p) if dt > 0.0 && cur >= p => (cur - p) as f64 / dt,
                _ => 0.0,
            }
        };
        let dt = prev.map(|(_, t)| (at - *t).as_seconds_f64()).unwrap_or(0.0);
        let sample = StatsSample {
            at,
            cpu_percent: cpu,
            online_cpus: raw.online_cpus,
            mem_used: raw.mem_usage.saturating_sub(raw.mem_cache),
            mem_limit: raw.mem_limit,
            net_rx_bps: rate(raw.net_rx, prev.map(|(p, _)| p.net_rx), dt),
            net_tx_bps: rate(raw.net_tx, prev.map(|(p, _)| p.net_tx), dt),
            net_rx_total: raw.net_rx,
            net_tx_total: raw.net_tx,
            blk_read_bps: rate(raw.blk_read, prev.map(|(p, _)| p.blk_read), dt),
            blk_write_bps: rate(raw.blk_write, prev.map(|(p, _)| p.blk_write), dt),
            blk_read_total: raw.blk_read,
            blk_write_total: raw.blk_write,
            pids: raw.pids,
        };
        self.prev = Some((raw, at));
        sample
    }
}

/// Docker CLI CPU % formula: `(cpu_delta / sys_delta) * online_cpus * 100` when both > 0.
pub fn cpu_percent(cpu_delta: u64, sys_delta: u64, online_cpus: u32) -> f64 {
    if cpu_delta == 0 || sys_delta == 0 {
        return 0.0;
    }
    (cpu_delta as f64 / sys_delta as f64) * online_cpus.max(1) as f64 * 100.0
}

/// Largest-Triangle-Three-Buckets downsampling of `(x, y)` points to at most `max` points
/// (keeps first and last). `max < 3` or `points.len() <= max` → unchanged copy.
pub fn downsample_lttb(points: &[(f64, f64)], max: usize) -> Vec<(f64, f64)> {
    let n = points.len();
    if max < 3 || n <= max {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(max);
    out.push(points[0]);
    let bucket = (n - 2) as f64 / (max - 2) as f64;
    let mut a = 0usize;
    for i in 0..max - 2 {
        let start = (i as f64 * bucket) as usize + 1;
        let end = (((i + 1) as f64 * bucket) as usize + 1).min(n - 1);
        // Average of the next bucket.
        let next_start = end;
        let next_end = (((i + 2) as f64 * bucket) as usize + 1).min(n);
        let next = &points[next_start..next_end.max(next_start + 1).min(n)];
        let (ax, ay) = next
            .iter()
            .fold((0.0, 0.0), |(sx, sy), p| (sx + p.0, sy + p.1));
        let (avg_x, avg_y) = (ax / next.len() as f64, ay / next.len() as f64);
        let (px, py) = points[a];
        let mut best = start;
        let mut best_area = -1.0;
        for (j, &(x, y)) in points
            .iter()
            .enumerate()
            .take(end.max(start + 1))
            .skip(start)
        {
            let area = ((px - avg_x) * (y - py) - (px - x) * (avg_y - py)).abs();
            if area > best_area {
                best_area = area;
                best = j;
            }
        }
        out.push(points[best]);
        a = best;
    }
    out.push(points[n - 1]);
    out
}

/// Fixed-capacity ring of samples (STA-003: ≤ 3,600). Oldest dropped first.
#[derive(Debug, Clone)]
pub struct StatsRing {
    cap: usize,
    buf: VecDeque<StatsSample>,
}

impl StatsRing {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            buf: VecDeque::new(),
        }
    }
    pub fn push(&mut self, s: StatsSample) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(s);
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn last(&self) -> Option<&StatsSample> {
        self.buf.back()
    }
    pub fn iter(&self) -> impl Iterator<Item = &StatsSample> {
        self.buf.iter()
    }
    /// Samples at or after `since`.
    pub fn since(&self, since: OffsetDateTime) -> Vec<StatsSample> {
        self.buf.iter().filter(|s| s.at >= since).cloned().collect()
    }
    pub fn to_vec(&self) -> Vec<StatsSample> {
        self.buf.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn sta_002_cpu_formula_matches_docker_cli() {
        assert_eq!(cpu_percent(0, 100, 4), 0.0);
        assert_eq!(cpu_percent(10, 0, 4), 0.0);
        assert!((cpu_percent(50, 1000, 4) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn sta_002_parses_cgroup_v2_docker_json() {
        let v = json!({
            "read": "2026-10-02T12:00:01.000000000Z",
            "cpu_stats": { "cpu_usage": { "total_usage": 2_000_000u64 }, "system_cpu_usage": 20_000_000u64, "online_cpus": 4 },
            "precpu_stats": { "cpu_usage": { "total_usage": 1_000_000u64 }, "system_cpu_usage": 10_000_000u64 },
            "memory_stats": { "usage": 300, "limit": 1000, "stats": { "inactive_file": 100 } },
            "networks": { "eth0": { "rx_bytes": 10, "tx_bytes": 20 }, "eth1": { "rx_bytes": 1, "tx_bytes": 2 } },
            "blkio_stats": { "io_service_bytes_recursive": [ { "op": "read", "value": 5 }, { "op": "write", "value": 7 }, { "op": "Read", "value": 1 } ] },
            "pids_stats": { "current": 3 }
        });
        let raw = RawStats::from_docker_json(&v).unwrap();
        assert_eq!(
            (raw.net_rx, raw.net_tx, raw.blk_read, raw.blk_write),
            (11, 22, 6, 7)
        );
        let s = StatsNormalizer::new().push(raw, OffsetDateTime::UNIX_EPOCH);
        assert!((s.cpu_percent - 40.0).abs() < 1e-9);
        assert_eq!(s.mem_used, 200);
        assert_eq!(s.pids, Some(3));
        assert_eq!(s.net_rx_bps, 0.0, "first sample yields rates of 0");
    }

    #[test]
    fn nfr_031_stats_tolerates_missing_fields() {
        let raw = RawStats::from_docker_json(&json!({})).unwrap();
        assert_eq!(raw.online_cpus, 1);
        assert!(RawStats::from_docker_json(&json!([1])).is_err());
    }

    #[test]
    fn sta_002_polled_engines_use_previous_sample_for_rates_and_cpu() {
        let t0 = OffsetDateTime::UNIX_EPOCH;
        let mut n = StatsNormalizer::new();
        let r1 = RawStats {
            read: Some(t0),
            cpu_total_usage: 100,
            system_cpu_usage: Some(1000),
            online_cpus: 2,
            net_rx: 1000,
            ..Default::default()
        };
        let r2 = RawStats {
            read: Some(t0 + time::Duration::seconds(2)),
            cpu_total_usage: 200,
            system_cpu_usage: Some(2000),
            online_cpus: 2,
            net_rx: 3000,
            ..Default::default()
        };
        n.push(r1, t0);
        let s = n.push(r2, t0);
        assert!((s.cpu_percent - 20.0).abs() < 1e-9);
        assert!((s.net_rx_bps - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn sta_003_ring_caps_and_drops_oldest() {
        let mut r = StatsRing::new(3);
        let base = crate::fake::fixtures::stats_sample(OffsetDateTime::UNIX_EPOCH, 1.0, 1);
        for i in 0..5 {
            let mut s = base.clone();
            s.at += time::Duration::seconds(i);
            r.push(s);
        }
        assert_eq!(r.len(), 3);
        assert_eq!(
            r.iter().next().unwrap().at,
            base.at + time::Duration::seconds(2)
        );
        assert_eq!(r.since(base.at + time::Duration::seconds(4)).len(), 1);
    }

    #[test]
    fn sta_003_downsamples_to_300_points() {
        let pts: Vec<(f64, f64)> = (0..3600)
            .map(|i| (i as f64, (i as f64 * 0.1).sin()))
            .collect();
        let d = downsample_lttb(&pts, 300);
        assert_eq!(d.len(), 300);
        assert_eq!(d[0], pts[0]);
        assert_eq!(*d.last().unwrap(), pts[3599]);
    }

    proptest! {
        #[test]
        fn sta_003_lttb_bounds(n in 0usize..2000, max in 0usize..400) {
            let pts: Vec<(f64, f64)> = (0..n).map(|i| (i as f64, (i % 17) as f64)).collect();
            let d = downsample_lttb(&pts, max);
            if max < 3 || n <= max {
                prop_assert_eq!(d.len(), n);
            } else {
                prop_assert_eq!(d.len(), max);
                prop_assert!(d.windows(2).all(|w| w[0].0 < w[1].0), "x strictly increasing");
            }
        }

        #[test]
        fn sta_002_cpu_percent_bounded(c in 0u64..1_000_000, s in 1u64..1_000_000, cpus in 1u32..128) {
            prop_assume!(c <= s);
            let p = cpu_percent(c, s, cpus);
            prop_assert!(p >= 0.0 && p <= 100.0 * cpus as f64 + 1e-9);
        }
    }
}
