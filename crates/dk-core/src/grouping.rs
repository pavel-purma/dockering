//! Container grouping (spec 21 §4, CON-010…013). Pure functions.
//!
//! SIGNATURES FIXED — bodies implemented by `rust-core`.

use std::collections::BTreeMap;

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
    let _ = labels;
    unimplemented!("rust-core")
}

/// Group `(key, label)` for one container under `by`, or `None` when ungrouped.
pub fn group_key(c: &ContainerSummary, by: &GroupBy) -> Option<(String, String)> {
    let _ = (c, by);
    unimplemented!("rust-core")
}

/// Rules 1–4. Groups first (in order of first member), then ungrouped containers, unless
/// `interleave` (rule 3), which keeps first-appearance order. The UI sorts on top.
pub fn group(containers: &[ContainerSummary], by: &GroupBy, interleave: bool) -> Vec<GroupNode> {
    let _ = (containers, by, interleave);
    unimplemented!("rust-core")
}

pub fn aggregate(members: &[&ContainerSummary]) -> GroupAggregate {
    let _ = members;
    unimplemented!("rust-core")
}

/// CON-013 *Start all* order: stages of member indices (into `members`). Each stage may run
/// in parallel (limit 4); stages run sequentially. Uses `com.docker.compose.depends_on`
/// (`svc:condition:restart,…`); without it, one stage containing everyone. Cycles → one
/// final stage with the remainder. *Stop all* uses the stages reversed.
pub fn start_order(members: &[&ContainerSummary]) -> Vec<Vec<usize>> {
    let _ = members;
    unimplemented!("rust-core")
}
