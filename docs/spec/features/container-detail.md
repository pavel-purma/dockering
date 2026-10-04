# Feature: Container detail

- **Status:** implemented (2026-10-04, including WSLC inspect parity CDT-030/040; cross-OS release validation remains open in the plan)
- **Requirement prefix:** CDT
- **Plan:** baseline built in milestone M3; WSLC inspect repair: [wslc-integration-repair](../../plan/features/wslc-integration-repair.md) (done; completion evidence/release gates)

## Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ‹ Containers › myshop › web-1                                                │
│ ● web-1   nginx:1.27 ↗   a1b2c3d4e5f6 ⧉   8080:80 ↗     [■ Stop][⟳][⋮][🗑] │
│ Running · Up 2 hours · healthy                                               │
├──────────────────────────────────────────────────────────────────────────────┤
│ [Overview] [Logs] [Terminal] [Stats] [Mounts] [Network] [Inspect]            │
├──────────────────────────────────────────────────────────────────────────────┤
│ tab content                                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Requirements

| ID | Requirement |
|---|---|
| CDT-001 | The header MUST show: status dot and name, image (link to image detail), short id with copy, published ports as links, status text, health chip, and the action buttons from CON-020. |
| CDT-002 | Tabs: Overview, Logs, Terminal, Stats, Mounts, Network, Inspect. The tab is part of the route, so deep links and back/forward work. |
| CDT-003 | Detail data (`inspect_container`) refreshes on any engine event for this container id. |
| CDT-010 | **Overview** (`DescriptionList` sections under `ui::section` headings): *General* (id, name, image + image id, created, started, finished, restart count, restart policy, platform, PID, exit code). *Command* (entrypoint, cmd, working dir, user, tty). *Compose* (project, service, number, working dir, config files), shown if grouped. *Environment* (table of key/value, with values masked if the key matches `pass|secret|token|key` and a reveal toggle per row). *Labels* (table). *Resources* (CPU limit, memory limit, pids limit). *Health* (last 5 results with exit code and output). |
| CDT-020 | **Mounts**: table with Type (bind/volume/tmpfs), Source (a volume name links to volume detail), Destination, Mode. Mode is `RW`/`RO` from the access flag plus any other mode options (e.g. `RO, Z`). Source and Destination get most of the width; Type and Mode are narrow. |
| CDT-030 | **Network**: (a) *Port bindings* table: Container port/proto, Host IP, Host port (link). (b) *Networks* table: network name (link), IPv4, IPv6, gateway, MAC, aliases. (c) Hostname, DNS, and network mode. WSLC top-level inspect `Ports` MUST populate both typed summary ports and port bindings through COM and CLI, including IPv4/IPv6 and protocol mappings (ENG-133); normalize typed fields without modifying original `raw` (CDT-040). |
| CDT-040 | **Inspect**: read-only, syntax-highlighted JSON of raw inspect output (pretty-printed on `background_spawn`) in the GPUI Kit code editor, with *Copy* and search (`Ctrl/Cmd+F` within the editor). Env values are **not** masked here; a warning says so. `ContainerDetails.raw` MUST retain the original parsed inspect object; typed normalization copies MUST NOT add/move `Ports` or otherwise alter raw JSON (ENG-133). Pretty-printing is allowed; transport framing arrays may be unwrapped to the selected original object. |
| CDT-050 | **Logs**: see [container-logs.md](container-logs.md). |
| CDT-060 | **Terminal**: see [container-terminal.md](container-terminal.md). |
| CDT-070 | **Stats**: see [container-stats.md](container-stats.md). |
| CDT-080 | When the container is removed while open: banner (SHL), all tabs read-only, and actions disabled. |
| CDT-081 | Tabs keep their state while switching between tabs on the same container: log scroll position, terminal session, stats buffer. Leaving the container detail tears them down, except terminals (see TRM-008). |
| CDT-082 | Keyboard: tab switching, header actions, breadcrumb, links, and copying values per [keyboard.md](keyboard.md) KBD-040…044. Overview, Mounts, and Network are each one focusable rows panel (↑/↓ cursor, `Enter` follows a link, `Space` reveals a masked value, `Mod+C` copies). |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| CDT-001 | `cdt_001_delete_running_confirms_with_force`, `cdt_041_header_letters_start_and_stop` |
| CDT-002 | `cdt_002_tab_in_route_and_back_forward_keep_tab` |
| CDT-003 | `cdt_003_refreshes_on_engine_event` |
| CDT-010 | `cdt_010_overview_masks_secrets_and_reveal_works` |
| CDT-020 | `cdt_020_mounts_volume_link_opens_volume_detail`, `cdt_020_mode_label_merges_access_and_options` |
| CDT-030 | `cdt_030_network_link_and_port_link` |
| CDT-040 | `cdt_040_inspect_shows_pretty_json_unmasked` |
| CDT-050…070 | See the logs, terminal, and stats specs |
| CDT-080 | `cdt_080_removed_container_banner_and_read_only`, `cdt_080_removed_detected_from_list_refetch`, `trm_008_closed_on_container_removal` |
| CDT-081 | `cdt_081_tab_state_survives_switching` |
| CDT-082 | `kbd_042_alt_up_focuses_row_in_parent_list`, `kbd_044_rows_arrows_and_enter_follows_image_link`, `kbd_040_step_tab_skips_disabled` |

Loading and error states: `cdt_states_loading_and_error_with_retry`.

## WSLC repair verification / limits (2026-10-04)

- Both delegates use shared `inspect::container_details`; typed normalization copies the original JSON and restores untouched `raw`. Passing tests: `eng_133_ports_and_cdt_040_original_raw`, `eng_133_recorded_com_cli_inspect_raw_deep_equality`, `eng_133_cdt_030_040_published_ports_both_delegate_paths`, `img_005_run_exited_detail_cdt_030_040`, plus existing Network/Inspect view tests.
- Expanded live harness passed both typed inspect/log paths for four hello-world containers, Exited/0 `/hello`, original COM raw deep equality. Published-port coverage is synthetic/recorded fixture and fake-delegate evidence (IPv4/IPv6, TCP/UDP, exposed-only, absent NetworkSettings); live hello-world has empty ports. No live published-port Run is claimed. See [plan completion/release gates](../../plan/features/wslc-integration-repair.md); cross-OS validation remains open.
