# Changelog

All notable changes to Dockering are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Spec changes are tracked separately in [`docs/spec/CHANGELOG.md`](docs/spec/CHANGELOG.md).
Both changelogs are written only at release time, by the `/release` skill in the release PR
(REL-011, REL-019): feature pull requests don't edit them. Edit the wording in the release PR
before merging it.

## [0.3.0] - 2026-10-11

Downloads are unsigned, as in 0.2.0. Automatic updates and winget remain deferred.

### Added

- Containers can be grouped in a new *Combined* mode, now the default: containers of a Compose
  project are grouped by project, and the others by a label. Settings has a *Group label key*
  (default `app.kubernetes.io/part-of`) that *Combined* and *Label* grouping both use; an
  existing label grouping keeps working and carries over its key.
- Keyboard: `Enter` in the search field of Containers, Images, Volumes and Networks applies the
  query at once and moves to the first matching row.
- Keyboard: on a detail page, `Down` on the tab bar moves into the tab's content, `PgUp` and
  `PgDn` scroll it by a page, `Esc` or `Up` on the first row returns to the tab bar, and
  `Backspace` goes back.
- WSLC: when the default WSL containers session doesn't exist, the engine shows a calm *stopped*
  page with a **Start WSL containers** button, and a *Start engine* command in the command
  palette, instead of a red connection failure that kept retrying.

### Changed

- `PgUp` and `PgDn` in the container logs scroll by one viewport page instead of a fixed distance.

## [0.2.0] - 2026-10-06

Downloads are unsigned, as in 0.1.0. Automatic updates and winget remain deferred.

This build replaces a first 0.2.0 build that was withdrawn: it crashed at launch after an upgrade
from 0.1.0. If you installed that build, install this one over it.

### Added

- The status bar names the active engine (*Docker*, *WSL containers*) instead of a generic
  "Engine", with its version, API, platform, and transport in a clearer layout.

### Changed

- WSL containers (WSLC) in Auto mode now pick the native COM API or the `wslc.exe` command per
  operation. COM is used only on WSL 3.0.1.0, the version it was verified against; other versions
  use `wslc.exe`. In Auto mode, Run goes through `wslc.exe`.
- Settings → Engines and Diagnostics show a transport note only when it matters, in a few words.
  The route note chip is gone from the status bar.

### Fixed

- Windows: no empty console window opens next to the app on launch. `dockering.exe` is now a GUI
  program, so PowerShell does not wait for it; read the version with `dockering --version | Out-String`.
- WSLC: running an image works on an Auto connection. It failed with "Run via COM is not verified
  for this WSL version".
- WSLC: container details list published ports (IPv4/IPv6, TCP/UDP) on either route, and the
  Inspect tab shows the engine's original JSON.
- WSLC: an action that may already have reached the engine is not repeated automatically after a
  connection loss. When the app can't tell whether it took effect, it says so and asks you to
  refresh before retrying.
- WSLC: terminal startup and cleanup are hardened, canceled queued calls no longer run, and engine
  discovery lists extra sessions only when it can prove they are yours.
- If a run fails because the engine is unreachable, the Run dialog now shows the suggested fix.

## [0.1.0] - 2026-10-04

First release of Dockering. Downloads are unsigned; macOS builds are not notarized.
Automatic updates and winget are deferred until signing is configured.

### Added

- Native Rust/GPUI desktop client for Docker, Docker inside WSL distros, and WSL containers.
- Multiple engine connections, engine switching, explicit rescans, and a pinned default engine.
- Container list with Compose grouping, search, sorting, lifecycle actions, and bulk actions.
- Container details, logs, interactive terminals, live stats, mounts, networks, and inspect views.
- Images, volumes, and networks, with supported create, pull, run, delete, and prune operations.
- Light/dark themes, configurable settings, command palette, and keyboard navigation.
- Windows x64/ARM64 installers and portable ZIPs; macOS Intel/Apple silicon DMGs;
  Linux x64/ARM64 AppImage, Debian package, and tar archive.
- Complete six-platform release assembly with SHA256SUMS, build provenance, update metadata,
  and release notes. GitHub releases remain drafts until explicitly published.
- Explicit unsigned mode that disables signing, notarization, automatic updates, and winget.
- MIT license and bundled third-party notices, with dependency license/advisory checks in CI.
- Cross-platform tests, six-target builds, and Windows installation/uninstallation smoke tests.
- Optional release-plz version/changelog automation and future signing/updater/winget support.
