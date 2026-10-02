# Plan: Keyboard navigation & shortcuts

- **Slug:** `keyboard-navigation`
- **Status:** draft
- **Spec:** [docs/spec/features/keyboard.md](../../spec/features/keyboard.md)
- **Milestone:** cross-cutting. The foundation is in **M2**; each later milestone (M3–M9) ships its screens keyboard-complete.
- **Requirement IDs:** KBD-001…010, 017…029, 030…039, 040…044, 050…053, 060…063, 070…075, 080…084, 090…093 (new) · SHL-010 (changed) · CON-034, CDT-082, LOG-009, TRM-012, STA-011, SET-070, NFR-042 (new)
- **Created:** 2026-10-01

## 1. Goal
Dockering must be fully usable without a mouse. Every screen, control, and action is reachable and
operable from the keyboard, focus is always visible and predictable, and shortcuts are discoverable
through tooltips, menus, a command palette, and a shortcut reference.

## 2. Scope
**In:** a single action and keymap layer, focus regions and Tab order, roving focus in tables
(group rows included), single-letter row actions, a command palette, a shortcut reference, focus
management (dialogs, deletes, navigation), terminal capture with an escape hatch, keyboard behaviour
for logs/stats/forms/toasts, and tests (keystroke, reachability, action coverage, conflicts).

**Out (v1):** user-rebindable keymaps (KBD-081, reserved), screen-reader / OS accessibility-tree
support (GPUI has no AccessKit integration yet, so this is tracked as a follow-up risk), and vim-style
navigation.

## 3. Assumptions & open questions
| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | Shortcut style | **Modifier chords + single-letter row actions** (user decision, 2026-10-01) |
| 2 | Rebinding | **Fixed defaults in v1. `keymap.toml` overrides post-v1**, and the v1 code is data-driven (user decision, 2026-10-01) |
| 3 | The GPUI Kit `DataTable` built-in arrow/Home/End/Page bindings are kept. `tab`/`shift-tab` inside the table move *columns* by default, which conflicts with KBD-004 (a table is one Tab stop). | Override the `Table` context: `tab` leaves the table, and `left`/`right` handle columns and group collapse (spike S-8) |
| 4 | Screen-reader support | Not in v1. Visible text alternatives for charts are covered (KBD-070) |

## 4. Requirements (as written into the spec)
See [keyboard.md](../../spec/features/keyboard.md) for the full text. The cross-references added to feature
specs are CON-034, CDT-082, LOG-009, TRM-012, STA-011, SET-070, NFR-042, and the SHL-010 rewrite.

## 5. Engine contract impact
None. This is UI-only. Actions call the same `HubHandle` methods as their buttons do.

## 6. Design

### 6.1 Action & keymap layer (`crates/dockering/src/keymap.rs`, `actions.rs`)
```rust
// actions.rs — one place for every user command (KBD-002)
actions!(dk, [CommandPalette, ShortcutReference, EngineSwitcher, GoContainers, GoImages, GoVolumes,
    GoNetworks, OpenSettings, FocusSearch, Refresh, Back, Forward, ToggleSidebar, NextRegion,
    PrevRegion, ToggleTheme, FocusNotifications]);
actions!(list, [OpenDetail, ToggleGroup, CollapseAll, ExpandAll, ToggleRowSelected, SelectAll,
    ClearSelection, ContextMenu, CopyId, Delete, BulkStart, BulkStop, BulkDelete, GroupBy, SortByColumn /*(usize)*/]);
actions!(container, [StartStop, Restart, PauseToggle, Logs, Terminal, Inspect, OpenPort]);
actions!(detail, [NextTab, PrevTab, GoTab /*(usize)*/, ParentList]);
actions!(logs, [FindNext, FindPrev, Bottom, Top, ToggleTimestamps, ToggleWrap, ClearView, Save, CopyAll]);
actions!(term, [LeaveTerminal, NewSession, CloseSession, NextSession, PrevSession]);

// keymap.rs — data, per OS (KBD-080/081)
pub struct BindingSpec { keys: &'static str, action: fn() -> Box<dyn Action>, context: Option<&'static str>, os: OsMask }
pub static DEFAULT_KEYMAP: &[BindingSpec] = &[ /* … mirrors the tables in keyboard.md … */ ];
pub fn install(cx: &mut App) { cx.bind_keys(DEFAULT_KEYMAP.iter().filter(current_os).map(to_binding)); }
```
- Key contexts: `Workspace` (root), `Sidebar`, `Toolbar`, `ListTable`, `DetailHeader`, `DetailTabs`, `Logs`, `Terminal`, `Dialog`, `Palette`. Single-letter bindings are registered **only** in `ListTable` and `DetailHeader`, which is how KBD-008 is satisfied by construction.
- Buttons, menus, and the palette dispatch actions (`window.dispatch_action`). Click handlers contain no logic.
- `Kbd::binding_for_action` renders shortcut hints in tooltips and menus (KBD-010).
- **Physical keys (KBD-085, S-8):** `install` loads digit/punctuation chords twice: logically, and again with `use_key_equivalents = true` through `cx.keyboard_mapper()`. Windows: full. macOS: partial. Linux: logical only (no GPUI mapper; known gap). See the [S-8 report](../spikes/2026-10-s8-focus-keys.md).

### 6.2 Focus model
- `FocusRegions` on `AppShell`: an ordered list of `(RegionId, FocusHandle, last_focused: Option<FocusHandle>)`. `NextRegion` / `PrevRegion` cycle through them and restore `last_focused` (KBD-005).
- Each page exposes `primary_focus()`. The `Navigator` focuses it after a route change (KBD-007).
- Dialog helper `confirm_destructive(...)`: initial focus on *Cancel*, focus trapped inside, invoker focus restored on close (KBD-071).
- Delete flow: before the delete, record the neighbour row's id. After the refetch, move the cursor to that id (KBD-007).
- Refresh preserves the cursor and selection by **id**, not by index (KBD-007, SHL-004).
- Focus ring: a shared `.focus_ring(cx)` style helper using the theme `ring` token, applied to custom elements. GPUI Kit components use their built-in focus style, checked in both themes (KBD-003).

### 6.3 Tables (roving focus)
- `ListTable` wraps the GPUI Kit `DataTable` (`TableState` with `row_selectable(true)`) and overrides its bindings: `tab`/`shift-tab` leave the table, and `left`/`right` collapse/expand group rows (assumption 3, spike S-8).
- **Override technique (S-8.4):** bind the same keys with the context predicate **`ListTable > DataTable`**. GPUI ranks bindings by context depth first, then by insertion order (later wins). The predicate matches at the `DataTable` depth like the built-in bindings, and ours are installed after `gpui_kit::init`, so ours win. A plain `ListTable` predicate matches one level shallower and loses. Pinned by `kbd_033_left_right_collapse_group`.
- The cursor row (TableSelection::Row) is separate from the **checkbox multi-selection** (`HashSet<Id>` in the page state). `Space` toggles membership (KBD-034).
- Type-ahead buffer with a 1 s reset (KBD-037). It's ignored for bound letters.

### 6.4 Command palette & shortcut reference
- The palette uses the GPUI Kit `Command` in a `Dialog`. Items come from a `CommandRegistry`: `(label, action, context predicate, group)`, plus dynamic "Go to …" items built from the active `EngineStore` lists. The filtering runs on `background_spawn` above 2,000 items.
- The shortcut reference is a `Dialog` with a searchable table generated from `DEFAULT_KEYMAP` for the current OS (KBD-022). Settings → Keyboard reuses the same view (SET-070).

### 6.5 Terminal
- `TerminalView` key handling: a keystroke interceptor, active only while the terminal is focused, encodes PTY-bound keys and stops propagation before keymap dispatch. The reserved app chords (KBD-061/062, `Ctrl+Tab`) pass through to the keymap. All other keys, including `Tab`, `Esc`, `F6`, and `Alt+digit`, go to the shell.

### 6.6 Errors
None new. Keyboard actions reuse the same error paths as the equivalent buttons (SHL-003).

## 7. Tasks
| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Spike S-8: GPUI focus APIs (`tab_index`/`tab_stop`, `focus_next`), overriding the `DataTable` `Table`-context bindings, and `Kbd::binding_for_action` | gpui-ui | KBD-004, 010 | Spike note + prototype |
| 2 | `actions.rs` + `keymap.rs` (data table, OS masks, `install`), key contexts on the shell | gpui-ui | KBD-002, 009, 080, 081 | Unit: binding table per OS |
| 3 | Conflict + layout-safety + reserved-chord tests over `DEFAULT_KEYMAP` | qa-engineer | KBD-082…084 | `cargo test keymap_` |
| 4 | `FocusRegions`, `F6` cycling, the focus ring helper, `primary_focus` + navigator focus | gpui-ui | KBD-003, 005, 007 | View tests |
| 5 | Global actions: pages, settings, search, refresh, back/forward, sidebar, theme, quit | gpui-ui | KBD-017…029 | Keystroke view tests |
| 6 | `ListTable` roving focus: the cursor vs the multi-selection, group expand/collapse, select-all, type-ahead, context menu via `Shift+F10` | gpui-ui | KBD-031…037 | View tests with `FakeEngine` |
| 7 | Single-letter row actions + bulk chords, capability- and state-gated, with confirmations | gpui-ui | KBD-030, 038, 039, CON-034 | View tests: action → hub call |
| 8 | Escape semantics + focus restoration (dialogs, deletes, refresh) | gpui-ui | KBD-006, 007, 071 | View tests |
| 9 | Command palette (`CommandRegistry`, "Go to" items) + shortcut reference dialog + Settings → Keyboard | gpui-ui | KBD-020, 022, 010, SET-070 | View tests + screenshot |
| 10 | Detail pages: tab switching, header letters, breadcrumb `Alt+↑`, focusable description lists | gpui-ui | KBD-040…044, CDT-082 | View tests (M3) |
| 11 | Logs keyboard | gpui-ui | KBD-050…053, LOG-009 | View tests (M3) |
| 12 | Terminal app-chord passthrough + escape hatch + sub-tabs | gpui-ui | KBD-060…063, TRM-012 | `dk-terminal` key tests (M4) |
| 13 | Stats selector + chart text values | gpui-ui | KBD-070, STA-011 | View tests (M5) |
| 14 | Dialogs/forms/menus/toasts/engine screens keyboard pass | gpui-ui | KBD-071…075 | View tests (M6–M9) |
| 15 | Reachability test harness (Tab-walk every page) + action-coverage test | qa-engineer | KBD-092, 093 | `cargo test a11y_` |
| 16 | Keyboard-only E2E walkthrough added to the release checklist (US + Czech layouts, 3 OSes) | qa-engineer | KBD-090, NFR-042 | Checklist run in M9 |
| 17 | Review | reviewer | all | verdict: approve |

Tasks 1–9 and 15 are part of **M2** (the foundation, plus the containers list). Tasks 10–14 land with
the milestone that builds each screen, and the keyboard pass is part of every UI PR's Definition of
Done (spec 60, DoD item 7).

## 8. Test plan
| ID | Layer | Test name(s) |
|---|---|---|
| KBD-082…084 | unit | `keymap_no_conflicts_{windows,macos,linux}`, `keymap_no_altgr_chords`, `keymap_no_reserved_chords` |
| KBD-005, 007 | view | `kbd_005_f6_cycles_regions`, `kbd_007_focus_after_delete_moves_to_next_row`, `kbd_007_dialog_restores_invoker_focus` |
| KBD-006 | view | `kbd_006_escape_priority_order` |
| KBD-030, 008 | view | `kbd_030_s_starts_stopped_container`, `kbd_008_letters_ignored_in_search_input` |
| KBD-031…037 | view | `kbd_033_left_right_collapse_group`, `kbd_034_space_toggles_selection`, `kbd_037_type_ahead_jumps` |
| KBD-020, 022 | view | `kbd_020_palette_runs_action`, `kbd_022_reference_lists_os_bindings` |
| KBD-061 | dk-terminal | `kbd_061_escape_hatch_not_sent_to_pty`, `kbd_060_tab_and_esc_sent_to_pty` |
| KBD-092 | view | `a11y_tab_walk_reaches_all_interactive_{page}` |
| KBD-093 | unit | `a11y_every_action_bound_or_in_palette` |
| KBD-090 | manual | release checklist "Keyboard-only walkthrough" |

## 9. Risks & spikes
| Risk | Mitigation / spike |
|---|---|
| GPUI Kit `DataTable` binds `tab` to column navigation (it's in the `Table` context) | Override in our `ListTable` context (spike S-8). Contribute upstream if needed. |
| GPUI has no accessibility tree (no screen-reader support) | Out of scope for v1. Track GPUI/AccessKit progress. Keep text alternatives (KBD-070). |
| Single-letter actions trigger accidentally | Context-limited to the list and header. Destructive actions still confirm (SHL-002). Not active in inputs (KBD-008). |
| International layouts (AltGr) | KBD-083 rule + test. Manual check with a Czech layout. |
| Terminal swallowing app shortcuts | Explicit passthrough list + escape hatch (KBD-061/062) |

## 10. Revision log
- 2026-10-01: created (shortcut style and rebinding scope decided by the user)
- 2026-10-02: spike S-8 done; added the `ListTable > DataTable` override technique, the KBD-085 twin-binding approach, and the terminal keystroke interceptor.
