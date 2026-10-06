# Signing setup (agent procedure)

`/release signing` and the signing part of `/release status` use this file. The human-readable
guide is [`docs/signing.md`](../../../docs/signing.md); read it first. Don't repeat its text to the
maintainer: tell them the next phase, what they must do, and what you do.

## Detect the phase

Run these (read-only) and decide:

```sh
gh variable list                         # WINDOWS_SIGNING, MACOS_SIGNING, SIGNPATH_*, WINDOWS_SIGNER_SUBJECT, PUBLIC_RELEASES
gh secret list --env release             # names only: SIGNPATH_API_TOKEN, UPDATE_SIGNING_KEY, APPLE_*
git show origin/main:crates/dk-update/src/keys.rs
gh run list --workflow release.yml --limit 5 --json event,headBranch,conclusion,createdAt,displayTitle
```

| Observed | Phase to do next |
|---|---|
| no `SIGNPATH_*` variables and no `SIGNPATH_API_TOKEN` | **1–2**: the maintainer applies to SignPath and configures it; offer to draft the application answers |
| SignPath configured by the maintainer, variables or token missing | **3** |
| `UPDATE_SIGNING_KEY` missing or `PUBLIC_KEYS` empty | **4** |
| 3 and 4 done, no successful `release.yml` dispatch with `unsigned=false` | **5** |
| rehearsal done, `WINDOWS_SIGNER_SUBJECT` unset | **6** |
| everything set, `PUBLIC_RELEASES` unset | **7** (maintainer's decision) |
| `PUBLIC_RELEASES=true` | done: report the channel |

Ask the maintainer which of phases 1–2 they have completed; you can't see SignPath.

## What you may do

- **Phase 3:** `gh variable set` for the three variables, with values the maintainer gives you. Never ask for or accept the API token in the chat. Give them the exact command `gh secret set SIGNPATH_API_TOKEN --env release` to run in their own terminal, and verify with `gh secret list --env release` that the name appears.
- **Phase 4:** see docs/signing.md. Create a temporary directory outside the repository (for example under the OS temp directory). `cargo xtask gen-update-keys <dir>`. Set the secret with `gh secret set UPDATE_SIGNING_KEY --env release < <dir>/current.key`. Do not `cat`, `Read` or print any `*.key` file, and do not echo the xtask output that lists the secret key path contents (it prints only public keys). Edit `crates/dk-update/src/keys.rs` through a PR (branch, `cargo xtask` tests including `cargo test -p dk-update`, conventional title, CI green, squash merge). Tell the maintainer to copy `next.key` into their password manager now; wait for their confirmation, then delete the directory and check that it is gone.
- **Phase 5:** `gh workflow run release.yml --ref main -f unsigned=false` (about 35 minutes; no tag, no release). Tell the maintainer to approve the four SignPath requests, watch the run, download the artifact (`gh run download <id> -n release-review-<version> -D <dir>`), run `verify-release.ps1 -Expect Signed` and `cargo xtask verify-manifest`, and report the signer subject and issuer exactly as printed. If a signing step fails, read its log: the usual causes are a wrong organization id or project slug, a policy that refuses branch builds (origin verification), a missing artifact configuration slug, and a `product-version` mismatch.
- **Phase 6:** give the maintainer the `gh variable set WINDOWS_SIGNER_SUBJECT` command with the observed subject, or set it after they confirm the string.
- **Phase 7:** never set `PUBLIC_RELEASES` yourself; it is the maintainer's decision. After it is set and the first signed release is published, update the README (Download notice, *Updates*, *Code signing policy*), `docs/release.md` and the spec status lines in the record PR.

## What you never do

Contact SignPath or Apple on the maintainer's behalf; approve a signing request; accept, display, log or store a token, certificate or private key; commit a key; change signing variables while a release is in flight; mark a phase done without observing it.
