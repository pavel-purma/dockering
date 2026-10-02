# Feature: Keyboard navigation & shortcuts

- **Status:** in-progress
- **Requirement prefix:** KBD
- **Plan:** [docs/plan/features/keyboard-navigation.md](../../plan/features/keyboard-navigation.md)

## Goal

**Every screen, control, and action in Dockering MUST be reachable and operable with the keyboard
alone.** The mouse is optional. Shortcuts follow platform conventions (`Ctrl` on Windows/Linux, `Cmd`
on macOS, written `Mod` below). Common row actions also have single-letter keys when a list has focus.

## Principles

| ID | Requirement |
|---|---|
| KBD-001 | **Full operability.** Every interactive element (button, link, tab, menu, input, checkbox, table row, group row, chart window selector, dialog control) MUST be focusable and activatable from the keyboard. Any mouse-only feature is a bug. |
| KBD-002 | **Everything is an action.** Every user command is a GPUI `Action` (registered with `actions!` / `#[derive(Action)]`) dispatched in a `key_context`. Buttons, menus, the command palette, and shortcuts all invoke the same action. No logic lives only in click handlers. |
| KBD-003 | **Visible focus.** The focused element always shows a focus ring drawn with the theme's `ring` colour, at ≥ 3:1 contrast in light and dark themes. Focus is never invisible. |
| KBD-004 | **Logical Tab order.** `Tab` / `Shift+Tab` cycle focus *within the current region* in visual order (left→right, top→bottom), using `tab_index` / `tab_stop`. Rows inside a table are **not** separate Tab stops: the table is one stop with arrow-key navigation inside it (roving focus). |
| KBD-005 | **Regions (landmarks).** The window has focus regions: *Title bar*, *Sidebar*, *Page header/toolbar*, *Content* (table, tab panel), *Detail tab bar*, *Status bar*. `F6` / `Shift+F6` cycle between regions. Each region remembers its last-focused child. |
| KBD-006 | **Escape semantics**, in priority order: close the open popup or menu → close the dialog → clear the search field when it's focused and non-empty → clear the multi-selection → return focus from the content to the page's primary list. `Esc` never navigates back (that's `Alt+←`). |
| KBD-007 | **No focus loss.** After an action, focus lands somewhere predictable. After deleting rows, focus goes to the next row (or the previous one at the end). After a dialog closes, focus returns to its invoker. After a navigation, focus goes to the new page's primary control (the list, or the first tab). A data refresh never moves focus or selection (extends SHL-004). |
| KBD-008 | **Text-input safety.** Single-letter shortcuts (KBD-030) MUST NOT fire while focus is in a text input, the code editor, or the terminal. `Mod`-chords that the focused input handles itself (copy, paste, select all, undo) go to the input first. |
| KBD-009 | **Platform conventions.** `Mod` = `Cmd` on macOS and `Ctrl` elsewhere. Back/forward is `Cmd+[` / `Cmd+]` on macOS and `Alt+←` / `Alt+→` elsewhere. Quit is `Cmd+Q` on macOS and `Ctrl+Q` elsewhere. Default bindings live in one table (below) and are defined in one module (`keymap.rs`). |
| KBD-010 | **Discoverability.** Every shortcut is shown (a) in the tooltips of buttons and icons (`Kbd` via `Kbd::binding_for_action`), (b) in menus and context menus next to items, (c) in the command palette, and (d) in the shortcut reference (KBD-022). Labels are generated from the live keymap, never hard-coded. |

## Global shortcuts (work everywhere except where an input consumes the key)

| ID | Keys | Action |
|---|---|---|
| KBD-017 | `Mod+Shift+L` | Toggle light/dark theme |
| KBD-018 | macOS `Cmd+Q` quit, `Cmd+W` close window, `Cmd+H` / `Cmd+M` system. Windows/Linux: `Alt+F4` closes the window (system), and `Mod+Q` quits from the command palette only (no default chord, so it can't be hit by accident) |
| KBD-020 | `Mod+Shift+P` (also `Mod+P`) | **Command palette** (GPUI Kit `Command`). Fuzzy-search every action available in the current context, plus "Go to container/image/volume/network <name>" jump entries. Shows each entry's shortcut. `Enter` runs it, `Esc` closes. |
| KBD-021 | `Mod+K` | Engine switcher (ENG-101). Arrows select, `Enter` switches, and typing filters. |
| KBD-022 | `Mod+/` and `F1` | **Keyboard shortcut reference**: a dialog listing all bindings grouped by context (Global, Lists, Detail, Logs, Terminal, Dialogs). Searchable. Shows only bindings valid on the current OS. |
| KBD-023 | `Mod+1` … `Mod+4` | Go to Containers / Images / Volumes / Networks (from SHL-010) |
| KBD-024 | `Mod+,` | Settings |
| KBD-025 | `Mod+F` | Focus the page search field (in Logs and Inspect: the in-view search) |
| KBD-026 | `Mod+R`, `F5` | Refresh the current page |
| KBD-027 | `Alt+←` / `Alt+→` (macOS `Cmd+[` / `Cmd+]`) | Back / forward |
| KBD-028 | `Mod+B` | Toggle sidebar collapsed |
| KBD-029 | `F6` / `Shift+F6` (macOS alternative without `fn`: `Ctrl+Option+Tab` / `Ctrl+Option+Shift+Tab`) | Next / previous focus region (KBD-005) |

## List pages (Containers, Images, Volumes, Networks): table focused

| ID | Keys | Action |
|---|---|---|
| KBD-030 | **Single-letter row actions** (no modifier; table focus only; capability- and state-gated; the confirm rules SHL-002 still apply). Containers: `S` start/stop toggle, `R` restart, `P` pause/unpause, `L` logs, `T` terminal, `I` inspect, `O` open the first published port in the browser. Images: `U` run…, `G` pull… Volumes: `N` new volume… All lists: `C` copy id, `Del` / `Backspace` (macOS `Cmd+Backspace`) delete (with confirm). |
| KBD-031 | `↑` / `↓`, `Home` / `End`, `PgUp` / `PgDn` | Move the row cursor (GPUI Kit `DataTable` built-ins). The cursor scrolls into view. |
| KBD-032 | `Enter` | Open detail (CON-033). On a **group row**: open/close the group instead. |
| KBD-033 | `←` / `→` on a group row | Collapse / expand the group (on a member row, `←` moves to the parent group row). `Mod+←` / `Mod+→` collapse / expand all groups. |
| KBD-034 | `Space` | Toggle the selection checkbox of the cursor row. On a group row, it toggles all members. |
| KBD-035 | `Shift+↑/↓` | Extend the selection. `Mod+A` selects all visible rows. `Esc` clears the selection (KBD-006). |
| KBD-036 | `Shift+F10`, the `Menu` key | Open the row context menu (all row actions, with shortcuts shown), navigable with arrows/`Enter`/`Esc`. |
| KBD-037 | `/` then type | **Quick find**: `/` opens an inline find field over the focused table. Typing jumps to the first row whose name contains the text, `Enter` / `↓` jump to the next match, and `Esc` closes it. Plain letters are never treated as type-ahead, because they're row actions (KBD-030). |
| KBD-038 | Multi-selection semantics | When ≥ 2 rows are selected, `S` / `R` / `Del` (and the context menu) act on the **whole selection**, and the confirm dialog lists every item. With no multi-selection they act on the cursor row. `Mod+A` selects all *visible* rows; members of collapsed groups count as selected through their group. |
| KBD-039 | `Mod+Shift+G` | Open the *Group by* selector. Filter chips (All / Running / Stopped …) are a focusable segmented control (arrows, `Enter`), and `Mod+Shift+F` focuses it. No `Alt+digit` bindings: they produce characters on macOS and some layouts. |

Column sorting: `Mod+Shift+O` opens a *Sort by* menu (column list with direction, type-to-filter). The column header
row is also reachable with `Tab` from the toolbar, and `Enter` / `Space` sorts by the focused header.
No digit chords are used for sorting (layout safety, KBD-083).

## Detail pages

| ID | Keys | Action |
|---|---|---|
| KBD-040 | `Ctrl+Tab` / `Ctrl+Shift+Tab` (all OSes), plus `Mod+]` / `Mod+[`-style next/previous on the focused tab bar via arrows. Direct jump: `Mod+Shift+P` → "Go to tab …" | Switch detail tabs (Overview, Logs, Terminal, Stats, Mounts, Network, Inspect). Arrow keys move between tabs when the tab bar is focused. |
| KBD-041 | Header actions | The same single letters as KBD-030 work when focus is on the detail header or a non-input tab body (S, R, P, T, L, Del…). |
| KBD-042 | Breadcrumb | Reachable with `Tab`. `Alt+↑` goes to the parent list (focuses the row of this resource). |
| KBD-043 | Links (image, volume, network, port) | Tab-focusable. `Enter` follows the link, and `Mod+Enter` opens a port link in the browser. |
| KBD-044 | Description lists / tables in tabs | Focusable as one stop. Arrows move between rows, and `Mod+C` copies the focused value. |

## Logs (LOG-*)

| ID | Keys | Action |
|---|---|---|
| KBD-050 | `Mod+F`, then `Enter` / `Shift+Enter` (or `F3` / `Shift+F3`) | Search, next / previous match |
| KBD-051 | `End` / `Mod+↓` | Jump to bottom and re-enable follow (LOG-003). `Home` / `Mod+↑` goes to the top, `PgUp` / `PgDn` scrolls. |
| KBD-052 | `Alt+T` / `Alt+W` | Toggle timestamps / wrap |
| KBD-053 | `Mod+Shift+K` | Clear the view. `Mod+S` saves to a file. `Mod+Shift+C` copies everything. |

## Terminal (TRM-*)

| ID | Requirement |
|---|---|
| KBD-060 | The terminal captures **all** keys while focused (TRM-003), including `Tab`, `Esc`, and `F6`. |
| KBD-061 | **Focus escape hatch**: `Mod+Shift+F6` moves focus out of the terminal to the detail tab bar. (`Ctrl+Shift+Esc` is avoided because Windows reserves it for Task Manager.) macOS alternative without `fn`: `Cmd+Esc`. This binding is reserved and documented in the terminal's empty-state hint and in the shortcut reference. From the terminal, only `Ctrl+Tab` / `Ctrl+Shift+Tab` switch detail tabs. `Alt+digit` is passed to the shell, since readline uses it. |
| KBD-062 | On **macOS**, `Cmd`-chords are never sent to the PTY: all app Cmd shortcuts keep working, because shells don't use Cmd. On Windows/Linux only the listed app chords are intercepted. `Mod+Shift+T` new terminal sub-tab. `Mod+Shift+W` closes the sub-tab. `Ctrl+PgUp` / `Ctrl+PgDn` switch sub-tabs. The terminal key handler passes exactly these app chords (plus KBD-061 and `Ctrl+Tab`) to the app before encoding keys for the PTY. Everything else goes to the shell. |
| KBD-063 | `Shift+PgUp` / `Shift+PgDn` scroll the scrollback. Copy and paste follow TRM-003. |

## Stats, dialogs, menus, forms

| ID | Requirement |
|---|---|
| KBD-070 | Stats: the time-window selector is a focusable segmented control (`←` / `→`, or `1`/`5`/`F` for 1m / 5m / 15m). Each chart card is focusable, and its current values are exposed as text (no information only in the chart). |
| KBD-071 | Dialogs (`Dialog`/`AlertDialog`): initial focus goes to the **safest** control. That's *Cancel* for destructive confirmations and the first field for forms. Focus is trapped inside while open. `Enter` submits (except in multi-line inputs), and `Esc` cancels. Destructive confirms require an explicit `Tab` to the destructive button, or `Mod+Enter`. |
| KBD-072 | Forms (Run image, Create volume, Add engine, Settings): every field is reachable in visual order. Repeating rows (ports, env, mounts) support `Mod+Shift+Enter` (add row) and `Mod+Shift+Backspace` (remove row), which don't collide with text-editing chords or with `Mod+Enter` = submit/confirm (KBD-071). Each row also has Tab-reachable add/remove buttons. File pickers open with `Enter` / `Space`. |
| KBD-073 | Menus, popovers, and selects: arrows navigate, `Enter` / `Space` activate, `Esc` closes and returns focus to the invoker. Type-ahead selects items. |
| KBD-074 | Notifications (toasts): `Mod+Shift+N` focuses the newest notification. Its actions (*Copy details*, *Retry*) are Tab-reachable, and `Esc` dismisses it. Toasts with actions don't auto-dismiss while focused. |
| KBD-075 | Engine switcher and Settings → Engines: all engine actions (Rescan, Test, Start & connect, Remove) are reachable with Tab, arrows, and `Enter`. |

## Customisation (v1 scope)

| ID | Requirement |
|---|---|
| KBD-080 | v1 ships a **fixed default keymap** per OS, defined in one place (`crates/dockering/src/keymap.rs`) as data (`[(keys, action, context)]`). |
| KBD-081 | Reserved for post-v1: user overrides via `keymap.toml` in the config dir (same shape as the default table), with conflict detection in Settings → Keyboard. The v1 code MUST load bindings from the data table, so overrides can be added without touching the views. |
| KBD-082 | No default binding may conflict within the same context. A unit test enforces this per OS. |
| KBD-083 | **Layout safety.** On Windows/Linux, default bindings MUST NOT use `Ctrl+Alt+<printable>`, because it equals AltGr on many European layouts (for example Czech, Polish, and German) and would collide with typing. Bindings use logical keys, not physical scancodes, and are verified with at least a US and a Czech layout (KBD-090). |
| KBD-084 | OS-reserved chords are never bound: Windows `Ctrl+Alt+Del`, `Ctrl+Shift+Esc`, and `Win+*`; macOS `Cmd+Tab`, `Cmd+Space`, `Cmd+H`, and `Cmd+M` (`Cmd+H`/`Cmd+M` keep their system meaning); Linux desktop `Super+*`. |
| KBD-085 | **Digit and punctuation keys** (`Mod+1…4`, `Mod+/`, `Mod+,`, `Mod+[`/`]`, zoom) match the **physical key position** (US-QWERTY equivalent), so they work on layouts where the top row produces non-digits (for example Czech `+ľščť…`). GPUI matches logical keys only (spike S-8, [report](../../plan/spikes/2026-10-s8-focus-keys.md)), so each of these chords gets a second binding loaded with `use_key_equivalents` through the platform keyboard mapper. Support per OS: **Windows** full (US vkey table → the current layout). **macOS** partial (the system key-equivalent table). **Linux** not supported: GPUI has no keyboard mapper there, so only logical matching applies. *Known gap*; the command palette (`Mod+Shift+P`) always works. |

## Verification

| ID | Requirement |
|---|---|
| KBD-090 | **Keyboard-only E2E walkthrough** in the release checklist, on all three OSes, with the mouse disconnected. Switch engine → containers list → expand a group → open detail → each tab → open a terminal and escape it → logs search → start/stop → delete with confirm → images pull → volumes create → settings → shortcut reference. |
| KBD-091 | View tests (`TestAppContext`) simulate keystrokes for every KBD requirement that has behaviour: focus moves, actions dispatch, Escape semantics, focus restoration. |
| KBD-092 | A **reachability test** walks the Tab order of each page in a test window and asserts that every rendered interactive element is reached (no traps, no orphans). |
| KBD-093 | **Action coverage test**: every registered `Action` is either bound to a key or listed in the command palette (and is therefore keyboard-reachable). |
