# Updater signing keys (UPD-003)

The app only trusts an update manifest (`dockering-update.json`) whose minisign signature checks
out against one of the public keys in [`src/keys.rs`](../src/keys.rs). If that list is empty, every
manifest is rejected (fail closed).

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
