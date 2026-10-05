# T6–T11 integration notes

## Final implementation status — 2026-10-04

The repair is implemented and reviewer-approved. The authoritative completion record,
requirement-to-test matrix and release gates are in the
[central plan](../../docs/plan/features/wslc-integration-repair.md).
Parent final Windows validation: **WSLC 183 passed, 18 live opt-in ignores**;
workspace tests passed, including **hub 77** and **UI 243**. Workspace all-target clippy,
fmt and blocking checks passed. These are parent-reported final results, not a docs-only rerun.

Expanded exact `repair_live` acceptance passed **1 test in 20.03 s**: default Auto,
explicit Auto, explicit CLI and independent raw CLI Run, each verified through both typed
COM/CLI inspect and logs as Exited/0, `/hello`, Hello from Docker. All six preference paths
passed; strict COM Run remained 501. Existing admin-session access was rejected under all
three preferences without elevation. All four owned probes were removed; zero owned and
zero unrelated baseline containers remained, and the hello-world image was retained.

Native CreateContainer remains deferred. Linux validation is blocked by the missing compiler
needed for `ring`; macOS was not run. Cross-OS validation remains a release gate, not a
completed full DoD. No live fault injection or marshalling stress is claimed.

The factory now returns the private `router::WslcEngine` for both COM-primary and CLI-only
connections. The router consumes windows-platform's per-invocation typed HRESULT/phase
capture and per-stream source evidence. No public Engine contract changed. Native Run
remains disabled; Auto chooses the pinned-session CLI route before dispatch.

Metadata handoff: `info()` retains `transport = com` for COM-primary connections and reports
`COM primary; Run uses CLI — native Run unverified`, followed by sorted/deduplicated degraded
operation names. Future CLI pulls remove PULL_PROGRESS; CLI stats set list_stats_limit to zero.
Run alone does neither. T12/T13 hub/UI full-info propagation is now implemented and verified
in the final Windows workspace run; see the central plan rather than treating this backend
handoff as pending work.

Superseded 2026-10-05 (plan status-bar-engine-label): Auto no longer reports a route note; COM-only reports `COM only — Run unavailable`; fallbacks report `CLI fallback: <ops>`.

Safety gates: mixed volume prune is refused (COM all-unused versus CLI anonymous-only).
Mixed create-volume with explicit `local` driver is refused on CLI fallback (CLI substitutes
its default, COM forwards the driver). Supplied pull auth cannot enter CLI or be ignored.
CLI-only prune keeps its existing semantics. No new CLI flags are claimed as verified.

Completed native volume creation uses the returned name and read-only inspect recovery;
failed enrichment never repeats creation. Lost prune/delete reports, restart/signal/exec/pull
results and lost Run IDs remain unknown rather than fabricated. Start/stop and removals use
bounded same-identity reads where the primary postcondition can be established. Ancillary
container volume removal cannot be inferred from container absence.

Stream cancellation drops the delegate and awaits tracked native producer completion under
a deadline before opening CLI, including sticky resubscriptions after a post-item fault.
Sibling log readers are included; teardown itself stays on dedicated threads. This does not claim an
atomic CLI identity check-and-dispatch (the CLI has no such API), or synchronous joining of
producer threads. Terminal sessions remain the exact delegate object returned at creation.

Historical T6–T11/follow-up validation on Windows (superseded by final results above):
- At that stage, WSLC suite: 156 passed, 18 ignored (115 unit, 18 CLI, 15 COM, one allocation,
  seven repair contract; ignored tests are live only).
- Includes `dk_core::contract::run_suite` against the private COM-primary router.
- 20 router fault/policy/budget/stream/metadata tests; shared inspect tests use recorded COM
  and CLI fixtures plus IPv4/IPv6/TCP/UDP/exposed-only cases and raw deep equality.
- crate all-target clippy with `-D warnings`, formatting and diff whitespace checks passed.
- Live factory smoke passed after updating its old whole-delegate metadata assertion.
  One intervening run returned final E_FAIL (0x80004005) from info; it was not classified as
  a transport fault or bypassed with CLI. A repeat passed with COM-primary metadata.

That historical follow-up did not rerun hello-world. T14/T15 subsequently passed the expanded
maintained harness, including actual mixed CLI argv/child paths and admin-session rejection;
see [QA evidence](tests/REPAIR-ACCEPTANCE-EVIDENCE.md). Native CreateContainer is unverified.

QA follow-up: malformed CLI inspect now remains Protocol through actual CLI adapters for
containers/images/volumes/networks. Only a valid empty array means NotFound; blank/scalar/null,
nonobject/mixed arrays are Protocol. The formerly ignored regression is enabled and passes.
Additional tests exercise a protected Stop that commits after caller timeout/drop (one write),
Exec success then GetStdHandle failure with/without partial handle (one process, no CLI), native
terminal resize/write cancellation/wait/close after future routing changes, native producer drain
and delayed drain before replacement/resubscription, final reopen domain/second-read protocol
errors, and production factory decision closures with zero CLI preparation on policy/strict COM.
Factory preparation counts imply zero spawn on those rejected paths; no OS-wide spawn monitor
or out-of-process fault injection is claimed. COM fake changes are fault hooks only.
