# Dockering — Implementation Plan

This is the roadmap from an empty repository to v1. **What** we build is defined in
[`docs/spec/`](../spec/README.md). This document defines **in what order and how**.
Per-feature plans live in [`features/`](features/) and are produced by the
`feature-planning` skill.

## Status (2026-10-02)

The v1 implementation is merged on `main`. Every feature spec is `implemented` except
`macos-native-engine` (`deferred`). Gaps are listed per spec under *Known gaps (v1)*.

| Milestone | Status | Notes |
|---|---|---|
| M0 Foundation + early spikes | done | Spikes S-1, S-2, S-5, S-8 done; S-3 done except `CreateContainer`; S-4 partial (§2) |
| M1 Core + hub + Docker backend | done | |
| M2 Shell, keyboard foundation, Containers list | done | |
| M3 Container detail, logs | done | |
| M4 Terminal | done | TRM-009 limited to host-`docker exec` transports |
| M5 Stats & charts | done | |
| M6 Images, volumes, networks | done | |
| M7 WSL distros | done | |
| M8 WSLC (COM + CLI) | done | `run_image` over COM returns 501 until `CreateContainer` is verified |
| M10 Distribution (post-v1) | in-progress | [windows-distribution](features/windows-distribution.md): CI installer, release flow, Inno installer, signing, winget, updater, icon. Public-only steps wait for the public launch (REL-060) |
| M9 Settings, polish, packaging | done, except release-only items | Open: signing/notarisation secrets in the `release` environment; CI runs on real runners (incl. the self-hosted `wsl` runner); the [release checklist](release-checklist.md) on 3 OSes; the keyboard-only walkthrough (KBD-090) on macOS, on Linux, and with a Czech layout |

## 0. Summary of key decisions

| # | Decision | Why | ADR |
|---|---|---|---|
| 1 | Rust workspace with 7 crates; UI isolated from engine I/O | Testability, compile times, enforceable boundaries | [ADR-0001](adr/0001-workspace-and-layering.md) |
| 2 | GPUI Kit 0.7 (`gpui-kit` crate) for all UI | User requirement. It has every component needed (DataTable, Sidebar, Tabs, Charts, Dialog, Notification, code editor). | — |
| 3 | Dedicated tokio runtime ("hub") + `futures` channels bridged into GPUI tasks | bollard needs tokio, and GPUI's executor isn't tokio. This keeps the UI thread non-blocking by construction. | [ADR-0002](adr/0002-async-hub-runtime.md) |
| 4 | `bollard` for the Docker Engine API (socket / npipe / TCP / TLS) | Mature, async, and supports hijacked exec | — |
| 5 | WSL distros through a **named-pipe ⇄ `wsl.exe … docker system dial-stdio` bridge** | Needs zero configuration inside the distro, and reuses the full Docker backend | [ADR-0004](adr/0004-wsl-distro-bridge.md) |
| 6 | WSLC through **native COM** calls to the WSLC session service (`IWSLCSession`, the API `wslc.exe` itself uses), version-gated, with automatic fallback to a `wslc.exe --format json` CLI transport | WSLC exposes no Docker API. The public SDK can't open the user's existing session or list containers, volumes, and networks. COM gives full fidelity without spawning processes. | [ADR-0003](adr/0003-wslc-transport.md) |
| 7 | Terminal = `alacritty_terminal` + a custom GPUI element (`dk-terminal`) | GPUI Kit has no terminal widget | — |
| 7b | Backends plug in via `EngineFactory`. Apple `container` (macOS native) is researched, kept possible (ENG-030…033), and **deferred post-v1** | v1 macOS users are served by Docker-compatible engines; avoids a third native transport before v1 | [ADR-0005](adr/0005-backend-extensibility-apple-container.md) |
| 8 | Spec-driven agentic workflow (`CLAUDE.md`, `.claude/agents`, `feature-planning` skill) | User requirement; keeps spec and code in sync | — |

## 1. Milestones

Every milestone ends with a demoable app on **all three OSes** with green CI. Requirement IDs
refer to the spec.

### M0 — Repository foundation + early spikes (≈ 1.5 weeks)

| Task | Owner agent | Output |
|---|---|---|
| Cargo workspace skeleton with the 7 crates, `rust-toolchain.toml`, lints, profiles | `release-engineer` | Builds everywhere |
| `dockering` binary opens a GPUI Kit window with an empty Sidebar + TitleBar + StatusBar | `gpui-ui` | Hello-shell |
| CI: lint, test, and build matrix (6 targets), rust-cache, NFR-001 grep, `cargo deny` | `release-engineer` | Green pipeline |
| `scripts/bootstrap.sh/.ps1` with OS prerequisites | `release-engineer` | — |
| Tracing setup, panic hook, config/state file loading (`dk-hub::config`), single instance | `rust-core` | 10 §6–7, SHL-022 |
| Windows bootstrap: `vswhere`/`vcvars64` dev script, x64 CRT check (spike F-1) | `release-engineer` | 50 |
| Licences: `LICENSE-*`, `cargo about`, `cargo deny` allowlist | `release-engineer` | REL-001…003 |
| **Early spikes S-1 (terminal render), S-5 + S-8 (DataTable groups and focus, physical-key bindings)**: their results gate M2/M4 design | `gpui-ui` | — |
| Self-hosted Windows runner with WSL ≥ 3.0, an Ubuntu distro with Docker Engine, and WSLC (label `wsl`) | `release-engineer` | 50 |

**Exit:** A window opens on Windows, macOS, and Linux from CI artifacts. Spike results are recorded.

### M1 — Core domain + hub + Docker backend (≈ 3–4 weeks)

| Task | Owner | Spec |
|---|---|---|
| `dk-core`: DTOs, `Engine` trait, `Capabilities`, `EngineError`, `TerminalSession`, `FakeEngine`, contract suite skeleton | `rust-core` | 21 §1–3 |
| `dk-core::grouping`, `dk-core::stats` (CPU %, rates, downsampling), formatting helpers + unit tests | `rust-core` | 21 §3.3, §4 |
| `dk-hub`: runtime, `HubHandle::call/subscribe`, `HubStream` drop-cancel, registry, supervisor state machine with backoff, `hub_events` | `rust-core` | 10 §3, 20 §6 |
| `dk-engine-docker`: connect (unix/npipe/tcp/tls), version negotiation, all list/inspect/action ops, events, logs, stats normalisation | `engine-integrator` | 20 §3, 21 §6 |
| Discovery: ENG-001…006 | `engine-integrator` | 20 §2 |
| Contract suite against real dockerd (Linux CI) + fixtures (`xtask record-fixtures`) | `qa-engineer` | 60 |

**Exit:** `cargo run -p dk-hub --example dump` prints containers, images, and volumes of the local engine on all OSes.

### M2 — Shell, navigation, keyboard foundation, Containers list (≈ 4 weeks)

| Task | Owner | Spec |
|---|---|---|
| `AppShell`: TitleBar, Sidebar with counts, StatusBar, `Navigator` (routes, back/forward), theme (System/Light/Dark) | `gpui-ui` | 30 |
| **Keyboard foundation**: actions + `keymap.rs`, focus regions, focus ring, roving table focus, command palette, shortcut reference, reachability tests ([plan](features/keyboard-navigation.md) tasks 1–9, 15) | `gpui-ui` + `qa-engineer` | KBD-* |
| `EngineListStore`, engine switcher popover, connection states, disconnected page | `gpui-ui` | ENG-100…103, 107, 108, SHL-013 |
| `EngineStore` with `Resource<T>`, event-driven refresh + polling fallback | `gpui-ui` + `rust-core` | 10 §4 |
| Containers page: DataTable delegate with tree flattening, grouping, sort, filter, search, row/group/bulk actions, confirmations, notifications, empty/loading/error states | `gpui-ui` | CON-* |
| View tests with `FakeEngine` | `qa-engineer` | 60 |

**Exit:** You can browse and control containers on a local Docker, grouped by Compose project.

### M3 — Container detail: Overview / Mounts / Network / Inspect / Logs (≈ 2 weeks)

| Task | Owner | Spec |
|---|---|---|
| Detail page frame, header, tab routing, removed-container banner | `gpui-ui` | CDT-001…003, 080, 081 |
| Overview (DescriptionList sections, env masking), Mounts, Network, Inspect (read-only highlighted JSON editor) | `gpui-ui` | CDT-010…040 |
| Logs view: virtual list, ANSI SGR parser (`dk-core::ansi`), follow/pause, search, timestamps, wrap, save | `gpui-ui` + `rust-core` | LOG-* |

### M4 — Terminal (≈ 3 weeks, highest UI risk)

| Task | Owner | Spec |
|---|---|---|
| Spike S-1: `alacritty_terminal` grid rendered by a GPUI element at 60 fps; key encoding | `gpui-ui` | TRM-002 |
| `dk-terminal`: `TerminalModel`, `TerminalView`, selection/copy/paste, scrollback, resize, mouse mode | `gpui-ui` | TRM-002…005, 010, 011 |
| Docker exec `TerminalSession` (hijacked stream, resize, exit code) | `engine-integrator` | 21 §5 |
| Terminal tab with sub-tabs, shell picker, `TerminalRegistry` persistence | `gpui-ui` | TRM-001, 004, 006…008 |

### M5 — Stats & charts (≈ 1.5 weeks)

| Task | Owner | Spec |
|---|---|---|
| Hub `StatsService` (dedup subscribers, ring buffers, 15 min history) | `rust-core` | STA-002, 003, 006 |
| Stats tab: 4 chart cards (AreaChart/LineChart), window selector, totals, top processes | `gpui-ui` | STA-001, 004, 005, 007, 008, 010 |
| Optional CPU/Memory columns in the containers list (visible rows only) | `gpui-ui` | CON-002 |
| Chart render benchmark (STA-009) | `qa-engineer` | — |

### M6 — Images, Volumes, Networks (≈ 2.5 weeks)

| Task | Owner | Spec |
|---|---|---|
| Images list + detail (overview, layers, used by, inspect), pull dialog with progress, run dialog, tag, delete, prune | `gpui-ui` + `engine-integrator` | IMG-* |
| Volumes list (lazy sizes via `disk_usage`), detail, create, delete, prune | `gpui-ui` | VOL-* |
| Networks list + detail, delete, prune | `gpui-ui` | NET-* |

**Exit:** Feature-complete on Docker (Linux/macOS/Windows Docker Desktop).

### M7 — Windows: WSL distros (≈ 1.5 weeks)

| Task | Owner | Spec |
|---|---|---|
| `dk-wsl`: registry enumeration, `wsl.exe` runner (UTF-16 decode, no window, timeouts), running-state detection | `windows-platform` | 20 §4.2 |
| `PipeBridge`: tokio named-pipe server with a current-user ACL, a `wsl.exe` child per connection, bidirectional copy, idle shutdown | `windows-platform` | ENG-011, NFR-021 |
| Probe (dial-stdio / socat / socket-only), hints, *Start & connect*, TCP mode | `windows-platform` | ENG-007, 012, ENG-106 |
| Contract suite through the bridge (Windows runner with a WSL distro, or manual) | `qa-engineer` | — |

### M8 — Windows: WSLC (≈ 4 weeks)

| Task | Owner | Spec |
|---|---|---|
| **Spike S-3 (COM, do first):** Rust `windows`-crate client: `CoCreateInstance(WSLCSessionManager)` → `OpenSessionByName(NULL)` → `ListContainers`, `Logs` (pipe read), `Exec` + `ResizeTty`, `GetEvents`, `PullImage` with a Rust `IProgressCallback`. Confirm the security/impersonation setup and marshalling. | `windows-platform` | 20 §5.4, 5.8 |
| Vendor `wslc.idl` + `WSLCShared.idl` at the WSL tag. Write ABI module v1 (vtables, structs, HRESULT constants). Write `xtask wslc-abi-check`. | `windows-platform` | 20 §5.3, 5.7 |
| COM transport: MTA worker pool in the hub, COM security init, proxy blanket, session cache + reconnect, `BeginContainerOperation` guards, RAII for `CoTaskMem`/handles, HRESULT → `EngineError` mapping | `windows-platform` | 20 §5.4 |
| COM transport ops: list/inspect/actions, logs (pipe → stream), polled stats, events (`GetNext` loop with a cancel event), images incl. pull progress, volumes, networks | `engine-integrator` | 20 §5.4 |
| COM exec → `TerminalSession` (std handles, `ResizeTty`, exit event) | `windows-platform` | 21 §5 |
| Spike S-2 + CLI fallback transport: process runner, tolerant parsers, ConPTY exec | `engine-integrator` | 20 §5.5 |
| Transport selection (version gating, self-check, user override), discovery ENG-008/ENG-109, transport display ENG-110 | `windows-platform` + `gpui-ui` | 20 §5.2–5.3 |
| Tests: fake in-process COM server + fake `wslc.exe`. Contract suite for both transports. Weekly WSL ABI-diff CI job. | `qa-engineer` + `release-engineer` | 21 §7, 20 §5.7 |

### M9 — Settings, polish, packaging, signing, v1.0 (≈ 3 weeks)

| Task | Owner | Spec |
|---|---|---|
| Settings page (all sections), Manage engines + Add engine dialog | `gpui-ui` | SET-*, ENG-104/105 |
| Persistence of window/UI state | `gpui-ui` | SHL-011 |
| Accessibility pass, keyboard-only E2E walkthrough on 3 OSes (US + Czech layouts), light/dark screenshot review | `gpui-ui` + `qa-engineer` + `reviewer` | 30 §5, KBD-090 |
| Packaging: MSI/zip, DMG, AppImage/deb/tar; **Authenticode signing + macOS notarisation (required for public builds)**; release workflow | `release-engineer` | 50 |
| macOS native menu, first-run screen, UI zoom | `gpui-ui` | SHL-020…024, ENG-111 |
| Performance pass against NFR-010…013; release checklist on 3 OSes | `qa-engineer` | 40 |

**Total ≈ 26–28 weeks of focused work for one developer working with agents; about 18–20 calendar weeks
when M7/M8 run in parallel with M5/M6 (they depend only on M1 and, for terminals, M4).** The earlier
12-week figure was optimistic: it didn't count the keyboard system, the full contract and test infrastructure,
the dual WSLC transports, and signing on 6 targets. Add a **15–20 % buffer** for GPUI Kit churn.

The spikes de-risked feasibility (see [spikes/2026-10-feasibility.md](spikes/2026-10-feasibility.md)), not effort.

```
M0 ─▶ M1 ─▶ M2 ─▶ M3 ─▶ M4 ─▶ M5 ─▶ M6 ─▶ M9
             └────────▶ M7 ─────▶ M8 ──┘     (M8 terminal part needs M4)
```

### Post-v1 — M10: macOS native engine (Apple `container`) — not scheduled

Tasks when scheduled: spikes S-6/S-7, `dk-engine-apple` crate (XPC transport + CLI fallback), `AppleContainerFactory` discovery, fixtures and contract suite on a macOS 26 Apple-silicon runner, ENG-120…125. Prerequisite: ENG-030…033 are honoured in M1 (`dk-core`) and M2 (UI uses capabilities, not kind).

## 2. Spikes (de-risking, do early)

| ID | Question | Timebox | Blocks |
|---|---|---|---|
| S-1 | ✅ **Done** (2026-10-02): `alacritty_terminal` grid in a GPUI element. A full reshape of a 209×52 grid takes 2.2–2.5 ms per frame; a frame with no changed rows ~0.3 ms. Key → echo 20–32 ms on a debug build (TRM-010). IME/AltGr via `InputHandler`. | 2 days | M0 → M4 |
| S-2 | ✅ **Done** (F-10, 2026-10-02): `wslc` 3.0.1 CLI shapes, exit codes, and stderr texts recorded as fixtures (spec 20 §5.5) | 0.5 day | M8 (fallback) |
| S-3 | ✅ **Done except `CreateContainer`** (F-6…F-8, 2026-10-02): session manager, sessions, ListContainers, ListNetworks, Stats, Logs handles, Exec + TTY + ResizeTty, security init order, `GetEvents` stream, and `PullImage` with a Rust `IProgressCallback`, all verified live. Remaining: the `CreateContainer` options layout (`run_image` over COM returns 501 until verified, spec 20 §5.4) | 1.5 days | M8 (primary transport) |
| S-4 | ◐ **Partially done**: bridge request/response and events (F-4); bridge live tests cover request/response and concurrent connections. Remaining: bollard hijacked exec through the bridge (resize, stdin half-close) is not separately verified | 0.5 day | M7 |
| S-5 | ✅ **Done**: group rows (indent and chevron) are rendered through the `ListTable` wrapper over GPUI Kit `DataTable` | 0.5 day | M0 → M2 |
| S-6 | (post-v1) Rust XPC client for `com.apple.container.apiserver`: list, logs fd passing, stats, exec + resize, `containerEvent` | 2 days | M10 |
| S-7 | (post-v1) Apple image helper XPC vs CLI for image ops | 0.5 day | M10 |
| S-8 | ✅ **Done** (2026-10-02, [report](spikes/2026-10-s8-focus-keys.md)): focus APIs, `ListTable > DataTable` binding overrides, roving focus. KBD-085: physical keys via the keyboard mapper on Windows (full) and macOS (partial); Linux logical only (known gap) | 1 day | M0 → M2 |

Spike results are recorded as ADRs or spec updates.

## 3. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| GPUI / GPUI Kit API churn (pre-1.0) | Breakage on upgrade | Pin exact versions. Upgrade in dedicated PRs. Wrap less-stable components in thin local adapters (`ui::table`, `ui::chart`). |
| Internal WSLC COM ABI changes in a WSL update (Microsoft says it may) | WSLC COM transport unusable on the new WSL | Version-gated ABI modules + self-check → automatic CLI fallback (degraded, never broken). Weekly IDL diff job. Never call unverified vtables. |
| WSLC CLI output changes | CLI fallback breaks | Tolerant parsers, version detection, fixture tests |
| `wsl.exe` process per connection | Measured ~65 ms spawn, ~240 ms first call (F-4), so acceptable | hyper connection pooling. Cap list stats streams at 8 on bridge engines (STA-006). |
| Terminal emulator complexity | M4 slips | Spike S-1 first. Ship a basic VT first (no mouse mode), then iterate. |
| Linux Vulkan requirement | Some VMs can't run the app | Document it. Suggest Mesa `lavapipe` as the software fallback. |
| The same daemon is reachable via several endpoints (two Docker Desktop pipes, WSL integration distros, F-4) | Duplicate engines | Endpoint canonicalisation + dedupe by `/info.ID` (ENG-009) |
| GitHub-hosted runners can't run WSL2/WSLC | M7/M8 untestable in CI | Self-hosted `wsl` runner (M0) + manual release checklist |
| COM security must be initialised before GPUI (F-8) | WSLC COM unusable if the order is wrong | Fixed bootstrap order (10 §7) + test that asserts `CoInitializeSecurity` returns S_OK at startup |

## 4. Repository layout (target)

```
.
├── CLAUDE.md                     # agent operating manual (read first)
├── README.md
├── Cargo.toml / Cargo.lock / rust-toolchain.toml / deny.toml
├── crates/{dk-core,dk-hub,dk-engine-docker,dk-engine-wslc,dk-wsl,dk-terminal,dockering}
├── xtask/                        # record-fixtures, lint helpers
├── assets/ packaging/ scripts/
├── .github/workflows/{ci.yml,release.yml}
├── .claude/
│   ├── agents/*.md               # sub-agent definitions
│   └── skills/feature-planning/  # spec-driven planning skill
└── docs/
    ├── spec/                     # source of truth (what)
    └── plan/                     # roadmap, ADRs, per-feature plans (how)
        ├── README.md             # this file
        ├── adr/
        └── features/
```

## 5. Agentic workflow (how work flows through the repo)

```
 idea / issue
     │
     ▼
 /feature-planning <idea>          (skill; run by main agent with `architect`)
     │  reads docs/spec/**, writes docs/plan/features/<slug>.md,
     │  updates docs/spec/features/<x>.md (+ CHANGELOG), status → planned
     ▼
 user approves plan
     │
     ▼
 implementation, per plan task, delegated to owner agents
     │  rust-core · engine-integrator · windows-platform · gpui-ui · release-engineer
     │  each task references requirement IDs; spec status → in-progress
     ▼
 qa-engineer (tests)  ──▶  reviewer (spec + NFR + threading review)
     │
     ▼
 /feature-planning complete <slug>  → reconcile spec with what was built,
                                      status → implemented, CHANGELOG entry
```

Agent definitions are in `.claude/agents/`. Rules every agent follows are in `CLAUDE.md`.
