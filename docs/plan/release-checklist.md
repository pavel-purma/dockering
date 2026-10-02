# Release checklist (manual)

- **Owner:** `qa-engineer` (initial version by `architect`, 2026-10-02)
- **When:** before every public release tag (`v*`), on a release build of the exact commit being tagged
- **Spec:** [60-quality.md](../spec/60-quality.md) (E2E manual layer), [40-non-functional.md](../spec/40-non-functional.md), [keyboard.md](../spec/features/keyboard.md) KBD-090, [50-build-and-release.md](../spec/50-build-and-release.md)

Copy this file into the release PR description (or an issue), tick each box, and note the OS
build, engine versions, and any deviation. A failed MUST item blocks the release. A failed
SHOULD item needs a tracked issue.

## 0. Run record

| Field | Value |
|---|---|
| Commit / tag | |
| Windows build (x64 / arm64) | |
| macOS version (arm64 / x64) | |
| Linux distro + desktop (X11/Wayland) | |
| Docker versions (Desktop / Engine) | |
| WSL version (`wsl --version`) + WSLC ABI module | |
| Tester / date | |

## 1. Automated gates (green on the tag commit)

- [ ] CI `ci.yml` green on all 6 targets (lint, clippy `-D warnings`, nextest, NFR-001 `cargo xtask check-blocking`, `cargo deny`, THIRD_PARTY_LICENSES fresh)
- [ ] `wsl-integration.yml` run manually on the self-hosted `wsl` runner: `-p dk-wsl -p dk-engine-wslc -- --ignored` green
- [ ] Docker live tests (`dk-engine-docker --features it`) green against a real daemon on Linux, and on Windows (Docker Desktop)
- [ ] `wslc-abi-watch` last weekly run green, or no open "WSLC ABI changed" issue

## 2. Keyboard-only walkthrough (KBD-090, NFR-042)

Disconnect or ignore the mouse. Run on **each OS**, and on Windows **and** Linux with both a **US** and a **Czech** layout (macOS: US + Czech if available).

| # | Step | Expect | Win US | Win CZ | mac | Linux US | Linux CZ |
|---|---|---|---|---|---|---|---|
| 1 | `Mod+K`, type to filter, arrows, `Enter` | Engine switches; focus lands on the list | | | | | |
| 2 | `Mod+1` (CZ: physical `1` key) | Containers page, table focused | | | | | |
| 3 | Arrow to a Compose group, `→` / `←`, `Mod+→` | Expand / collapse / expand all | | | | | |
| 4 | `Space`, `Shift+↓`, `Mod+A`, `Esc` | Selection toggles, extends, all, clears | | | | | |
| 5 | `/` + text, `Enter` | Quick find jumps to matches | | | | | |
| 6 | `Shift+F10` (and `Menu` key) | Row menu with shortcuts; arrows + `Enter` run an item | | | | | |
| 7 | `Enter` on a running container | Detail opens, first tab focused | | | | | |
| 8 | `Ctrl+Tab` through all 7 tabs | Each tab renders; disabled tabs skipped | | | | | |
| 9 | Overview: ↑/↓, `Space` on a masked env, `Mod+C`, `Enter` on the image row | Reveal, copy, navigate | | | | | |
| 10 | Terminal tab: type `ls`, `Tab` completion, `Esc` in vim | Keys reach the shell | | | | | |
| 11 | `Mod+Shift+T`, `Ctrl+PgDn`, `Mod+Shift+W` | Sub-tab new / switch / close | | | | | |
| 12 | `Mod+Shift+F6` (mac also `Cmd+Esc`) | Focus leaves the terminal to the tab bar | | | | | |
| 13 | Logs: `Mod+F`, type, `Enter` / `Shift+Enter`, `F3`, `Alt+T`, `Alt+W`, `End` | Search and toggles work; follow resumes | | | | | |
| 14 | Stats: focus selector, `←/→`, `1`/`5`/`F` | Window changes; values readable as text | | | | | |
| 15 | Header `S` (stop), `S` (start) | State changes; spinner, then the refetched state | | | | | |
| 16 | `Del` on a container, `Tab` to *Delete*, `Enter` (or `Mod+Enter`) | Confirm starts on *Cancel*; deletes; focus moves to the next row | | | | | |
| 17 | `Alt+↑` (mac `Cmd+[` for back) | Back to the list on the right row | | | | | |
| 18 | `Mod+2`, `G`, type `alpine:3.20`, `Enter` | Pull notification with progress; *Cancel* reachable | | | | | |
| 19 | `Mod+Shift+N` | Newest toast focused; `Tab` to its action; `Esc` dismisses (KBD-074, no automated test) | | | | | |
| 20 | `Mod+3`, `N`, fill the form, `Mod+Shift+Enter` adds a row, `Mod+Enter` | Volume created | | | | | |
| 21 | `Mod+,`, arrows in the section nav, `Tab` through every control | Every control reachable, no trap | | | | | |
| 22 | Settings → Engines → *Add engine…*, `Esc` | Dialog closes; focus back on the invoker | | | | | |
| 23 | `Mod+/` (and `F1`), type to filter | Shortcut reference shows only this OS's bindings | | | | | |
| 24 | `F6` / `Shift+F6` repeatedly | Cycles Title bar → Sidebar → Content → Status bar and restores focus | | | | | |
| 25 | `Mod+Shift+P` → "Quit" (Win/Linux), `Cmd+Q` (mac) | App quits | | | | | |

- [ ] Known gap confirmed and documented in the release notes: on **Linux with a Czech layout**, `Mod+1…4`, `Mod+/`, `Mod+,`, and zoom don't fire (KBD-085). The palette works.

## 3. Real engines per OS

### Windows 11
- [ ] **Docker Desktop** (npipe): discovered, connects, browse containers/images/volumes/networks, start/stop, logs follow, terminal, stats, pull, run
- [ ] **WSL distro** with Docker Engine (bridge): discovered under *WSL distros*. A stopped distro shows *Start & connect* and isn't booted by background pings (ENG-106). Terminal, logs, and stats work. List stats are capped at 8.
- [ ] Docker Desktop's WSL integration distro is **merged** into the Docker Desktop engine ("also reachable via", ENG-009)
- [ ] **WSLC over COM**: status bar / Settings → Engines show `via com`, the WSL version, and the note "Run via COM is not verified…". Lists, logs, exec + resize, events, stats (2 s), and pull with structured progress work. *Run* shows the 501 error notification (known v1 limitation).
- [ ] **WSLC over CLI**: add the session as a manual engine with transport *CLI*. Lists, logs, exec (ConPTY), stats (≥ 1 s), and **run** work. The CPU/Mem list columns are hidden.
- [ ] Kill `dockerd` (or quit Docker Desktop) while connected: *Reconnecting…*, then automatic recovery
- [ ] Switching engines while logs stream: the stream stops and no stale rows appear
- [ ] TRM-009: with a template such as `wt.exe {cmd}`, *Open in external terminal* works on Docker Desktop and is hidden on WSL-distro and WSLC engines

### macOS (Apple silicon, and Intel if shipping x64)
- [ ] Docker Desktop **or** Colima/OrbStack socket discovered; full browse + actions + terminal + logs + stats
- [ ] Native menu bar, `Cmd+Q`/`Cmd+W`, `Cmd+H`/`Cmd+M` keep their system meaning

### Linux (Ubuntu 24.04 or similar; X11 and Wayland if possible)
- [ ] `/var/run/docker.sock` connects. With the user **not** in the `docker` group, the permission hint is shown (ENG-107).
- [ ] Rootless socket (`$XDG_RUNTIME_DIR/docker.sock`) discovered if present
- [ ] Remote **TCP+TLS** engine added through *Add engine* (CA/cert/key pickers), *Test*, then *Save*
- [ ] Plain TCP shows the persistent insecure chip (NFR-021)

### Every OS
- [ ] First run with no engine: the welcome page, with per-OS guidance, *Rescan*, and *Add engine…* (ENG-111)
- [ ] `docker compose up` of a 3-service project: one collapsed group → 3 rows. *Stop all* → `0/3 running`. *Delete all* lists the members and keeps networks/volumes (CON-013).
- [ ] A container started from the CLI appears in under 1 s (CON-030)

## 4. Performance (NFR-010…013, release build)

| ID | Check | Budget | Win | mac | Linux |
|---|---|---|---|---|---|
| NFR-010 | Cold start → first frame (warm disk; stopwatch or trace timestamp) | ≤ 500 ms | | | |
| NFR-011 | Scroll a 1,000-container list; Stats tab with 4 charts + 1 log stream live (GPUI frame-time overlay) | 60 fps, no hitches | | | |
| NFR-012 | Idle CPU, engine connected, Containers page, no stats columns, 1 min | < 1 % | | | |
| NFR-013 | RSS with 1,000 containers, 1 log view at its cap (50k lines), 4 charts | < 250 MB | | | |
| TRM-010 | Keypress → echo, local engine | < 30 ms | | | |
| STA-009 | `cargo test -p dockering --release sta_009 -- --nocapture` printed time | ≤ 2 ms | | | |

A 1,000-container fixture: `for i in $(seq 1 1000); do docker create --name dk-perf-$i alpine true; done` (clean up afterwards).

## 5. WSLC ABI module validation (ADR-0003, spec 20 §5.3/§5.7)

Required for every release, and **before shipping any new ABI module**.

- [ ] `wsl --version` on the test machine matches a module's verified range (`v3_0`: `3.0.0..=3.0.x`)
- [ ] `cargo xtask wslc-abi-check latest` → no ABI change versus the newest vendored IDL (`crates/dk-engine-wslc/idl/3.0.1/`), or a new module ships with this release
- [ ] The COM self-check passes on a live session (`com_live::live_factory_discover_connect_auto_uses_com`)
- [ ] The full live COM suite is green: `pwsh scripts/dev.ps1 test -p dk-engine-wslc --test com_live -- --ignored` (lists, events, stats/logs/exec on a running container, volume create/remove, pull with progress callback, CLI preference)
- [ ] The CLI contract suite is green against real `wslc.exe` (`cli_live`, ignored tests)
- [ ] On a WSL version **outside** every module's range (or with a forced mismatch), the engine falls back to CLI with the note "WSL x.y.z not yet verified — using CLI" (or "COM self-check failed — using CLI"), and no COM vtable call is made
- [ ] `CoInitializeSecurity` succeeded at startup: with log level *Debug*, the log has `platform initialised com_security=true`, which means it ran before GPUI

## 6. Packaging & signing (spec 50)

- [ ] `release` environment secrets present: Azure Trusted Signing (`AZURE_*`) and Apple (`APPLE_*`). Without them the build is **unsigned and must not be published**.
- [ ] Windows: `.msi` (x64), NSIS `.exe` (arm64), and `.zip` install and launch. `signtool verify /pa` passes on `dockering.exe` and the installer. No SmartScreen block on a clean VM.
- [ ] macOS: `.dmg` is signed (Developer ID), notarised, and stapled. `spctl -a -vv` accepts it. Gatekeeper opens it on a clean user account (macOS 15+).
- [ ] Linux: `.AppImage` runs on a clean Ubuntu; `.deb` installs and appears in the app launcher with the app id `dev.dockering.Dockering` (SHL-022); `.tar.gz` runs
- [ ] `THIRD_PARTY_LICENSES.html` bundled in every package and opened from Settings → Diagnostics → Licences (REL-002)
- [ ] Single instance: a second launch focuses the first window (SHL-022)
- [ ] Checksums file attached to the GitHub Release. The release notes include the *Known gaps (v1)* summary.

## 7. Visual review (light + dark)

- [ ] Screenshots in **light and dark** of: first run, Containers (grouped, with the bulk bar), container detail (each tab), Images + pull notification, Volumes, Networks, Settings (each section), engine switcher, shortcut reference, a destructive confirm, an error toast
- [ ] No focus ring or accent border on any focused control (KBD-003)
- [ ] The Network/Disk overlaid line charts are aligned (shared axes, no doubled labels) (STA-004)
- [ ] HiDPI and mixed-DPI monitors render crisply (NFR-041). UI zoom (`Mod+=`/`Mod+-`/`Mod+0`) persists.
- [ ] `reviewer` signs off on the screenshot set
