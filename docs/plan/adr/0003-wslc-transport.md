# ADR-0003: WSLC via native COM (internal session API) with CLI fallback

- **Status:** accepted · 2026-10-01 (revised the same day: originally "CLI adapter only")
- **Validated:** spike S-3 on WSL 3.0.1 (2026-10-02). See [spike report](../spikes/2026-10-feasibility.md) F-6…F-8. COM from Rust works (list, stats, logs, exec + TTY resize).

## Context

WSL containers (WSLC, GA September 2026, WSL ≥ 2.9.3) run `containerd` + `dockerd` inside a per-user
session VM. `wslservice.exe` hosts the COM class `WSLCSessionManager`
(CLSID `a9b7a1b9-0671-405c-95f1-e0612cb4ce8f`), which brokers a per-user `wslcsession.exe` that
implements `IWSLCSession`. No Docker API endpoint is exposed to Windows.

Sources: `microsoft/WSL` @ `c34e75a` (`src/windows/service/inc/wslc.idl`, `WSLCCompat.idl`,
`src/windows/WslcSDK/`, `src/windows/wslc/services/SessionService.cpp`,
`src/windows/service/exe/WSLCSessionManager.{h,cpp}`), and the devblogs posts "WSL container is now
available for public preview" and "WSLC Architecture deep dive".

The user asked for an SDK-based integration instead of calling a command-line tool. Three
programmatic surfaces exist:

| Option | Surface | Can list existing containers, volumes, networks? | Can attach to the user's default session? | Logs / stats / events / TTY resize | ABI stability |
|---|---|---|---|---|---|
| A | **Public SDK** (`wslcsdk.dll`, NuGet `Microsoft.WSL.Containers`; built on `IWSLCCompat*`) | ✘ (images only) | ✘ (`WslcCreateSession` always creates; an existing name → `ERROR_ALREADY_EXISTS`) | ✘ | Stable |
| B | **Internal COM** (`IWSLCSessionManager` / `IWSLCSession` / `IWSLCContainer` / `IWSLCProcess` / `IWSLCEventStream`, from `wslc.idl`); this is what `wslc.exe` uses | ✔ | ✔ (`ListSessions`, `OpenSessionByName`) | ✔ | **Unstable by design**: "ABI breaking changes in this file are OK, since both client & server always ship together" |
| C | **CLI** (`wslc.exe --format json`) | ✔ | ✔ | ✔ (ConPTY for TTY) | De facto stable (user-facing) |

Option A can't fulfil the product: Dockering must show the containers the user runs with `wslc`.

## Decision

Use **B as the primary transport and C as an automatic fallback**, behind one `WslcEngine`
(spec 20 §5):

- The COM client is written in Rust with the `windows` crate. Vtables are hand-declared from a vendored `wslc.idl`, one **ABI module per verified WSL version range**.
- At connect: read the WSL version **without COM** (`wslservice.exe` file version), select an ABI module, self-check through it (`GetVersion`, `ListSessions`), then use COM. Otherwise use the CLI. The user can force either in Settings.
- An unverified vtable is never called.
- COM threading lives in `dk-engine-wslc::com`: a small MTA RPC pool for short calls, plus a dedicated MTA thread per long-lived stream. `CoInitializeSecurity(IMPERSONATE, EOAC_STATIC_CLOAKING)` and `WSAStartup` run in `main()` before GPUI (otherwise `RPC_E_TOO_LATE`), and `CoSetProxyBlanket(IMPERSONATE)` is set on each proxy, mirroring `wslc.exe`.
- A weekly CI job diffs the IDL of the latest WSL release against the vendored copies and opens an issue on change.

## Consequences

**Positive**

- No process spawning per call. Latency is that of a local RPC.
- Full fidelity: native events stream, logs via pipe handles, a real TTY with `ResizeTty` (no ConPTY), and structured pull progress via a Rust-implemented `IProgressCallback`.
- Attaches to the user's real default session, so the containers created with `wslc` are visible.

**Negative / risks**

- We depend on an interface Microsoft explicitly may break. Mitigation: version gating, self-check, automatic CLI fallback, and the weekly ABI diff. The worst case is degraded mode, never a crash or undefined behaviour.
- There are two transports to maintain and test. Mitigation: both sit behind the same `Engine` contract suite. The fake in-process COM server tests our vtable/struct handling and HRESULT mapping (not marshalling). Marshalling is covered by the self-hosted `wsl` runner against real WSLC.
- `unsafe` FFI in `dk-engine-wslc`. Each `unsafe` block is confined to `com/abi/*` and `com/ffi.rs`, with `// SAFETY:` comments and reviewer sign-off.
- A real-WSL validation run is needed per new ABI module (release checklist).

## Revisit when

- Microsoft extends the stable `IWSLCCompat` / public SDK with list, logs, stats, and events, plus opening an existing session. Then switch the primary transport to the public SDK (C API via `windows`/bindgen) and drop the internal ABI modules.
- Or WSLC exposes a Docker-compatible endpoint. Then reuse `DockerEngine`.
