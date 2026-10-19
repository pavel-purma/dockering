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
| PowerShell 7 (`pwsh`) (REL-018) | `scripts/verify-release.ps1`, which the `/release` skill runs against every draft | `winget install Microsoft.PowerShell` · preinstalled on GitHub runners |
| `release-plz` (dormant; don't enable with the skill, REL-010) | Bot-driven release PRs, only if the skill is retired | `cargo install release-plz --locked` |
| Inno Setup **6.3+** (`ISCC.exe`) (REL-020) | `cargo xtask package` on Windows | CI checks and installs it if needed · `winget install JRSoftware.InnoSetup` |

Windows packaging uses Inno Setup for both architectures; cargo-packager is used for macOS
and Linux. Linux AppImage tooling is downloaded on first use. The release workflow installs
Inno Setup if the Windows runner does not already provide it. macOS CI/release packaging uses
`scripts/package-macos.sh`: it retries only the observed `hdiutil` busy-image eject error, at
most three package attempts, and detaches only a reported disk whose image path is verified
inside the current target distribution directory. Other failures retain their original exit status.

**Release signing** is optional for the explicitly authorized unsigned releases (REL-016).
Unsigned mode skips OS signing/notarization even when secrets exist. Planned independent
`UPDATER=none|all` controls the updater on all six targets (REL-083), not OS signing.
Updater-enabled unsigned binaries still have a signed manifest. Tag pushes default to unsigned while `PUBLIC_RELEASES` is not true; manual dispatches
expose an `unsigned` boolean defaulting to true. Public build-provenance attestations remain
independent of signing. The signed channel (`PUBLIC_RELEASES=true`) supports `WINDOWS_SIGNING` =
`signpath` | `azure` and, independently, `MACOS_SIGNING` = `apple` | `none` (REL-034); its secrets live
in the `release` environment, which only `main` and `v*` refs can use. Setup and operation:
[signing guide](../signing.md); the release steps: [runbook](../release.md) and the `/release` skill.
SignPath: `SIGNPATH_API_TOKEN` and the variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`; the artifact
configurations are in `packaging/signpath/`.
Azure: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`.
macOS: `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`.
Updater: `UPDATE_SIGNING_KEY` (minisign), reviewed current/next public keys in `crates/dk-update/src/keys.rs`, and `UPDATER` (`none` | `all`, default none; REL-083). A manual run has the input `updater` = `inherit` (default) | `none` | `all`, so an unsigned or signed rehearsal can build the updater before the variable is set. Missing keys fail closed. Planned, not current workflow behaviour.

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
| `gate` (`CI gate`) *(REL-081)* | ubuntu-24.04, `if: always()` | Succeeds only if `lint`, `test`, `integration-docker` and `build` all succeeded. The one `ci.yml` check the ruleset requires (REL-080). |
| `release` | on `v*` tags or manual dispatch; 6 targets | Verify independent signing/updater gates → package six targets → require all 12 distributions → manifest (signature required with updater=all OR signed OS mode; REL-083/084), notes, SHA256SUMS → public provenance → review artifact; tags create drafts only. Native installation claims require REL-087. Gate revision planned, not yet implemented. |
| `winget` *(planned)* | `release: published` (stable only) | Open a `microsoft/winget-pkgs` PR (REL-041) |
| `release-plz` *(dormant)* | push to `main` / `release/*` | Skips without the release-bot secrets. Not used while the `/release` skill cuts releases (REL-010) |
| `pr-title` | pull_request | The PR title is a conventional commit (it becomes the squash-commit message that `cargo xtask release-plan` classifies, REL-011). Required by the ruleset (REL-080). |
| Dependabot | weekly (Mondays), `.github/dependabot.yml` | Version-update PRs for `cargo` and `github-actions`, titled `chore(deps): …`. Minor/patch bumps grouped per ecosystem; majors separate. Security updates are on too. |
| `wslc-abi-watch` | weekly schedule, ubuntu-24.04 | `cargo xtask wslc-abi-check latest`. If the latest `microsoft/WSL` release changed `wslc.idl`, open an issue (20 §5.7). |
| `linux-smoke` *(REL-070…077, REL-079; release and schedule triggers not yet exercised)* | `release: published`, weekly, manual, PRs touching it; ubuntu-24.04 runner, four distribution containers | Installs the published Linux x86_64 packages into Ubuntu 24.04/26.04, Fedora and Arch containers, launches the app on Xvfb and headless sway, walks the pages, quits, keeps screenshots. See [distribution §8](features/distribution.md#8-install-and-launch-smoke-test-linux-and-macos-rel-070079). |
| `linux-update-e2e` *(REL-085/086; planned)* | weekly, manual, relevant PRs; Linux runner, `contents: read`, no secrets; not a required check | One seam-enabled old AppImage and one release-shaped new AppImage; scenarios S1 (FUSE and extract-and-run) to S4; screenshots and request log. [Distribution §9](features/distribution.md#9-cross-platform-update-gate-and-native-tests-rel-083087). |
| Windows updater acceptance *(REL-087; planned)* | `installer-smoke.ps1` update leg on Windows x64/ARM64 runners; spike W1 on a real machine | The installer started with the updater's argv replaces the files and relaunches; unsigned trust, share-locked start and UAC checks are manual. macOS has no installation (UPD-016). Linux runs cannot complete these gates. |
| `macos-smoke` *(REL-078, REL-079; green for a pull request, release and schedule triggers not yet exercised)* | `release: published`, weekly, manual (tag or draft run), PRs touching it; hosted macOS 15 arm64, macOS 15 Intel and macOS 26 arm64 runners | Installs the published `.dmg` of the runner's architecture, launches the app with `--demo` and on an upgrade profile, walks the pages through System Events, quits, checks Gatekeeper's behaviour on a quarantined copy, keeps screenshots. See [distribution §8](features/distribution.md#8-install-and-launch-smoke-test-linux-and-macos-rel-070079). |

Caching: `Swatinem/rust-cache`, saved from `main` only (also when a test fails) and restored by PRs. Dev/test builds carry no debuginfo (`CARGO_PROFILE_DEV_DEBUG=0`). The Windows `test` job builds on a trusted ReFS Dev Drive. Linux runners install the prerequisites above.

## Merge gate (REL-080…082)

*Implemented (2026-10-11), plan: [ci-merge-gate](../plan/features/ci-merge-gate.md). Until then the `Protect main` ruleset required a pull request and squash merges but no status check, so a red `lint` job did not stop #38 or #39 from merging.*

| ID | Requirement |
|---|---|
| REL-080 | **Required checks.** The `Protect main` ruleset MUST require the status checks `CI gate` (`ci.yml`) and `conventional commit title` (`pr-title.yml`), both from the GitHub Actions app, before a pull request can merge. It keeps the pull-request and squash-only rules, its only bypass is the repository Admin role in *pull request* mode (the maintainer's "Bypass rules and merge" button, live since 2026-10-11; nobody can push to `main` directly), and it does not require the branch to be up to date. No other workflow gates a pull request: `release`, `release-plz`, `winget`, `wsl-integration`, `wslc-abi-watch` and the Linux/macOS smoke tests are not required. |
| REL-081 | **One aggregate job.** `ci.yml` ends with a job `gate` (display name `CI gate`) that `needs` every other job in the file, runs with `if: always()`, and fails unless every needed job's result is `success`: a failed, cancelled or skipped job all fail it (GitHub counts a skipped required check as passing). The ruleset names the gate, not the matrix legs, so changing a matrix needs no ruleset change. A new `ci.yml` job MUST be added to `needs`. |
| REL-082 | **Always reported.** `ci.yml` and `pr-title.yml` MUST run on every pull request to `main`: no `paths` or `paths-ignore` filter on `pull_request`, and no job-level `if` in `ci.yml`. A required check that never reports blocks the pull request for good. |

Verification: `scripts/tests/test_ci_gate.py` (REL-081, REL-082, and that the names in REL-080 are real job names); the live ruleset is checked with `gh api repos/pavel-purma/dockering/rules/branches/main`, which lists both contexts. Live evidence, 2026-10-11: the rule was added after #41 merged (`CI gate` green on its run); canary PR #42, with a deliberate formatting error, failed `lint` and `CI gate`, showed `mergeStateStatus: BLOCKED`, and `gh pr merge` refused it ("the base branch policy prohibits the merge"); it was closed unmerged. The Admin bypass was not exercised.

Known gap: a pull request whose branch predates `ci.yml`'s `gate` job never reports `CI gate` and stays blocked until it is rebased or merged with `main`.

## Packaging (`cargo-packager`, config in `packaging/`)

| OS | Artifacts | Signing |
|---|---|---|
| Windows | `Dockering-Setup-<x64|arm64>.exe` (Inno) + `Dockering-<x64|arm64>.zip` | Optional Authenticode via SignPath or Azure. Explicit unsigned first release permitted (REL-016); signed channel requires verified signatures. |
| macOS | `.dmg` with `Dockering.app`, per architecture | Developer ID/notarization in signed mode; unsigned first release permitted with documented Gatekeeper prompts (REL-016). |
| Linux | `.AppImage`, `.deb`, `.tar.gz` (Flatpak post-v1) | — |

App id: `dev.dockering.Dockering`. Icons in `assets/app-icon/` (ico, icns, png 16…1024). The "Stacked D" icon is generated from SVG masters by `cargo xtask icons` (REL-050/051).

The macOS `.dmg` is built by cargo-packager's pinned `create-dmg` script, which now and then fails to eject its temporary disk image (`hdiutil: couldn't eject "diskN" - Resource busy`). `cargo xtask package` handles it: when cargo-packager fails and `/Volumes/Dockering` is still mounted, it force-detaches the volume and runs cargo-packager again, up to 3 runs in all. A failure that leaves no volume mounted (signing, notarisation, a bad config) is not repeated.

## Licensing (REL-001…)

- **REL-001** Project licence: **MIT**, in `LICENSE` (changed from MIT OR Apache-2.0 on 2026-10-03, before any public release). Dependencies keep their own licences; their notices are in `THIRD_PARTY_LICENSES.html` (REL-002).
- **REL-002** Third-party notices are generated by `cargo about` into `THIRD_PARTY_LICENSES.html`, bundled with every package, and shown in Settings → Diagnostics → Licences (SET-060). Assets need their own notices (Lucide icons: ISC). The vendored `wslc.idl` copies keep their MIT header and are listed.
- **REL-003** `cargo deny` allowlist: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CC0-1.0, OpenSSL (only if ring/aws-lc requires it). Any GPL/AGPL/LGPL dependency fails CI. Zed's `terminal` and `terminal_view` crates are GPL and must not be copied (reference only).

## Updates

Releases go out through GitHub Releases and OS package managers (winget; Homebrew cask and Flathub post-v1).

[distribution.md](features/distribution.md) UPD-001…017 and [proposed ADR-0007](../plan/adr/0007-cross-platform-updater.md): a planned shared opt-out updater on Windows, macOS and Linux (schedule, manual checks, signed-manifest trust, one notification per version, title-bar button, retained offer, Settings). `UPDATER=none|all` is independent of OS signing and a signed manifest stays mandatory; updates install without OS code signing. Only the install strategy differs: the Inno installer (Windows) and an in-place AppImage swap (Linux) install; macOS (in-app install deferred, UPD-016) and every other install show *Download*, never install-success. Gate and artifact checks: REL-083/084/087. Release flow: REL-010.

## Versioning

SemVer. `CHANGELOG.md` at the repo root follows the "Keep a Changelog" format. The version shows in Settings → Diagnostics and in `--version`. The `/release` skill computes the next version from the conventional commits (`cargo xtask release-plan`) and writes both changelogs in the release PR (REL-010, REL-011, REL-018, REL-019); feature PRs never edit them.
