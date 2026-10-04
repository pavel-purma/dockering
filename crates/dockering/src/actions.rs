//! Every user command is a GPUI `Action` (KBD-002). Buttons, menus, the command palette, and
//! key bindings all dispatch these; no logic lives only in click handlers.
//!
//! Bindings are in [`crate::keymap`]; palette entries in [`crate::commands`].

use gpui_kit::{Action, SharedString};

use crate::nav::Route;

gpui_kit::actions!(
    dk,
    [
        /// `Mod+Shift+P`, `Mod+P` (KBD-020)
        CommandPalette,
        /// `Mod+/`, `F1` (KBD-022)
        ShortcutReference,
        /// `Mod+K` (KBD-021)
        EngineSwitcher,
        GoContainers,
        GoImages,
        GoVolumes,
        GoNetworks,
        OpenSettings,
        FocusSearch,
        Refresh,
        Back,
        Forward,
        ToggleSidebar,
        NextRegion,
        PrevRegion,
        ToggleTheme,
        FocusNotifications,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Quit,
        CloseWindow,
        Rescan,
        ManageEngines,
        About,
        OpenLogsFolder,
        Minimize,
        ZoomWindow,
        Hide,
        HideOthers,
        /// Retry connecting the active engine (SHL-013).
        RetryEngine,
        /// Boot a stopped WSL distro and connect (ENG-106).
        StartEngine,
        /// Pin the active engine as the startup default (ENG-116).
        SetActiveEngineDefault,
        /// Lone `Alt` on Windows/Linux: focus the title-bar overflow menu (SHL-021).
        FocusMenuBar,
        /// KBD-076 / UPD-004: manual update check (palette, Settings › Updates, macOS menu).
        CheckForUpdates,
        /// KBD-076 / UPD-007: start the downloaded installer and quit.
        RestartToUpdate,
        /// KBD-076: release notes of the offered (or current) version.
        ViewReleaseNotes,
    ]
);

/// Navigate to a route (palette "Go to …" entries, cross links).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = dk, no_json)]
pub struct Navigate {
    pub route: Route,
}

/// Switch the active engine (engine switcher rows).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = dk, no_json)]
pub struct SwitchEngine {
    pub id: SharedString,
}

/// Copy text to the clipboard (Copy id, Copy details). Handled app-wide.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = dk, no_json)]
pub struct CopyText {
    pub text: SharedString,
}

/// Open a URL in the default browser (port links). Handled app-wide.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = dk, no_json)]
pub struct OpenUrl {
    pub url: SharedString,
}

pub mod sidebar {
    gpui_kit::actions!(
        sidebar,
        [
            /// Roving cursor in the sidebar (KBD-004).
            Prev,
            Next,
            First,
            Last,
            Activate,
        ]
    );
}

pub mod list {
    use gpui_kit::{Action, SharedString};

    gpui_kit::actions!(
        list,
        [
            /// `Enter` (KBD-032)
            OpenDetail,
            ToggleGroup,
            /// `←` (KBD-033)
            CollapseOrParent,
            /// `→`
            Expand,
            CollapseAll,
            ExpandAll,
            /// `Space` (KBD-034)
            ToggleRowSelected,
            /// `Shift+↑/↓` (KBD-035)
            ExtendUp,
            ExtendDown,
            SelectAll,
            ClearSelection,
            /// `Esc` inside a list (KBD-006 chain).
            Escape,
            First,
            Last,
            /// `Tab` / `Shift+Tab` leave the table (KBD-004, S-8).
            FocusNext,
            FocusPrev,
            /// `Shift+F10`, `Menu` (KBD-036)
            ContextMenu,
            /// `C`
            CopyId,
            /// `Del` (KBD-030)
            Delete,
            BulkStart,
            BulkStop,
            BulkDelete,
            /// `/` (KBD-037)
            QuickFind,
            QuickFindNext,
            QuickFindClose,
            /// `Mod+Shift+G` (KBD-039)
            GroupBy,
            /// `Mod+Shift+O`
            SortMenu,
            /// `Mod+Shift+F`
            FocusFilter,
            /// CON-023
            Prune,
        ]
    );

    /// Sort by a column key (sort menu, palette).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = list, no_json)]
    pub struct SortByColumn {
        pub key: SharedString,
    }

    /// Status filter segment (CON-004).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = list, no_json)]
    pub struct SetFilter {
        pub filter: SharedString,
    }

    /// Group-by mode (CON-010): `compose`, `none`, or `label:<key>`.
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = list, no_json)]
    pub struct SetGroupBy {
        pub mode: SharedString,
    }

    /// A row-scoped command from a cell button or a menu: makes `row` the cursor, then runs
    /// `action` on it (so clicks and keys share one code path).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = list, no_json)]
    pub struct OnRow {
        pub row: SharedString,
        pub action: RowCommand,
    }

    /// Row-scoped commands (see [`OnRow`]).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RowCommand {
        Open,
        ToggleGroup,
        ToggleSelected,
        Start,
        Stop,
        Restart,
        Pause,
        Unpause,
        Kill,
        Delete,
        Logs,
        Terminal,
        Inspect,
        CopyId,
        OpenPort,
        ContextMenu,
    }
}

pub mod container {
    gpui_kit::actions!(
        container,
        [
            /// `S` start/stop toggle (KBD-030)
            StartStop,
            /// `R`
            Restart,
            /// `P` (capability `PAUSE`)
            PauseToggle,
            Kill,
            /// `L`
            Logs,
            /// `T` (capability `EXEC_TTY`)
            Terminal,
            /// `I`
            Inspect,
            /// `O`
            OpenPort,
        ]
    );
}

pub mod image {
    gpui_kit::actions!(
        image,
        [
            /// `U` (phase 2)
            Run,
            /// `G` (phase 2)
            Pull,
        ]
    );
}

pub mod volume {
    gpui_kit::actions!(
        volume,
        [
            /// `N` (phase 2)
            Create,
        ]
    );
}

pub mod detail {
    gpui_kit::actions!(
        detail,
        [
            /// `Ctrl+Tab` (KBD-040)
            NextTab,
            PrevTab,
            /// `Alt+↑` (KBD-042)
            ParentList,
        ]
    );
}

pub mod logs {
    gpui_kit::actions!(
        logs,
        [
            FindNext,
            FindPrev,
            Bottom,
            Top,
            ToggleTimestamps,
            ToggleWrap,
            ClearView,
            Save,
            CopyAll,
        ]
    );
}

pub mod term {
    gpui_kit::actions!(term, [NewSession, CloseSession, NextSession, PrevSession]);
}

pub mod dialog {
    gpui_kit::actions!(
        dialog,
        [
            /// `Mod+Enter` in a destructive confirmation (KBD-071).
            ConfirmDestructive,
        ]
    );
}

pub mod form {
    gpui_kit::actions!(form, [AddRow, RemoveRow]);
}

pub mod switcher {
    gpui_kit::actions!(
        switcher,
        [
            /// Arrow keys / Enter inside the engine switcher (KBD-021).
            Up,
            Down,
            Choose,
            Close,
        ]
    );
}

pub mod palette {
    gpui_kit::actions!(
        palette,
        [
            /// Escape the shortcut reference / notification center.
            Dismiss,
        ]
    );
}

// ── M6: Images, Volumes, Networks (IMG-*, VOL-*, NET-*) ─────────────────────────────────────
pub mod res {
    use gpui_kit::{Action, SharedString};

    use crate::nav::Route;

    gpui_kit::actions!(
        res,
        [
            /// Open the *Tag image* dialog (IMG-011).
            TagImage,
            /// Copy the first repo digest (IMG-011).
            CopyDigest,
            /// IMG-003 overflow.
            PruneDangling,
            PruneUnused,
        ]
    );

    /// Replace the current route without a history entry (detail tab switches).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = res, no_json)]
    pub struct ReplaceRoute {
        pub route: Route,
    }

    /// The *Run* cell button of an image row: moves the cursor to `row`, then runs the same
    /// handler as `U` (`image::Run`).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = res, no_json)]
    pub struct RunRow {
        pub row: SharedString,
    }
}

// ── M9: Settings (SET-*, ENG-104/105) ───────────────────────────────────────────────────────
pub mod settings {
    use gpui_kit::{Action, SharedString};

    gpui_kit::actions!(
        settings,
        [
            /// Settings › Engines › *Add engine…* (ENG-105, ENG-111 first-run button).
            AddEngine,
            /// SET-060: diagnostics text to the clipboard.
            CopyDiagnostics,
            /// REL-002: third-party notices.
            ViewLicenses,
        ]
    );

    /// An engine row command in Settings › Engines (KBD-075). Row buttons run the same
    /// handler; the action makes it dispatchable (tests, future bindings).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = settings, no_json)]
    pub struct EngineOp {
        pub id: SharedString,
        pub op: EngineOpKind,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum EngineOpKind {
        Test,
        ToggleEnabled,
        ToggleHidden,
        Remove,
        StartAndConnect,
        /// Pin `id` as the startup default engine (ENG-116).
        SetDefault,
        /// Clear the pin (ENG-116).
        ClearDefault,
    }
}

/// Every action namespace this crate defines (KBD-093 coverage test).
pub const NAMESPACES: &[&str] = &[
    "dk::",
    "sidebar::",
    "list::",
    "container::",
    "image::",
    "volume::",
    "detail::",
    "logs::",
    "term::",
    "dialog::",
    "form::",
    "switcher::",
    "palette::",
    "res::",
    "settings::",
    // wip/detail
    "rows::",
    "stats::",
];

pub use list::{OnRow, RowCommand, SetFilter, SetGroupBy, SortByColumn};

// ── container detail (CDT-*, LOG-*, TRM-*, STA-*; wip/detail) ─────────────────────────────
// Kept in their own modules so other pages' additions above don't collide.

/// Detail tab selection (KBD-040 "Go to tab …").
pub mod detail_tab {
    use gpui_kit::Action;

    use crate::nav::ContainerTab;

    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = detail, no_json)]
    pub struct SelectTab {
        pub tab: ContainerTab,
    }
}

/// Focusable description lists / tables in detail tabs (KBD-044).
pub mod rows {
    use gpui_kit::{Action, SharedString};

    gpui_kit::actions!(
        rows,
        [
            Up,
            Down,
            First,
            Last,
            PageUp,
            PageDown,
            /// `Mod+C`: copy the focused value.
            CopyValue,
            /// `Enter`: follow the row's link (volume, network, port) or reveal a secret.
            Activate,
            /// `Space`: reveal / hide a masked value (CDT-010).
            ToggleReveal,
        ]
    );

    /// Reveal toggle clicked on a row (dispatched on the panel's focus handle).
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = rows, no_json)]
    pub struct RevealRow {
        pub key: SharedString,
    }
}

/// Extra log navigation (KBD-051).
pub mod logs_nav {
    gpui_kit::actions!(logs, [LineUp, LineDown, PageUp, PageDown]);
}

/// Stats tab (KBD-070, STA-010).
pub mod stats {
    gpui_kit::actions!(
        stats,
        [
            /// `1`
            Window1m,
            /// `5`
            Window5m,
            /// `F`
            Window15m,
            PrevWindow,
            NextWindow,
            /// STA-010 (on demand, `size=true` is expensive).
            LoadDiskUsage,
        ]
    );
}

/// Terminal tab extras (TRM-004, TRM-009).
pub mod term_ext {
    use gpui_kit::Action;

    gpui_kit::actions!(
        term,
        [
            /// Restart the sub-tab's session with the picked shell / user.
            Reconnect,
            /// TRM-009 external terminal.
            OpenExternal,
        ]
    );

    /// Sub-tab clicked.
    #[derive(Clone, PartialEq, Debug, Action)]
    #[action(namespace = term, no_json)]
    pub struct SelectSession {
        pub ix: usize,
    }
}
