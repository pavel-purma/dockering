# Release checklist section (template)

Copy this into `docs/plan/release-checklist.md` above the previous release's section. Replace the
`<…>` placeholders and tick **only** what was verified in this run. Leave manual checks that were
not performed unchecked. Record variables by value, never secrets. If the release replaced a
withdrawn build, add a paragraph after the intro that says what failed and why the checks missed it.

```markdown
## vX.Y.Z (unsigned | signed)

Released from `main` at `<short sha>` as workspace version `X.Y.Z`: <why this bump, REL-011>.

### Before execution

- [ ] The maintainer approved the release (preparation PR, merge, tag, build, draft verification, publication): "<quote>", <date>.
- [ ] Preparation [PR #<n>](<url>) merged as `<sha>`; its checks passed ([run](<url>)).
- [ ] Workspace version and `CHANGELOG.md` match `vX.Y.Z`; the changelog date is the UTC publication date.
- [ ] Before the tag: `main` was `<sha>`, no tag or release for `vX.Y.Z` existed, no release run was active.
- [ ] Channel: `PUBLIC_RELEASES=<…>`, `WINDOWS_SIGNING=<…>`, `MACOS_SIGNING=<…>`, `WINDOWS_SIGNER_SUBJECT=<…>`.
- [ ] Third-party notices are current (CI checks this).

### Draft verification

- [ ] The release matrix passed on all six targets ([run](<url>)).
- [ ] `scripts/verify-release.ps1 -Expect <Signed|Unsigned>`: `<RESULT line>`.
- [ ] Exactly 15 files (16 with `.minisig` when signed); all `SHA256SUMS` entries and the six-platform manifest match.
- [ ] All attestations pass `gh attestation verify` with source digest `<sha>`, ref `refs/tags/vX.Y.Z`, the `release.yml` signer, self-hosted runners denied.
- [ ] Notes identify <unsigned downloads | the code signing policy and attribution>.
- [ ] Signed only: every Windows executable and installer is `Valid`, timestamped, SHA-256, one signer `<subject>`; `cargo xtask verify-manifest` verified the manifest with key #<n>.
- [ ] Windows CI installer tests passed for x64 and ARM64, per user and `/ALLUSERS` (they do not start the application).
- [ ] Launch check on this machine (profile written by `<previous version>`, then `--demo`): ran and passed, or not run (<why>).
- [ ] Windows machine: install over the previous release, launch from the Start Menu, Docker connection, uninstall.
- [ ] macOS machine: mount the DMG, copy and launch the app, Docker-compatible connection.
- [ ] Linux machine: launch the AppImage or tar archive, install the `.deb`, Docker connection.
- [ ] Application sanity: containers, detail and logs, terminal, one lifecycle operation, images, volumes, networks, engine switching, Settings, keyboard navigation.
- [ ] WSL and WSLC recorded separately: Run from Images, container ports and Inspect, terminal. Hosted CI does not verify these.

### Publication

- [ ] Draft and notes reviewed; the maintainer's approval covers publication.
- [ ] The existing draft was published; no assets were replaced.
- [ ] Published as `vX.Y.Z`, <stable/latest | pre-release>, `draft=false`, at `<UTC time>`.
- [ ] `verify-release.ps1 -Published`: every version-pinned URL and the `latest` URLs return HTTP 200; the latest API returns `vX.Y.Z` with all files.
- [ ] README download text and the release table in `docs/release.md` point to X.Y.Z.
- [ ] The `winget` run: <skipped | result> ([run](<url>)).

### Evidence for vX.Y.Z

Exact release commit: [`<sha>`](<url>). CI: [<id>](<url>). Release run: [<id>](<url>).
<Notes on anything unusual: reruns, flakes, approvals, deviations.>

### Open items

- <Anything found and not fixed, with where it is tracked.>
```
