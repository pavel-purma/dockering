# Plan: WSLC existing integration repair — COM-first operation fallback

- **Slug:** `wslc-integration-repair`
- **Status:** done (core repair completed 2026-10-04; cross-OS release validation remains open)
- **Spec:** [Engines](../../spec/features/engines.md), [Images](../../spec/features/images.md), [Container detail](../../spec/features/container-detail.md), [Backends §5](../../spec/20-engine-backends.md), [Engine contract](../../spec/21-engine-api-contract.md)
- **Milestone:** post-v1 repair (this checkout has no milestone README/table)
- **Requirement IDs:** ENG-126…136 (new); ENG-013, ENG-109, ENG-110, IMG-005, CDT-030, CDT-040 (changed/clarified); ENG-114, ENG-022, ENG-030…033, NFR-001…005, NFR-020, NFR-022 (preserved)
- **Created:** 2026-10-04
- **Approval:** user explicitly approved implementation on 2026-10-04.
- **Completion:** current production code/tests inspected; parent reports final reviewer **approve**, no remaining production blockers. This completion pass changes documentation only, runs no tests and makes no commit.

## Completion record — 2026-10-04 (authoritative current evidence)

This record supersedes the historical baseline below and intermediate COM/router/QA handoff counters. Requirements retain their IDs. T1–T14 are implemented; T15 Windows acceptance/fault coverage and T16 final review are complete. Cross-OS portions of T15 move to the explicit release gate below; native CreateContainer is a separate deferred spike, not unfinished repair production work.

### Final validation

| Check | Final result / provenance |
|---|---|
| Windows `pwsh -NoProfile scripts/dev.ps1 test --workspace` | **PASS**. Completion read full parent output `C:/Users/pavel/.local/share/opencode/tool-output/tool_10886253800114mDMBqVZmY48a`; all test groups/doc-tests succeeded. |
| WSLC subset of workspace run | **183 passed, 0 failed, 18 ignored**: 141 unit + 18 CLI + 15 COM + 1 allocation + 7 repair-contract + 1 repair-live parser. Ignores are live opt-ins (1 unit binding spike, 6 cli_live, 10 com_live, 1 repair_live), not known-defect ignores. |
| Hub / UI subset | **77 hub / 243 UI passed**, zero failures. Includes coherent full-info and stale-connection tests, focus/page preservation, exited Run detail, 501/unknown-outcome no-resubmission tests. |
| Workspace clippy all targets `-D warnings`, fmt check, `xtask check-blocking` | **PASS**, final parent-reported results; not rerun in this docs-only pass. |
| Expanded maintained live harness | **1 passed in 20.03 s**, parent-reported final execution of exact command below. Four Runs, not the superseded three-Run harness. Normal-user FileVersion **3.0.1.0**, default **wslc-cli-pavel**, security init passed, self-check 2 sessions. |
| Reviewer | Final **approve**, no remaining production blockers (parent report). |
| Linux / macOS | **Not green / not validated**: Linux cross-check blocked building ring because `x86_64-linux-gnu-gcc` is absent; macOS not run. Follow-up release gate, not a production-code defect. Do not claim three-OS CI or full quality DoD completion. |

Exact maintained live command (outer timeout at least 240 s):

```powershell
pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test repair_live -- --ignored --exact eng_127_128_normal_user_hello_world_acceptance --nocapture
```

| Final Run path | Returned full ID | Verification |
|---|---|---|
| Default / Auto | `f869625df44e9b1431dae367c69c2b8ad7f1d8accfe4caf2303416221e223fc8` | Both typed COM/CLI inspect Exited, exit 0, `/hello`; both logs Hello from Docker |
| Explicit wslc-cli-pavel / Auto | `e90f165bcaffff5a1729de106f54796f8c23cf9a462c3d9f03829f07cdd1187e` | Same |
| Explicit wslc-cli-pavel / CLI | `f89e1a375afcf1e5a48cfc9e1de3a67c603e76e4689cb5edd38e7f0079b44601` | Same |
| Independent raw CLI / pinned wslc-cli-pavel | `76ce4012bfb53784d683be3bede1a124d695674f48a0c002abf181a5714d5891` | Same; bypasses Factory/RunSpec, exactly one full stdout ID |

Six default/explicit × Auto/COM/CLI preference paths passed ping/info; strict COM Run remains 501. Zero CLI preparation/dispatch for strict COM is established by production decision-closure/fake counter tests, not an OS-wide spawn monitor. Admin session rejection was checked for all three preferences without elevation or mutation. Four own resources were positively identified and removed; zero own containers remain. Unrelated baseline count was zero (a nonempty live baseline preservation exercise is **not** claimed); hello-world image retained. Auto stays COM-primary with mixed Run note. Native raw deep equality and typed port equality pass; hello-world has no published ports, whose coverage is fixture/fake evidence.

### As-built reconciliation and limitations

- Factory returns private `router::WslcEngine` for both native and CLI-only connections. No Engine/TerminalSession/EngineFactory signatures or public error/event types changed. Internal `com::dispatch::TransportFailure` holds original HRESULT and phase; `ConnectFailure` distinguishes policy/self-check mismatch/final failures.
- Exact module allowlist is **3.0.1.0 only**. Service-binary FileVersion must be readable to authorize activation; MSI fallback is presence evidence only. Version checks run before activation, on the MTA activation worker and before target revalidation/reopen.
- Mixed targeting binds exact name plus runtime u32 session ID, creator PID and SID, validating one matching row and caller ownership. Revalidation occurs at crossing and before CLI spawn **after permit waits**, on every stats poll, and inside queued exec spawn after inspect. Missing identity preserves the opened COM proxy but prohibits crossing/reopen. Explicit target contradiction is final, not fallback. **Residual race:** CLI has no atomic identity-check-and-dispatch API; replacement after last validation but before spawn cannot be excluded. No durable UUID/atomic guarantee is claimed.
- Read retries are bounded and classified; mutations never replay after possible dispatch. Actual automatic reconciliation is conservative: start/stop full ID and primary remove-container (without ancillary volumes), volume/network absence; successful native volume creation recovers inspect by its returned name. Restart/kill/tag/ambiguous creates, pulls, exec and lost reports/Run IDs stay unknown rather than claiming all suggested postconditions can be proved. Reconciliation deadline is 5 s.
- Mixed `prune_volumes` fallback is **disabled** (COM all-unused vs CLI anonymous-only). Mixed create-volume fallback with explicit `driver = local` is refused. Supplied pull auth is rejected by CLI before any spawn; no silent credential discard. CLI-only existing prune/driver semantics remain explicit.
- Stream source activity is tracked before filtering; errors after activity terminate, never splice. Old native producers (including sibling logs readers) must drain before replacement or subsequent sticky CLI subscription; 5 s drain failure prevents replacement. Events-lost still reports/refetches a gap. Terminal objects remain pinned; exec GetStdHandle/reader startup failure cannot re-exec.
- Checked tokens, atomic queued admission, MTA startup reporting, tagged socket/kernel owners, alias deduplication and overlapped cancellation/completion are implemented. Contradictory/unknown handle tags are rejected/quarantined because no safe destructor can be inferred; sustained live socket leak/marshalling stress is not claimed. ConPTY lifecycle tests cover flood-before-startup, dropped sessions and stalled writers with responsive hub teardown.
- Full info refresh is bounded outside registry locks, current-connection guarded, preserving prior snapshot on info-only failure. Existing StatusChanged carries full info even with unchanged caps; local EngineListEvent::InfoChanged/EngineStore apply_info updates UI without remount/focus loss. No public InfoChanged event was added.
- Native CreateContainer remains unverified and disabled. Strict COM 501 and Auto CLI are intended completed behavior; optional S1 enablement requires a separately approved version-specific live spike.

### Release validation gate (not a production implementation blocker)

| Follow-up | Owner | IDs | Verification / limit |
|---|---|---|---|
| RG1: Linux build/test/clippy with required compiler/toolchain installed | release-engineer | ENG-126…136, NFR-001…005 | Provision `x86_64-linux-gnu-gcc` for ring or use native Linux CI; attach successful results. Current blocked cross-check is not a pass. ≤1d validation task. |
| RG2: macOS build/test/clippy | release-engineer | ENG-126…136, NFR-001…005 | Run native macOS CI; attach successful results. Currently not run. ≤1d validation task. |
| RG3: release evidence reconciliation | qa-engineer | ENG-126…136, IMG-005, CDT-030/040 | Check RG1/RG2 and quality DoD before claiming three-OS green/release-ready. Record actual failures as follow-ups, not fictitious completion. ≤0.5d. |

### Evidence-note supersession

The existing `crates/dk-engine-wslc/src/com/REPAIR-EVIDENCE.md`, `ROUTER-REPAIR-NOTES.md` and `tests/REPAIR-ACCEPTANCE-EVIDENCE.md` describe intermediate owner handoffs. **Their earlier 156-pass counters, unrun expanded harness/compile blocker, pending hub/UI work and unrerun admin isolation statements are superseded by this completion record.** Historical three-Run IDs and 17.83 s remain history, not final acceptance. Detailed mechanism/stress limits in those notes still apply. This architect completion edits only docs/spec and docs/plan; it does not modify evidence files under crates, production code, or commits.

## 1. Goal

Repair the existing WSLC integration so Auto manages the caller's existing session through trusted COM first, with per-operation CLI fallback only when safe. In particular, Run must work through the already-proven CLI path while native Run remains unverified. Preserve exact session targeting, prevent duplicate mutations, repair inspect fidelity and resource lifetime defects, and make mixed/degraded routing visible without changing the public Engine trait or adding UI transport branches.

### 1.1 Evidence ledger — historical pre-repair baseline, 2026-10-04

This ledger records supplied live results and the repository audit, not tests rerun while writing these documents. Prioritize the observed disabled Run path over hypothetical runtime failures.

| Priority / evidence type | Observation | What it establishes / does not establish |
|---|---|---|
| P0 — live environment | Non-elevated process; `wslservice.exe` FileVersion **3.0.1.0**, verified with PowerShell; caller default **`wslc-cli-pavel`** | Exact initial version candidate and normal-user session; no elevation workaround is needed for this reproduction |
| P0 — live COM connect | `init_process_com_security()` returned **true**; self-check passed with **2 sessions**; Auto factory selected COM | Bootstrap/session open work in this environment, not proof of every internal operation |
| P0 — direct COM Run | `WslcComEngine::run_image` returned `Api { status: 501 }`, "Run via COM is not verified for this WSL version" | An intentional disabled path, not an observed failing `CreateContainer` RPC |
| P0 — raw CLI Run | `hello-world:latest` succeeded; ID `efdf10b2ffe6546627647c00c8880122f7b5968192971789904915d31dcd585d` | CLI Run works in the caller's existing session |
| P0 — typed CLI Run | `WslcCliEngine::run_image` succeeded; ID `fb435025c3c0b32d8c070a9729a70875cf8392c43c132269ea07a5fb26c9241c` | Existing RunSpec/CLI adapter can execute the simple test |
| P0 — typed inspect/logs | Both containers inspected through typed COM and CLI as **Exited**, exit code **0**, command **`/hello`**; logs contained **Hello from Docker** | Cross-transport visibility and simple DTO/log baseline; hello-world does not prove published ports, all RunSpec options, or native creation |
| P0 — cleanup | COM cleanup removed the typed CLI probe; CLI cleanup removed the raw CLI probe; container list now empty; hello-world image retained | No probe container remains; do not remove the retained image or unrelated resources in acceptance tests |
| Baseline automated tests | `pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc`: **114 passed** (82 unit, 18 CLI, 13 COM, 1 allocation); **16 ignored**. Separately the live factory test: **1 passed** | Existing suite baseline, not coverage of the new router or all live operations |
| Probe provenance | Temporary external probe outside the repository used existing `test-support`; no production changes | Not a maintained/reproducible repository harness; task T14 adds one |

Native `WSLCContainerOptions` appears to match the vendored IDL on inspection, but **CreateContainer was not exercised live**. Do not describe the layout as known bad or fully tested. This plan leaves native Run disabled.

### 1.2 Historical pre-repair repository audit — defects now repaired

| Source | Finding | Planned repair |
|---|---|---|
| `crates/dk-engine-wslc/src/factory.rs:384–443`; `src/lib.rs` | Factory returns COM or CLI Engine directly; no composite router. COM Run comment implies routing that does not exist | Private router returned by the factory; explicit predispatch Run exception |
| `src/com/engine.rs:754–756`; `src/cli/mod.rs:365–375,468–477` | COM typed inspect does not adapt top-level `Ports`; CLI does, but maps from an altered JSON value | Shared WSLC normalization for typed fields; preserve original `raw` |
| `src/com/engine.rs:142–220` | Generic `with_session` retries mutations after disconnect | Dispatch-aware calls and bounded read-only reconciliation |
| `src/com/engine.rs:174–187` | `begin_op` failure ignored | Required checked token before protected RPC |
| `src/com/win32.rs:226–255`; logs/exec consumers | Socket outputs owned as `CloseHandle` handles | Tagged socket/kernel ownership and audited cleanup |
| `src/com/pool.rs:75–80` | A canceled queued job still executes | Cancellation/admission check before dispatch |
| `src/factory.rs:355–380` | Missing SID accepted; enumeration errors including policy can fall through to CLI | Ownership fail-closed; policy final |
| `src/com/abi/mod.rs:35–39` | ABI trust covers all future `3.0.x` builds | Explicit exact-version allowlist |
| `crates/dk-hub/src/supervisor.rs:379–400` | Only capability flags refreshed | Full EngineInfo propagation even for unchanged caps |

**Runtime hypotheses:** disconnects causing duplicate mutation, socket leaks/cancellation races, default-session rebinding, and unproven future ABI compatibility are risks to inject/test, not claimed causes of the observed Run 501. The existing enabled COM operations are not presumed broken.

## 2. Scope

**In:** private WSLC router; Auto/forced preference semantics; exact session binding; read fallback; mutation outcome safety; stream boundaries; exact-version ABI trust; inspect ports/raw parity; RPC cancellation and operation tokens; tagged socket cleanup; fail-closed discovery; full metadata refresh; maintained hello-world acceptance harness.

**Out:** engine installation/VM management ([product non-goals](../../spec/00-product.md#non-goals-v1)); automatic elevation; creating/replacing sessions; Docker backend changes; new Engine methods, public transport types or capabilities; new UI controls; speculative native CreateContainer enablement; general retry framework for other runtimes.

## 3. Approved assumptions (resolved behavior/limits in completion record)

| # | Assumption / question | Default if unanswered / gate |
|---|---|---|
| A1 | Preference COM means strict COM-only | No CLI construction, probe, spawn or fallback for that configured connection; unverified Run retains descriptive 501 |
| A2 | Initial trusted service-file version | Exactly `3.0.1.0` → existing `v3_0`; unknown patch/build uses Auto CLI, forced COM refuses before internal activation |
| A3 | Resolved display name and available session identity | T1 complete: exact name/runtime ID/PID/SID checks; no mixed dispatch when binding is unproven, no USERNAME synthesis; unavoidable check-to-spawn race documented |
| A4 | CLI unavailable | COM connect/verified operations remain usable. Only an operation needing CLI fails with an actionable hint |
| A5 | Mutation completion/report cannot be reconstructed | Existing `Unreachable { reason, hint }` explains unknown outcome and refresh-before-retry; no new public error variant/type |
| A6 | Existing option/auth differences | Preserve documented semantics; gate fallback where equivalence is unproven. Do not silently drop supplied auth or weaken options |
| A7 | ADR/milestone files are absent in this checkout | Do not create missing ADR or whole milestone README. Record decisions here; any later ADR needs actual index/numbering and separate approval |
| A8 | Metadata timing | Refresh full info within the next active health cycle, bounded by a timeout; do not introduce a public backend metadata event merely for immediacy |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-126 | WSLC MUST expose one private router through EngineFactory. Auto MUST prefer trusted COM per operation; forced COM and CLI MUST prohibit the other transport for that connection. | New |
| ENG-127 | Routes, reopen attempts and reconciliation reads MUST preserve the same resolved session and caller security context. Failure to establish equivalent targeting MUST prohibit cross-transport dispatch. | New |
| ENG-128 | Auto MUST select CLI before dispatch for explicitly unverified native operations, initially run_image. It MUST NOT execute an unverified native call or catch arbitrary 501 errors to discover that condition. | New |
| ENG-129 | Read fallback MUST use an explicit transport-fault allowlist and bounded attempts. Domain, policy, authorization, validation, cancellation and payload/protocol errors MUST NOT trigger fallback. | New |
| ENG-130 | Possibly dispatched mutations MUST NOT be replayed automatically. The router MUST perform bounded read-only reconciliation where meaningful and otherwise report an unknown outcome with refresh-before-retry guidance. | New |
| ENG-131 | Read-stream fallback MUST occur only before the first source item. After emission, a transport fault MUST terminate the subscription without source splicing. Pull and exec MUST additionally obey mutation dispatch safety. | New |
| ENG-132 | Internal ABI calls MUST require an explicit trusted exact version selected before activation. Self-check MUST confirm, not establish, trust. Native operation enablement MUST require operation-specific evidence. | New |
| ENG-133 | COM and CLI inspect conversion MUST preserve equivalent typed ports and other shared fields and MUST preserve original inspect JSON in raw. | New |
| ENG-134 | Canceled queued RPCs MUST NOT dispatch. Required operation-token failure MUST prevent the protected call. Resources MUST have handle-type-correct ownership/cancellation without blocking teardown on UI/hub workers. | New |
| ENG-135 | Extra-session discovery MUST fail closed for unproven ownership. Policy errors MUST prohibit CLI enumeration/fallback. CLI table rows lacking ownership evidence MUST NOT become extra discovered engines. | New |
| ENG-136 | Routing metadata MUST describe mixed/degraded routes and propagate to registry, active store, status bar, Settings and Diagnostics even when capabilities do not change. | New |
| ENG-013 | Auto is COM-first per operation; forced COM/CLI are strict; ABI selection uses an exact-version allowlist and an operation-specific trust gate. | Changed in spec 20 §5.3 |
| ENG-109 | Default session remains listed; additional sessions require proven matching owner SID. CLI listing alone cannot prove ownership. | Changed |
| ENG-110 | Existing transport/note UI MUST describe primary/mixed/degraded operation routing and version without misleading last-call transport labels. | Changed |
| IMG-005 | Auto Run MUST use CLI in the same session while native Run is unverified; success navigates to returned ID. Forced COM reports 501 without CLI execution; unknown outcome never triggers automatic resubmission. | Changed |
| CDT-030 | WSLC top-level Ports MUST populate typed summary ports and port bindings through both transports. | Clarified |
| CDT-040 | Inspect raw MUST remain the original parsed backend JSON; typed normalization MUST NOT alter it. | Clarified |

All repair requirements below are implemented; final verification is in §8 and the completion record. The historical baseline alone does not prove these additions.

## 5. Engine contract impact

```rust
// No Engine, TerminalSession or EngineFactory signature changes.
async fn run_image(&self, spec: RunSpec) -> EngineResult<String>;
async fn info(&self) -> EngineResult<EngineInfo>;
fn capabilities(&self) -> Capabilities;

// Existing code field, now explicitly documented in spec 21:
pub transport_note: Option<String>;

// Existing hub notification already carries full EngineInfo:
HubEvent::StatusChanged(EngineStatus)

// As-built crate-private classification only; not a dk-core API:
enum DispatchPhase { NotDispatched, MayHaveDispatched, Completed }
struct TransportFailure {
    hresult: i32,
    phase: DispatchPhase,
}
```

No new operation or capability. UI/hub see only `Arc<dyn Engine>`; runtime differences use existing capabilities/optional DTO metadata (ENG-030…033). Internal classification must retain original HRESULT and dispatch phase before conversion to public errors, not recover them from strings/hints.

### 5.1 Complete trait mapping and routing groups

Policies: **R** = bounded read fallback; **M** = mutation safety; **S** = read-stream first-item boundary; **U** = unsupported. WSL distro uses the same Docker mapping over its existing bridge/TCP transport. Endpoint paths are relative to the Engine API prefix.

| Op | Docker (API / bollard) | WSL distro (Docker via bridge) | WSLC COM (`IWSLC*`) | WSLC CLI fallback (`wslc …`) | Policy / Capability |
|---|---|---|---|---|---|
| id, kind, capabilities | Local identity/profile | Same | Router identity/profile | Router identity/profile | No I/O; existing flags |
| ping, info | GET /_ping, /info, /version → ping/info/version | Same | GetState/GetVersion + existing count queries | version/system info + session-bound resource query for health | R; executable version alone is not session health |
| list_containers, inspect_container | GET /containers/json, /containers/{id}/json → list_containers/inspect_container | Same | ListContainers; OpenContainer→Inspect | container list/inspect | R |
| container_action: start/stop/restart/kill | POST /containers/{id}/start/stop/restart/kill → start_container/stop_container/restart_container/kill_container | Same | Start/Stop/Restart/Kill | container start/stop/restart/kill | M |
| container_action: pause/unpause | POST /containers/{id}/pause/unpause → pause_container/unpause_container | Same | — | — | U(PAUSE) on WSLC |
| remove_container, prune_containers | DELETE /containers/{id}, POST /containers/prune → remove_container/prune_containers | Same | Delete/PruneContainers | container remove/prune --force | M |
| logs, stats, events | GET /containers/{id}/logs, /stats, /events → logs/stats/events | Same | Logs; polled Stats; GetEvents/GetNext | logs child; stats polls; system events child | S; LOGS_FOLLOW/EVENTS; WSLC no native STATS_STREAM |
| top | GET /containers/{id}/top → top_processes | Same | — | — | U(TOP) on WSLC |
| exec | POST /containers/{id}/exec, /exec/{id}/start → create_exec/start_exec | Same | Exec→IWSLCProcess | container exec via ConPTY | M; EXEC_TTY/EXEC_RESIZE |
| list_images, inspect_image | GET /images/json, /images/{id}/json → list_images/inspect_image | Same | ListImages/InspectImage | image list/inspect | R |
| image_history | GET /images/{id}/history → image_history | Same | — | — | U(IMAGE_HISTORY) on WSLC |
| pull_image | POST /images/create → create_image | Same | PullImage + callbacks | image pull | M + emission boundary; PULL_PROGRESS only for native structured route |
| remove_image, prune_images, tag_image | DELETE /images/{id}, POST /images/prune, /images/{id}/tag → remove_image/prune_images/tag_image | Same | DeleteImage/PruneImages/TagImage | image remove/prune/tag | M |
| run_image | POST /containers/create + start → create_container/start_container | Same | CreateContainer+Start declared, not live verified; NOT called | container run --detach … | Auto predispatch CLI; forced COM 501 |
| list_volumes, inspect_volume | GET /volumes, /volumes/{n} → list_volumes/inspect_volume | Same | ListVolumes/InspectVolume + used-by reads | volume list/inspect + enrichment | R |
| create_volume, remove_volume, prune_volumes | POST /volumes/create, DELETE /volumes/{n}, POST /volumes/prune → create_volume/remove_volume/prune_volumes | Same | CreateVolume/DeleteVolume/PruneVolumes | volume create/remove/prune | M; parity gate for prune semantics |
| disk_usage | GET /system/df → df (API ≥ 1.52) | Same | — | — | U(DISK_USAGE) on WSLC |
| list_networks, inspect_network | GET /networks, /networks/{id} → list_networks/inspect_network | Same | ListNetworks/InspectNetwork | network list/inspect | R |
| remove_network, prune_networks | DELETE /networks/{id}, POST /networks/prune → remove_network/prune_networks | Same | DeleteNetwork/PruneNetworks | resolve name then network remove; network prune | M; NETWORK_MGMT |

`TerminalSession` is pinned to its creating transport: output = native socket reader / Docker attached output / CLI PTY; write = native socket / Docker attached input / PTY input; resize = ResizeTty / resize_exec / MasterPty::resize; wait = native exit event+GetState / inspect_exec / child exit; close = existing transport-specific cancellation/cleanup. Never migrate a live terminal or create a replacement process automatically.

## 6. Design (approved design; as-built refinements in completion record)

### 6.1 Data flow & threading

```text
GPUI stored tasks + revision guards
  → HubHandle::call / subscribe / terminal actor
    → Arc<dyn Engine> = private WslcEngine
      → per-operation policy + exact session target + route-state snapshot
        → COM delegate (trusted ABI, MTA RPC pool/dedicated stream threads)
        → lazy CLI delegate (argv runner, bounded children, ConPTY)
```

The router owns delegates and route state; it does not move I/O to GPUI. COM unsafe stays under `dk-engine-wslc/src/com/`. `dk-core` remains pure; no tokio/COM/GPUI types leak through the contract. Hub owns async tasks and terminal sessions. Existing GPUI task handles remain on their entities, stale results use engine/request/connection revisions, and existing logs/terminal batching and bounded channels remain intact (NFR-001…005).

#### Preference and connection policy

| Preference | Connection / operation behavior |
|---|---|
| Auto | Trusted exact version → COM self-check; verified ops prefer COM; explicitly unverified Run goes directly to CLI. Unknown ABI/eligible activation-self-check transport failure → CLI if safe and available. Domain/policy/authorization/cancellation errors are final. |
| COM | Exact trust and successful COM connection required. No CLI construction, availability probe, child spawn or fallback for this connection. Run remains 501. Discovery is a separate global scan, not an implicit action of this connection. |
| CLI | CLI-only connection; no internal COM activation/self-check on behalf of this connection. |

CLI is lazily prepared when needed; missing CLI does not break COM connect or verified COM operations. Do not interpret all COM connect errors as self-check transport failures. A recognized self-check mismatch can select Auto CLI but cannot authorize further ABI calls.

#### Session binding

- Keep configured endpoint identity separate from the resolved connection target.
- Explicit target: preserve the exact validated name; never substitute the caller default.
- COM default: resolve the opened session's display name successfully and bind CLI with `--session <exact-name>` immediately before the subcommand.
- Reopen COM against that same resolved target, not `OpenSessionByName(NULL)` again.
- Record/compare available session identity to detect disappearance/replacement. T1 verified exact name/runtime ID/creator PID/SID binding; no invented CLI identity API or atomic check-and-spawn guarantee.
- CLI-only default retains caller-default behavior; never synthesize a name from USERNAME. No automatic elevation or session creation.
- If equivalent cross-transport targeting is unproven, keep COM usable and refuse the cross-transport operation with a clear hint.

#### Read operations

Move retry decisions out of generic `with_session`. Default maximum budget: one COM attempt, one same-session reopen+COM read on an allowlisted disconnect, then one CLI attempt. No retry loops/multiplicative helper retries. A persistently failed operation route becomes CLI-sticky for this connection; reconnect reestablishes COM-first routing. The permanent native Run exception does not demote unrelated operations.

Initial fault-classification review covers existing `RPC_E_DISCONNECTED`, `RPC_S_SERVER_UNAVAILABLE`, `RPC_E_SERVER_DIED`, `RPC_E_SERVER_DIED_DNE`, `RPC_S_CALL_FAILED`, with explicit per-code/phase tests. These are candidate transport faults, not proof that a mutation never entered the server. Generic `Unreachable`, timeout, 500/501 or string matching are not sufficient routing criteria. Policy, authorization/elevation, missing session/resource, conflict, validation, cancellation and malformed JSON are final. Windows I/O errors need their own reviewed transport classification.

#### Mutation dispatch and reconciliation

| Phase | Required behavior |
|---|---|
| Before protected mutating RPC / CLI spawn | Validate inputs, target and option parity. Obtain required token. Eligible transport preparation failure may select CLI once in Auto; policy/domain errors may not. |
| Mutating RPC entered / CLI child spawned | Conservatively MayHaveDispatched. Never repeat the mutation automatically, even if no response/output arrived. |
| Mutation returned success, enrichment read fails | Preserve completed phase and known identity; retry/fallback the read only. Never rerun CreateVolume/Run/Exec to recover a DTO. |
| Ambiguous result | Bounded read-only reconciliation on the same target, under one deadline; return success only when the contractual result can be established. Otherwise existing Unreachable with unknown-outcome hint. |

Reconciliation rules:

| Mutation group | What reads can establish / limitations |
|---|---|
| Start/stop | Same full ID in requested state can establish primary postcondition. Avoid name/prefix rebinding. |
| Remove container/volume/network | Absence can establish primary resource removal; not necessarily ancillary volume deletion or lost deletion-report data. |
| Restart/kill | Running/exited alone does not prove restart occurrence or requested signal. No replay on weak evidence. |
| Named volume create / image tag | No automatic ambiguous-create/tag success inference is implemented. Successful volume create recovers inspect using its returned name; other unproved outcomes stay unknown. A matching name alone does not prove this request succeeded. |
| Run | Known returned ID may support post-read recovery; unnamed/lost-ID or auto-remove outcomes may remain unknown. No correlation labels or altered RunSpec introduced silently. |
| Exec / pull / prune / report-producing delete | Lost process ID, pull completion/digest, deleted items or reclaimed bytes may be unrecoverable. Do not fabricate them or issue a second mutation. |

A negative read does not prove an in-flight timed-out mutation cannot commit later. Do not auto-retry create after "not found". Cancellation after dispatch does not mean rollback. Internal state may outlive a dropped waiter for safe cleanup; never deliver a stale result to a different active connection.

#### Streams and terminal boundaries

- Logs/stats/events can fall back once on eligible failure **before the first source item**; cancel/drop the COM producer before opening CLI.
- Track source activity before event filtering so filtered-out events cannot hide an already-started source.
- After the first item, transport failure yields a terminal error and ends; no silent source splice. Clean EOF does not trigger fallback.
- `Protocol("events lost")` remains a gap/refetch signal and may continue on the same source; it is not a transport-fault trigger.
- Explicit resubscription uses the current sticky route. Events require full refetch across gaps; stats reset normalization baseline; logs use existing resume/dedupe semantics, not a new exactly-once guarantee.
- Pull: no progress is NOT evidence of no mutation. CLI fallback requires both predispatch proof and no emitted item; never discard supplied auth to make CLI work.
- Exec: failure after Exec but before GetStdHandle is possibly dispatched. No second process; no migration of returned TerminalSession output/write/resize/wait/close.
- Ensure sibling stdout/stderr readers and underlying producer cancellation complete safely before fallback. Use bounded buffers, not speculative parallel COM+CLI subscriptions.

#### COM lifetime/resource repair

- Required `BeginContainerOperation` returns a checked token; failure blocks the protected RPC. Hold it through complete protected mutation/stream/process lifetime, matching verified WSLC usage.
- Queued RPC admission checks caller cancellation before execution and immediately before protected mutation entry. Define an atomic admission/dispatch boundary: cancellation that wins prevents dispatch; after dispatch wins, report/cancel conservatively without pretending rollback.
- Preserve nonblocking RPC-pool teardown; do not join threads on hub/UI workers. Handle thread/MTA initialization failure explicitly.
- Separate owned socket (`closesocket`) and kernel/event/file (`CloseHandle`) resources. Honor WSLC handle tags; reject unknown tags; audit partial outputs, aliasing, failure cleanup and close-once ownership.
- Socket ReadFile/WriteFile use is existing evidence; verify cancellation/overlapped completion before freeing buffers or closing sockets. Do not mechanically replace the destructor without checking cancellation and stream ownership.

#### Exact-version ABI trust and discovery

Initial allowlist: `3.0.1.0 → v3_0`, pinned IDL tag `3.0.1`, linked live/contract evidence. Select from non-COM version before activation, including discovery. GetVersion's major/minor/revision must match the selected file version triple; it cannot attest the fourth component or broaden the allowlist.

Unknown version/build: Auto CLI, forced COM fails without internal vtable activation. Recheck non-COM trust on reconnect/reopen to avoid using a module after a WSL update. Identical IDL for 3.0.0/2.9.13 is supporting evidence, not automatic admission. Future entries require IDL/struct review and live marshalling/contract runs. Native CreateContainer remains a separate disabled operation gate even on a trusted module.

Discovery extra-session SID must be present, valid and equal to a present/valid caller SID. Unknown SID is excluded. CLI session table has no owner SID and is not an ownership proof; advertise default only until a verified ownership mechanism exists. Policy enumeration failure must stop CLI fallback. Keep the default engine visible and show a policy failure at connection; no new discovery return type is needed. Preserve ENG-114 for previously listed engines: routing/discovery trust repairs do not silently remove already-seen registry entries. Connecting an unproven extra target still requires authorization/target validation.

#### Metadata propagation

Router produces coherent route-state snapshots for capabilities/info. Use `transport = "com"` for COM-primary mixed routing, note `"COM primary; Run uses CLI — native Run unverified"`; CLI-only uses `"cli"` plus reason when applicable. Degradation notes name affected operation/reason, not arbitrary last-call transport. Stable ordering and deduplication avoid flapping notes.

`PULL_PROGRESS` follows the route of future pulls; a CLI stats route sets `list_stats_limit` to CLI-safe zero. Run via CLI alone does not drop COM pull progress or list stats. Refresh full EngineInfo on the active health cycle under a bounded timeout, outside registry locks. Commit only for the same engine/connection generation; preserve the prior snapshot if metadata refresh alone fails. Publish existing StatusChanged when note/transport/list limit changes without caps; keep CapabilitiesChanged for actual cap changes. EngineListStore and active EngineStore must consume the snapshot; a local InfoChanged event may be used without adding a public HubEvent. Metadata-only refresh must not falsely emit Reconnected, remount pages or rerun mutations.

### 6.2 UI

No new routes, components, controls or shortcuts. Existing GPUI Kit status info chip, Settings engine row, tooltip, Diagnostics and action notifications consume transport_note/full metadata. Existing loading/empty/error/data states remain; retain previous data on refresh failure. Unknown outcome notifications say the operation may have completed and require refresh before manual retry; no automatic resubmission.

Images `U`/existing Run Action and dialog remain keyboard-operable (KBD-002/007/030). Success navigates to the returned container ID, even if hello-world already exited. Dialog cancel/error restores invoking focus; metadata refresh preserves focus/selection. CDT Network/header uses normalized typed ports, while Inspect displays original raw JSON (formatting on background_spawn). No transport/kind branches in UI.

### 6.3 Errors

| Existing EngineError | Routing / UX |
|---|---|
| Unsupported(single capability) | WSLC pause/top/history/disk usage unchanged; hidden/disabled control safety net |
| Api 501 | Strict COM Run limitation only; never blanket-fallback on 501 |
| Api 400/403, Conflict, NotFound | Final validation/authorization/domain result; existing notification/inline error |
| Unreachable with policy/elevation/session hint | Final policy/auth/target error; no attempt to bypass with CLI |
| Classified transport failure | Read/predispatch-only fallback in Auto within budget; combined diagnostic context if CLI also fails |
| Unreachable with unknown-outcome hint | Possibly dispatched mutation cannot be proven complete; refresh before manual retry; no new public type |
| Protocol | Bad payload final; existing exact events-lost sentinel keeps gap behavior |
| Timeout / Cancelled | Not proof of no dispatch; no automatic mutation replay; cancel queued work and safely release active resources |

## 7. Tasks

Historical executable task breakdown below: T0 approval and T1–T14 implementation are complete; T15 Windows validation is complete, with cross-OS validation transferred to RG1–RG3 above; T16 reviewer approved. S1 remains separately deferred/out of repair scope. Estimates/dependencies are retained for traceability, not open implementation work.

| # / phase | Task | Owner agent | IDs | Depends on | Size | Verify |
|---|---|---|---|---|---|---|
| T0 / docs | Review/approve draft and reconcile any design changes; record approval only on explicit user approval. No missing ADR/milestone creation. | architect | ENG-126…136, ENG-013/109/110, IMG-005, CDT-030/040 | User approval gate | 0.5d | Spec/plan consistency and full mapping reviewed |
| T1 / trust | Spike exact target binding, display-name identity/uniqueness and replacement detection; record `3.0.1.0` evidence in maintained test notes. | windows-platform | ENG-127,132,135 | T0 | 0.5d | Default/explicit/admin isolation; unresolved equivalence blocks mixed dispatch |
| T2 / trust | Exact-version allowlist, reconnect/reopen checks and fail-closed extra-session discovery/policy handling. | windows-platform | ENG-132,135, ENG-013/109 | T1 | 1d | Unknown patch/build → zero activation; missing SID excluded; policy → zero CLI enumeration |
| T3 / safety | Private typed fault/phase classification; remove blanket session/container mutation retry. | windows-platform | ENG-129,130 | T0 | 1d | Fake COM call counts for read vs mutation disconnect; phase survives public mapping |
| T4 / safety | Required operation tokens and queued RPC cancellation/admission boundary. | windows-platform | ENG-134 | T3 | 1d | Token failure → zero protected calls; canceled queued job → zero dispatch; race tests |
| T5 / safety | Tagged socket/kernel RAII and stream/exec partial-failure cleanup; cancellation/overlapped audit. | windows-platform | ENG-134 | T4 | 1d | closesocket vs CloseHandle; close-once/partial-output tests; bounded drop/leak check |
| T6 / router | Private WslcEngine, strict preference matrix, lazy CLI, exact target, explicit predispatch Run exception. | engine-integrator | ENG-126…128, IMG-005 | T1,T2,T3,T4 | 1d | Auto Run has zero native calls; forced COM zero CLI; missing CLI preserves COM reads |
| T7 / router | Bounded read fallback and sticky per-operation routes. | engine-integrator | ENG-129 | T6 | 1d | HRESULT allowlist, domain-error exclusions and exact attempt budgets |
| T8 / mutations | Container mutation reconciliation and no replay, including unknown Run outcomes. | engine-integrator | ENG-130, IMG-005 | T6,T7 | 1d | Commit-then-disconnect/timeout → one mutation; full-ID reads; unknown outcome hint |
| T9 / mutations | Image/volume/network mutation safety, completed-write/post-read recovery and auth/option parity gates. | engine-integrator | ENG-130,131 | T7,T8 | 1d | Create+inspect failure → one create; prune equivalence or fallback disabled; auth not discarded |
| T10 / streams | Read-stream first-source-item boundaries, pinned terminal and pull dispatch rules. | engine-integrator | ENG-131,134 | T5,T7,T9 | 1d | Pre/post-item faults, filtered events, clean EOF, ambiguous pull/exec, producer cancellation |
| T11 / DTO | Shared WSLC inspect normalization for both delegates; retain original raw. | engine-integrator | ENG-133, CDT-030/040 | T0 | 0.5d | Ports IPv4/IPv6/TCP/UDP/exposed-only; missing NetworkSettings; raw deep equality |
| T12 / hub | Full info refresh/status broadcast with bounded I/O and stale-connection guards. | rust-core | ENG-136 | T6,T7,T10 | 1d | Note/list limit changes with identical caps propagate; old connection cannot overwrite new |
| T13 / UI | Active-store snapshot propagation and existing Run/detail/keyboard regression coverage. | gpui-ui | ENG-136, IMG-005, CDT-030/040 | T11,T12 | 0.5d | Chip/Diagnostics update without remount/focus loss; U→Run→exited detail; light/dark evidence if visuals change |
| T14 / acceptance | Add maintained opt-in hello-world harness replacing external probe; record fixtures/results and cleanup. | qa-engineer | ENG-127,128,132…134, IMG-005, CDT-030/040 | T6,T8,T10,T11 | 1d | Default and explicit session; raw/typed Run, both inspect/log paths, only own resources removed |
| T15 / acceptance | Complete router fault matrix/contract regressions and real normal-user Windows acceptance. | qa-engineer | ENG-126…136, IMG-005, CDT-030/040 | T2…T14 | 1d | ID→tests table, exact-version live smoke, 3-OS build/test/clippy/fmt/blocking checks |
| T16 / review | ABI/retry/security/ownership review; reconcile spec verification/status only for proven repairs. | reviewer | ENG-126…136, ENG-013/109/110, IMG-005, CDT-030/040 | T15 | 0.5d | Approve verdict; no duplicate dispatch, no false live/ABI claims |
| S1 / optional deferred | Native CreateContainer layout/semantics evidence spike; report only, no enablement in this repair. | windows-platform | ENG-128,132 | Separate user approval; T2,T14 | ≤1d timebox | Exact-version live options/cleanup evidence; follow-up plan needed before enablement |

T9 resolved parity conservatively: mixed volume-prune and explicit-local create-volume fallback are refused; supplied CLI pull auth is rejected before spawn. No unverified flag or weakened request semantics were enabled.

## 8. Test plan

Actual tests below were inspected in current sources and passed in the final Windows workspace run; the opt-in live harness was separately passed by the parent. Module prefixes are omitted for readability. Cross-OS/stress limits remain explicit above.

| ID | Layer | Test name(s) / acceptance |
|---|---|---|
| ENG-126, ENG-013 | Router/factory | `eng_126_strict_preferences_and_eng_128_run_exception`, `eng_126_missing_cli_preserves_com_and_strict_cli_no_com`, `eng_126_router_contract_suite`, `plan_matrix` — implemented/PASS |
| ENG-127 | COM/router/CLI admission + live | `eng_127_default_pinned_reopen_replacement_refused`, `eng_127_validate_before_each_crossing_replacement_refuses`, `eng_127_saturated_permit_revalidates_before_mutation_spawn`, `eng_127_exec_revalidates_after_inspect_inside_blocking_spawn`, `eng_127_cli_stats_revalidates_every_poll_and_stops_on_replacement`; live default/explicit/admin matrix — implemented/PASS, non-atomic residual race |
| ENG-128, IMG-005 | Router/view/live | `eng_126_strict_preferences_and_eng_128_run_exception`, `eng_130_img_005_cli_lost_run_id_not_fabricated`, `img_005_run_exited_detail_cdt_030_040`, `img_005_run_501_and_unknown_outcome_do_not_resubmit_restore_focus`, `eng_127_128_normal_user_hello_world_acceptance` — implemented/PASS |
| ENG-129 | Factory/router/real delegate fakes | `eng_129_allowlist_budget_and_sticky_routes`, `eng_129_domain_policy_protocol_cancel_never_fallback`, `eng_129_reopen_domain_and_second_read_protocol_final`, `eng_129_factory_only_typed_faults_or_selfcheck_mismatch_prepare_cli`, `eng_129_cli_malformed_inspect_must_remain_protocol_error`, `eng_129_com_malformed_payload_no_implicit_retry` — implemented/PASS |
| ENG-130 | COM/router/CLI | `eng_130_commit_disconnect_no_replay`, `eng_130_commit_then_disconnect_stop_one_mutation_full_id_read`, `eng_130_timeout_then_late_commit_is_one_mutation`, `eng_130_completed_create_enrichment_failure_never_recreates`, `eng_130_mutation_phase_matrix_one_dispatch_and_parity_gate`, `eng_130_shared_child_lost_response_classification` — implemented/PASS; no live lost-response injection claimed |
| ENG-131 | Stream/terminal/auth | `eng_131_source_boundary_clean_eof_and_pull_dispatch`, `eng_131_filtered_event_counts_as_source_activity`, `eng_131_events_lost_continues_and_auth_never_discarded`, `eng_131_exec_committed_getstdhandle_failure_never_reexecs`, `eng_131_terminal_remains_pinned_after_future_exec_route_changes`, `eng_131_after_item_failure_next_subscription_waits_for_old_source`, `eng_131_native_producer_drained_before_cli_logs_open` — implemented/PASS |
| ENG-132 | ABI/activation + live | `eng_132_exact_allowlist`, `eng_132_unknown_version_zero_activation`, `module_metadata_matches_submodule`, `struct_layouts_match_midl_x64`, `slot_offsets_of_called_methods`; live service version/self-check/inspect/logs — implemented/PASS; native CreateContainer deferred |
| ENG-133, CDT-030/040 | Shared parser/fake delegate/view | `eng_133_ports_and_cdt_040_original_raw`, `eng_133_recorded_com_cli_inspect_raw_deep_equality`, `eng_133_cdt_030_040_published_ports_both_delegate_paths`, `img_005_run_exited_detail_cdt_030_040` — implemented/PASS; published-port fixture synthetic, live hello ports empty |
| ENG-134 | RPC/Win32/ConPTY | `eng_134_cancelled_queue_no_rpc`, `eng_134_admission_race`, `eng_134_begin_token_failure_blocks`, `eng_134_token_failure_blocks_remove_logs_and_exec`, `eng_134_socket_close_once_and_kernel_tags`, `eng_134_socket_cancel_drains_overlapped_before_close`, `eng_134_cancel_queued_blocking_spawn_zero_admissions`, `conpty_stalled_writer_close_and_switch_keep_runtime_responsive` — implemented/PASS |
| ENG-135, ENG-109 | Factory | `eng_135_missing_sid_excluded`, `eng_135_factory_policy_zero_cli_preparation_and_default_visible`, `explicit_target_contradiction_final_zero_cli_and_protected_calls`, `discovery_default_plus_hidden_sessions`; live admin rejection — implemented/PASS; no live policy injection |
| ENG-136, ENG-110 | Router/hub/UI | `eng_136_coherent_metadata_no_last_call_demotion`, `eng_136_info_without_caps_change_and_cli_stats_limit`, `eng_136_caps_and_metadata_commit_one_coherent_snapshot`, `eng_136_stale_connection_info_ignored`, `eng_136_refresh_failure_timeout_and_panic_preserve_snapshot`, `eng_136_metadata_without_flags_preserves_focus_page_and_updates_stats` — implemented/PASS |
| ENG-114/022/030…033, NFR-001…005/020/022 (preserved) | Windows workspace/regression/static review | Existing stable-list/reconnect/capability-gate/keyboard/channel/stale-result tests, unchanged public traits/layers, argv validation; final check-blocking/fmt/clippy passed. Non-Windows build/test release gate remains open. |

### 8.1 Exact baseline / repeat commands

From the repository root, using a normal-user PowerShell session and installed WSL 3.0.1.0:

```powershell
# File version (already verified in supplied evidence):
(Get-Item "$env:ProgramFiles\WSL\wslservice.exe").VersionInfo.FileVersion

# Current Windows WSLC suite: final workspace subset 183 passed, 18 live opt-in ignores.
pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc

# Existing maintained factory smoke: supplied result 1 passed.
pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test com_live live_factory_discover_connect_auto_uses_com -- --ignored --exact --nocapture
```

The factory smoke proves connect/selection, not native Run. T14 now supplies the expanded `repair_live` harness and exact command in the completion record; the initial external probe is historical evidence only.

Exact CLI repetition with a unique name, explicitly bound to the observed session:

```powershell
$wslc = "$env:ProgramFiles\WSL\wslc.exe"
$session = 'wslc-cli-pavel' # replace only with an explicitly verified owned session
$name = 'dockering-repair-hello-' + [guid]::NewGuid().ToString('N')
& $wslc --session $session container run --detach --name $name hello-world:latest
if ($LASTEXITCODE -ne 0) { throw 'Run failed; inspect this unique name before considering any retry' }
try {
    # Wait for exit if still running; inspect/log reads may be repeated, Run may not.
    & $wslc --session $session container inspect --format json $name
    & $wslc --session $session container logs $name
} finally {
    & $wslc --session $session container remove --force $name
}
& $wslc --session $session container list --all --no-trunc --format json
```

Do not expect the old IDs or a globally empty list on another machine. Harness assertions wait boundedly for exit 0, `/hello`, and Hello from Docker; read via both typed transports, record session/version/preference, remove only harness-owned containers, and leave the image/unrelated resources. If Run failed ambiguously, reconcile the unique name without automatically rerunning it; clean up only a positively identified harness resource.

Final Windows validation/repeat commands:

```powershell
pwsh -NoProfile scripts/dev.ps1 test --workspace
pwsh -NoProfile scripts/dev.ps1 clippy --workspace --all-targets -- -D warnings
pwsh -NoProfile scripts/dev.ps1 fmt --all -- --check
pwsh -NoProfile scripts/dev.ps1 run -p xtask -- check-blocking
```

Windows final test/clippy/fmt/check-blocking passed. Linux/macOS validation is explicitly transferred to RG1–RG3; do not describe three-OS CI or full [quality DoD](../../spec/60-quality.md) as green yet. Fakes test routing, dispatch counts and allocation ownership, **not out-of-process COM marshalling**. New ABI entries/native operation enablement require real version-specific runs.

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Treating the proven Run 501 as a bootstrap/elevation failure | P0 repair is explicit predispatch CLI routing; do not rewrite working startup security or require elevation |
| Internal ABI self-check calls already unsafe on an unknown layout | Exact allowlist selected before any internal activation, including discovery; self-check cannot broaden trust |
| WSL update while connected / session replacement | Revalidate before reopen/reconnect; identity spike T1; fail closed when target equivalence is lost |
| Mutation succeeded despite error/timeout/cancel | Typed phase, no generic replay, bounded read-only reconciliation, clear unknown-outcome hint |
| Absence of progress/output mistaken for no mutation | Pull/exec/run follow dispatch boundary, not first-byte heuristic |
| Read streams silently mix histories | First-source-item boundary before filtering; after failure terminate and explicitly resubscribe/refetch |
| CLI missing breaks healthy COM | Lazy delegate; COM remains available, only CLI-required operation fails |
| COM vs CLI option mismatch | T9 implemented gates refuse mixed volume-prune/explicit-local create-volume fallback and reject supplied CLI pull auth before spawn. No unverified options silently substituted. |
| Closing socket with wrong API / freeing overlapped buffers early | Tagged RAII, close-once tests, cancellation/completion audit on real Windows |
| Missing SID/table name mistaken for owner proof | Fail closed for extras; default remains; policy final even during enumeration |
| Full info refresh adds I/O or stale UI | Bounded active-only refresh outside locks; connection-generation guard, existing full StatusChanged; no remount |
| Native CreateContainer declaration mistaken for live support | Optional S1 evidence-only spike; no enabled native Run or claimed bad layout in this repair |
| Baseline passes mistaken for feature completion | Maintain ID→test table and run maintained live harness; ignored tests remain explicitly unverified |

## 10. Revision log

- 2026-10-05: follow-up, docs only. Superseded in part by [status-bar-engine-label](status-bar-engine-label.md): the status bar no longer draws `transport_note`, and the router reports a note only for notable routes, not Auto's by-design Run through the CLI (the Metadata propagation paragraph in §6.1 and the ENG-110/136 wording). This completion record describes what was built on 2026-10-04.
- 2026-10-04: complete mode, docs only. Current implementation/tests and final Windows workspace output inspected; parent final reviewer approve and expanded four-Run live PASS recorded. Repair status done/feature specs implemented; cross-OS release validation explicitly remains open, native CreateContainer deferred, mixed identity-check/spawn race documented. No code change, test rerun or commit.
- 2026-10-04: user approved implementation of the repair plan; native CreateContainer enablement remains outside this repair.
- 2026-10-04: created draft documentation and planned spec changes from supplied live evidence and repository audit. No implementation or approval; native Run stays unverified.
