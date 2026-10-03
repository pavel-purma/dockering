# Feature: Distribution, releases & updates (Windows first)

- **Status:** in-progress (2026-10-03; implemented except the public-launch steps REL-041 automation, REL-060, and SignPath enrolment)
- **Requirement prefixes:** REL (release, packaging, signing, winget, branding; REL-001…003 licensing stay in [spec 50](../50-build-and-release.md#licensing-rel-001)) · UPD (in-app updates)
- **Plan:** [windows-distribution](../../plan/features/windows-distribution.md) · **ADR:** [0006](../../plan/adr/0006-windows-installer-and-updates.md)

Dockering ships through **GitHub Releases** (the only storage for binaries), a branded **Inno Setup**
installer on Windows, **winget**, and an opt-out **in-app updater** that reads a signed manifest
from the latest GitHub Release. Windows is the first platform to get the full flow. The release
flow, asset names, icon, and update manifest are cross-platform from day one, so macOS and Linux
can reuse them later (macOS/Linux get *notify-only* updates now).

The repository is **private** until the public launch (§6). Everything that depends on anonymous
access to release assets (winget, the updater's default source, shields.io badges, SignPath) is
gated on it.

## 1. Release flow (REL-010…015)

| ID | Requirement |
|---|---|
| REL-010 | **Release flow (release-plz).** Trunk-based on `main`, squash merges, and conventional-commit PR titles (enforced by a PR-title check). On every push to `main`, the `release-plz.yml` workflow opens or updates **one release PR** `chore(release): vX.Y.Z`. That PR bumps `[workspace.package] version` and `Cargo.lock` and adds the generated `CHANGELOG.md` section (REL-011). The steps are: (1) the maintainer reviews the release PR, edits the changelog wording if needed, runs the [release checklist](../../plan/release-checklist.md), and merges it; (2) release-plz then creates the tag `vX.Y.Z` on the merge commit with the release-bot GitHub App token, so the tag push starts `release.yml`; (3) `release.yml` builds, signs, and uploads every asset to a **draft** GitHub Release, with notes from that `CHANGELOG.md` section; (4) the maintainer smoke-tests the draft assets and clicks *Publish*. Publishing is the only step that makes a version visible to users, the updater (UPD-001), and winget (REL-041). release-plz never creates the GitHub Release itself. **Pre-releases** (`-alpha.N`, `-beta.N`, `-rc.N`): the maintainer sets the version in the release PR (`release-plz set-version` ⚠ verify, or a manual edit) before merging. They become GitHub *pre-releases* and are never "latest". **Hotfixes:** fix on `main` and release a patch. If `main` has unreleasable work, branch `release/X.Y` from the tag and cherry-pick; release-plz also runs on `release/*`. **Manual fallback:** the maintainer can still push a `v*` tag by hand (tag-ruleset bypass); `release.yml` checks it the same way. |
| REL-011 | **Versioning and changelog are computed from commits** by release-plz (`release-plz.toml`, git-only mode: the last version comes from `v*` tags, and nothing is published to crates.io). All workspace crates share the workspace version and form one `version_group`, so a `feat:` in any crate bumps the app. Only `dockering` gets the changelog section and the `vX.Y.Z` tag; `xtask` is never released. Bumps follow conventional commits: breaking change → minor while `0.x` (major from `1.0`); `feat` → minor; `fix`/`perf`/other → patch. `CHANGELOG.md` keeps the Keep a Changelog shape: `feat` → *Added*, `fix` → *Fixed*, `perf`/`refactor` → *Changed*; `docs`, `test`, `chore`, `ci`, and `style` commits are left out. Manual edits made in the release PR right before merging are kept. `release.yml` fails if the tag ≠ workspace version or if `CHANGELOG.md` has no section for the version. |
| REL-012 | **Stable asset names.** Every release uploads version-less names, so `https://github.com/pavel-purma/dockering/releases/latest/download/<name>` is a permanent download link. Windows: `Dockering-Setup-x64.exe`, `Dockering-Setup-arm64.exe`, `Dockering-x64.zip`, `Dockering-arm64.zip`. macOS: `Dockering-aarch64.dmg`, `Dockering-x86_64.dmg`. Linux: `Dockering-x86_64.AppImage`, `Dockering-aarch64.AppImage`, `Dockering-x86_64.deb`, `Dockering-aarch64.deb`, `Dockering-x86_64.tar.gz`, `Dockering-aarch64.tar.gz`. Plus `SHA256SUMS` (all assets), `dockering-update.json`, and `dockering-update.json.minisig` (UPD-002/003). |
| REL-013 | **Supply chain.** Signing secrets are only exposed to the steps that use them (job env holds only "is configured" booleans; the update key reaches only the prebuilt `xtask sign-manifest`). Once `PUBLIC_RELEASES=true`, every asset gets a GitHub build-provenance attestation (`actions/attest-build-provenance`; check with `gh attestation verify`); private repos on GitHub Free can't create them. Third-party actions in `release.yml` and `winget.yml` are pinned to a commit SHA. Signing jobs run in the GitHub environment `release`, which needs maintainer approval once the repo is public. Workflows use least-privilege `permissions`. A tag ruleset limits creating `v*` tags to the release-bot GitHub App (release-plz) and the maintainer (bypass). The App's private key is a repository secret (environments don't work in private repos on GitHub Free). Its permissions are only *Contents* and *Pull requests* read/write. GitHub *immutable releases* are enabled at the public launch. |
| REL-014 | **CI builds the installer.** On pushes to `main`, CI builds the unsigned Windows installer for x64 and arm64 and uploads it as a workflow artifact. An `installer-smoke` job on `windows-2025` does the following: silent per-user install → `dockering.exe --version` from the install dir prints the workspace version → silent uninstall → the install dir and the uninstall registry key are gone. The same runs for `/ALLUSERS`. |
| REL-015 | **Project home page.** `README.md` starts with the app icon, name, and one-line tagline. Then come badges, a screenshot, *Download* links per OS/arch (REL-012 stable links), install instructions (winget, installer, portable zip, build from source), the feature list, a *Privacy & network* section (what the updater contacts, how to turn it off; UPD-010), and links to the spec and contributing. Badges: CI (`ci.yml`) and Release (`release.yml`) workflow status now. Latest version, downloads, license, and winget version (shields.io) are added at the public launch, because shields.io can't read private repos. Repo *About*: description, topics, and the social-preview image (1280×640). Texts in [plan §A4](../../plan/features/windows-distribution.md#a4-home-page-copy). |

## 2. Windows installer (REL-020…027)

| ID | Requirement |
|---|---|
| REL-020 | **Inno Setup 6** installer per architecture (x64, arm64), built from `packaging/windows/dockering.iss` by `cargo xtask package` (format `inno`). It replaces the WiX `.msi` (x64) and NSIS `.exe` (arm64) on Windows. cargo-packager stays for macOS and Linux. |
| REL-021 | **Install scope.** The default is per user without admin rights (`PrivilegesRequired=lowest`), into `%LocalAppData%\Programs\Dockering`. All-users install (`/ALLUSERS`, or the wizard's scope dialog) goes into `%ProgramFiles%\Dockering`. The `AppId` GUID is fixed forever. Installing a newer version upgrades in place; re-running the same version repairs. |
| REL-022 | **Wizard.** `WizardStyle=modern` with branded images generated from the app icon (REL-050). Minimal pages: scope (only when interactive), optional desktop shortcut (off by default), install progress, finish with *Launch Dockering* (on). No licence-acceptance page (permissive licences); the licence files are installed. The uninstaller asks whether to remove settings and logs (default: keep). Silent uninstall keeps them. It removes the data of the user running the uninstaller only; for an all-users install run by an admin, that is the admin's profile. |
| REL-023 | **Shell integration.** Start Menu shortcut *Dockering* with AppUserModelID `dev.dockering.Dockering`. The app sets the same AUMID at startup, so taskbar pinning and grouping match. An *Installed apps* entry with icon, publisher, version, and help/update URLs (GitHub). Installs `LICENSE-MIT`, `LICENSE-APACHE`, and `THIRD_PARTY_LICENSES.html` (REL-002). |
| REL-024 | **Running app.** The app holds a named mutex `dev.dockering.Dockering` for its lifetime, next to the SHL-022 single-instance pipe. The installer uses it as `AppMutex` and closes a running Dockering via the Restart Manager (`CloseApplications=force`). Interactive installs ask first. Silent installs close it. |
| REL-025 | **Silent mode** (winget, updater, CI): `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-` exits 0 without UI. Silent uninstall leaves no files in the install dir and no *Installed apps* entry. |
| REL-026 | **Portable zip** (`Dockering-<arch>.zip`): `dockering.exe` plus licence files. It runs without installation and is detected as *portable* (UPD-006). |
| REL-027 | **Executable resources.** `dockering.exe` embeds the app icon as icon resource **ID 1** (GPUI's Windows backend loads resource 1 as the window and taskbar icon) and a `VERSIONINFO`: `ProductName`/`FileDescription` "Dockering", `CompanyName`, `LegalCopyright`, `OriginalFilename`, and `FileVersion`/`ProductVersion` from the workspace version. |

## 3. Code signing (REL-030…032)

| ID | Requirement |
|---|---|
| REL-030 | Every Windows executable in a **stable** public release (`dockering.exe`, the setup `.exe`, and the uninstaller that Inno writes) is Authenticode-signed with SHA-256 and an RFC 3161 timestamp. Signing runs only in CI, in the `release` environment. Keys and tokens never touch the repo or logs (NFR-020). |
| REL-031 | **Pluggable provider.** `release.yml` picks the provider from the repo variable `WINDOWS_SIGNING` = `signpath` (SignPath Foundation, the default target for the public launch), `azure` (Azure Artifact Signing; existing wiring, needs an eligible organisation), or `none`. With `none`, a stable tag fails the publish job once `PUBLIC_RELEASES=true`; before that (private phase) every release is an unsigned internal draft. Signing has two stages: binaries before packaging, then the installer. The order of steps is in [plan §A2](../../plan/features/windows-distribution.md#a2-windows-code-signing). |
| REL-032 | CI checks every signed file (`Get-AuthenticodeSignature` = `Valid`, one signer across files, and that signer = repo variable `WINDOWS_SIGNER_SUBJECT` when set) and fails the job on any mismatch. A stable public release also fails without `UPDATE_SIGNING_KEY`. |

## 4. winget (REL-040…042)

| ID | Requirement |
|---|---|
| REL-040 | winget package **`PavelPurma.Dockering`** (⚠ confirm before the first submission) in `microsoft/winget-pkgs`: `InstallerType: inno`, x64 + arm64, `Scope: user` (default) and `machine`, `UpgradeBehavior: install`, `ReleaseNotesUrl` = the GitHub release, `Moniker: dockering`, `License: MIT OR Apache-2.0`. The template lives in `packaging/winget/`. |
| REL-041 | The first version is submitted **manually** (`komac new` or `wingetcreate new`) after the first public, signed release. Every later **stable** release is submitted by `.github/workflows/winget.yml` on `release: published` (pre-releases are skipped) through `vedantmgoyal9/winget-releaser` (classic PAT secret `WINGET_TOKEN`, scope `public_repo`; a fork of `winget-pkgs` under the maintainer's account). The workflow is gated by the repo variable `WINGET_ENABLED == 'true'`. |
| REL-042 | The self-updater (UPD-007) updates the same *Installed apps* entry that winget reads, so `winget upgrade` shows the right version after a self-update. The manifest does **not** set `RequireExplicitUpgrade`. |

## 5. App icon & branding (REL-050…051)

| ID | Requirement |
|---|---|
| REL-050 | **Icon "Stacked D".** A rounded-square plate with a violet gradient (`#8E7AEB` → `#5840B5`, matching the SHL-009 accent `#6E56CF`) and a white **D** sliced into three stacked slabs: containers and image layers. Masters in `assets/app-icon/src/`: `icon.svg` (Windows/Linux, near full-bleed), `icon-macos.svg` (Apple 824/1024 grid with margin), and `icon-small.svg` (solid D for 16–24 px). `cargo xtask icons` (Rust, `resvg`) generates PNGs 16…1024, a multi-size `icon.ico` (16/20/24/32/40/48/64/256; ≤ 24 px from `icon-small.svg`), `icon.icns`, the Inno wizard bitmaps (100/125/150/175/200 % scale), `assets/brand/logo.svg`/`.png` for the README, and `assets/brand/social-preview.png`. CI fails if the generated files are stale. Concept renders: [plan folder](../../plan/features/windows-distribution/). |
| REL-051 | The icon is used everywhere: exe resource (REL-027), installer and uninstaller, Start Menu, *Installed apps*, window and taskbar, macOS `.app`/`.dmg`, Linux hicolor icons plus the `.desktop` file (SHL-023), README, and social preview. |

## 6. Public launch gate (REL-060)

| ID | Requirement |
|---|---|
| REL-060 | Before the repo goes public: secret scan of the full history (gitleaks), no private paths or tokens in fixtures, branch protection on `main` (squash merges, PR-title check), tag ruleset (REL-013), release-bot App permissions reviewed, `release` environment reviewers, Dependabot and secret-scanning alerts on, About/topics/social preview set (REL-015). Then set the repo variables `PUBLIC_RELEASES=true` (turns the updater on in release builds, UPD-005), `WINDOWS_SIGNING`, and `WINGET_ENABLED`, and do the first manual winget submission (REL-041). Until then, releases are unsigned internal **draft** releases in the private repo (any version, pre-release or not), never published. |

## 7. In-app updates (UPD-001…012)

| ID | Requirement |
|---|---|
| UPD-001 | **Source = GitHub Releases only.** The app fetches `https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json` and its `.minisig`. Every asset URL in the manifest must be `https://github.com/pavel-purma/dockering/releases/download/v<that version>/<plain file name>`; anything else is rejected. GitHub serves *latest* = the newest published non-pre-release, so drafts and pre-releases are never offered. No REST API calls are made, so there is no rate limit. A *Preview* channel (pre-releases) MAY be added later through the Releases API. |
| UPD-002 | **Manifest** (JSON, `schema: 1`): `version`, `pub_date`, `notes_url`, and `platforms` keyed `<os>-<arch>` (`windows-x86_64`, `windows-aarch64`, `macos-aarch64`, …) → `{ kind: inno|zip|dmg|appimage|deb|tar.gz, url, sha256, size }`. `release.yml` generates it from the built assets with `cargo xtask update-manifest`. Unknown fields are ignored; an unknown `schema` means "no update". |
| UPD-003 | **Integrity.** The manifest must carry a valid **minisign** (Ed25519) signature from a public key compiled into the app (current + next). The private key is the secret `UPDATE_SIGNING_KEY` in the `release` environment. Any manifest without a valid signature is rejected. A downloaded file must match `size` and `sha256` before use. On Windows, when the running `dockering.exe` is Authenticode-signed, the installer must also have a valid signature whose signer (full subject DN **and** issuer DN) matches the running exe's (`WinVerifyTrust`); a running exe whose own signature doesn't verify fails closed. Otherwise the installer is deleted and never run. Size, SHA-256 and the signature are checked again right before *Restart to update* starts it. |
| UPD-004 | **Schedule.** When enabled: first check 30 s after startup, then every 24 h (± 1 h jitter) while the app runs. *Check for updates* runs one now. Only a strictly greater SemVer is offered (no downgrades; a pre-release is never offered to a stable build). Errors on automatic checks are logged and retried next cycle. Only a manual check shows them. |
| UPD-005 | **Opt-out & build gate.** Updates are compiled only with the `dockering` Cargo feature `updater` (off by default). `release.yml` enables it when `vars.PUBLIC_RELEASES == 'true'`. Dev builds, distro builds, and private-phase releases make **no** update requests. At runtime, checks are off when Settings → Updates → *Check for updates automatically* is off (default on), `DOCKERING_DISABLE_UPDATES=1` is set, on Windows the policy value `HKLM` or `HKCU\Software\Policies\Dockering\DisableUpdates` = 1 is set (it also hides *Check now*), or in `--demo` mode. Off means zero *automatic* network requests. With only the setting off, *Check now* still runs one check because the user explicitly asked for it; under policy, the env var, demo mode, or a build without the feature, *Check now* is disabled. |
| UPD-006 | **Install kind** decides the mode. *Installed per user* (Inno, writable install dir) → download in the background, then *Restart to update*. *Installed for all users* → the same, but applying needs a UAC prompt and the label says so. *Portable zip*, macOS, and Linux → **notify only**: "Dockering X.Y.Z is available" with *Download* (opens the release page); nothing is downloaded. |
| UPD-007 | **Apply (Windows).** *Restart to update* makes the hub start the verified installer detached, with argv `/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /UPDATE /RELAUNCH` plus `/CURRENTUSER` or `/ALLUSERS` (no shell, NFR-022). Per-user: a detached child process. All users: `ShellExecuteExW` with the `runas` verb, so Windows asks for UAC *before* the app quits (Inno doesn't elevate itself for `/ALLUSERS` when `PrivilegesRequired=lowest`; checked 2026-10-03). If the user declines, the app shows *Couldn't start the update* and keeps running. The app then saves state and quits normally. The installer waits for the app to exit (REL-024), replaces the files behind a small progress window, and relaunches Dockering (`/RELAUNCH`; all-users installs relaunch through Explorer so Dockering never runs elevated). A downloaded but unapplied update stays offered across restarts (`state.json` `pending_version`; the file is kept and reused once a fresh signed-manifest check confirms it) and stays offered when automatic checks are turned off. Every check fetches the manifest again, so a newer release replaces a pending one. It is never applied without the user's action. Zed-style staged install with a post-exit swap helper MAY come later (ADR-0006). |
| UPD-008 | **UI.** A status-bar item at the right edge shows: *Downloading update… n%* · **Restart to update (X.Y.Z)** (accent button) · *Dockering X.Y.Z available* (notify-only) · *Checking…* and the error only during a manual check. When an update becomes ready, one notification per version appears with *Restart now*; clicking the notification opens the release notes (GPUI Kit notifications take one action button). After an update, the first launch shows "Updated to X.Y.Z" with *What's new* (`notes_url`). All URLs open in the browser through GPUI `open_url`. |
| UPD-009 | **Settings → Updates** (SET-090), between *Stats* and *Diagnostics*: *Check for updates automatically* (switch), current version, last checked time and result, *Check now* (inline result), *View release notes*. When a policy disables updates, the section says so, the switch is disabled, and *Check now* is hidden. In builds without the `updater` feature (and with the env var or in demo mode) the section shows *Updates aren't available in this build* and *Check now* is disabled. |
| UPD-010 | **Network & privacy.** HTTPS only, to `github.com` and its release-asset CDN hosts (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`); redirects anywhere else, or more than 5, fail. OS root certificates, so corporate TLS inspection works. `HTTPS_PROXY`/`NO_PROXY` are honoured. `User-Agent: Dockering/<version> (<os>; <arch>)`. No cookies, ids, or telemetry. NFR-023 is amended to allow exactly this. |
| UPD-011 | **Threading.** Fetching, verifying, and downloading run on the hub runtime in crate `dk-update` (no GPUI). The UI uses `HubHandle::update_status() -> HubStream<UpdateStatus>`, `check_for_updates() -> HubCall<UpdateCheck>`, and `apply_update() -> HubCall<()>`. The `UpdateStore` entity owns their tasks (NFR-004) and ignores results of superseded checks (NFR-005). |
| UPD-012 | **Files.** Downloads go to `<data-local>/updates/<version>/` (`%LocalAppData%`, never the roaming profile). Partial or stale downloads are removed at startup. State (`last_check`, `last_result`, `last_run_version`, `notified_version`, `pending_version`) lives in `state.json`. The *Check automatically* switch lives in `config.toml` `[updates] check`. |

## UI

```
StatusBar:  ● Connected · Docker 29.8.1 · API 1.53 · linux/amd64          [⟳ Restart to update (0.3.0)]
                                                                            ^ accent button, focusable (F6 → status bar)
```

Keyboard (KBD-076): *Check for Updates*, *Restart to Update*, and *View Release Notes* are command-palette
entries without default chords. On macOS, *Check for Updates…* is also in the app menu (SHL-020).

## Verification

| ID | Tests |
|---|---|
| REL-011 | release-plz 0.3.169 dry run (plan §9, S-9); `release.yml` `verify` job |
| REL-012 | `xtask` `rel_012_stable_asset_names_per_target`, `rel_012_versions_validated` |
| REL-014, 020…026 | `scripts/installer-smoke.ps1` (CI `installer` job, user + all users; release x64); local update round trip (plan §9, S-9) |
| REL-027 | `installer-smoke.ps1` VERSIONINFO check |
| REL-030…032 | `release.yml` "Verify Windows signatures" + the stable-unsigned guard |
| REL-050 | `xtask` `rel_050_ico_header_lists_all_sizes`; CI `cargo xtask icons --check` |
| UPD-001 | `upd_001_asset_url_pinned_to_repo_and_version`, `upd_010_only_github_hosts` |
| UPD-002 | `upd_002_manifest_roundtrip_ignores_unknown`, `upd_002_unknown_schema_is_no_update`, `upd_002_accepts_xtask_manifest`, `upd_002_parse_never_panics`, xtask `upd_002_manifest_from_assets` |
| UPD-003 | `upd_003_rejects_bad_signature`, `upd_003_accepts_next_key`, `upd_003_no_keys_fails_closed`, `upd_003_rejects_hash_mismatch`, `upd_003_rejects_size_mismatch`, `upd_003_stream_hash_mismatch_leaves_no_file`, `upd_003_untrusted_installer_rejected`, `authenticode_*`, xtask `upd_003_signed_manifest_verifies` |
| UPD-004 | `upd_004_first_check_after_30s_then_daily`, `upd_004_no_downgrade_or_prerelease`, `upd_004_same_version_is_up_to_date`, `upd_004_older_versions` |
| UPD-005 | `upd_005_disabled_makes_no_requests`, `upd_005_not_compiled_or_demo_is_disabled`, `upd_005_policy_read_does_not_panic` |
| UPD-006 | `upd_006_install_kind_from_path` |
| UPD-007 | `upd_007_installer_args`, `upd_007_manual_check_downloads_and_is_ready`; manual round trip 0.1.0 → 0.2.0 with relaunch (S-9) |
| UPD-008 | `upd_008_status_bar_renders_each_state`, `upd_008_ready_notification_once_per_version`, `upd_008_previous_version_and_notified_once` |
| UPD-009 / SET-090 | `set_090_updates_section_states`, `set_090_policy_disables_controls`, `set_090_manual_check_without_updater_reports_disabled`, `set_090_short_time` |
| UPD-011 | `upd_011_stale_manual_check_dropped` |
| UPD-012 | `upd_012_cleanup_keeps_only_pending`, `upd_012_*` |
| KBD-076 | `kbd_076_update_actions_in_palette`, `a11y_every_action_bound_or_in_palette` |

## Known gaps

- *Restart to update* doesn't list running terminal sessions or image pulls before quitting (the plan's risk table promised a confirmation). Follow-up.
- Remote signing (SignPath or Azure) signs `dockering.exe` and the setup, but not the uninstaller Inno writes at install time. `SignedUninstaller` needs a local `SignTool` (`DOCKERING_INNO_SIGNTOOL`). Follow-up in S-9.
- `/ALLUSERS` silent install in CI and the all-users update path through `runas` were not run on a real machine yet (CI `installer` job covers the first; release checklist covers the second).
- `PUBLIC_KEYS` in `dk-update/src/keys.rs` is empty until the release keys are generated, so updates fail closed until then (REL-060 checklist).

- macOS and Linux get notify-only updates. In-app install there (Sparkle-style `.app` swap, AppImage replace) is a follow-up.
- No *Preview* update channel yet (UPD-001 MAY).
- No delta updates. A full installer is about 20–30 MB.
