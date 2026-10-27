# Plan: Admit WSL 3.0.3.0 to the WSLC native COM allowlist

- **Slug:** `wslc-abi-3-0-3`
- **Status:** done (2026-10-27; approved the same day when the user asked to run the whole flow and resolve issue #53)
- **Spec:** [Backends §5.3/§5.7/§5.8](../../spec/20-engine-backends.md), [Engines](../../spec/features/engines.md) (ENG-013, ENG-132)
- **Milestone:** post-v1 maintenance (this checkout has no milestone table)
- **Requirement IDs:** ENG-013, ENG-132 (changed/clarified); no new IDs
- **Created:** 2026-10-27

## 1. Goal

[Issue #53](https://github.com/pavel-purma/dockering/issues/53): the weekly `wslc-abi-check` found that WSL 3.0.3 (the current stable release, published 2026-10-09) differs from the vendored 3.0.1 IDL. Native COM is trusted for the exact service version `3.0.1.0` only (ENG-132), so every user on 3.0.3 runs on the `wslc.exe` CLI fallback: no structured pull progress (a COM-only capability) and one child process per operation.

After this change `3.0.3.0` is also trusted, as its own exact entry with its own vendored IDL and recorded live evidence. Nothing else widens: `3.0.2.x`, `3.0.3.1` and `3.0.4` stay on the CLI.

## 2. Scope

**In:** vendor `idl/3.0.3/` verbatim; record the IDL review as a test; make the allowlist exact by construction; admit `3.0.3.0` on the existing `v3_0` module; fake-COM tests for the new version; relax the live harness's hard-coded `3.0.1.0`; run the live suite on a real 3.0.3.0 machine and record the evidence; update the spec; close #53.

**Out:**
- WSL **3.0.4** (pre-release, 2026-10-20). Admit it after it is stable, through the same steps. The weekly job tracks `releases/latest`, so it ignores pre-releases.
- Native `CreateContainer` / Run (stays disabled, ENG-128, no new evidence for it).
- Any range-based trust (removed on 2026-10-04).
- Recording `tests/fixtures/com-3.0.3/` (the existing 3.0.1 fixtures still parse the same shapes; not needed for admission).
- Changing the weekly job.

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| A1 | The `v3_0` module serves 3.0.3.0. The only ABI difference is `WSLCBuildImageOptions` (one member gained `[unique,string]`, two appended); the module declares `BuildImage`'s options as an opaque `*const c_void` and nothing calls it. No interface, IID, method signature or other struct changed. If the review finds anything else, a new `v3_3` module is needed instead | Yes |
| A2 | Exactness becomes structural: `AbiInfo` carries one exact `version` per entry instead of `min`/`max`, so a gap such as 3.0.2 between two admitted versions cannot be expressed | Yes |
| A3 | Live evidence comes from this dev machine: non-elevated, WSL 3.0.3.0, no WSLC session at the start. Starting the default session uses the documented S-4 command (`wslc container list --all --format json`), the same thing the app's *Start* does. Everything created is removed afterwards and the starting state is restored | Yes |
| A4 | Hosted CI cannot provide live evidence (no WSLC). CI covers the gate, the vendored IDL and the review test; the live run is recorded in spec 20 §5.7 | Yes |
| A5 | The CLI changed in 3.0.3 ("reject commands with extra arguments"). That is outside admission, but the issue says affected users fall back to the CLI, so a quick `cli_live` pass is run and any breakage is reported separately | Yes |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-013 | Transport selection stays as written. Current allowlist entries (spec 20 §5.3): `3.0.1.0` (IDL `3.0.1`) and `3.0.3.0` (IDL `3.0.3`), both served by `v3_0` | Changed (entries) |
| ENG-132 | Unchanged rule, one clarification: an exact version enters the allowlist only with its own vendored IDL directory, a recorded IDL review and recorded live evidence for that version (§5.7) | Clarified |

## 5. Engine contract impact

None. No `Engine` method, DTO or capability changes. Crate-private change only:

```rust
// crates/dk-engine-wslc/src/com/abi/mod.rs
pub struct AbiInfo { pub module: AbiModule, pub version: WslVersion /* exact */, pub idl_tag: &'static str }
pub const MODULES: &[AbiInfo] = &[/* 3.0.3.0 → V3_0 (idl 3.0.3), 3.0.1.0 → V3_0 (idl 3.0.1) */];
pub fn select(v: &WslVersion) -> Option<AbiModule>; // exact equality, unchanged signature
```

Mapping for backends is unchanged (spec 20 §5.4/§5.5). On 3.0.3.0 Auto selects COM per verified operation as on 3.0.1.0; native Run stays on the CLI.

## 6. Design

### 6.1 Data flow & threading
Unchanged. `select` runs before activation, on the MTA activation worker and before reopen (spec 20 §5.3); it now compares for equality against the list.

### 6.2 UI
No UI change. A 3.0.3 engine reports COM as its primary transport (Settings → Engines, Diagnostics) and `PULL_PROGRESS`.

### 6.3 Errors
Unchanged. An unadmitted version still yields zero activation and the CLI route.

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Vendor `idl/3.0.3/{wslc.idl,WSLCShared.idl}` byte-for-byte from tag `3.0.3` (commit `15ea3b38…`), add the README row, and add an xtask test that pins the review result (only `WSLCBuildImageOptions` differs from 3.0.1) | release-engineer | ENG-132 | `cargo xtask wslc-abi-check 3.0.3` shows only that type; the new xtask test passes |
| 2 | Make `AbiInfo` exact (`version`), add the `3.0.3.0` entry, update `v3_0` metadata and docs | windows-platform | ENG-013, ENG-132 | `eng_132_exact_allowlist`, metadata-matches test |
| 3 | Tests: select/zero-activation for 3.0.3.0, 3.0.3.1, 3.0.2.0, 3.0.4.0; fake-COM self-check on a 3.0.3 server | qa-engineer | ENG-132 | `cargo test -p dk-engine-wslc` |
| 4 | Replace the hard-coded `3.0.1.0` assertion in `repair_live` with the exact-allowlist check | qa-engineer | ENG-132 | harness compiles and runs on 3.0.3.0 |
| 5 | Live evidence on WSL 3.0.3.0 (spike, ⚠ verify): self-check, discovery, lists, events, pull progress, volume create/remove, running-container stats/logs/exec, `repair_live` acceptance, identity reopen; then restore the baseline | windows-platform | ENG-132 | results in §10 and spec 20 §5.7 |
| 6 | Reconcile spec 20 §5.3/§5.7/§5.8, engines.md verification row, spec 21 evidence note, plan index | architect | ENG-013, ENG-132 | spec matches code |
| 7 | Review | reviewer | all | verdict: approve |
| 8 | PR (`Closes #53`); confirm `cargo xtask wslc-abi-check latest` exits 0 | release-engineer | — | exit 0 |

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| ENG-132 | unit | `eng_132_exact_allowlist` (3.0.1.0 and 3.0.3.0 admitted; 3.0.0, 3.0.1.1, 3.0.2.0, 3.0.3.1, 3.0.4.0, 3.1.0 refused), `module_metadata_matches_submodule`, `vendored_idl_exists_for_every_module` |
| ENG-132 | unit | `eng_132_unknown_version_zero_activation` extended with 3.0.3.1 and 3.0.4.0 |
| ENG-132 | fake COM | self-check passes for a 3.0.3 server through the `v3_0` module and fails for a mismatched triple |
| ENG-132 | xtask | vendored 3.0.1 vs 3.0.3 differ only in `WSLCBuildImageOptions` |
| ENG-013, ENG-132 | live (opt-in) | `com_live` suite and `repair_live::eng_127_128_normal_user_hello_world_acceptance` on 3.0.3.0 |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Live evidence comes from one non-elevated dev machine | Same scope as the 3.0.1 admission; elevated and fault/leak stress stay unclaimed, as before |
| 3.0.3 changes behaviour without changing the ABI (event filters, extra-argument rejection) | The COM events path passes no filters (`GetEvents(…, null, 0)`). The live events test covers it. The CLI pass (A5) covers argument rejection |
| WSL ships weekly, so the allowlist lags by design | Exact allowlist is the spec'd trade-off; each admission is small (this plan is the template). 3.0.4 follows when stable |
| A misjudged IDL review would mean undefined behaviour | The review is pinned by an xtask test and checked by the `reviewer` agent; the live suite exercises every called vtable slot |

## 10. Revision log

- 2026-10-27: created and approved; implementation follows.
- 2026-10-27: implemented. IDL review pinned by an xtask test; `MODULES` is exact per version; live evidence on WSL 3.0.3.0 recorded in spec 20 §5.8 (S-3). The live harness no longer hard-codes 3.0.1.0 (`repair_live`, `eng_127_live_default_explicit_identity_and_reopen`). The dev machine's WSLC state was restored (no session).
