# Release checklist

Use the exact release commit. Record run URLs and tested platforms; leave unavailable manual
checks unchecked. Each release has its own section, newest first; copy the latest one for the next release.

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
- `net_004_prune_unused_networks` failed once on Linux CI in the first preparation PR
  (`assert!(h.has_dialog(cx))`), and passed on rerun and across 300 local scheduler seeds. #25 already
  fixed one race in it, so another remains and needs a root-cause look.
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
