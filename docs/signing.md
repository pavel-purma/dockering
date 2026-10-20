# Code signing

What Dockering's downloads are signed with, what is prepared for signing them later, and what to
do when that changes. Requirements: [REL-030…032, REL-034, UPD-003](spec/features/distribution.md#3-code-signing-rel-030034).
Running a release: [release.md](release.md) or the `/release` skill.

## Status

**Every release is unsigned.** No Windows certificate provider and no Apple Developer ID are set up,
so Windows files carry no Authenticode signature and macOS files are neither signed nor notarised
(REL-016). `WINDOWS_SIGNING` and `MACOS_SIGNING` default to `none`, and `PUBLIC_RELEASES` is unset.
The release notes say so, and users see the SmartScreen and Gatekeeper prompts described in
[release.md](release.md#unsigned-installation).

The release pipeline keeps the hooks for signing later, so adding a provider is configuration, not a
rewrite. They stay off until the phases below are done. Azure Artifact Signing is the Windows
provider the workflow supports (`WINDOWS_SIGNING=azure`); Apple Developer ID is the macOS one
(`MACOS_SIGNING=apple`). Nothing in the app or the updater requires an OS signature (UPD-003).

## What is signed

| Artifact | Signature | Signer | Verified by |
|---|---|---|---|
| `dockering.exe` (Windows x64, ARM64) | none today. In the OS-signed channel: Authenticode, SHA-256, RFC 3161 timestamp | the configured provider (Azure Artifact Signing) | `release.yml`, `scripts/verify-release.ps1`, the app's updater (UPD-003) |
| `Dockering-Setup-<arch>.exe` | the same | the same | the same |
| `dockering-update.json` | minisign (Ed25519) → `dockering-update.json.minisig`, whenever `sign_manifest` is true: `UPDATER=all` **or** an OS-signed build (REL-083) | the Dockering update key (the **first** key of `PUBLIC_KEYS`) | `cargo xtask verify-update-artifacts` (REL-084; `verify-manifest` is its narrower form), the app |
| every release file | GitHub build-provenance attestation | GitHub Actions | `gh attestation verify` |
| macOS DMGs | none today. With `MACOS_SIGNING=apple`: Developer ID + notarisation (REL-034) | not set up | `codesign`, `spctl` |
| Portable zips | none; in the OS-signed channel the `dockering.exe` inside is signed, the zip itself is not | | `SHA256SUMS`, attestations |
| Linux files | none | | `SHA256SUMS`, attestations |

The Inno-generated uninstaller (`unins000.exe`) is not signed in the OS-signed channel either: remote
signing can't reach it (known gap in REL-030).

## Who would sign

**Azure Artifact Signing** keeps the key in Azure; Dockering never holds a certificate or private key
for Windows. It signs without a per-request approval, but needs a paid Azure subscription, and files
show the maintainer's own validated name as the publisher. Individual developers qualify only in the
US and Canada.

A trusted certificate cannot be generated locally: Windows trusts only certificates issued by a
certificate authority after it validated an identity. A self-signed one gives the same "unknown
publisher" warning as no signature.

## Credentials

Three separate credentials are involved once a provider is set up:

| Credential | Held by | Used for |
|---|---|---|
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` (secrets in the `release` environment) | you, from Azure | the Azure signing steps of `release.yml`; they reach only those steps (REL-013) |
| `UPDATE_SIGNING_KEY` (+ optional `UPDATE_SIGNING_KEY_PASSWORD`, secret in the `release` environment) | you | the update-manifest signature (UPD-003); independent of OS signing |
| your own `gh` login | you | what `/release` uses to open PRs, push tags and publish, as you |

- The **release-bot App** in `release-plz.yml` is the dormant bot flow. It isn't created, isn't
  needed for signing or for `/release`, and shouldn't be enabled next to the skill ([release.md](release.md#release-plz-dormant)).
- Only collaborators can open pull requests here, so `/release` runs as you. Its PRs and tags are
  yours, which also means the tag push starts `release.yml` (a push made with `GITHUB_TOKEN` would not).

## One-time setup

`/release signing` detects which phase is next and guides you through it. Phases 1, 2, 6 and 7 are
yours; phases 3–5 an agent can do with you, but no secret value ever passes through a chat.
Nothing here is needed to ship unsigned releases, and the update-manifest key (phase 4) is the only
phase the updater needs.

### Phase 1: Azure account and validated identity (you)

1. Turn on two-factor authentication for your GitHub account (Settings → Password and authentication) and keep it on.
2. In an Azure subscription, create an Artifact Signing account and complete its identity validation.

### Phase 2: certificate profile and service principal (you)

1. Create a certificate profile for the validated identity.
2. Create a service principal that is allowed to sign with that profile, and note the six values the workflow needs (see *Credentials*).

### Phase 3: repository variable and secrets (you + agent)

Variables aren't secret; an agent may set them. The secrets you set yourself:

```sh
gh variable set WINDOWS_SIGNING --body azure
gh secret set AZURE_TENANT_ID --env release          # paste the value at the prompt; do this yourself
gh secret set AZURE_CLIENT_ID --env release
gh secret set AZURE_CLIENT_SECRET --env release
gh secret set AZURE_SIGNING_ENDPOINT --env release
gh secret set AZURE_SIGNING_ACCOUNT --env release
gh secret set AZURE_CERT_PROFILE --env release
```

`PUBLIC_RELEASES` stays unset until phase 7: a branch rehearsal needs no tag, and tags stay unsigned
until then.

### Phase 4: update-manifest key (you; the agent only receives the public keys)

The updater accepts only a manifest signed with a key whose public half is compiled into the app
(UPD-003). Until then `PUBLIC_KEYS` in `crates/dk-update/src/keys.rs` is empty and updates fail
closed. This phase needs no OS code signing. No agent generates, sees or stores a private key, so **you**:

1. run `cargo xtask gen-update-keys <temporary directory outside the repo>` (it writes `current.key`, `next.key` and the `.pub` files, and prints only the public keys; the secret files are created with mode 0600 on Unix, and the command refuses to overwrite an existing key);
2. set the secret straight from the file, so the key never appears in a command line or a chat: `gh secret set UPDATE_SIGNING_KEY --env release < <dir>/current.key`;
3. store `next.key` offline in a password manager (it is the rotation key: if both private keys are lost, installed copies can't update themselves again), then delete the temporary directory;
4. give the agent the two **public** key lines (`current` first); it puts them into `keys.rs` in a reviewed PR titled `chore(update): add the update manifest public keys [UPD-003]`. Updater inclusion is decided by `UPDATER=none|all` ([plan](plan/features/linux-appimage-updates.md), REL-083), not by OS signing, and the manifest signature is mandatory even for OS-unsigned binaries.

### Phase 5: rehearsal (agent + you)

Nothing is tagged or published:

```sh
gh workflow run release.yml --ref main -f unsigned=false
```

The run builds all six targets and signs the two Windows files of each Windows job through Azure.
The updater is its own switch (REL-083, implemented in `release.yml`), independent of OS signing: the run's
`update channel:` notice and its `updater` output say what it built. This `-f unsigned=false` rehearsal builds the
updater only when the `UPDATER` variable is `all`, unless you add `-f updater=none|all` (a branch run's input wins). A rehearsal
from `main` with `-f updater=all` builds the updater into all six targets whether or not OS signing is used, and
`-f unsigned=true -f updater=all` needs no signing provider at all; that run, with the update key from Phase 4, is the rehearsal
required before you set `UPDATER=all`. Because it signs the manifest of the *next* version with the production
key, its review artifact is kept 7 days. The updater's behaviour in the app (title-bar button, in-app AppImage
install, retained offer) is still planned: don't set `UPDATER=all` until it lands.
Then the agent downloads the `release-review-<version>`
artifact and runs, with `-Expect` for the OS signing mode of that run and `-Updater` for its `updater` output:

```sh
pwsh -NoProfile -File scripts/verify-release.ps1 -Dir <dir> -Version <version> -Expect Signed|Unsigned -Updater none|all
cargo xtask verify-update-artifacts --assets <dir> --version <version> --updater none|all
```

`verify-release.ps1` has no cargo and no minisign: it checks the file set (the `.minisig` is required when
`-Updater all` or `-Expect Signed`, forbidden otherwise) and the notes' update sentence. `verify-update-artifacts` is
the other half and runs next to it: the `.minisig` must verify against the keys in `keys.rs` **and** have been made by the
first (current) key, so a `UPDATE_SIGNING_KEY` set to `next.key` fails with "the secret `UPDATE_SIGNING_KEY` is not the
current key"; the manifest `version` must be `<version>`; and every one of the six platform entries must equal its
asset's kind, URL, size and SHA-256. The `updater-evidence-<version>` artifact of the run lists, per target, whether the
unpackaged binary contains the updater (`updater=`) and the test seam (`seam=`, always `false`).

In a signed rehearsal the verifier reports the signer **subject** and **issuer** it observed. Nothing downloaded from a rehearsal is published.

### Phase 6: pin the signer (you)

Set the observed subject exactly (the `release.yml` check compares the whole subject string):

```sh
gh variable set WINDOWS_SIGNER_SUBJECT --body "<subject from the rehearsal>"
```

A tag run without it fails in `verify` (REL-032), so a certificate swap can't pass unnoticed.

### Phase 7: switch the channel on (you)

```sh
gh variable set PUBLIC_RELEASES --body true
```

`PUBLIC_RELEASES` controls OS signing only (REL-083, implemented in `release.yml`): the next `/release`
creates the first OS-signed release, with the updater as the `UPDATER` variable says. `UPDATER=all` separately
turns the updater on everywhere (after the REL-060/084/087 checks, and not before the app side lands: it is planned). An OS-unsigned build with the updater
still has a signed manifest; an unsigned Windows installer installs on manifest and hash trust, a signed
Windows exe also pins the installer's subject and issuer, and macOS offers *Download*.
Developer ID/hardened runtime/notarization and native bundle replacement gate macOS install,
never discovery on other targets. The notes of an OS-signed release say *Signed release* and, while `MACOS_SIGNING` is `none`,
that macOS files aren't signed. That release's PR (not the record PR after it) also rewrites the README's
*Download* notice and *Updates* paragraph, which say the downloads are unsigned. The first signed release is
also when to submit the first winget version by hand (`packaging/winget/README.md`).

## Each release

Nothing to do while releases are unsigned. Once Azure signing is on, the tag run signs without a
manual step.

## Verifying a download (users)

```powershell
Get-AuthenticodeSignature .\Dockering-Setup-x64.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
gh attestation verify .\Dockering-Setup-x64.exe -R pavel-purma/dockering
```

Today `Status` is `NotSigned` (REL-016); the attestation and `SHA256SUMS` are the proof of origin. In the
OS-signed channel `Status` must be `Valid`. A new signature doesn't remove SmartScreen warnings at once:
reputation builds with downloads of that signed file.

## Operations

- **Certificate renewal.** When the signing certificate is renewed or replaced, check that `verify-release.ps1` shows the same subject and issuer as the previous release. The updater accepts an installer only when its signer's subject **and issuer** equal those of the running `dockering.exe` (`dk-update/src/authenticode.rs`). If either changed, installed copies reject the update: say so in the release notes, tell users to reinstall, and update `WINDOWS_SIGNER_SUBJECT` before the tag.
- **Rotating the update key.** `crates/dk-update/keys/README.md`. Installed builds trust the *current* and the *next* key, so rotate to `next` before `current` is lost or exposed. The release gate (REL-084) requires the manifest to be signed by the **first** key of `PUBLIC_KEYS`: change the secret and the order of `PUBLIC_KEYS` (`[next, new]`) in the same release, or `cargo xtask verify-update-artifacts` fails.
- **Key or secret exposure.** Revoke the Azure client secret and create another. If the update key leaked, rotate it at once (the old public keys stay compiled into old builds, so also publish a release signed with `next`) and treat releases signed since the exposure as suspect.
- **Signing service unavailable or the request rejected.** The tag run fails at the signing step and no draft is created. Rerun the failed Windows jobs after the service is back. Only in an emergency, and only with the maintainer's explicit agreement, dispatch the tag with `-f unsigned=true` to ship unsigned and say so in the notes: such a release is not OS-signed, so users must download it by hand (a signed, installed Windows copy refuses an unsigned installer and offers *Download* instead; with `UPDATER=all` the manifest is still signed, and unsigned installs update normally).
- **Changing the signer.** When the certificate or the provider changes, set `WINDOWS_SIGNING`, its variables and secrets, and `WINDOWS_SIGNER_SUBJECT` to the new signer in one step *before* a release. Installed copies pin the old signer, so they reject the update to the first release signed by the new one; treat it like a certificate change above.
