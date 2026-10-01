//! Container grouping (spec 21 §4, CON-010…013). Pure functions.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{ComposeInfo, ContainerSummary};

pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
pub const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";
pub const COMPOSE_NUMBER_LABEL: &str = "com.docker.compose.container-number";
pub const COMPOSE_WORKING_DIR_LABEL: &str = "com.docker.compose.project.working_dir";
pub const COMPOSE_CONFIG_FILES_LABEL: &str = "com.docker.compose.project.config_files";
pub const COMPOSE_ONEOFF_LABEL: &str = "com.docker.compose.oneoff";
pub const COMPOSE_DEPENDS_ON_LABEL: &str = "com.docker.compose.depends_on";

/// What containers are grouped by (CON-010).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "key", rename_all = "snake_case")]
pub enum GroupBy {
    #[default]
    Compose,
    None,
    Label(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregateState {
    /// All members run.
    Running,
    /// Some run (`2/3 running`).
    Partial,
    /// None run.
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupAggregate {
    pub running: usize,
    pub total: usize,
    pub state: AggregateState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerGroup {
    /// Stable key, e.g. `compose:myshop` or `label:app=web`.
    pub key: String,
    /// Display name (project name / label value).
    pub label: String,
    pub is_compose: bool,
    /// From `com.docker.compose.project.working_dir` (CON-012).
    pub working_dir: Option<String>,
    /// Indices into the input slice, in input order.
    pub members: Vec<usize>,
    pub aggregate: GroupAggregate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GroupNode {
    Group(ContainerGroup),
    /// Ungrouped container (index into the input slice).
    Container(usize),
}

/// Parse compose metadata from labels (also used for volumes/networks, rule 6).
pub fn compose_info_from_labels(labels: &BTreeMap<String, String>) -> Option<ComposeInfo> {
    let project = labels.get(COMPOSE_PROJECT_LABEL)?.clone();
    let service = labels
        .get(COMPOSE_SERVICE_LABEL)
        .cloned()
        .unwrap_or_default();
    let number = labels
        .get(COMPOSE_NUMBER_LABEL)
        .and_then(|value| value.parse().ok());
    let working_dir = labels.get(COMPOSE_WORKING_DIR_LABEL).cloned();
    let config_files = comma_separated(labels.get(COMPOSE_CONFIG_FILES_LABEL));
    let oneoff = labels
        .get(COMPOSE_ONEOFF_LABEL)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let depends_on = labels
        .get(COMPOSE_DEPENDS_ON_LABEL)
        .map(|value| {
            value
                .split(',')
                .filter_map(|entry| {
                    let service = entry.split(':').next().unwrap_or_default().trim();
                    (!service.is_empty()).then(|| service.to_owned())
                })
                .collect()
        })
        .unwrap_or_default();

    Some(ComposeInfo {
        project,
        service,
        number,
        working_dir,
        config_files,
        oneoff,
        depends_on,
    })
}

fn comma_separated(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Group `(key, label)` for one container under `by`, or `None` when ungrouped.
pub fn group_key(c: &ContainerSummary, by: &GroupBy) -> Option<(String, String)> {
    match by {
        GroupBy::None => None,
        GroupBy::Compose => {
            let project = c
                .compose
                .as_ref()
                .map(|compose| compose.project.as_str())
                .or_else(|| c.labels.get(COMPOSE_PROJECT_LABEL).map(String::as_str))?;
            Some((format!("compose:{project}"), project.to_owned()))
        }
        GroupBy::Label(key) => c
            .labels
            .get(key)
            .map(|value| (format!("label:{key}={value}"), value.clone())),
    }
}

/// Rules 1–4. Groups first (in order of first member), then ungrouped containers, unless
/// `interleave` (rule 3), which keeps first-appearance order. The UI sorts on top.
pub fn group(containers: &[ContainerSummary], by: &GroupBy, interleave: bool) -> Vec<GroupNode> {
    if matches!(by, GroupBy::None) {
        return (0..containers.len()).map(GroupNode::Container).collect();
    }

    let mut groups = Vec::<ContainerGroup>::new();
    let mut group_indices = HashMap::<String, usize>::new();
    let mut ungrouped = Vec::new();
    // An entry is either a group index or an ungrouped container index.
    let mut appearance = Vec::<(bool, usize)>::new();

    for (container_index, container) in containers.iter().enumerate() {
        let Some((key, label)) = group_key(container, by) else {
            ungrouped.push(container_index);
            appearance.push((false, container_index));
            continue;
        };

        if let Some(&group_index) = group_indices.get(&key) {
            groups[group_index].members.push(container_index);
            continue;
        }

        let group_index = groups.len();
        group_indices.insert(key.clone(), group_index);
        appearance.push((true, group_index));
        groups.push(ContainerGroup {
            key,
            label,
            is_compose: matches!(by, GroupBy::Compose),
            working_dir: if matches!(by, GroupBy::Compose) {
                container
                    .compose
                    .as_ref()
                    .and_then(|compose| compose.working_dir.clone())
                    .or_else(|| container.labels.get(COMPOSE_WORKING_DIR_LABEL).cloned())
            } else {
                None
            },
            members: vec![container_index],
            aggregate: GroupAggregate {
                running: 0,
                total: 0,
                state: AggregateState::Exited,
            },
        });
    }

    for group in &mut groups {
        let members: Vec<_> = group
            .members
            .iter()
            .map(|&index| &containers[index])
            .collect();
        group.aggregate = aggregate(&members);
        // Prefer a later member's working directory when the first member did not carry it.
        if group.working_dir.is_none() && group.is_compose {
            group.working_dir = members.iter().find_map(|container| {
                container
                    .compose
                    .as_ref()
                    .and_then(|compose| compose.working_dir.clone())
                    .or_else(|| container.labels.get(COMPOSE_WORKING_DIR_LABEL).cloned())
            });
        }
    }

    if interleave {
        appearance
            .into_iter()
            .map(|(is_group, index)| {
                if is_group {
                    GroupNode::Group(groups[index].clone())
                } else {
                    GroupNode::Container(index)
                }
            })
            .collect()
    } else {
        groups
            .into_iter()
            .map(GroupNode::Group)
            .chain(ungrouped.into_iter().map(GroupNode::Container))
            .collect()
    }
}

pub fn aggregate(members: &[&ContainerSummary]) -> GroupAggregate {
    let running = members
        .iter()
        .filter(|container| container.state.is_running())
        .count();
    let total = members.len();
    let state = if total > 0 && running == total {
        AggregateState::Running
    } else if running > 0 {
        AggregateState::Partial
    } else {
        AggregateState::Exited
    };
    GroupAggregate {
        running,
        total,
        state,
    }
}

/// CON-013 *Start all* order: stages of member indices (into `members`). Each stage may run
/// in parallel (limit 4); stages run sequentially. Uses `com.docker.compose.depends_on`
/// (`svc:condition:restart,…`); without it, one stage containing everyone. Cycles → one
/// final stage with the remainder. *Stop all* uses the stages reversed.
pub fn start_order(members: &[&ContainerSummary]) -> Vec<Vec<usize>> {
    if members.is_empty() {
        return Vec::new();
    }

    let compose: Vec<Option<ComposeInfo>> = members
        .iter()
        .map(|container| {
            container
                .compose
                .clone()
                .or_else(|| compose_info_from_labels(&container.labels))
        })
        .collect();
    if compose
        .iter()
        .all(|info| info.as_ref().is_none_or(|info| info.depends_on.is_empty()))
    {
        return vec![(0..members.len()).collect()];
    }

    let mut services: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, info) in compose.iter().enumerate() {
        if let Some(info) = info {
            services.entry(&info.service).or_default().push(index);
        }
    }

    let mut dependencies = vec![HashSet::<usize>::new(); members.len()];
    for (index, info) in compose.iter().enumerate() {
        let Some(info) = info else { continue };
        for dependency in &info.depends_on {
            if let Some(indices) = services.get(dependency.as_str()) {
                dependencies[index].extend(indices.iter().copied().filter(|&other| other != index));
            }
        }
    }

    let mut remaining: Vec<usize> = (0..members.len()).collect();
    let mut completed = HashSet::new();
    let mut stages = Vec::new();
    while !remaining.is_empty() {
        let stage: Vec<usize> = remaining
            .iter()
            .copied()
            .filter(|&index| dependencies[index].is_subset(&completed))
            .collect();
        if stage.is_empty() {
            // Deterministic cycle fallback: preserve member input order.
            stages.push(remaining);
            break;
        }
        let in_stage: HashSet<usize> = stage.iter().copied().collect();
        remaining.retain(|index| !in_stage.contains(index));
        completed.extend(stage.iter().copied());
        stages.push(stage);
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::fixtures;
    use crate::model::ContainerState;

    #[test]
    fn con_010_groups_by_compose_project() {
        let containers = vec![
            fixtures::compose_container("shop", "web", ContainerState::Running),
            fixtures::container("loose", ContainerState::Exited),
            fixtures::compose_container("shop", "db", ContainerState::Running),
            fixtures::compose_container("single", "job", ContainerState::Exited),
        ];
        let nodes = group(&containers, &GroupBy::Compose, false);
        assert_eq!(nodes.len(), 3);
        let GroupNode::Group(shop) = &nodes[0] else {
            panic!("first node was not a group")
        };
        assert_eq!(shop.key, "compose:shop");
        assert_eq!(shop.members, [0, 2]);
        let GroupNode::Group(single) = &nodes[1] else {
            panic!("single-member Compose project was not a group")
        };
        assert_eq!(single.members, [3]);
        assert_eq!(nodes[2], GroupNode::Container(1));
    }

    #[test]
    fn con_010_none_is_flat() {
        let containers = vec![
            fixtures::compose_container("shop", "web", ContainerState::Running),
            fixtures::container("loose", ContainerState::Exited),
            fixtures::compose_container("shop", "db", ContainerState::Running),
        ];
        assert_eq!(
            group(&containers, &GroupBy::None, false),
            [
                GroupNode::Container(0),
                GroupNode::Container(1),
                GroupNode::Container(2)
            ]
        );
        assert_eq!(group_key(&containers[0], &GroupBy::None), None);
    }

    #[test]
    fn con_010_interleave() {
        let containers = vec![
            fixtures::container("first", ContainerState::Running),
            fixtures::compose_container("shop", "web", ContainerState::Running),
            fixtures::container("middle", ContainerState::Exited),
            fixtures::compose_container("shop", "db", ContainerState::Exited),
            fixtures::compose_container("blog", "app", ContainerState::Running),
        ];
        let nodes = group(&containers, &GroupBy::Compose, true);
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0], GroupNode::Container(0));
        assert!(
            matches!(&nodes[1], GroupNode::Group(g) if g.key == "compose:shop" && g.members == [1, 3])
        );
        assert_eq!(nodes[2], GroupNode::Container(2));
        assert!(
            matches!(&nodes[3], GroupNode::Group(g) if g.key == "compose:blog" && g.members == [4])
        );

        // Without interleave the groups come first, then the ungrouped rows.
        let nodes = group(&containers, &GroupBy::Compose, false);
        assert!(matches!(&nodes[0], GroupNode::Group(g) if g.key == "compose:shop"));
        assert!(matches!(&nodes[1], GroupNode::Group(g) if g.key == "compose:blog"));
        assert_eq!(
            &nodes[2..],
            [GroupNode::Container(0), GroupNode::Container(2)]
        );
    }

    #[test]
    fn con_010_group_key_uses_labels_without_compose_info() {
        let mut c = fixtures::compose_container("shop", "web", ContainerState::Running);
        c.compose = None;
        assert_eq!(
            group_key(&c, &GroupBy::Compose),
            Some(("compose:shop".into(), "shop".into()))
        );
        assert_eq!(
            group_key(&c, &GroupBy::Label(COMPOSE_SERVICE_LABEL.into())),
            Some((format!("label:{COMPOSE_SERVICE_LABEL}=web"), "web".into()))
        );
        assert_eq!(compose_info_from_labels(&BTreeMap::new()), None);
    }

    #[test]
    fn con_011_aggregate_running_and_exited() {
        let a = fixtures::container("a", ContainerState::Running);
        let b = fixtures::container("b", ContainerState::Restarting);
        let c = fixtures::container("c", ContainerState::Exited);
        assert_eq!(aggregate(&[&a, &b]).state, AggregateState::Running);
        assert_eq!(aggregate(&[&c]).state, AggregateState::Exited);
        assert_eq!(aggregate(&[]).state, AggregateState::Exited);
    }

    #[test]
    fn con_013_no_depends_on_is_one_stage() {
        let a = fixtures::compose_container("shop", "a", ContainerState::Exited);
        let b = fixtures::container("b", ContainerState::Exited);
        assert_eq!(start_order(&[&a, &b]), vec![vec![0, 1]]);
        assert!(start_order(&[]).is_empty());
    }

    #[test]
    fn con_013_missing_dependency_is_root() {
        let mut a = fixtures::compose_container("shop", "a", ContainerState::Exited);
        let b = fixtures::compose_container("shop", "b", ContainerState::Exited);
        a.compose.as_mut().unwrap().depends_on = vec!["not-here".into(), "b".into()];
        assert_eq!(start_order(&[&a, &b]), vec![vec![1], vec![0]]);
    }

    #[test]
    fn con_010_custom_label() {
        let mut a = fixtures::container("a", ContainerState::Running);
        let mut b = fixtures::container("b", ContainerState::Exited);
        let c = fixtures::container("c", ContainerState::Exited);
        a.labels.insert("app".into(), "api".into());
        b.labels.insert("app".into(), "api".into());
        let nodes = group(&[a, c, b], &GroupBy::Label("app".into()), true);
        assert!(matches!(&nodes[0], GroupNode::Group(g) if g.members == [0, 2]));
        assert_eq!(nodes[1], GroupNode::Container(1));
    }

    #[test]
    fn con_011_aggregate_partial() {
        let running = fixtures::container("running", ContainerState::Running);
        let stopped = fixtures::container("stopped", ContainerState::Exited);
        assert_eq!(
            aggregate(&[&running, &stopped]),
            GroupAggregate {
                running: 1,
                total: 2,
                state: AggregateState::Partial,
            }
        );
    }

    #[test]
    fn con_012_working_dir() {
        let c = fixtures::compose_container("shop", "web", ContainerState::Running);
        let GroupNode::Group(group) = &group(&[c], &GroupBy::Compose, false)[0] else {
            panic!("expected group")
        };
        assert_eq!(group.working_dir.as_deref(), Some("/home/dev/proj"));
    }

    #[test]
    fn con_013_parses_compose_metadata() {
        let labels = BTreeMap::from([
            (COMPOSE_PROJECT_LABEL.into(), "shop".into()),
            (COMPOSE_SERVICE_LABEL.into(), "web".into()),
            (COMPOSE_NUMBER_LABEL.into(), "2".into()),
            (COMPOSE_CONFIG_FILES_LABEL.into(), "a.yml, b.yml".into()),
            (COMPOSE_ONEOFF_LABEL.into(), "True".into()),
            (
                COMPOSE_DEPENDS_ON_LABEL.into(),
                "db:service_healthy:false, cache".into(),
            ),
        ]);
        let info = compose_info_from_labels(&labels).expect("project label");
        assert_eq!(info.number, Some(2));
        assert_eq!(info.config_files, ["a.yml", "b.yml"]);
        assert!(info.oneoff);
        assert_eq!(info.depends_on, ["db", "cache"]);
    }

    #[test]
    fn con_013_start_order_respects_depends_on() {
        let db = fixtures::compose_container("shop", "db", ContainerState::Exited);
        let mut api = fixtures::compose_container("shop", "api", ContainerState::Exited);
        let mut web = fixtures::compose_container("shop", "web", ContainerState::Exited);
        api.compose.as_mut().unwrap().depends_on = vec!["db".into()];
        web.labels.insert(
            COMPOSE_DEPENDS_ON_LABEL.into(),
            "api:service_started:false".into(),
        );
        // Exercise the labels fallback for one member.
        web.compose = None;
        assert_eq!(
            start_order(&[&web, &db, &api]),
            vec![vec![1], vec![2], vec![0]]
        );
    }

    #[test]
    fn con_013_cycle_falls_back() {
        let mut a = fixtures::compose_container("shop", "a", ContainerState::Exited);
        let mut b = fixtures::compose_container("shop", "b", ContainerState::Exited);
        let c = fixtures::compose_container("shop", "c", ContainerState::Exited);
        a.compose.as_mut().unwrap().depends_on = vec!["b".into()];
        b.compose.as_mut().unwrap().depends_on = vec!["a".into()];
        assert_eq!(start_order(&[&a, &b, &c]), vec![vec![2], vec![0, 1]]);
    }
}
