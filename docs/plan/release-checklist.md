# Release checklist

Use the exact release commit. Record run URLs and tested platforms; leave unavailable manual
checks unchecked. Each release has its own section, newest first; copy the latest one for the next release.

## v0.2.0 (unsigned)

Prepared from `main` at `910283d` as workspace version `0.2.0`: a `feat` (status bar names the
engine) since `v0.1.0` makes this a minor bump under REL-011. The other changes are the WSLC
COM/CLI repair and the no-console-window fix (REL-028).

### Before execution

- [ ] User approved pushing the preparation PR, tagging `v0.2.0`, and publishing it. The 2026-10-04 approval covered `v0.1.0` only.
- [x] Baseline: CI on `main` at `910283d` passes six builds, three test platforms, lint, and Docker integration ([37362705682](https://github.com/pavel-purma/dockering/actions/runs/37362705682)).
- [ ] Preparation PR is merged; exact-commit CI passes six builds, three test platforms, lint, and Docker integration.
- [ ] Workspace version and `CHANGELOG.md` match `v0.2.0`; the changelog date is the actual publication date.
- [ ] No `v0.2.0` tag or release exists, or an unpublished draft/run is intentionally being resumed.
- [ ] Repository variable list is empty; `PUBLIC_RELEASES`, `WINGET_ENABLED`, and `WINDOWS_SIGNING` are unset.
- [ ] Third-party notices are current (CI checks this).

### Draft verification

- [ ] Release matrix passes Windows x64/ARM64, macOS Intel/Apple silicon, Linux x64/ARM64.
- [ ] Draft has 12 distributions, `dockering-update.json`, `SHA256SUMS`, and `RELEASE_NOTES.md` (15 files); no `.minisig`.
- [ ] Downloaded files verify against `SHA256SUMS`; the manifest has six platforms and `v0.2.0` URLs.
- [ ] `gh attestation verify` passes for the downloaded files (source digest, tag ref, release workflow).
- [ ] Notes identify unsigned downloads, installation prompts, disabled updater, and unavailable winget.
- [ ] Windows CI installer tests pass for x64/ARM64, per user and `/ALLUSERS`, including the GUI-subsystem and `--version` checks (REL-028).
- [ ] Available Windows machine: install over an existing 0.1.0, launch (only the app window, no console window), version 0.2.0, Docker connection, uninstall.
- [ ] Available macOS machine: mount DMG, copy/launch app, and Docker-compatible connection.
- [ ] Available Linux machine: launch AppImage/tar archive, install `.deb`, and Docker connection.
- [ ] Application sanity on available machines: containers, detail/logs, terminal, one lifecycle
      operation, images/volumes/networks, engine switching, Settings, and keyboard navigation.
- [ ] WSL/WSLC results recorded separately: Run from Images, container ports and Inspect, terminal. Hosted CI does not verify these integrations.

### Publication

- [ ] Draft and notes reviewed; user approval covers publication.
- [ ] Published the existing draft; no assets replaced after publication.
- [ ] Published version is `v0.2.0`, stable/latest, `draft=false`, `prerelease=false`.
- [ ] Version-pinned asset URLs and permanent latest distribution links return HTTP 200 anonymously; all files remain present.
- [ ] `README.md` download text and verification links point to 0.2.0; release/run URLs and remaining manual checks recorded in this file.

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
