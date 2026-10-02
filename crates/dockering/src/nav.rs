//! Navigation (spec 30 §2): `Route`, back/forward history, and engine-switch fallbacks.

use dk_core::grouping::GroupBy;
use serde::{Deserialize, Serialize};

use crate::strings as s;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ContainerTab {
    #[default]
    Overview,
    Logs,
    Terminal,
    Stats,
    Mounts,
    Network,
    Inspect,
}

impl ContainerTab {
    pub const ALL: [ContainerTab; 7] = [
        ContainerTab::Overview,
        ContainerTab::Logs,
        ContainerTab::Terminal,
        ContainerTab::Stats,
        ContainerTab::Mounts,
        ContainerTab::Network,
        ContainerTab::Inspect,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ContainerTab::Overview => "Overview",
            ContainerTab::Logs => "Logs",
            ContainerTab::Terminal => "Terminal",
            ContainerTab::Stats => "Stats",
            ContainerTab::Mounts => "Mounts",
            ContainerTab::Network => "Network",
            ContainerTab::Inspect => "Inspect",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ImageTab {
    #[default]
    Overview,
    Layers,
    UsedBy,
    Inspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum VolumeTab {
    #[default]
    Overview,
    UsedBy,
    Inspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NetworkTab {
    #[default]
    Overview,
    Containers,
    Inspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SettingsSection {
    #[default]
    General,
    Engines,
    Containers,
    Logs,
    Terminal,
    Stats,
    Diagnostics,
    Keyboard,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 8] = [
        SettingsSection::General,
        SettingsSection::Engines,
        SettingsSection::Containers,
        SettingsSection::Logs,
        SettingsSection::Terminal,
        SettingsSection::Stats,
        SettingsSection::Diagnostics,
        SettingsSection::Keyboard,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::General => "General",
            SettingsSection::Engines => "Engines",
            SettingsSection::Containers => "Containers",
            SettingsSection::Logs => "Logs",
            SettingsSection::Terminal => "Terminal",
            SettingsSection::Stats => "Stats",
            SettingsSection::Diagnostics => "Diagnostics",
            SettingsSection::Keyboard => "Keyboard",
        }
    }
}

/// Exactly the routes of spec 30 §2.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Route {
    Containers,
    ContainerDetail { id: String, tab: ContainerTab },
    Images,
    ImageDetail { id: String, tab: ImageTab },
    Volumes,
    VolumeDetail { name: String, tab: VolumeTab },
    Networks,
    NetworkDetail { id: String, tab: NetworkTab },
    Settings { section: SettingsSection },
}

/// Sidebar pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Containers,
    Images,
    Volumes,
    Networks,
}

impl Page {
    pub const ALL: [Page; 4] = [
        Page::Containers,
        Page::Images,
        Page::Volumes,
        Page::Networks,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Page::Containers => s::PAGE_CONTAINERS,
            Page::Images => s::PAGE_IMAGES,
            Page::Volumes => s::PAGE_VOLUMES,
            Page::Networks => s::PAGE_NETWORKS,
        }
    }
    pub fn route(self) -> Route {
        match self {
            Page::Containers => Route::Containers,
            Page::Images => Route::Images,
            Page::Volumes => Route::Volumes,
            Page::Networks => Route::Networks,
        }
    }
}

impl Route {
    /// The sidebar page this route belongs to (`None` for Settings).
    pub fn page(&self) -> Option<Page> {
        match self {
            Route::Containers | Route::ContainerDetail { .. } => Some(Page::Containers),
            Route::Images | Route::ImageDetail { .. } => Some(Page::Images),
            Route::Volumes | Route::VolumeDetail { .. } => Some(Page::Volumes),
            Route::Networks | Route::NetworkDetail { .. } => Some(Page::Networks),
            Route::Settings { .. } => None,
        }
    }

    pub fn is_detail(&self) -> bool {
        matches!(
            self,
            Route::ContainerDetail { .. }
                | Route::ImageDetail { .. }
                | Route::VolumeDetail { .. }
                | Route::NetworkDetail { .. }
        )
    }

    /// The parent list of a detail route; list routes return themselves (spec 10 §4.3).
    pub fn parent_list(&self) -> Route {
        match self {
            Route::ContainerDetail { .. } => Route::Containers,
            Route::ImageDetail { .. } => Route::Images,
            Route::VolumeDetail { .. } => Route::Volumes,
            Route::NetworkDetail { .. } => Route::Networks,
            other => other.clone(),
        }
    }

    /// Engine-specific ids don't survive an engine switch: detail routes fall back to their
    /// parent list (spec 10 §4.3).
    pub fn for_engine_switch(&self) -> Route {
        self.parent_list()
    }

    /// Persisted form (`UiState.last_route`); only list pages are stored.
    pub fn to_state(&self) -> Option<String> {
        if self.is_detail() {
            return None;
        }
        serde_json::to_string(self).ok()
    }

    pub fn from_state(s: &str) -> Option<Route> {
        serde_json::from_str::<Route>(s)
            .ok()
            .filter(|r| !r.is_detail())
    }
}

/// Back/forward history (KBD-027). Pure data; the view layer wraps it.
#[derive(Debug, Clone)]
pub struct History {
    current: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
}

const MAX_HISTORY: usize = 100;

impl History {
    pub fn new(start: Route) -> Self {
        Self {
            current: start,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    pub fn current(&self) -> &Route {
        &self.current
    }

    /// Pushes `route`; returns false if it is already current.
    pub fn push(&mut self, route: Route) -> bool {
        if route == self.current {
            return false;
        }
        let prev = std::mem::replace(&mut self.current, route);
        self.back.push(prev);
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
        true
    }

    /// Replaces the current route without touching history (tab switches on a detail page).
    pub fn replace(&mut self, route: Route) {
        self.current = route;
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> bool {
        let Some(prev) = self.back.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.current, prev);
        self.forward.push(cur);
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.current, next);
        self.back.push(cur);
        true
    }

    /// Engine switch (spec 10 §4.3): keep list routes, map details to their parent, and drop
    /// history entries that point at engine-specific ids.
    pub fn engine_switched(&mut self) {
        self.current = self.current.for_engine_switch();
        self.back.retain(|r| !r.is_detail());
        self.forward.retain(|r| !r.is_detail());
        self.back.dedup();
        self.forward.dedup();
        if self.back.last() == Some(&self.current) {
            self.back.pop();
        }
    }
}

/// Group-by mode as a `SetGroupBy` string (`compose`, `none`, `label:<key>`).
pub fn group_by_to_mode(g: &GroupBy) -> String {
    match g {
        GroupBy::Compose => "compose".into(),
        GroupBy::None => "none".into(),
        GroupBy::Label(k) => format!("label:{k}"),
    }
}

pub fn group_by_from_mode(mode: &str) -> GroupBy {
    match mode {
        "none" => GroupBy::None,
        "compose" => GroupBy::Compose,
        m => match m.strip_prefix("label:") {
            Some(k) if !k.trim().is_empty() => GroupBy::Label(k.trim().to_owned()),
            _ => GroupBy::Compose,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(id: &str) -> Route {
        Route::ContainerDetail {
            id: id.into(),
            tab: ContainerTab::Overview,
        }
    }

    #[test]
    fn history_back_forward() {
        let mut h = History::new(Route::Containers);
        assert!(h.push(Route::Images));
        assert!(h.push(detail("a")));
        assert!(!h.push(detail("a")));
        assert!(h.back());
        assert_eq!(h.current(), &Route::Images);
        assert!(h.forward());
        assert_eq!(h.current(), &detail("a"));
        assert!(!h.forward());
        h.back();
        h.push(Route::Volumes);
        assert!(!h.can_forward(), "a push clears forward history");
    }

    #[test]
    fn eng_102_engine_switch_keeps_lists_and_drops_details() {
        let mut h = History::new(Route::Containers);
        h.push(Route::Images);
        h.push(Route::Containers);
        h.push(detail("x"));
        h.engine_switched();
        assert_eq!(h.current(), &Route::Containers);
        assert!(h.back.iter().all(|r| !r.is_detail()));
        let mut h = History::new(Route::Volumes);
        h.engine_switched();
        assert_eq!(h.current(), &Route::Volumes);
    }

    #[test]
    fn route_state_roundtrip_lists_only() {
        let s = Route::Images.to_state().unwrap();
        assert_eq!(Route::from_state(&s), Some(Route::Images));
        assert_eq!(detail("a").to_state(), None);
        assert_eq!(Route::from_state("garbage"), None);
    }

    #[test]
    fn group_by_modes_roundtrip() {
        for g in [
            GroupBy::Compose,
            GroupBy::None,
            GroupBy::Label("app".into()),
        ] {
            assert_eq!(group_by_from_mode(&group_by_to_mode(&g)), g);
        }
        assert_eq!(group_by_from_mode("label: "), GroupBy::Compose);
    }
}
