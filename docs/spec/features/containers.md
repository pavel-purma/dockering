# Feature: Containers list & grouping

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** CON
- **Plan:** no per-feature plan; built in milestones M2 and M5 of the [v1 plan](../../plan/README.md)

## User stories

- I see every container on the active engine, with Compose projects collapsed into one group row that I can expand.
- I can start, stop, restart, or delete a single container, a whole Compose project, or a multi-selection.
- I can find a container quickly by name, image, or project.

## Requirements

| ID | Requirement |
|---|---|
| CON-001 | The list MUST show all containers (`all=true`) of the active engine. |
| CON-002 | Columns: ☐ select · Name (with status icon; a link that opens the detail) · Image (text; the image link is in the detail header, CDT-001; truncated, with the full name in a tooltip when it doesn't fit) · Status (chip + health chip + "Up 2 hours" / "Exited (1) 3 min ago"; with a health chip the text drops Docker's `(healthy)` suffix) · Port(s) (links on one line; when they don't fit, a chevron at the end opens a menu listing every port, one per line, published ones open in the browser) · CPU % (optional) · Memory (optional) · Created (relative; the last data column on every list) · Actions. Inline start/stop icons use a soft "chalk" green/red outline that fills while the pointer is over that button. The checkbox and Name columns are pinned to the left edge (they don't scroll horizontally). Columns are resizable and their widths persist (`UiState.column_widths`). *Actions* is fixed-width and pinned to the right edge (always visible). CPU and memory columns are toggled in Settings because they need stats for every visible running container. *v1: the list shows* Created *rather than* Last started, *because `ContainerSummary` has no start time and getting one would need an inspect call per row.* |
| CON-003 | Sort by Name, Image, Status, Created, CPU, and Memory (header click or `Mod+Shift+O`). Inside groups, members sort by the same key. Groups sort by name, or by their first member under the active key. |
| CON-004 | Filter: a segmented control *All* / *Running* / *Stopped* (the selection persists per engine). |
| CON-005 | Search per SHL-006. A match in a group member auto-expands that group. |
| CON-010 | **Group by** (`Select`): *Compose project* (default) / *None* / *Label…* (custom key). Rules in [21 §4](../21-engine-api-contract.md#4-grouping-rules-con-010). |
| CON-011 | A group row is visibly a group header: a tinted band with an accent stripe on its left edge, a disclosure chevron, a stack icon, the project name with a "compose" tag, summed CPU and memory, and group actions (Start all / Stop all / Restart all / Delete all). Groups are **expanded by default**, so every container is visible; the expanded state persists per engine and project. A click anywhere on the group row expands/collapses it. |
| CON-012 | Group rows display the working directory as a tooltip, from `com.docker.compose.project.working_dir`. |
| CON-013 | **Group action semantics.** *Start all*: members start in Compose dependency order when `com.docker.compose.depends_on` labels exist, otherwise in parallel (limit 4). *Stop all*: reverse order, then parallel. *Restart all* = stop all + start all. *Delete all*: the confirm dialog lists the members. Running members need *Force* (checkbox, off by default). Project **networks and volumes are kept** and the dialog says so ("use `docker compose down` to remove networks/volumes"). One-off containers (`com.docker.compose.oneoff=True`) are included and marked. Partial failures are reported per member in one notification. |
| CON-020 | Row actions: Start (if not running), Stop (if running), Restart, Pause/Unpause (overflow menu, capability-gated), Kill (overflow), Delete (`⋮` menu and selection actions only, SHL-026). Inline buttons: start/stop, restart, `⋮`. Also: open in Terminal, View logs, Copy id, and Inspect, through the overflow menu and the right-click `context_menu`. |
| CON-021 | Delete of a running container MUST confirm with "Force delete running container?". Delete of a stopped one asks unless "Don't ask again" is set. |
| CON-022 | Bulk actions bar appears for ≥ 2 selected rows. Selecting a group selects all of its members. |
| CON-023 | *Prune stopped containers* in the header overflow, with confirmation listing the count. |
| CON-030 | The list updates live from engine events within 300 ms of the event (debounce 150 ms + fetch). |
| CON-031 | Optimistic UI: an action immediately shows a spinner on the row. The final state comes from the refetch, and the row is never guessed. |
| CON-032 | Empty state: "No containers yet". When the engine has images, a *Run an image* CTA links to Images. |
| CON-033 | A click anywhere on an item row, or `Enter`, opens [container detail](container-detail.md). |
| CON-034 | Keyboard: all list, group, selection, and row actions are operable from the keyboard per [keyboard.md](keyboard.md) KBD-030…039 (single-letter row actions, `←/→` collapse/expand groups, `Space` select, `Shift+F10` context menu). |

## UI flow

```
Containers page ──(double-click row)──▶ Container detail (Overview)
      │ ──(row ⋮ → Logs)──────────────▶ Container detail (Logs)
      │ ──(row ⋮ → Terminal)──────────▶ Container detail (Terminal)  [disabled if not running]
      │ ──(Image link in detail header)─▶ Image detail
      └ ──(Port link)─────────────────▶ system browser http://localhost:<port>
```

## Data & contract usage

- `EngineStore.containers: Resource<Vec<ContainerSummary>>` ← `Engine::list_containers` (`ContainerQuery::default()` = `all: true`)
- Grouping: `dk_core::grouping::group(&[ContainerSummary], GroupBy, interleave) -> Vec<GroupNode>`. Filter, search, grouping, and sorting run in `background_spawn` above 2,000 rows (`BACKGROUND_THRESHOLD`), with a revision id that drops stale results.
- Actions: `Engine::container_action`, `Engine::remove_container`, `Engine::prune_containers`. Group and bulk fan-out runs 4 in parallel.
- Live: `Engine::events(container)` → 150 ms debounce → refetch
- Optional CPU/mem columns: `hub.stats` (the hub `StatsService`) for **visible running rows only** (`visible_rows_changed`), capped at `EngineInfo.list_stats_limit`, which is per transport (STA-006: Docker/WSLC COM 20, WSL bridge 8, WSLC CLI 0 = columns hidden).

## Acceptance

Manual, on real engines: tracked in the [release checklist](../../plan/release-checklist.md).

- [ ] `docker compose up` of a 3-service project shows one expanded group with 3 rows; clicking the group row collapses it. (Automated with `FakeEngine`: `con_010_compose_group_expanded_then_collapses`, `con_033_row_click_toggles_group_and_opens_item`.)
- [ ] Stopping a group stops all members. (Automated: `con_013_stop_all_stops_running_members`.)
- [ ] 1,000 containers scroll at 60 fps (virtualised). (NFR-011, manual.)
- [ ] Starting a container from the CLI appears in the list in under 1 s. (Automated: `con_030_engine_event_triggers_refetch`.)

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| CON-001 | `containers_data_state_renders_grouped_rows`, `con_010_list_containers` (WSLC CLI contract) |
| CON-002 | `set_020_cpu_columns_toggle_live`; column widths persist through `UiState` (no dedicated test) |
| CON-003 | `con_003_sort_by_status_desc_and_name`, `con_003_sort_by_memory_usage` |
| CON-004 | `con_004_filter_running_and_stopped`, `con_004_filter_clicks_work_after_clicking_header_background` |
| CON-005 | `con_005_search_matches_member_and_expands_group` |
| CON-010 | `con_010_*` (dk-core grouping, list model, view), `kbd_039_group_by_none_flattens` |
| CON-011 | `con_011_aggregate_*`, `con_011_list_default_expands_groups`, `con_010_groups_start_collapsed_and_flatten_on_expand` (model) |
| CON-012 | `con_012_working_dir` |
| CON-013 | `con_013_*` (start order, cycles, stop all, delete all) |
| CON-020 | `kbd_030_s_starts_stopped_container`, `kbd_036_shift_f10_opens_row_menu` |
| CON-021 | `con_021_running_delete_needs_force`, `set_001_start_page_and_confirm_stopped_persist` |
| CON-022 | `kbd_034_space_on_group_toggles_members`, `kbd_038_select_all_includes_collapsed_members_and_targets` |
| CON-023 | Implemented (`on_prune`, engine `prune_containers` contract tests); no view test |
| CON-030 | `con_030_engine_event_triggers_refetch` |
| CON-031 | `kbd_030_s_starts_stopped_container` (pending, then refetched state) |
| CON-032 | `containers_empty_state` (empty list; the CTA render isn't asserted) |
| CON-033 | `con_033_enter_opens_detail_with_primary_focus`, `con_033_row_click_toggles_group_and_opens_item` |
| CON-034 | KBD-030…039 tests (see [keyboard.md](keyboard.md)) |

## Known gaps (v1)

- CON-002/003: *Last started* isn't a column or a sort key. *Created* replaces it (see CON-002).
- CON-020: *Pause/Unpause* and *Kill* are implemented (capability-gated) but have no view test. CON-023 *Prune stopped containers* has no view test.
- Acceptance items are verified on real engines only through the release checklist.
