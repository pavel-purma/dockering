# Signing setup (agent procedure)

`/release signing` and the signing part of `/release status` use this file. It describes the current
behaviour only. The human-readable
guide is [`docs/signing.md`](../../../docs/signing.md); read it first. Don't repeat its text to the
maintainer: tell them the next phase, what they must do, and what you do.

## Detect the phase

Run these (read-only) and decide:

```sh
gh variable list                         # WINDOWS_SIGNING, MACOS_SIGNING, WINDOWS_SIGNER_SUBJECT, PUBLIC_RELEASES
gh secret list --env release             # names only: AZURE_*, UPDATE_SIGNING_KEY, APPLE_*
git show origin/main:crates/dk-update/src/keys.rs
gh run list --workflow release.yml --limit 5 --json event,headBranch,conclusion,createdAt,displayTitle
```

| Observed | Phase to do next |
|---|---|
| no `WINDOWS_SIGNING` variable and no `AZURE_*` secrets | **1–2**: the maintainer sets up Azure Artifact Signing (account, validated identity, certificate profile, service principal) |
| Azure set up by the maintainer, variable or secrets missing | **3** |
| `UPDATE_SIGNING_KEY` missing or `PUBLIC_KEYS` empty | **4** |
| 3 and 4 done, no successful `release.yml` dispatch with `unsigned=false` | **5** |
| rehearsal done, `WINDOWS_SIGNER_SUBJECT` unset | **6** |
| everything set, `PUBLIC_RELEASES` unset | **7** (maintainer's decision) |
| `PUBLIC_RELEASES=true` | done: report the channel |

When nothing is set, the repository is in its normal state: releases are unsigned and no phase is pending. Say so, and
start phase 1 only if the maintainer asks for signing. Ask the maintainer which of phases 1–2 they have completed; you can't see Azure.

## What you may do

- **Phase 3:** `gh variable set WINDOWS_SIGNING --body azure`. Never ask for or accept an Azure value in the chat. Give them the exact `gh secret set AZURE_… --env release` commands from docs/signing.md to run in their own terminal, and verify with `gh secret list --env release` that the six names appear.
- **Phase 4:** see docs/signing.md. The **maintainer** runs `cargo xtask gen-update-keys <dir>` and `gh secret set UPDATE_SIGNING_KEY --env release < <dir>/current.key` in their own terminal and keeps `next.key` offline; you never run `gen-update-keys`, and you never read, print or store a `*.key` file. Verify with `gh secret list --env release` that the name appears, ask the maintainer for the two public key lines (`current` first), and edit `crates/dk-update/src/keys.rs` through a PR (branch, `cargo xtask` tests including `cargo test -p dk-update`, conventional title, CI green, squash merge).
- **Phase 5:** `gh workflow run release.yml --ref main -f unsigned=false` (no tag, no release; the updater follows the `UPDATER` variable unless you add `-f updater=none|all`). Watch the run, download the artifact (`gh run download <id> -n release-review-<version> -D <dir>`), and run `verify-release.ps1 -Expect Signed -Updater <none|all>` together with `cargo xtask verify-update-artifacts --assets <dir> --version <version> --updater <none|all>` (the channel is the run's `updater` output, shown in its `update channel:` notice; a manifest signed by the rotation key fails), then report the signer subject and issuer exactly as printed. A rehearsal that signs the manifest signs the **next** version with the production key: the review artifact is kept 7 days. If a signing step fails, read its log: the usual causes are a wrong endpoint, account or certificate profile name, and a service principal without the right to sign.
- **Phase 6:** give the maintainer the `gh variable set WINDOWS_SIGNER_SUBJECT` command with the observed subject, or set it after they confirm the string.
- **Phase 7:** never set `PUBLIC_RELEASES` yourself; it is the maintainer's decision. After it is set and the first signed release is published, update the README (Download notice and *Updates*), `docs/release.md` and the spec status lines in the record PR.

## What you never do

Contact Azure or Apple on the maintainer's behalf; accept, display, log or store a token, certificate or private key; commit a key; change signing variables while a release is in flight; mark a phase done without observing it.
