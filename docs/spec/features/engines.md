# Feature: Engine connections & switching

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** ENG (backend reqs ENG-001…025 live in [20-engine-backends.md](../20-engine-backends.md))
- **Plan:** no per-feature plan; built in milestones M1, M2, M7, M8, and M9 of the [v1 plan](../../plan/README.md)

## User stories

- As a Windows user with Docker inside `Ubuntu-22.04` (WSL) and also WSLC, I see both engines in the switcher without configuring anything, and I can switch between them in one click.
- As a Linux user, the app connects to `/var/run/docker.sock` on first launch. If permission is denied, I'm told exactly how to fix it.
- As a user with a remote Docker host, I can add a TCP+TLS engine manually.

## Requirements

| ID | Requirement |
|---|---|
| ENG-100 | The title bar MUST show the active engine: kind icon, name, and a status dot (green connected, amber connecting/degraded, red failed, grey disabled or stopped). |
| ENG-101 | The engine switcher (`Ctrl/Cmd+K`) MUST list every enabled engine grouped by kind, with status, version, and OS/arch on hover. |
| ENG-102 | Selecting an engine MUST switch the whole window to it within one frame. Data loads asynchronously, with skeletons. |
| ENG-103 | First launch MUST auto-select the first *connected* engine. Preference order: `DOCKER_HOST` > current Docker context > local socket/pipe > WSL distro > WSLC. |
| ENG-104 | Settings → Engines MUST list discovered and manual engines with: name (editable), endpoint, origin, enabled toggle, *Test connection*, *Remove* (manual only), and *Hide* (discovered). |
| ENG-105 | *Add engine* dialog: kind = Unix socket / Named pipe / TCP / TCP+TLS (CA, cert, key file pickers) / WSL distro (dropdown of detected distros, mode Bridge or TCP port) / WSLC (session name; transport *Auto*/*COM*/*CLI*). *Test* before *Save*. |
| ENG-106 | A WSL distro that is stopped MUST show a *Start & connect* action. It MUST NOT be booted silently by background pings. |
| ENG-107 | A failed connection MUST show the error with an actionable hint (permission, daemon not running, WSL version too old, socat missing, and so on). |
| ENG-108 | The status bar MUST show the engine version, API version, and OS/arch of the active engine. |
| ENG-109 | WSLC sessions: the default session is listed. Further sessions are listed when "Show all WSLC sessions" is on. |
| ENG-110 | For WSLC engines, the hover tooltip and Diagnostics MUST show the active transport (COM or CLI) and the WSL version. For WSLC, `EngineInfo.server_version` is the WSL version. When the app has fallen back to CLI, or a transport has a limitation, `EngineInfo.transport_note` shows as a small info chip in the status bar, in Settings → Engines, and in Diagnostics (for example, "WSL 2.10 not yet verified — using CLI", or on COM in v1: "Run via COM is not verified for this WSL version"). |
| ENG-111 | **First run / no engine.** When no engine is discovered or connected, the window shows a full-page welcome with per-OS guidance (Linux: install Docker Engine, add the user to the `docker` group. macOS: Docker Desktop / Colima / OrbStack. Windows: Docker Desktop, Docker in a WSL distro, or `wsl --update` for WSLC), plus *Rescan* and *Add engine…*. It's keyboard-operable (KBD-001). |
| ENG-112 | **Unsupported engines** (ssh contexts, API < 1.41) are listed greyed out with the reason and are never contacted (ENG-010). |

## UI

- Switcher popover rows: `[icon] Ubuntu-22.04   Docker 27.3 · linux/amd64   ●`.
- Group headers: *Local*, *WSL distros*, *WSL containers*, *Remote*.
- Footer: `Rescan` · `Manage engines…`.

## Contract usage

`HubHandle::hub_events()`, `HubHandle::set_active(id)`, `HubHandle::add_engine(cfg)`,
`HubHandle::test_engine(cfg) -> HubCall<EngineInfo>`, `HubHandle::rescan()`, `Engine::info()`.

## Acceptance

Manual, on real engines: tracked in the [release checklist](../../plan/release-checklist.md) §3.

- [ ] On Windows with Docker Desktop, Ubuntu+dockerd, and WSLC, all three appear and each can be browsed.
- [ ] Killing dockerd while connected shows *Reconnecting…* and recovers automatically when it returns. (Automated with `FakeEngine`: `eng_021_*`, `eng_022_reconnect_emits_event`.)
- [ ] Switching engines while logs are streaming cancels the stream. No stale rows from the previous engine appear. (Automated: `eng_102_switch_engine_drops_store`, `trm_008_closed_on_engine_switch`.)

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| ENG-100 | Title bar render, no dedicated test (manual, release checklist) |
| ENG-101 | `kbd_021_switcher_filter_arrows_enter` |
| ENG-102 | `eng_102_switch_engine_drops_store`, `eng_102_engine_switch_keeps_lists_and_drops_details` |
| ENG-103 | `eng_103_autoselect_prefers_lower_preference`, `eng_103_last_engine_connects_first` |
| ENG-104 | `eng_104_*` (hub + Settings view tests) |
| ENG-105 | `eng_105_*` (mapping, dialog, test-then-save, failed test) |
| ENG-106 | `eng_106_stopped_distro_is_listed_stopped_and_not_probed`, `eng_106_start_and_connect_activates_a_stopped_non_active_distro` |
| ENG-107 | `eng_107_unreachable_hints`, `eng_107_decodes_wsl_errors_in_either_encoding`, `eng_105_failed_test_shows_hint_and_save_anyway` |
| ENG-108 | `eng_108_engine_info_from_json` (data); the status-bar render is checked manually |
| ENG-109 | `eng_109_show_all_toggles_persist`, `eng_008_list_sessions_parses_table` |
| ENG-110 | `cli_contract` (`transport_note`), `com_live` (ignored, `wsl` runner); the render is checked manually |
| ENG-111 | `eng_111_first_run_add_engine_opens_dialog` |
| ENG-112 | `eng_010_ssh_endpoint_unsupported_never_contacted`, `eng_010_ssh_and_unknown_are_unsupported` |

## Known gaps (v1)

- ENG-009 *Un-merge* in Settings → Engines is offered only for merged engines whose name was seen in the current session (`EngineListStore` resolves "also reachable via" names to ids). After a restart, an engine that has stayed merged since startup has no *Un-merge* button until it appears on its own again.
- ENG-100, ENG-108, and ENG-110 rendering, and the three acceptance items above, are verified only on real engines (release checklist).
