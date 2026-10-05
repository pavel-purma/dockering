# T14/T15 backend acceptance — 2026-10-04

The repair is **implemented and reviewer-approved**. The
[central plan](../../../docs/plan/features/wslc-integration-repair.md) is authoritative for
the full requirement-to-test matrix, completion record and remaining release gates.
This crate-local note records QA provenance and the parent's final evidence, without
duplicating the full test inventory. This final reconciliation is docs-only: no code/test
changes, test rerun or commit.

## Latest final validation (parent-reported)

| Validation | Result |
|---|---|
| Windows WSLC suite | **183 passed, 0 failed, 18 live opt-in ignores** |
| Windows workspace tests | PASS, including **hub 77** and **UI 243** |
| Workspace all-target clippy, fmt, blocking check | PASS |
| Reviewer | APPROVE |
| Expanded exact live acceptance | **1 passed in 20.03 s**, four Runs |

The malformed CLI inspect Protocol regression is **enabled and passing**, not ignored.
Malformed JSON and invalid inspect shapes preserve Protocol; valid empty `[]` maps to
NotFound. The old NotFound-for-every-parser-error defect is resolved.
Hub/UI metadata propagation and Run/detail regressions are implemented and verified; they
are no longer pending handoffs. See the central plan for individual tests and fault coverage.

## Maintained live command

Run in a normal-user session with an outer timeout of 300 seconds:

```powershell
pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test repair_live -- --ignored --exact eng_127_128_normal_user_hello_world_acceptance --nocapture
```

## Final four-Run live evidence

Environment: non-elevated Windows process, WSL service FileVersion **3.0.1.0**, resolved
caller default **`wslc-cli-pavel`**. COM security/Winsock and self-check succeeded.

| Run path | Both typed COM/CLI inspect | Both typed COM/CLI logs |
|---|---|---|
| Factory default / Auto | Returned full ID, Exited, exit 0, `/hello` | Hello from Docker |
| Factory explicit default session / Auto | Same checks | Hello from Docker |
| Factory explicit default session / CLI | Same checks | Hello from Docker |
| Separate raw CLI / explicit resolved session | Same checks | Hello from Docker |

Raw CLI bypasses Factory/RunSpec and uses argv:
`--session <resolved-name> container run --detach --name <fourth-unique-name> hello-world:latest`.
It requires a successful exit and exactly one full 64-hex returned ID; pull notices are
allowed. Each Run is issued once, never replayed after timeout/error/lost ID.

All **six default/explicit × Auto/COM/CLI** connection paths passed health/info checks.
Auto retained COM-primary mixed metadata (`COM primary; Run uses CLI — native Run unverified`)
after Run; strict COM Run returned **501** in both targets. Native CreateContainer was
not exercised or enabled.

Existing admin-session access was **rejected under Auto, COM and CLI without elevation**.
No session was created; no administrator resource was inspected or removed. This proves
normal-user rejection isolation, not successful admin-session enumeration or elevated-client
acceptance. Explicit/default cross-transport visibility was verified on the owned session.

Cleanup identified all **four owned probes** by unique name/full ID/image, excluded baseline
IDs, removed them by full ID and verified **zero owned containers**. The unrelated baseline
count was **zero**, so preservation with a nonempty live baseline was not exercised.
`hello-world:latest` was retained. Calls/read/exit waits and cleanup have deadlines; panic
handling attempts cleanup with a bounded Drop backup. Process termination, permanent engine
failure or a commit after the reconciliation window cannot guarantee physical cleanup;
failure reports only owned names for manual reconciliation and never repeats Run.

## Fixtures and evidence boundaries

Published-port fixture coverage remains passing: IPv4/IPv6, TCP/UDP, exposed-only ports,
absent NetworkSettings, shared typed fields and original raw deep equality through actual
fake COM and CLI delegates. These sanitised repair fixtures are **synthetic**, not live
published-port recordings. Live hello-world ports are empty.

Native creation is deferred. Linux validation is blocked by the missing compiler needed for
`ring`; macOS was not run. Cross-OS validation remains a **release gate**, so three-OS CI
and full quality DoD are **not** claimed. Fakes do not prove out-of-process COM marshalling;
no live lost-response/policy/stream fault injection or sustained socket leak stress is claimed.
The remaining gates and non-atomic target-check/spawn limitation are tracked in the central plan.

## Historical stages (superseded, not current blockers)

- Initial QA suite: 146 passed, 19 ignored, with a narrowly ignored malformed-CLI Protocol
  regression. An explicit run reproduced the defect; the engine owner subsequently fixed it
  and enabled the regression.
- Engine-owner intermediate suite: 156 passed, 18 ignored, before final exec/target fixes.
- Earlier three-factory-Run live harness: 1 passed in 17.83 s. Two earlier assertion failures
  also cleaned up their sole probes. Those runs predated the independent raw CLI extension.
- The first raw-CLI extension compile attempt encountered concurrent `ConPtySession.killer`
  edits and executed no tests. That transient build failure and the extension's pending-live
  status are superseded by the final 183-pass suite and four-Run live acceptance above.
- The earlier COM elevation rejection appeared as Api 500 with HRESULT 0x800702E4. This is
  historical diagnostic evidence, not a claim about the final public error mapping; final
  acceptance confirms rejection without elevation for all three preferences.
