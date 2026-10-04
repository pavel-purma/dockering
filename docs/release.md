# Releasing Dockering

The repository is public. The first release is **0.1.0**, with unsigned downloads for all six
supported targets. Code signing, macOS notarization, the updater, and winget are deferred.
The workflow always creates a **draft**; publication is a separate maintainer action.
See [the specification](spec/features/distribution.md) and
[first-release plan](plan/features/unsigned-first-release.md).

## First-release setup

The current GitHub account has ADMIN access and Actions are enabled. The manual-tag flow
needs no signing secrets, repository variables, release-bot App, or existing release.
Keep `PUBLIC_RELEASES` and `WINGET_ENABLED` unset/false and `WINDOWS_SIGNING` unset/`none`.

| Platform | Runner | Required downloads |
|---|---|---|
| Windows x64 | `windows-2025` | `Dockering-Setup-x64.exe`, `Dockering-x64.zip` |
| Windows ARM64 | `windows-11-arm` | `Dockering-Setup-arm64.exe`, `Dockering-arm64.zip` |
| macOS Apple silicon | `macos-15` | `Dockering-aarch64.dmg` |
| macOS Intel | `macos-15-intel` | `Dockering-x86_64.dmg` |
| Linux x64 | `ubuntu-24.04` | `Dockering-x86_64.AppImage`, `Dockering-x86_64.deb`, `Dockering-x86_64.tar.gz` |
| Linux ARM64 | `ubuntu-24.04-arm` | `Dockering-aarch64.AppImage`, `Dockering-aarch64.deb`, `Dockering-aarch64.tar.gz` |

All 12 distribution files must exist and be nonempty. Assembly rejects duplicate names.
The release also includes `dockering-update.json`, `SHA256SUMS`, and `RELEASE_NOTES.md`.
The unsigned manifest is informational; the app is built without the updater. Public GitHub
builds generate provenance attestations independently of code signing. macOS packaging retries
the observed busy DMG eject failure at most three times, with cleanup restricted to its own image. Packages bundle the
MIT license and third-party notices.

## Execute after approval

Preparation is local. **Wait for the user's approval before pushing preparation changes,
creating a release tag, dispatching release builds, or publishing.** After approval:

1. Commit the prepared changes, open a PR, and merge after CI passes. Set the `0.1.0`
   changelog date to the actual release date before merging. Register the PR with the thread.
2. Confirm CI on the exact release commit is green for all six targets. Follow
   [the release checklist](plan/release-checklist.md).
3. Create and push the annotated tag on the approved `main` commit:

   ```sh
   git switch main
   git pull --ff-only origin main
   git tag -a v0.1.0 -m "Dockering 0.1.0 (unsigned)"
   git push origin v0.1.0
   ```

4. Find and monitor the tag's workflow, then inspect its draft:

   ```sh
   gh run list --workflow release.yml --branch v0.1.0
   gh run watch <run-id> --exit-status
   gh release view v0.1.0 --json isDraft,isPrerelease,assets,url
   ```

   Confirm all 12 distributions and three metadata files are present. A failed matrix blocks
   assembly. Fix the cause and rerun failed jobs; do not publish a partial release.
5. Download the draft and verify checksums. On Linux:

   ```sh
   gh release download v0.1.0 --dir target/release-review/v0.1.0
   cd target/release-review/v0.1.0
   sha256sum --check SHA256SUMS
   ```

   On macOS use `shasum -a 256 -c SHA256SUMS`. On Windows, from the repository root:

   ```powershell
   gh release download v0.1.0 --dir target/release-review/v0.1.0
   $reviewDir = Join-Path (Get-Location) 'target/release-review/v0.1.0'
   foreach ($line in Get-Content (Join-Path $reviewDir 'SHA256SUMS')) {
       if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { throw "Invalid checksum line: $line" }
       $expectedHash = $Matches[1]
       $assetName = $Matches[2]
       $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $reviewDir $assetName)).Hash
       if ($actualHash.ToLowerInvariant() -ne $expectedHash) { throw "Checksum mismatch: $assetName" }
   }
   ```

   Verify provenance for a downloaded installer (run from the repository root):

   ```sh
   gh attestation verify target/release-review/v0.1.0/Dockering-Setup-x64.exe -R pavel-purma/dockering
   ```

6. Review the notes and available smoke tests, then publish the existing draft:

   ```sh
   gh release edit v0.1.0 --draft=false --latest
   gh release view v0.1.0 --json isDraft,isPrerelease,url,assets
   ```

   Verify the published assets and permanent download links. Never rerun uploads against a
   published release; publish a new version for corrections. The release URL will be
   `https://github.com/pavel-purma/dockering/releases/tag/v0.1.0`.

## Dry runs and retries

After the prepared workflow exists on the default branch, manual dispatch on a branch builds
all six targets and assembles a review artifact without creating a tag or GitHub Release:

```sh
gh workflow run release.yml --ref main -f unsigned=true
```

The review bundle has the same distributions, metadata, and notes as a tag release. Its manifest
URLs describe the eventual version tag and resolve only after publication. Dispatching on an
existing version tag creates or resumes its draft:

```sh
gh workflow run release.yml --ref v0.1.0 -f unsigned=true
```

The `unsigned` input defaults to true and skips signing/notarization even if secrets exist.
Tag pushes select unsigned mode while `PUBLIC_RELEASES` is not true. Tags must match the workspace
version, have a changelog section, and be contained in `main` or a `release/*` branch.

## Unsigned installation

- **Windows:** SmartScreen may show a prompt. For a verified download, use *More info → Run
  anyway* where local policy permits it. Installers support per-user, `/ALLUSERS`, and silent
  installation; CI tests installation and uninstallation for both architectures.
- **macOS:** DMGs are not Developer ID signed or notarized. Gatekeeper may block the application.
  After checking the download, use *System Settings → Privacy & Security → Open Anyway* where
  macOS and local policy permit it.
- **Linux:** AppImages need executable permission (`chmod +x Dockering-*.AppImage`) and may need
  FUSE support. `.deb` and `.tar.gz` are alternatives. Builds use Ubuntu 24.04 and need compatible
  runtime libraries, Wayland/X11, and a Vulkan driver.

Minimum systems: Windows 10 22H2, macOS 15, and compatible Linux with Vulkan. Updates for this
unsigned release are manual downloads from GitHub Releases.

## Future release-plz automation

The release bot is optional for this first release. To automate later release PRs and tags:

1. Create a GitHub App installed only here, with **Contents** and **Pull requests** read/write
   permissions and no webhook.
2. Add repository secrets `RELEASE_BOT_APP_ID` (App ID or Client ID) and
   `RELEASE_BOT_PRIVATE_KEY` (full PEM).
3. release-plz keeps one `chore(release): vX.Y.Z` PR open. Merge after reviewing its version,
   changelog, and checklist. The App token creates the tag and triggers the release workflow.

Without the secrets, release-plz skips its jobs; manual tags work. `GITHUB_TOKEN` cannot trigger
other workflows by pushing tags. Application crates share a version; nothing goes to crates.io.
Conventional `feat:` commits bump minor while in `0.x`; fixes normally bump patch. For prereleases,
edit the workspace version and changelog to e.g. `0.2.0-rc.1` and refresh workspace lockfile versions.
Prereleases do not become latest stable. Use `main` or `release/X.Y` for hotfixes.

## Future signed releases

Configure signing secrets in the `release` environment and appropriate maintainer protection.
Signing secrets reach only the signing steps. `WINDOWS_SIGNING` selects:

| Value | Requirements |
|---|---|
| `none` | Unsigned; valid for this first release. |
| `signpath` | `SIGNPATH_API_TOKEN`; variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`; `exe`/`installer` artifact configurations; `release-signing` policy. |
| `azure` | `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`. |

Set `WINDOWS_SIGNER_SUBJECT` to pin the expected publisher. Windows binaries are signed before
packaging, then installers are signed and verified. Inno's generated uninstaller needs a local
compile-time `DOCKERING_INNO_SIGNTOOL`; remote signing alone does not cover it.

macOS requires `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_API_KEY`, `APPLE_API_ISSUER`, and `APPLE_API_KEY_P8` for signing and notarization.

Generate updater keys with `cargo xtask gen-update-keys <private-directory>`, store the current
private key as `UPDATE_SIGNING_KEY`, and commit reviewed current/next **public** keys to
`crates/dk-update/src/keys.rs`. Optional password: `UPDATE_SIGNING_KEY_PASSWORD`.
See [key rotation](../crates/dk-update/keys/README.md). Never commit private keys.

Only then set `PUBLIC_RELEASES=true`. Its historical name now means the signed/updater channel,
not repository visibility. Manual `unsigned=true` overrides it. Signed mode checks configuration
before builds and preserves signing guards. Public build provenance is independent of this setting.

Submit the first signed winget version manually, configure `WINGET_TOKEN`, then set
`WINGET_ENABLED=true`. The workflow also requires published stable release metadata with a
signed update manifest, so an explicit unsigned override is skipped; see [winget setup](../packaging/winget/README.md).
