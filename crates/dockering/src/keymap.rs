//! The default keymap as data (KBD-080/081). One table, filtered per OS at install time.
//!
//! - `Mod` is written `secondary-` and resolves to `cmd` on macOS and `ctrl` elsewhere (KBD-009).
//!   Rows that differ per OS use [`Os`] masks instead.
//! - Single-letter bindings live only in `ListTable` and `DetailHeader` contexts, so they can
//!   never fire inside a text input, the code editor, or the terminal (KBD-008 by construction).
//! - KBD-085 (S-8): GPUI matches *logical* keys. On Windows the platform mapper can map a
//!   binding to its US-QWERTY physical key (`use_key_equivalents`), so digit/punctuation chords
//!   are loaded through it; see `docs/plan/spikes/2026-10-s8-focus-keys.md`.
//! - KBD-081: a future `keymap.toml` only has to produce more [`BindingSpec`]s.

use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate};

use crate::actions::*;

/// Operating systems a binding applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Os(u8);

impl Os {
    pub const WINDOWS: Os = Os(1);
    pub const MACOS: Os = Os(2);
    pub const LINUX: Os = Os(4);
    pub const ALL: Os = Os(7);
    pub const NOT_MAC: Os = Os(5);

    pub const fn contains(self, other: Os) -> bool {
        self.0 & other.0 == other.0
    }

    /// The OS this binary runs on.
    pub const fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MACOS
        } else if cfg!(target_os = "windows") {
            Os::WINDOWS
        } else {
            Os::LINUX
        }
    }

    pub const fn is_mac(self) -> bool {
        self.0 == Os::MACOS.0
    }
}

/// Key contexts (spec keyboard plan §6.1). `Workspace` is the root.
pub mod ctx {
    pub const WORKSPACE: &str = "Workspace";
    pub const SIDEBAR: &str = "Sidebar";
    pub const TOOLBAR: &str = "Toolbar";
    /// Element key context of a ListTable wrapper.
    pub const LIST_TABLE: &str = "ListTable";
    /// Binding predicate for keys while the table body has focus. It must reach the inner
    /// GPUI Kit `DataTable` context so our bindings out-rank the built-in `DataTable` ones
    /// (same depth, installed later; spike S-8).
    pub const LIST_KEYS: &str = "ListTable > DataTable";
    pub const DETAIL_HEADER: &str = "DetailHeader";
    pub const DETAIL_TABS: &str = "DetailTabs";
    pub const LOGS: &str = "Logs";
    pub const TERMINAL: &str = dk_terminal::KEY_CONTEXT;
    pub const DIALOG: &str = "Dialog";
    pub const PALETTE: &str = "Palette";
    /// Engine switcher popover.
    pub const SWITCHER: &str = "EngineSwitcher";
    /// The ListTable quick-find field.
    pub const QUICK_FIND: &str = "QuickFind";
}

/// Shortcut-reference group (KBD-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Global,
    Lists,
    Detail,
    Logs,
    Terminal,
    Dialogs,
}

impl Group {
    pub fn label(self) -> &'static str {
        use crate::strings as s;
        match self {
            Group::Global => s::REF_GLOBAL,
            Group::Lists => s::REF_LISTS,
            Group::Detail => s::REF_DETAIL,
            Group::Logs => s::REF_LOGS,
            Group::Terminal => s::REF_TERMINAL,
            Group::Dialogs => s::REF_DIALOGS,
        }
    }
}

/// One default binding.
#[derive(Clone, Copy)]
pub struct BindingSpec {
    /// GPUI keystroke syntax; `secondary-` = Mod.
    pub keys: &'static str,
    pub action: fn() -> Box<dyn Action>,
    pub context: Option<&'static str>,
    pub os: Os,
    /// Human description for the shortcut reference.
    pub label: &'static str,
    pub group: Group,
}

macro_rules! b {
    ($keys:literal, $action:expr, $ctx:expr, $os:expr, $label:expr, $group:ident) => {
        BindingSpec {
            keys: $keys,
            action: || Box::new($action),
            context: $ctx,
            os: $os,
            label: $label,
            group: Group::$group,
        }
    };
}

use crate::strings as s;
use ctx::*;

const W: Option<&str> = Some(WORKSPACE);
const L: Option<&str> = Some(LIST_KEYS);
const D: Option<&str> = Some(DETAIL_HEADER);
const T: Option<&str> = Some(DETAIL_TABS);
const G: Option<&str> = Some(LOGS);
const TERM: Option<&str> = Some(TERMINAL);
const DLG: Option<&str> = Some(DIALOG);
const SB: Option<&str> = Some(SIDEBAR);
const SW: Option<&str> = Some(SWITCHER);
const QF: Option<&str> = Some(QUICK_FIND);
const PAL: Option<&str> = Some(PALETTE);

/// The default keymap. Mirrors the tables in `docs/spec/features/keyboard.md`.
pub static DEFAULT_KEYMAP: &[BindingSpec] = &[
    // ── global (Workspace) ──────────────────────────────────────────────────────────────
    b!("secondary-shift-p", CommandPalette, W, Os::ALL, s::CMD_PALETTE, Global),
    b!("secondary-p", CommandPalette, W, Os::ALL, s::CMD_PALETTE, Global),
    b!("secondary-/", ShortcutReference, W, Os::ALL, s::CMD_SHORTCUTS, Global),
    b!("f1", ShortcutReference, W, Os::ALL, s::CMD_SHORTCUTS, Global),
    b!("secondary-k", EngineSwitcher, W, Os::ALL, s::CMD_ENGINE_SWITCHER, Global),
    b!("secondary-1", GoContainers, W, Os::ALL, s::CMD_GO_CONTAINERS, Global),
    b!("secondary-2", GoImages, W, Os::ALL, s::CMD_GO_IMAGES, Global),
    b!("secondary-3", GoVolumes, W, Os::ALL, s::CMD_GO_VOLUMES, Global),
    b!("secondary-4", GoNetworks, W, Os::ALL, s::CMD_GO_NETWORKS, Global),
    b!("secondary-,", OpenSettings, W, Os::ALL, s::CMD_SETTINGS, Global),
    b!("secondary-f", FocusSearch, W, Os::ALL, s::CMD_FOCUS_SEARCH, Global),
    b!("secondary-r", Refresh, W, Os::ALL, s::CMD_REFRESH, Global),
    b!("f5", Refresh, W, Os::ALL, s::CMD_REFRESH, Global),
    b!("alt-left", Back, W, Os::NOT_MAC, s::CMD_BACK, Global),
    b!("alt-right", Forward, W, Os::NOT_MAC, s::CMD_FORWARD, Global),
    b!("cmd-[", Back, W, Os::MACOS, s::CMD_BACK, Global),
    b!("cmd-]", Forward, W, Os::MACOS, s::CMD_FORWARD, Global),
    b!("secondary-b", ToggleSidebar, W, Os::ALL, s::CMD_TOGGLE_SIDEBAR, Global),
    b!("f6", NextRegion, W, Os::ALL, s::CMD_NEXT_REGION, Global),
    b!("shift-f6", PrevRegion, W, Os::ALL, s::CMD_PREV_REGION, Global),
    b!("ctrl-alt-tab", NextRegion, W, Os::MACOS, s::CMD_NEXT_REGION, Global),
    b!("ctrl-alt-shift-tab", PrevRegion, W, Os::MACOS, s::CMD_PREV_REGION, Global),
    b!("secondary-shift-l", ToggleTheme, W, Os::ALL, s::CMD_TOGGLE_THEME, Global),
    b!("secondary-shift-n", FocusNotifications, W, Os::ALL, s::CMD_FOCUS_NOTIFICATIONS, Global),
    b!("secondary-=", ZoomIn, W, Os::ALL, s::CMD_ZOOM_IN, Global),
    b!("secondary-+", ZoomIn, W, Os::ALL, s::CMD_ZOOM_IN, Global),
    b!("secondary--", ZoomOut, W, Os::ALL, s::CMD_ZOOM_OUT, Global),
    b!("secondary-0", ZoomReset, W, Os::ALL, s::CMD_ZOOM_RESET, Global),
    // KBD-018: macOS quits/closes with Cmd; Windows/Linux use Alt+F4 (system) and the palette.
    b!("cmd-q", Quit, W, Os::MACOS, s::CMD_QUIT, Global),
    b!("cmd-w", CloseWindow, W, Os::MACOS, s::CMD_QUIT, Global),
    // ── sidebar (roving cursor) ─────────────────────────────────────────────────────────
    b!("up", crate::actions::sidebar::Prev, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    b!("down", crate::actions::sidebar::Next, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    b!("home", crate::actions::sidebar::First, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    b!("end", crate::actions::sidebar::Last, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    b!("enter", crate::actions::sidebar::Activate, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    b!("space", crate::actions::sidebar::Activate, SB, Os::ALL, s::PAGE_CONTAINERS, Global),
    // ── engine switcher ─────────────────────────────────────────────────────────────────
    b!("up", switcher::Up, SW, Os::ALL, s::CMD_ENGINE_SWITCHER, Global),
    b!("down", switcher::Down, SW, Os::ALL, s::CMD_ENGINE_SWITCHER, Global),
    b!("enter", switcher::Choose, SW, Os::ALL, s::CMD_ENGINE_SWITCHER, Global),
    b!("escape", switcher::Close, SW, Os::ALL, s::CMD_ENGINE_SWITCHER, Global),
    // ── palette / reference / notifications ─────────────────────────────────────────────
    b!("escape", palette::Dismiss, PAL, Os::ALL, s::CANCEL, Dialogs),
    // ── lists (ListTable, table focused) ────────────────────────────────────────────────
    b!("up", gpui_kit::base::actions::SelectUp, L, Os::ALL, "Previous row", Lists),
    b!("down", gpui_kit::base::actions::SelectDown, L, Os::ALL, "Next row", Lists),
    b!("home", list::First, L, Os::ALL, "First row", Lists),
    b!("end", list::Last, L, Os::ALL, "Last row", Lists),
    b!("pageup", gpui_kit::base::actions::SelectPageUp, L, Os::ALL, "Page up", Lists),
    b!("pagedown", gpui_kit::base::actions::SelectPageDown, L, Os::ALL, "Page down", Lists),
    b!("enter", list::OpenDetail, L, Os::ALL, s::CMD_OPEN_DETAIL, Lists),
    b!("left", list::CollapseOrParent, L, Os::ALL, "Collapse group / go to group", Lists),
    b!("right", list::Expand, L, Os::ALL, "Expand group", Lists),
    b!("secondary-left", list::CollapseAll, L, Os::ALL, s::CMD_COLLAPSE_ALL, Lists),
    b!("secondary-right", list::ExpandAll, L, Os::ALL, s::CMD_EXPAND_ALL, Lists),
    b!("space", list::ToggleRowSelected, L, Os::ALL, "Toggle selection", Lists),
    b!("shift-up", list::ExtendUp, L, Os::ALL, "Extend selection up", Lists),
    b!("shift-down", list::ExtendDown, L, Os::ALL, "Extend selection down", Lists),
    b!("secondary-a", list::SelectAll, L, Os::ALL, s::CMD_SELECT_ALL, Lists),
    b!("escape", list::Escape, L, Os::ALL, s::CMD_CLEAR_SELECTION, Lists),
    b!("tab", list::FocusNext, L, Os::ALL, "Leave the table", Lists),
    b!("shift-tab", list::FocusPrev, L, Os::ALL, "Leave the table (backwards)", Lists),
    b!("shift-f10", list::ContextMenu, L, Os::ALL, s::CMD_CONTEXT_MENU, Lists),
    b!("menu", list::ContextMenu, L, Os::ALL, s::CMD_CONTEXT_MENU, Lists),
    b!("/", list::QuickFind, L, Os::ALL, s::CMD_QUICK_FIND, Lists),
    b!("c", list::CopyId, L, Os::ALL, s::CMD_COPY_ID, Lists),
    b!("delete", list::Delete, L, Os::ALL, s::CMD_DELETE, Lists),
    b!("backspace", list::Delete, L, Os::NOT_MAC, s::CMD_DELETE, Lists),
    b!("cmd-backspace", list::Delete, L, Os::MACOS, s::CMD_DELETE, Lists),
    b!("s", container::StartStop, L, Os::ALL, s::CMD_START_STOP, Lists),
    b!("r", container::Restart, L, Os::ALL, s::CMD_RESTART, Lists),
    b!("p", container::PauseToggle, L, Os::ALL, s::CMD_PAUSE, Lists),
    b!("l", container::Logs, L, Os::ALL, s::CMD_LOGS, Lists),
    b!("t", container::Terminal, L, Os::ALL, s::CMD_TERMINAL, Lists),
    b!("i", container::Inspect, L, Os::ALL, s::CMD_INSPECT, Lists),
    b!("o", container::OpenPort, L, Os::ALL, s::CMD_OPEN_PORT, Lists),
    b!("u", image::Run, L, Os::ALL, "Run image…", Lists),
    b!("g", image::Pull, L, Os::ALL, "Pull image…", Lists),
    b!("n", volume::Create, L, Os::ALL, "New volume…", Lists),
    // Page-level list commands (work anywhere on a list page).
    b!("secondary-shift-g", list::GroupBy, W, Os::ALL, s::CMD_GROUP_BY, Lists),
    b!("secondary-shift-o", list::SortMenu, W, Os::ALL, s::CMD_SORT_BY, Lists),
    b!("secondary-shift-f", list::FocusFilter, W, Os::ALL, s::CMD_FOCUS_FILTER, Lists),
    // Quick find field (KBD-037).
    b!("enter", list::QuickFindNext, QF, Os::ALL, "Next match", Lists),
    b!("down", list::QuickFindNext, QF, Os::ALL, "Next match", Lists),
    b!("escape", list::QuickFindClose, QF, Os::ALL, "Close find", Lists),
    // ── detail pages ────────────────────────────────────────────────────────────────────
    b!("ctrl-tab", detail::NextTab, W, Os::ALL, s::CMD_NEXT_TAB, Detail),
    b!("ctrl-shift-tab", detail::PrevTab, W, Os::ALL, s::CMD_PREV_TAB, Detail),
    b!("alt-up", detail::ParentList, W, Os::ALL, s::CMD_PARENT_LIST, Detail),
    b!("s", container::StartStop, D, Os::ALL, s::CMD_START_STOP, Detail),
    b!("r", container::Restart, D, Os::ALL, s::CMD_RESTART, Detail),
    b!("p", container::PauseToggle, D, Os::ALL, s::CMD_PAUSE, Detail),
    b!("l", container::Logs, D, Os::ALL, s::CMD_LOGS, Detail),
    b!("t", container::Terminal, D, Os::ALL, s::CMD_TERMINAL, Detail),
    b!("i", container::Inspect, D, Os::ALL, s::CMD_INSPECT, Detail),
    b!("c", list::CopyId, D, Os::ALL, s::CMD_COPY_ID, Detail),
    b!("delete", list::Delete, D, Os::ALL, s::CMD_DELETE, Detail),
    b!("right", detail::NextTab, T, Os::ALL, s::CMD_NEXT_TAB, Detail),
    b!("left", detail::PrevTab, T, Os::ALL, s::CMD_PREV_TAB, Detail),
    // ── logs (KBD-050…053) ──────────────────────────────────────────────────────────────
    b!("enter", logs::FindNext, G, Os::ALL, "Next match", Logs),
    b!("shift-enter", logs::FindPrev, G, Os::ALL, "Previous match", Logs),
    b!("f3", logs::FindNext, G, Os::ALL, "Next match", Logs),
    b!("shift-f3", logs::FindPrev, G, Os::ALL, "Previous match", Logs),
    b!("end", logs::Bottom, G, Os::ALL, "Jump to bottom and follow", Logs),
    b!("secondary-down", logs::Bottom, G, Os::ALL, "Jump to bottom and follow", Logs),
    b!("home", logs::Top, G, Os::ALL, "Jump to top", Logs),
    b!("secondary-up", logs::Top, G, Os::ALL, "Jump to top", Logs),
    b!("alt-t", logs::ToggleTimestamps, G, Os::ALL, "Toggle timestamps", Logs),
    b!("alt-w", logs::ToggleWrap, G, Os::ALL, "Toggle wrap", Logs),
    b!("secondary-shift-k", logs::ClearView, G, Os::ALL, "Clear view", Logs),
    b!("secondary-s", logs::Save, G, Os::ALL, "Save to file", Logs),
    b!("secondary-shift-c", logs::CopyAll, G, Os::ALL, "Copy everything", Logs),
    // ── terminal (KBD-060…063) ──────────────────────────────────────────────────────────
    b!("secondary-shift-f6", dk_terminal::actions::LeaveTerminal, TERM, Os::ALL, "Leave the terminal", Terminal),
    b!("cmd-escape", dk_terminal::actions::LeaveTerminal, TERM, Os::MACOS, "Leave the terminal", Terminal),
    b!("ctrl-shift-c", dk_terminal::actions::Copy, TERM, Os::NOT_MAC, "Copy", Terminal),
    b!("ctrl-shift-v", dk_terminal::actions::Paste, TERM, Os::NOT_MAC, "Paste", Terminal),
    b!("cmd-c", dk_terminal::actions::Copy, TERM, Os::MACOS, "Copy", Terminal),
    b!("cmd-v", dk_terminal::actions::Paste, TERM, Os::MACOS, "Paste", Terminal),
    b!("shift-pageup", dk_terminal::actions::ScrollPageUp, TERM, Os::ALL, "Scroll up", Terminal),
    b!("shift-pagedown", dk_terminal::actions::ScrollPageDown, TERM, Os::ALL, "Scroll down", Terminal),
    b!("secondary-shift-t", term::NewSession, TERM, Os::ALL, "New terminal session", Terminal),
    b!("secondary-shift-w", term::CloseSession, TERM, Os::ALL, "Close terminal session", Terminal),
    b!("ctrl-pagedown", term::NextSession, TERM, Os::ALL, "Next session", Terminal),
    b!("ctrl-pageup", term::PrevSession, TERM, Os::ALL, "Previous session", Terminal),
    // ── dialogs / forms (KBD-071/072) ───────────────────────────────────────────────────
    b!("secondary-enter", dialog::ConfirmDestructive, DLG, Os::ALL, "Confirm destructive action", Dialogs),
    b!("secondary-shift-enter", form::AddRow, DLG, Os::ALL, "Add row", Dialogs),
    b!("secondary-shift-backspace", form::RemoveRow, DLG, Os::ALL, "Remove row", Dialogs),
];

/// Bindings for one OS.
pub fn bindings_for(os: Os) -> impl Iterator<Item = &'static BindingSpec> {
    DEFAULT_KEYMAP.iter().filter(move |b| b.os.contains(os))
}

/// Concrete keystroke text for `os` (`secondary-` expanded), as GPUI would parse it.
pub fn resolve_keys(keys: &str, os: Os) -> String {
    let m = if os.is_mac() { "cmd-" } else { "ctrl-" };
    keys.replace("secondary-", m)
}

/// True for keys whose meaning depends on the physical position (KBD-085).
pub fn is_position_key(key: &str) -> bool {
    key.len() == 1 && !key.chars().all(|c| c.is_ascii_alphabetic())
}

fn last_key(keys: &str) -> &str {
    keys.rsplit_once('-').map_or(keys, |(_, k)| if k.is_empty() { "-" } else { k })
}

/// Installs the default keymap for the running OS (KBD-080). Digit/punctuation chords are
/// loaded through the platform keyboard mapper with key equivalents (US-QWERTY positions),
/// so they work on layouts like Czech whose top row isn't digits (KBD-085).
pub fn install(cx: &mut App) {
    let os = Os::current();
    let mut out = Vec::new();
    for spec in bindings_for(os) {
        let keys = resolve_keys(spec.keys, os);
        let predicate = spec
            .context
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok())
            .map(std::rc::Rc::new);
        let physical = is_position_key(last_key(&keys));
        // Logical binding (US layout, or the layout produces this key directly).
        if let Ok(b) = KeyBinding::load(
            &keys,
            (spec.action)(),
            predicate.clone(),
            false,
            None,
            cx.keyboard_mapper().as_ref(),
        ) {
            out.push(b);
        } else {
            tracing::warn!(keys, "invalid default key binding");
        }
        // Physical-position twin (KBD-085). Identical on US layouts; GPUI deduplicates by
        // precedence, so the duplicate is harmless.
        if physical
            && let Ok(b) = KeyBinding::load(
                &keys,
                (spec.action)(),
                predicate,
                true,
                None,
                cx.keyboard_mapper().as_ref(),
            )
        {
            out.push(b);
        }
    }
    cx.bind_keys(out);
}

/// Bindings in display form for the shortcut reference (KBD-022), current OS only.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceRow {
    pub group: Group,
    pub label: &'static str,
    pub keys: String,
    pub context: Option<&'static str>,
}

/// Rows for the shortcut reference, grouped and deduplicated (one row per label+keys).
pub fn reference_rows(os: Os) -> Vec<ReferenceRow> {
    let mut rows: Vec<ReferenceRow> = Vec::new();
    for spec in bindings_for(os) {
        // Roving-cursor plumbing isn't worth listing per widget.
        if matches!(spec.context, Some(SIDEBAR) | Some(SWITCHER) | Some(PALETTE)) {
            continue;
        }
        let keys = display_keys(&resolve_keys(spec.keys, os), os);
        if rows
            .iter()
            .any(|r| r.label == spec.label && r.keys == keys && r.group == spec.group)
        {
            continue;
        }
        rows.push(ReferenceRow {
            group: spec.group,
            label: spec.label,
            keys,
            context: spec.context,
        });
    }
    rows.sort_by_key(|r| r.group);
    rows
}

/// `ctrl-shift-p` → `Ctrl+Shift+P` (`⌘⇧P` on macOS).
pub fn display_keys(keys: &str, os: Os) -> String {
    let mut parts = Vec::new();
    let mut rest = keys;
    loop {
        let Some((head, tail)) = rest.split_once('-') else {
            break;
        };
        if tail.is_empty() {
            break; // the key itself is '-'
        }
        let label = match (head, os.is_mac()) {
            ("ctrl", true) => "⌃",
            ("ctrl", false) => "Ctrl",
            ("alt", true) => "⌥",
            ("alt", false) => "Alt",
            ("shift", true) => "⇧",
            ("shift", false) => "Shift",
            ("cmd", true) => "⌘",
            ("cmd", false) => "Win",
            _ => break,
        };
        parts.push(label.to_owned());
        rest = tail;
    }
    let key = match rest {
        "escape" => "Esc".to_owned(),
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDn".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "delete" => "Del".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "enter" => "Enter".to_owned(),
        "space" => "Space".to_owned(),
        "tab" => "Tab".to_owned(),
        "menu" => "Menu".to_owned(),
        "home" => "Home".to_owned(),
        "end" => "End".to_owned(),
        k => k.to_uppercase(),
    };
    parts.push(key);
    if os.is_mac() {
        parts.concat()
    } else {
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn normalized(keys: &str, os: Os) -> String {
        // Canonical modifier order so `shift-ctrl-x` == `ctrl-shift-x`.
        let k = resolve_keys(keys, os);
        let key = last_key(&k).to_owned();
        let mut mods: Vec<&str> = k
            .strip_suffix(&key)
            .unwrap_or("")
            .split('-')
            .filter(|m| !m.is_empty())
            .collect();
        mods.sort_unstable();
        format!("{}|{key}", mods.join("+"))
    }

    fn assert_no_conflicts(os: Os) {
        let mut seen: HashMap<(String, Option<&str>), &str> = HashMap::new();
        for b in bindings_for(os) {
            let action = (b.action)();
            let key = (normalized(b.keys, os), b.context);
            if let Some(prev) = seen.insert(key.clone(), action.name())
                && prev != action.name()
            {
                panic!(
                    "KBD-082: {:?} in {:?} is bound to both {prev} and {}",
                    key.0,
                    key.1,
                    action.name()
                );
            }
        }
    }

    #[test]
    fn keymap_no_conflicts_windows() {
        assert_no_conflicts(Os::WINDOWS);
    }

    #[test]
    fn keymap_no_conflicts_macos() {
        assert_no_conflicts(Os::MACOS);
    }

    #[test]
    fn keymap_no_conflicts_linux() {
        assert_no_conflicts(Os::LINUX);
    }

    /// KBD-083: no `Ctrl+Alt+<printable>` on Windows/Linux (AltGr collisions).
    #[test]
    fn keymap_no_altgr_chords() {
        for os in [Os::WINDOWS, Os::LINUX] {
            for b in bindings_for(os) {
                let k = resolve_keys(b.keys, os);
                let key = last_key(&k);
                let printable = key.chars().count() == 1;
                assert!(
                    !(k.contains("ctrl-") && k.contains("alt-") && printable),
                    "KBD-083: {k} is an AltGr chord"
                );
            }
        }
    }

    /// KBD-084: OS-reserved chords are never bound.
    #[test]
    fn keymap_no_reserved_chords() {
        let reserved_win_linux = ["ctrl-alt-delete", "ctrl-shift-escape", "alt-f4", "alt-tab"];
        let reserved_mac = ["cmd-tab", "cmd-space", "cmd-h", "cmd-m", "cmd-alt-escape"];
        for os in [Os::WINDOWS, Os::LINUX] {
            for b in bindings_for(os) {
                let k = resolve_keys(b.keys, os);
                assert!(!reserved_win_linux.contains(&k.as_str()), "KBD-084: {k}");
                assert!(!k.contains("cmd-") && !k.contains("super-") && !k.contains("win-"),
                    "KBD-084: Win/Super chord {k}");
            }
        }
        for b in bindings_for(Os::MACOS) {
            let k = resolve_keys(b.keys, Os::MACOS);
            assert!(!reserved_mac.contains(&k.as_str()), "KBD-084: {k}");
        }
    }

    /// KBD-008 by construction: single printable keys only in list/detail contexts.
    #[test]
    fn keymap_single_letters_only_in_list_contexts() {
        for b in DEFAULT_KEYMAP {
            let single = b.keys.chars().count() == 1;
            if single {
                assert!(
                    matches!(b.context, Some(LIST_KEYS) | Some(DETAIL_HEADER)),
                    "KBD-008: single key {:?} bound in {:?}",
                    b.keys,
                    b.context
                );
            }
        }
    }

    #[test]
    fn keymap_bindings_parse() {
        for os in [Os::WINDOWS, Os::MACOS, Os::LINUX] {
            for b in bindings_for(os) {
                let k = resolve_keys(b.keys, os);
                for stroke in k.split_whitespace() {
                    gpui_kit::Keystroke::parse(stroke)
                        .unwrap_or_else(|e| panic!("bad binding {k}: {e}"));
                }
            }
        }
    }

    #[test]
    fn display_keys_formats_per_os() {
        assert_eq!(display_keys("ctrl-shift-p", Os::WINDOWS), "Ctrl+Shift+P");
        assert_eq!(display_keys("cmd-shift-p", Os::MACOS), "⌘⇧P");
        assert_eq!(display_keys("ctrl--", Os::LINUX), "Ctrl+-");
        assert_eq!(display_keys("f6", Os::LINUX), "F6");
    }

    #[test]
    fn position_keys() {
        assert!(is_position_key("1"));
        assert!(is_position_key("/"));
        assert!(is_position_key(","));
        assert!(!is_position_key("a"));
        assert!(!is_position_key("f1"));
        assert_eq!(last_key("ctrl--"), "-");
        assert_eq!(last_key("ctrl-shift-p"), "p");
    }
}
