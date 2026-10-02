# Feature: Containers list & grouping

- **Status:** in-progress
- **Requirement prefix:** CON
- **Plan:** [docs/plan/features/containers.md](../../plan/features/containers.md) (to be written)

## User stories

- I see every container on the active engine, with Compose projects collapsed into one group row that I can expand.
- I can start, stop, restart, or delete a single container, a whole Compose project, or a multi-selection.
- I can find a container quickly by name, image, or project.

## Requirements

| ID | Requirement |
|---|---|
| CON-001 | The list MUST show all containers (`all=true`) of the active engine. |
| CON-002 | Columns: ☐ select · Name (with status icon) · Image (link) · Status (chip + "Up 2 hours" / "Exited (1) 3 min ago") · CPU % (optional) · Memory (optional) · Port(s) (links) · Last started (relative) · Actions. Columns are resizable and their widths persist. CPU and memory columns are toggled in Settings because they need stats for every running container. |
| CON-003 | Sort by Name, Image, Status, Last started, and CPU. Inside groups, members sort by the same key. Groups sort by name or by aggregated key. |
| CON-004 | Filter: a segmented control *All* / *Running* / *Stopped* (the selection persists per engine). |
| CON-005 | Search per SHL-006. A match in a group member auto-expands that group. |
| CON-010 | **Group by** (`Select`): *Compose project* (default) / *None* / *Label…* (custom key). Rules in [21 §4](../21-engine-api-contract.md#4-grouping-rules-con-010). |
| CON-011 | A group row shows a disclosure chevron, the project name with a "compose" tag, an aggregated status (`3/3 running`), summed CPU and memory, and group actions (Start all / Stop all / Restart all / Delete all). The expanded state persists per engine and project. |
| CON-012 | Group rows display the working directory as a tooltip, from `com.docker.compose.project.working_dir`. |
| CON-013 | **Group action semantics.** *Start all*: members start in Compose dependency order when `com.docker.compose.depends_on` labels exist, otherwise in parallel (limit 4). *Stop all*: reverse order, then parallel. *Restart all* = stop all + start all. *Delete all*: the confirm dialog lists the members. Running members need *Force* (checkbox, off by default). Project **networks and volumes are kept** and the dialog says so ("use `docker compose down` to remove networks/volumes"). One-off containers (`com.docker.compose.oneoff=True`) are included and marked. Partial failures are reported per member in one notification. |
| CON-020 | Row actions: Start (if not running), Stop (if running), Restart, Pause/Unpause (overflow menu, capability-gated), Kill (overflow), Delete. Also: open in Terminal, View logs, Copy id, and Inspect, through the overflow menu and the right-click `context_menu`. |
| CON-021 | Delete of a running container MUST confirm with "Force delete running container?". Delete of a stopped one asks unless "Don't ask again" is set. |
| CON-022 | Bulk actions bar appears for ≥ 2 selected rows. Selecting a group selects all of its members. |
| CON-023 | *Prune stopped containers* in the header overflow, with confirmation listing the count. |
| CON-030 | The list updates live from engine events within 300 ms of the event (debounce 150 ms + fetch). |
| CON-031 | Optimistic UI: an action immediately shows a spinner on the row. The final state comes from the refetch, and the row is never guessed. |
| CON-032 | Empty state: "No containers yet". When the engine has images, a *Run an image* CTA links to Images. |
| CON-033 | Double-click, `Enter`, or a click on the name opens [container detail](container-detail.md). |
| CON-034 | Keyboard: all list, group, selection, and row actions are operable from the keyboard per [keyboard.md](keyboard.md) KBD-030…039 (single-letter row actions, `←/→` collapse/expand groups, `Space` select, `Shift+F10` context menu). |

## UI flow

```
Containers page ──(double-click row)──▶ Container detail (Overview)
      │ ──(row ⋮ → Logs)──────────────▶ Container detail (Logs)
      │ ──(row ⋮ → Terminal)──────────▶ Container detail (Terminal)  [disabled if not running]
      │ ──(Image link)────────────────▶ Image detail
      └ ──(Port link)─────────────────▶ system browser http://localhost:<port>
```

## Data & contract usage

- `EngineStore.containers: Resource<Vec<ContainerSummary>>` ← `Engine::list_containers`
- Grouping: `dk_core::grouping::group(&[ContainerSummary], GroupBy) -> Vec<GroupNode>`, run in `background_spawn` when there are more than 500 rows.
- Actions: `Engine::container_action`, `Engine::remove_container`, `Engine::prune_containers`
- Live: `Engine::events(container)` → debounce → refetch
- Optional CPU/mem columns: `StatsService` multiplexes `Engine::stats` for **visible running rows only** (`visible_rows_changed`), with at most 20 concurrent streams.

## Acceptance

- [ ] `docker compose up` of a 3-service project shows one collapsed group that expands to 3 rows.
- [ ] Stopping a group stops all members, and the aggregate turns to `0/3 running`.
- [ ] 1,000 containers scroll at 60 fps (virtualised).
- [ ] Starting a container from the CLI appears in the list in under 1 s.
