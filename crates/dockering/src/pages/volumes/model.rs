//! Pure logic of the Volumes page (VOL-001…005): rows with lazily-known size and in-use
//! counts (VOL-002), filters, search, sort, prune candidates. Unit-tested; no GPUI.

use std::collections::HashMap;

use dk_core::{ContainerSummary, DiskUsage, MountKind, VolumeSummary};
use gpui_kit::SharedString;
use time::OffsetDateTime;

use crate::strings as s;
use crate::ui::list_table::{ListNode, SortState};

/// Filter segment (VOL-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VolumeFilter {
    #[default]
    All,
    InUse,
    Unused,
}

impl VolumeFilter {
    pub const OPTIONS: &'static [(&'static str, &'static str)] = &[
        ("all", s::FILTER_ALL),
        ("in-use", s::FILTER_IN_USE),
        ("unused", s::FILTER_UNUSED),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VolumeFilter::All => "all",
            VolumeFilter::InUse => "in-use",
            VolumeFilter::Unused => "unused",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "in-use" => VolumeFilter::InUse,
            "unused" => VolumeFilter::Unused,
            _ => VolumeFilter::All,
        }
    }

    pub fn matches(self, r: &VolumeRow) -> bool {
        match self {
            VolumeFilter::All => true,
            VolumeFilter::InUse => r.in_use() > 0,
            VolumeFilter::Unused => r.in_use() == 0,
        }
    }
}

/// What the page knows about sizes (VOL-002).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum UsageState {
    /// `DISK_USAGE` and the fetch is running: skeleton cells.
    #[default]
    Loading,
    /// The engine's `disk_usage()` result.
    Known(DiskUsage),
    /// No `DISK_USAGE` capability (or the fetch failed): "—" and mount-derived in-use.
    Unavailable,
}

/// One volume row.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeRow {
    pub key: SharedString,
    pub name: String,
    pub driver: String,
    pub compose: Option<String>,
    pub created: Option<OffsetDateTime>,
    /// `None` = unknown (loading or unavailable; see [`UsageState`]).
    pub size: Option<u64>,
    /// In-use count from `disk_usage` (`ref_count`), when known.
    pub ref_count: Option<i64>,
    /// Containers mounting it (from the container list).
    pub used_by: Vec<String>,
}

impl VolumeRow {
    /// Containers using it: the engine's ref count when known, else mount-derived (VOL-002).
    pub fn in_use(&self) -> usize {
        match self.ref_count {
            Some(n) if n >= 0 => (n as usize).max(self.used_by.len()),
            _ => self.used_by.len(),
        }
    }
}

/// Container names mounting each volume (named volumes only).
pub fn users_by_volume(containers: &[ContainerSummary]) -> HashMap<&str, Vec<String>> {
    let mut m: HashMap<&str, Vec<String>> = HashMap::new();
    for c in containers {
        for mnt in c.mounts.iter().filter(|m| m.kind == MountKind::Volume) {
            m.entry(mnt.source.as_str())
                .or_default()
                .push(c.name.clone());
        }
    }
    m
}

pub fn rows(
    volumes: &[VolumeSummary],
    containers: &[ContainerSummary],
    usage: &UsageState,
) -> Vec<VolumeRow> {
    let users = users_by_volume(containers);
    let du = match usage {
        UsageState::Known(d) => Some(d),
        _ => None,
    };
    volumes
        .iter()
        .map(|v| {
            let size = du
                .and_then(|d| {
                    d.volumes
                        .iter()
                        .find(|(n, _)| *n == v.name)
                        .map(|(_, s)| *s)
                })
                .or(v.size.filter(|_| du.is_some()));
            let ref_count = du
                .and_then(|d| {
                    d.volume_refs
                        .iter()
                        .find(|(n, _)| *n == v.name)
                        .map(|(_, r)| *r)
                })
                .or(v.ref_count);
            VolumeRow {
                key: v.name.clone().into(),
                name: v.name.clone(),
                driver: v.driver.clone(),
                compose: v.compose.as_ref().map(|c| c.project.clone()),
                created: v.created,
                size,
                ref_count,
                used_by: users.get(v.name.as_str()).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// Case-insensitive substring over the name and compose project (VOL-003, SHL-006).
pub fn matches_search(r: &VolumeRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || r.name.to_lowercase().contains(&q)
        || r.compose
            .as_ref()
            .is_some_and(|p| p.to_lowercase().contains(&q))
}

pub mod sort_keys {
    pub const NAME: &str = "name";
    pub const CREATED: &str = "created";
    pub const SIZE: &str = "size";
}

pub fn compare(a: &VolumeRow, b: &VolumeRow, sort: &SortState) -> std::cmp::Ordering {
    let ord = match sort.key.as_ref() {
        sort_keys::CREATED => a.created.cmp(&b.created),
        sort_keys::SIZE => a.size.cmp(&b.size),
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    };
    let ord = if sort.descending { ord.reverse() } else { ord };
    ord.then_with(|| a.name.cmp(&b.name))
}

pub fn build(
    rows: &[VolumeRow],
    filter: VolumeFilter,
    query: &str,
    sort: Option<&SortState>,
) -> Vec<ListNode<(), VolumeRow>> {
    let default_sort = SortState {
        key: sort_keys::NAME.into(),
        descending: false,
    };
    let sort = sort.unwrap_or(&default_sort);
    let mut v: Vec<&VolumeRow> = rows
        .iter()
        .filter(|r| filter.matches(r) && matches_search(r, query))
        .collect();
    v.sort_by(|a, b| compare(a, b, sort));
    v.into_iter()
        .map(|r| ListNode::Item {
            key: r.key.clone(),
            item: r.clone(),
        })
        .collect()
}

/// Unused volumes and the bytes a prune would reclaim (`None` when sizes are unknown).
pub fn prune_candidates(rows: &[VolumeRow]) -> (Vec<String>, Option<u64>) {
    let unused: Vec<&VolumeRow> = rows.iter().filter(|r| r.in_use() == 0).collect();
    let size = unused
        .iter()
        .map(|r| r.size)
        .try_fold(0u64, |acc, s| s.map(|s| acc + s));
    (unused.iter().map(|r| r.name.clone()).collect(), size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::fake::fixtures;
    use dk_core::{ContainerState, MountSummary};

    fn sample() -> (Vec<VolumeSummary>, Vec<ContainerSummary>) {
        let vols = vec![
            fixtures::volume("data"),
            fixtures::volume("cache"),
            fixtures::volume("logs"),
        ];
        let mut c = fixtures::container("db", ContainerState::Running);
        c.mounts = vec![MountSummary {
            kind: MountKind::Volume,
            source: "data".into(),
            destination: "/data".into(),
            rw: true,
        }];
        (vols, vec![c])
    }

    fn du() -> DiskUsage {
        DiskUsage {
            volumes: vec![
                ("data".into(), 300),
                ("cache".into(), 100),
                ("logs".into(), 5),
            ],
            volume_refs: vec![("data".into(), 1), ("cache".into(), 0), ("logs".into(), 0)],
            ..Default::default()
        }
    }

    #[test]
    fn vol_002_sizes_only_from_disk_usage() {
        let (v, c) = sample();
        let loading = rows(&v, &c, &UsageState::Loading);
        assert!(loading.iter().all(|r| r.size.is_none()));
        // Without DISK_USAGE, in-use is computed from container mounts.
        let na = rows(&v, &c, &UsageState::Unavailable);
        assert_eq!(na.iter().find(|r| r.name == "data").unwrap().in_use(), 1);
        assert!(na.iter().all(|r| r.size.is_none()));
        let known = rows(&v, &c, &UsageState::Known(du()));
        assert_eq!(
            known.iter().find(|r| r.name == "data").unwrap().size,
            Some(300)
        );
    }

    #[test]
    fn vol_003_filter_search_sort() {
        let (v, c) = sample();
        let r = rows(&v, &c, &UsageState::Known(du()));
        let names = |n: Vec<ListNode<(), VolumeRow>>| -> Vec<String> {
            n.into_iter()
                .map(|n| match n {
                    ListNode::Item { item, .. } => item.name,
                    ListNode::Group { .. } => unreachable!(),
                })
                .collect()
        };
        assert_eq!(
            names(build(&r, VolumeFilter::All, "", None)),
            ["cache", "data", "logs"]
        );
        assert_eq!(names(build(&r, VolumeFilter::InUse, "", None)), ["data"]);
        assert_eq!(names(build(&r, VolumeFilter::Unused, "LO", None)), ["logs"]);
        let by_size = SortState {
            key: sort_keys::SIZE.into(),
            descending: true,
        };
        assert_eq!(
            names(build(&r, VolumeFilter::All, "", Some(&by_size))),
            ["data", "cache", "logs"]
        );
    }

    #[test]
    fn vol_005_prune_reclaimable() {
        let (v, c) = sample();
        let r = rows(&v, &c, &UsageState::Known(du()));
        assert_eq!(
            prune_candidates(&r),
            (vec!["cache".into(), "logs".into()], Some(105))
        );
        let r = rows(&v, &c, &UsageState::Unavailable);
        assert_eq!(prune_candidates(&r).1, None);
    }
}
