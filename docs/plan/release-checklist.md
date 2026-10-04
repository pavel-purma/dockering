# Release checklist

Use the exact release commit. Record run URLs and tested platforms; leave unavailable manual
checks unchecked.

## Before execution

- [x] User approved pushing preparation changes and executing the release, including publication (2026-10-04).
- [ ] Preparation PR is merged; exact-commit CI passes six builds and three test platforms.
- [ ] Workspace version/changelog match `v0.1.0`; changelog date is the actual release date.
- [ ] No existing tag/release conflicts, or an existing draft/run is intentionally being resumed.
- [ ] `PUBLIC_RELEASES`/`WINGET_ENABLED` are unset/false; `WINDOWS_SIGNING` is unset/`none`.
- [ ] Third-party notices are current (CI checks this).

## Draft verification

- [ ] Release matrix passes Windows x64/ARM64, macOS Intel/Apple silicon, Linux x64/ARM64.
- [ ] Draft has 12 distributions, `dockering-update.json`, `SHA256SUMS`, and `RELEASE_NOTES.md`.
- [ ] Downloaded files verify against SHA256SUMS; manifest has six platforms and correct tag URLs.
- [ ] Public provenance verifies with `gh attestation verify`.
- [ ] Notes identify unsigned downloads, installation prompts, disabled updater, and unavailable winget.
- [ ] Windows CI installer tests pass; manual per-user/all-users results are recorded if run.
- [ ] Available Windows machine: launch, version, shortcut, Docker connection, and uninstall.
- [ ] Available macOS machine: mount DMG, copy/launch app, and Docker-compatible connection.
- [ ] Available Linux machine: launch AppImage/tar archive, install `.deb`, and Docker connection.
- [ ] Application sanity on available machines: containers, detail/logs, terminal, one lifecycle
      operation, images/volumes/networks, engine switching, Settings, and keyboard navigation.
- [ ] WSL/WSLC results recorded separately; hosted Windows CI does not verify these integrations.

## Publication

- [ ] Draft and notes reviewed; user approval covers publication.
- [ ] Publish the existing draft; do not replace assets after publication.
- [ ] Published version, stable/prerelease status, and latest setting are correct.
- [ ] Published asset URLs and permanent download links work; all distributions remain present.
- [ ] Record release/run URLs and manual checks that remain unverified.
