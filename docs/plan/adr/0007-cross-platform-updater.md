# ADR-0007: One shared updater; install without OS code signing

- **Date:** 2026-10-19
- **Status:** accepted (2026-10-19), with the [plan](../features/linux-appimage-updates.md)
- **Requirements:** UPD-003…017, REL-016/060/083…087, SHL-022

## Context

The updater (UPD-001…012) exists but is compiled in only when OS code signing is on, so no published
build checks for updates, and Linux and macOS can only notify. OS code signing (Windows Authenticode, Apple) is not
available and must not gate updates. Common desktop updaters (for example VS Code, Electron/Squirrel and Zed)
work the same way: check on a schedule, download in the background, show one 'ready' prompt, install
on restart, and fall back to a download link where in-place install isn't possible. Historical ADRs 0001, 0005
and 0006 are not in this checkout; spec 10/21 and AGENTS.md carry their layering rules.

## Decision

| Topic | Decision |
|---|---|
| Shared behaviour | One hub coordinator and one UI on Windows, macOS and Linux: schedule, opt-outs, manual checks, signed-manifest trust, one notification per version, title-bar button, Settings, retained offer. |
| Install strategies | Windows: the existing Inno installer. Linux: in-place AppImage swap. Everything else, **macOS included**, offers *Download*. macOS in-app install is deferred (UPD-016): nothing in the release flow produces a hardened-runtime, notarised `.app` yet, and a bundle exchange needs `unsafe` FFI. |
| Trust root | The minisign signature of `dockering-update.json` (embedded keys: current + next), plus the size and SHA-256 of each asset. OS signatures are an extra pin only where the *running* app has one (a signed Windows exe never accepts an unsigned installer). An unsigned release installs on manifest trust alone, as an AppImage always does. |
| Inclusion | `UPDATER=none\|all` (repo variable; manual-run input `inherit\|none\|all`), independent of `PUBLIC_RELEASES` and the `unsigned` input. `sign_manifest = updater==all or signed`. |
| Offer | `offer.json` in `<data-local>/updates/` holds the signed manifest and the stage. It mirrors the newest authenticated manifest that is newer than the running version: failed checks keep it, a successful check with nothing newer withdraws it, and an offline copy older than 14 days unconfirmed is dropped. No separate transaction or recovery journal: the offer is deleted when the running version catches up, and a cancelled installer simply leaves the offer in place. |
| Apply | Own flag, separate from the check lock, so a click never waits for or fails because of a re-check. Typed `ApplyOutcome`: `Restarting` or `InstalledRestartManually`. |
| Windows installer safety | Open with `FILE_SHARE_READ` only and hold the handle from the final check until the app exits; a per-machine (UAC) install that can't be held falls back to *Download*. |
| Linux relaunch | Stage in the target's directory, run `--version` on the staged file, `rename`, start with `--update-relaunch=1` and a piped stdin that the old process holds until it exits; the new process blocks on stdin EOF (30 s) before it reads any state. |

## Threat model and limits

- **Network attacker / compromised CDN:** can't forge a manifest (signature) or swap an asset (SHA-256, size); host and URL are pinned (UPD-001, UPD-010).
- **Same-user malware:** already runs as the user; it can edit the AppImage, `~/.local/bin` or the app data. The design only makes sure the updater is not an *extra* way to get more: no execution of an unchecked file, the Windows installer is locked between check and run, the Linux swap needs the signed hash first, and `APPIMAGE` must be an AppImage under `APPDIR`.
- **Replay or freeze:** an old signed manifest can be replayed. It can't downgrade anyone (only versions newer than the running one are offered) and a restored offline offer expires after 14 days. A signed manifest has no revocation: a bad release is fixed by publishing a newer one, and the offer follows the latest manifest.
- **Key compromise or loss:** a leaked signing key lets an attacker sign a manifest, so the remedy is a new release with a rotated key set (`next` becomes `current`); copies that only know the compromised key must be updated by hand. If both private keys are lost, installed copies can't update themselves (see `keys/README.md`). Custody follows signing.md Phase 4: the maintainer generates and stores the keys; no agent holds a private key.
- **Frozen formats:** the manifest `schema`/`kind` values, the `<os>-<arch>` platform keys and asset names (REL-012), the output of `--version` (`dockering <semver>`, exit 0: an old binary tests a new one with it), the embedded key list, the `--update-relaunch=1` flag and the stdin-EOF handshake are contracts between an old binary and a new one from the first updater release on; changing them needs a new version of the contract and the old behaviour kept for a release.
- **A leaked key alone doesn't deliver code:** asset URLs are pinned to this project's GitHub release for the manifest's own version (UPD-001), so an attacker also needs write access to the release assets or the repository.

## Alternatives rejected

| Alternative | Why |
|---|---|
| Linux-only inclusion switch | Repeats the signing/discovery coupling; Windows and macOS would stay dark. |
| Require OS signing before any updater | Blocks everything on credentials that are not available. |
| `AppImageUpdate`/zsync, Sparkle, `electron-updater`-style plugin layer | A second trust root and extra binaries for one small strategy each. |
| A recovery journal and a successor-acknowledgement protocol | The offer file already survives a cancelled or failed install, and the running version catching up is the acknowledgement. |
| PID + process-birth + nonce handoff | A piped stdin gives the same guarantee in safe Rust, with no PID reuse and no `/proc` parsing. |
| Keeping the previous AppImage | No UI or restore path; the self-test before the swap is the safety net, and every release stays downloadable. |

## Consequences

The update signing key becomes release-critical (custody, rotation, `verify-update-artifacts` requiring the *current* key).
`apply_update()` and `update_status()` change shape (`ApplyOutcome`, `UpdateSnapshot`). The first updater release is
installed by hand, and a live proof needs a second release. Windows behaviour beyond the existing installer smoke tests
is a spike (W1) and is not claimed. This ADR authorises no keys, release or changelog entry: those stay with the maintainer and the `/release` skill.

One exception to "no `unsafe` in the AppImage strategy" was found by the end-to-end test and accepted (2026-10-20): the child of the swap marks the descriptors it inherits close-on-exec in a `pre_exec` closure (`dk-update/src/apply/appimage/fds.rs`, a raw `close_range` or `fcntl` loop, no allocation), because the AppImage runtime keeps an fd on its mount directory without `CLOEXEC` and the old mount (and its FUSE daemon) would otherwise outlive the update. It adds `libc` as a Linux-only dependency of `dk-update`. The environment scrub itself stays free of `unsafe` (it is set on the `Command`).
