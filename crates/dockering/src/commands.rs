//! `CommandRegistry` (KBD-020, KBD-093): every palette entry with its label, action, group
//! and an availability predicate. The palette adds dynamic "Go to <resource>" entries.

use gpui_kit::{Action, App};

use crate::actions::detail_tab::SelectTab;
use crate::actions::*;
use crate::nav::ContainerTab;
use crate::strings as s;

/// Palette group headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandGroup {
    General,
    Navigation,
    Engines,
    Containers,
    /// M6
    Resources,
    View,
}

impl CommandGroup {
    pub fn label(self) -> &'static str {
        match self {
            CommandGroup::General => "General",
            CommandGroup::Navigation => "Navigation",
            CommandGroup::Engines => "Engines",
            CommandGroup::Containers => "Containers",
            CommandGroup::Resources => "Images, volumes & networks",
            CommandGroup::View => "View",
        }
    }
}

/// When a command is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// Only with an active engine.
    Engine,
    /// Only on the Containers page.
    ContainersPage,
    // ── M6 ──
    /// The Images list.
    ImagesPage,
    /// The Images list or an image detail page.
    ImagePages,
    /// An image detail page.
    ImageDetail,
    /// The Volumes list.
    VolumesPage,
    /// The Networks list.
    NetworksPage,
    /// Any of the Images, Volumes, Networks lists.
    ResourceLists,
    /// Any of the Images, Volumes, Networks lists or detail pages.
    ResourcePages,
    // ── container detail (wip/detail) ──
    /// The container detail page (any tab).
    ContainerDetail,
    /// A verified update is ready to install (KBD-076).
    UpdateReady,
}

/// Context passed to availability checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandContext {
    pub has_engine: bool,
    pub on_containers: bool,
    /// M6: the sidebar page of the current route, and whether it is a detail page.
    pub page: Option<crate::nav::Page>,
    pub on_detail: bool,
    /// KBD-076: *Restart to update* is offered only then.
    pub update_ready: bool,
}

pub struct CommandSpec {
    pub label: &'static str,
    pub action: fn() -> Box<dyn Action>,
    pub group: CommandGroup,
    pub when: When,
    pub keywords: &'static [&'static str],
}

macro_rules! c {
    ($label:expr, $action:expr, $group:ident, $when:ident) => {
        CommandSpec {
            label: $label,
            action: || Box::new($action),
            group: CommandGroup::$group,
            when: When::$when,
            keywords: &[],
        }
    };
    ($label:expr, $action:expr, $group:ident, $when:ident, $kw:expr) => {
        CommandSpec {
            label: $label,
            action: || Box::new($action),
            group: CommandGroup::$group,
            when: When::$when,
            keywords: $kw,
        }
    };
}

/// The static registry. Every action that has no default key binding must be listed here
/// (KBD-093, `a11y_every_action_bound_or_in_palette`).
pub static COMMANDS: &[CommandSpec] = &[
    c!(s::CMD_GO_CONTAINERS, GoContainers, Navigation, Always),
    c!(s::CMD_GO_IMAGES, GoImages, Navigation, Always),
    c!(s::CMD_GO_VOLUMES, GoVolumes, Navigation, Always),
    c!(s::CMD_GO_NETWORKS, GoNetworks, Navigation, Always),
    c!(
        s::CMD_SETTINGS,
        OpenSettings,
        Navigation,
        Always,
        &["preferences"]
    ),
    c!(s::CMD_BACK, Back, Navigation, Always),
    c!(s::CMD_FORWARD, Forward, Navigation, Always),
    c!(
        s::CMD_FOCUS_SEARCH,
        FocusSearch,
        Navigation,
        Always,
        &["find"]
    ),
    c!(s::CMD_NEXT_REGION, NextRegion, Navigation, Always),
    c!(s::CMD_PREV_REGION, PrevRegion, Navigation, Always),
    c!(
        s::CMD_FOCUS_NOTIFICATIONS,
        FocusNotifications,
        Navigation,
        Always
    ),
    c!(s::CMD_PARENT_LIST, detail::ParentList, Navigation, Always),
    c!(s::CMD_NEXT_TAB, detail::NextTab, Navigation, Always),
    c!(s::CMD_PREV_TAB, detail::PrevTab, Navigation, Always),
    c!(
        s::CMD_ENGINE_SWITCHER,
        EngineSwitcher,
        Engines,
        Always,
        &["engine", "context"]
    ),
    c!(s::CMD_RESCAN, Rescan, Engines, Always, &["discover"]),
    c!(s::CMD_MANAGE_ENGINES, ManageEngines, Engines, Always),
    c!(
        s::CMD_SET_ACTIVE_DEFAULT,
        SetActiveEngineDefault,
        Engines,
        Engine,
        &["pin", "startup"]
    ),
    c!(s::RETRY, RetryEngine, Engines, Engine, &["reconnect"]),
    c!(
        s::START_AND_CONNECT,
        StartEngine,
        Engines,
        Engine,
        &["wsl", "boot"]
    ),
    c!(s::CMD_REFRESH, Refresh, General, Engine, &["reload"]),
    c!(s::CMD_PALETTE, CommandPalette, General, Always),
    c!(
        s::CMD_SHORTCUTS,
        ShortcutReference,
        General,
        Always,
        &["keys", "help", "keyboard"]
    ),
    c!(s::CMD_ABOUT, About, General, Always),
    c!(s::CMD_OPEN_LOGS_FOLDER, OpenLogsFolder, General, Always),
    c!(
        s::CMD_CHECK_FOR_UPDATES,
        CheckForUpdates,
        General,
        Always,
        &["update", "upgrade", "version"]
    ),
    c!(
        s::CMD_RESTART_TO_UPDATE,
        RestartToUpdate,
        General,
        UpdateReady,
        &["update", "install"]
    ),
    c!(
        s::CMD_VIEW_RELEASE_NOTES,
        ViewReleaseNotes,
        General,
        Always,
        &["changelog", "what's new"]
    ),
    c!(s::CMD_QUIT, Quit, General, Always, &["exit"]),
    c!(
        s::CMD_TOGGLE_THEME,
        ToggleTheme,
        View,
        Always,
        &["dark", "light"]
    ),
    c!(s::CMD_TOGGLE_SIDEBAR, ToggleSidebar, View, Always),
    c!(s::CMD_ZOOM_IN, ZoomIn, View, Always),
    c!(s::CMD_ZOOM_OUT, ZoomOut, View, Always),
    c!(s::CMD_ZOOM_RESET, ZoomReset, View, Always),
    c!(s::CMD_MINIMIZE, Minimize, View, Always),
    c!(s::CMD_ZOOM_WINDOW, ZoomWindow, View, Always, &["maximize"]),
    c!(
        s::CMD_PRUNE_CONTAINERS,
        list::Prune,
        Containers,
        ContainersPage,
        &["clean"]
    ),
    c!(
        s::CMD_GROUP_BY,
        list::GroupBy,
        Containers,
        ContainersPage,
        &["compose"]
    ),
    c!(s::CMD_SORT_BY, list::SortMenu, Containers, ContainersPage),
    c!(
        s::CMD_FOCUS_FILTER,
        list::FocusFilter,
        Containers,
        ContainersPage,
        &["running", "stopped"]
    ),
    c!(
        "Expand/collapse group",
        list::ToggleGroup,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_COLLAPSE_ALL,
        list::CollapseAll,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_EXPAND_ALL,
        list::ExpandAll,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_SELECT_ALL,
        list::SelectAll,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_CLEAR_SELECTION,
        list::ClearSelection,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_BULK_START,
        list::BulkStart,
        Containers,
        ContainersPage
    ),
    c!(s::CMD_BULK_STOP, list::BulkStop, Containers, ContainersPage),
    c!(
        s::CMD_BULK_DELETE,
        list::BulkDelete,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_QUICK_FIND,
        list::QuickFind,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_START_STOP,
        container::StartStop,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_RESTART,
        container::Restart,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_PAUSE,
        container::PauseToggle,
        Containers,
        ContainersPage
    ),
    c!(s::CMD_KILL, container::Kill, Containers, ContainersPage),
    c!(s::CMD_LOGS, container::Logs, Containers, ContainersPage),
    c!(
        s::CMD_TERMINAL,
        container::Terminal,
        Containers,
        ContainersPage,
        &["shell", "exec"]
    ),
    c!(
        s::CMD_INSPECT,
        container::Inspect,
        Containers,
        ContainersPage
    ),
    c!(
        s::CMD_OPEN_PORT,
        container::OpenPort,
        Containers,
        ContainersPage,
        &["browser"]
    ),
    c!(s::CMD_COPY_ID, list::CopyId, Containers, ContainersPage),
    c!(
        s::CMD_DELETE,
        list::Delete,
        Containers,
        ContainersPage,
        &["remove"]
    ),
    // ── M6: images, volumes, networks ─────────────────────────────────────────────────
    c!(
        s::CMD_PULL_IMAGE,
        image::Pull,
        Resources,
        ImagesPage,
        &["download", "docker pull"]
    ),
    c!(
        s::CMD_RUN_IMAGE,
        image::Run,
        Resources,
        ImagePages,
        &["start"]
    ),
    c!(s::CMD_TAG_IMAGE, res::TagImage, Resources, ImageDetail),
    c!(s::CMD_COPY_DIGEST, res::CopyDigest, Resources, ImageDetail),
    c!(
        s::CMD_PRUNE_DANGLING,
        res::PruneDangling,
        Resources,
        ImagesPage,
        &["clean"]
    ),
    c!(
        s::CMD_PRUNE_UNUSED_IMAGES,
        res::PruneUnused,
        Resources,
        ImagesPage,
        &["clean"]
    ),
    c!(
        s::CMD_NEW_VOLUME,
        volume::Create,
        Resources,
        VolumesPage,
        &["create"]
    ),
    c!(
        s::CMD_PRUNE_VOLUMES,
        list::Prune,
        Resources,
        VolumesPage,
        &["clean"]
    ),
    c!(
        s::CMD_PRUNE_NETWORKS,
        list::Prune,
        Resources,
        NetworksPage,
        &["clean"]
    ),
    c!(s::CMD_SORT_BY, list::SortMenu, Resources, ResourceLists),
    c!(
        s::CMD_FOCUS_FILTER,
        list::FocusFilter,
        Resources,
        ResourceLists
    ),
    c!(s::CMD_SELECT_ALL, list::SelectAll, Resources, ResourceLists),
    c!(
        s::CMD_BULK_DELETE,
        list::BulkDelete,
        Resources,
        ResourceLists
    ),
    c!(s::CMD_QUICK_FIND, list::QuickFind, Resources, ResourceLists),
    c!(
        s::CMD_COPY_RESOURCE_ID,
        list::CopyId,
        Resources,
        ResourcePages
    ),
    c!(
        s::CMD_DELETE_RESOURCE,
        list::Delete,
        Resources,
        ResourcePages,
        &["remove"]
    ), // ── container detail (CDT-*, LOG-*, TRM-*, STA-*; wip/detail) ─────────────────────────
    // KBD-040: "Go to tab …" entries, then the tab commands without a default chord.
    c!(
        s::CMD_DETAIL_TAB_OVERVIEW,
        tab(ContainerTab::Overview),
        Containers,
        ContainerDetail,
        &["tab"]
    ),
    c!(
        s::CMD_DETAIL_TAB_LOGS,
        tab(ContainerTab::Logs),
        Containers,
        ContainerDetail,
        &["tab"]
    ),
    c!(
        s::CMD_DETAIL_TAB_TERMINAL,
        tab(ContainerTab::Terminal),
        Containers,
        ContainerDetail,
        &["tab", "shell"]
    ),
    c!(
        s::CMD_DETAIL_TAB_STATS,
        tab(ContainerTab::Stats),
        Containers,
        ContainerDetail,
        &["tab", "cpu", "memory"]
    ),
    c!(
        s::CMD_DETAIL_TAB_MOUNTS,
        tab(ContainerTab::Mounts),
        Containers,
        ContainerDetail,
        &["tab", "volumes"]
    ),
    c!(
        s::CMD_DETAIL_TAB_NETWORK,
        tab(ContainerTab::Network),
        Containers,
        ContainerDetail,
        &["tab", "ports"]
    ),
    c!(
        s::CMD_DETAIL_TAB_INSPECT,
        tab(ContainerTab::Inspect),
        Containers,
        ContainerDetail,
        &["tab", "json"]
    ),
    c!(
        s::CMD_START_STOP,
        container::StartStop,
        Containers,
        ContainerDetail
    ),
    c!(
        s::CMD_RESTART,
        container::Restart,
        Containers,
        ContainerDetail
    ),
    c!(
        s::CMD_PAUSE,
        container::PauseToggle,
        Containers,
        ContainerDetail
    ),
    c!(s::CMD_KILL, container::Kill, Containers, ContainerDetail),
    c!(
        s::CMD_OPEN_PORT,
        container::OpenPort,
        Containers,
        ContainerDetail,
        &["browser"]
    ),
    c!(s::CMD_COPY_ID, list::CopyId, Containers, ContainerDetail),
    c!(
        s::CMD_DELETE,
        list::Delete,
        Containers,
        ContainerDetail,
        &["remove"]
    ),
    c!(
        s::CMD_TERM_RECONNECT,
        term_ext::Reconnect,
        Containers,
        ContainerDetail,
        &["shell"]
    ),
    c!(
        s::CMD_TERM_EXTERNAL,
        term_ext::OpenExternal,
        Containers,
        ContainerDetail,
        &["shell", "console"]
    ),
    c!(
        s::CMD_STATS_1M,
        stats::Window1m,
        Containers,
        ContainerDetail
    ),
    c!(
        s::CMD_STATS_5M,
        stats::Window5m,
        Containers,
        ContainerDetail
    ),
    c!(
        s::CMD_STATS_15M,
        stats::Window15m,
        Containers,
        ContainerDetail
    ),
    c!(
        s::CMD_STATS_DISK,
        stats::LoadDiskUsage,
        Containers,
        ContainerDetail,
        &["size"]
    ),
    // ── M9: Settings (SET-*, ENG-105) ──
    c!(
        s::CMD_ADD_ENGINE,
        settings::AddEngine,
        Engines,
        Always,
        &["remote", "tcp", "tls", "connect"]
    ),
    c!(
        s::CMD_COPY_DIAGNOSTICS,
        settings::CopyDiagnostics,
        General,
        Always,
        &["support", "bug"]
    ),
    c!(
        s::CMD_VIEW_LICENSES,
        settings::ViewLicenses,
        General,
        Always,
        &["licenses", "notices", "about"]
    ),
];

/// `detail::SelectTab` for a palette entry.
const fn tab(tab: ContainerTab) -> SelectTab {
    SelectTab { tab }
}

/// M6 availability (`When::ImagesPage` …).
fn resource_when(when: When, cx: &CommandContext) -> bool {
    use crate::nav::Page;
    let page = cx.page;
    match when {
        When::ImagesPage => page == Some(Page::Images) && !cx.on_detail,
        When::ImagePages => page == Some(Page::Images),
        When::ImageDetail => page == Some(Page::Images) && cx.on_detail,
        When::VolumesPage => page == Some(Page::Volumes) && !cx.on_detail,
        When::NetworksPage => page == Some(Page::Networks) && !cx.on_detail,
        When::ResourceLists => {
            matches!(page, Some(Page::Images | Page::Volumes | Page::Networks)) && !cx.on_detail
        }
        When::ResourcePages => matches!(page, Some(Page::Images | Page::Volumes | Page::Networks)),
        When::ContainerDetail => page == Some(Page::Containers) && cx.on_detail,
        When::Always | When::Engine | When::ContainersPage | When::UpdateReady => false,
    }
}

impl CommandSpec {
    pub fn available(&self, cx: &CommandContext) -> bool {
        match self.when {
            When::Always => true,
            When::Engine => cx.has_engine,
            When::ContainersPage => cx.has_engine && cx.on_containers,
            When::UpdateReady => cx.update_ready,
            other => cx.has_engine && resource_when(other, cx),
        }
    }
}

/// Commands offered in `cx`.
pub fn available(cx: &CommandContext) -> impl Iterator<Item = &'static CommandSpec> + '_ {
    COMMANDS.iter().filter(move |c| c.available(cx))
}

/// Actions intentionally reachable only through UI elements that are themselves
/// keyboard-operable (rows' `OnRow`, switcher rows, internal plumbing), excluded from KBD-093.
pub fn plumbing_action(name: &str) -> bool {
    const PLUMBING: &[&str] = &[
        "dk::Navigate",
        "dk::SwitchEngine",
        "dk::CopyText",
        "dk::OpenUrl",
        "dk::CloseWindow",
        // Windows/Linux only (bound to a lone Alt there); macOS has the native menu bar.
        "dk::FocusMenuBar",
        "dk::Hide",
        "dk::HideOthers",
        "list::OnRow",
        "list::SortByColumn",
        "list::SetFilter",
        "list::SetGroupBy",
        // M6: tab-switch plumbing and row-cell buttons (the keyed equivalents are bound).
        "res::ReplaceRoute",
        "res::RunRow",
        // M9: Settings › Engines row buttons (Tab-reachable, KBD-075).
        "settings::EngineOp",
        // wip/detail: click plumbing (the keyed equivalents are bound or in the palette).
        "detail::SelectTab",
        "rows::RevealRow",
        "term::SelectSession",
    ];
    PLUMBING.contains(&name)
}

/// For the shortcut-reference & palette: does the app know this action at all.
pub fn app_has_action(name: &str, cx: &App) -> bool {
    cx.all_action_names().contains(&name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::keymap::{DEFAULT_KEYMAP, Os};

    /// KBD-093: every registered action of ours is bound on every OS, or in the palette.
    #[test]
    fn a11y_every_action_bound_or_in_palette() {
        let in_palette: HashSet<&str> = COMMANDS.iter().map(|c| (c.action)().name()).collect();
        let mut names: Vec<&'static str> = Vec::new();
        // Collect names from the action registry via inventory (same as GPUI's).
        for b in gpui_kit::private::inventory::iter::<gpui_kit::MacroActionBuilder> {
            names.push((b.0)().name);
        }
        let ours: Vec<&str> = names
            .into_iter()
            .filter(|n| {
                crate::actions::NAMESPACES
                    .iter()
                    .any(|ns| n.starts_with(ns))
            })
            .collect();
        assert!(!ours.is_empty(), "action registry enumeration works");
        for os in [Os::WINDOWS, Os::MACOS, Os::LINUX] {
            let bound: HashSet<&str> = DEFAULT_KEYMAP
                .iter()
                .filter(|b| b.os.contains(os))
                .map(|b| (b.action)().name())
                .collect();
            let missing: Vec<&&str> = ours
                .iter()
                .filter(|n| !(bound.contains(*n) || in_palette.contains(*n) || plumbing_action(n)))
                .collect();
            assert!(
                missing.is_empty(),
                "KBD-093: neither bound on {os:?} nor in the palette: {missing:?}"
            );
        }
    }

    #[test]
    fn availability() {
        let none = CommandContext::default();
        assert!(available(&none).all(|c| c.when == When::Always));
        let all = CommandContext {
            has_engine: true,
            on_containers: true,
            ..Default::default()
        };
        assert!(available(&all).all(|c| !matches!(c.group, CommandGroup::Resources)));
    }
}
