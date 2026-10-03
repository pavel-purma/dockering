# 50 — Build & Release

## Toolchain

- Rust **stable**, pinned in `rust-toolchain.toml` (initially `1.98`). GPUI Kit needs ≥ 1.92. Components: `rustfmt` and `clippy`.
- Edition 2024. Workspace-wide `[lints]`: `unsafe_code = "deny"` except in `dk-wsl` (named-pipe ACLs) and `dk-engine-wslc/src/com/` (WSLC COM FFI), where each use is documented with `// SAFETY:`; also `clippy::dbg_macro`, `todo`, and `unwrap_used` (warn).
- `gpui-kit = "0.7"` is pinned exactly in `Cargo.lock`. GPUI Kit pins its own `gpui` (`gpui-pre =0.3.7`). Upgrades are deliberate PRs, guided by the `gpui-kit` changelog.

## Platform prerequisites (from GPUI Kit)

| OS | Requirements |
|---|---|
| Windows 10+/11 | MSVC toolchain (VS 2022 Build Tools, "Desktop development with C++", Windows SDK) and CMake on `PATH`. Rust `*-pc-windows-msvc` only. |
| macOS 15+ | Xcode Command Line Tools |
| Linux | `gcc g++ clang libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1 mesa-vulkan-drivers` (Ubuntu 24.04 names). Needs a Wayland or X11 session and a Vulkan driver. |

`scripts/bootstrap.{sh,ps1}` installs or checks these. On Windows, `bootstrap.ps1` locates Visual Studio / Build Tools with `vswhere` and verifies the **x64 CRT libs** (`msvcrt.lib`) and the Windows SDK. A VS install without the x64 CRT fails at link time (spike F-1). `scripts/dev.ps1` runs cargo inside `vcvars64.bat`. CMake ships with Build Tools; it doesn't need to be on `PATH` when you build through `dev.ps1`.

**Self-hosted runner.** GitHub-hosted Windows runners can't run WSL2 distros or WSLC, so the WSL/WSLC integration tests (M7/M8) run on a self-hosted Windows 11 runner (label `wsl`) with WSL ≥ 3.0, an Ubuntu distro with Docker Engine, and WSLC enabled.

**Release tooling** (CI installs these; locally you only need them for the matching task):

| Tool | Needed for | Install |
|---|---|---|
| `cargo-nextest` | `cargo nextest run` | `cargo install cargo-nextest --locked` |
| `cargo-deny` | REL-003 checks (`deny.toml`) | `cargo install cargo-deny --locked` |
| `cargo-about` | Regenerating `THIRD_PARTY_LICENSES.html` (REL-002; CI fails if it's stale) | `cargo install cargo-about --locked --features cli` |
| `cargo-packager` **0.11.8** (pinned; `packaging/packager.toml` is checked against its schema) | `cargo xtask package` / `dist` (macOS, Linux) | `cargo install cargo-packager --locked --version 0.11.8` |
| `release-plz` *(planned, REL-010)* | Release PRs and tags (CI); locally only for `release-plz update --dry-run` previews | `cargo install release-plz --locked` |
| Inno Setup **6.3+** (`ISCC.exe`) *(planned, REL-020)* | `cargo xtask package` on Windows | Preinstalled on GitHub Windows runners (⚠ verify, spike S-9) · `winget install JRSoftware.InnoSetup` |

cargo-packager downloads WiX 3 / NSIS (Windows) and linuxdeploy (AppImage) on first use. WiX 3 can't target ARM64, so `aarch64-pc-windows-msvc` ships an NSIS `.exe` instead of an `.msi`. *(Planned: Windows moves to one Inno Setup installer for both architectures and drops WiX/NSIS, per [distribution.md](features/distribution.md) REL-020 and ADR-0006.)*

**Release signing secrets** live only in the GitHub environment `release`. Steps are skipped when the secrets are absent, which gives an unsigned build. *(Planned, REL-031: the Windows provider is chosen by the repo variable `WINDOWS_SIGNING` = `signpath` | `azure` | `none`. SignPath adds `SIGNPATH_API_TOKEN`, the updater adds `UPDATE_SIGNING_KEY` (UPD-003), and winget adds `WINGET_TOKEN` (REL-041). An unsigned **stable** tag fails the release.)*
Windows: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`.
macOS: `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`.

## Cargo profiles

```toml
[profile.dev]          opt-level = 1            # GPUI is painfully slow at 0
[profile.dev.package."*"] opt-level = 3
[profile.release]      lto = "thin", codegen-units = 1, strip = "symbols", panic = "unwind"
```

## CI (GitHub Actions)

| Job | Runners / targets | Steps |
|---|---|---|
| `lint` | ubuntu-24.04 | `cargo fmt --check` · `cargo clippy --workspace --all-targets -D warnings` · blocking-call grep (NFR-001) over `crates/dockering/src` and `crates/dk-terminal/src/view` · `cargo deny check` (licences, advisories) |
| `test` | ubuntu-24.04, windows-2025, macos-15 | `cargo nextest run --workspace` (unit, contract with fixtures, gpui `TestAppContext` view tests) |
| `integration-docker` | ubuntu-24.04 (Docker preinstalled) | `cargo nextest run -p dk-engine-docker --features it` against the real dockerd |
| `build` | x86_64 + aarch64 for Linux (`ubuntu-24.04`, `ubuntu-24.04-arm`), Windows (`windows-2025`, `windows-11-arm`), macOS (`macos-15` arm64, `macos-15-intel`/cross) | `cargo build --release` + package (on `main`). On Windows, every push and PR also builds the Inno installer (x64, arm64) and runs silent install → `--version` → uninstall (REL-014) |
| `release` | on tag `v*` | Build all, sign, upload artifacts to a GitHub Release. *(Planned: verify → package/sign → update manifest → attest → draft; [distribution.md](features/distribution.md) REL-010…013.)* |
| `winget` *(planned)* | `release: published` (stable only) | Open a `microsoft/winget-pkgs` PR (REL-041) |
| `release-plz` *(planned)* | push to `main` / `release/*` | `release-pr`: open or update the release PR (version + changelog). `release`: after a release PR merges, create the `v*` tag, which starts `release` (REL-010/011) |
| `pr-title` *(planned)* | pull_request | The PR title is a conventional commit (it becomes the squash-commit message that release-plz reads) |
| Dependabot | weekly (Mondays), `.github/dependabot.yml` | Version-update PRs for `cargo` and `github-actions`, titled `chore(deps): …`. Minor/patch bumps grouped per ecosystem; majors separate. Security updates are on too. |
| `wslc-abi-watch` | weekly schedule, ubuntu-24.04 | `cargo xtask wslc-abi-check latest`. If the latest `microsoft/WSL` release changed `wslc.idl`, open an issue (20 §5.7). |

Caching: `Swatinem/rust-cache`, saved from `main` only (also when a test fails) and restored by PRs. Dev/test builds carry no debuginfo (`CARGO_PROFILE_DEV_DEBUG=0`). The Windows `test` job builds on a trusted ReFS Dev Drive. Linux runners install the prerequisites above.

## Packaging (`cargo-packager`, config in `packaging/`)

| OS | Artifacts | Signing |
|---|---|---|
| Windows | `.msi` (WiX) + portable `.zip`. *Planned:* `Dockering-Setup-<arch>.exe` (Inno) + `Dockering-<arch>.zip` (REL-012/020) | Authenticode (Azure Trusted Signing; *planned:* pluggable, SignPath Foundation for the public launch, REL-030/031). **Required for public releases**: unsigned binaries that spawn hidden `wsl.exe` processes and create named pipes trigger SmartScreen and Defender heuristics. Nightly/dev builds may be unsigned. |
| macOS | `.dmg` with `Dockering.app` (per-arch, universal2 later) | Developer ID + **notarisation required for public releases** (Gatekeeper on macOS 15+ blocks un-notarised apps for normal users) |
| Linux | `.AppImage`, `.deb`, `.tar.gz` (Flatpak post-v1) | — |

App id: `dev.dockering.Dockering`. Icons in `assets/app-icon/` (ico, icns, png 16…1024). *Planned:* a new "Stacked D" icon generated from SVG masters by `cargo xtask icons` (REL-050/051).

## Licensing (REL-001…)

- **REL-001** Project licence: **MIT**, in `LICENSE` (changed from MIT OR Apache-2.0 on 2026-10-03, before any public release). Dependencies keep their own licences; their notices are in `THIRD_PARTY_LICENSES.html` (REL-002).
- **REL-002** Third-party notices are generated by `cargo about` into `THIRD_PARTY_LICENSES.html`, bundled with every package, and shown in Settings → Diagnostics → Licences (SET-060). Assets need their own notices (Lucide icons: ISC). The vendored `wslc.idl` copies keep their MIT header and are listed.
- **REL-003** `cargo deny` allowlist: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CC0-1.0, OpenSSL (only if ring/aws-lc requires it). Any GPL/AGPL/LGPL dependency fails CI. Zed's `terminal` and `terminal_view` crates are GPL and must not be copied (reference only).

## Updates

Releases go out through GitHub Releases and OS package managers (winget; Homebrew cask and Flathub post-v1).

[distribution.md](features/distribution.md) UPD-001…012, ADR-0006: an opt-out updater, compiled into public release builds only (`--features updater` when `PUBLIC_RELEASES=true`). It reads a minisign-signed `dockering-update.json` from the latest GitHub Release. Windows installs get *Restart to update*; portable zip, macOS, and Linux get a notification only. The release flow standard is REL-010.

## Versioning

SemVer. `CHANGELOG.md` at the repo root follows the "Keep a Changelog" format. The version shows in Settings → Diagnostics and in `--version`. release-plz computes the next version from conventional commits and writes the changelog in a release PR (REL-010/011).
