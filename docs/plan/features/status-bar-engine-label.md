# Plan: Status bar names the engine; WSLC route notes become rare and short

- **Slug:** `status-bar-engine-label`
- **Status:** approved
- **Spec:** [Engines](../../spec/features/engines.md) (ENG-108, ENG-110, ENG-136), [UI shell §1](../../spec/30-ui-shell.md), [Backends §5.4/§5.6](../../spec/20-engine-backends.md), [Engine contract §3.1](../../spec/21-engine-api-contract.md), [Settings](../../spec/features/settings.md) (SET-010/060 wording)
- **Milestone:** post-v1 UI polish (this checkout has no milestone README/table)
- **Requirement IDs:** ENG-108, ENG-110, ENG-136 (changed; no new IDs) · ENG-030, SHL-024, NFR-001 (preserved)
- **Created:** 2026-10-05 (revised the same day with the user's answers)
- **Approval:** the user approved implementation on 2026-10-05 ("continue in implementation", after answering the three design questions).

## 1. Goal

On a WSL containers (WSLC) engine in Auto mode the status bar ends with a cyan chip: "COM primary; Run uses CLI — native Run unverified". It explains a permanent, by-design difference in how the backend works. The user can't act on it, and it reads like a warning.

After this change:

- The status bar names the engine (*WSL containers 3.0.1*) and keeps one compact line of facts: version, API, OS/arch and transport. Clearer hierarchy, no coloured chips.
- Settings → Engines and Diagnostics keep the note chip, but a note appears only when the user should know something, and in a few words. A by-design difference gets no note.

## 2. Scope

**In:** the status bar's left segment (`render_status_bar`, `strings::status_bar_engine`); the WSLC router's `transport_note` text (`dk-engine-wslc/src/router.rs`, `info()`); the WSLC COM `api_version` string; tests; the spec text for ENG-108/110/136.

**Out:** routing behaviour, capabilities and hub propagation (ENG-136 still publishes the full `EngineInfo`); the Settings and Diagnostics rendering code (they already draw a note only when one is present); the title bar and the switcher; the CLI-only reasons produced by the factory (already short); native Run.

## 3. Assumptions & open questions

User answers, 2026-10-05: (1) the label is "WSL containers"; (2) "keep there some compact brief information but not too long and better design"; (3) the note "can stay in configuration but again just important short info".

| # | Assumption / decision | Default if unanswered |
|---|---|---|
| A1 | *(answer 1)* The engine label is `EngineKind::label()`: "WSL containers", the text the switcher rows already use. It applies to every kind through the label, with no per-kind branching (ENG-030): *Docker*, *WSL distro*, *WSL containers* | Yes |
| A2 | *(answer 2)* The status bar keeps version, API, OS/arch and transport, made compact with a clearer hierarchy (§6.2). No coloured chips; the state dot is the only colour | Yes |
| A3 | *(answer 3)* The note stays in Settings → Engines and Diagnostics, but only when notable and short (table below). The healthy Auto state, where Run goes through the CLI by design, has no note. This changes `dk-engine-wslc` output, not only the UI | Yes |
| A4 | WSLC COM `api_version` becomes the module name, `v3_0`, shown as "API v3_0". Today it is "COM ABI v3_0", shown as "API COM ABI v3_0", which repeats "COM" next to the `via com` tag. Shorter in the status bar, Settings, Diagnostics and the Add engine test result. No test or contract depends on the old string | Yes; veto and the old string stays |
| A5 | The note is not reachable from the status bar, not even on hover. Real connection trouble still shows through the state dot/label and the *Degraded* banner | Yes |
| A6 | Visual details (spacing, weight, divider height) are settled from light and dark screenshots during the UI task (spec 60 DoD 6), within the rules in §6.2 | Yes |

Notes, before → after (A3):

| Situation | Before | After |
|---|---|---|
| Auto, COM primary, Run through the CLI (the normal state) | `COM primary; Run uses CLI — native Run unverified` | *no note* |
| COM-only preference | `COM only; native Run unverified` | `COM only — Run unavailable` |
| Operations fell back to the CLI after a COM fault | `stats uses CLI — native transport fault` (one clause per operation, joined with "; ") | `CLI fallback: pull_image, stats` (sorted, deduplicated; at most three names, then `+N more`) |
| WSL version unverified or unknown, or COM self-check failed (CLI-only connection) | `WSL 3.1.0 not yet verified — using CLI`, `COM self-check failed — using CLI`, `WSL version unknown — using CLI` | unchanged |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-108 | The status bar MUST name the active engine by its kind label (for example *Docker*, *WSL containers*) and show, compactly, its version, API version, OS/arch and transport. It MUST NOT draw `transport_note` or any other coloured explanation of how a backend is implemented; the state dot is its only colour. | Changed |
| ENG-110 | WSLC hover tooltip and Diagnostics MUST show primary transport and WSL version. `transport_note` MUST be set only when the user should know something, in a few words, and be `None` otherwise: a connection using the CLI instead of COM, COM-only mode, or operations that fell back to the CLI after a fault. A by-design difference, such as Auto sending Run through the CLI, gets no note. It shows as the info chip in Settings → Engines and Diagnostics and MUST NOT show in the status bar. Do not label the whole engine CLI merely because Run uses CLI, or use a last-call transport label; metadata MUST refresh even with unchanged capabilities (ENG-136). | Changed |
| ENG-136 | Routing metadata MUST describe notable mixed/degraded routes (not Auto's by-design Run through the CLI) and propagate to registry, active store, status bar, Settings and Diagnostics even when capabilities do not change. The status bar refreshes its facts from the snapshot but never draws `transport_note`. | Changed |

## 5. Engine contract impact

No `Engine`, DTO, capability, hub-event or `EngineFactory` change. `EngineInfo.transport_note` keeps its type and meaning ("why a fallback transport is in use"); it is `None` more often. Only a value changes: `EngineInfo.api_version` for WSLC COM goes from `"COM ABI v3_0"` to `"v3_0"`. No backend mapping applies (no new operation).

| Reader of `transport_note` | After this plan |
|---|---|
| Status bar (`shell/app_shell.rs`) | draws version, API, OS/arch and transport only |
| Settings → Engines, Settings → Diagnostics, hub diagnostics text | code unchanged; they show the (now rarer, shorter) note |
| Switcher tooltip | unchanged |

## 6. Design

### 6.1 Data flow & threading

Unchanged. Info snapshots keep flowing as today (`StatusChanged`, local `InfoChanged`), and `render_status_bar` re-renders from the active store's info, so the facts follow metadata changes without reconnect or focus movement (ENG-136). No new tasks, revisions or blocking calls (NFR-001). The router builds the note inside `info()` from state it already holds (`strict`, the sticky routes).

### 6.2 UI

No new routes, controls or shortcuts. The `Tag::info` chip leaves the status bar and the left segment is restyled. Sketch (`│` is a thin divider):

```text
Before  ● Connected  Engine 3.0.1 · API COM ABI v3_0 · linux/amd64  [via com] [COM primary; Run uses CLI — native Run unverified]
After   ● Connected │ WSL containers 3.0.1 · API v3_0 · linux/amd64 │ [via com]                      CPUs 4 · RAM 8 GB
Docker  ● Connected │ Docker 27.3.1 · API 1.47 · linux/amd64 │ [via unix]                              CPUs 8 · RAM 16 GB
```

Rules:

- Left to right: state (dot + label) │ engine │ transport. Each `│` is a 1 px vertical divider in the theme border colour, about 12 px tall.
- Engine: the kind label (`info.kind.label()`) in the theme foreground colour, medium weight; version, API and OS/arch muted and joined with `·`, as today.
- Transport: the existing quiet `Tag::secondary().small()` with `via <transport>`, the same text as Settings and the switcher.
- No `Tag::info` or other coloured chip in the bar. The right side (CPUs/RAM, update item) is unchanged.
- Without `info` (connecting, failed, disconnected) the bar shows only the state, as today.
- Keyboard and focus are unchanged: the bar stays the *Status bar* region (KBD-005, `F6`) with no new controls. Theme tokens only, so light and dark both work. Strings live in `strings.rs` (SHL-024).

### 6.3 Errors

None new. Connection trouble is still shown by the state dot/label, the *Degraded* banner and the disconnected page.

## 7. Tasks

Tasks 1 and 2 are independent; 3 follows both.

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Router `info()`: note only when notable (§3 table); WSLC COM `api_version` = module name; fix the `// ENG-110` comments | engine-integrator | ENG-110, ENG-136 | `pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc`; new unit tests in §8 |
| 2 | Status bar: split the engine text into the emphasised kind label and a muted facts string (`strings.rs`); `render_status_bar` with dividers, no chip, `debug_selector`s `status-engine` / `status-transport` | gpui-ui | ENG-108, ENG-110, ENG-136 | clippy `-D warnings`, `test -p dockering`, `xtask check-blocking` |
| 3 | Tests in §8; update the live (ignored) expectations in `com_live.rs` and `repair_live.rs`; add a supersession line to `ROUTER-REPAIR-NOTES.md` and `tests/REPAIR-ACCEPTANCE-EVIDENCE.md` | qa-engineer | ENG-108, ENG-110, ENG-136 | named tests pass; live tests compile (`--no-run`) |
| 4 | Manual check on a WSLC engine in Auto mode: no cyan chip, "WSL containers 3.0.1" leads the bar, Settings and Diagnostics show no note. Light and dark screenshots for the PR | qa-engineer | ENG-108, ENG-110 | screenshots attached |
| 5 | Review | reviewer | all | verdict: approve (no kind branching ENG-030, strings via `strings.rs` SHL-024, no blocking NFR-001) |
| 6 | After merge: `/feature-planning complete status-bar-engine-label` (drop the "planned" markers, update the Verification tables, set this plan `done`) | architect | all | spec matches code |

## 8. Test plan

| ID | Layer | Test name(s) (proposed) |
|---|---|---|
| ENG-108 | unit (`strings.rs`) | `eng_108_status_bar_facts_format` (with and without an API version); `eng_108_status_bar_names_the_kind_not_the_note`: `StatusBarEngine::of` gives "WSL containers" / "Docker", the facts, `via com`, and nothing of a `transport_note` |
| ENG-110 | view (FakeEngine harness) | `eng_110_status_bar_draws_no_route_note`: the left segment ends at the transport tag, before and after a `StatusChanged` that carries a note. A re-added chip would widen the segment and fail it. `render_status_bar` builds its text from `StatusBarEngine`, which has no note field |
| ENG-110, ENG-136 | unit (router) | `eng_136_note_only_when_notable`: healthy Auto, also after a Run through the CLI → `None`; strict → "COM only — Run unavailable"; sticky `stats` + `pull_image` → "CLI fallback: pull_image, stats"; more than three → "…, +1 more" |
| ENG-110 | unit (COM, fake pipeline) | `eng_110_wslc_api_version_is_module_name` |
| ENG-136 | existing | `eng_136_*` hub and view tests stay green; their sample notes move to realistic short text |
| ENG-110 | live (ignored) | `com_live` and `repair_live` expect no note for Auto; compile-checked only. `live_factory_discover_connect_auto_uses_com` passed on 2026-10-05 against the real WSLC session (it asserts `transport_note == None`; an earlier attempt that day found no session running). The maintained `repair_live` harness creates and removes its own containers, so it is run only on request |
| ENG-108, ENG-110 | manual | task 4 |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Users lose a standing hint that Run goes through the CLI | It's by design and documented (spec 20 §5.3/§5.4, IMG-005), and Run works. A real fallback after a fault still shows in Settings and Diagnostics |
| "API v3_0" is less self-explanatory than "COM ABI v3_0" | The `via com` tag sits next to it, and Diagnostics keeps the transport. Veto A4 to keep the old string |
| The router change touches `dk-engine-wslc` (Windows-only COM code) | Output text only; routing, capabilities and `list_stats_limit` are untouched, and the router tests cover them |
| The restyle is subjective | A6: iterate on light and dark screenshots before review |
| "WSL distro 27.3.1" reads oddly for Docker inside a WSL distro (the version is Docker's) | Same text as the switcher row today, so not new. Reword separately if wanted |

## 10. Revision log

- 2026-10-05: created (draft, awaiting approval) after the user flagged the cyan note chip on a WSLC engine. Docs only.
- 2026-10-05: the user approved implementation ("continue in implementation"). Implemented the same day; review fixes (note-free `StatusBarEngine`, kit `Separator`, capped fallback note, spec mock-ups) applied. Verified on Windows: fmt, workspace clippy `-D warnings`, `xtask check-blocking`, full workspace tests (729 passed, 24 ignored, 0 failed), and a mutation check that re-adding a note chip fails `eng_110_status_bar_draws_no_route_note`. Manual check (task 4): the `--demo` build (fake *Docker* engine) in dark and light, then the real app on this machine's WSLC engine (Auto, COM, WSL 3.0.1) in dark. The bar reads `WSL containers 3.0.1 · API v3_0 · linux/amd64 │ via com` with no chip, and Settings → Engines and Diagnostics show no note. The light theme was not seen on WSLC. Live run on the real session: pulled `nginx:alpine`, ran a container with 18080→80 (host `curl` → HTTP 200), used its Terminal tab (gateway ping, DNS, outbound HTTP), stop, start, restart and delete from the UI, then removed the pulled image; the session is back to its starting state. Not exercised live: the notable-note cases (`COM only — Run unavailable`, `CLI fallback: …`), which are unit-tested only. Not committed. Open: `/feature-planning complete status-bar-engine-label`.
- 2026-10-05: revised with the user's answers: label "WSL containers"; the status bar keeps a compact facts line with better hierarchy and no chips; the Settings/Diagnostics note stays but only when notable and short, which adds an engine-integrator task (router note text, WSLC `api_version`). No contract change.
