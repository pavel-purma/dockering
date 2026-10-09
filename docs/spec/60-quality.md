# 60 — Quality

## Test pyramid

| Layer | Where | What | Tooling |
|---|---|---|---|
| Unit | `dk-core` | grouping, stats math (CPU %, rates, downsampling), size and time formatting, filters and search, id validation | `cargo test`, `proptest` for stats math |
| Parser | `dk-engine-docker`, `dk-engine-wslc` | DTO mapping from recorded JSON fixtures (`tests/fixtures/<engine>/<op>/*.json`) | snapshot tests (`insta`) |
| Contract | each engine | `dk_core::contract::run_suite` (see [21 §7](21-engine-api-contract.md#7-contract-tests)) | fake `wslc.exe` binary, fake in-process WSLC COM server, fixture HTTP server, real dockerd in CI |
| Hub | `dk-hub` | supervisor state machine, backoff, cancellation on drop, stale-revision handling, StatsService dedup | `tokio::test(start_paused = true)`, `FakeEngine` |
| View | `dockering` | stores react to hub results; pages render states (loading, empty, error, data); actions call the hub | `gpui::TestAppContext` + `FakeEngine` |
| E2E (manual) | release checklist | Real engines on all three OSes, including WSL distro + WSLC on Windows | `docs/plan/release-checklist.md` |
| E2E (Linux, automated; first GitHub run pending) | `linux-smoke.yml` ([distribution §8](features/distribution.md#8-linux-install-and-launch-smoke-test-rel-070077)) | The published Linux packages installed in clean Ubuntu, Fedora and Arch containers; launch, keyboard walk, quit, screenshots; software rendering, `--demo` engine | Xvfb, headless sway, xdotool/wtype, AT-SPI |

`dk-core` provides `FakeEngine`: a scriptable in-memory engine with configurable latency,
errors, and event injection. It's used by the hub and view tests. View tests MUST NOT need
Docker.

## Fixtures

- Recorded with `cargo xtask record-fixtures --engine <id>`, which calls each op against a live engine and writes sanitised JSON (env values and auth stripped).
- WSLC fixtures are recorded on a Windows machine with WSL ≥ 2.9.3: CLI output (spike S-2) and COM responses (the JSON strings and struct arrays returned by `IWSLCSession`, spike S-3). They're committed and refreshed when WSL changes.

## Definition of Done (per feature PR)

1. Requirement IDs are referenced in the PR description. The feature spec status is updated (`in-progress` → `implemented` when complete).
2. Unit, parser, and view tests cover the new logic and all four UI states (loading, empty, error, data).
3. `cargo fmt`, `clippy -D warnings`, and the NFR-001 grep pass. CI is green on all three OSes.
4. No blocking call on the UI thread. New long-lived tasks are stored on an entity.
5. Capability gating is applied for any op that isn't universal.
6. A screenshot (light and dark) is attached to the PR for UI changes.
7. **Keyboard:** every new control or action is a GPUI `Action`, has a binding or a command-palette entry, is reachable by Tab/arrows, shows its shortcut in its tooltip/menu, and has a keystroke view test (KBD-002, 010, 091…093).
8. The PR description says what changed in the spec, because the release skill writes `docs/spec/CHANGELOG.md` from it at release time (REL-019). Neither changelog gets an entry in the PR.
