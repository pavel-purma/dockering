# Feature: Engine connections & switching

- **Status:** in-progress
- **Requirement prefix:** ENG (backend reqs ENG-001…025 live in [20-engine-backends.md](../20-engine-backends.md))
- **Plan:** [docs/plan/features/engines.md](../../plan/features/engines.md) (to be written)

## User stories

- As a Windows user with Docker inside `Ubuntu-22.04` (WSL) and also WSLC, I see both engines in the switcher without configuring anything, and I can switch between them in one click.
- As a Linux user, the app connects to `/var/run/docker.sock` on first launch. If permission is denied, I'm told exactly how to fix it.
- As a user with a remote Docker host, I can add a TCP+TLS engine manually.

## Requirements

| ID | Requirement |
|---|---|
| ENG-100 | The title bar MUST show the active engine: kind icon, name, and a status dot (green connected, amber connecting/degraded, red failed, grey disabled or stopped). |
| ENG-101 | The engine switcher (`Ctrl/Cmd+K`) MUST list every enabled engine grouped by kind, with status, version, and OS/arch on hover. |
| ENG-102 | Selecting an engine MUST switch the whole window to it within one frame. Data loads asynchronously, with skeletons. |
| ENG-103 | First launch MUST auto-select the first *connected* engine. Preference order: `DOCKER_HOST` > current Docker context > local socket/pipe > WSL distro > WSLC. |
| ENG-104 | Settings → Engines MUST list discovered and manual engines with: name (editable), endpoint, origin, enabled toggle, *Test connection*, *Remove* (manual only), and *Hide* (discovered). |
| ENG-105 | *Add engine* dialog: kind = Unix socket / Named pipe / TCP / TCP+TLS (CA, cert, key file pickers) / WSL distro (dropdown of detected distros, mode Bridge or TCP port) / WSLC (session name; transport *Auto*/*COM*/*CLI*). *Test* before *Save*. |
| ENG-106 | A WSL distro that is stopped MUST show a *Start & connect* action. It MUST NOT be booted silently by background pings. |
| ENG-107 | A failed connection MUST show the error with an actionable hint (permission, daemon not running, WSL version too old, socat missing, and so on). |
| ENG-108 | The status bar MUST show the engine version, API version, and OS/arch of the active engine. |
| ENG-109 | WSLC sessions: the default session is listed. Further sessions are listed when "Show all WSLC sessions" is on. |
| ENG-110 | For WSLC engines, the hover tooltip and Diagnostics MUST show the active transport (COM or CLI) and the WSL version. When the app has fallen back to CLI, a small info chip explains why (for example, "WSL 2.10 not yet verified — using CLI"). |
| ENG-111 | **First run / no engine.** When no engine is discovered or connected, the window shows a full-page welcome with per-OS guidance (Linux: install Docker Engine, add the user to the `docker` group. macOS: Docker Desktop / Colima / OrbStack. Windows: Docker Desktop, Docker in a WSL distro, or `wsl --update` for WSLC), plus *Rescan* and *Add engine…*. It's keyboard-operable (KBD-001). |
| ENG-112 | **Unsupported engines** (ssh contexts, API < 1.41) are listed greyed out with the reason and are never contacted (ENG-010). |

## UI

- Switcher popover rows: `[icon] Ubuntu-22.04   Docker 27.3 · linux/amd64   ●`.
- Group headers: *Local*, *WSL distros*, *WSL containers*, *Remote*.
- Footer: `Rescan` · `Manage engines…`.

## Contract usage

`HubHandle::hub_events()`, `HubHandle::set_active(id)`, `HubHandle::add_engine(cfg)`,
`HubHandle::test_engine(cfg) -> HubCall<EngineInfo>`, `HubHandle::rescan()`, `Engine::info()`.

## Acceptance

- [ ] On Windows with Docker Desktop, Ubuntu+dockerd, and WSLC, all three appear and each can be browsed.
- [ ] Killing dockerd while connected shows *Reconnecting…* and recovers automatically when it returns.
- [ ] Switching engines while logs are streaming cancels the stream. No stale rows from the previous engine appear.
