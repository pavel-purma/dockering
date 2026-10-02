# 10 — Architecture

## 1. Overview

```
┌──────────────────────────────── dockering (binary) ────────────────────────────────┐
│  UI thread (GPUI foreground executor)                                              │
│  ┌──────────┐  ┌───────────────┐  ┌──────────────┐  ┌───────────────────────────┐  │
│  │ AppShell │─▶│ Pages / Views │─▶│ Stores       │◀─│ Bridge tasks (cx.spawn)   │  │
│  │ (Root)   │  │ (GPUI Kit)    │  │ (Entities)   │  │ await oneshot / mpsc      │  │
│  └──────────┘  └───────────────┘  └──────────────┘  └─────────────▲─────────────┘  │
│                                                                   │ futures chans  │
│ ──────────────────────────────── thread boundary ─────────────────┼──────────────  │
│  Hub runtime (dedicated multi-thread tokio runtime, "dk-hub-*")   │                │
│  ┌────────────────────────────────────────────────────────────────┴─────────────┐  │
│  │ EngineHub: registry · connection supervisor · discovery · stats service      │  │
│  └───────┬──────────────────────┬────────────────────────┬──────────────────────┘  │
│          │ dyn Engine           │ dyn Engine             │ dyn Engine              │
│  ┌───────▼────────┐   ┌─────────▼──────────┐   ┌─────────▼─────────┐               │
│  │ DockerEngine   │   │ DockerEngine       │   │ WslcEngine        │               │
│  │ (bollard)      │   │ via WSL pipe bridge│   │ COM ▸ CLI fallback│               │
│  └───────┬────────┘   └─────────┬──────────┘   └─────────┬─────────┘               │
└──────────┼──────────────────────┼────────────────────────┼─────────────────────────┘
           ▼                      ▼                        ▼
   unix socket / npipe /   wsl.exe -d X -e docker    COM: WSLCSessionManager →
   tcp(+tls)               system dial-stdio         IWSLCSession (session VM)
                                                     fallback: wslc.exe --format json
```

There are two worlds, and only **owned DTOs and executor-agnostic `futures` channels** cross
between them:

1. **UI world.** GPUI runs on the main thread. It holds views, stores, and navigation.
2. **Hub world.** A dedicated tokio runtime owns every socket, pipe, child process, and timer that talks to an engine.

## 2. Cargo workspace layout

```
dockering/
├── Cargo.toml                 # [workspace], shared deps, lints, profiles
├── rust-toolchain.toml
├── crates/
│   ├── dk-core/               # Domain: DTOs, Engine trait, Capabilities, errors, grouping & stats math
│   ├── dk-engine-docker/      # Engine impl over Docker Engine API (bollard)
│   ├── dk-wsl/                # Windows: WSL distro discovery, wsl.exe runner, named-pipe stdio bridge
│   ├── dk-engine-wslc/        # Engine impl for WSLC: native COM transport (primary) + wslc.exe CLI transport (fallback)
│   ├── dk-hub/                # EngineHub: runtime, registry, supervisor, discovery, stats service, config
│   ├── (dk-engine-apple/)     # FUTURE, deferred: Apple `container` backend (XPC + CLI), see 20 §7
│   ├── dk-terminal/           # Terminal model (alacritty_terminal) + GPUI TerminalView element
│   └── dockering/             # The app binary: GPUI Kit shell, pages, stores, settings UI
├── assets/                    # icons, app icon, fonts (if bundled)
├── packaging/                 # cargo-packager config, wix/dmg/appimage resources
├── docs/
└── .claude/                   # agents + skills
```

### Dependency rules (enforced in review)

```
dockering ──▶ dk-hub ──▶ dk-engine-docker ──▶ dk-core
    │            │   ──▶ dk-engine-wslc   ──▶ dk-core
    │            │   ──▶ dk-wsl ──▶ dk-engine-docker, dk-core   (Windows code behind cfg; builds everywhere)
    │            └──────────────────────────▶ dk-core
    ├──▶ dk-terminal ──▶ dk-core
    └──▶ dk-core
```

- `dk-core` MUST NOT depend on tokio, bollard, or GPUI. It depends only on serde, serde_json, futures, bytes, thiserror, time, bitflags, and async-trait.
- Only `dockering` and `dk-terminal` depend on `gpui-kit`.
- Only `dk-hub` and the engine crates depend on `tokio`. The UI never names a tokio type.
- `dk-wsl` and `dk-engine-wslc` compile on every OS. Their Windows-only code sits behind `#[cfg(windows)]`, and on other OSes discovery returns an empty list. This keeps CI building the same crate graph everywhere.

### Key third-party crates

| Crate | Use | Where |
|---|---|---|
| `gpui-kit` 0.7 (re-exports `gpui`, `gpui-component`) | UI framework and component library | dockering, dk-terminal |
| `bollard` 0.21 (`ssl`, `pipe` features) | Docker Engine API client | dk-engine-docker |
| `tokio` 1.x (`rt-multi-thread`, `process`, `net`, `io-util`, `time`, `sync`, `macros`) | Hub runtime | dk-hub, engines |
| `tokio-util` (`CancellationToken`, codecs) | Cancellation, line framing | dk-hub, engines |
| `futures` | Executor-agnostic channels and streams across the boundary | all |
| `alacritty_terminal` | VT parser and terminal grid | dk-terminal |
| `serde`, `serde_json`, `toml` | DTOs and config | all |
| `thiserror`, `anyhow` (bin only) | Errors | all |
| `tracing`, `tracing-subscriber`, `tracing-appender` | Logging | all |
| `directories` | Config and data dirs | dk-hub |
| `windows` (`Win32_System_Com`, `implement`) | COM client for WSLC (`IWSLC*` interfaces, `IProgressCallback` impl) | dk-engine-wslc |
| `windows` / `windows-sys` | Named pipes, ACLs, `CREATE_NO_WINDOW` | dk-wsl |
| `portable-pty` | ConPTY for the WSLC CLI-fallback terminal | dk-engine-wslc |
| `time` or `jiff` | Timestamps and relative time | dk-core |

Exact versions are pinned in the workspace `Cargo.toml` during M1.

## 3. Threading & async model

### 3.1 Rules (normative)

- **NFR-001** No code running on the GPUI foreground executor may block. That rules out `std::fs`, `std::process`, `std::net`, `block_on`, `thread::sleep`, and mutex waits on contended locks. A CI grep check enforces this for `crates/dockering/src/**`. The only exception is the startup code before the first window opens.
- **NFR-002** All engine I/O runs on the hub runtime. The UI starts work with `cx.spawn(async move |this, cx| …)` and *awaits a hub future*. Awaiting a `futures::channel::oneshot::Receiver` doesn't block; it yields to GPUI.
- **NFR-003** CPU-heavy, UI-adjacent work runs on `cx.background_spawn`. Examples are formatting a 5 MB inspect JSON, ANSI-parsing a large log batch, and sorting 5,000 rows.
- **NFR-004** Every long-lived task handle (`gpui::Task`) is stored on its owning entity. When the entity drops, the task is cancelled. Bare `.detach()` is allowed only for fire-and-forget actions whose result is reported through a notification.
- **NFR-005** Stream results are tagged with a request or revision id. The UI drops stale results, for example after the user switched engine or container.

### 3.2 Hub runtime

`EngineHub::start(config) -> HubHandle` creates a `tokio::runtime::Builder::new_multi_thread()`
runtime with 2–4 worker threads named `dk-hub-N`, plus `enable_all()`. The runtime lives for
the whole app lifetime. `HubHandle` is `Clone + Send + Sync` (an `Arc` inside) and is stored as
a GPUI `Global`.

### 3.3 The UI ⇄ Hub bridge contract

```rust
// dk-hub/src/bridge.rs (shape, not final code)

/// A request/response: future resolves on any executor.
pub struct HubCall<T>(futures::channel::oneshot::Receiver<Result<T, EngineError>>);
impl<T> Future for HubCall<T> { type Output = Result<T, EngineError>; /* Canceled → EngineError::Cancelled */ }

/// A live subscription. Dropping it cancels the producer on the hub runtime.
pub struct HubStream<T> {
    rx: futures::channel::mpsc::Receiver<Result<T, EngineError>>, // bounded (default 256)
    _guard: DropGuard,                                             // tokio_util CancellationToken
}
impl<T> Stream for HubStream<T> { … }

/// Stream item wrapper for "latest-state" streams (events, stats): the consumer learns when items
/// were dropped and must resynchronise (full refetch) instead of silently missing e.g. `destroy`.
pub enum Feed<T> { Item(T), Lagged { dropped: u64 } }

impl HubHandle {
    // ── generic engine access (closures run on the hub runtime) ───────────────────────────
    pub fn call<T, F, C>(&self, engine: &EngineId, f: C) -> HubCall<T>
    where C: FnOnce(Arc<dyn Engine>) -> F + Send + 'static,
          F: Future<Output = Result<T, EngineError>> + Send + 'static, T: Send + 'static;
    pub fn subscribe<T, S, C>(&self, engine: &EngineId, f: C) -> HubStream<T>
    where C: FnOnce(Arc<dyn Engine>) -> S + Send + 'static,
          S: Stream<Item = Result<T, EngineError>> + Send + 'static, T: Send + 'static;
    // If the engine isn't Connected: `call` fails fast with EngineError::Unreachable (no queuing);
    // `subscribe` yields one Err(Unreachable) and ends. Stores resubscribe on ENG-022 reconnect.

    // ── engines (ENG-*) ────────────────────────────────────────────────────────────────────
    pub fn hub_events(&self) -> HubStream<Feed<HubEvent>>;      // added/removed/status/capabilities changed
    pub fn engines(&self) -> HubCall<Vec<EngineStatus>>;
    pub fn set_active(&self, id: &EngineId) -> HubCall<()>;
    pub fn add_engine(&self, cfg: EngineConfig) -> HubCall<EngineId>;
    pub fn update_engine(&self, cfg: EngineConfig) -> HubCall<()>;
    pub fn remove_engine(&self, id: &EngineId) -> HubCall<()>;
    pub fn test_engine(&self, cfg: EngineConfig) -> HubCall<EngineInfo>;
    pub fn rescan(&self) -> HubCall<()>;
    pub fn start_wsl_distro(&self, id: &EngineId) -> HubCall<()>; // ENG-106 explicit user action only

    // ── services owned by the hub ─────────────────────────────────────────────────────────
    pub fn events(&self, engine: &EngineId) -> HubStream<Feed<EngineEvent>>;   // shared, deduped per engine
    pub fn stats(&self, engine: &EngineId, id: &str) -> HubStream<Feed<StatsSample>>; // StatsService, replays history first (STA-006)
    pub fn open_terminal(&self, engine: &EngineId, id: &str, req: ExecRequest) -> HubCall<TerminalHandle>;
    pub fn save_file(&self, path: PathBuf, bytes: Bytes) -> HubCall<()>;        // LOG-004 export etc.
    pub fn config(&self) -> ConfigHandle;                                       // read snapshot / write (debounced)
}

/// Terminal sessions live on the hub as actors (the engine's TerminalSession never leaves the hub,
/// because its I/O needs the tokio reactor / COM threads). The UI only sees channels:
pub struct TerminalHandle {
    pub output: HubStream<Bytes>,                          // PTY bytes; ends on process exit
    pub input: futures::channel::mpsc::Sender<TermCmd>,    // TermCmd::{Data(Bytes), Resize{cols,rows}, Close}
    pub exit: HubCall<Option<i64>>,
}
```

UI-side pattern (normative):

```rust
fn refresh(&mut self, cx: &mut Context<Self>) {
    let hub = cx.global::<HubHandle>().clone();
    let engine = self.engine_id.clone();
    let rev = self.bump_revision();
    self.loading = true;
    cx.notify();
    self.refresh_task = Some(cx.spawn(async move |this, cx| {
        let result = hub.call(&engine, |e| async move { e.list_containers(Default::default()).await }).await;
        this.update(cx, |this, cx| {
            if this.revision != rev { return; }          // stale
            this.loading = false;
            this.apply(result);                           // Ok → data, Err → error state
            cx.notify();
        }).ok();                                          // view gone → treated as cancel
    }));
}
```

Streaming pattern: a foreground task loops over `HubStream::next().await`. It **batches**
items that arrive within the same tick before calling `this.update` + `cx.notify()`, so the
UI renders at most about once per frame:

- logs flush every 50 ms or 500 lines,
- stats flush per sample,
- terminal output flushes on every chunk, coalesced by a 4 ms timer.

Backpressure: hub-side producers use bounded channels.
- **Stats and events** use a hub-side ring (tokio `broadcast`-style). When a consumer falls behind,
  it receives `Feed::Lagged { dropped }`. For events the store then does a **full refetch**, so a missed
  `destroy` can't leave ghost rows. For stats the gap is just shown as a gap.
- **Logs and terminal** apply backpressure to the engine stream, so no data is lost.

**Ownership (single owner per resource):** stats ring buffers → hub `StatsService`. Terminal
sessions → hub terminal actors, indexed by `TerminalRegistry` (TRM-008). Log buffers → the UI
`LogsView` (not retained after leaving the detail page). `ContainerDetailState` holds only
*handles* (stream subscriptions, the `TerminalHandle`), never the data's source of truth.

## 4. State management

### 4.1 Entities

| Entity | Lifetime | Holds |
|---|---|---|
| `AppState` (Global) | app | `HubHandle`, config snapshot, active `EngineId`, theme |
| `EngineListStore` | app | All configured and discovered engines with `EngineStatus`. Fed by `hub_events()`. |
| `EngineStore` | per active engine | `Resource<Vec<ContainerSummary>>`, images, volumes, networks, `EngineInfo`, the event subscription |
| `Navigator` | window | `Route` stack (back/forward), current route |
| Page views | while mounted | View state (search text, filters, sort, selection, collapsed groups), plus a handle to `EngineStore` |
| `ContainerDetailState` | while detail page open | Inspect data, the logs view buffer, and *subscriptions* to hub stats/terminal (the data lives in the hub, see §3.3 Ownership) |

```rust
pub enum Resource<T> {
    Idle,
    Loading { previous: Option<T> },   // keep old data visible while refreshing
    Ready { data: T, fetched_at: Instant },
    Failed { error: EngineError, previous: Option<T> },
}
```

### 4.2 Refresh strategy

1. When an engine becomes active, `EngineStore` loads containers, images, volumes, and networks in parallel, then subscribes to `Engine::events()`.
2. Each event from the engine → debounce for 150 ms → refetch **only the affected collection**. A container event refetches containers and also invalidates the images "in use" flag. The DTOs include enough data that partial patches aren't worth the complexity in v1.
3. If the events stream errors or the engine lacks `Capability::Events`, the store falls back to polling every *N* seconds (default 5 s for Docker and WSLC-COM, 3 s for WSLC-CLI).
4. A manual refresh (`F5` / `Cmd+R`) refetches everything.
5. Volume sizes come from `disk_usage()`, which is expensive. They load lazily and only on the Volumes page.

### 4.3 Engine switching

Switching drops the current `EngineStore`, which cancels all of its subscriptions, and creates a
new one. The route is preserved when it's a list page. Detail routes fall back to their parent
list page because ids are engine-specific. The last active engine is persisted.

## 5. Errors

```rust
#[derive(thiserror::Error, Debug, Clone)]
pub enum EngineError {
    #[error("engine unreachable: {reason}")]      Unreachable { reason: String, hint: Option<String> },
    #[error("{kind} '{id}' not found")]           NotFound { kind: ResourceKind, id: String },
    #[error("conflict: {0}")]                     Conflict(String),           // HTTP 409
    #[error("not supported by this engine: {0:?}")] Unsupported(Capability),
    #[error("timed out after {0:?}")]            Timeout(Duration),
    #[error("engine API error {status}: {message}")] Api { status: u16, message: String },
    #[error("protocol error: {0}")]              Protocol(String),           // bad JSON, unexpected CLI output
    #[error("cancelled")]                        Cancelled,
}
```

- HTTP `304 Not Modified` on start or stop is treated as success, because the container is already in that state.
- `Unreachable.hint` holds actionable text, for example "Is Docker running? `sudo systemctl start docker`" or "Add your user to the `docker` group".
- UI mapping: list load failure → inline error panel with *Retry*; action failure → `Notification` (error) with the message; `Unsupported` → the control is hidden or disabled with a tooltip and never reached at runtime.

## 6. Configuration & persistence

| File | Location (via `directories::ProjectDirs("dev", "dockering", "Dockering")`) | Content |
|---|---|---|
| `config.toml` | config dir | User settings and the manually added engine list |
| `state.json` | data dir | Window bounds, last engine, last route, collapsed groups, column widths |
| `logs/dockering.log*` | data local dir | Rotating tracing logs (daily, keep 7) |

`dk-hub::config` loads both files at startup (synchronously, before the first window) and saves
them asynchronously on the hub runtime with a 500 ms debounce, writing atomically via a temp
file and rename. The schema is versioned (`version = 1`) with forward-compatible defaults.

## 7. App bootstrap sequence

1. `main()` → install the panic hook (logs and crash file) → init `tracing` → single-instance check (second launch focuses
   the running window via a local socket/pipe, then exits) → load config (sync).
   **Windows only, before anything touches COM or GPUI:** `WSAStartup(2.2)` (WSLC handles are sockets),
   then `CoIncrementMTAUsage` (keeps the process MTA alive), then on a short-lived MTA thread `CoInitializeEx(MTA)` + `CoInitializeSecurity(IMPERSONATE, EOAC_STATIC_CLOAKING)`.
   Without `CoIncrementMTAUsage`, COM is uninitialised when that thread exits, the security settings are lost, and
   `OpenSessionByName` later fails with `0x80070542` (spec 20 §5.4).
   Calling it after GPUI starts fails with `RPC_E_TOO_LATE` (spike F-8). If it fails, WSLC uses per-proxy
   `CoSetProxyBlanket` only, and the self-check decides whether COM is usable.
   → `EngineHub::start(config)`.
2. `gpui_kit::application().with_assets(Assets).run(|cx| { gpui_kit::init(cx); … })`.
3. Register the `HubHandle` global, apply the theme, register actions and keybindings.
4. `gpui_kit::open_window(options_from_state, cx, |window, cx| AppShell::new(window, cx))`.
5. `AppShell` creates `EngineListStore`. The hub runs discovery ([spec 20 §2](20-engine-backends.md#2-discovery-eng-001eng-010)) in the background, and the last-used engine connects first.
   If no engine is found, the **first-run screen** (ENG-111) is shown.
6. First frame shows the shell with skeleton tables. Data streams in as it arrives.

## 8. Logging & diagnostics

- `tracing` spans per engine call: `engine.call{engine=…, op=list_containers}` with duration.
- The level is set by `RUST_LOG` or by a Settings → Diagnostics toggle (info/debug).
- Settings → Diagnostics → "Open logs folder" and "Copy diagnostics". The second copies version, OS, engines, and capability matrix to the clipboard.
- Secrets are never logged: registry auth, TLS keys, and env values from inspect.
