# Code signing

How Dockering's downloads get signed, who does what, and what to do when something changes.
Requirements: [REL-030…034, UPD-003](spec/features/distribution.md#3-code-signing-rel-030034).
Running a release: [release.md](release.md) or the `/release` skill.

## What is signed

| Artifact | Signature | Signer | Verified by |
|---|---|---|---|
| `dockering.exe` (Windows x64, ARM64) | Authenticode, SHA-256, RFC 3161 timestamp | SignPath Foundation | `release.yml`, `scripts/verify-release.ps1`, the app's updater (UPD-003) |
| `Dockering-Setup-<arch>.exe` | the same | the same | the same |
| `dockering-update.json` | minisign (Ed25519) → `dockering-update.json.minisig`, whenever `sign_manifest` is true: `UPDATER=all` **or** an OS-signed build (REL-083) | the Dockering update key (the **first** key of `PUBLIC_KEYS`) | `cargo xtask verify-update-artifacts` (REL-084; `verify-manifest` is its narrower form), the app |
| every release file | GitHub build-provenance attestation | GitHub Actions | `gh attestation verify` |
| macOS DMGs | Developer ID + notarisation, only when `MACOS_SIGNING=apple` (REL-034) | not set up | `codesign`, `spctl` |
| Portable zips | the `dockering.exe` inside is signed; the zip itself is not | | `SHA256SUMS`, attestations |
| Linux files | none | | `SHA256SUMS`, attestations |

The Inno-generated uninstaller (`unins000.exe`) is not signed: remote signing can't reach it (known gap in REL-030).

## Who signs

**SignPath Foundation** gives open-source projects a code-signing certificate at no cost. The key
stays in SignPath's HSM; Dockering never holds a certificate or private key for Windows. Files show
**SignPath Foundation** as the publisher (the certificate is issued to the Foundation, which vouches
for the connection between the binary and this open-source repository). Every release needs a
manual approval by a team member in SignPath. Their terms are at <https://signpath.org/terms>. The
rules that bind this project are in the README's *Code signing policy* (REL-033):

- all components are OSI-licensed (MIT) and built from this repository by GitHub-hosted runners;
- every team member uses multi-factor authentication on SignPath and GitHub;
- non-committers' changes are reviewed by a team member (the maintainer reviews pull requests from anyone else);
- the product name is Dockering and the product version is identical in every file of a build (the artifact configurations pin both);
- SignPath may revoke the certificate or stop the subscription without notice.

Azure Artifact Signing is the supported alternative (`WINDOWS_SIGNING=azure`): it signs without
per-request approval but needs a paid Azure subscription and shows the maintainer's own validated
name; individual developers qualify only in the US and Canada.

A trusted certificate cannot be generated locally: Windows trusts only certificates issued by a
certificate authority after it validated an identity. A self-signed one gives the same "unknown
publisher" warning as no signature.

## Credentials and GitHub Apps

No GitHub App of ours takes part in signing. Three separate credentials are involved:

| Credential | Held by | Used for |
|---|---|---|
| `GITHUB_TOKEN` of the release job | GitHub, created per run, `actions: read` + `contents: read` | the SignPath action reads the job and the artifact it uploaded, so SignPath can check where the file came from |
| `SIGNPATH_API_TOKEN` (secret in the `release` environment) | you, from SignPath's CI user | the action authenticates to SignPath and submits the signing request; it can only do what that CI user's policy allows |
| your own `gh` login | you | what `/release` uses to open PRs, push tags and publish, as you |

- **SignPath** has its own GitHub App. As far as its documentation says, it is needed only for
  *audit-log* checks of repository rulesets (a stricter policy option), and for some private-repository
  permission setups. Dockering's `release-signing` policy doesn't depend on it, so don't install it
  unless SignPath asks for it. Its public GitHub listing requests broad permissions: `contents`,
  `actions` and `metadata` read, but also `administration` write on a repository and
  `organization_administration` write. If you do install it, read the permission prompt first and
  limit it to this one repository.
- The **release-bot App** in `release-plz.yml` is the dormant bot flow. It isn't created, isn't
  needed for signing or for `/release`, and shouldn't be enabled next to the skill ([release.md](release.md#release-plz-dormant)).
- Only collaborators can open pull requests here, so `/release` runs as you. Its PRs and tags are
  yours, which also means the tag push starts `release.yml` (a push made with `GITHUB_TOKEN` would not).

## One-time setup

`/release signing` detects which phase is next and guides you through it. Phases 1, 2, 6 and 7 are
yours; phases 3–5 an agent can do with you, but no secret value ever passes through a chat.

### Phase 1: apply to SignPath Foundation (you)

1. Turn on two-factor authentication for your GitHub account (Settings → Password and authentication) and keep it on.
2. Open <https://signpath.org/apply> and request the Foundation's free OSS subscription. Suggested answers:
   - **Project:** Dockering, <https://github.com/pavel-purma/dockering>, MIT license.
   - **Description:** a native desktop client for Docker, Docker inside WSL distros and WSL containers (Rust, GPUI), for Windows, macOS and Linux.
   - **Release channel:** GitHub Releases; two Windows executables per release (`dockering.exe` in a zip, and the Inno Setup installer), x64 and ARM64.
   - **Build:** GitHub Actions on GitHub-hosted runners (`.github/workflows/release.yml`), on `v*` tags; the workflow, the packaging scripts and the SignPath artifact configurations (`packaging/signpath/`) are in the repository.
   - **Team:** one maintainer (author, reviewer and approver), who uses MFA on both services.
   - **Privacy:** "This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it." (Builds with the updater enabled check GitHub Releases for updates, can be turned off, and are described in the README's *Privacy & network* section; say so if asked.)
3. If the Foundation asks for the *Code signing policy* and the attribution on the project's home page before it approves, add the one sentence "Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)" to the README's *Code signing policy* section in a docs PR (the section already exists, REL-033). Otherwise it is added with the first signed release (phase 7), because it would be untrue before.
4. Wait for the Foundation's reply. Approval time isn't published. Their terms require that the project has already released the binaries in the form to be signed; 0.1.0 and 0.2.0 are public releases of the same kind of files, unsigned.
5. Turn on two-factor authentication in SignPath.

### Phase 2: configure SignPath (you)

The Foundation may create some of this for you when it approves the project; check what exists and
complete the rest. At the end the SignPath organization needs:

1. A **project** (note its slug, for example `dockering`) linked to the repository.
2. **Artifact configurations:** import `packaging/signpath/exe.xml` as slug `exe` and `packaging/signpath/installer.xml` as slug `installer` ([details](../packaging/signpath/README.md)).
3. The predefined **GitHub.com trusted build system**, linked to the project.
4. A **signing policy** with slug `release-signing` that uses the Foundation's open-source certificate, requires **manual approval** (you are the approver), and has origin verification on (it needs the trusted build system above).
5. A **CI user** and API token allowed to submit requests for `release-signing`. Copy the token for phase 3 and don't share it anywhere else.

### Phase 3: repository variables and the token (you + agent)

Variables aren't secret; an agent may set them. The token you set yourself:

```sh
gh variable set WINDOWS_SIGNING --body signpath
gh variable set SIGNPATH_ORGANIZATION_ID --body <organization id from SignPath>
gh variable set SIGNPATH_PROJECT_SLUG --body <project slug>
gh secret set SIGNPATH_API_TOKEN --env release        # paste the token at the prompt; do this yourself
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

The run builds all six targets and, for each Windows job, submits two signing requests; approve all
four in SignPath (they appear under the project's *Signing requests*, and SignPath e-mails you).
The updater is its own switch (REL-083, implemented in `release.yml`), independent of OS signing: the run's
`update channel:` notice and its `updater` output say what it built. This `-f unsigned=false` rehearsal builds the
updater only when the `UPDATER` variable is `all`, unless you add `-f updater=none|all` (a branch run's input wins). A rehearsal
from `main` with `-f updater=all` builds the updater into all six targets whether or not OS signing is used, and
`-f unsigned=true -f updater=all` needs no SignPath at all; that run, with the update key from Phase 4, is the rehearsal
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

It reports the signer **subject** and **issuer** it observed. Nothing downloaded from a rehearsal is published.

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
never discovery on other targets. The notes of an OS-signed release say *Signed release* with the SignPath attribution and, while `MACOS_SIGNING` is `none`,
that macOS files aren't signed. Because those notes link to the README's *Code signing policy*,
that release's PR (not the record PR after it) also rewrites the README: the Download notice, the
*Updates* paragraph and the status paragraph of *Code signing policy*, which gains the attribution
sentence SignPath requires. The first signed release is also when to submit the first winget
version by hand (`packaging/winget/README.md`).

## Each release

Nothing to do except the four approvals in SignPath while the tag run is at the signing steps. The
`/release` skill tells you when. The workflow waits up to an hour for each approval.

## Verifying a download (users)

```powershell
Get-AuthenticodeSignature .\Dockering-Setup-x64.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
gh attestation verify .\Dockering-Setup-x64.exe -R pavel-purma/dockering
```

`Status` must be `Valid` and the signer `SignPath Foundation`. A new signature doesn't remove
SmartScreen warnings at once: reputation builds with downloads of that signed file.

## Operations

- **Certificate renewal.** When the signing certificate is renewed or replaced, check that `verify-release.ps1` shows the same subject and issuer as the previous release. The updater accepts an installer only when its signer's subject **and issuer** equal those of the running `dockering.exe` (`dk-update/src/authenticode.rs`). If either changed, installed copies reject the update: say so in the release notes, tell users to reinstall, and update `WINDOWS_SIGNER_SUBJECT` before the tag.
- **Rotating the update key.** `crates/dk-update/keys/README.md`. Installed builds trust the *current* and the *next* key, so rotate to `next` before `current` is lost or exposed. The release gate (REL-084) requires the manifest to be signed by the **first** key of `PUBLIC_KEYS`: change the secret and the order of `PUBLIC_KEYS` (`[next, new]`) in the same release, or `cargo xtask verify-update-artifacts` fails.
- **Key or token exposure.** Revoke the SignPath API token and create another. If the update key leaked, rotate it at once (the old public keys stay compiled into old builds, so also publish a release signed with `next`) and treat releases signed since the exposure as suspect.
- **SignPath unavailable or the request rejected.** The tag run fails at the signing step and no draft is created. Rerun the failed Windows jobs after the service is back (new requests, new approvals). Only in an emergency, and only with the maintainer's explicit agreement, dispatch the tag with `-f unsigned=true` to ship unsigned and say so in the notes: such a release is not OS-signed, so users must download it by hand (a signed, installed Windows copy refuses an unsigned installer and offers *Download* instead; with `UPDATER=all` the manifest is still signed, and unsigned installs update normally).
- **Leaving the Foundation, or switching to Azure.** Set `WINDOWS_SIGNING`, its variables and secrets, and `WINDOWS_SIGNER_SUBJECT` to the new signer in one step *before* a release. Installed copies pin the old signer, so they reject the update to the first release signed by the new one; treat it like a certificate change above.
