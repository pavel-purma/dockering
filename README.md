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

## Build and run

Rust is pinned in `rust-toolchain.toml` (rustup installs it automatically). GPUI Kit also needs
native prerequisites. See [spec 50](docs/spec/50-build-and-release.md#platform-prerequisites-from-gpui-kit).

| OS | Prerequisites | Set up |
|---|---|---|
| Windows 10 22H2+ / 11 | VS 2022+ Build Tools ("Desktop development with C++", x64 CRT, Windows SDK) | `pwsh -NoProfile scripts/bootstrap.ps1` checks them; add `-Install` to install Build Tools with winget |
| macOS 15+ | Xcode Command Line Tools | `scripts/bootstrap.sh` (or `xcode-select --install`) |
| Linux (Ubuntu 24.04 names) | `gcc g++ clang libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1 mesa-vulkan-drivers`, plus a Wayland or X11 session with a Vulkan driver | `scripts/bootstrap.sh` (apt) |

On Windows, run cargo through `scripts/dev.ps1`. It loads the VS Build Tools environment
(`vcvars64.bat`) and passes its arguments to cargo. Without it, linking fails with
`msvcrt.lib` not found.

```sh
# Run the app
cargo run -p dockering                               # macOS / Linux
pwsh -NoProfile scripts/dev.ps1 run -p dockering     # Windows

# Demo mode with a built-in fake engine, no Docker needed (planned; not wired up yet)
cargo run -p dockering -- --demo

# Tests and lints (prefix with `pwsh -NoProfile scripts/dev.ps1` on Windows)
cargo nextest run --workspace                        # or: cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask check-blocking                           # NFR-001: no blocking calls on the UI thread
cargo deny check                                     # licences and advisories (cargo install cargo-deny)

# Packages (needs: cargo install cargo-packager --locked --version 0.11.8)
cargo xtask dist                                     # release build + package for the host
cargo xtask package --target <triple>                # package an existing release build
```

Packages go to `target/dist/<triple>/`: `.msi` (x64) or NSIS `.exe` (ARM64) plus a `.zip` on
Windows, `.dmg` on macOS, and `.deb`, `.AppImage`, and `.tar.gz` on Linux.

After changing dependencies, regenerate the third-party notices with
`cargo about generate about.hbs -o THIRD_PARTY_LICENSES.html` (`cargo install cargo-about --locked --features cli`).
CI fails if the file is stale.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Third-party notices: [THIRD_PARTY_LICENSES.html](THIRD_PARTY_LICENSES.html).

## Contributing with agents

This repository uses a spec-driven agentic workflow. Start with [CLAUDE.md](CLAUDE.md).
Plan features with `/feature-planning <idea>`.
