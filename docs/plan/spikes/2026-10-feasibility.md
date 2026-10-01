# Feasibility spikes: results (2026-10-01 / 02)

These are throwaway probes in a scratch crate, run on the maintainer's Windows 11 machine with Docker Desktop
29.8.0 (API 1.56), WSL **3.0.1** (WSLC GA), and an Ubuntu-22.04 WSL distro.
The probe code isn't in the repo. Its findings are recorded here and fed into the spec.

## Summary

| # | Question | Result | Evidence |
|---|---|---|---|
| F-1 | Do GPUI Kit 0.7, bollard 0.21, tokio, alacritty_terminal 0.26, portable-pty 0.9, and windows 0.61 build together on Windows MSVC? | ✅ Yes, after loading the **VS Build Tools** environment (`vcvars64.bat`). Without it, linking failed (`msvcrt.lib` not found; the VS 2026 Enterprise install had no x64 CRT libs). CMake comes from Build Tools, not `PATH`. | `cargo build` OK |
| F-2 | Does a GPUI Kit window open, render, and quit? | ✅ | example `window` |
| F-3 | Does bollard on a dedicated tokio runtime work against the local daemon (named pipe)? | ✅ version/list in **~40 ms**, events stream opens | example `main` |
| F-4 | **WSL distro bridge** (ADR-0004): named-pipe server → `wsl.exe -d Ubuntu-22.04 --exec docker system dial-stdio` → unmodified bollard | ✅ Works. First call ~240 ms. 5 pooled pings ~330 ms total. One `wsl.exe` spawn is **~65 ms**. ⚠ On this machine the distro's `docker` is Docker Desktop's WSL integration, so the bridge reaches the **same daemon** as `\\.\pipe\docker_engine`. Engines MUST be de-duplicated by `/info.ID`. | example `main` |
| F-5 | `alacritty_terminal` parses ANSI into a grid | ✅ | example `term` |
| F-6 | **WSLC internal COM** (ADR-0003) from Rust (`windows` crate, hand-declared vtables from `wslc.idl` @ tag 3.0.1) | ✅ `GetVersion` → 3.0.1 (3 ms). `ListSessions` → 2 sessions. `OpenSessionByName(NULL)` → `wslc-cli-<user>`. `ListContainers` → **~4 ms**, labels and ports included. `ListNetworks` → Docker-shaped JSON. | example `wslc_com` |
| F-7 | WSLC COM: `Stats`, `Logs` (pipe handles), `Exec` with TTY + `ResizeTty` | ✅ `Stats()` returns the **raw Docker stats JSON** (cumulative counters). `Logs` returns socket-type `WSLCHandle`s (type 2) that are readable with `ReadFile`. `Exec` with `Tty` flags gives a TTY handle (type 3). `stty size` printed `24 80`, then `40 132` after `ResizeTty`, and `$TERM` was passed. Exit code came from `GetState`. ⚠ The handles are **sockets**, so the process MUST call `WSAStartup` before reading them, otherwise `WSANOTINITIALISED (0x8007276D)`. | example `wslc_exec` |
| F-8 | `CoInitializeSecurity` together with GPUI | ✅ Works when called in `main()` **before** `application().run()` (on a throwaway MTA thread). ❌ Called after GPUI started, it fails with `RPC_E_TOO_LATE (0x80010119)`. COM calls from an MTA worker while the GPUI window is open: ✅. | examples `gpui_com`, `gpui_com2` |
| F-9 | WSL version **without COM** (needed to pick an ABI module safely) | ✅ `C:\Program Files\WSL\wslservice.exe` FileVersion = `3.0.1.0`. Also the Appx package version, and `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss\MSI\Version`. | PowerShell |
| F-10 | WSLC CLI (fallback) reality check vs spec | Mostly as specified. Differences: `session` is under `wslc system session` (no `--format json`, table only). `system events` has **no `--format`**: text lines like `<ts> container start <id> (k=v, …)`. `container list --format json` emits **NDJSON** (one object per line), with human strings (`"Size":"0B"`, `"CreatedAt":"… +0200 SELČ"`, a localised TZ name!). `stats` is a one-shot snapshot (`--format json`), **~1.05 s per call** (sampling window). `list` is ~60 ms. Volume driver is `guest`. | `wslc` 3.0.1 |
| F-11 | WSLC port publishing | ✅ `-p 18080:80` binds `127.0.0.1:18080` on Windows. HTTP 200 from Windows. Ports also appear in the `com.microsoft.wsl.container.metadata` label. | curl |
| F-12 | IDL drift between tag 3.0.1 and `main` (2026-10-01) | `IWSLCSessionManager` is identical. The remaining diff is outside the probed slots. The weekly ABI-diff job is still required. | diff |

## Consequences for the spec (applied 2026-10-02)

1. **ABI module selection** uses the version from F-9 (no COM). `GetVersion` over COM is only a post-selection self-check (spec 20 §5.3).
2. **`CoInitializeSecurity` placement** is in `main()` before GPUI starts (F-8). It's a bootstrap step in spec 10 §7.
3. **`WSAStartup`** runs at process start on Windows (F-7).
4. **WSLC stats**: COM `Stats()` gives raw counters, so the Docker normaliser applies unchanged and polling at 2 s is cheap. The CLI fallback `stats` costs ~1 s per container, so the CLI transport caps live stats to the detail page only (no list columns).
5. **WSLC events in CLI fallback** are parsed from text lines. Everywhere else, COM `GetEvents` (JSON) is the primary path.
6. **Engine de-duplication** by `/info.ID` is mandatory (F-4).
7. **Windows dev prerequisite**: VS Build Tools 2022+/2026 with the x64 CRT plus the Windows SDK. `scripts/bootstrap.ps1` must locate `vcvars64.bat` via `vswhere`.
8. The **CLI-only fallback** is confirmed viable but clearly second-class (stats latency, text events, localised dates).
