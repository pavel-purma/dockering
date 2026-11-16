# Updater signing keys (UPD-003)

If [`src/keys.rs`](../src/keys.rs) lists public keys, the app only trusts an update manifest
(`dockering-update.json`) whose minisign signature checks out against one of them. While that list is
empty the updater runs on the **unsigned channel** (ADR-0007, 2026-11-16): the manifest is accepted as
served over HTTPS from the project's GitHub release, and each asset's size and SHA-256 are checked.
Adding a key switches every later build to mandatory signatures.

## One-time setup

```sh
cargo xtask gen-update-keys <dir outside the repo>
```

This writes `current.key`/`current.pub` and `next.key`/`next.pub` and prints both public-key lines.

1. Paste both lines into `PUBLIC_KEYS` in `src/keys.rs`, `current` first.
2. Store the full contents of `current.key` as the secret `UPDATE_SIGNING_KEY` in the GitHub
   environment `release`. The keys are unencrypted, so the environment and its required reviewers
   are what protect them.
3. Keep `next.key` offline (for example in a password manager). It is never used in CI until a
   rotation.
4. Delete the key files from disk.

## Rotation

Builds that are already installed trust both keys. To rotate:

1. Change `UPDATE_SIGNING_KEY` to the contents of `next.key`.
2. Generate a fresh pair and replace `PUBLIC_KEYS` with `[next, new]`.
3. Release. Old builds verify the manifest with `next`, new builds have the new key ready.

If **both** private keys are lost, installed builds can no longer update themselves. Users then
have to install a new version by hand (winget or the installer).
