# 20 — Engine Backends

Dockering talks to engines through one internal trait (`Engine`, see
[21-engine-api-contract.md](21-engine-api-contract.md)). This document describes each
**engine kind**: how it's discovered and connected, which transport it uses, and its quirks.

## 1. Engine kinds

| Kind | Implementation | Transport | OS |
|---|---|---|---|
| `docker` (local) | `DockerEngine` (bollard) | Unix socket / Windows named pipe | all |
| `docker` (remote) | `DockerEngine` (bollard) | `tcp://` or `https://` with TLS client certs | all |
| `wsl-distro` | `DockerEngine` (bollard) over **WSL stdio bridge** (default) or TCP | local named pipe → `wsl.exe` → in-distro `docker.sock` | Windows |
| `apple-container` *(deferred, §7)* | `AppleContainerEngine` | XPC to `com.apple.container.apiserver`, CLI fallback | macOS 26, Apple silicon |
| `wslc` | `WslcEngine` | **Primary:** native COM calls to the WSLC session service (`IWSLCSession`, …). **Fallback:** `wslc.exe … --format json` CLI | Windows |

```rust
pub enum EngineEndpoint {
    UnixSocket { path: PathBuf },
    NamedPipe  { path: String },                       // \\.\pipe\docker_engine
    Tcp        { host: String, port: u16, tls: Option<TlsFiles> },
    WslDistro  { distro: String, mode: WslMode },       // WslMode::DialStdio | WslMode::Tcp { port }
    Wslc       { session: Option<String>, transport: WslcTransportPref }, // Auto | Com | Cli
}

pub struct EngineConfig {
    pub id: EngineId,              // stable slug, e.g. "docker-desktop", "wsl-ubuntu-22.04", "wslc-default"
    pub name: String,              // user-visible
    pub endpoint: EngineEndpoint,
    pub origin: EngineOrigin,      // Discovered | Manual
    pub enabled: bool,
}
```

## 2. Discovery (ENG-001…ENG-010)

The hub runs discovery once at startup and again only on an explicit **Rescan** (Settings → Engines, the command palette, or the first-run screen; ENG-113).
Discovery runs on the hub runtime, in parallel, with a 3 s timeout per probe. Discovered
engines are merged with manual engines by `id`. Manual entries win. A discovered engine that
disappears stays listed as *unavailable* until the user removes it or it reappears.

| ID | Source | OS | Probe |
|---|---|---|---|
| ENG-001 | `DOCKER_HOST` env var | all | Parse the URL into an endpoint. Name: "DOCKER_HOST". |
| ENG-002 | Docker contexts: `~/.docker/config.json` → `currentContext`, `~/.docker/contexts/meta/*/meta.json` | all | Read files directly, no `docker` CLI needed. Import `unix://`, `npipe://`, and `tcp://` hosts. Skip `ssh://` in v1. |
| ENG-003 | Default local socket `/var/run/docker.sock` | Linux, macOS | `GET /_ping` |
| ENG-004 | Rootless socket `$XDG_RUNTIME_DIR/docker.sock` | Linux | `GET /_ping` |
| ENG-005 | Docker Desktop / Colima / OrbStack / Rancher / Podman sockets: `~/.docker/run/docker.sock`, `~/.colima/default/docker.sock`, `~/.orbstack/run/docker.sock`, `~/.rd/docker.sock`, `$XDG_RUNTIME_DIR/podman/podman.sock` | macOS, Linux | `GET /_ping`, then name from `/info.Name` / `OperatingSystem` |
| ENG-006 | Named pipe `\\.\pipe\docker_engine` (Docker Desktop / Docker CE for Windows) and `\\.\pipe\dockerDesktopLinuxEngine` | Windows | Pipe exists → `GET /_ping` |
| ENG-007 | WSL distros (see §4.2) | Windows | Registry enumeration + per-distro probe |
| ENG-008 | WSLC (see §5.2) | Windows | WSL version from `wslservice.exe` (no COM) ≥ 2.9.3, plus the COM class registered or `wslc.exe` present |

Transport selection for WSLC (ENG-013) is specified in §5.3.

**De-duplication (ENG-009, revised 2026-10-04).** Discovery de-duplicates by canonical endpoint string (for example, a context that points at
the default socket). After a successful connect the hub learns the **daemon identity** (`/info.ID`). Engines that share one are **annotated, never hidden** (ENG-114):
each lists the others as "Same daemon as: …" (for example, a WSL distro with Docker Desktop's WSL integration, spike F-4, or Docker Desktop's two pipes
`\\.\pipe\docker_engine` and `\\.\pipe\dockerDesktopLinuxEngine`, which report the same ID). The earlier behaviour (hide the lower-ranked engine as *merged*, plus an *Un-merge* setting)
made an engine vanish as soon as another engine became active, and a rescan didn't bring it back.

**Unsupported endpoints (ENG-010).** Docker contexts with `ssh://` hosts, and engines with API < 1.41, are listed
greyed out as *unsupported* with the reason. They're never silently dropped.

## 3. Docker Engine API backend (`dk-engine-docker`)

- Built on `bollard` 0.21 (`Docker::connect_with_unix`, `connect_with_named_pipe`, `connect_with_http`, `connect_with_ssl`).
- **API version negotiation**: call `/version` after connecting. Use `min(server.ApiVersion, CLIENT_MAX)` (`CLIENT_MAX` = 1.53 with bollard 0.21) and require `>= 1.41` (Docker 20.10). Older engines show as *unsupported version*.
- **Timeouts**: request/response calls get 30 s. Streams (events, logs, stats, exec attach) have no timeout but are cancellable.
- **Health**: `GET /_ping` every 10 s while connected, and immediately after a stream error.
- Podman's Docker-compatible socket is best-effort: the same backend, with capabilities trimmed by version probing.

## 4. WSL distro backend (`dk-wsl` + `DockerEngine`)

### 4.1 Problem

A Docker Engine running *inside* a WSL2 distro listens on `/var/run/docker.sock` within the
distro's Linux VM. Windows processes can't open that socket directly.

### 4.2 Discovery (ENG-007)

1. Enumerate distros from the registry: `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss\{GUID}` → `DistributionName`, `Version` (2 only), `State`. This avoids parsing `wsl.exe -l -v`, which prints UTF-16LE with locale-dependent headers. Fallback: `wsl.exe --list --verbose`, decoded as UTF-16LE.
2. Skip `docker-desktop` and `docker-desktop-data`. Docker Desktop is covered by ENG-006.
3. Check whether the distro is running: `wsl.exe --list --running --quiet`.
4. For each **running** distro, probe with **one** spawn: `wsl.exe -d <distro> --exec sh -c '<probe>'`. The script checks the socket, the bridge tool (`docker` or `socat`), and socket access, and reports the result as its exit code:

   | Exit | Result |
   |---|---|
   | 0 | `available` |
   | 11 | `no-docker` (no socket) |
   | 12 | `socket-no-client` (socket, but neither `docker` nor `socat`) |
   | 13 | `permission-denied` (socket not readable/writable by the user, §4.3) |

5. **Stopped** distros are not probed, because probing would boot them. They're listed as *"Ubuntu (stopped) — Start & connect"*. Connecting starts the distro.
6. Results: `available` (socket and bridge tool present), `no-docker` (hidden by default, shown with "Show all WSL distros"), `socket-no-client` (offer the TCP mode or suggest `apt install socat`), `permission-denied` (hint in §4.3).

All `wsl.exe` invocations use `CREATE_NO_WINDOW` and a 10 s timeout. stdout is decoded as
UTF-8 for `--exec` and as UTF-16LE for `--list`. `wsl.exe`'s **own** error messages (for example
`WSL_E_DISTRO_NOT_FOUND`) are UTF-16LE even with `--exec`. The runner detects them (NUL-interleaved
bytes) and decodes them as UTF-16LE.

### 4.3 Transport modes

**Mode A — stdio bridge (default, ENG-011).** No configuration is needed inside the distro.

```
bollard ──npipe──▶ \\.\pipe\dockering-wsl-<distro>-<rand>   (server owned by dk-wsl, ACL: current user only)
                         │  per accepted pipe connection:
                         ▼
             wsl.exe -d <distro> --exec docker system dial-stdio      (preferred)
             wsl.exe -d <distro> --exec socat - UNIX-CONNECT:/var/run/docker.sock   (fallback)
                         │  stdin/stdout ⇄ pipe bytes (tokio::io::copy_bidirectional)
                         ▼
                 /var/run/docker.sock inside the distro
```

- `dk-wsl::PipeBridge` is a tokio named-pipe server loop. Each client connection spawns one `wsl.exe` child and pumps bytes both ways. When either side closes, the bridge kills the child.
- `bollard::Docker::connect_with_named_pipe(bridge_path, …)` is then used unchanged. The whole Docker backend, including hijacked exec, works as is.
- Cost: one `wsl.exe` process per HTTP connection, with about 100–300 ms startup. hyper keeps connections alive, so request/response traffic reuses a few pooled connections. Long-lived streams each hold one connection (events, plus one per open logs/stats/terminal view).
- The pipe name is randomised per app run. A pipe security descriptor restricts access to the current user's SID.
- If the user isn't in the in-distro `docker` group, the probe returns `permission denied`. The hint suggests `sudo usermod -aG docker $USER` followed by `wsl --terminate <distro>`.

**Mode B — TCP (ENG-012, opt-in).** For users who expose dockerd on TCP inside the distro, for
example `"hosts": ["unix:///var/run/docker.sock", "tcp://127.0.0.1:2375"]` in `daemon.json`.
WSL2 localhost forwarding makes `127.0.0.1:<port>` reachable from Windows. Dockering connects
with `connect_with_http`. The UI warns that unauthenticated TCP is reachable by any local
process, and supports TLS certificates.

### 4.4 Lifecycle

- WSL shuts a distro down after it's idle, but an open bridge child keeps the distro alive. **v1:** when the user switches away from a WSL distro engine, the hub drops that engine. That stops the bridge and kills its `wsl.exe` children immediately, so the distro can shut down when idle.
- *Follow-up:* a 60 s grace period (keep the bridge for quick switches back) is implemented in `PipeBridge::set_active`, but the hub doesn't call it yet.
- If the distro is terminated externally, the bridge child exits and requests fail with `Unreachable`. The supervisor (§6) reconnects with backoff, and reconnecting boots the distro again. The status shows *"Distro stopped"* with a **Start** button rather than looping forever: after 3 failures the supervisor stops auto-reconnecting for WSL engines.

## 5. WSLC backend (`dk-engine-wslc`)

> **Status: implemented repair (2026-10-04).** [WSLC integration repair](../plan/features/wslc-integration-repair.md) completion records ENG-126…136, actual code/tests and final Windows evidence: workspace PASS (WSLC 183 pass/18 live opt-in ignores, hub 77/UI 243), expanded four-Run normal-user acceptance PASS on service FileVersion **3.0.1.0**, reviewer approve. The earlier broad ABI-range/whole-delegate selection/blind retries are historical, not current behavior. **Cross-OS release gate remains open:** Linux cross-check blocked by missing compiler for ring; macOS not run. Native CreateContainer remains separately unverified/disabled; no three-OS green or full DoD claim.

### 5.1 Background (research 2026-10-01, `microsoft/WSL` @ `c34e75a`)

- WSL containers (WSLC) are GA as of 2026-09-29 and need WSL ≥ 2.9.3. The user-facing CLI is `wslc.exe` (alias `container.exe`).
- **Architecture.** `wslc.exe` / SDK → COM → `wslservice.exe` (SYSTEM) hosts the **`WSLCSessionManager`** COM class (CLSID `a9b7a1b9-0671-405c-95f1-e0612cb4ce8f`, `CLSCTX_LOCAL_SERVER`). It brokers a per-user `wslcsession.exe` that implements `IWSLCSession`, and that process drives a **per-session utility VM** running `containerd` and `dockerd`. Session state lives in a VHD under `%LOCALAPPDATA%\wslc\sessions`.
- **No Docker API endpoint is exposed to Windows.** `wslcsession.exe` reaches `/var/run/docker.sock` inside the VM privately.
- The COM server implements **two interface families** on the same class:

| Interface family | IDL | Stability | Used by | What it can do |
|---|---|---|---|---|
| `IWSLCCompat*` (`IWSLCCompatSessionManager` `279F2047-…`, `IWSLCCompatSession` `DD7B2EF9-…`, `IWSLCCompatContainer`, `IWSLCCompatProcess`) | `src/windows/service/inc/WSLCCompat.idl` | **Stable**: "Changes in this file must maintain backwards compatibility". Has `IsClientVersionSupported`. | Public SDK `wslcsdk.dll` (NuGet `Microsoft.WSL.Containers`, C / C++ / C# WinRT) | Create a **new** session; pull, list, delete, and tag images; create and open a container by name; start, stop, delete, inspect, and exec. **No** list containers / volumes / networks, logs, stats, events, prune, or TTY resize. Cannot list or open sessions other than the one it creates. |
| `IWSLC*` (`IWSLCSessionManager` `82A7ABC8-6B50-43FC-AB96-15FBBE7E8760`, `IWSLCSession` `EF0661E4-6364-40EA-B433-E2FDF11F3519`, `IWSLCContainer` `7577FE8D-DE85-471E-B870-11669986F332`, `IWSLCProcess` `1AD163CD-393D-4B33-83A2-8A3F3F23E608`, `IWSLCEventStream` `7EC66D3B-D098-4D48-B69E-69166F6C4745`) | `src/windows/service/inc/wslc.idl` | **Internal**: "ABI breaking changes in this file are OK, since both client & server always ship together". Proxy/stubs are registered machine-wide by the WSL MSI. | `wslc.exe` itself | **Everything**: `ListSessions`, `OpenSessionByName`, `ListContainers`, `ListImages`, `InspectImage`, `ListVolumes`, `ListNetworks` (+ inspect / prune / delete), `GetEvents` (docker-events mirror), `IWSLCContainer::Stats/Logs/Inspect/Exec/Kill/Restart`, `IWSLCProcess::ResizeTty`, `BeginContainerOperation` |

- **Why the public SDK can't power Dockering.** A management UI needs to see the containers the user created with `wslc` in their **default session**. The SDK can only *create* sessions. Its `WslcCreateSession` always passes `WSLCSessionFlagsNone`, so naming an existing session fails with `ERROR_ALREADY_EXISTS`. It also lacks list, logs, stats, and events. It's designed for apps that *embed* their own private containers.

**Architecture (ADR-0003 direction; implemented 2026-10-04):** the factory returns one private `router::WslcEngine` behind `Arc<dyn Engine>` (ENG-126), owning `WslcComEngine` / `WslcCliEngine` delegates. Transport details stay inside the backend crate; public trait signatures are unchanged.

1. **Primary: native COM.** Direct calls to verified `IWSLC*` operations through the `windows` crate, without child processes.
2. **Fallback: CLI.** Existing argv-based `wslc.exe` commands, JSON where supported and verified text where not. Selected per operation in Auto (§5.3), not as a blanket response to any COM error. Lazy CLI preparation MUST allow healthy COM operations to work when CLI is missing.

### 5.2 Discovery (ENG-008)

1. **Detect without COM.** Read the WSL version from the file version of `%ProgramFiles%\WSL\wslservice.exe`, falling back to `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss\MSI\Version` (spike F-9). Check that CLSID `a9b7a1b9-…ce8f` is registered and that `wslc.exe` exists. If the version is below 2.9.3 or nothing is registered, WSLC is absent: show the hint "WSL containers available with `wsl --update`".
2. A COM failure of `WSLC_E_CONTAINER_DISABLED` (`0x8004060C`) means WSLC is disabled by Group Policy. Show it as *disabled by policy*; never fall back to CLI, **including during session enumeration** (ENG-135). Keep the default engine visible; connection surfaces the policy hint. Do not invent a new discovery return type.
3. `ListSessions` enumerates sessions only through an exact trusted ABI module selected before internal activation (§5.3). CLI offers `wslc system session list` (table columns `ID`, `Creator PID`, `Display Name`, verified on 3.0.1), but discovery does **not** invoke it: no creator SID means no ownership proof for extras. Unknown ABI or failed COM enumeration yields default-only discovery, including policy failures with no CLI invocation. Observed normal/elevated default naming is not ownership proof or a target-name synthesis rule.
4. Create one engine for the **default session** (`OpenSessionByName(NULL)` resolves the caller's default). Additional sessions appear with "Show all WSLC sessions" only when the creator SID and caller SID are present, valid and equal (ENG-109/135). Missing/invalid SID MUST fail closed. With CLI-only enumeration, list default only until a verified ownership mechanism exists. Preserve already-listed registry entries under ENG-114; a routing/trust failure MUST NOT silently remove an engine.

### 5.3 Transport selection & version gating (ENG-013)

- **ENG-013 / ENG-132:** `com/abi/` modules contain hand-written `#[windows::core::interface]` vtables/structs from a pinned WSL IDL tag. Trust is an explicit **exact-version allowlist**, not a major/minor range. Current entry: **service FileVersion `3.0.1.0` → `v3_0`, IDL tag `3.0.1` only**. Unknown patch/build/version MUST NOT activate internal interfaces. Historical `3.0.0..=3.0.x` selection is removed; byte-identical IDL for other releases does not admit them. MSI fallback can establish presence but cannot authorize native activation when service FileVersion is unreadable; selection is rechecked on the MTA activation worker.
- Select from non-COM version (§5.2) **before activation**, including discovery. Then self-check through the selected module: `GetVersion` major/minor/revision equals the file version triple, sane session count/NUL-terminated names, and successful target-session open. `GetVersion` does not attest the fourth FileVersion component. Self-check confirms trust; it does not make an unknown ABI safe. Revalidate non-COM version before reconnect/reopen after a WSL update.
- Preferences are stored in `EngineEndpoint::Wslc.transport`, chosen in Add engine (ENG-105); discovered engines use Auto and existing engines' preferences remain uneditable in v1:

  | Preference | Required behavior |
  |---|---|
  | Auto | Trusted COM first per verified operation. Unknown/unavailable ABI or eligible activation/self-check transport failure may select CLI. Known unverified native Run goes to CLI before dispatch. Domain, policy, authorization and cancellation errors are final. |
  | COM | Strict COM-only for this connection: no CLI construction, probe, spawn or fallback. Unknown ABI or failed connect returns error; native Run remains descriptive 501. |
  | CLI | CLI-only for this connection: no internal COM activation or self-check. |

- `E_NOINTERFACE` may indicate unavailable COM; an unchanged IID is not ABI evidence. Recognized self-check failure may select Auto CLI but MUST NOT authorize further native calls. Do not catch arbitrary 500/501 errors as transport availability.
- **Exact target (ENG-127):** keep configured identity separate from resolved target. Resolve/pin exact name, runtime u32 session ID, creator PID/SID; validate one matching owned row, and reopen that target under the same caller context. CLI uses `--session <name>` before the subcommand; revalidate before crossing/spawn after permit waits, on every stats poll and inside queued exec spawn after inspect. Do not re-resolve NULL on reopen, synthesize USERNAME defaults, create a session or elevate. Unproven identity preserves opened native proxy but refuses crossing/reopen; explicit name contradiction is final. CLI-only default retains caller-default behavior. **Residual race:** no atomic CLI identity-check-and-dispatch API exists; replacement after last validation cannot be excluded. No durable UUID/atomic guarantee is claimed.
- **Read fallback (ENG-129):** one COM read, at most one same-session reopen/read for an allowlisted disconnect fault, then at most one CLI read. Preserve typed original HRESULT/class before public error mapping; never classify by message/hint parsing. Policy/access/elevation, missing target/resource, conflict, validation, cancellation and malformed payload/protocol errors MUST NOT trigger fallback. A persistent operation fault makes that route CLI-sticky until reconnect; the native Run exception does not demote unrelated operations.
- **Mutation fallback (ENG-130):** only a proven predispatch transport preparation failure may select CLI in Auto, with equivalent target/options. Once a mutating RPC is entered or CLI child spawned, outcome may be committed: no automatic repeat. Reconcile by bounded reads where meaningful; otherwise return existing `Unreachable { reason, hint }` explicitly saying outcome is unknown and to refresh before retry. Successful mutation plus failed enrichment retries the read only. Do not fabricate IDs, deletion/prune reports, signals or reclaimed bytes. Timeout/cancellation or a negative inspect is not proof that an in-flight create cannot commit later.
- **Implemented reconciliation:** 5 s bounded reads can establish start/stop on the same full ID, primary container removal without ancillary volume deletion, volume/network absence, or inspect recovery using a successful CreateVolume's returned name. Restart/kill/tag/ambiguous creates/pull/exec and lost reports/Run IDs remain unknown where completion cannot be proved; no generalized postcondition reconstruction is claimed.
- **Streams (ENG-131):** logs/stats/events may fall back before the first source item on classified transport failure; cancel the old producer before starting CLI. Track source activity before filtering. After the first item a transport fault ends the subscription, without source splicing; explicit resubscription/refetch uses current route. Clean EOF is not a fallback trigger. Events-lost remains a gap/refetch signal, not a transport switch. Pull additionally requires proven predispatch failure, even with zero progress; exec MUST NOT recreate a process after a possibly dispatched call. Live TerminalSession methods remain transport-pinned.
- **Safety.** Unverified vtables are never called; mismatched ABI can be undefined behavior. Operation-specific trust is separate from module trust, initially disabling native CreateContainer. IDL review and real marshalling/contract evidence are required to admit a new exact version (§5.7).
- Primary/mixed/degraded route reasons and WSL version appear in existing tooltip/chip/Diagnostics metadata (ENG-110/136; §5.6).

### 5.4 COM transport (`WslcComTransport`)

**Threading.** COM calls are blocking RPCs. The COM machinery lives in `dk-engine-wslc::com` (not in `dk-hub`):
- **Short RPCs** (list, inspect, actions, `Stats()` polls) run on a small **RPC pool** of 3 OS threads, each `CoInitializeEx(COINIT_MULTITHREADED)`.
- **Long-lived blocking streams** each get a **dedicated thread** (MTA): the events `GetNext` loop, each logs pipe reader, and each exec TTY reader/writer. This way a detail page with events + logs + terminal can't starve the RPC pool. Cancellation: signal the cancel event (`GetNext`) or `CancelIoEx` on the handle, then close it.
- Handles from `Logs`/`Exec` are **sockets** (`WSLCHandleTypeSocket`), so `WSAStartup` must have run (spike F-7). Existing `ReadFile` I/O is verified; tagged ownership uses `closesocket` for sockets and `CloseHandle` for kernel events/files. CancelIoEx is followed by completion drain on dedicated threads before freeing buffers/OVERLAPPED resources (ENG-134). Unknown/contradictory tags are rejected/quarantined rather than guessing a destructor.
- Process-wide `CoInitializeSecurity(…, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE, …, EOAC_STATIC_CLOAKING)` runs in `main()` **before GPUI starts**, mirroring `wslc.exe` (spec 10 §7; after GPUI it fails with `RPC_E_TOO_LATE`, spike F-8). It runs on a throwaway MTA thread, so `main()` first calls **`CoIncrementMTAUsage`** to keep the process MTA alive. Without it, COM is uninitialised when that thread exits, the security settings are lost, and `OpenSessionByName` later fails with `0x80070542` (`ERROR_BAD_IMPERSONATION_LEVEL`). Every obtained proxy gets `CoSetProxyBlanket(… RPC_C_IMP_LEVEL_IMPERSONATE …)`, the equivalent of WSL's `ConfigureForCOMImpersonation`. Async callers await a oneshot; the UI never touches COM.

**Lifetime.** One session proxy per resolved target is cached. Required `BeginContainerOperation` returns a checked token before the protected call; failure blocks it. Hold tokens for protected mutations and full logs/exec/events lifetime. Generic helpers do not reopen/retry mutation closures. Read retry is bounded by §5.3; ambiguous mutations use read-only reconciliation. Canceled queued RPCs are skipped; atomic admission defines whether cancellation or dispatch wins. Dispatch winning permits later completion, not rollback. MTA startup failure is explicitly reported. Pool/stream teardown does not join blocking threads on UI/hub workers. Before fallback/sticky stream replacement, native producer completion (including sibling logs readers) is awaited up to 5 s; failure prevents replacement (ENG-131/134).

**Memory.** Strings/arrays use `CoTaskMemFree` RAII; tagged sockets use `closesocket`, kernel/event/file handles use `CloseHandle`. Partial out-params are adopted before HRESULT handling; compatible aliases deduplicate by destructor category. Contradictory/unknown tags are quarantined because neither destructor can safely be inferred. Owners/buffers/OVERLAPPED/events remain alive through cancellation completion. Synthetic malformed-output tests do not prove live marshalling/leak stress.

| Engine op | COM call | Notes |
|---|---|---|
| `info` | `IWSLCSessionManager::GetVersion` + session state | No docker `/info`. `api_version = "COM ABI v3_0"` (the ABI module). `cpus` / `mem_total` are `None`. |
| `list_containers` | `IWSLCSession::ListContainers(opts{all})` → `WSLCContainerEntry[]` + `WSLCContainerPortMapping[]` | Labels, networks, and mounts are strings; parse them (Docker CLI-like `k=v,…`). |
| `inspect_container` | `OpenContainer(id)` → `IWSLCContainer::Inspect(size) → LPSTR` | Shared `inspect::container_details` adapts WSLC top-level `Ports` for summary/bindings while preserving original parsed `raw` (ENG-133, CDT-030/040). |
| start / stop / restart / kill | `IWSLCContainer::Start(flags, NULL, cb)` / `Stop(signal, timeout)` / `Restart` / `Kill(signal)` | `WSLC_STOP_TIMEOUT_DEFAULT` when no timeout is given |
| pause / unpause | — | `Unsupported(PAUSE)` |
| `remove_container` / `prune_containers` | `Delete(flags)` / `IWSLCSession::PruneContainers` | |
| `logs` | `IWSLCContainer::Logs(flags{follow,timestamps}, since, until, tail) → stdout/stderr WSLCHandle` | Tagged socket outputs read on dedicated threads; cancellation/completion and type-correct closure follow ENG-134. First-item fallback boundary follows §5.3. |
| `stats` | `IWSLCContainer::Stats() → LPSTR` (raw Docker stats JSON with cumulative counters, verified F-7) | Polled every 2 s on the RPC pool; reuses the Docker CPU % / rate normaliser in `dk-core::stats` |
| `exec` (terminal) | `IWSLCContainer::Exec(WSLCProcessOptions{tty}, StartOptions{TtyRows, TtyColumns}) → IWSLCProcess` → `GetStdHandle` / `ResizeTty` / `GetExitEvent` / `GetState` | **Real TTY with resize, no ConPTY needed** |
| `events` | `IWSLCSession::GetEvents(since, 0, filters) → IWSLCEventStream::GetNext(cancelEvent)` | Blocking pull loop on a dedicated thread; the cancel event is signalled on drop. With no `since`, pass `since = now`: `0` replays the server's whole buffered history. `WSLC_E_EVENTS_LOST` → the stream yields `Protocol("events lost")` and continues (the server resets the reader); the store does a full refetch. |
| `list_images` / `inspect_image` | `ListImages(opts)` / `InspectImage(id) → LPSTR` | |
| `pull_image` | `PullImage(image, auth, FALSE, IProgressCallback, IWarningCallback)` | **We implement `IProgressCallback`** (a COM object in Rust via `#[implement]`); structured per-layer progress. Verified live (S-3). |
| `remove_image` / `prune_images` / `tag_image` | `DeleteImage` / `PruneImages` / `TagImage` | |
| `list_volumes` / `inspect_volume` | `ListVolumes(filters) → LPSTR` / `InspectVolume → LPSTR` | JSON |
| `create_volume` / `remove_volume` / `prune_volumes` | `CreateVolume` / `DeleteVolume` / `PruneVolumes` | `remove_volume(force)`: `force` is ignored (`DeleteVolume` has no force flag). |
| `list_networks` / `inspect_network` / `remove_network` / `prune_networks` | `ListNetworks → LPSTR` (`wslc_schema::NetworkListEntry[]`) / `InspectNetwork` / `DeleteNetwork` / `PruneNetworks` | |
| `run_image` | `CreateContainer(WSLCContainerOptions)` → `IWSLCContainer::Start` is declared but **not called** | Direct/strict COM returns `Api { status: 501, message: "Run via COM is not verified for this WSL version" }` without CLI. Auto routes directly to pinned-session CLI before dispatch. Declaration appears to match IDL; native creation/layout semantics are not live verified. Separate deferred spike/evidence/approval required to enable it. |
| `top` / `image_history` / `disk_usage` | — | `Unsupported`. VOL-002 shows "—" for sizes (WSLC reports `Size: N/A`) |

**HRESULT mapping**: `WSLC_E_CONTAINER_NOT_FOUND` / `IMAGE_NOT_FOUND` / `VOLUME_NOT_FOUND` / `NETWORK_NOT_FOUND` / `SESSION_NOT_FOUND` → `NotFound`. `WSLC_E_CONTAINER_IS_RUNNING` / `NOT_RUNNING` → `Conflict`. `WSLC_E_CONTAINER_PREFIX_AMBIGUOUS` → `Conflict`. `WSLC_E_VM_NOT_RUNNING`, `RPC_E_DISCONNECTED` → `Unreachable`. `WSLC_E_CONTAINER_DISABLED` → `Unreachable{hint: policy}`. `WSLC_E_REGISTRY_BLOCKED_BY_POLICY` → `Api`. Anything else → `Api { status: 500, message: "0x<hr> <IErrorInfo text>" }`. The HRESULT goes in the message because `status` is a `u16`; the `IErrorInfo` text is included when present (the server supports `ISupportErrorInfo`).

The router retains original allowlisted HRESULT/dispatch phase and typed connect classification internally **before** this lossy public mapping. `EngineError` variants and Engine signatures remain unchanged. Generic `Unreachable` or an HRESULT formatted in a message MUST NOT authorize fallback/retry. Live COM elevation-required still surfaces generic Api 500 with exact HRESULT (unlike CLI's hint); rejection is final and never bypassed.

### 5.5 CLI fallback transport (`WslcCliTransport`)

Every invocation: `wslc.exe [--session <s>] <cmd…>`, with `CREATE_NO_WINDOW`, `NO_COLOR=1`, an argv vector, captured UTF-8 output, a timeout, and at most 4 concurrent processes (`Semaphore`). Verified on `wslc` 3.0.1 (S-2; exit codes and stderr texts are recorded as fixtures).

**Invocation rules**

| Rule | Detail |
|---|---|
| Session flag position | `--session <s>` MUST come right after `wslc`, before the subcommand. |
| No `--` | `wslc` treats `--` as an id. Argument safety relies on per-kind validation (NFR-022) only; additionally, the first exec command word MUST NOT start with `-`. |
| Line endings | Output is CRLF, except `logs` (LF). Parsers accept both. |
| Credentials | `wslc` has no auth flags; secrets MUST NOT enter argv. CLI rejects supplied `Some(auth)` before spawn; Auto cannot discard it to fall back. `None` uses WSLC registry state (IMG-007). |

**Errors.** A failed command exits with code 1. stderr is a message line, then `Error code: <SYMBOL>`, then a boilerplate line. Lookup commands such as `inspect` print only the message, with `[]` on stdout.

| `Error code` symbol (or message text) | `EngineError` |
|---|---|
| `WSLC_E_CONTAINER_DISABLED`, `WSLC_E_SESSION_NOT_FOUND`, `WSLC_E_VM_NOT_RUNNING`, `RPC_*` | as §5.4 (`Unreachable`, with a hint for policy and session) |
| `ERROR_ELEVATION_REQUIRED` (e.g. opening the admin session as a normal user) | `Unreachable { hint }`: run elevated or pick the user's own session |
| `WSLC_E_*_NOT_FOUND`, or a message containing "not found" (lookups) | `NotFound` |
| `WSLC_E_CONTAINER_IS_RUNNING` / `NOT_RUNNING` / `PREFIX_AMBIGUOUS`, `ERROR_ALREADY_EXISTS`, `ERROR_SHARING_VIOLATION` | `Conflict` |
| anything else | `Api { status: <exit code>, message: <message line> }` |

**Mapping**

| Engine op | wslc invocation | Notes |
|---|---|---|
| `info` | `system info --format json` + `version --format json` | `version` returns only `{"Client":{"Version":…}}`. `system info` gives the kernel, session-manager version, and sessions; no CPU or memory (`None`). |
| `list_containers` / `inspect_container` | `container list --all --no-trunc --format json` (**NDJSON**) / `container inspect <id>` (JSON array) | Dates may contain localized TZ names; prefer inspect timestamps. UI hides internal metadata label. Inspect has top-level `Ports`, no `Config.Tty`; shared typed normalization leaves original selected object unchanged in `raw`. Only valid empty arrays mean NotFound; malformed inspect JSON remains Protocol (ENG-129/133). |
| start / stop / restart / kill / remove | `container start|stop|restart|kill|remove …` | |
| `prune_*` | `container|image|volume|network prune --force` | Without `-f`, `prune` prompts and silently declines with no stdin, so `--force` is always passed. |
| `logs` | `container logs [-f] [--timestamps] [--tail N] [--since T] <id>` (long-running child) | `--tail 0` is rejected, so tail 0 becomes `--since now`. No `Config.Tty` → frames are always `Stdout`/`Stderr`. |
| `stats` | `container stats <id> --format json`: one-shot | Docker **CLI** strings (`CPUPerc`, `MemUsage`, `NetIO`, `BlockIO`, `PIDs`), not Docker API JSON; parsed into `StatsSample`. ~1 s per call for a running container, instant for a stopped one. The poll loop enforces ≥ 1 s between calls. Detail page only; no list columns on the CLI transport. |
| `exec` | `container exec -i -t <id> <cmd>` inside **ConPTY** (`portable-pty`); resize via `MasterPty::resize` | The pseudo console sends a cursor-position query (`ESC[6n`) and waits for an answer. The transport answers it once and strips it from the output. |
| `run_image` | `container run -d [--name] [-p] [-e] [-v] [-l] [--rm] <image> [cmd]` | The id is on stdout; pull progress goes to stderr. |
| images | `image list|inspect|pull|remove|tag …` | Pull progress is text only. `Digest: sha256:…` gives the digest. |
| `list_volumes` | `volume list --format json` + one batch `volume inspect <n…>` | The list has no creation date, and `Size`/`Links` are `N/A`; created dates come from the batch `inspect`. |
| `create_volume` | `volume create [--driver d] <n>` | The only drivers are `guest` and `vhd`; `local` is rejected. `local` (the default in the UI) is treated as "default" and not passed. |
| `remove_volume` | `volume remove <n>` | |
| `list_networks` | `network list --format json` + one batch `network inspect <n…>` | Enriched with subnets and container count. `CreatedAt` looks like `… +0000 UTC`; booleans are strings (`"true"`). |
| `remove_network` | `network remove <name>` | `remove` with an **id** fails with "not found" (`inspect` accepts ids), so the transport resolves the name first. |
| `events` | `system events [--since T] [--until T] [--filter k=v]` (long-running child) | **No JSON option in 3.0.1**: parse text lines `<RFC3339 ts> <type> <action> <id> (k=v, …)` (F-10). The id can be empty. Container and network events were seen; none were seen for images or volumes. |

Parsers are tolerant: unknown fields are ignored, and human-formatted sizes and times are parsed with fallbacks.

**Option-parity gate (ENG-130/131).** Mixed volume-prune fallback is disabled (COM all-unused vs CLI anonymous-only). Mixed create-volume fallback with explicit `local` driver is refused (CLI substitutes default, COM forwards driver); supplied pull auth is rejected by CLI before spawn. CLI-only prune/driver behavior remains explicit. Request filters/options MUST NOT silently weaken. CLI child timeout/lost response is an unknown mutation outcome, never automatic replay. CLI ping tests session-bound container health, not only `version`.

### 5.6 Capabilities by transport

| Capability | COM | CLI |
|---|---|---|
| EVENTS, LOGS_FOLLOW, NETWORK_MGMT, EXEC_TTY, EXEC_RESIZE | ✔ | ✔ |
| PULL_PROGRESS (structured) | ✔ | ✘ |
| STATS_STREAM (native) | ✘ (polled) | ✘ (polled) |
| PAUSE, TOP, IMAGE_HISTORY, DISK_USAGE | ✘ | ✘ |

**Router metadata (ENG-136).** Capabilities/info come from one coherent operation-route snapshot. `PULL_PROGRESS` follows future pull routes; CLI stats use `list_stats_limit = 0`. Run's CLI exception does not demote COM pull/stats. `transport = "com"` means COM-primary, with mixed/degraded note (e.g. "COM primary; Run uses CLI — native Run unverified"); strict COM note is "COM only; native Run unverified". Degraded names are sorted/deduplicated; CLI-only uses `"cli"`. No last-call transport label.

Full EngineInfo refresh runs after successful active health ping under a bounded timeout outside registry locks, current-connection guarded. Existing StatusChanged publishes metadata changes even with unchanged flags; CapabilitiesChanged remains for real flag changes. Info-only failure preserves prior snapshot without degrading a healthy engine. Local EngineListEvent::InfoChanged/EngineStore::apply_info updates active store, Settings, status bar and Diagnostics without reconnect/remount/focus movement. No new Engine method or public hub event was added.

### 5.7 Keeping up with WSL releases

- `crates/dk-engine-wslc/idl/<wsl-tag>/{wslc.idl,WSLCShared.idl}` are vendored copies. `xtask wslc-abi-check <tag>` diffs a new WSL tag's IDL against the newest vendored one and reports changed vtables and structs.
- A scheduled CI job (weekly) checks the latest release IDL and opens an issue on change. Until its exact version is admitted, Auto uses CLI if available; forced COM refuses. CLI availability/compatibility must still be reported accurately, not promised universally.
- Admitting a new exact version requires pinned IDL/vtable/struct review plus real Windows marshalling/contract evidence for that version. Byte-identical IDL alone is insufficient. Maintain per-operation evidence, including disabled native Run; fake COM tests cannot establish out-of-process marshalling.
- **Historical baseline (2026-10-02):** vendored `3.0.1` IDL is byte-identical to `3.0.0`/`2.9.13`; `2.9.5` differs. Broad `3.0.x` code trust was removed on 2026-10-04. Current allowlist is **`3.0.1.0` only** using `v3_0`; other patch/build versions remain untrusted until admitted. IDL equality does not prove native Run.

### 5.8 Spikes

- **S-2 (CLI):** ✅ done. Real `wslc` 3.0.1 output, exit codes, and stderr texts are recorded as fixtures (§5.5).
- **S-3 (COM):** prior live evidence covers session manager/default open, ListContainers, Logs (socket ReadFile), Exec+ResizeTty, GetEvents and PullImage callbacks from a non-WSL-signed process. **CreateContainer remains unverified live**; declaration/IDL agreement is not enablement evidence (§5.4).
- **Final repair evidence (2026-10-04):** Windows workspace passed (WSLC 183/18 live opt-in ignores, hub 77/UI 243), clippy/fmt/check-blocking passed. Expanded maintained `repair_live` harness passed 1 in 20.03 s: default Auto, explicit Auto/CLI and pinned raw CLI Runs, both typed inspect/log paths Exited/0 `/hello`/Hello from Docker, six preference paths, strict COM 501, admin rejection without elevation, four own containers removed and image retained. See [plan completion](../plan/features/wslc-integration-repair.md) for full IDs/provenance; initial external probe and smaller counters are historical.
- **Deferred/release limits:** native CreateContainer spike remains separately approved/evidence-only before enablement. Linux compiler provisioning/macOS validation remain release gates; live replacement/fault/marshalling stress and nonempty-baseline cleanup preservation are not claimed.

## 6. Connection supervisor (ENG-020…ENG-025)

Per-engine state machine in `dk-hub`:

```
          connect()            ping ok
 Disconnected ───────▶ Connecting ───────▶ Connected ◀──┐
      ▲                    │ fail              │ error  │ ok
      │ user disables      ▼                   ▼        │
      └──────────── Failed{retry_at} ◀──── Degraded ────┘ (ping retry)
```

- **ENG-020** Only the **active** engine stays continuously connected. Other enabled engines get a lightweight status check to show status dots in the switcher: `_ping` every 30 s for local/remote Docker; for **WSL distros** a running-state check via `wsl.exe --list --running --quiet` / the registry every 30 s (**no** Docker ping, so an idle distro can still shut down, §4.4); for WSLC a non-COM version check plus whether `wslcsession.exe` is running, every 60 s. Stopped distros are never contacted.
- **ENG-021** Reconnect uses exponential backoff of 1 s, 2 s, 4 s … capped at 30 s, plus jitter. A manual "Retry" resets the backoff.
- **ENG-022** On reconnect, the `EngineStore` resubscribes to events and refetches everything.
- **ENG-023** `EngineStatus` is broadcast on `hub_events()`: `{ id, state, version, api_version, os, arch, error }`.
- **ENG-024** Capabilities are computed after a successful connect and are part of `EngineInfo`.
- **ENG-025** Engines can be enabled or disabled. Disabled engines are never contacted.

## 7. Future backend: Apple `container` (macOS native), **deferred, not in v1**

> Status: **researched, deferred**. Nothing in this section is implemented in v1. The architecture
> MUST keep this backend possible (ENG-030…ENG-033). See [ADR-0005](../plan/adr/0005-backend-extensibility-apple-container.md)
> and [features/macos-native-engine.md](features/macos-native-engine.md).

### 7.1 Background (research 2026-10-01, `apple/container` @ `0a48a1b`, latest release 1.5.0)

- Apple's open-source `container` runs Linux containers on macOS, with **one lightweight VM per container** (Virtualization.framework + vmnet). It's written in Swift on top of the `Containerization` package. It requires **Apple silicon** and is supported on **macOS 26** (it runs on 15 with networking limits, and has no multi-network support on 15).
- **Architecture.** The `container` CLI → XPC → the launch agent **`container-apiserver`** (mach service `com.apple.container.apiserver`, started by `container system start`). That agent spawns XPC helpers: `container-core-images` (`com.apple.container.core.container-core-images`, image store), `container-network-vmnet`, and one `container-runtime-linux` per container.
- **No Docker Engine API** is exposed (no `docker.sock` and no HTTP endpoint), and there's **no Compose support**.
- **Programmatic surfaces:**

| Surface | Stability (from `apple/container` README) | Notes |
|---|---|---|
| `container-apiserver` **XPC API** | "preserves forward and backward compatibility **within a major version**" | Routes include `containerList`, `containerCreate`, `containerStop`, `containerKill`, `containerDelete`, `containerLogs` (returns file descriptors), `containerStats`, `containerCreateProcess` / `containerStartProcess` / `containerResize` (exec + TTY), `containerEvent`, `containerDiskUsage`, `networkList/Create/Delete`, `volumeList/Create/Delete/Inspect`, `systemDiskUsage`, `ping`. Messages are XPC dictionaries with a `com.apple.container.xpc.route` key and **JSON-encoded `Codable` payloads** (`ContainerSnapshot`, `ContainerStats`, …). The server rejects callers whose EUID differs from its own; we run as the same user, so that's fine. |
| Image helper XPC (`container-core-images`) | "non-public XPC helpers do not guarantee … API compatibility" | Image list, pull, delete, and tag go through this helper. |
| Swift client libraries (`ContainerAPIClient`, `ContainerImagesServiceClient`, …) | Tied to the release | SwiftPM products. Usable from Rust only through a Swift→C shim. |
| `container` CLI | "generally preserves backward compatibility within a major release" | `--format json` on `list`, `image list`, `network list`, `volume list`, `stats`, `system status/version`. `inspect` emits JSON. `stats --no-stream` gives a one-shot snapshot. `logs [-f] [-n N]`. `exec -it` needs a PTY. |

### 7.2 Planned transports (same pattern as WSLC, §5)

1. **Primary: native XPC** (`AppleXpcTransport`). Rust uses the `xpc_*` C API from libSystem (through the `block2` / `dispatch2` / `objc2` ecosystem or a thin FFI) to talk to `com.apple.container.apiserver`. Payloads are JSON (`serde`) mirroring Apple's `Codable` types, vendored per major version. This is gated by `container system version` / XPC `ping`, with a self-check. Image operations go through the image helper only if verified; otherwise they use the CLI.
2. **Fallback: CLI** (`AppleCliTransport`). `container … --format json`. Exec uses a PTY (`portable-pty`, which uses the Unix PTY on macOS).
3. **Not chosen:** a Swift shim library linked into Dockering. It would add a Swift toolchain to the build and couple us to Apple's internal Swift API.

### 7.3 Expected capability profile

| Capability | Apple `container` | Notes |
|---|---|---|
| EVENTS | ✔ (verify: `containerEvent` route semantics) | Otherwise poll |
| PAUSE | ✘ | No pause command |
| EXEC_TTY / EXEC_RESIZE | ✔ / ✔ | `containerResize` |
| STATS_STREAM | ✘ (polled) | `ContainerStats` has cumulative counters (cpu usec, mem, net rx/tx, block r/w, pids). The `dk-core::stats` normaliser computes rates. CPU % uses the cpu usec delta / wall time / vCPUs. |
| TOP | ✘ | |
| IMAGE_HISTORY | ✘ | |
| DISK_USAGE | ✔ | `systemDiskUsage`, `containerDiskUsage` |
| PULL_PROGRESS | ✔ (XPC progress updates) / ✘ (CLI) | |
| NETWORK_MGMT | ✔ (macOS 26) / ✘ (macOS 15) | |
| LOGS_FOLLOW | ✔ | Plus `--boot` logs, a candidate extra tab |

**Model differences the contract must absorb** (ENG-031):

- One VM per container. Each container has its own IP on the vmnet network. Published ports work differently, and there's no shared "engine host". `EngineInfo` has no kernel or storage driver, and CPU and memory are per container.
- Compose grouping works only if containers carry `com.docker.compose.project` labels (`container run -l` supports labels), so grouping (21 §4) still applies to label-based groups.
- Container machines (`container machine …`) and the experimental k8s commands are out of scope.

### 7.4 Requirements that keep the door open (apply to v1)

- **ENG-030** `EngineKind` MUST be an open enum with an `AppleContainer` variant reserved. UI code MUST NOT `match` exhaustively on engine kind for behaviour. Behaviour is driven by `Capabilities` and `EngineInfo` only.
- **ENG-031** DTOs in spec 21 MUST tolerate missing engine-wide facts. `EngineInfo` fields such as `kernel`, `storage_driver`, `root_dir`, `cpus`, and `mem_total` are `Option`. Per-container `ContainerSummary` MAY carry `ip_addresses`, since some engines have no host port mapping.
- **ENG-032** Engine backends live in separate crates registered with the hub through an `EngineFactory` (see 21 §8). Adding `dk-engine-apple` MUST NOT require changes to UI crates, beyond optional icons and strings.
- **ENG-033** Discovery is pluggable per factory. On macOS, v1 discovers only Docker-compatible sockets (ENG-003/005). A future Apple factory adds: `container` on `PATH` + `container system status --format json`, or an XPC `ping` to `com.apple.container.apiserver`.

### 7.5 Spikes (when this backend is scheduled)

- **S-6:** Rust XPC client to `com.apple.container.apiserver`: `ping`, `containerList`, `containerLogs` (fd passing), `containerStats`, exec with `containerResize`, and `containerEvent` semantics. Record the JSON payloads as fixtures.
- **S-7:** image helper XPC vs CLI for images: decide per operation.
