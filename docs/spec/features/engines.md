# Feature: Engine connections & switching

- **Status:** implemented (2026-10-04)
- **Requirement prefix:** ENG (backend reqs ENG-001…025 live in [20-engine-backends.md](../20-engine-backends.md))
- **Plan:** built in milestones M1, M2, M7, M8, and M9 of the v1 plan; scan lifecycle, stable listing, and default engine: [engine-scan-and-defaults](../../plan/features/engine-scan-and-defaults.md)

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
| ENG-103 | **Startup engine** (changed 2026-10-04). In order: (1) the pinned *default engine* (ENG-116) if it exists, is enabled, is not hidden, and is contactable (not unsupported); (2) the last-used engine (`state.json` `last_engine`) under the same conditions; (3) auto-select the first *connected* engine. Preference order for (3): `DOCKER_HOST` > current Docker context > local socket/pipe > WSL distro > WSLC. |
| ENG-104 | Settings → Engines MUST list discovered and manual engines with: name (editable), endpoint, origin, enabled toggle, *Test connection*, *Remove* (manual only), and *Hide* (discovered). |
| ENG-105 | *Add engine* dialog: kind = Unix socket / Named pipe / TCP / TCP+TLS (CA, cert, key file pickers) / WSL distro (dropdown of detected distros, mode Bridge or TCP port) / WSLC (session name; transport *Auto*/*COM*/*CLI*). *Test* before *Save*. |
| ENG-106 | A WSL distro that is stopped MUST show a *Start & connect* action. It MUST NOT be booted silently by background pings. |
| ENG-107 | A failed connection MUST show the error with an actionable hint (permission, daemon not running, WSL version too old, socat missing, and so on). |
| ENG-108 | The status bar MUST show the engine version, API version, and OS/arch of the active engine. |
| ENG-109 | WSLC sessions: the default session is listed. Further sessions are listed when "Show all WSLC sessions" is on. |
| ENG-110 | For WSLC engines, the hover tooltip and Diagnostics MUST show the active transport (COM or CLI) and the WSL version. For WSLC, `EngineInfo.server_version` is the WSL version. When the app has fallen back to CLI, or a transport has a limitation, `EngineInfo.transport_note` shows as a small info chip in the status bar, in Settings → Engines, and in Diagnostics (for example, "WSL 2.10 not yet verified — using CLI", or on COM in v1: "Run via COM is not verified for this WSL version"). |
| ENG-111 | **First run / no engine.** When no engine is discovered or connected, the window shows a full-page welcome with per-OS guidance (Linux: install Docker Engine, add the user to the `docker` group. macOS: Docker Desktop / Colima / OrbStack. Windows: Docker Desktop, Docker in a WSL distro, or `wsl --update` for WSLC), plus *Rescan* and *Add engine…*. It's keyboard-operable (KBD-001). |
| ENG-112 | **Unsupported engines** (ssh contexts, API < 1.41) are listed greyed out with the reason and are never contacted (ENG-010). |
| ENG-113 | **Scan lifecycle** (new). Discovery MUST run once per app start and again only on an explicit *Rescan*: the button in Settings → Engines, the *Rescan engines* command-palette entry, or the first-run screen (ENG-111). Nothing else (engine switch, connect, reconnect, background probe, ping) may add, remove, or hide an engine. Background probes only change an engine's *state*. A *Rescan* made while no engine is active applies the ENG-103 startup order (default, last-used, auto-select). |
| ENG-114 | **Stable listing** (new; replaces the hiding half of ENG-009). Every discovered or manual engine MUST stay in the registry and in `hub_events()` for the whole session, whichever engine is active. The only things that suppress an engine are the user's own *Enabled* (ENG-025) and *Hide* (ENG-104) toggles, and the *Show all …* discovery toggles (ENG-007/109). A *Rescan* may reclassify a distro or session as "no Docker" only before the user has seen it: an engine that is already listed is never hidden by a rescan, while a hidden one becomes listed once it qualifies. After a restart the first scan classifies each engine afresh, so a stored override of a no-Docker distro stays unlisted while *Show all …* is off. An engine that reaches the same daemon as another (ENG-009) is annotated, never hidden. |
| ENG-115 | **Per-engine settings survive rescans** (new). A discovered engine's *name*, *Enabled*, and *Hidden* values are kept in memory at once and written to `config.toml` within 0.5 s (a debounced atomic save, flushed again at quit, and retried if a write fails), and are re-applied on every merge (startup and *Rescan*), including for engines the scan didn't find (they stay listed as unavailable, spec 20 §2). A rescan MUST NOT re-enable, un-hide, or rename an engine. |
| ENG-116 | **Default engine** (new). Settings → Engines lets the user pin one engine as the *default* (`engines.default` in `config.toml`, an engine id). Row control: *Set as default* / *Clear default*, plus a *Default* tag. Pinning also stores the engine's config in `engines.entries`, so the hub registers it before discovery runs and a slow or timed-out startup scan can't make another engine win. A *Make active engine the default* command-palette entry does the same for the active engine, and refuses an engine that is disabled, hidden, or unsupported. The switcher marks the default's row. The pin survives restarts and rescans. Switching engines at runtime does NOT change it. Removing a *manual* engine clears its pin. *Remove* on a discovered engine only hides it, so the pin stays. A default that's disabled, hidden, unsupported, or not found falls through to ENG-103 (2)/(3), and its row shows *Default (unavailable)*. A default or last-used engine that is a stopped WSL distro is still opened: it shows *Stopped* with *Start & connect* (ENG-106) rather than being replaced silently. |

## UI

- Switcher popover rows: `[icon] Ubuntu-22.04   Docker 27.3 · linux/amd64   ●`.
- Group headers: *Local*, *WSL distros*, *WSL containers*, *Remote*.
- Footer: `Manage engines…` (the *Rescan* button moved to Settings → Engines, ENG-113).
- Same-daemon note (ENG-009/114): the tooltip and the Settings row list "Same daemon as: <name>".

## Contract usage

`HubHandle::hub_events()`, `HubHandle::set_active(id)`, `HubHandle::add_engine(cfg)`,
`HubHandle::test_engine(cfg) -> HubCall<EngineInfo>`, `HubHandle::rescan()`, `Engine::info()`, `ConfigHandle` (`engines.default`).

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
| ENG-103 | `eng_103_autoselect_prefers_lower_preference`, `eng_103_last_engine_connects_first`, `eng_103_default_engine_connects_first`, `eng_103_unavailable_default_falls_back`, `eng_103_rescan_without_active_engine_prefers_the_default`, `eng_116_hidden_default_falls_through` |
| ENG-104 | `eng_104_*` (hub + Settings view tests) |
| ENG-105 | `eng_105_*` (mapping, dialog, test-then-save, failed test) |
| ENG-106 | `eng_106_stopped_distro_is_listed_stopped_and_not_probed`, `eng_106_start_and_connect_activates_a_stopped_non_active_distro` |
| ENG-107 | `eng_107_unreachable_hints`, `eng_107_decodes_wsl_errors_in_either_encoding`, `eng_105_failed_test_shows_hint_and_save_anyway` |
| ENG-108 | `eng_108_engine_info_from_json` (data); the status-bar render is checked manually |
| ENG-109 | `eng_109_show_all_toggles_persist`, `eng_008_list_sessions_parses_table` |
| ENG-110 | `cli_contract` (`transport_note`), `com_live` (ignored, `wsl` runner); the render is checked manually |
| ENG-111 | `eng_111_first_run_add_engine_opens_dialog` |
| ENG-112 | `eng_010_ssh_endpoint_unsupported_never_contacted`, `eng_010_ssh_and_unknown_are_unsupported` |
| ENG-113 | `eng_113_no_discovery_after_start_until_rescan` |
| ENG-114 | `eng_114_same_daemon_engines_stay_listed_after_switch_and_rescan` (regression for the vanishing-engine bug), `eng_114_same_daemon_note_is_symmetric`, `eng_114_same_daemon_note_follows_rename_and_disable`, `eng_009_same_daemon_is_annotated_not_hidden` |
| ENG-115 | `eng_115_disabled_hidden_name_survive_rescan`, `eng_115_overrides_of_unfound_engines_are_kept`, `eng_115_overrides_survive_restart` (the pinned default beats preference and last-used after a restart), `eng_115_disable_active_then_rescan_then_reenable`, `eng_114_rescan_never_hides_a_listed_engine`, `eng_114_stored_override_of_a_no_docker_distro_stays_unlisted_after_restart`, `eng_115_failed_config_save_is_retried` |
| ENG-116 | `eng_116_runtime_switch_keeps_default_and_remove_clears_it`, `eng_116_removing_a_discovered_default_keeps_the_pin`; `eng_116_default_wins_even_when_the_startup_scan_misses_it`, `eng_116_default_mark_follows_what_the_hub_would_open`; view tests `eng_116_set_and_clear_default_persist`, `eng_116_palette_command_pins_active_engine`, `eng_116_palette_command_refuses_a_disabled_engine`, `eng_116_refresh_config_follows_a_default_changed_by_the_hub`; config compat in `config_roundtrip_and_corrupt_file_backup` |

## Known gaps (v1)

- ~~ENG-009 *Un-merge*~~ (removed 2026-10-04): engines are no longer merged away (ENG-114), so there is nothing to un-merge. The `engines.unmerged` key in old `config.toml` files is ignored.
- A discovered engine that arrives under a new id but the same endpoint as a stale entry (for example `DOCKER_HOST` unset, the same socket now found as `docker-local`) is dropped by the endpoint-clash filter, so it stays under the stale id and name. The filter compares against the entry's old endpoint, so a pinned engine whose endpoint has moved can also shadow another engine that now sits on its old one.
- *Clear default* leaves the engine's stored config in `engines.entries` (pinning stored it). If that engine later disappears from discovery it stays listed as unavailable and ranks as a manual engine in auto-select.
- The "Same daemon as" note appears only once an engine has connected in the current session (`/info.ID` is read on connect). It names peers by display name, so two engines with the same name are ambiguous, and it lists only peers that are enabled, not hidden, and listed.
- The *Default* tag and *Default (unavailable)* are decided by `default_mark` (unit-tested) and drawn in Settings and the switcher; the drawing itself is checked manually.
- ENG-100, ENG-108, and ENG-110 rendering, and the three acceptance items above, are verified only on real engines (release checklist).
