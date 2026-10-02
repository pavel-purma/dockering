# Feature: Keyboard navigation & shortcuts

- **Status:** implemented (2026-10-02); verification items KBD-090 (release) and KBD-092 (partial) are open, see *Known gaps (v1)*
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
| KBD-003 | **No focus rings.** Focused controls draw no ring or accent border (theme `focus_ring = false`, `ring` = the input border colour). Keyboard position stays visible through the controls' own state: the table cursor row, the sidebar cursor tint, the text caret, and open menus. (Revised 2026-10-02: shortcuts are the goal, not a focus-ring-driven keyboard-only UI.) |
| KBD-004 | **Logical Tab order.** `Tab` / `Shift+Tab` cycle focus *within the current region* in visual order (left→right, top→bottom), using `tab_index` / `tab_stop`. Rows inside a table are **not** separate Tab stops: the table is one stop with arrow-key navigation inside it (roving focus). |
| KBD-005 | **Regions (landmarks).** The window has four focus regions: *Title bar*, *Sidebar*, *Content*, and *Status bar*. The page header/toolbar and the detail tab bar belong to *Content* and are reached with `Tab` inside it. On Settings routes the *Sidebar* region holds the settings section nav (SET-080). `F6` / `Shift+F6` cycle between regions. Each region remembers its last-focused child. |
| KBD-006 | **Escape semantics**, in priority order: close the open popup or menu → close the dialog → clear the search field when it's focused and non-empty → clear the multi-selection → return focus from the content to the page's primary list. `Esc` never navigates back (that's `Alt+←`). |
| KBD-007 | **No focus loss.** After an action, focus lands somewhere predictable. After deleting rows, focus goes to the next row (or the previous one at the end). After a dialog closes, focus returns to its invoker. After a navigation, focus goes to the new page's primary control (the list, or the first tab). A data refresh never moves focus or selection (extends SHL-004). |
| KBD-008 | **Text-input safety.** Single-letter shortcuts (KBD-030) MUST NOT fire while focus is in a text input, the code editor, or the terminal. `Mod`-chords that the focused input handles itself (copy, paste, select all, undo) go to the input first. |
| KBD-009 | **Platform conventions.** `Mod` = `Cmd` on macOS and `Ctrl` elsewhere. Back/forward is `Cmd+[` / `Cmd+]` on macOS and `Alt+←` / `Alt+→` elsewhere. Quit is `Cmd+Q` on macOS. On Windows/Linux, Quit is a command-palette entry with no default chord (KBD-018). Default bindings live in one table (below) and are defined in one module (`keymap.rs`). |
| KBD-010 | **Discoverability.** Every shortcut is shown (a) in the tooltips of buttons and icons (`Kbd` via `Kbd::binding_for_action`), (b) in menus and context menus next to items, (c) in the command palette, and (d) in the shortcut reference (KBD-022). Labels are generated from the live keymap, never hard-coded. |

## Global shortcuts (work everywhere except where an input consumes the key)

| ID | Keys | Action |
|---|---|---|
| KBD-017 | `Mod+Shift+L` | Toggle light/dark theme |
| KBD-018 | macOS `Cmd+Q`, `Cmd+W`; `Cmd+H` / `Cmd+M` stay system | macOS: quit and close the window. Windows/Linux: `Alt+F4` closes the window (system), and *Quit Dockering* is a command-palette entry with no default chord, so it can't be hit by accident |
| KBD-020 | `Mod+Shift+P` (also `Mod+P`) | **Command palette** (GPUI Kit `Command`). Fuzzy-search every action available in the current context, plus "Go to container/image/volume/network <name>" jump entries. Shows each entry's shortcut. `Enter` runs it, `Esc` closes. |
| KBD-021 | `Mod+K` | Engine switcher (ENG-101). Arrows select, `Enter` switches, and typing filters. |
| KBD-022 | `Mod+/` and `F1` | **Keyboard shortcut reference**: a dialog listing all bindings grouped by context (Global, Lists, Detail, Logs, Terminal, Dialogs). Searchable. Shows only bindings valid on the current OS. |
| KBD-023 | `Mod+1` … `Mod+4` | Go to Containers / Images / Volumes / Networks (from SHL-010) |
| KBD-024 | `Mod+,` | Settings (focus lands on the sidebar's section nav, SET-080) |
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

Column sorting: `Mod+Shift+O` opens a *Sort by* menu listing the sortable columns, with the current direction
shown. Choosing a column cycles ascending → descending → unsorted. The same command is in the palette. Column headers
aren't Tab stops in v1: clicking a header sorts with the mouse, and the menu is the keyboard path.
No digit chords are used for sorting (layout safety, KBD-083).

## Detail pages

| ID | Keys | Action |
|---|---|---|
| KBD-040 | `Ctrl+Tab` / `Ctrl+Shift+Tab` (all OSes). `←` / `→` when the tab bar is focused. Direct jump: `Mod+Shift+P` → "Go to tab …" | Switch detail tabs (Overview, Logs, Terminal, Stats, Mounts, Network, Inspect). Disabled tabs are skipped. |
| KBD-041 | Header actions | The same single letters as KBD-030 work when focus is on the detail header or a non-input tab body (S, R, P, T, L, I, C, Del; `U` run on image detail). |
| KBD-042 | Breadcrumb | Reachable with `Tab`. `Alt+↑` goes to the parent list (focuses the row of this resource). |
| KBD-043 | Links (image, volume, network, port) | Reachable from the keyboard: header links are Tab stops, and links inside rows panels are followed from the row cursor (KBD-044). `Enter` follows the link. A port link opens the browser. |
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

## Implementation status (2026-10-02)

| ID | Tests / evidence |
|---|---|
| KBD-001, 002 | `a11y_every_action_bound_or_in_palette` (all actions reachable); buttons and menus dispatch actions (`OnRow`, `EngineOp`, …) |
| KBD-003 | `theme.rs` sets `focus_ring = false` and `ring = input`; no app element draws a `ring` border |
| KBD-004 | `ListTable > DataTable` `tab`/`shift-tab` overrides; `a11y_tab_walk_reaches_all_interactive_settings` |
| KBD-005 | `kbd_005_f6_cycles_regions` |
| KBD-006 | `kbd_006_escape_priority_order` |
| KBD-007 | `kbd_007_focus_after_delete_moves_to_next_row`, `kbd_007_cursor_moves_to_next_row_after_delete`, `shl_004_refresh_keeps_cursor_and_selection_by_key`, `kbd_071_escape_cancels_add_engine_and_restores_focus` |
| KBD-008 | `kbd_008_letters_ignored_in_search_input`, `kbd_008_g_u_n_ignored_in_search_inputs`, `keymap_single_letters_only_in_list_contexts` |
| KBD-009, 080, 081 | `keymap.rs` data table with OS masks; `keymap_bindings_parse` |
| KBD-010 | `tooltip_with_action` / `Kbd` in tooltips, `keymap::hint_for` in row menus, palette and reference generated from the keymap |
| KBD-017, 028 | `kbd_017_028_theme_toggle_and_sidebar` |
| KBD-018 | `cmd-q`/`cmd-w` macOS-only bindings; Quit in the palette (`a11y_every_action_bound_or_in_palette`) |
| KBD-020 | `kbd_020_palette_runs_action`, `kbd_020_palette_closes_after_non_navigation_command`, `kbd_020_palette_go_to_volume_opens_detail` |
| KBD-021 | `kbd_021_switcher_filter_arrows_enter` |
| KBD-022 | `kbd_022_reference_lists_os_bindings`, `kbd_022_reference_lists_m6_letters`, `set_070_keyboard_section_lists_bindings` |
| KBD-023, 027 | `kbd_023_mod_digits_navigate_and_back_forward`, `cdt_002_tab_in_route_and_back_forward_keep_tab` |
| KBD-024 | `kbd_024_mod_comma_opens_settings_with_nav_focus` |
| KBD-025 | `kbd_008_letters_ignored_in_search_input` (`Mod+F`), `log_004_search_next_prev`, `cdt_040_inspect_shows_pretty_json_unmasked` |
| KBD-026 | `kbd_026_f5_refetches_everything` |
| KBD-029 | `kbd_005_f6_cycles_regions` (`F6`); the macOS `Ctrl+Option+Tab` alternative is bound but not tested |
| KBD-030 | `kbd_030_s_starts_stopped_container`, `cdt_041_header_letters_start_and_stop`, `img_004_pull_progress_notification` (`G`), `img_005_run_dialog_runs_and_navigates` (`U`), `vol_004_create_volume_with_n` (`N`) |
| KBD-031 | `ListTable > DataTable` bindings; exercised by most list view tests |
| KBD-032 | `con_010_compose_group_expanded_then_collapses`, `con_033_enter_opens_detail_with_primary_focus` |
| KBD-033 | `kbd_033_left_right_collapse_group`, `kbd_033_left_on_member_goes_to_group` |
| KBD-034 | `kbd_034_space_toggles_selection`, `kbd_034_space_on_group_toggles_members` |
| KBD-035 | `extend_selection_moves_cursor` (model), `kbd_038_select_all_includes_collapsed_members_and_targets` |
| KBD-036 | `kbd_036_shift_f10_opens_row_menu` |
| KBD-037 | `kbd_037_quick_find_jumps`, `kbd_037_find_wraps` |
| KBD-038 | `kbd_038_select_all_includes_collapsed_members_and_targets`, `con_013_delete_all_lists_members_and_needs_force` |
| KBD-039 | `kbd_039_group_by_none_flattens` |
| KBD-040 | `cdt_002_tab_in_route_and_back_forward_keep_tab`, `kbd_040_step_tab_skips_disabled` |
| KBD-041 | `cdt_041_header_letters_start_and_stop` |
| KBD-042 | `kbd_042_alt_up_focuses_row_in_parent_list` |
| KBD-043, 044 | `kbd_044_rows_arrows_and_enter_follows_image_link`, `cdt_020_mounts_volume_link_opens_volume_detail`, `cdt_030_network_link_and_port_link`, `img_010_used_by_enter_follows_link` |
| KBD-050…053 | `log_003_follow_pill`, `log_004_search_next_prev`, `log_004_toggles_clear_and_copy_all`, `log_004_save_writes_through_hub` |
| KBD-060 | `kbd_060_tab_and_esc_sent_to_pty`, `kbd_060_terminal_keys_win_over_app_bindings` |
| KBD-061 | `kbd_061_escape_hatch_not_sent_to_pty`, `kbd_061_reserved_chords_are_not_sent`, `kbd_061_leave_terminal_focuses_tab_bar` |
| KBD-062 | `kbd_062_reserved_chords_pass_to_app`, `trm_007_sub_tabs_new_switch_close` |
| KBD-063 | `copy_selection_and_scrollback` (dk-terminal: `Shift+PgUp`/`Shift+PgDn` scroll and aren't sent to the PTY) |
| KBD-070 | `kbd_070_window_selector_keys` |
| KBD-071 | `kbd_071_destructive_confirm_focuses_cancel`, `kbd_071_escape_cancels_add_engine_and_restores_focus`, `kbd_071_add_engine_from_palette_escape_closes` |
| KBD-072 | `img_005_run_dialog_runs_and_navigates`, `vol_004_create_volume_with_n` (`Mod+Shift+Enter` / `Mod+Shift+Backspace`) |
| KBD-073 | `kbd_036_shift_f10_opens_row_menu`, `kbd_039_group_by_none_flattens` (arrows + `Enter` in `KeyMenu`) |
| KBD-074 | `FocusNotifications` (`Mod+Shift+N`) bound and in the palette; **no test** |
| KBD-075 | `kbd_021_switcher_filter_arrows_enter`, `eng_104_*` Settings view tests, `a11y_tab_walk_reaches_all_interactive_settings` |
| KBD-082…084 | `keymap_no_conflicts_{windows,macos,linux}`, `keymap_no_altgr_chords`, `keymap_no_reserved_chords`, `kbd_083_altgr_and_option_text_is_not_encoded` |
| KBD-085 | `keymap::install` twin bindings (`use_key_equivalents`); see Known gaps |
| KBD-090 | Release checklist ([release-checklist.md](../../plan/release-checklist.md) §2); not yet run on macOS/Linux or with a Czech layout |
| KBD-091 | The view tests above |
| KBD-092 | `a11y_tab_walk_reaches_all_interactive_settings` (Settings only) |
| KBD-093 | `a11y_every_action_bound_or_in_palette` |

## Known gaps (v1)

- **KBD-085 (Linux):** GPUI has no keyboard mapper on Linux, so digit and punctuation chords match logical keys only. On layouts whose top row isn't digits (for example Czech), `Mod+1…4`, `Mod+/`, `Mod+,`, and zoom don't fire. The command palette (`Mod+Shift+P`) always works. macOS support is partial (the system key-equivalent table).
- **KBD-074:** `Mod+Shift+N` focuses the newest toast by walking the Tab order, because GPUI Kit keeps the toast stack's focus handle private. There's no automated test. It's in the KBD-090 walkthrough.
- **KBD-092:** the Tab-walk reachability test covers the Settings page only. Lists, detail pages, and dialogs are covered by the KBD-091 keystroke tests and the KBD-090 walkthrough, not by a Tab-walk harness. *Follow-up:* extend `tab_walk` to every page.
- **KBD-090:** the keyboard-only walkthrough is a release-checklist item. Still due on macOS, on Linux, and with a Czech layout. No recorded checklist run exists in the repo yet.
- **KBD-073:** type-ahead inside menus and selects is whatever GPUI Kit `PopupMenu`/`Select` provide. It isn't verified.
- **Column headers** (sorting) aren't Tab stops. Sorting from the keyboard uses `Mod+Shift+O` (see *List pages*).
