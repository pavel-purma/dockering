# Plan: Keyboard flow polish (palette height, search hand-off, list state on return, keyboard viewing of detail tabs)

- **Slug:** `keyboard-flow-polish`
- **Status:** in-progress
- **Spec:** [Keyboard](../../spec/features/keyboard.md) (KBD-006/020/030/042/051/061 changed, KBD-094…098 new), [Container detail](../../spec/features/container-detail.md) (CDT-082 changed, CDT-083 new), [UI shell §2–3](../../spec/30-ui-shell.md) (SHL-014 new), [Architecture §4.1](../../spec/10-architecture.md), [Containers](../../spec/features/containers.md) (CON-034 reference)
- **Milestone:** post-v1 UI polish (this checkout has no milestone table)
- **Requirement IDs:** KBD-094, KBD-095, KBD-096, KBD-097, KBD-098, SHL-014, CDT-083 (new) · KBD-006, KBD-020, KBD-030, KBD-042, KBD-051, KBD-061, CDT-082, CON-034 (changed) · KBD-003, KBD-008, KBD-082, KBD-093, NFR-001 (preserved)
- **Created:** 2026-10-10

## 1. Goal

Make the keyboard flow "find a row, open it, read it, go back" work without the mouse, and fix the palette's layout:

1. The command palette (`Mod+P`) panel is as tall as its list. Today an empty band shows under the last row.
2. `Enter` in a list page's search field moves focus into the table, onto the first match. A second `Enter` opens its detail.
3. Returning from a detail (Back, `Alt+←`, mouse Back, `Backspace`, breadcrumb, `Alt+↑`) shows the list as it was left: search, filter, sort, selection, cursor row and scroll position.
4. On a detail page, from the tab bar: `↓` enters the tab's content (terminal, log lines, first property row, …), `PgUp` / `PgDn` scroll it, and `Esc` brings focus back. `Backspace` goes back.

## 2. Scope

**In:** the four list pages (Containers, Images, Volumes, Networks) and the four detail pages; the palette and shortcut-reference panel; `keymap.rs`, `commands.rs`, `actions.rs`, `strings.rs`; one public scroll method on the `dk-terminal` view; spec text; tests.

**Out:** new visual cues (no focus rings, KBD-003); turning the static Overview panels of image, volume and network details into focusable rows (follow-up); Settings pages and the Settings search; the Logs search (keeps `Enter` = next match, KBD-050); the Tab-walk reachability test for detail pages (the KBD-092 known gap); keeping list state across an engine switch or a disconnect; retaining detail pages.

## 3. Assumptions and decisions to confirm

Defaults apply unless you veto them. **D1–D4 are the ones to look at.**

| # | Decision / assumption | Default if unanswered |
|---|---|---|
| D1 | **Scope of state retention.** Every list page keeps its state while the engine is unchanged, and restores it on any return route (also the sidebar, `Mod+1…4`, the palette), not only on a detail round trip. One simple rule, no special cases. | Yes. Veto → keep the list only while one of its own details is open |
| D2 | **`Backspace` = Back (history)**, the same as `Alt+←`, not "up to the parent list" (`Alt+↑`). From a detail opened from the list both land on the list. Detail pages only; never in a text field, the Inspect editor or the terminal. | Back |
| D3 | **Remove `Backspace` as a Delete alias on lists** (Windows/Linux; `Del` stays, macOS keeps `Cmd+Backspace`). Without this, a double or held `Backspace` goes back and then deletes (or asks to delete) the row the list lands on. Changes KBD-030. | Remove it. Veto → keep it and ignore `Backspace` for 400 ms after a Back |
| D4 | **Search hand-off target.** `Enter` lands on the first *item* row of the current results (skips Compose group header rows). With an empty query the cursor stays where it was. With no results, focus stays in the field. Only plain `Enter` (not `Shift` or `Mod`). | As described |
| A1 | The palette cause below is diagnosed from the code, not yet reproduced. Task 1 starts with a failing test and a before screenshot. | — |
| A2 | Ways back to the tab bar from content: `Esc` (rows, log lines, stats, Inspect with its search closed), `↑` on the first row of a rows panel, and `Mod+Shift+F6` from anywhere in a tab (already the terminal's escape hatch, KBD-061). The terminal keeps `Esc` for the shell. | As described |
| A3 | `PgUp` / `PgDn` move one viewport page (minus one line of overlap). Log lines page by viewport instead of a fixed 400 px, so the bar and the content agree. | As described |
| A4 | Image, volume and network details get the same keys through the shared frame. Their static Overview panels have no focus target: `↓` does nothing there and `PgUp` / `PgDn` scroll the body. | As described |
| A5 | No new focus cue (KBD-003). Rows show the cursor tint and the editor its caret. Log lines and the stats selector have no cue beyond the keys working. | As described |
| A6 | No changelog edits (REL-019). The spec changes are in the PR description. | — |

## 4. Requirements (as written into the spec, status `planned`)

| ID | Requirement | New/Changed |
|---|---|---|
| KBD-094 | `Enter` in the search field of a list page applies the query at once, then focuses the table with the cursor on the first matching item row. A second `Enter` opens it (KBD-032). | New |
| KBD-095 | `↓` on a detail page's tab bar moves focus into the active tab's content. When the tab has nothing to focus (loading, error), it does nothing. | New |
| KBD-096 | `PgUp` / `PgDn` on the tab bar scroll the active tab's content by one viewport page. The terminal scrolls its scrollback. | New |
| KBD-097 | `Esc` in a tab's content, `↑` on the first row of a rows panel, and `Mod+Shift+F6` anywhere in a tab return focus to the tab bar. | New |
| KBD-098 | `Backspace` on a detail page goes back (the same as `Alt+←`), except in a text field, the code editor or the terminal. | New |
| SHL-014 | List pages keep their view state while another page is shown and restore it on return; the state is dropped on an engine switch. | New |
| CDT-083 | The per-tab keyboard viewing table (§6.5) for container detail. | New |
| KBD-006, 020, 030, 042, 051, 061, CDT-082, CON-034 | Wording changes listed in the spec edits. | Changed |

## 5. Engine contract impact

None. UI only: no `Engine` method, DTO, capability or hub change, and nothing branches on `EngineKind` (ENG-030).

## 6. Design

### 6.1 Palette height (KBD-020)

**Cause.** In `render_overlay` (`shell/app_shell.rs`) the panel sits in a backdrop that is a flex row, and flex items stretch across the cross axis by default. So the panel (`max_h` 560 px) stretches to 560 px whatever it holds. The palette's own list is capped at 420 px (`palette::build`), so the search row, the list and the padding come to roughly 490 px and about 70 px of empty popover shows below the last row. It's worse after a filter, where a few rows sit in a 560 px panel. The shortcut reference uses the same panel.

**Fix.** The panel keeps its content height: `self_start()` on the panel, the same idiom `tab_bar_frame` uses. The inner list caps (palette 420 px, shortcut list 460 px) are derived from one panel cap, which is clamped to the window height minus the top offset, so the chrome plus the list always fits the panel. Without that, the shortcut reference (title, filter and a 460 px list, roughly 565 px with padding by my estimate) can already exceed the 560 px panel when its list is long, and a short window clips both. Add `debug_selector`s on the panel and the list.

### 6.2 Search → table hand-off (KBD-094)

- `ListTable` gets `focus_first_item(window, cx)`: moves the model cursor to the first non-group row (row 0 if there is none), which also scrolls it into view, and focuses the table.
- Each of the four pages handles `InputEvent::PressEnter` (plain only) in `on_search_event`: cancel the pending debounce task, apply the query now (`rebuild`), then hand off. With an empty query it only focuses the table. With no rows it stays in the field.
- Containers has a background build above 2,000 rows (`BACKGROUND_THRESHOLD`). There the page sets a "cursor to first item" flag that `apply_build` consumes when the current revision lands (the existing stale-revision guard). Focus moves immediately.
- The search field is outside the table's key context, so the same `Enter` can't also fire the table's `Enter` binding. `kbd_094_single_enter_does_not_open_detail` proves it.
- The Logs and quick-find inputs are untouched.

### 6.3 List state on return (SHL-014)

Today `AppShell::mount_page` builds a new page entity on every list route, so search, selection, cursor and scroll are lost. Containers already persists its status filter, group-by, expanded groups and column widths; Images, Volumes and Networks persist only column widths; sort is not persisted anywhere.

**Approach: park, don't snapshot.** When the shell leaves a list route it moves the list page entity into `parked: HashMap<Route, ShellPage>` (at most four) instead of dropping it, and takes it back when that route is mounted again. Everything the user sees (search text, filter, sort, checkbox selection, cursor, the `TableState` scroll handle, quick-find) is already stored on the entity, so no serialisation and no per-page restore code is needed. A snapshot design would need one restore path per page plus scroll-offset restore after first layout.

- The shell remembers which route its current page was mounted for, so `mount_page` knows what to park.
- Restore only when the parked page's `EngineStore` is the active store. `set_engine` clears `parked`, and so does `mount_page` when it shows the disconnected state (SHL-013: no stale ids), so no page outlives an engine switch or a disconnect. While *Degraded* (data stays visible, read-only) parked pages restore normally.
- Parked pages hold no hub streams or timers. `PageView` gets default no-op `suspend` / `resume`. `ContainersPage::suspend` stops the list stats subscriptions (STA-006 streams only for visible rows), and a `suspended` flag stops `update_stats_subscriptions` from re-subscribing on store events. It also closes menus and quick-find. A parked page still applies store refreshes (cheap and id-stable, SHL-004), so the restored rows are current. Task 3 audits Volumes' lazy size loading (VOL-002) the same way.
- On restore the shell focuses the table as it does for any navigation (KBD-007), and does not move the cursor or call `reveal_row`: `TableState::set_selected_row` scrolls the selected row to the top or bottom edge, which would shift the view the user left. The scroll offset lives on the `TableState` handle, which stays alive while the page is parked.
- `Alt+↑` / breadcrumb still call `reveal_row`. If the restored search or filter hides that container, the search is cleared, then the status filter reset to *All*, so the row can be revealed (KBD-042).
- Detail pages are not retained (CDT-081 unchanged). After a delete from the detail, the restored list's cursor falls back to the first row.

### 6.4 `Backspace` (KBD-098, KBD-030)

`backspace → Back` is bound in the `DetailHeader` context. That context covers the header, the tab bar and the non-input tab bodies (rows, log lines, stats selector). It never covers the Logs search, the Inspect editor or the terminal, so KBD-008 holds by construction and the keymap test `keymap_single_letters_only_in_list_contexts` is not affected. The image, volume and network tab bars gain the `DetailHeader` context too (they have only `DetailTabs` today), which also makes the KBD-041 single letters work there. D3 removes the list `backspace → Delete` row for Windows/Linux.

### 6.5 Keyboard viewing of detail tabs (KBD-095…097, CDT-083)

Focus flow on a detail page:

```text
 Header ──Tab──▶ Tab bar ──────────↓──────────▶ Content (rows · log lines · terminal · stats selector · editor)
                  │ ← → switch tab                  │
                  │ PgUp/PgDn scroll the content    │
                  ◀──────── Esc · Mod+Shift+F6 ─────┘ (↑ on the first row of a rows panel)
 Backspace / Alt+← / mouse Back  →  previous page (a list comes back as it was left)
```

New actions in `actions::detail`: `FocusContent`, `FocusTabs`, `ScrollPageUp`, `ScrollPageDown`. Each is bound and also a palette entry (KBD-093). `↓`, `PgUp` and `PgDn` are bound in `DetailTabs`. `Esc` is bound in the content contexts (`DetailRows`, `LogLines`, `StatsWindow`, and a new `DetailInspect` wrapper). `Mod+Shift+F6` (`Cmd+Esc` on macOS) is bound in a new `DetailPage` context on each detail page root; inside the terminal the existing `Terminal`-context binding is deeper and keeps winning. The page handlers dispatch on the current tab, calling each tab's own public methods.

Container detail:

| Tab | `↓` on the bar focuses | `PgUp` / `PgDn` on the bar scroll | Mechanism |
|---|---|---|---|
| Overview, Mounts, Network | the rows panel, cursor on the first row (kept if set) | the panel by one viewport | `RowsState` scroll handle (`scroll_page`) |
| Logs | the log lines | the list by one viewport; scrolling up pauses follow (LOG-003) | `ListState::scroll_by`, `viewport_bounds()` |
| Terminal | the active terminal; with no session open (stopped container) nothing | the scrollback, nothing sent to the shell | new `TerminalView::scroll_page` in `dk-terminal` |
| Stats | the time-window selector | the charts column | new scroll handle on the `StatsTab` root |
| Inspect | the editor (read-only) | the editor viewport | `EditorState::set_scroll_offset`, `visible_row_range`, `line_height` |

- `Esc` in the Inspect editor relies on key-binding fall-through: the read-only editor registers no `Escape` listener, so the shallower `DetailInspect` binding runs. While the editor's own search field is focused, its `Esc` still closes the search. Task 6 verifies this; the fallback is `Mod+Shift+F6` only.
- Image, volume and network details (task 8): the body gets a scroll handle (`PgUp` / `PgDn` from the bar), `↓` focuses the `LinkList` (Used by, Containers) or the Inspect editor. `LinkList` gains `PgUp` / `PgDn`, scroll-into-view and `Esc` / `↑` back to the bar.
- A removed container (CDT-080) or a degraded engine (SHL-013) changes nothing: content stays viewable, so the keys work the same.

### 6.6 Threading

All changes are UI-thread entity updates. No new hub calls, no blocking calls (NFR-001, checked by `xtask check-blocking`). The only async path is the existing Containers background build, guarded by its revision (NFR-005). Parked pages keep their tasks on their own entities.

### 6.7 Errors

None new.

## 7. Tasks

Each task is at most a day. Tasks 1–4 are independent; 6–8 follow 5. Suggested PRs: (a) tasks 1 + 2, (b) task 3, (c) tasks 4–8, which share the keymap.

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Failing test first, then panel `self_start` + viewport clamp for the palette and shortcut reference; before/after screenshots | gpui-ui | KBD-020 | `kbd_020_palette_panel_hugs_its_content`; light and dark screenshots |
| 2 | `ListTable::focus_first_item`; `PressEnter` on Containers, Images, Volumes, Networks (flush debounce; async-build flag on Containers) | gpui-ui | KBD-094 | `kbd_094_*` |
| 3 | Park list pages in `AppShell`; `suspend` / `resume`; stats and menus; Volumes audit; `reveal_row` clears hiding filters; architecture row | gpui-ui | SHL-014, KBD-042 | `shl_014_*`, `sta_006_parked_*`, `kbd_042_*` |
| 4 | `backspace → Back` on `DetailHeader` (and the image/volume/network tab-bar context); remove the list `backspace → Delete` row on Windows/Linux (D3) | gpui-ui | KBD-098, KBD-030 | `kbd_098_*`, keymap tests |
| 5 | Detail frame: four actions, keymap rows (`↓`, `PgUp` / `PgDn`, `Esc`, `Mod+Shift+F6`), palette entries, strings, container page handlers; Overview, Mounts, Network rows panels (`RowsState::scroll_page`, `↑` on the first row) | gpui-ui | KBD-095…097, CDT-083 | `kbd_095_*`, `kbd_096_*`, `kbd_097_*` (rows tabs) |
| 6 | Logs: focus from the bar and viewport paging; Inspect: focus, scroll and `Esc` with the search closed | gpui-ui | KBD-095…097, KBD-051 | same tests, Logs and Inspect cases |
| 7 | Stats: scroll handle and selector `Esc`; `TerminalView::scroll_page` and `↓` into the terminal | gpui-ui | KBD-095…097, KBD-061 | view tests + a `dk-terminal` test |
| 8 | Image, volume and network details: body scroll handle, `LinkList` paging and return keys, Inspect | gpui-ui | KBD-095…098 | `kbd_09x_resource_details_*` |
| 9 | Test matrix check; KBD-082 and KBD-093 still green; ID → test table; KBD-090 walkthrough wording | qa-engineer | all | table in the PR |
| 10 | Review | reviewer | all | verdict: approve (UI thread, no `EngineKind`, strings via `strings.rs`, keymap conflicts) |
| 11 | After merge: `/feature-planning complete keyboard-flow-polish` | architect | all | spec matches code |

## 8. Test plan

View tests use the `testing.rs` harness (`press`, `type_text`, `draw`, `debug_bounds`).

| ID | Layer | Test name(s) (proposed) |
|---|---|---|
| KBD-020 | view | `kbd_020_palette_panel_hugs_its_content` (full list, and filtered to two rows); same panel check for the shortcut reference |
| KBD-094 | view | `kbd_094_enter_in_search_focuses_first_match` (type then `Enter` at once, so the debounce is flushed; `Enter` again opens the detail); `kbd_094_single_enter_does_not_open_detail`; `kbd_094_enter_skips_group_header_and_stays_without_matches`; `kbd_094_enter_hands_off_on_images_volumes_networks`; `kbd_094_enter_waits_for_background_build` (more than 2,000 containers) |
| SHL-014 | view | `shl_014_containers_state_survives_detail_round_trip` (search, selection, cursor, filter, sort; returning by `alt-left`, `backspace`, breadcrumb, `alt-up`, sidebar); `shl_014_scroll_position_survives`; `shl_014_images_volumes_networks_survive_detail_round_trip`; `shl_014_engine_switch_drops_parked_lists`; `sta_006_parked_list_has_no_stats_subscriptions` |
| KBD-042 | view | `kbd_042_alt_up_clears_search_and_filter_hiding_the_row` |
| KBD-098, KBD-030 | view + unit | `kbd_098_backspace_goes_back_on_detail`; `kbd_098_backspace_is_inert_in_logs_search_inspect_editor_and_terminal`; `kbd_030_backspace_no_longer_deletes_on_windows_linux` (cfg); `keymap_no_conflicts_*` |
| KBD-095 | view | `kbd_095_down_enters_content_on_every_tab` (seven tabs; terminal needs a running fake container); `kbd_095_down_without_content_does_nothing` |
| KBD-096, KBD-051 | view + `dk-terminal` | `kbd_096_page_keys_scroll_content_from_the_bar` (rows offset, logs top and follow state, editor offset, stats offset); `kbd_096_terminal_scrolls_scrollback_not_pty`; `kbd_051_logs_page_is_viewport_sized` |
| KBD-097 | view | `kbd_097_esc_up_and_mod_shift_f6_return_to_the_bar` (including the Inspect search-open case) |
| KBD-095…098 | view | `kbd_09x_resource_details_keys` (image, volume, network) |
| KBD-093 | existing | `a11y_every_action_bound_or_in_palette` covers the four new actions |
| KBD-090 | manual | the release-checklist walkthrough gains: search, `Enter`, detail, `Backspace`, ↓ into each tab, `PgUp` / `PgDn`, `Esc` |

## 9. Risks and spikes

| Risk | Mitigation / spike |
|---|---|
| The palette cause is from reading the code | Task 1 writes the failing test and takes a before screenshot first. If the panel is already content-sized, stop and re-diagnose |
| Parked pages do hidden work or keep an old store alive | `suspend` plus the `suspended` flag, `parked.clear()` in `set_engine`, a store-identity check on restore, and the stats-subscription test |
| Scroll restore moves the view | Don't move the cursor on restore (§6.3); `shl_014_scroll_position_survives` guards it. A store refresh that shifts rows above the cursor still scrolls to keep the cursor row visible, as it does on a visible list today (SHL-004) |
| A parked table loses its scroll offset when unmounted | Not expected (the offset lives on the retained handle). If the test shows it, store the top visible row key when parking and call `scroll_to_row` after the first layout |
| `Backspace` deletes after going back | D3; `Backspace` is bound only in non-input contexts |
| Inspect `Esc` shadows the editor's search `Esc` | Relies on a read-only editor having no `Escape` listener; test with the search open; fallback `Mod+Shift+F6` only |
| Logs and Stats give no visible cue when entered | KBD-003 forbids rings; accepted (A5). Tell me if you want a subtle cue |
| Logs paging changes from 400 px to a viewport | Intended (A3); one unit test |
| `↓` into the terminal captures every key | `Mod+Shift+F6` is the documented escape hatch (KBD-061), shown in the terminal hint |
| Scope creep into image, volume and network details | Task 8 is separate and can be dropped without affecting the others |

## 10. Revision log

- 2026-10-10: created (draft, awaiting approval) from the user's four keyboard and palette requests. Docs only; the spec rows are marked planned.
- 2026-10-10: approved by the user ("Approved, implement the plan") with D1–D4 as written. Implementation started.
- 2026-10-10: implementation progress. Done and tested (mutation-checked where noted): task 1 palette panel (`self_start`, viewport cap; `kbd_020_palette_panel_hugs_its_content`, `kbd_022_reference_panel_hugs_its_content`), task 2 search hand-off (`kbd_094_*`), task 3 list parking (`shl_014_*`, `sta_006_parked_*`, `kbd_042_alt_up_clears_*`), task 4 `Backspace` (`kbd_098_*`, D3 applied). Written and compiling, **not yet tested**: tasks 5–8 (actions `detail::FocusContent/FocusTabs/ScrollPageUp/ScrollPageDown`, keymap rows, palette entries, container-page handlers, `ui::scroll`, `RowsState::scroll_page`, Logs viewport paging, Stats scroll handle, Inspect scroll, `TerminalView::scroll_page`, resource details via `LinkList`). Remaining: tests `kbd_095_*`, `kbd_096_*`, `kbd_097_*` (container and resource details), full `cargo test -p dockering`, `cargo test -p dk-terminal`, clippy `-D warnings`, `cargo fmt`, `xtask check-blocking`, KBD-093 coverage test, spec/test tables (`Implementation status`), release-checklist wording, light and dark screenshots, review. Not committed.
- 2026-10-10: tasks 5–8 tested (`kbd_095_*`, `kbd_096_*`, `kbd_097_*`, `kbd_095_resource_detail_*`). Verified on Windows: `cargo test -p dockering --lib` (277 tests, 0 failed), `cargo test -p dk-terminal`, workspace `clippy -D warnings`, `cargo fmt --all`, `xtask check-blocking`. Spec verification tables and the KBD-090 walkthrough updated. Open: full workspace test run, reviewer pass, light and dark palette screenshots, commit, then `/feature-planning complete keyboard-flow-polish`.

