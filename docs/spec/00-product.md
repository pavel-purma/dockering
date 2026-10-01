# 00 — Product

## Vision

**Dockering** is a fast, native, cross-platform desktop app for working with container
engines. It feels like Docker Desktop's core screens (Containers, Images, Volumes) without
the rest: no AI assistant, no extensions marketplace, no Scout, no Kubernetes, no Builds view,
no account sign-in.

It is a **client only**. It does not install, bundle, or manage a container engine. It
connects to engines that already exist:

- a local Docker Engine (Linux socket, macOS Docker Desktop/Colima/OrbStack socket, Windows named pipe),
- a Docker Engine installed **inside a WSL distro** (Windows),
- **WSL containers (WSLC)**, the container runtime built into WSL (Windows),
- a remote Docker Engine over TCP (+TLS).

## Target users

Developers who run containers locally, often several engines side by side (for example, Docker
Desktop and Docker inside an Ubuntu WSL distro), and want one lightweight UI for all of them.

## Platforms

| OS | Architectures | Status |
|---|---|---|
| Windows 10 22H2+ / 11 | x86_64, aarch64 | MUST |
| macOS 15+ | aarch64, x86_64 | MUST |
| Linux (Wayland + X11, Vulkan) | x86_64, aarch64 | MUST |

macOS 15+ and Vulkan on Linux are requirements inherited from GPUI Kit
(see [50-build-and-release.md](50-build-and-release.md)).

## Scope (v1)

| Area | In scope |
|---|---|
| Engines | Discover, add, edit, remove, and switch engines. Show connection status. Reconnect automatically. |
| Containers | List (flat or grouped by Compose project), search, filter, sort. Start, stop, restart, pause, unpause, kill, and delete, one at a time or in bulk. |
| Container detail | Overview, Logs, Terminal (exec), Stats (charts), Mounts, Network, Inspect (raw JSON) |
| Images | List, search, sort. Detail (overview, layers, used by, inspect). Pull with progress, delete, prune, run (simple dialog). |
| Volumes | List, search, sort, size. Detail (overview, used by, inspect). Create, delete, prune. |
| Networks | List and detail (read-only, plus delete and prune). Per-container network info in container detail. |
| Settings | Theme, engines, refresh and stats options, terminal font, default shell |

## Non-goals (v1)

- Managing engine installation or VM resources (CPU and RAM limits of Docker Desktop or WSL).
- Kubernetes, Swarm, Docker Scout or vulnerability scanning, extensions, AI features, Builds view, Docker Hub browsing, sign-in.
- Running Compose files (`docker compose up`). Grouping is **read-only**, based on labels. Group-level start, stop, and delete act on the member containers through the engine API.
- File browser inside containers or volumes. This MAY come post-v1.
- Windows containers (Linux containers only).
- Apple's native `container` runtime on macOS. This is **planned post-v1**: it was researched and the architecture keeps it possible (spec 20 §7, ADR-0005). In v1, macOS uses Docker-compatible engines (Docker Desktop, Colima, OrbStack, Rancher Desktop, Podman).

## Glossary

| Term | Meaning |
|---|---|
| **Engine** | One configured connection to a container engine (a Docker daemon, a WSL distro's dockerd, or a WSLC session). |
| **Engine kind** | `docker` (socket, pipe, or TCP), `wsl-distro`, `wslc`; `apple-container` is reserved for a future release |
| **Backend** | A crate implementing the `Engine` contract for one runtime. It may contain several **transports**, for example WSLC's COM and CLI. |
| **Active engine** | The engine whose resources are currently shown. Exactly one at a time per window. |
| **Group** | A set of containers that share a group key. By default this is the Compose project label `com.docker.compose.project`. |
| **Capability** | A feature flag an engine advertises (for example, `exec_resize`). The UI hides or disables what the engine can't do. |
| **Hub** | `EngineHub`, the background service that owns the async runtime and all engine connections. |
| **Store** | A GPUI entity that caches an engine's resource lists for the UI. |
