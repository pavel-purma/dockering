# CLAUDE.md — Operating manual for agents working on Dockering

Dockering is a cross-platform (Windows/macOS/Linux) desktop client for container engines.
It's written in Rust with **GPUI Kit** (`gpui-kit` 0.7) and talks to Docker over a socket, pipe,
or TCP, to Docker inside WSL distros, and to WSL containers (WSLC).

## Read before you work

1. `docs/spec/README.md`: the spec index and conventions (requirement IDs, statuses).
2. The spec files relevant to your task, especially `docs/spec/10-architecture.md` (threading rules) and `docs/spec/21-engine-api-contract.md` (the `Engine` trait and DTOs).
3. `docs/plan/README.md`: milestones, and which agent owns what.
4. The feature plan in `docs/plan/features/` if one exists for your task.

**The spec is the source of truth.** If the code you're about to write disagrees with the spec, stop.
Either follow the spec, or update the spec first through the `feature-planning` skill and say so.

## Hard rules

- **Never block the UI thread.** No `std::fs`, `std::process`, `std::net`, `block_on`, `thread::sleep`, or contended locks in `crates/dockering/src/**` or `crates/dk-terminal/src/view/**` (both covered by the CI grep). Engine I/O goes through `HubHandle::call` / `subscribe`. CPU-heavy work goes through `cx.background_spawn`. (NFR-001…005)
- **Store task handles.** `gpui::Task`s live on the entity that owns them. Use `.detach()` only for fire-and-forget actions that report through a notification.
- **Guard stale results** with a revision or request id before applying async results.
- **Layering** (ADR-0001): `dk-core` has no tokio, bollard, or GPUI. Only `dockering` and `dk-terminal` use GPUI Kit. The UI never names tokio types.
- **Keyboard-first UI** (`docs/spec/features/keyboard.md`): everything must work without a mouse. Every command is a GPUI `Action` with a binding in `keymap.rs` or a command-palette entry, has a visible focus ring, and restores focus correctly.
- **GPUI Kit first.** Use GPUI Kit components and theme tokens. Write custom elements only when no component exists, and note it in the PR.
- **Capability gating.** Any op that isn't universal is checked against `Capabilities` in the UI. Never branch UI behaviour on `EngineKind` (ENG-030). New runtimes plug in as an `EngineFactory` crate (spec 21 §0/§8). Apple `container` is reserved but deferred (ADR-0005).
- **No shell interpolation.** Child processes get argv vectors. Validate ids and names (NFR-022).
- **No secrets in logs** (NFR-020).
- Platform-specific code uses `#[cfg(...)]` inside crates. Every crate compiles on every OS.
- **WSLC COM** (ADR-0003): the internal `IWSLC*` ABI may change with any WSL release. Only call vtables from a verified ABI module selected by version + self-check. Otherwise fall back to the CLI transport. `unsafe` stays inside `dk-engine-wslc/src/com/`. The WSL version used to pick the ABI module comes from `wslservice.exe` (no COM), and `CoInitializeSecurity` + `WSAStartup` run in `main()` before GPUI (spec 10 §7, spike report `docs/plan/spikes/2026-10-feasibility.md`).

## Commands

```sh
cargo run -p dockering                         # run the app
cargo nextest run --workspace                  # all tests (or: cargo test --workspace)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask record-fixtures --engine <id>   # record engine fixtures (needs a live engine; xtask alias in .cargo/config.toml)
pwsh -NoProfile scripts/dev.ps1 <cargo args> # Windows: runs cargo inside the VS Build Tools environment
```

## Workflow

- New feature or behaviour change → `/feature-planning <description>`. It produces a plan and updates the spec. Wait for user approval before implementing.
- Implement plan tasks by delegating to the owning agent (`.claude/agents/`):
  `architect`, `rust-core`, `engine-integrator`, `windows-platform`, `gpui-ui`,
  `qa-engineer`, `reviewer`, `release-engineer`.
- Reference requirement IDs (for example, `CON-011`) in commit messages, PR descriptions, and test names.
- When a feature is complete → `/feature-planning complete <slug>` to reconcile the spec, set the status to `implemented`, and add a `docs/spec/CHANGELOG.md` entry.
- Definition of Done: `docs/spec/60-quality.md`.

## Commit style

Conventional commits: `feat(containers): group rows by compose project [CON-010]`.
Scopes: `core`, `hub`, `docker`, `wsl`, `wslc`, `terminal`, `ui`, `containers`, `images`,
`volumes`, `networks`, `settings`, `keyboard`, `ci`, `docs`, `spec`.
