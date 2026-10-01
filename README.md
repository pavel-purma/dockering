# Dockering

A fast, native desktop client for container engines: Docker, Docker inside WSL distros, and
WSL containers (WSLC). It runs on Windows, macOS, and Linux, and is written in Rust with
[GPUI Kit](https://gpui-kit.com/).

> Status: **planning**. See the [specification](docs/spec/README.md) and the [implementation plan](docs/plan/README.md).

## Features (v1)

- Containers: flat list or grouped by Compose project, with start, stop, restart, and delete, including bulk actions
- Container detail: overview, logs, interactive terminal, live CPU/memory/network/disk charts, mounts, network, and inspect
- Images (pull, run, delete, prune), volumes (create, delete, prune), and networks
- Multiple engines with one-click switching: local socket or pipe, remote TCP/TLS, WSL distros, and WSLC

## Contributing with agents

This repository uses a spec-driven agentic workflow. Start with [CLAUDE.md](CLAUDE.md).
Plan features with `/feature-planning <idea>`.
