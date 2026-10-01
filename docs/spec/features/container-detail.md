# Feature: Container detail

- **Status:** planned
- **Requirement prefix:** CDT
- **Plan:** [docs/plan/features/container-detail.md](../../plan/features/container-detail.md) (to be written)

## Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ‹ Containers › myshop › web-1                                                │
│ ● web-1   nginx:1.27 ↗   a1b2c3d4e5f6 ⧉   8080:80 ↗     [■ Stop][⟳][⋮][🗑] │
│ Running · Up 2 hours · healthy                                               │
├──────────────────────────────────────────────────────────────────────────────┤
│ [Overview] [Logs] [Terminal] [Stats] [Mounts] [Network] [Inspect]            │
├──────────────────────────────────────────────────────────────────────────────┤
│ tab content                                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Requirements

| ID | Requirement |
|---|---|
| CDT-001 | The header MUST show: status dot and name, image (link to image detail), short id with copy, published ports as links, status text, health chip, and the action buttons from CON-020. |
| CDT-002 | Tabs: Overview, Logs, Terminal, Stats, Mounts, Network, Inspect. The tab is part of the route, so deep links and back/forward work. |
| CDT-003 | Detail data (`inspect_container`) refreshes on any engine event for this container id. |
| CDT-010 | **Overview** (`DescriptionList` sections in `GroupBox`es): *General* (id, name, image + image id, created, started, finished, restart count, restart policy, platform, PID, exit code). *Command* (entrypoint, cmd, working dir, user, tty). *Compose* (project, service, number, working dir, config files), shown if grouped. *Environment* (table of key/value, with values masked if the key matches `pass|secret|token|key` and a reveal toggle per row). *Labels* (table). *Resources* (CPU limit, memory limit, pids limit). *Health* (last 5 results with exit code and output). |
| CDT-020 | **Mounts**: table with Type (bind/volume/tmpfs), Source (a volume name links to volume detail), Destination, Mode, RW. |
| CDT-030 | **Network**: (a) *Port bindings* table: Container port/proto, Host IP, Host port (link). (b) *Networks* table: network name (link), IPv4, IPv6, gateway, MAC, aliases. (c) Hostname, DNS, and network mode. |
| CDT-040 | **Inspect**: read-only, syntax-highlighted JSON of the raw inspect output, with *Copy* and a search box (`Ctrl/Cmd+F` within the editor). Env values are **not** masked here, and a warning label says so. |
| CDT-050 | **Logs**: see [container-logs.md](container-logs.md). |
| CDT-060 | **Terminal**: see [container-terminal.md](container-terminal.md). |
| CDT-070 | **Stats**: see [container-stats.md](container-stats.md). |
| CDT-080 | When the container is removed while open: banner (SHL), all tabs read-only, and actions disabled. |
| CDT-081 | Tabs keep their state while switching between tabs on the same container: log scroll position, terminal session, stats buffer. Leaving the container detail tears them down, except terminals (see TRM-008). |
| CDT-082 | Keyboard: tab switching, header actions, breadcrumb, links, and copying values per [keyboard.md](keyboard.md) KBD-040…044. |
