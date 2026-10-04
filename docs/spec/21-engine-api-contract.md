# 21 — Engine API Contract

This is the internal contract between the UI/hub and every backend. It lives in `dk-core`
(`dk_core::engine`, `dk_core::model`). Every DTO is an **owned, `Send + 'static`, `Clone`,
`serde`-serialisable** struct. No bollard types, WSLC COM types, or `wslc` CLI shapes leak out of the engine crates.

## 0. Layering: one contract, many backends

The UI and the hub know **only** this contract. Each container runtime is a separate crate that
implements it, and may have several *transports* inside:

```
           dockering (GPUI UI)  ──▶  dk-hub (HubHandle, stores, supervisor, StatsService)
                                              │  Arc<dyn Engine>   ◀── the only interface the app sees
            ┌──────────────────┬──────────────┼──────────────────────┬───────────────────────────┐
            ▼                  ▼              ▼                      ▼                           ▼
   dk-engine-docker     dk-engine-docker   dk-engine-wslc         dk-engine-apple (future)    FakeEngine (tests)
   socket/pipe/TCP      + dk-wsl bridge    ├ COM transport  ◀ primary    ├ XPC transport ◀ primary
   (Docker Engine API)  (WSL distros)      └ CLI transport  ◀ fallback   └ CLI transport ◀ fallback
```

Rules:
- A new runtime = a new crate that implements `Engine` and registers an `EngineFactory` (§8). Nothing in `dockering` changes except optional icons and labels (ENG-032).
- Runtime differences are expressed **only** through `Capabilities` (§2) and `Option` fields in DTOs (§3). The UI never branches on `EngineKind` for behaviour (ENG-030).
- When a runtime has several transports, they're private to its crate. The transport currently in use is reported in `EngineInfo.transport` for display and diagnostics only.

**Implemented WSLC repair (2026-10-04, [completed plan](../plan/features/wslc-integration-repair.md), ENG-126…136):** private `WslcEngine` owns COM/CLI delegates and reports primary/mixed/degraded routes through existing metadata. Auto is COM-first per verified operation; forced preferences are strict. Native Run remains unverified: Auto selects pinned-session CLI before dispatch, strict COM returns 501. Read fallback is classified/bounded; mutations never blindly replay, with conservative read reconciliation where possible. Stream fallback is pre-first-source-item only, additionally predispatch-safe for pull/exec; terminals never migrate. Public signatures/error/event types are unchanged. See spec 20 §5 for exact-version trust, parity gates and non-atomic mixed-session check/spawn limitation. Cross-OS release validation is still open; Windows completion is not three-OS CI proof.

## 1. The `Engine` trait

```rust
pub type EngineResult<T> = Result<T, EngineError>;
pub type EngineStream<T> = Pin<Box<dyn Stream<Item = EngineResult<T>> + Send + 'static>>;

#[async_trait::async_trait]
pub trait Engine: Send + Sync + 'static {   // implementations live on the hub runtime only
    // ── identity & health ──────────────────────────────────────────────
    fn id(&self) -> &EngineId;
    fn kind(&self) -> EngineKind;
    fn capabilities(&self) -> Capabilities;            // may change after transport switch → HubEvent::CapabilitiesChanged
    async fn ping(&self) -> EngineResult<()>;
    async fn info(&self) -> EngineResult<EngineInfo>;
    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent>;

    // ── containers ─────────────────────────────────────────────────────
    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>>;
    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails>;
    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()>;
    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()>;
    async fn prune_containers(&self) -> EngineResult<PruneReport>;
    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk>;
    fn stats(&self, id: &str) -> EngineStream<StatsSample>;              // live, ~1 sample / 1–2 s
    async fn top(&self, id: &str) -> EngineResult<ProcessList>;          // Capability::Top
    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>>; // consumed by the hub terminal actor, never handed to the UI (10 §3.3)

    // ── images ─────────────────────────────────────────────────────────
    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>>;
    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails>;
    async fn image_history(&self, id: &str) -> EngineResult<Vec<ImageLayer>>;  // Capability::ImageHistory
    fn pull_image(&self, reference: &str, auth: Option<RegistryAuth>) -> EngineStream<PullProgress>;
    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>>;
    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport>;
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()>;
    async fn run_image(&self, spec: RunSpec) -> EngineResult<String /* container id */>;

    // ── volumes ────────────────────────────────────────────────────────
    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>>;
    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails>;
    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary>;
    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()>;
    async fn prune_volumes(&self) -> EngineResult<PruneReport>;
    async fn disk_usage(&self) -> EngineResult<DiskUsage>;               // Capability::DiskUsage

    // ── networks ───────────────────────────────────────────────────────
    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>>;
    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails>;
    async fn remove_network(&self, id: &str) -> EngineResult<()>;
    async fn prune_networks(&self) -> EngineResult<PruneReport>;
}
```

A method an engine doesn't support MUST return `EngineError::Unsupported(cap)`, where `cap: Capabilities` is a single flag. The UI checks
`capabilities()` beforehand, so this path is a safety net only.

## 2. Capabilities

```rust
bitflags! { pub struct Capabilities: u32 {
    const EVENTS          = 1 << 0;   // live event stream
    const PAUSE           = 1 << 1;
    const EXEC_TTY        = 1 << 2;   // interactive terminal
    const EXEC_RESIZE     = 1 << 3;
    const STATS_STREAM    = 1 << 4;   // native streaming stats (else polled)
    const TOP             = 1 << 5;
    const IMAGE_HISTORY   = 1 << 6;
    const DISK_USAGE      = 1 << 7;
    const PULL_PROGRESS   = 1 << 8;   // structured per-layer progress
    const NETWORK_MGMT    = 1 << 9;
    const LOGS_FOLLOW     = 1 << 10;
}}
```

| Capability | Docker (≥1.41) | WSL distro (Docker) | WSLC — COM (primary) | WSLC — CLI (fallback) |
|---|---|---|---|---|
| EVENTS | ✔ | ✔ | ✔ (`GetEvents`) | ✔ (`system events`) — verified text output on 3.0.1 (S-2/F-10), no JSON option |
| PAUSE | ✔ | ✔ | ✘ | ✘ |
| EXEC_TTY / EXEC_RESIZE | ✔ / ✔ | ✔ / ✔ | ✔ / ✔ (`IWSLCProcess::ResizeTty`) | ✔ / ✔ (ConPTY) |
| STATS_STREAM | ✔ | ✔ | ✘ (polled `Stats()`) | ✘ (polled) |
| TOP | ✔ | ✔ | ✘ | ✘ |
| IMAGE_HISTORY | ✔ | ✔ | ✘ | ✘ |
| DISK_USAGE | ✔ (API ≥ 1.52) | ✔ (API ≥ 1.52) | ✘ (derived) | ✘ (derived) |
| PULL_PROGRESS | ✔ | ✔ | ✔ (`IProgressCallback`) | ✘ (text) |
| NETWORK_MGMT | ✔ | ✔ | ✔ | ✔ |
| LOGS_FOLLOW | ✔ | ✔ | ✔ | ✔ |

## 3. DTOs (normative field lists)

### 3.0 Request / option types

```rust
pub struct ContainerQuery { pub all: bool /* default true */, pub size: bool, pub label_filter: Vec<(String, Option<String>)> }
pub struct EventFilter    { pub kinds: Vec<ResourceKind>, pub since: Option<OffsetDateTime> }
pub struct RunSpec { pub image: String, pub name: Option<String>, pub ports: Vec<PortMapping>, pub env: Vec<(String,String)>,
                     pub mounts: Vec<MountRequest /* volume|bind, source, target, read_only */>, pub auto_remove: bool,
                     pub cmd: Option<Vec<String>>, pub labels: BTreeMap<String,String> }
pub struct VolumeSpec { pub name: Option<String>, pub driver: Option<String>, pub driver_opts: BTreeMap<String,String>,
                        pub labels: BTreeMap<String,String> }
pub struct RegistryAuth { pub server: String, pub username: Option<String>, pub password: Option<SecretString>, pub identity_token: Option<SecretString> }
pub struct ProcessList { pub titles: Vec<String>, pub processes: Vec<Vec<String>> }
pub struct TlsFiles { pub ca: PathBuf, pub cert: PathBuf, pub key: PathBuf, pub verify: bool }
pub struct EngineConfigSchema { pub fields: Vec<ConfigField /* key, label, kind: Text|Path|Port|Choice|Bool, required, validate */> }
```

`SecretString` never implements `Debug` / `Display` with its content (NFR-020).

All timestamps are `OffsetDateTime` (UTC). All sizes are `u64` bytes. Ids are full 64-hex for
Docker. The UI shortens them to 12 characters.

### 3.1 Engine

```rust
#[non_exhaustive]
pub enum EngineKind { Docker, WslDistro, Wslc, AppleContainer /* reserved, ENG-030 */ }

pub struct EngineInfo {
    pub name: String, pub kind: EngineKind,
    pub transport: Option<String>,          // e.g. "com", "cli", "xpc"; display/diagnostics only
    pub transport_note: Option<String>,     // existing code field; limitation/mixed/degraded route explanation (ENG-110/136)
    pub server_version: String, pub api_version: Option<String>,
    pub os: String, pub arch: String, pub kernel: Option<String>,   // engine-wide facts are optional (ENG-031)
    pub cpus: Option<u32>, pub mem_total: Option<u64>,
    pub containers: ContainerCounts,      // running/paused/stopped
    pub images: u32,
    pub storage_driver: Option<String>, pub root_dir: Option<String>,
    pub capabilities: Capabilities,
}
pub enum EngineState { Disconnected, Connecting, Connected, Degraded, Failed { error: EngineError, retry_at: Option<Instant> } }
```

For WSLC operation routing, `transport` describes the primary transport, not the last call; `transport_note` names CLI exceptions/degraded routes. Capabilities and existing `list_stats_limit` reflect a coherent snapshot. Hub full-info refresh publishes existing StatusChanged even when flags are unchanged (ENG-136); CapabilitiesChanged remains for actual flag changes. Local UI InfoChanged/apply_info propagation does not add a public hub event or Engine method.

### 3.2 Containers

```rust
pub struct ContainerSummary {
    pub id: String, pub name: String,                 // name without leading '/'
    pub image: String, pub image_id: String,
    pub command: String,
    pub created: OffsetDateTime,
    pub state: ContainerState,                         // Created|Running|Paused|Restarting|Removing|Exited|Dead|Unknown
    pub status_text: String,                           // "Up 3 minutes (healthy)"
    pub health: Option<Health>,                        // Starting|Healthy|Unhealthy
    pub exit_code: Option<i64>,
    pub ports: Vec<PortMapping>,
    pub labels: BTreeMap<String, String>,
    pub networks: Vec<String>,
    pub ip_addresses: Vec<IpAddr>,                     // per-container IPs (VM-per-container engines, ENG-031)
    pub mounts: Vec<MountSummary>,
    pub size_rw: Option<u64>, pub size_root_fs: Option<u64>,   // only when q.size
    pub compose: Option<ComposeInfo>,                  // derived from labels (see §4)
}
pub struct PortMapping { pub ip: Option<IpAddr>, pub private: u16, pub public: Option<u16>, pub proto: Proto }
pub struct ComposeInfo { pub project: String, pub service: String, pub number: Option<u32>,
                         pub working_dir: Option<String>, pub config_files: Vec<String> }

pub struct ContainerDetails {
    pub summary: ContainerSummary,
    pub started_at: Option<OffsetDateTime>, pub finished_at: Option<OffsetDateTime>,
    pub restart_count: u32, pub restart_policy: Option<String>,
    pub pid: Option<u32>, pub platform: Option<String>,
    pub entrypoint: Vec<String>, pub cmd: Vec<String>, pub working_dir: Option<String>, pub user: Option<String>,
    pub hostname: Option<String>, pub tty: bool,
    pub env: Vec<EnvVar>,                              // UI masks values matching /(?i)pass|secret|token|key/
    pub mounts: Vec<MountDetail>,                      // type, source, destination, mode, rw, propagation, volume name
    pub network_settings: ContainerNetworking,          // per network: ip, ipv6, gateway, mac, aliases, network id
    pub port_bindings: Vec<PortMapping>,
    pub resources: ResourceLimits,                     // nano_cpus, memory, memory_swap, pids_limit, cpu_shares
    pub health_log: Vec<HealthCheckResult>,
    pub raw: serde_json::Value,                        // full inspect JSON for the Inspect tab
}

pub enum ContainerAction { Start, Stop { timeout_s: Option<u32> }, Restart { timeout_s: Option<u32> },
                           Kill { signal: Option<String> }, Pause, Unpause }
pub struct RemoveContainerOpts { pub force: bool, pub volumes: bool }
```

### 3.3 Logs / stats / exec

```rust
pub struct LogOpts { pub follow: bool, pub tail: Option<u32 /* default 1000 */>, pub since: Option<OffsetDateTime>,
                     pub timestamps: bool }
pub struct LogChunk { pub stream: LogStream /* Stdout|Stderr|Console */, pub ts: Option<OffsetDateTime>, pub bytes: Bytes }

/// One normalised sample. Engine converts raw counters → rates; UI never sees cumulative counters.
pub struct StatsSample {
    pub at: OffsetDateTime,
    pub cpu_percent: f64,          // 0..=100*online_cpus (Docker CLI semantics); UI also shows / online_cpus
    pub online_cpus: u32,
    pub mem_used: u64,             // usage - inactive_file (cgroup v2) / - cache (v1), as docker CLI
    pub mem_limit: u64,
    pub net_rx_bps: f64, pub net_tx_bps: f64,       // summed across interfaces
    pub net_rx_total: u64, pub net_tx_total: u64,
    pub blk_read_bps: f64, pub blk_write_bps: f64,  // io_service_bytes_recursive Read/Write
    pub blk_read_total: u64, pub blk_write_total: u64,
    pub pids: Option<u64>,
}

pub struct ExecRequest { pub cmd: Vec<String>, pub tty: bool, pub env: Vec<String>, pub user: Option<String>,
                         pub working_dir: Option<String>, pub cols: u16, pub rows: u16 }
```

**CPU % formula** (Docker CLI compatible), computed in `dk-core::stats` from consecutive raw samples:
`cpu_delta = cpu.total_usage - precpu.total_usage`,
`sys_delta = cpu.system_cpu_usage - precpu.system_cpu_usage`,
`cpu_percent = (cpu_delta / sys_delta) * online_cpus * 100` when both deltas are positive. Otherwise 0.
Rates (`*_bps`) = Δbytes / Δt between successive samples. The first sample yields rates of 0.

### 3.4 Images / volumes / networks

```rust
pub struct ImageSummary { pub id: String, pub repo_tags: Vec<String>, pub repo_digests: Vec<String>,
    pub created: OffsetDateTime, pub size: u64, pub shared_size: Option<u64>, pub containers: Option<u32>,
    pub labels: BTreeMap<String,String>, pub dangling: bool }
pub struct ImageDetails { pub summary: ImageSummary, pub architecture: String, pub os: String, pub variant: Option<String>,
    pub author: Option<String>, pub config: ImageConfig /* env, cmd, entrypoint, exposed ports, workdir, user, volumes, labels */,
    pub root_fs_layers: Vec<String>, pub raw: serde_json::Value }
pub struct ImageLayer { pub id: Option<String>, pub created: OffsetDateTime, pub created_by: String, pub size: u64, pub comment: String }
pub enum PullProgress { Layer { id: String, status: String, current: Option<u64>, total: Option<u64> },
                        Status(String), Done { digest: Option<String> } }

pub struct VolumeSummary { pub name: String, pub driver: String, pub mountpoint: String, pub created: Option<OffsetDateTime>,
    pub scope: String, pub labels: BTreeMap<String,String>, pub size: Option<u64>, pub ref_count: Option<i64>,
    pub compose: Option<ComposeInfo> }
pub struct VolumeDetails { pub summary: VolumeSummary, pub options: BTreeMap<String,String>, pub status: Option<serde_json::Value>,
    pub used_by: Vec<ContainerRef>, pub raw: serde_json::Value }   // used_by computed from container mounts

pub struct NetworkSummary { pub id: String, pub name: String, pub driver: String, pub scope: String,
    pub internal: bool, pub attachable: bool, pub ipv6: bool, pub created: OffsetDateTime,
    pub subnets: Vec<IpamConfig>, pub labels: BTreeMap<String,String>, pub compose: Option<ComposeInfo> }
pub struct NetworkDetails { pub summary: NetworkSummary, pub containers: Vec<NetworkEndpoint /* name, id, ipv4, ipv6, mac */>,
    pub options: BTreeMap<String,String>, pub raw: serde_json::Value }

pub struct DiskUsage { pub images_size: u64, pub containers_size: u64, pub volumes: Vec<(String, u64)>, pub build_cache: u64 }
pub struct PruneReport { pub deleted: Vec<String>, pub space_reclaimed: u64 }
```

### 3.5 Events

```rust
pub struct EngineEvent { pub at: OffsetDateTime, pub kind: ResourceKind /* Container|Image|Volume|Network|Daemon */,
                         pub action: String /* start, die, destroy, pull, create, ... */, pub id: String,
                         pub attributes: BTreeMap<String,String> }
```

## 4. Grouping rules (CON-010…)

Implemented as pure functions in `dk-core::grouping` and unit-tested.

1. **Compose**: a container that has label `com.docker.compose.project` belongs to group `compose:<project>`. The group shows the project name, plus `working_dir` taken from `com.docker.compose.project.working_dir` when present.
2. **Custom label** (setting): the user may choose another label key to group by, for example `app` or `com.example.stack`.
3. **Ungrouped**: containers without the label are listed at top level, after groups (or interleaved, sorted by the active sort key; this is a setting).
4. Group aggregate state: **Running** if all members run, **Partial** (`2/3 running`) if some run, **Exited** if none run. Group CPU% and memory are the sums of member values, when stats are enabled.
5. Group actions apply to members **in parallel with a limit of 4**: start, stop, restart, delete. Delete asks for confirmation and lists the members.
6. Volumes and networks also get `compose` from `com.docker.compose.project` labels. This is shown as a column and filter, not as tree grouping, in v1.

## 5. Terminal session contract

```rust
#[async_trait]
pub trait TerminalSession: Send + 'static {
    /// Bytes from the process (PTY output). Ends when the process exits.
    fn output(&mut self) -> EngineStream<Bytes>;
    /// Send keystrokes / pasted text.
    async fn write(&self, data: Bytes) -> EngineResult<()>;
    async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()>;   // no-op if !EXEC_RESIZE
    async fn wait(&self) -> EngineResult<Option<i64 /* exit code */>>;
    async fn close(&self) -> EngineResult<()>;
}
```

| Engine | Implementation |
|---|---|
| Docker / WSL distro | `POST /containers/{id}/exec` (`AttachStdin/Stdout/Stderr`, `Tty: true`, `Cmd`), then `POST /exec/{id}/start` with a hijacked bidirectional stream through bollard (`StartExecResults::Attached { output, input }`), `POST /exec/{id}/resize`, and `GET /exec/{id}/json` for the exit code. |
| WSLC — COM | `IWSLCContainer::Exec` with a TTY and initial rows/cols → `IWSLCProcess`. I/O goes through the handles from `GetStdHandle`, resize through `ResizeTty`, and the exit code comes from `GetExitEvent` + `GetState`. |
| WSLC — CLI fallback | `portable-pty` ConPTY running `wslc.exe container exec -i -t <id> <cmd>`. Resize via `MasterPty::resize`. The exit code comes from the child's exit status. |

**Safety (ENG-130/131):** exec can mutate before a TerminalSession/output handle is returned. A failure after possible creation MUST NOT start another process automatically, even with no output. Returned output/write/resize/wait/close stay transport-pinned; cleanup follows tagged ownership (ENG-134).

**Shell selection** (TRM-004): the default command is `["/bin/sh", "-c", "if command -v bash >/dev/null; then exec bash; else exec sh; fi"]`. The user can override it per terminal tab (bash, sh, zsh, ash, or custom).

## 6. Mapping: Docker Engine API (via bollard)

| Contract op | Docker Engine API | bollard |
|---|---|---|
| ping / info | `GET /_ping`, `GET /info`, `GET /version` | `ping`, `info`, `version` |
| events | `GET /events?filters=` | `events` |
| list_containers | `GET /containers/json?all=1&size=<q.size>` | `list_containers` |
| inspect_container | `GET /containers/{id}/json` | `inspect_container` |
| start / stop / restart / kill / pause / unpause | `POST /containers/{id}/start|stop|restart|kill|pause|unpause` | `start_container` … |
| remove_container | `DELETE /containers/{id}?force&v` | `remove_container` |
| logs | `GET /containers/{id}/logs?follow&stdout&stderr&timestamps&tail&since` | `logs` |
| stats | `GET /containers/{id}/stats?stream=1` | `stats` |
| top | `GET /containers/{id}/top` | `top_processes` |
| exec | `POST /containers/{id}/exec`, `POST /exec/{id}/start`, `POST /exec/{id}/resize`, `GET /exec/{id}/json` | `create_exec`, `start_exec`, `resize_exec`, `inspect_exec` |
| list/inspect/history images | `GET /images/json`, `/images/{id}/json`, `/images/{id}/history` | `list_images`, `inspect_image`, `image_history` |
| pull | `POST /images/create?fromImage&tag` (JSON progress stream) | `create_image` |
| remove/prune/tag images | `DELETE /images/{id}`, `POST /images/prune`, `POST /images/{id}/tag` | … |
| run_image | `POST /containers/create` + `start` | `create_container`, `start_container` |
| volumes | `GET /volumes`, `GET/DELETE /volumes/{n}`, `POST /volumes/create`, `POST /volumes/prune` | … |
| disk_usage | `GET /system/df` | `df` |
| networks | `GET /networks`, `GET/DELETE /networks/{id}`, `POST /networks/prune` | … |

**Docker mapping notes (verified on Docker Desktop 29.8.1)**

| Topic | Behaviour |
|---|---|
| API version | bollard 0.21's maximum is **1.53** (`CLIENT_MAX`, 20 §3), so newer daemons are driven at 1.53. |
| `disk_usage` | At API ≥ 1.52, `/system/df` returns `ImageUsage` / `ContainerUsage` / `VolumeUsage` / `BuildCacheUsage`, each with `TotalSize`, `Reclaimable`, and `Items`. The older `LayersSize` / `Images` / `Volumes` arrays are gone. The mapper handles both shapes, but bollard 0.21's `SystemDataUsageResponse` only models the new one and silently drops the old fields. So `DISK_USAGE` is advertised only at negotiated API ≥ 1.52; below that, `disk_usage` returns `Unsupported(DISK_USAGE)` and the UI uses its no-`DISK_USAGE` fallbacks (VOL-002, IMG-003). |
| `pull_image` reference | Adds `tag=latest` when the reference has neither a tag nor a digest. |
| `pull_image(ref, None)` | The engine resolves credentials itself from the Docker config (IMG-007). |
| Registry auth errors | Not always 401: ghcr returns 500 `denied`, and Docker Hub returns `pull access denied`. All of these map to the IMG-007 "Authentication required" message. |

## 7. Contract tests

**WSLC mapping/routing (ENG-126…136):** Docker/WSL mappings are unchanged. Native/CLI mappings remain spec 20 §5.4/§5.5; [plan §5.1](../plan/features/wslc-integration-repair.md#51-complete-trait-mapping-and-routing-groups) covers the complete trait surface. Native Run is declared, not dispatched; Auto CLI/strict COM 501 remain intentional. WSLC pause/top/history/disk usage return Unsupported(single flag). No new operation/capability. Mixed volume-prune/explicit-local create-volume fallback is refused; supplied CLI pull auth is rejected before spawn.

`dk-core` ships a reusable test suite, `dk_core::contract::run_suite(engine)`. Each backend runs it in CI:

- `DockerEngine` runs it against a real dockerd (the Linux CI service container), and against a recorded HTTP fixture server for Windows and macOS.
- `WslcEngine` (CLI transport) runs it against a **fake `wslc.exe`**: a small test binary that replays recorded JSON fixtures by argv.
- `WslcEngine` (COM transport) runs it against a **fake in-process COM server** that implements the vendored `IWSLC*` vtables and returns fixture data. This tests our vtable/struct declarations, memory freeing, and HRESULT mapping without WSL. It does **not** test marshalling; that's covered by the self-hosted `wsl` runner against real WSLC.
- A real-WSLC run (both transports) happens on a self-hosted or manual Windows job, and is required before shipping a new ABI module (20 §5.7).
- **Router coverage:** current factory/router/COM/CLI tests verify strict preferences/session binding, predispatch Run, typed fault budgets, no duplicate mutation after lost responses/timeouts, completed create/failed inspect, stream boundaries/pull/exec ambiguity, cancellation/tokens/tagged resource closure, fail-closed discovery and full metadata without flag changes. Actual test names/results are in the plan §8 and feature verification tables.
- **Inspect coverage:** `eng_133_ports_and_cdt_040_original_raw`, `eng_133_recorded_com_cli_inspect_raw_deep_equality`, `eng_133_cdt_030_040_published_ports_both_delegate_paths` cover original raw and typed ports. Synthetic published-port fixtures are not live recordings.
- **Evidence boundaries:** final Windows workspace PASS includes WSLC 183 passed/18 live opt-in ignores; hub 77/UI 243. Maintained `repair_live::eng_127_128_normal_user_hello_world_acceptance` separately passed four Runs/both typed inspect-log paths/strict COM rejection/admin rejection/cleanup on 3.0.1.0. Fake tests cannot prove out-of-process marshalling, live fault/leak stress or native CreateContainer. New exact-version/native enablement needs separate real evidence. Linux compiler provisioning and unrun macOS validation remain explicit release gates; no three-OS green claim. Historical external-probe/smaller counters are superseded by the plan completion record.
- The suite checks DTO invariants (non-empty ids, monotonic stats timestamps, grouping) and checks that capability gating is honoured.
- Future backends (Apple `container`, 20 §7) MUST pass the same suite before they're enabled.

## 8. Backend registration (`EngineFactory`)

```rust
/// Implemented once per backend crate; registered with the hub at startup (cfg-gated per OS).
#[async_trait]
pub trait EngineFactory: Send + Sync + 'static {
    fn kind(&self) -> EngineKind;
    /// Find engines of this kind on the current machine (ENG-001…008, ENG-033).
    async fn discover(&self) -> Vec<EngineConfig>;
    /// Validate a manual/stored config and build a connected engine (transport selection happens here).
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>>;
    /// Settings UI schema for the "Add engine" dialog (fields, validation), so the UI stays generic.
    fn config_schema(&self) -> EngineConfigSchema;
}
```

| Factory | Crate | OS (cfg) | v1 |
|---|---|---|---|
| `DockerFactory` | `dk-engine-docker` | all | ✔ |
| `WslDistroFactory` | `dk-wsl` (+ `dk-engine-docker`) | windows | ✔ |
| `WslcFactory` | `dk-engine-wslc` | windows | ✔ |
| `AppleContainerFactory` | `dk-engine-apple` | macos (aarch64) | ✘ deferred (20 §7) |

`EngineEndpoint` (20 §1) gains a variant per backend. Stored configs with an unknown variant, for
example a config written by a newer app version, are kept and shown as *unsupported*, never deleted.
