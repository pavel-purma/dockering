# Release checklist

Use the exact release commit. Record run URLs and tested platforms; leave unavailable manual
checks unchecked. Each release has its own section, newest first. The `/release` skill copies the
[template](../../.agents/skills/release/checklist-template.md) for the next release; ticks only what it verified.

## Windows engine start and setup (not executed)

[Spec](../spec/features/windows-engine-start.md), [plan](features/windows-engine-start.md), ENG-142…164 / REL-088, REL-089. Unchecked until a person runs them on the named machine.

- [ ] Docker Desktop **stopped** → open Dockering → the first-run page lists *Docker Desktop isn't running* → *Start Docker Desktop* → it connects without another click. Record: the Docker Desktop version, the time to connect, and whether the Docker Desktop window opened. Not observed in the spike.
- [ ] Docker Desktop in Resource Saver / paused: no start is offered and the engine connects on first use; record the `docker desktop status` text for stopped, starting, paused and Resource Saver (spike W-1.5) and whether both pipes are absent when stopped.
- [ ] Docker Desktop installed per user (HKCU) and an old version without the `desktop` plugin: the same start works (exe fallback).
- [ ] A distro with Docker installed and stopped (`sudo systemctl stop docker`) → *Start Docker* → connects. A distro that only has Docker Desktop's integration is not offered a start.
- [ ] A machine with WSL and no Docker, once the first `rootfs-<n>` is published and the pins flipped: *Set up Docker in WSL* → the dialog opens with nothing selected and *Set up* disabled → pick Ubuntu → progress → the engine is active and `hello-world` runs; repeat on a clean machine with Alpine. Cancel during the download leaves no distro and no cache file (`%LOCALAPPDATA%\Dockering\cache`); cancel during the import leaves no `dockering-engine`.
- [ ] After a setup, the Start Menu and Windows Terminal show no `dockering-engine` entry on either image (ENG-164); close Dockering and see the containers stop about 15 s later, matching the consent text; reopen and the page of the stopped engine states the same (ENG-163).
- [ ] An existing distro named `dockering-engine` (made by hand) is never overwritten or unregistered, and no setup is offered.
- [ ] Linux and macOS: no start or setup offer appears.
- [ ] Keyboard only: reach *Start*, *Set up Docker in WSL…* (palette), pick an image with the arrow keys, run and cancel; focus returns to the page. Screenshots of the first-run page with a start row and an offer, the empty dialog, the dialog after a pick and the progress view, in light and dark.
- [ ] CI: `engine-rootfs.yml` runs once from `main` without `publish` (4 builds, 2 smoke jobs on the `wsl` runner), then with `publish`; remove `--allow-unpublished` from `ci.yml` and `release.yml` in the same PR that flips the pins.

## Planned updater gates (not executed)

Draft [plan](features/linux-appimage-updates.md), REL-083…087 / UPD-013…017.
These unchecked gates do not alter any historical release result below.

- [x] Approve the revised plan and ADR-0007 (shared updater, installs without OS signing, macOS *Download* only): approved 2026-10-19.
- [ ] Maintainer: update keys per signing.md Phase 4 (no agent holds a private key); public keys committed.
- [ ] Gate: `updater` / `sign_manifest` truth table; a rehearsal from `main` with `updater=all` (unsigned is enough) builds
  all six targets with the updater, `updater-evidence-*.txt` agree, no seam, `.minisig` made by the current key,
  manifest URL/kind/hash/size/version match the draft.
- [ ] Shared on all three OSes (view tests in CI): one notification per version, title-bar button from the offer,
  retention through re-checks and offline start, withdrawal of a pulled release, Settings offer row.
- [x] Windows x64/ARM64, CI (REL-087 b): the `installer-smoke.ps1` update leg (`-UpdateFrom` / `-UpdateFromSums`; build step
  *Update leg, last release to this build*) is green on `windows-2025` and `windows-11-arm`: the last stable release's installer,
  checked against its `SHA256SUMS`, installed per user; the installer under test run with exactly the updater's argv; version,
  files and a `/RELAUNCH` process from the install dir checked; silent uninstall leaves nothing. Record the run, the commit, the
  old and the new version and which mode ran: strict (a version bump) or `-AllowSameVersion` (the last release has this
  version, so the version change itself is not asserted; the step prints a `SKIP` line). A skipped step (no release, no GitHub)
  is not a pass. A CI run is not a W1 result and proves none of the rows below. **Green 2026-10-20 on both architectures:** [run 39393494930](https://github.com/pavel-purma/dockering/actions/runs/39393494930), commit `2ab9bec` (pull request #54), `Dockering-Setup-x64.exe` and `Dockering-Setup-arm64.exe` of v0.3.0 over a build that also reports 0.3.0, so the mode was `-AllowSameVersion` and the step printed its `SKIP` line (the version change is not asserted until a newer build exists, so the next release's PR is the first strict run). The installer under test ran with `/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /UPDATE /RELAUNCH /CURRENTUSER` and exited 0, `/RELAUNCH` started `dockering.exe`, the uninstall was clean.
- [ ] Windows, **spike W1, manual** (REL-087 a): the maintainer on a real Windows 10/11 machine, with a throwaway per-user
  profile or VM for the all-users rows. Record per row: pass or fail, Windows build and architecture, the commit, the installer's
  artifact name and SHA-256, and a screenshot for UAC and SmartScreen. A Linux run, a mock or a CI runner completes none of
  them. Nothing below is checked, and the plan's expectations stay expectations until it is:
  - [ ] **Mark-of-the-Web / SmartScreen:** an installer written by the app's own HTTP client has no `Zone.Identifier` stream
    (`Get-Item -Stream * <installer>` lists only `:$DATA`), and SmartScreen does not intervene when the app starts it. Compare
    with the same installer saved from a browser.
  - [ ] **Defender and Smart App Control:** an unsigned app starting an unsigned installer is not blocked or quarantined, with
    Smart App Control on and off. If either blocks it: the offer must become *Download* with the reason (UPD-007), and the
    finding is recorded.
  - [ ] **UAC text:** the prompt for an **all-users** unsigned install says *Unknown publisher* (screenshot). Declining it shows
    *Couldn't start the update* and leaves the app running with the same offer.
  - [ ] **Share-locked start:** Inno starts while the app holds the installer `FILE_SHARE_READ`-locked, per user and all users.
    If it does not: a per-user install retries once after another hash check, an all-users install falls back to *Download*
    rather than elevating an unlocked file. Record which of the three happened.
  - [ ] **Cancel:** cancelling the installer (close the progress window, or end the process) leaves the old app installed and
    working, and the next launch shows the same offer again.
  - [ ] **Relaunch:** after an all-users update the new Dockering is started by Explorer and is **not elevated** (Task Manager,
    *Elevated* column); after a per-user update it starts as the user. The window appears (the CI leg asserts only the process).
  - [ ] **`WinVerifyTrust` for an unsigned PE:** on the Windows machine run
    `cargo test -p dk-update --lib -- _w1 --nocapture` (the three tests below are `#[cfg(windows)]` and not `#[ignore]`d, so this
    filter selects exactly them; on Linux and macOS it lists 0 tests; CI's `test (windows-2025)` job runs them too, on a hosted
    runner, which is CI evidence only). `upd_003_windows_unsigned_exe_is_no_signature_w1`
    passes only if the unsigned test binary maps to *no signature*, that is, the status is in `NO_SIGNATURE_STATUSES`
    (today only `TRUST_E_NOSIGNATURE`, `0x800B0100`); on failure its message carries `WinVerifyTrust status 0x…`: record that
    value, and widen the list only from a recorded result. It prints no status on success. Cross-check the released installer
    with `(Get-AuthenticodeSignature .\Dockering-Setup-x64.exe).Status` (expected `NotSigned`).
  - [ ] **`upd_003_windows_open_handle_verdict_matches_path_w1`** passes: the verdict through an open handle equals the verdict by
    path, for the unsigned test binary and, where they exist, `C:\Program Files\PowerShell\7\pwsh.exe` and
    `C:\Program Files\Git\cmd\git.exe` (install both at those paths so the signed-file half runs).
  - [ ] **`upd_007_locked_per_user_start_works_w1`** passes: a per-user start of a `FILE_SHARE_READ`-locked image (a copy of
    `hostname.exe`). If it fails with `LockedStartFailed`, Windows refuses such a start and the UPD-007 fallbacks apply.
- [x] Linux, end-to-end (REL-085, REL-087 c): `linux-update-e2e.yml` (a green pull-request run on `ubuntu-24.04`, with its
  `linux-update-e2e` artifact) runs S1 (extract-and-run and FUSE legs) to S4. **Green 2026-10-20:** [run 39393495043](https://github.com/pavel-purma/dockering/actions/runs/39393495043) on commit `2ab9bec` (pull request #54), image `mirror.gcr.io/library/ubuntu:24.04` digest `sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac`, 16 minutes with the build; S1 extract 23, S1 FUSE 23, S2 24, S3 19, S4 24 checks, no FAIL or SKIP; the artifact has no AppImage and no key. *Before that, run locally 2026-10-20 through `scripts/linux-update/run.sh` on `ghcr.io/archlinux/archlinux:latest`
  (glibc 2.44, the build host's): `--scenario all` passed S1 in both legs (23 of 23 checks) and S2, S3 and S4 in the extract leg (24, 19 and 24 checks). The same
  AppImages built on the Arch host are rejected on Ubuntu 24.04 by the loader (`GLIBC_2.44 not found`), which is expected: the CI builds its own on the Ubuntu runner.*
  - [ ] **This machine, by hand** (the maintainer's desktop; real display, real FUSE; use a scratch `HOME` or back up
    `~/.local/share/dockering` first, because the old build in this test overwrites `state.json` and `offer.json`):
    1. Build the pair: `scripts/linux-update/build-appimages.sh ~/dk-e2e --source worktree`, then `cargo xtask update-e2e-fixture
       --out ~/dk-e2e/fixture --appimage ~/dk-e2e/Dockering-x86_64.AppImage --version <its version>` and serve
       `~/dk-e2e/fixture/root` with `python3 scripts/linux-update/serve.py --root ~/dk-e2e/fixture/root --port 8099 --log ~/dk-e2e/req.jsonl`.
    2. Install the OLD AppImage as `~/.local/bin/dockering` and start it **from the launcher** (the `.desktop` entry's `Exec`,
       not a terminal) with `DOCKERING_UPDATE_E2E_URL=http://127.0.0.1:8099`, `DOCKERING_UPDATE_E2E_KEY=$(tr -d '\n' < ~/dk-e2e/fixture/public-key.txt)` and `DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS=5` in the environment the launcher gives it.
    3. Within seconds: one notification with *Restart now*, and the same icon in the title bar next to the other icons
       (screenshot). Close and reopen the window: the icon is still there and there is no second notification.
    4. *Restart now*: the window closes and the new version opens by itself (screenshot of 'Updated to …'), the icon is gone,
       `sha256sum ~/.local/bin/dockering` equals the new AppImage's, and `pgrep -a dockering` shows no process of the old mount.
    5. Repeat with `~/.local/bin/dockering` a **symlink** to an AppImage in another directory: the target is replaced, the link
       stays a link. Repeat with the directory not writable (`chmod a-w ~/.local/bin`): a *Download* offer, no asset request.
    6. Record the Hyprland and runtime versions, the commit, and attach the screenshots.
- [ ] Linux, `.deb` and tarball installs: the offer is *Download* (UPD-006). *Covered by Tier 1 tests; not run on a real `.deb`.*
- [ ] macOS: the update shows a *Download* offer (no in-app install; UPD-016 is deferred).
- [ ] Attach exact commit, artifact, OS and architecture to each result. Linux runs and mocks do not complete the Windows items.
- [ ] First updater release installed by hand, then a second release updates it by itself.

## v0.3.0 (unsigned)

Released from `main` at `7bef9c1` as workspace version `0.3.0`: three `feat` commits since `v0.2.0` (keyboard navigation polish, Combined grouping, the WSLC start page) make this a minor bump under REL-011.

### Before execution

- [x] The maintainer approved the release PR at Gate 1 and publication at Gate 2 (2026-10-11).
- [x] Preparation [PR #45](https://github.com/pavel-purma/dockering/pull/45) merged as `7bef9c1`; all checks passed.
- [x] Workspace version and `CHANGELOG.md` match `v0.3.0`; the changelog date is the UTC publication date.
- [x] Before the tag: `main` was `73d8374`, no tag or release for `v0.3.0` existed, no release run was active.
- [x] Channel: unsigned; no repository variables are set.
- [x] Third-party notices are current (CI checks this).

### Draft verification

- [x] The release matrix passed on all six targets ([run](https://github.com/pavel-purma/dockering/actions/runs/38156108105)).
- [x] `scripts/verify-release.ps1 -Expect Unsigned`: `RESULT 57 checks: 56 passed, 0 failed, 1 skipped`.
- [x] All 15 attestations pass with source digest `7bef9c1`, ref `refs/tags/v0.3.0`.
- [x] Windows CI installer tests passed (they do not start the application).
- [ ] Launch check on this machine: not run (declined at Gate 2).
- [ ] Windows machine: install over the previous release, launch from the Start Menu, Docker connection, uninstall.
- [ ] macOS machine (a physical Mac): mount the DMG, copy and launch the app, Docker-compatible connection.
- [x] **Draft gate** (before publication): `Linux smoke` [run](https://github.com/pavel-purma/dockering/actions/runs/38157437542) and `macOS smoke` [run](https://github.com/pavel-purma/dockering/actions/runs/38157439583) passed on every leg. Screenshots not reviewed.
- [ ] Real Linux machine (not a container): launch the app and connect to Docker.
- [ ] Application sanity: containers, detail and logs, terminal, one lifecycle operation, images, volumes, networks, engine switching, Settings, keyboard navigation (new: Combined grouping, search `Enter`, detail-tab keys).
- [ ] WSL and WSLC recorded separately, including the new start page against a missing default session.

### Publication

- [x] Draft reviewed; the maintainer approved publication.
- [x] The existing draft was published; no assets were replaced.
- [x] Published as `v0.3.0`, stable/latest, `draft=false`, at `2026-10-11T17:29:04Z`.
- [x] `verify-release.ps1 -Published`: `RESULT 70 checks: 68 passed, 0 failed, 2 skipped`.
- [x] `Linux smoke` run for the published `v0.3.0` ([run](https://github.com/pavel-purma/dockering/actions/runs/38159829824)): passed. Screenshots not reviewed.
- [x] `macOS smoke` run for the published `v0.3.0` ([run](https://github.com/pavel-purma/dockering/actions/runs/38159829781)): passed. Screenshots not reviewed.
- [x] README download text and the release table in `docs/release.md` point to 0.3.0.
- [x] The `winget` run: skipped.

### Evidence for v0.3.0

Exact release commit: [`7bef9c1`](https://github.com/pavel-purma/dockering/commit/7bef9c151f8246801a6d00a10a64e1157e8cda35). CI: [38156094214](https://github.com/pavel-purma/dockering/actions/runs/38156094214). Release run: [38156108105](https://github.com/pavel-purma/dockering/actions/runs/38156108105).
Exact-commit CI failed once in `test (windows-2025)` on `hub_shutdown_flushes_and_returns_quickly` (the only failure) and passed on `gh run rerun --failed`; reported as a flake, not a pass.

### Open items

- Flaky test `hub_shutdown_flushes_and_returns_quickly` on the Windows runner.

## v0.2.0 (unsigned)

Released from `main` at `2d25f6c` as workspace version `0.2.0`: a `feat` (status bar names the
engine) since `v0.1.0` makes this a minor bump under REL-011. The other changes are the WSLC
COM/CLI repair, the no-console-window fix (REL-028), and the launch-crash fix below.

**A first build of v0.2.0 (`813f543`, published `2026-10-05T22:04:24Z`) was withdrawn.** It crashed
at launch on any profile written by an earlier version: `AppShell::new` pushed the UPD-008 "Updated
to Dockering 0.2.0" notice before the window's root existed, which panics with `component window
state is missing` before `last_run_version` is saved, so every launch crashed (exit code 101 after
310 ms; a window flashed and closed). A profile with no previous version pushes no notice and is not
affected. The maintainer found it by installing over 0.1.0 and removed the release. The fix is
[PR #35](https://github.com/pavel-purma/dockering/pull/35) (`2d25f6c`). The maintainer asked to
"re-release v 0.2.0", so the tag `v0.2.0`, whose release had been removed, was moved from `813f543`
to `2d25f6c` (`--force-with-lease` on the old tag object), and the release was rebuilt, verified and
published again. Everything below describes that second build. The first build's checks (checksums, manifest,
attestations, PE/ELF/Debian/DMG formats, `--version`) all passed and none of them started the
application; see *Open items*.

### Before execution

- [x] User approved the full release flow (preparation PR, merge, tag, build, draft verification, publication) on 2026-10-05 (UTC): "Just go ahead and do all necessary for the release publishing". After the crash the user asked for a fix and a re-release: "fix that. Then re-release v 0.2.0". The 2026-10-04 approval covered `v0.1.0` only.
- [x] First build: preparation [PR #33](https://github.com/pavel-purma/dockering/pull/33) merged as `813f543`, CI [37375900373](https://github.com/pavel-purma/dockering/actions/runs/37375900373) and release run [37376009959](https://github.com/pavel-purma/dockering/actions/runs/37376009959) green; withdrawn as described above.
- [x] Fix: [PR #35](https://github.com/pavel-purma/dockering/pull/35) passes six builds, three test platforms, lint, and Docker integration ([37390924336](https://github.com/pavel-purma/dockering/actions/runs/37390924336), 12 of 12 checks, first attempt). Exact-commit CI on `2d25f6c` passes the same ([37393162785](https://github.com/pavel-purma/dockering/actions/runs/37393162785), 11 of 11 jobs, first attempt). The PR's tree is identical to the merge commit's.
- [x] Workspace version and `CHANGELOG.md` match `v0.2.0`; the changelog date, 2026-10-06, is the UTC publication date, and the section says that it replaces a withdrawn first build.
- [x] Before the tag was moved: `main` was `2d25f6c`, the remote tag still pointed at `813f543`, no release for `v0.2.0` existed (draft or published), and no release run was active. Immutable releases are off.
- [x] Repository variable list is empty; `PUBLIC_RELEASES`, `WINGET_ENABLED`, and `WINDOWS_SIGNING` are unset. The `winget` run started by publishing was skipped ([37394968138](https://github.com/pavel-purma/dockering/actions/runs/37394968138)).
- [x] Third-party notices are current (CI checks this).

### Draft verification

- [x] Release matrix passes Windows x64/ARM64, macOS Intel/Apple silicon, Linux x64/ARM64 ([37393414312](https://github.com/pavel-purma/dockering/actions/runs/37393414312), first attempt, all nine jobs).
- [x] Draft has 12 distributions, `dockering-update.json`, `SHA256SUMS`, and `RELEASE_NOTES.md` (15 files); no `.minisig`.
- [x] All 14 `SHA256SUMS` entries match (PowerShell `Get-FileHash`); the manifest has six platforms and `v0.2.0` URLs, kinds, hashes and sizes.
- [x] All 15 files pass `gh attestation verify` with the exact source digest `2d25f6c…`, ref `refs/tags/v0.2.0`, the `release.yml` signer, and self-hosted runners denied.
- [x] Notes identify unsigned downloads, installation prompts, disabled updater, and unavailable winget; the published body equals the `RELEASE_NOTES.md` asset and starts with the verbatim `0.2.0` changelog section.
- [x] Windows CI installer tests pass for x64/ARM64, per user and `/ALLUSERS`, including the GUI-subsystem and `--version` checks (REL-028), inside the release run. They do not start the application.
- [x] Downloaded Windows packages: both portable executables have the expected x64/ARM64 PE machine type and the **GUI subsystem** (REL-028; v0.1.0 had the console subsystem), embed version 0.2.0, and report `NotSigned`, as do both installers. Both archives hold the executable, `LICENSE` and `THIRD_PARTY_LICENSES.html`.
- [x] Linux packages: ELF machine types of the tar.gz and AppImage binaries match their architecture; the `.deb` control files say version `0.2.0` with the right architecture, and the payload binaries match. Both DMGs are nonempty with the UDIF `koly` trailer. These are binary and archive checks, not launches.
- [x] **Launch checks** (Windows 11 x64, the downloaded `Dockering-x64.zip` executable): on the real profile written by 0.1.0, the condition that crashed the first build, it stayed running for 12 s, opened a window titled "Dockering" showing "Updated to Dockering 0.2.0." with *What's new*, connected to the local WSL containers engine (3.0.1, COM), and exited with code 0 on a close request, with no crash file and no error or warning in the log; a second launch on the same profile did the same; `--demo` (an isolated fresh profile) did too. The profile files were restored byte for byte afterwards. The first build crashed on the same profile.
- [ ] Windows machine: install the new installer over the withdrawn first build and over 0.1.0, launch from the Start Menu, Docker connection, uninstall. Not done: the launch checks above used the portable executable.
- [ ] Available macOS machine: mount DMG, copy/launch app, and Docker-compatible connection.
- [ ] Available Linux machine: launch AppImage/tar archive, install `.deb`, and Docker connection.
- [ ] Application sanity on available machines: containers, detail/logs, terminal, one lifecycle
      operation, images/volumes/networks, engine switching, Settings, and keyboard navigation. Only the launch and the empty Containers page were seen.
- [ ] WSL/WSLC results recorded separately: Run from Images, container ports and Inspect, terminal. Hosted CI does not verify these integrations.

### Publication

- [x] Draft and notes reviewed; user approval covers publication.
- [x] Published the existing draft; no assets replaced after publication.
- [x] Published version is `v0.2.0`, stable/latest, `draft=false`, `prerelease=false`, at `2026-10-06T00:37:20Z`; `v0.1.0` is no longer latest.
- [x] All 15 version-pinned asset URLs and 12 permanent latest distribution links return HTTP 200 anonymously after redirects; the latest API returns `v0.2.0` with all 15 files.
- [x] `README.md` download text and verification links point to 0.2.0; release/run URLs and remaining manual checks recorded in this file.

### Evidence for v0.2.0

Exact release commit: [`2d25f6c108f2749f7a2225b4b968742516713d23`](https://github.com/pavel-purma/dockering/commit/2d25f6c108f2749f7a2225b4b968742516713d23).
CI: [37393162785](https://github.com/pavel-purma/dockering/actions/runs/37393162785). Release:
[37393414312](https://github.com/pavel-purma/dockering/actions/runs/37393414312), at the same commit.
[Dockering 0.2.0 (unsigned)](https://github.com/pavel-purma/dockering/releases/tag/v0.2.0) was
published stable/latest at `2026-10-06T00:37:20Z`; its
[SHA256SUMS](https://github.com/pavel-purma/dockering/releases/download/v0.2.0/SHA256SUMS) are listed above.

Scripted checks ran against the draft (a local script, not committed): file set, `SHA256SUMS`,
manifest, notes, PE machine/subsystem/version, ELF machine, `.deb` control and payload, DMG trailer,
and archive contents. 114 checks, 0 failures, for both builds. As a self-test, the same script
passed against the published v0.1.0 when told to expect that release's console subsystem, and
failed exactly the two GUI-subsystem checks otherwise.

### Open items

- **No CI step starts the application.** `scripts/installer-smoke.ps1` runs `--version` and checks the
  PE subsystem, and the view tests build the window without a previous-version profile, so a crash on
  the first launch after an upgrade passed everything. The check that found it was installing over an
  older version and starting the app. A launch check on a profile with an older `last_run_version` is
  the missing test; whether hosted Windows runners can open a GPUI window is not known.
- `net_004_prune_unused_networks` is flaky on CI: `assert!(h.has_dialog(cx))` failed on Linux in the
  first preparation PR and on Windows in the CI of the docs-only PR that records this release (the code
  was identical to the green run on `main`), and it passed on rerun. 300 local scheduler seeds did not
  reproduce it, and #25 already fixed one race in it. Root cause not found.
- The first preparation PR's CI also lost three jobs ("The job was not acquired by Runner of type
  hosted even after multiple attempts") during a GitHub Actions incident; reruns passed.

## v0.1.0 (unsigned)

### Before execution

- [x] User approved pushing preparation changes and executing the release, including publication (2026-10-04).
- [x] Preparation [PR #24](https://github.com/pavel-purma/dockering/pull/24) and fixes [#25](https://github.com/pavel-purma/dockering/pull/25)/[#27](https://github.com/pavel-purma/dockering/pull/27) are merged; [exact-commit CI](https://github.com/pavel-purma/dockering/actions/runs/37228704953) passes all six packages, three test platforms, lint, and Docker integration.
- [x] Workspace version/changelog match `v0.1.0`; downloaded x64 executable prints `dockering 0.1.0`.
- [x] Changelog date matches the actual publication date, 2026-10-04.
- [x] Before publication, the tag was intentionally repointed to corrected commit `041d8fac06f1a2d26a61ed5e13ad16f47371f628`; obsolete failed run [37227501895](https://github.com/pavel-purma/dockering/actions/runs/37227501895) was superseded.
- [x] Repository variable list is empty; `PUBLIC_RELEASES`, `WINGET_ENABLED`, and `WINDOWS_SIGNING` are unset.
- [x] Third-party notices are current (exact-commit CI check passed).

### Draft verification

- [x] Release matrix passes Windows x64/ARM64, macOS Intel/Apple silicon, Linux x64/ARM64.
- [x] Verified draft and published release have 12 distributions, `dockering-update.json`, `SHA256SUMS`, and `RELEASE_NOTES.md` (15 files); no `.minisig`.
- [x] All 14 `SHA256SUMS` entries match; six-platform manifest schema/version/URLs/filenames/kinds/hashes/sizes and notes URL are correct.
- [x] All 15 files pass `gh attestation verify` with the exact source digest, tag ref, release workflow signer, and self-hosted runners denied.
- [x] Notes identify unsigned downloads, installation prompts, disabled updater, and unavailable winget.
- [x] Native Windows x64/ARM64 CI installer tests pass for per-user and `/ALLUSERS`; real-machine GUI installation/update checks are not performed.
- [ ] Available Windows machine: launch (only the app window, no console window, REL-028), version, shortcut, Docker connection, and uninstall.
- [ ] Available macOS machine: mount DMG, copy/launch app, and Docker-compatible connection.
- [ ] Available Linux machine: launch AppImage/tar archive, install `.deb`, and Docker connection.
- [ ] Application sanity on available machines: containers, detail/logs, terminal, one lifecycle
      operation, images/volumes/networks, engine switching, Settings, and keyboard navigation.
- [ ] WSL/WSLC results recorded separately; hosted Windows CI does not verify these integrations.

### Publication

- [x] Draft and notes reviewed; user approval covers publication.
- [x] Published the existing draft; no assets replaced after publication.
- [x] Published version is `v0.1.0`, stable/latest, `draft=false`, `prerelease=false`.
- [x] All 15 version-pinned asset URLs and 12 permanent latest distribution links return HTTP 200 anonymously after redirects; all files remain present.
- [x] Release/run URLs and remaining manual checks recorded below.

### Evidence for v0.1.0

Exact release commit: [`041d8fac06f1a2d26a61ed5e13ad16f47371f628`](https://github.com/pavel-purma/dockering/commit/041d8fac06f1a2d26a61ed5e13ad16f47371f628).
CI: [37228704953](https://github.com/pavel-purma/dockering/actions/runs/37228704953), successful,
including eight recovery/parse tests under native macOS Bash 3.2. Release:
[37228719100](https://github.com/pavel-purma/dockering/actions/runs/37228719100), successful
at the same exact commit. [Dockering 0.1.0 (unsigned)](https://github.com/pavel-purma/dockering/releases/tag/v0.1.0)
was published stable/latest at `2026-10-04T20:09:38Z`.

Downloaded all 15 files; all 14 entries in
[SHA256SUMS](https://github.com/pavel-purma/dockering/releases/download/v0.1.0/SHA256SUMS) match.
The manifest's schema/version, six platform entries, asset names/kinds/hashes/sizes/version URLs,
and notes URL verify; no signature file is included. All 15 attestations verify against the
release source SHA, `refs/tags/v0.1.0`, and `release.yml` signer, denying self-hosted runners.
Anonymous requests follow redirects to HTTP 200 for all 15 version-pinned asset URLs and all
12 latest distribution links. The latest API returns `v0.1.0` with all 15 files and
`draft=false`, `prerelease=false`. Repository signing/updater/winget variables remain unset.

Downloaded Windows portable packages have the expected x64/ARM64 PE architecture, and setup/
portable executables report `NotSigned`. Downloaded x64 `--version` prints `dockering 0.1.0`.
Linux ELF and Debian metadata/payload architectures/version are correct. Both nonempty DMGs
have the UDIF `koly` trailer. Portable zip/tar packages include the MIT license and third-party
notices. These are binary/archive checks; GUI, physical macOS/Linux,
and WSL/WSLC walkthroughs remain unperformed.
