# Plan: Engine scan lifecycle, stable listing, and default engine

- **Slug:** `engine-scan-and-defaults`
- **Status:** done <!-- draft | approved | in-progress | done | abandoned -->
- **Spec:** [docs/spec/features/engines.md](../../spec/features/engines.md)
- **Milestone:** post-v1 (bug fix plus a small feature; this tree has no milestone table)
- **Requirement IDs:** ENG-113, ENG-114, ENG-115, ENG-116 (new) · ENG-103, ENG-009 (changed)
- **Created:** 2026-10-04

## 1. Goal

Engines never disappear from the list on their own. Discovery runs once at app start, then only when
the user asks (Rescan). Whatever the user decided per engine (name, enabled, hidden) survives rescans
and restarts, and the user can pin a **default engine** that the app opens on at startup.

## 1a. Root cause of "engines disappear after switching to WSLC and never come back"

Reproduced with a hub test (a `SameDaemon` fake, three engines) and confirmed on the dev machine.

1. On this machine **both Docker Desktop pipes reach the same daemon**: `\\.\pipe\docker_engine` and
   `\\.\pipe\dockerDesktopLinuxEngine` both answer `/info` with ID `4e151bca-…`. Discovery lists them as
   `context-desktop-linux` (rank 10) and `docker-desktop` (rank 20).
2. When the second of them connects, `Registry::dedupe_daemon` (`crates/dk-hub/src/registry.rs`)
   marks the **lower-ranked one** `merged_into = Some(..)` (ENG-009).
3. `Registry::visible()` hides a merged engine **unless it is the active one**. So it looks fine while
   active. The moment another engine becomes active (here: WSLC), `activate()` emits
   `HubEvent::Removed` for it, and the UI store drops it from the switcher and Settings.
4. `merged_into` lives only in memory and **nothing ever clears it**. `merge_discovered` (Rescan) never
   resets it, so a rescan can't bring the engine back. Only the *Un-merge* button (and only if
   the name was seen this session, `EngineListStore::seen_names`) or an app restart does.

Repro trace from the test: `start: [ctx, dd, wslc]` → `on dd: [ctx, dd, wslc]` → `after ->wslc: [ctx, wslc]`
→ `after rescan: [ctx, wslc]`.

So it isn't a scan bug. It's silent hiding by same-daemon de-duplication, which the user never asked for.

## 2. Scope

**In:**
- Stop hiding same-daemon engines (annotate them symmetrically instead). Remove *Un-merge* and
  `engines.unmerged`. Regression test for the bug above.
- Scan lifecycle: discovery at start and on explicit Rescan only (already true in code; make it a
  tested requirement). Rescan moves out of the switcher footer.
- Tests proving name/enabled/hidden overrides survive rescans and restarts, including for engines a
  rescan doesn't find.
- Pinned default engine: `engines.default`, startup order default → last-used → auto-select, Settings
  row control, command-palette entry.

**Out:** persisting the *scan results* themselves (a catalog shown before the first scan finishes);
changing probe/ping behaviour; engines per window; any new `Engine` trait method.

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | Same-daemon engines are annotated, never hidden (answered by the user, 2026-10-04). | — |
| 2 | "Default engine" means an explicit pin in Settings (answered by the user, 2026-10-04). | — |
| 3 | If the pinned default is contactable but down at startup, the app still activates it and shows *Failed / Reconnecting* with Retry (same as last-used today). It does NOT silently open a different engine. | as stated |
| 4 | A default that is disabled, hidden, unsupported, or no longer exists falls through to last-used, then auto-select. Its row says *Default (unavailable)*. | as stated |
| 5 | The switcher footer loses *Rescan* (only *Manage engines…* stays). Rescan stays in Settings → Engines, the command palette, and the first-run screen (ENG-111 needs it). | as stated |
| 6 | Old `config.toml` files with `engines.unmerged` load fine (unknown key ignored) and the key is dropped on the next save. | as stated |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-113 | Discovery once per app start and on explicit Rescan only; nothing else adds/removes/hides engines. | New |
| ENG-114 | Every discovered/manual engine stays listed all session; only Enabled/Hide/Show-all toggles suppress. Same-daemon engines are annotated. | New (replaces hiding half of ENG-009) |
| ENG-115 | Name/Enabled/Hidden persist immediately and are re-applied on every merge, including for unfound engines. | New |
| ENG-116 | Pinned default engine (`engines.default`), Settings control, palette entry, survives restart/rescan, cleared on remove. | New |
| ENG-103 | Startup order: default → last-used → auto-select. | Changed |
| ENG-009 | Annotate, don't hide; `Un-merge` removed. | Changed |

## 5. Engine contract impact

None. No `Engine`/`EngineFactory` change and no new capability. `EngineStatus.also_reachable_via`
keeps its name and type; its meaning becomes "names of other engines on the same daemon" and it is
filled **symmetrically** (both sides list each other).

## 6. Design

### 6.1 Data flow & threading (`dk-hub`, hub runtime only; no UI-thread work)

- `registry.rs`: delete `Entry.merged_into`, `View.unmerged`, and the merged branch in `visible()`.
  `dedupe_daemon(id, daemon_id)` records `daemon_id` and recomputes `also_reachable_via` for **every**
  entry sharing it (both directions), emitting `StatusChanged` for each entry that changed. It never emits
  `Removed`. `activate()`'s `Some(_) => Removed` arm can only be hit by `show_only_when_all` now; keep it.
- `config.rs`: drop `EngineSettings.unmerged`; add `default: Option<EngineId>`. `update_config`'s
  view-diff drops the `unmerged` comparison.
- `supervisor.rs`: `bootstrap` and `rescan` both call `select_engine`, which uses `startup_engine(inner)`:
  `engines.default`, then `ui_state.last_engine`, each required to exist, be enabled, not hidden, and
  `contactable()`; else `autoselect`. `hub.rs::remove_engine` clears `engines.default` (and `last_engine`)
  when the removed id matches.
- `discover()` / `merge_discovered()` are unchanged in behaviour. Step 3 ("vanished engines stay listed as
  Disconnected") already keeps unfound stored engines. Tests lock this in (ENG-115).
- Config writes keep the existing 500 ms debounced atomic save; a `config()` update is the only
  write path for `default`, so no new hub call is needed. UI uses `AppState::update_config`.
- Stale-result guard: unchanged (`EngineListStore.revision`).

### 6.2 UI (`dockering`)

- **Settings → Engines row:** a *Default* tag (or *Default (unavailable)* tag), a *Set as default* /
  *Clear default* button (`EngineOpKind::SetDefault` / `ClearDefault`, dispatched like the other row
  ops), and a "Same daemon as: …" line replacing the *Un-merge* buttons. Rescan stays on top.
- Remove `EngineOpKind::Unmerge`, `EngineListStore::seen_names` / `merged_into`, and the *Un-merge*
  strings.
- **Switcher:** footer drops *Rescan*; the tooltip shows "Same daemon as: …"; a *Default* marker on the
  default's row.
- **Command palette:** *Make active engine the default* (`SetActiveEngineDefault` action). *Rescan engines*
  stays. No default chord (KBD-075).
- Keyboard: every new control is a Tab stop in visual order (the Settings Tab-walk test counts the new
  per-row button). *Set as default* / *Clear default* keep focus on the row's button because the row is
  re-rendered in place; there is no test for that. No focus rings.
- Four states: the default row follows the existing row states; no new loading state.

### 6.3 Errors

No new `EngineError`. A failed default connects like any failed engine (ENG-107 hint). Config write
errors are already logged by `flush_now`.

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Add the failing regression test `eng_114_same_daemon_engines_stay_listed_after_switch_and_rescan` (ctx + dd sharing a daemon + wslc; switch around; `rescan()`; all three listed) | qa-engineer | ENG-114 | test fails on `main`, passes after task 2 |
| 2 | Registry: remove `merged_into`/`unmerged`; symmetric `also_reachable_via`; drop `Removed` on dedupe; remove `EngineSettings.unmerged`; update `eng_009_*` tests and the `registry.rs` unit test | rust-core | ENG-009, ENG-114 | `cargo nextest run -p dk-hub` |
| 3 | `EngineSettings.default`; `startup_candidate` in bootstrap; `remove_engine` clears it; tests `eng_103_default_engine_connects_first`, `eng_103_unavailable_default_falls_back`, `eng_116_runtime_switch_keeps_default_and_remove_clears_it` | rust-core | ENG-103, ENG-116 | hub tests |
| 4 | Lock in lifecycle and overrides: `eng_113_no_discovery_after_start_until_rescan` (count `discover()` calls across switch/disconnect/probe ticks), `eng_115_disabled_hidden_name_survive_rescan`, `eng_115_overrides_of_unfound_engines_are_kept`, `eng_115_overrides_survive_restart` (save, start a second hub on the same `Paths`). Fix anything these find. | qa-engineer → rust-core | ENG-113, ENG-115 | hub tests |
| 5 | Settings row (Default tag, Set/Clear default, same-daemon line), remove *Un-merge*, switcher footer/tooltip/marker, palette entries, strings, action wiring | gpui-ui | ENG-009, ENG-114, ENG-116, KBD-075 | view tests |
| 6 | View tests `eng_116_*` (button persists the key, tag shows, unavailable tag), update `view_tests_settings.rs` (`eng_009_unmerge_adds_id_to_config` removed), keyboard Tab-walk test still green | qa-engineer | ENG-116, KBD-075 | `cargo nextest run --workspace` |
| 7 | Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, full tests; check the grep rules for the UI thread | qa-engineer | all | CI-equivalent green |
| 8 | Review | reviewer | all | verdict: approve |
| 9 | `/feature-planning complete engine-scan-and-defaults` | architect | all | spec reconciled, status `implemented` |

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| ENG-114 | hub (paused clock, `SameDaemon` fake) | `eng_114_same_daemon_engines_stay_listed_after_switch_and_rescan`, `eng_114_same_daemon_note_is_symmetric` |
| ENG-009 | hub + registry unit | `eng_009_same_daemon_is_annotated_not_hidden` (replaces `eng_009_dedupe_by_daemon_id` / `eng_009_daemon_dedupe_keeps_lower_preference`); `eng_009_dedupe_by_canonical_endpoint` stays |
| ENG-113 | hub | `eng_113_no_discovery_after_start_until_rescan` |
| ENG-115 | hub + config | `eng_115_disabled_hidden_name_survive_rescan`, `eng_115_overrides_of_unfound_engines_are_kept`, `eng_115_overrides_survive_restart` |
| ENG-116 | hub | `eng_116_runtime_switch_keeps_default_and_remove_clears_it`, `eng_116_removing_a_discovered_default_keeps_the_pin`, `eng_116_hidden_default_falls_through`, `eng_116_default_wins_even_when_the_startup_scan_misses_it` |
| ENG-103 | hub | `eng_103_default_engine_connects_first`, `eng_103_unavailable_default_falls_back` (+ existing two) |
| ENG-116, KBD-075 | GPUI view + unit | `eng_116_set_and_clear_default_persist`, `eng_116_palette_command_pins_active_engine`, `eng_116_palette_command_refuses_a_disabled_engine`, `eng_116_refresh_config_follows_a_default_changed_by_the_hub`, `eng_116_default_mark_follows_what_the_hub_would_open`, Tab-walk test |
| Config compat | config | an old file with `engines.unmerged = ["x"]` loads and drops the key on save |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Two entries for one Docker Desktop daemon look redundant in the switcher | The "Same daemon as" note; users can *Hide* one (ENG-115 persists it). Not a spike. |
| ENG-115 may already hold, or may hide a real gap (for example `hidden` on an unfound engine, or `Disabled` state after a merge) | Task 4 writes the tests first and fixes what they find; the plan assumes, but doesn't claim, that they pass. |
| A pinned default that is down at startup leaves the user on a Failed screen | Assumption 3: same as today's last-used behaviour, with Retry and the switcher one keystroke away. Revisit if it annoys. |
| `also_reachable_via` now lists on both sides, and UI code assumed one side | Task 5 greps every use (`switcher.rs`, `engines.rs`); view tests cover both rows. |

## 10. Revision log

- 2026-10-04: created. Root cause reproduced; same-daemon handling and default-engine semantics chosen by the user.
- 2026-10-04: third pass (second review): pinning now stores the engine's config (`AppState::pin_default`) so a slow startup scan can't lose the default; a rescan never hides an already-listed engine (`show_only_when_all` is sticky); failed config saves stay dirty and are retried; notes refresh when the old active engine is dropped from the list. Known and left as is: a discovered engine that arrives under a new id but the same endpoint as a stale entry is dropped by the endpoint-clash filter (it stays under the stale id).
- 2026-10-04: implemented. Review follow-ups folded in: the same-daemon note is recomputed on rename, enable/disable, endpoint change, and *Show all* toggles, and lists only enabled, listed peers; the palette entry refuses engines that can't be the default; `AppState::refresh_config` re-reads the hub's config after add/remove so a reused id doesn't inherit a stale *Default* tag. ENG-103 text now states that hidden candidates are skipped and a Stopped distro is still opened. Second review pass: *Rescan* with no active engine now applies the startup order (`supervisor::select_engine`) instead of plain auto-select; hidden peers are left out of the same-daemon note; the tag decision moved to a shared, unit-tested `default_mark`; the Settings Tab-walk bound counts the new per-row button. The tag drawing itself is not render-tested (Known gaps).
