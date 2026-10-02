//! Pure logic of the Networks page (NET-001…004): rows, search, sort. Unit-tested.

use dk_core::{ContainerSummary, NetworkSummary};
use gpui_kit::SharedString;
use time::OffsetDateTime;

use crate::ui::list_table::{ListNode, SortState};

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkRow {
    pub key: SharedString,
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub subnets: Vec<String>,
    pub gateways: Vec<String>,
    /// Attached containers: from the list call when the engine reports it, else counted
    /// from `ContainerSummary.networks`.
    pub containers: usize,
    pub compose: Option<String>,
    pub created: OffsetDateTime,
    pub builtin: bool,
}

pub fn rows(networks: &[NetworkSummary], containers: &[ContainerSummary]) -> Vec<NetworkRow> {
    networks
        .iter()
        .map(|n| {
            let counted = containers
                .iter()
                .filter(|c| c.networks.iter().any(|x| *x == n.name || *x == n.id))
                .count();
            NetworkRow {
                key: n.id.clone().into(),
                id: n.id.clone(),
                name: n.name.clone(),
                driver: n.driver.clone(),
                scope: n.scope.clone(),
                subnets: n.subnets.iter().filter_map(|s| s.subnet.clone()).collect(),
                gateways: n.subnets.iter().filter_map(|s| s.gateway.clone()).collect(),
                containers: n
                    .containers
                    .map(|c| c as usize)
                    .unwrap_or(counted)
                    .max(counted),
                compose: n.compose.as_ref().map(|c| c.project.clone()),
                created: n.created,
                builtin: n.is_builtin(),
            }
        })
        .collect()
}

pub fn matches_search(r: &NetworkRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || r.name.to_lowercase().contains(&q)
        || r.driver.to_lowercase().contains(&q)
        || r.id.starts_with(&q)
        || r.subnets.iter().any(|s| s.contains(&q))
        || r.compose
            .as_ref()
            .is_some_and(|p| p.to_lowercase().contains(&q))
}

pub mod sort_keys {
    pub const NAME: &str = "name";
    pub const DRIVER: &str = "driver";
    pub const CONTAINERS: &str = "containers";
    pub const CREATED: &str = "created";
}

pub fn compare(a: &NetworkRow, b: &NetworkRow, sort: &SortState) -> std::cmp::Ordering {
    let ord = match sort.key.as_ref() {
        sort_keys::DRIVER => a.driver.cmp(&b.driver),
        sort_keys::CONTAINERS => a.containers.cmp(&b.containers),
        sort_keys::CREATED => a.created.cmp(&b.created),
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    };
    let ord = if sort.descending { ord.reverse() } else { ord };
    ord.then_with(|| a.name.cmp(&b.name))
}

pub fn build(
    rows: &[NetworkRow],
    query: &str,
    sort: Option<&SortState>,
) -> Vec<ListNode<(), NetworkRow>> {
    let default_sort = SortState {
        key: sort_keys::NAME.into(),
        descending: false,
    };
    let sort = sort.unwrap_or(&default_sort);
    let mut v: Vec<&NetworkRow> = rows.iter().filter(|r| matches_search(r, query)).collect();
    v.sort_by(|a, b| compare(a, b, sort));
    v.into_iter()
        .map(|r| ListNode::Item {
            key: r.key.clone(),
            item: r.clone(),
        })
        .collect()
}

/// Custom networks with no attached containers (NET-004 confirm list).
pub fn prune_candidates(rows: &[NetworkRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.builtin && r.containers == 0)
        .map(|r| r.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::ContainerState;
    use dk_core::fake::fixtures;

    #[test]
    fn net_002_rows_counts_and_builtin() {
        let nets = vec![
            fixtures::network("bridge", "bridge"),
            fixtures::network("app_default", "bridge"),
            fixtures::network("idle", "bridge"),
        ];
        let mut c = fixtures::container("web", ContainerState::Running);
        c.networks = vec!["app_default".into()];
        let r = rows(&nets, &[c]);
        assert!(r[0].builtin && !r[1].builtin);
        assert_eq!(r[1].containers, 1);
        assert_eq!(r[1].subnets, ["172.17.0.0/16"]);
        assert_eq!(prune_candidates(&r), ["idle"]);
        let names: Vec<String> = build(&r, "app", None)
            .into_iter()
            .map(|n| match n {
                ListNode::Item { item, .. } => item.name,
                ListNode::Group { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(names, ["app_default"]);
    }
}
