# Spec Changelog

Newest first. One line per change: date · area · summary · link to plan or PR.

- 2026-10-02 · all · **Whole-plan review + feasibility spikes.** Spikes F-1…F-12 ran on real Docker, WSL distro, and WSLC 3.0.1 ([report](../plan/spikes/2026-10-feasibility.md)). Fixes:
  - **Hub API:** HubHandle API completed: terminal sessions are hub-side actors with a channel handle, `Feed::Lagged` for events/stats, single owner per resource.
  - **WSLC COM:** ABI module chosen from the `wslservice.exe` version (no COM). COM threading split into an RPC pool plus a thread per stream. `CoInitializeSecurity` and `WSAStartup` run before GPUI.
  - **WSLC CLI:** mapping corrected to verified 3.0.1 behaviour (NDJSON lists, text events, `system session list`, ~1 s stats).
  - **New requirements:** ENG-009/010 (de-duplication by daemon id, unsupported endpoints), ENG-111/112 (first run, unsupported list), IMG-007 (registry auth from Docker config), CON-013 (Compose group action semantics), SHL-020…024 (macOS menu, single instance, Linux app id, English-only, zoom), REL-001…003 (licensing).
  - **Spec fixes:** request DTOs defined. Per-kind argument validation (NFR-022). Stats stream caps per transport. Terminal parsing on the foreground thread with IME via `InputHandler`.
  - **Keyboard conflicts resolved:** quick find via `/` instead of type-ahead; no `Alt+digit` or digit-sort chords; physical-key matching (KBD-085); macOS Cmd chords bypass the PTY; multi-selection semantics.
  - **Releases:** signing and notarisation required for public builds.
  - **Estimates:** revised to ≈ 26–28 dev-weeks.

- 2026-10-01 · keyboard · New feature spec `keyboard.md` (KBD-001…093): full keyboard operability, focus regions, a command palette, a shortcut reference, single-letter row actions, terminal escape hatch, and layout safety. SHL-010 rewritten. Added CON-034, CDT-082, LOG-009, TRM-012, STA-011, SET-070, NFR-042. DoD gains a keyboard item. · [plan](../plan/features/keyboard-navigation.md)
- 2026-10-01 · engines/architecture · Documented backend layering (`Engine` contract + `EngineFactory`, spec 21 §0/§8). Researched Apple `container` (macOS native) and added it as a **deferred** backend (spec 20 §7, feature `macos-native-engine`). Binding v1 requirements ENG-030…033; future ENG-120…125. · [ADR-0005](../plan/adr/0005-backend-extensibility-apple-container.md)
- 2026-10-01 · engines/WSLC · WSLC backend switched from "CLI adapter only" to **native COM (internal `IWSLCSession` API) as the primary transport + automatic `wslc.exe` CLI fallback**. Version-gated ABI modules. Added ENG-013, ENG-110. The capability matrix gains structured pull progress over COM. · [ADR-0003](../plan/adr/0003-wslc-transport.md)
- 2026-10-01 · all · Initial specification (v1 scope): architecture, engine backends (Docker, WSL distro, WSLC), API contract, UI shell, feature specs for engines, containers, container detail, logs, terminal, stats, images, volumes, networks, settings. · [plan](../plan/README.md)
