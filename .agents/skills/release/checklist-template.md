# Release checklist section (template)

This file describes the current behaviour only. Copy this into `docs/plan/release-checklist.md` above the previous release's section. Replace the
`<…>` placeholders and tick **only** what was verified in this run. Leave manual checks that were
not performed unchecked. Record variables by value, never secrets. If the release replaced a
withdrawn build, add a paragraph after the intro that says what failed and why the checks missed it.
The channel has two independent parts (REL-083): OS signing (`PUBLIC_RELEASES`, `-Expect`) and the updater
(`UPDATER`, `-Updater`); record both, and read them from the draft, not from whether a `.minisig` exists.

```markdown
## vX.Y.Z (unsigned | signed)

Released from `main` at `<short sha>` as workspace version `X.Y.Z`: <why this bump, REL-011>.

### Before execution

- [ ] The maintainer approved the release (preparation PR, merge, tag, build, draft verification, publication): "<quote>", <date>.
- [ ] Preparation [PR #<n>](<url>) merged as `<sha>`; its checks passed ([run](<url>)).
- [ ] Workspace version and `CHANGELOG.md` match `vX.Y.Z`; the changelog date is the UTC publication date.
- [ ] Before the tag: `main` was `<sha>`, no tag or release for `vX.Y.Z` existed, no release run was active.
- [ ] Channel: `PUBLIC_RELEASES=<…>`, `UPDATER=<none|all|unset>`, `WINDOWS_SIGNING=<…>`, `MACOS_SIGNING=<…>`, `WINDOWS_SIGNER_SUBJECT=<…>`; so OS-<signed|unsigned>, updater <none|all>, manifest <signed|unsigned> (`sign_manifest`).
- [ ] Third-party notices are current (CI checks this).

### Draft verification

- [ ] The release matrix passed on all six targets ([run](<url>)).
- [ ] `scripts/verify-release.ps1 -Expect <Signed|Unsigned> -Updater <none|all>`: `<RESULT line>`.
- [ ] Update channel (REL-084): the `updater-evidence-X.Y.Z` artifact of the release run has six files, every one `updater=<true|false>` as the channel says and `seam=false`; `cargo xtask verify-update-artifacts --assets <dir> --version X.Y.Z --updater <none|all>`: `<result: signed by the current key #0 or no .minisig with --updater none; version; six entries>`.
- [ ] Exactly 15 files (16 with `.minisig` when the manifest is signed: updater `all` or an OS-signed build); all `SHA256SUMS` entries and the six-platform manifest match.
- [ ] All attestations pass `gh attestation verify` with source digest `<sha>`, ref `refs/tags/vX.Y.Z`, the `release.yml` signer, self-hosted runners denied.
- [ ] Notes identify <unsigned downloads | the code signing policy and attribution> and carry the sentence for the update channel (*Dockering checks for updates…* | *Automatic updates are not included…*).
- [ ] OS-signed only: every Windows executable and installer is `Valid`, timestamped, SHA-256, one signer `<subject>`.
- [ ] Windows CI installer tests passed for x64 and ARM64, per user and `/ALLUSERS` (they do not start the application).
- [ ] Launch check on this machine (profile written by `<previous version>`, then `--demo`): ran and passed, or not run (<why>).
- [ ] Windows machine: install over the previous release, launch from the Start Menu, Docker connection, uninstall.
- [ ] macOS machine (a physical Mac): mount the DMG, copy and launch the app, Docker-compatible connection. Install, launch, the page walk, quit and Gatekeeper's dialog on a quarantined copy are covered by the macOS smoke runs, which use `--demo` and a hosted runner.
- [ ] **Draft gate** (before publication, on the draft's own files): `Linux smoke` run ([run](<url>)) and `macOS smoke` run ([run](<url>)) for `vX.Y.Z`. Linux: all four legs passed (ubuntu-24.04, ubuntu-26.04, fedora, arch). macOS: all three legs passed (macos-15, macos-15-intel, macos-26). Screenshots reviewed. If a leg failed, the release was not published; leave this unticked and record the failed check under Open items.
- [ ] Real Linux machine (not a container): launch the app with the machine's GPU driver and connect to Docker. Install and launch in clean containers are covered by the Linux smoke runs, which use `--demo` and software rendering.
- [ ] Application sanity: containers, detail and logs, terminal, one lifecycle operation, images, volumes, networks, engine switching, Settings, keyboard navigation.
- [ ] WSL and WSLC recorded separately: Run from Images, container ports and Inspect, terminal. Hosted CI does not verify these.

### Publication

- [ ] Draft and notes reviewed; the maintainer's approval covers publication.
- [ ] The existing draft was published; no assets were replaced.
- [ ] Published as `vX.Y.Z`, <stable/latest | pre-release>, `draft=false`, at `<UTC time>`.
- [ ] `verify-release.ps1 -Updater <none|all> -Published`: every version-pinned URL and the `latest` URLs return HTTP 200; the latest API returns `vX.Y.Z` with all files.
- [ ] `Linux smoke` run for the published `vX.Y.Z` ([run](<url>)): all four legs passed (ubuntu-24.04, ubuntu-26.04, fedora, arch); their contact sheets (`contact-<slug>.png`) reviewed; `libc_floor` <INFO | passed>. If a leg failed or the run did not happen, leave this unticked and record the failed check and leg under Open items.
- [ ] `macOS smoke` run for the published `vX.Y.Z` ([run](<url>)): all three legs passed (macos-15, macos-15-intel, macos-26); first-frame and page screenshots reviewed; `bundle_seal` <INFO | passed>. If a leg failed or the run did not happen, leave this unticked and record the failed check and leg under Open items.
- [ ] README download text and the release table in `docs/release.md` point to X.Y.Z.
- [ ] The `winget` run: <skipped | result> ([run](<url>)).

### Evidence for vX.Y.Z

Exact release commit: [`<sha>`](<url>). CI: [<id>](<url>). Release run: [<id>](<url>).
<Notes on anything unusual: reruns, flakes, approvals, deviations.>

### Open items

- <Anything found and not fixed, with where it is tracked.>
```
