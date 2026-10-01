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

The hub runs discovery at startup and when the user clicks **Rescan** in the engine switcher.
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

**De-duplication (ENG-009).** Discovery first de-duplicates by canonical endpoint string (for example, a context that points at
the default socket). After the first successful connect, engines are de-duplicated by **daemon identity**
(`/info.ID`). A WSL distro whose `docker` is Docker Desktop's WSL integration reaches the same daemon as
`\.\pipe\docker_engine` (spike F-4), so it's merged into the existing engine. The tooltip lists "also reachable via: Ubuntu-22.04".
The user can un-merge it in Settings.

**Unsupported endpoints (ENG-010).** Docker contexts with `ssh://` hosts, and engines with API < 1.41, are listed
greyed out as *unsupported* with the reason. They're never silently dropped.

## 3. Docker Engine API backend (`dk-engine-docker`)

- Built on `bollard` 0.21 (`Docker::connect_with_unix`, `connect_with_named_pipe`, `connect_with_http`, `connect_with_ssl`).
- **API version negotiation**: call `/version` after connecting. Use `min(server.ApiVersion, CLIENT_MAX)` and require `>= 1.41` (Docker 20.10). Older engines show as *unsupported version*.
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
4. For each **running** distro, probe: `wsl.exe -d <distro> --exec sh -c 'test -S /var/run/docker.sock && (command -v docker || command -v socat)'`.
5. **Stopped** distros are not probed, because probing would boot them. They're listed as *"Ubuntu (stopped) — Start & connect"*. Connecting starts the distro.
6. Results: `available` (socket and bridge tool present), `no-docker` (hidden by default, shown with "Show all WSL distros"), `socket-no-client` (offer the TCP mode or suggest `apt install socat`).

All `wsl.exe` invocations use `CREATE_NO_WINDOW` and a 10 s timeout. stdout is decoded as
UTF-8 for `--exec` and as UTF-16LE for `--list`.

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

- WSL shuts a distro down after it's idle, but an open bridge child keeps the distro alive. Once the user switches away from that engine and no streams remain, the bridge closes its connections after 60 s.
- If the distro is terminated externally, the bridge child exits and requests fail with `Unreachable`. The supervisor (§6) reconnects with backoff, and reconnecting boots the distro again. The status shows *"Distro stopped"* with a **Start** button rather than looping forever: after 3 failures the supervisor stops auto-reconnecting for WSL engines.

## 5. WSLC backend (`dk-engine-wslc`)

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

**Decision (ADR-0003, revised 2026-10-01):** `WslcEngine` has two transports behind one `Engine` impl:

1. **Primary: native COM** (`WslcComTransport`). Direct calls to the `IWSLC*` interfaces through the `windows` crate. No child processes; full fidelity.
2. **Fallback: CLI** (`WslcCliTransport`). `wslc.exe … --format json`. Used automatically when the COM path is unavailable or untrusted (§5.3).

### 5.2 Discovery (ENG-008)

1. **Detect without COM.** Read the WSL version from the file version of `%ProgramFiles%\WSL\wslservice.exe`, falling back to `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss\MSI\Version` (spike F-9). Check that CLSID `a9b7a1b9-…ce8f` is registered and that `wslc.exe` exists. If the version is below 2.9.3 or nothing is registered, WSLC is absent: show the hint "WSL containers available with `wsl --update`".
2. A COM failure of `WSLC_E_CONTAINER_DISABLED` (`0x8004060C`) means WSLC is disabled by Group Policy. Show it as *disabled by policy*; never fall back to the CLI.
3. `ListSessions` enumerates the sessions (COM). CLI fallback: `wslc system session list` (table output only, with columns `ID`, `Creator PID`, `Display Name`, verified on 3.0.1). Sessions named `wslc-cli-<user>` / `wslc-cli-admin-<user>` are the CLI's default sessions for normal and elevated callers.
4. Create one engine for the **default session** (`OpenSessionByName(NULL)` resolves the caller's default). Create one engine for each other session owned by the user when "Show all WSLC sessions" is on.

### 5.3 Transport selection & version gating (ENG-013)

- `dk-engine-wslc` ships `com/abi/` modules, one per **supported WSL ABI version**: hand-written `#[windows::core::interface]` vtables, generated from `wslc.idl` at a pinned WSL tag. Each module records the WSL version range it was verified against, for example `2.9.3..=2.9.x`.
- At connect: **select the ABI module from the non-COM version** (§5.2 step 1). Only *after* selection, and only through
  the selected module, run a **self-check**: `GetVersion` must equal the file version, `ListSessions` must return a sane count with NUL-terminated names, and `OpenSessionByName(NULL)` must succeed. If all succeed, use COM.
  The self-check is a confirmation, not a safety mechanism. Safety comes from the version → module mapping, which CI validates (§5.7).
- Use the CLI when any of these happen:
  - no ABI module matches the version (newer WSL than we know);
  - `QueryInterface` for an internal IID returns `E_NOINTERFACE`. This is a hint only: IIDs are not guaranteed to change on ABI breaks;
  - the self-check fails;
  - the user set Settings → Engines → WSLC transport = *CLI*. The options are *Auto* (default), *COM*, and *CLI*.
- The chosen transport and WSL version show in the engine tooltip and in Diagnostics.
- **Safety.** Internal-ABI calls only ever run with a verified ABI module. An unverified vtable is never called, because a mismatched vtable would be undefined behaviour, not just an error. CI checks the vendored IDL against the WSL release tag (§5.7).

### 5.4 COM transport (`WslcComTransport`)

**Threading.** COM calls are blocking RPCs. The COM machinery lives in `dk-engine-wslc::com` (not in `dk-hub`):
- **Short RPCs** (list, inspect, actions, `Stats()` polls) run on a small **RPC pool**: 2–4 OS threads, each `CoInitializeEx(COINIT_MULTITHREADED)`.
- **Long-lived blocking streams** each get a **dedicated thread** (MTA): the events `GetNext` loop, each logs pipe reader, and each exec TTY reader/writer. This way a detail page with events + logs + terminal can't starve the RPC pool. Cancellation: signal the cancel event (`GetNext`) or `CancelIoEx` on the handle, then close it.
- Handles from `Logs`/`Exec` are **sockets** (`WSLCHandleTypeSocket`), so `WSAStartup` must have run (spike F-7). They're read with `ReadFile`.
- Process-wide `CoInitializeSecurity(…, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE, …, EOAC_STATIC_CLOAKING)` runs in `main()` **before GPUI starts**, mirroring `wslc.exe` (spec 10 §7; after GPUI it fails with `RPC_E_TOO_LATE`, spike F-8). Every obtained proxy gets `CoSetProxyBlanket(… RPC_C_IMP_LEVEL_IMPERSONATE …)`, the equivalent of WSL's `ConfigureForCOMImpersonation`. Async callers await a oneshot; the UI never touches COM.

**Lifetime.** One `IWSLCSession` proxy per engine is cached. Each mutation on a container holds a `BeginContainerOperation` token for its duration, so idle VM termination can't disconnect mid-operation. `RPC_E_DISCONNECTED` / `RPC_S_SERVER_UNAVAILABLE` → reopen the session once and retry; otherwise `Unreachable`.

**Memory.** Strings and arrays returned by `[out]` params are freed with `CoTaskMemFree` (via RAII wrappers). `system_handle` outputs (pipes, events) are wrapped in `OwnedHandle`.

| Engine op | COM call | Notes |
|---|---|---|
| `info` | `IWSLCSessionManager::GetVersion` + session state | No docker `/info`. CPUs and memory come from session settings where available. |
| `list_containers` | `IWSLCSession::ListContainers(opts{all})` → `WSLCContainerEntry[]` + `WSLCContainerPortMapping[]` | Labels, networks, and mounts are strings; parse them (Docker CLI-like `k=v,…`). |
| `inspect_container` | `OpenContainer(id)` → `IWSLCContainer::Inspect(size) → LPSTR` | Docker-inspect-shaped JSON (`docker_schema`) |
| start / stop / restart / kill | `IWSLCContainer::Start(flags, NULL, cb)` / `Stop(signal, timeout)` / `Restart` / `Kill(signal)` | `WSLC_STOP_TIMEOUT_DEFAULT` when no timeout is given |
| pause / unpause | — | `Unsupported(PAUSE)` |
| `remove_container` / `prune_containers` | `Delete(flags)` / `IWSLCSession::PruneContainers` | |
| `logs` | `IWSLCContainer::Logs(flags{follow,timestamps}, since, until, tail) → stdout/stderr WSLCHandle` | Pipe handles read on the COM pool, then chunks go to `EngineStream`. Close the handles to cancel. |
| `stats` | `IWSLCContainer::Stats() → LPSTR` (raw Docker stats JSON with cumulative counters, verified F-7) | Polled every 2 s on the RPC pool; reuses the Docker CPU % / rate normaliser in `dk-core::stats` |
| `exec` (terminal) | `IWSLCContainer::Exec(WSLCProcessOptions{tty}, StartOptions{TtyRows, TtyColumns}) → IWSLCProcess` → `GetStdHandle` / `ResizeTty` / `GetExitEvent` / `GetState` | **Real TTY with resize, no ConPTY needed** |
| `events` | `IWSLCSession::GetEvents(since, 0, filters) → IWSLCEventStream::GetNext(cancelEvent)` | Blocking pull loop on a COM-pool thread; the cancel event is signalled on drop. `WSLC_E_EVENTS_LOST` → full refetch. |
| `list_images` / `inspect_image` | `ListImages(opts)` / `InspectImage(id) → LPSTR` | |
| `pull_image` | `PullImage(image, auth, FALSE, IProgressCallback, IWarningCallback)` | **We implement `IProgressCallback`** (a COM object in Rust via `#[implement]`); structured per-layer progress |
| `remove_image` / `prune_images` / `tag_image` | `DeleteImage` / `PruneImages` / `TagImage` | |
| `list_volumes` / `inspect_volume` | `ListVolumes(filters) → LPSTR` / `InspectVolume → LPSTR` | JSON |
| `create_volume` / `remove_volume` / `prune_volumes` | `CreateVolume` / `DeleteVolume` / `PruneVolumes` | |
| `list_networks` / `inspect_network` / `remove_network` / `prune_networks` | `ListNetworks → LPSTR` (`wslc_schema::NetworkListEntry[]`) / `InspectNetwork` / `DeleteNetwork` / `PruneNetworks` | |
| `run_image` | `CreateContainer(WSLCContainerOptions)` → `IWSLCContainer::Start` | Ports, env, named volumes, labels. ⚠ Verify the struct layout in the M8 spike |
| `top` / `image_history` / `disk_usage` | — | `Unsupported`. VOL-002 shows "—" for sizes (WSLC reports `Size: N/A`) |

**HRESULT mapping**: `WSLC_E_CONTAINER_NOT_FOUND` / `IMAGE_NOT_FOUND` / `VOLUME_NOT_FOUND` / `NETWORK_NOT_FOUND` / `SESSION_NOT_FOUND` → `NotFound`. `WSLC_E_CONTAINER_IS_RUNNING` / `NOT_RUNNING` → `Conflict`. `WSLC_E_CONTAINER_PREFIX_AMBIGUOUS` → `Conflict`. `WSLC_E_VM_NOT_RUNNING`, `RPC_E_DISCONNECTED` → `Unreachable`. `WSLC_E_CONTAINER_DISABLED` → `Unreachable{hint: policy}`. `WSLC_E_REGISTRY_BLOCKED_BY_POLICY` → `Api`. Anything else → `Api { status: hr }` with `IErrorInfo` text when present (the server supports `ISupportErrorInfo`).

### 5.5 CLI fallback transport (`WslcCliTransport`)

Every invocation: `wslc.exe [--session <s>] <cmd…>`, with `CREATE_NO_WINDOW`, `NO_COLOR=1`, an argv vector, captured UTF-8 output, a timeout, and at most 4 concurrent processes (`Semaphore`). A non-zero exit code → `EngineError` mapped from stderr.

| Engine op | wslc invocation |
|---|---|
| `info` | `system info --format json` + `version` |
| `list_containers` / `inspect_container` | `container list --all --no-trunc --format json` (**NDJSON**, one object per line; dates contain localised TZ names such as `SELČ`, so prefer `inspect` timestamps) / `container inspect <id>` (JSON array) |
| start / stop / restart / kill / remove / prune | `container start|stop|restart|kill|remove|prune …` |
| `logs` | `container logs [-f] [--timestamps] [--tail N] [--since T] <id>` (long-running child) |
| `stats` | `container stats <id> --format json`: one-shot, **~1 s per call** (F-10). Detail page only, polled back-to-back. No list columns on the CLI transport |
| `exec` | `container exec -i -t <id> <cmd>` inside **ConPTY** (`portable-pty`); resize via `MasterPty::resize` |
| `run_image` | `container run -d [--name] [-p] [-e] [-v] [-l] [--rm] <image> [cmd]` |
| images | `image list|inspect|pull|remove|prune|tag …` (pull progress is text only) |
| volumes / networks | `volume …` / `network …` (`list --format json`, `inspect`, `create`, `remove`, `prune`) |
| `events` | `system events` (long-running child). **No JSON option in 3.0.1**, so parse text lines `<RFC3339 ts> <type> <action> <id> (k=v, …)` (F-10) |

Parsers are tolerant: unknown fields are ignored, and human-formatted sizes and times are parsed with fallbacks.

### 5.6 Capabilities by transport

| Capability | COM | CLI |
|---|---|---|
| EVENTS, LOGS_FOLLOW, NETWORK_MGMT, EXEC_TTY, EXEC_RESIZE | ✔ | ✔ |
| PULL_PROGRESS (structured) | ✔ | ✘ |
| STATS_STREAM (native) | ✘ (polled) | ✘ (polled) |
| PAUSE, TOP, IMAGE_HISTORY, DISK_USAGE | ✘ | ✘ |

Capabilities are recomputed whenever the transport changes.

### 5.7 Keeping up with WSL releases

- `crates/dk-engine-wslc/idl/<wsl-tag>/{wslc.idl,WSLCShared.idl}` are vendored copies. `xtask wslc-abi-check <tag>` diffs a new WSL tag's IDL against the newest vendored one and reports changed vtables and structs.
- A scheduled CI job (weekly) runs the check against the latest WSL release tag and opens an issue when the ABI changed. Until a new ABI module ships, users on the new WSL automatically use the CLI fallback, so nothing breaks; it only degrades.
- Releasing a new ABI module requires running the WSLC contract suite on a real Windows machine with that WSL version (release checklist).

### 5.8 Spikes

- **S-2 (CLI):** record real `wslc` JSON output for the fallback mapping; confirm `stats` one-shot semantics and the events format.
- **S-3 (COM, blocking for M8):** in Rust, `CoCreateInstance` the session manager → `OpenSessionByName(NULL)` → `ListContainers`, `Logs` (pipe read), `Exec` + `ResizeTty`, `GetEvents`, and `PullImage` with a Rust-implemented `IProgressCallback`. Confirm that the proxy/stub marshalling works from a non-WSL-signed process, and that impersonation and cloaking settings are correct.

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
