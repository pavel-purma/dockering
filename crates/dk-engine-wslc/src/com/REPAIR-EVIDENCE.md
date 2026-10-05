# ENG-127/132/134 repair evidence — 2026-10-04

## Final implementation status

The repair is implemented and reviewer-approved; the
[central plan](../../../../docs/plan/features/wslc-integration-repair.md) owns the complete
verification matrix and release gates. Parent final Windows evidence: **WSLC 183 passed,
18 live opt-in ignores**, workspace tests passed (**hub 77**, **UI 243**), and workspace
all-target clippy, fmt and blocking checks passed. This update records supplied final evidence;
it does not rerun tests.

Expanded exact `repair_live` acceptance passed **1 test in 20.03 s** on normal-user service
FileVersion **3.0.1.0**: default Auto, explicit Auto, explicit CLI and separate raw CLI Run
all inspected through both typed transports as Exited/0 with `/hello`, and both logs contained
Hello from Docker. Six preference paths passed; strict COM Run returned 501. Admin-session
access was rejected under Auto/COM/CLI without elevation or touching admin resources.
All four own probes were removed, zero own containers remained, baseline unrelated count was
zero, and the hello-world image was retained. See
[QA evidence](../../tests/REPAIR-ACCEPTANCE-EVIDENCE.md) for the maintained command.

Native creation remains deferred. Linux validation is blocked by the missing compiler for
`ring`; macOS was not run. Cross-OS checks remain release gates; full DoD is not claimed.

## Historical binding spike and lifetime audit

Maintained read-only binding spike:

```powershell
pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --lib eng_127_live_default_explicit_identity_and_reopen -- --ignored --nocapture
```

Passed on Windows, caller default `wslc-cli-pavel`, service FileVersion **3.0.1.0**.
GetId returned 2; ListSessions matched one exact name, ID, creator PID and caller SID.
Explicit name open and same-name reopen returned the identical observed identity. No session,
container or VM was created by the spike. Also passed the existing live factory discovery/
Auto COM connect smoke. Admin isolation/elevated-client acceptance was **not** rerun at
that historical spike stage. Final normal-user admin rejection is recorded above;
elevated-client acceptance is still not claimed.

Available identity is a runtime u32 session ID with creator PID/SID, not a durable UUID.
Revalidation refuses disappearance, duplicate names or changed identity. CLI exposes no
identity-check-and-dispatch operation: replacement between validation and CLI invocation
cannot be atomically excluded. No stronger identity guarantee should be advertised.

Exact-version trust is checked before activation, again on the MTA worker before activation,
and before reopening/target revalidation. MSI fallback can discover presence but cannot
authorize internal activation when the service FileVersion is unreadable.

Lifetime audit: socket/pipe tags choose closesocket/CloseHandle respectively; events always
use CloseHandle. Logs adopt both outputs even on RPC failure and deduplicate aliases. Exec
adopts partial std-handle and exit-event outputs before HRESULT handling. Unknown union tags
are rejected without guessing a destructor (an unknown-tag resource cannot safely be freed).
Pending reads/writes retain the owner and buffers/OVERLAPPED/event until completion;
cancellation uses CancelIoEx followed by waiting GetOverlappedResult on dedicated threads.
Pool/drop does not join. Socket cancellation/drain and destructor choice have real Windows
tests; stuck exec write cancellation is covered by the fake pipe contract. Full live WSLC
socket leak/stress testing and malformed out-parameter marshalling are not claimed.

Native CreateContainer remains disabled (strict COM Run 501; Auto Run uses pinned CLI).
Mutation closures are never replayed;
typed capture retains original allowlisted HRESULT and NotDispatched/MayHaveDispatched/
Completed phase. Completed create enrichment must use the known name returned by
create_volume_raw, never invoke create again. Router owns bounded reconciliation and retry.

## Reviewer follow-up

RPC submission now carries per-invocation Evidence to every worker, including the real
activation/self-check pipeline. Discovery confirms GetVersion before ListSessions/default
open. `capture_connect` exposes typed policy/self-check mismatch metadata; public message
parsing is not routing evidence. `capture_with_phase` exposes final dispatch phase independently
of transport fault classification, including Completed create followed by E_ABORT enrichment.

Unproven mixed identity retains the opened native session; resolved_session is None and
crossing/reopen refuses. Stream startup has an explicit MTA success/error handshake; exec
reader startup failure after creation reports unknown outcome without issuing another Exec.
Pull terminal results use reliable channel send; only progress callbacks remain best-effort.

Compatible aliases deduplicate only within the same destructor category. Contradictory tags
are rejected and quarantined (neither destructor can safely be selected for that invalid
union output). Socket cancellation test signals cancellation only after actual
ERROR_IO_PENDING admission; no timing sleep is used to guess pending state.
