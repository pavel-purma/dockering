//! Stats math (spec 21 §3.3, STA-003). SIGNATURES FIXED — bodies by `rust-core`.

use std::collections::VecDeque;

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

impl RawStats {
    /// Parse one Docker `/containers/{id}/stats` JSON object (also WSLC COM `Stats()`).
    /// Tolerant: missing fields → 0/None (NFR-031).
    pub fn from_docker_json(v: &serde_json::Value) -> EngineResult<RawStats> {
        let _ = v;
        unimplemented!("rust-core")
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
        let _ = (raw, now, &self.prev);
        unimplemented!("rust-core")
    }
}

/// Docker CLI CPU % formula: `(cpu_delta / sys_delta) * online_cpus * 100` when both > 0.
pub fn cpu_percent(cpu_delta: u64, sys_delta: u64, online_cpus: u32) -> f64 {
    let _ = (cpu_delta, sys_delta, online_cpus);
    unimplemented!("rust-core")
}

/// Largest-Triangle-Three-Buckets downsampling of `(x, y)` points to at most `max` points
/// (keeps first and last). `max < 3` or `points.len() <= max` → unchanged copy.
pub fn downsample_lttb(points: &[(f64, f64)], max: usize) -> Vec<(f64, f64)> {
    let _ = (points, max);
    unimplemented!("rust-core")
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
