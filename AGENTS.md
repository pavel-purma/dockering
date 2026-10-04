# AGENTS.md — Shared operating manual for agents working on Dockering

Dockering is a cross-platform (Windows/macOS/Linux) desktop client for container engines.
It's written in Rust with **GPUI Kit** (`gpui-kit` 0.7) and talks to Docker over a socket, pipe,
or TCP, to Docker inside WSL distros, and to WSL containers (WSLC).

## Read before you work

1. `docs/spec/README.md`: the spec index and conventions (requirement IDs, statuses).
2. The spec files relevant to your task, especially `docs/spec/10-architecture.md` (threading rules) and `docs/spec/21-engine-api-contract.md` (the `Engine` trait and DTOs).
3. `docs/plan/README.md`, if present: milestones, and which agent owns what.
4. The feature plan in `docs/plan/features/` if one exists for your task.

**The spec is the source of truth.** If the code you're about to write disagrees with the spec, stop.
Either follow the spec, or update the spec first through the `feature-planning` skill and say so.

## Hard rules

- **Never block the UI thread.** No `std::fs`, `std::process`, `std::net`, `block_on`, `thread::sleep`, or contended locks in `crates/dockering/src/**` or `crates/dk-terminal/src/view/**` (both covered by the CI grep). Engine I/O goes through `HubHandle::call` / `subscribe`. CPU-heavy work goes through `cx.background_spawn`. (NFR-001…005)
- **Store task handles.** `gpui::Task`s live on the entity that owns them. Use `.detach()` only for fire-and-forget actions that report through a notification.
- **Guard stale results** with a revision or request id before applying async results.
- **Layering** (ADR-0001): `dk-core` has no tokio, bollard, or GPUI. Only `dockering` and `dk-terminal` use GPUI Kit. The UI never names tokio types.
- **Keyboard shortcuts** (`docs/spec/features/keyboard.md`): almost every action is reachable by a shortcut. Every command is a GPUI `Action` with a binding in `keymap.rs` or a command-palette entry, and restores focus correctly. No focus rings or accent focus borders (KBD-003).
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

## Shared agent setup

- This file is the shared repository guidance for Claude Code and OpenCode. `CLAUDE.md`
  imports it for Claude Code; OpenCode discovers `AGENTS.md` automatically.
- Agent definitions and prompts have one source in `.claude/agents/`. Claude Code loads
  them natively; `opencode.json` registers the same names and reads those files as prompts.
  OpenCode permissions are defined in its config, not by the Claude frontmatter.
- Skills and their supporting files have one source in `.agents/skills/`, the neutral
  discovery location supported by OpenCode and Codex. Claude Code command adapters in
  `.claude/commands/` read the shared skills; OpenCode commands load them by name.
- For feature planning, revision, completion, or status requests, load `feature-planning`
  with the host's skill tool, or read `.agents/skills/feature-planning/SKILL.md` if the host
  does not discover that directory. Resolve supporting files relative to its directory.
- Use the host's equivalent tools: `AskUserQuestion` in Claude Code is `question` in
  OpenCode; `Task` delegation is `task`; `Read`/`Grep`/`Glob` are `read`/`grep`/`glob`.
- See `docs/agent-setup.md` for usage, maintenance, and validation commands.

## Commit style

Conventional commits: `feat(containers): group rows by compose project [CON-010]`.
Scopes: `core`, `hub`, `docker`, `wsl`, `wslc`, `terminal`, `ui`, `containers`, `images`,
`volumes`, `networks`, `settings`, `keyboard`, `ci`, `docs`, `spec`.
