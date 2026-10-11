# Plan: WSLC "no session" state with a Start action

- **Slug:** `wslc-no-session-start`
- **Status:** in-progress
- **Spec:** [Engines](../../spec/features/engines.md) (ENG-137…141 new; ENG-106/107 changed), [Backends §5.2/§5.8/§6](../../spec/20-engine-backends.md) (ENG-127 wording, spike S-4), [Contract §8](../../spec/21-engine-api-contract.md) (`StartPrompt`), [UI shell](../../spec/30-ui-shell.md) (SHL-013), [Keyboard](../../spec/features/keyboard.md) (KBD-075)
- **Milestone:** post-v1 polish (this checkout has no milestone table)
- **Requirement IDs:** ENG-137, ENG-138, ENG-139, ENG-140, ENG-141 (new) · ENG-106, ENG-107, ENG-127 (changed) · ENG-030, ENG-129, ENG-135, KBD-002, NFR-001 (preserved)
- **Created:** 2026-10-11

## 1. Goal

A WSLC session is the per-user environment containers live in (a utility VM running containerd and dockerd). The service creates the default session when a `wslc` command first needs it. Dockering only looks for one, so when none exists the profile shows a red failure page, `session 'default' not found`, and retries with backoff until something else creates a session.

Evidence: `dockering.2026-10-11.log` shows `engine connect failed engine=wslc-default error=session 'default' not found` at 08:16, 09:17 (five retries) and 12:14, and a clean connect at 12:50 with no change in the app.

After this change a missing default session is a normal, calm state: the page says what is missing, offers **Start WSL containers**, and re-checks quietly, so it connects by itself once a session appears.

## 2. Scope

**In:** detect the missing default session as `Stopped`; a data-driven `StartPrompt`; a CLI-based start of the default session on explicit user action; the full-page state, switcher row and Settings row; the quiet recheck; the action's palette entry and focus; tests; spec text.

**Out:** COM `CreateSession`/`EnterSession` (needs the unstable internal ABI and full session settings, and the stable SDK can't open the default session, spec 20 §5.1); creating non-default or custom-named sessions; stopping or terminating sessions; elevation or the admin session; VM/resource management (a [product non-goal](../../spec/00-product.md)); native Run.

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| A1 | Start uses the CLI: one fixed `wslc` argv, no `--session`. COM creation is out of scope (above), and the `wslc` CLI is what creates the user's default session today (`wslc-cli-pavel`) | Yes |
| A2 | Which command? Spike **S-4** decides. Candidates: `wslc info`, `wslc system info`, `wslc container list`. It must have no side effect other than creating the session | `wslc system info` ⚠ verify |
| A3 | Only the **default** session is startable. A missing explicitly named session stays `Failed` with its hint | Yes |
| A4 | No confirmation dialog: starting a session is non-destructive, and the button is the confirmation. Boot time is shown as a *Starting…* state | Yes |
| A5 | The recheck uses trusted-ABI `ListSessions` (read-only). With an unknown ABI or a CLI-only connection there is no recheck: the state stays until *Start* or *Rescan* | Yes |
| A6 | Does the session survive idle or reboot, and what does the CLI return when none exists? Unknown today | Spike S-4 |
| A7 | The shared `StartEngine` action is renamed *Start engine* in the palette (it served only WSL distros). The WSL distro button keeps its *Start & connect* label through `StartPrompt` | Yes |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-137 | Missing default session → `Stopped`, not `Failed`; quiet, no backoff | New |
| ENG-138 | *Start* creates the default session via one fixed CLI command, only on user action; never COM | New |
| ENG-139 | `StartPrompt { title, body, action_label }` from the factory; rendered by page, switcher and Settings | New |
| ENG-140 | Read-only recheck every 10 s active / 60 s otherwise; auto-connect when the session appears | New |
| ENG-141 | Palette entry *Start engine*, initial focus on the button, focus restored | New |
| ENG-106 | Generalised from WSL distros to any startable stopped backend | Changed |
| ENG-107 | A start-action condition isn't an error page | Changed |
| ENG-127 | The "no session creation" rule has one exception, ENG-138 | Changed |

## 5. Engine contract impact

```rust
// dk-core
pub struct StartPrompt { pub title: String, pub body: String, pub action_label: String }
pub struct EngineStatus { /* … */ #[serde(default)] pub start_prompt: Option<StartPrompt> }
trait EngineFactory { fn start_prompt(&self, cfg: &EngineConfig) -> Option<StartPrompt> { None } }
// EngineFactory::start already exists (ENG-106); WslcFactory implements it.
// HubHandle::start_wsl_distro is renamed start_engine.
```

No `Engine` trait change and no new `Capability`: this is a connection-level state, not an operation (Capabilities gate operations on a connected engine). The factory returning a prompt is the gate, so the UI never branches on `EngineKind` (ENG-030). The existing `ProbeResult::Stopped` is reused.

| Op | Docker | WSL distro | WSLC COM | WSLC CLI | Capability |
|---|---|---|---|---|---|
| Detect no session | n/a | n/a | `OpenSessionByName(NULL)` → `WSLC_E_SESSION_NOT_FOUND` (already mapped to `NotFound{Session}`) | `Session not found` / `WSLC_E_SESSION_NOT_FOUND` (already classified in `cli/runner.rs`) | none |
| Start | default `Err` | `wsl.exe -d <d> --exec true` (unchanged) | never | one fixed `wslc` command ⚠ S-4 | none (factory `start`) |
| Recheck | n/a | n/a | `ListSessions` (read-only) | none | none |

## 6. Design

### 6.1 Data flow & threading

- The supervisor and `autoselect` get `NotFound{kind: Session}` from `connect`. If the factory returns a `start_prompt(cfg)` for that engine and the target is the unnamed default, they set `EngineState::Stopped` instead of `Failed`. No new error variant. `probe` returns `ProbeResult::Stopped` when `ListSessions` shows no default.
- Start: `HubHandle::start_engine(id)` → `factory.start(cfg)` on the hub runtime (how ENG-106 works today, `hub.rs` `start_engine`): `Connecting` → the CLI through the existing `cli::runner` (argv vector, `CREATE_NO_WINDOW`, timeout, NFR-022) → `Disconnected` and wake the supervisor, or `Failed` with the message. The UI keeps the `HubCall` task on the shell (existing `hub_call`).
- The supervisor's wait branch for `Stopped` gains a timed wakeup (10 s for the active engine; the 60 s probe covers inactive ones, ENG-020) that re-runs the read-only check. The stale guard is the existing engine-id/connection revision check. Nothing runs on the UI thread (NFR-001).
- *Start* clicks while the state is `Connecting` are ignored.

### 6.2 UI

Reuses the full-page state in `shell/engine_views.rs`. For a stopped engine with a `StartPrompt` it draws an empty state, not `Alert::error`:

```
              [icon]
   WSL containers isn't running
   There's no active WSL containers session, so there
   are no containers or images to show yet. Start one to continue.

      [ Start WSL containers ]   [ Switch engine ]
```

States: *Stopped* (above), *Starting…* (button disabled with a spinner, state `Connecting`), *Failed* (the existing error page with the CLI message and a hint, plus *Retry* and *Start*), then data. The switcher row and Settings row show the same label (ENG-106). The wording names no COM, CLI or process (the project rule: no implementation notes in the UI). Keyboard: the button takes initial focus, `StartEngine` stays a palette command (*Start engine*, no chord, KBD-075), focus returns to the page through `ui::dispatch` (KBD-002), and no focus ring is drawn (KBD-003).

### 6.3 Errors

| Situation | State | Shown |
|---|---|---|
| Default session missing | `Stopped` | start prompt |
| Start CLI fails or times out | `Failed` | CLI message + hint, *Retry* |
| Named session missing | `Failed` | existing hint (ENG-129) |
| Policy / elevation / admin session | `Failed` | unchanged; never offers start (ENG-135) |
| `wslc.exe` missing and COM-only | `Failed` | start is offered only if the CLI exists |

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 0 | **Spike S-4**: on 3.0.1, terminate the session and record the CLI and COM results when none exists; find the safe start command; time it; check persistence and idle. Write the findings into spec 20 §5.8 | windows-platform | ENG-137/138/140 | notes + recorded fixtures |
| 1 | `StartPrompt`, `EngineStatus.start_prompt`, `EngineFactory::start_prompt`; hub: `NotFound{Session}` → `Stopped`, rename to `start_engine`, `Stopped` recheck timer | rust-core | ENG-106/137/139/140 | hub tests with `FakeEngine` and a fake factory |
| 2 | `WslcFactory`: `start_prompt`, `start` (fixed argv via `cli::runner`); `probe` and discovery mark the default `Stopped` | engine-integrator | ENG-137/138/140 | `fake_wslc` cases, no-session fixtures from S-4 |
| 3 | UI: empty state, button, switcher and Settings rows, *Starting…*, palette rename, focus | gpui-ui | ENG-139/141, SHL-013 | view tests + light/dark screenshots |
| 4 | Tests and ID→test table; opt-in live test that terminates and starts a session | qa-engineer | all | `cargo nextest`, `repair_live`-style ignored test |
| 5 | Review | reviewer | all | verdict: approve |

Task 0 gates tasks 2 and 4. Tasks 1 and 3 can start in parallel after approval.

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| ENG-137 | hub, wslc | `eng_137_missing_default_session_is_stopped_not_failed`, `eng_137_named_session_missing_stays_failed`, `eng_137_policy_and_elevation_never_become_stopped` |
| ENG-138 | wslc (`fake_wslc`), hub | `eng_138_start_runs_one_fixed_argv_and_connects`, `eng_138_start_failure_is_failed_with_hint`, `eng_138_never_started_by_connect_probe_or_retry` |
| ENG-139 | core, UI | `eng_139_start_prompt_has_no_transport_words`, `eng_139_stopped_page_is_not_an_error_alert` |
| ENG-140 | hub | `eng_140_stopped_wslc_connects_when_session_appears`, `eng_140_recheck_never_creates_a_session` |
| ENG-141 | UI | `eng_141_start_button_focus_and_palette_entry`, `a11y_every_action_bound_or_in_palette` (existing) |
| ENG-106/107 | hub, UI | existing `eng_106_*`, `eng_107_*` stay green |

## 9. Risks & spikes

| Risk | Mitigation |
|---|---|
| Unknown start command and session lifetime | Spike S-4 before tasks 2 and 4; the spec stays `⚠ verify` until it is done |
| Hiding a real fault behind "not running" | Only `WSLC_E_SESSION_NOT_FOUND` for the unnamed default maps to `Stopped`; everything else is unchanged and tested |
| Starting a session boots a VM (seconds, memory) | Explicit user action only, with a *Starting…* state; never from probes or retries |
| `EngineStatus` gains a field | `#[serde(default)]`, so old state files and snapshots stay compatible |
| Rename of `start_wsl_distro` touches callers | Mechanical and compiler-checked |

## 10. Revision log

- 2026-10-11: created after a WSLC connection-failure investigation (log evidence in §1).
- 2026-10-11: approved by the user ("Approved, implement this"), including the S-4 spike that terminates the live session.
