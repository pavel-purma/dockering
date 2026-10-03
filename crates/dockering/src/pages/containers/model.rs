//! Pure logic of the Containers page: filter (CON-004), search (SHL-006, CON-005), sort
//! (CON-003), and tree building from `dk_core::grouping` (CON-010/011). Unit-tested.

use std::collections::HashSet;

use dk_core::grouping::{self, ContainerGroup, GroupBy, GroupNode};
use dk_core::{ContainerState, ContainerSummary};
use gpui_kit::SharedString;

use crate::ui::list_table::{ListNode, SortState};

/// Status filter segment (CON-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Running,
    Stopped,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 3] = [
        StatusFilter::All,
        StatusFilter::Running,
        StatusFilter::Stopped,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Running => "running",
            StatusFilter::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => StatusFilter::Running,
            "stopped" => StatusFilter::Stopped,
            _ => StatusFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        use crate::strings as s;
        match self {
            StatusFilter::All => s::FILTER_ALL,
            StatusFilter::Running => s::FILTER_RUNNING,
            StatusFilter::Stopped => s::FILTER_STOPPED,
        }
    }

    pub fn matches(self, c: &ContainerSummary) -> bool {
        match self {
            StatusFilter::All => true,
            // Paused containers are "up" (Docker Desktop shows them under running).
            StatusFilter::Running => c.state.is_running() || c.state == ContainerState::Paused,
            StatusFilter::Stopped => !(c.state.is_running() || c.state == ContainerState::Paused),
        }
    }
}

/// Group payload rendered by group rows.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupInfo {
    pub group: ContainerGroup,
    /// Member summaries (in display order).
    pub members: Vec<ContainerSummary>,
}

/// Case-insensitive substring over name, image, id prefix, and compose project (SHL-006).
pub fn matches_search(c: &ContainerSummary, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    c.name.to_lowercase().contains(&q)
        || c.image.to_lowercase().contains(&q)
        || c.id.to_lowercase().starts_with(&q)
        || c.compose
            .as_ref()
            .is_some_and(|ci| ci.project.to_lowercase().contains(&q))
}

/// Sort keys (CON-003).
pub mod sort_keys {
    pub const NAME: &str = "name";
    pub const IMAGE: &str = "image";
    pub const STATUS: &str = "status";
    pub const CREATED: &str = "created";
    pub const CPU: &str = "cpu";
    pub const MEM: &str = "mem";
}

fn state_rank(s: ContainerState) -> u8 {
    match s {
        ContainerState::Running => 0,
        ContainerState::Restarting => 1,
        ContainerState::Paused => 2,
        ContainerState::Created => 3,
        ContainerState::Removing => 4,
        ContainerState::Exited => 5,
        ContainerState::Dead => 6,
        ContainerState::Unknown => 7,
    }
}

/// Latest list stats lookups for the CPU / Memory sort keys (`None` = no sample yet).
pub struct StatsLookup<'a> {
    pub cpu: &'a dyn Fn(&str) -> Option<f64>,
    pub mem: &'a dyn Fn(&str) -> Option<u64>,
}

/// Compare two containers by `sort` (ties by name, so order is stable). Rows without a
/// stats sample sort below every sampled row.
pub fn compare(
    a: &ContainerSummary,
    b: &ContainerSummary,
    sort: &SortState,
    stats: &StatsLookup<'_>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let ord = match sort.key.as_ref() {
        sort_keys::IMAGE => a.image.to_lowercase().cmp(&b.image.to_lowercase()),
        sort_keys::STATUS => state_rank(a.state).cmp(&state_rank(b.state)),
        sort_keys::CREATED => a.created.cmp(&b.created),
        sort_keys::CPU => (stats.cpu)(&a.id)
            .unwrap_or(-1.0)
            .partial_cmp(&(stats.cpu)(&b.id).unwrap_or(-1.0))
            .unwrap_or(Ordering::Equal),
        sort_keys::MEM => (stats.mem)(&a.id).cmp(&(stats.mem)(&b.id)),
        _ => Ordering::Equal,
    };
    let ord = if sort.descending { ord.reverse() } else { ord };
    ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
}

/// The inputs of a tree build.
pub struct BuildInput<'a> {
    pub containers: &'a [ContainerSummary],
    pub group_by: &'a GroupBy,
    pub interleave: bool,
    pub filter: StatusFilter,
    pub query: &'a str,
    pub sort: Option<&'a SortState>,
    pub stats: StatsLookup<'a>,
}

/// The output: tree nodes plus the groups to force open because a member matched the search
/// (CON-005).
pub struct BuildOutput {
    pub nodes: Vec<ListNode<GroupInfo, ContainerSummary>>,
    pub force_expanded: HashSet<SharedString>,
    pub running: usize,
    pub stopped: usize,
}

pub fn group_key(g: &ContainerGroup) -> SharedString {
    g.key.clone().into()
}

/// Filters, searches, groups, and sorts. Pure; runs on `background_spawn` for large lists.
pub fn build(input: BuildInput<'_>) -> BuildOutput {
    let running = input
        .containers
        .iter()
        .filter(|c| StatusFilter::Running.matches(c))
        .count();
    let stopped = input.containers.len() - running;

    let searching = !input.query.trim().is_empty();
    let visible: Vec<ContainerSummary> = input
        .containers
        .iter()
        .filter(|c| input.filter.matches(c) && matches_search(c, input.query))
        .cloned()
        .collect();

    // Default order: name ascending.
    let default_sort = SortState {
        key: sort_keys::NAME.into(),
        descending: false,
    };
    let sort = input.sort.unwrap_or(&default_sort);
    let cmp = |a: &ContainerSummary, b: &ContainerSummary| compare(a, b, sort, &input.stats);

    let grouped = grouping::group(&visible, input.group_by, input.interleave);
    let mut nodes = Vec::with_capacity(grouped.len());
    let mut force_expanded = HashSet::new();
    for node in grouped {
        match node {
            GroupNode::Group(g) => {
                let mut members: Vec<ContainerSummary> =
                    g.members.iter().map(|&i| visible[i].clone()).collect();
                members.sort_by(&cmp);
                let key = group_key(&g);
                if searching {
                    force_expanded.insert(key.clone());
                }
                let children = members
                    .iter()
                    .map(|c| (SharedString::from(c.id.clone()), c.clone()))
                    .collect();
                nodes.push(ListNode::Group {
                    key,
                    group: GroupInfo { group: g, members },
                    children,
                });
            }
            GroupNode::Container(i) => {
                let c = visible[i].clone();
                nodes.push(ListNode::Item {
                    key: c.id.clone().into(),
                    item: c,
                });
            }
        }
    }

    // Groups sort by name (or by their first member under the active key); ungrouped
    // containers come after groups unless interleaving (spec 21 §4 rule 3).
    let group_first = |n: &ListNode<GroupInfo, ContainerSummary>| -> Option<ContainerSummary> {
        match n {
            ListNode::Group { group, .. } => group.members.first().cloned(),
            ListNode::Item { item, .. } => Some(item.clone()),
        }
    };
    let by_name = sort.key.as_ref() == sort_keys::NAME;
    nodes.sort_by(|a, b| {
        use std::cmp::Ordering;
        let rank = |n: &ListNode<GroupInfo, ContainerSummary>| match n {
            ListNode::Group { .. } if !input.interleave => 0,
            ListNode::Item { .. } if !input.interleave => 1,
            _ => 0,
        };
        let r = rank(a).cmp(&rank(b));
        if r != Ordering::Equal {
            return r;
        }
        let name = |n: &ListNode<GroupInfo, ContainerSummary>| match n {
            ListNode::Group { group, .. } => group.group.label.to_lowercase(),
            ListNode::Item { item, .. } => item.name.to_lowercase(),
        };
        if by_name {
            let o = name(a).cmp(&name(b));
            if sort.descending { o.reverse() } else { o }
        } else {
            match (group_first(a), group_first(b)) {
                (Some(x), Some(y)) => cmp(&x, &y),
                _ => Ordering::Equal,
            }
        }
    });

    BuildOutput {
        nodes,
        force_expanded,
        running,
        stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::fake::fixtures;

    fn sample() -> Vec<ContainerSummary> {
        vec![
            fixtures::compose_container("shop", "web", ContainerState::Running),
            fixtures::compose_container("shop", "db", ContainerState::Exited),
            fixtures::container("redis", ContainerState::Running),
            fixtures::container("alpha", ContainerState::Exited),
            fixtures::container("paused", ContainerState::Paused),
        ]
    }

    fn input<'a>(
        cs: &'a [ContainerSummary],
        by: &'a GroupBy,
        filter: StatusFilter,
        query: &'a str,
        sort: Option<&'a SortState>,
    ) -> BuildInput<'a> {
        BuildInput {
            containers: cs,
            group_by: by,
            interleave: false,
            filter,
            query,
            sort,
            stats: StatsLookup {
                cpu: &|_| None,
                mem: &|_| None,
            },
        }
    }

    fn keys(o: &BuildOutput) -> Vec<String> {
        o.nodes
            .iter()
            .map(|n| match n {
                ListNode::Group { group, .. } => format!("G:{}", group.group.label),
                ListNode::Item { item, .. } => item.name.clone(),
            })
            .collect()
    }

    #[test]
    fn con_010_groups_first_then_ungrouped_sorted_by_name() {
        let cs = sample();
        let out = build(input(&cs, &GroupBy::Compose, StatusFilter::All, "", None));
        assert_eq!(keys(&out), ["G:shop", "alpha", "paused", "redis"]);
        let ListNode::Group { children, .. } = &out.nodes[0] else {
            panic!()
        };
        // Members sorted by name: db before web.
        assert_eq!(children[0].1.name, "shop-db-1");
        assert_eq!(out.running, 3);
        assert_eq!(out.stopped, 2);
    }

    #[test]
    fn con_004_filter_running_and_stopped() {
        let cs = sample();
        let out = build(input(&cs, &GroupBy::None, StatusFilter::Running, "", None));
        assert_eq!(keys(&out), ["paused", "redis", "shop-web-1"]);
        let out = build(input(&cs, &GroupBy::None, StatusFilter::Stopped, "", None));
        assert_eq!(keys(&out), ["alpha", "shop-db-1"]);
    }

    #[test]
    fn con_005_search_matches_member_and_expands_group() {
        let cs = sample();
        let out = build(input(
            &cs,
            &GroupBy::Compose,
            StatusFilter::All,
            "WEB",
            None,
        ));
        assert_eq!(keys(&out), ["G:shop"]);
        assert!(out.force_expanded.contains("compose:shop"));
        // Project name and id prefix match too.
        let out = build(input(
            &cs,
            &GroupBy::Compose,
            StatusFilter::All,
            "shop",
            None,
        ));
        assert_eq!(keys(&out), ["G:shop"]);
        let id_prefix = &cs[2].id[..6];
        let out = build(input(
            &cs,
            &GroupBy::None,
            StatusFilter::All,
            id_prefix,
            None,
        ));
        assert_eq!(keys(&out), ["redis"]);
    }

    #[test]
    fn con_003_sort_by_status_desc_and_name() {
        let cs = sample();
        let sort = SortState {
            key: sort_keys::STATUS.into(),
            descending: false,
        };
        let out = build(input(
            &cs,
            &GroupBy::None,
            StatusFilter::All,
            "",
            Some(&sort),
        ));
        assert_eq!(
            keys(&out),
            ["redis", "shop-web-1", "paused", "alpha", "shop-db-1"]
        );
        let sort = SortState {
            key: sort_keys::NAME.into(),
            descending: true,
        };
        let out = build(input(
            &cs,
            &GroupBy::None,
            StatusFilter::All,
            "",
            Some(&sort),
        ));
        assert_eq!(keys(&out)[0], "shop-web-1");
    }

    #[test]
    fn con_003_sort_by_memory_usage() {
        let cs = sample();
        let mem = |id: &str| -> Option<u64> {
            if id == cs[2].id {
                Some(300)
            } else if id == cs[0].id {
                Some(100)
            } else {
                None
            }
        };
        let sort = SortState {
            key: sort_keys::MEM.into(),
            descending: true,
        };
        let out = build(BuildInput {
            stats: StatsLookup {
                cpu: &|_| None,
                mem: &mem,
            },
            ..input(&cs, &GroupBy::None, StatusFilter::Running, "", Some(&sort))
        });
        // Descending: most memory first; rows without a sample last.
        assert_eq!(keys(&out), ["redis", "shop-web-1", "paused"]);
    }

    #[test]
    fn label_grouping() {
        let mut cs = sample();
        cs[2].labels.insert("app".into(), "cache".into());
        let by = GroupBy::Label("app".into());
        let out = build(input(&cs, &by, StatusFilter::All, "", None));
        assert_eq!(keys(&out)[0], "G:cache");
    }

    #[test]
    fn filter_roundtrip() {
        for f in StatusFilter::ALL {
            assert_eq!(StatusFilter::parse(f.as_str()), f);
        }
    }
}
