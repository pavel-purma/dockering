# Feature: Engine connections & switching

- **Status:** implemented (2026-10-04, including WSLC repairs ENG-126…136; cross-OS release validation remains open in the plan)
- **Requirement prefix:** ENG (backend reqs ENG-001…025 live in [20-engine-backends.md](../20-engine-backends.md))
- **Plan:** built in milestones M1, M2, M7, M8, and M9; scan/defaults: [engine-scan-and-defaults](../../plan/features/engine-scan-and-defaults.md) (done); WSLC repairs: [wslc-integration-repair](../../plan/features/wslc-integration-repair.md) (done; completion evidence/release gates)

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
| ENG-109 | WSLC sessions: the default session MUST remain listed. Further sessions appear with "Show all WSLC sessions" only when both caller and creator SIDs are known, valid and equal. Missing SID MUST fail closed; CLI session-table names alone are not ownership evidence (ENG-135), so discovery does not invoke CLI enumeration for extras. Preserve previously listed engines under ENG-114. |
| ENG-110 | WSLC hover tooltip and Diagnostics MUST show primary transport and WSL version (`EngineInfo.server_version`). The existing `transport_note` info chip in the status bar, Settings → Engines and Diagnostics MUST describe limitations and mixed/degraded routes (e.g. "COM primary; Run uses CLI — native Run unverified"). Do not label the whole engine CLI merely because Run uses CLI, or use a last-call transport label; metadata MUST refresh even with unchanged capabilities (ENG-136). Strict COM note: "COM only; native Run unverified". |
| ENG-111 | **First run / no engine.** When no engine is discovered or connected, the window shows a full-page welcome with per-OS guidance (Linux: install Docker Engine, add the user to the `docker` group. macOS: Docker Desktop / Colima / OrbStack. Windows: Docker Desktop, Docker in a WSL distro, or `wsl --update` for WSLC), plus *Rescan* and *Add engine…*. It's keyboard-operable (KBD-001). |
| ENG-112 | **Unsupported engines** (ssh contexts, API < 1.41) are listed greyed out with the reason and are never contacted (ENG-010). |
| ENG-113 | **Scan lifecycle** (new). Discovery MUST run once per app start and again only on an explicit *Rescan*: the button in Settings → Engines, the *Rescan engines* command-palette entry, or the first-run screen (ENG-111). Nothing else (engine switch, connect, reconnect, background probe, ping) may add, remove, or hide an engine. Background probes only change an engine's *state*. A *Rescan* made while no engine is active applies the ENG-103 startup order (default, last-used, auto-select). |
| ENG-114 | **Stable listing** (new; replaces the hiding half of ENG-009). Every discovered or manual engine MUST stay in the registry and in `hub_events()` for the whole session, whichever engine is active. The only things that suppress an engine are the user's own *Enabled* (ENG-025) and *Hide* (ENG-104) toggles, and the *Show all …* discovery toggles (ENG-007/109). A *Rescan* may reclassify a distro or session as "no Docker" only before the user has seen it: an engine that is already listed is never hidden by a rescan, while a hidden one becomes listed once it qualifies. After a restart the first scan classifies each engine afresh, so a stored override of a no-Docker distro stays unlisted while *Show all …* is off. An engine that reaches the same daemon as another (ENG-009) is annotated, never hidden. |
| ENG-115 | **Per-engine settings survive rescans** (new). A discovered engine's *name*, *Enabled*, and *Hidden* values are kept in memory at once and written to `config.toml` within 0.5 s (a debounced atomic save, flushed again at quit, and retried if a write fails), and are re-applied on every merge (startup and *Rescan*), including for engines the scan didn't find (they stay listed as unavailable, spec 20 §2). A rescan MUST NOT re-enable, un-hide, or rename an engine. |
| ENG-116 | **Default engine** (new). Settings → Engines lets the user pin one engine as the *default* (`engines.default` in `config.toml`, an engine id). Row control: *Set as default* / *Clear default*, plus a *Default* tag. Pinning also stores the engine's config in `engines.entries`, so the hub registers it before discovery runs and a slow or timed-out startup scan can't make another engine win. A *Make active engine the default* command-palette entry does the same for the active engine, and refuses an engine that is disabled, hidden, or unsupported. The switcher marks the default's row. The pin survives restarts and rescans. Switching engines at runtime does NOT change it. Removing a *manual* engine clears its pin. *Remove* on a discovered engine only hides it, so the pin stays. A default that's disabled, hidden, unsupported, or not found falls through to ENG-103 (2)/(3), and its row shows *Default (unavailable)*. A default or last-used engine that is a stopped WSL distro is still opened: it shows *Stopped* with *Start & connect* (ENG-106) rather than being replaced silently. |

### WSLC repairs (implemented 2026-10-04)

The stable IDs below describe implemented behavior. ENG-120…125 remain reserved for the deferred macOS backend. See the plan completion record for actual Windows tests/live evidence and open cross-OS release validation; native CreateContainer remains separately deferred.

| ID | Requirement |
|---|---|
| ENG-126 | WSLC MUST expose one private router through `EngineFactory`. Auto MUST prefer trusted COM per operation; forced COM and CLI MUST prohibit the other transport for that connection. |
| ENG-127 | Routes, reopen attempts and reconciliation reads MUST preserve the same resolved session and caller security context. Failure to establish equivalent targeting MUST prohibit cross-transport dispatch. |
| ENG-128 | Auto MUST select CLI before dispatch for explicitly unverified native operations, initially `run_image`. It MUST NOT execute an unverified native call or catch arbitrary 501 errors to discover that condition. |
| ENG-129 | Read fallback MUST use an explicit transport-fault allowlist and bounded attempts. Domain, policy, authorization, validation, cancellation and payload/protocol errors MUST NOT trigger fallback. |
| ENG-130 | Possibly dispatched mutations MUST NOT be replayed automatically. The router MUST perform bounded read-only reconciliation where meaningful and otherwise report an unknown outcome with refresh-before-retry guidance. |
| ENG-131 | Read-stream fallback MUST occur only before the first source item. After emission, a transport fault MUST terminate the subscription without source splicing. Pull and exec MUST additionally obey mutation dispatch safety. |
| ENG-132 | Internal ABI calls MUST require an explicit trusted exact version selected before activation. Self-check MUST confirm, not establish, trust. Native operation enablement MUST require operation-specific evidence. |
| ENG-133 | COM and CLI inspect conversion MUST preserve equivalent typed ports and other shared fields and MUST preserve original inspect JSON in `raw`. |
| ENG-134 | Canceled queued RPCs MUST NOT dispatch. Required operation-token failure MUST prevent the protected call. Resources MUST have handle-type-correct ownership/cancellation without blocking teardown on UI/hub workers. |
| ENG-135 | Extra-session discovery MUST fail closed for unproven ownership. Policy errors MUST prohibit CLI enumeration/fallback. CLI table rows lacking ownership evidence MUST NOT become extra discovered engines. |
| ENG-136 | Routing metadata MUST describe mixed/degraded routes and propagate to registry, active store, status bar, Settings and Diagnostics even when capabilities do not change. |

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

- Native CreateContainer remains unverified: strict COM Run returns 501; Auto uses pinned-session CLI. Mixed targeting revalidates runtime session ID/name/PID/SID before spawn, but CLI has no atomic identity-check-and-dispatch API: replacement after validation is a documented residual race, not a durable/atomic identity guarantee. Mixed volume-prune and explicit-local create-volume fallback are refused; supplied CLI pull auth is rejected before spawn.
- Cross-OS release validation remains open: Linux cross-check blocked by missing `x86_64-linux-gnu-gcc` for ring; macOS not run. These are release gates, not reported production blockers. No three-OS green/full DoD claim. Live policy/fault injection, sustained socket leak/marshalling stress and nonempty-baseline cleanup preservation are not claimed.

## WSLC repair verification (2026-10-04)

Windows workspace PASS: WSLC 183 passed/18 live opt-in ignores, hub 77/UI 243 passed. Expanded `eng_127_128_normal_user_hello_world_acceptance` separately passed (1 test, 20.03 s). Full provenance, exact command and coverage are in [plan §8/completion](../../plan/features/wslc-integration-repair.md).

| ID | Actual passing tests (module prefixes omitted) |
|---|---|
| ENG-013, ENG-126 | `plan_matrix`, `eng_126_strict_preferences_and_eng_128_run_exception`, `eng_126_missing_cli_preserves_com_and_strict_cli_no_com`, `eng_126_router_contract_suite` |
| ENG-127 | `eng_127_default_pinned_reopen_replacement_refused`, `eng_127_validate_before_each_crossing_replacement_refuses`, `eng_127_saturated_permit_revalidates_before_mutation_spawn`, `eng_127_exec_revalidates_after_inspect_inside_blocking_spawn`, `eng_127_cli_stats_revalidates_every_poll_and_stops_on_replacement`; live default/explicit/admin matrix |
| ENG-128 | `eng_126_strict_preferences_and_eng_128_run_exception`; expanded live harness |
| ENG-129 | `eng_129_allowlist_budget_and_sticky_routes`, `eng_129_domain_policy_protocol_cancel_never_fallback`, `eng_129_reopen_domain_and_second_read_protocol_final`, `eng_129_cli_malformed_inspect_must_remain_protocol_error`, `eng_129_com_malformed_payload_no_implicit_retry` |
| ENG-130 | `eng_130_commit_disconnect_no_replay`, `eng_130_timeout_then_late_commit_is_one_mutation`, `eng_130_completed_create_enrichment_failure_never_recreates`, `eng_130_mutation_phase_matrix_one_dispatch_and_parity_gate`, `eng_130_img_005_cli_lost_run_id_not_fabricated` |
| ENG-131 | `eng_131_source_boundary_clean_eof_and_pull_dispatch`, `eng_131_filtered_event_counts_as_source_activity`, `eng_131_events_lost_continues_and_auth_never_discarded`, `eng_131_exec_committed_getstdhandle_failure_never_reexecs`, `eng_131_terminal_remains_pinned_after_future_exec_route_changes`, `eng_131_after_item_failure_next_subscription_waits_for_old_source` |
| ENG-132 | `eng_132_exact_allowlist`, `eng_132_unknown_version_zero_activation`, `struct_layouts_match_midl_x64`, `slot_offsets_of_called_methods`; live 3.0.1.0 self-check/inspect/logs |
| ENG-133 | `eng_133_ports_and_cdt_040_original_raw`, `eng_133_recorded_com_cli_inspect_raw_deep_equality`, `eng_133_cdt_030_040_published_ports_both_delegate_paths` |
| ENG-134 | `eng_134_cancelled_queue_no_rpc`, `eng_134_admission_race`, `eng_134_begin_token_failure_blocks`, `eng_134_token_failure_blocks_remove_logs_and_exec`, `eng_134_socket_close_once_and_kernel_tags`, `eng_134_socket_cancel_drains_overlapped_before_close`, `eng_134_cancel_queued_blocking_spawn_zero_admissions` |
| ENG-109, ENG-135 | `eng_135_missing_sid_excluded`, `eng_135_factory_policy_zero_cli_preparation_and_default_visible`, `explicit_target_contradiction_final_zero_cli_and_protected_calls`; live admin rejection |
| ENG-110, ENG-136 | `eng_136_coherent_metadata_no_last_call_demotion`, `eng_136_info_without_caps_change_and_cli_stats_limit`, `eng_136_caps_and_metadata_commit_one_coherent_snapshot`, `eng_136_stale_connection_info_ignored`, `eng_136_refresh_failure_timeout_and_panic_preserve_snapshot`, `eng_136_metadata_without_flags_preserves_focus_page_and_updates_stats` |

- ~~ENG-009 *Un-merge*~~ (removed 2026-10-04): engines are no longer merged away (ENG-114), so there is nothing to un-merge. The `engines.unmerged` key in old `config.toml` files is ignored.
- A discovered engine that arrives under a new id but the same endpoint as a stale entry (for example `DOCKER_HOST` unset, the same socket now found as `docker-local`) is dropped by the endpoint-clash filter, so it stays under the stale id and name. The filter compares against the entry's old endpoint, so a pinned engine whose endpoint has moved can also shadow another engine that now sits on its old one.
- *Clear default* leaves the engine's stored config in `engines.entries` (pinning stored it). If that engine later disappears from discovery it stays listed as unavailable and ranks as a manual engine in auto-select.
- The "Same daemon as" note appears only once an engine has connected in the current session (`/info.ID` is read on connect). It names peers by display name, so two engines with the same name are ambiguous, and it lists only peers that are enabled, not hidden, and listed.
- The *Default* tag and *Default (unavailable)* are decided by `default_mark` (unit-tested) and drawn in Settings and the switcher; the drawing itself is checked manually.
- ENG-100, ENG-108, and ENG-110 rendering, and the three acceptance items above, are verified only on real engines (release checklist).
