# Releasing Dockering

The repository is public. Releases so far are unsigned for all six supported targets. Windows code
signing through SignPath Foundation is prepared ([signing.md](signing.md)); macOS notarization, the
updater, winget, and the release bot stay deferred. The workflow always creates a **draft**;
publication is a separate maintainer action. See [the specification](spec/features/distribution.md),
the [signing guide](signing.md) and the [first-release plan](plan/features/unsigned-first-release.md).

**Releases are cut with the `/release` skill** ([`.agents/skills/release`](../.agents/skills/release/SKILL.md),
REL-018). The rest of this page is what it automates, so a maintainer can follow or repeat any step by hand.

| Release | Published | Release commit | Record |
|---|---|---|---|
| [0.2.0](https://github.com/pavel-purma/dockering/releases/tag/v0.2.0) | 2026-10-06 00:37 UTC | [`2d25f6c`](https://github.com/pavel-purma/dockering/commit/2d25f6c108f2749f7a2225b4b968742516713d23) | [checklist](plan/release-checklist.md#evidence-for-v020); a first build (`813f543`) crashed at launch after an upgrade and was withdrawn |
| [0.1.0](https://github.com/pavel-purma/dockering/releases/tag/v0.1.0) | 2026-10-04 20:09 UTC | [`041d8fa`](https://github.com/pavel-purma/dockering/commit/041d8fac06f1a2d26a61ed5e13ad16f47371f628) | [checklist](plan/release-checklist.md#evidence-for-v010) |

## Later releases

Each release gets a new workspace version and tag; never recreate a published tag or upload to a
published release.

### With the skill

```text
/release                # next version, from the merged commits
/release minor          # or patch, or an explicit 0.3.0 / 0.3.0-rc.1
/release --dry-run      # changelogs, version and local build; pushes nothing
/release status         # unreleased changes, channel, signing readiness
```

The skill runs these steps and pauses twice: before the release PR is created (it shows the version,
the reason for it and the changelog diff) and before the verified draft is published. It resumes
from whatever state it finds if a run is interrupted.

| # | Skill step | Manual equivalent below |
|---|---|---|
| 1–3 | Plan from the conventional commits since the last tag (`cargo xtask release-plan`), write both changelogs ([rules](../.agents/skills/release/changelog.md)) | **Prepare** |
| 4–5 | Local gate on a scratch worktree, bump, local release build | **Prepare** |
| 6–7 | Gate 1, release PR, wait for every check, squash-merge | **Pull request** |
| 8–9 | Annotated tag on the merge commit, watch `release.yml` (signed channel: approve the SignPath requests) | **Tag**, **Build** |
| 10–11 | `scripts/verify-release.ps1`, Gate 2, publish, check the public URLs, wait for the [Linux smoke test](#linux-smoke-test) | **Verify the draft**, **Publish** |
| 12 | Record PR: README, release table, checklist | **Record** |

`CHANGELOG.md` and `docs/spec/CHANGELOG.md` are written only here (REL-019): feature PRs never edit them.

### By hand (fallback)

1. **Prepare.** From current `main`, run `cargo xtask release-plan` for the next version (a `feat:` since the last tag
   is a minor bump while in `0.x`; fixes alone are a patch, REL-011). Set `[workspace.package] version` in
   `Cargo.toml`, run `cargo update --workspace` (only the nine workspace crates change in `Cargo.lock`), and add a
   `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md` (`release.yml` fails without it and uses it as the release
   notes) and the matching lines to `docs/spec/CHANGELOG.md`. Check with `cargo xtask release-verify --version X.Y.Z`.
   Copy the latest section of the [release checklist](plan/release-checklist.md) for the new version.
2. **Pull request.** Title `chore(release): vX.Y.Z`; squash-merge once every check is green.
3. **CI incidents.** If a job fails with "The job was not acquired by Runner of type hosted even after
   multiple attempts", it was cancelled before any step ran. Check [githubstatus.com](https://www.githubstatus.com),
   then `gh run rerun <run-id> --failed`. A job that fails *after* starting is a real failure: read its
   log and fix the cause. Do not wave a red test through as a flake without reading it.
4. **Tag** the merge commit on `main` (annotated, same message style as before), after checking that
   `main` is still that commit and the tag does not exist:

   ```sh
   git fetch origin
   git tag -a vX.Y.Z -m "Dockering X.Y.Z (unsigned)" <merge-commit>
   git push origin vX.Y.Z
   ```

5. **Build.** The tag starts `release.yml`: `gh run list --workflow release.yml --branch vX.Y.Z`, then
   `gh run watch <run-id> --exit-status`. It ends with a draft; a failed matrix blocks assembly. On the
   signed channel each Windows job waits for two SignPath approvals ([signing.md](signing.md)).
6. **Verify the draft** (`gh release download vX.Y.Z --dir target/release-review/vX.Y.Z`):

   ```sh
   pwsh -NoProfile -File scripts/verify-release.ps1 -Dir target/release-review/vX.Y.Z -Version X.Y.Z \
     -Expect Unsigned -Changelog CHANGELOG.md -Attest -Commit <release-commit>
   ```

   (`-Expect Signed` on the signed channel, plus `cargo xtask verify-manifest --assets <dir>`). It checks exactly the
   15 files (16 with `.minisig`), every `SHA256SUMS` line, the manifest's six platforms, versions, URLs, hashes and
   sizes, the notes, PE/ELF/Debian/DMG formats and architectures, that the Windows executables are GUI-subsystem,
   carry the version and are `NotSigned` (or `Valid`, timestamped, SHA-256 and one signer when signed), and that the
   portable x64 build prints the version. `-Attest` runs `gh attestation verify` for every file, pinned with
   `--source-digest <release-commit> --source-ref refs/tags/vX.Y.Z --signer-workflow pavel-purma/dockering/.github/workflows/release.yml --deny-self-hosted-runners`.

   **Start the application from the downloaded files.** Every check above passed for the first v0.2.0
   build, which crashed at launch after an upgrade. `-LaunchSmoke` does it: it backs up `state.json` and
   `config.toml` (under `%APPDATA%\dockering\Dockering`), seeds `updates.last_run_version` with an older version,
   runs the downloaded portable `dockering.exe`, requires that it stays running for several seconds, shows a window,
   closes with exit code 0 and leaves no `crash-*.txt` next to `state.json`, repeats that for `--demo`, and restores
   the backed-up files. Crash reports and the log are in `%APPDATA%\dockering\Dockering\data` and
   `%LOCALAPPDATA%\dockering\Dockering\data\logs`. Run it only with Dockering closed.
7. **Publish** after approval, once the draft passes:

   ```sh
   gh release edit vX.Y.Z --title "Dockering X.Y.Z (unsigned)" --draft=false --latest
   ```

   Then `verify-release.ps1 ... -Published`: anonymous HTTP 200 for the 15 version-pinned and 12 `latest` asset
   URLs and the latest-release API. Publishing also starts the `winget` workflow, which must be skipped while
   `PUBLIC_RELEASES` and `WINGET_ENABLED` are unset. It starts the [Linux smoke test](#linux-smoke-test) as well:
   wait for it and record its run and result in the checklist.
8. **If a published build is broken,** take it down first (`gh release delete vX.Y.Z --yes`; the tag stays),
   fix `main` through a PR whose test fails without the fix, and ship the next patch version. Reusing the
   version is an exception that needs the maintainer's say-so, as for 0.2.0: check that no release
   exists for the tag, then `git tag -f -a vX.Y.Z -m "Dockering X.Y.Z (unsigned)" <fix-commit>` and
   `git push origin vX.Y.Z --force-with-lease=refs/tags/vX.Y.Z:<old-tag-object>`, which starts a new tag run.
   Say in the changelog section and the checklist that a build was withdrawn. The skill never does this on its own.
9. **Record.** In a docs PR: update the README download text and links, complete the checklist's
   evidence section, and reconcile the spec status lines. Leave unperformed manual checks unchecked.

## Linux smoke test

`release.yml` builds and inspects the Linux packages but does not run them. The `Linux smoke` workflow
(`.github/workflows/linux-smoke.yml`; [REL-070…077](spec/features/distribution.md#8-linux-install-and-launch-smoke-test-rel-070077),
[plan](plan/features/linux-install-smoke-test.md)) tests the **published** x86_64 `.deb`, `.tar.gz` and
`.AppImage`. It downloads them, verifies their checksums and attestations, installs them in clean Ubuntu 24.04,
Ubuntu 26.04, Fedora (latest) and Arch Linux (latest) containers, starts the app on Xvfb (X11) and on headless
sway (Wayland), walks the pages with the keyboard, quits, and keeps screenshots. It also runs the AppImage and
starts the app on a profile written by an older version.

**When it runs.** By itself when a release is published, every Monday, and on pull requests that change the
workflow or `scripts/linux-smoke/`; and by hand, as below. A release run uses the workflow and scripts at the tag's
commit, so a tag cut before the workflow existed never starts it: dispatch it for that tag. Weekly and manual runs use
the files on `main` (or on the branch given to `gh workflow run --ref`). Without a tag, a weekly run or a dispatch with
an empty tag tests the latest published release. Pull requests and manual runs also run a `selftest` job that proves
the harness fails when it should.

**Run it for a tag.**

```sh
gh workflow run linux-smoke.yml -f tag=v0.2.0       # an empty tag means the latest published release
gh run list --workflow linux-smoke.yml --limit 3
gh run watch <run-id> --exit-status
```

**Results.** Each leg's job summary shows the image digest and the pass and fail counts per scenario. Each leg also
keeps two artifacts on the run page for 14 days; `<slug>` is `ubuntu-24.04`, `ubuntu-26.04`, `fedora` or `arch`:

| Artifact | Contents |
|---|---|
| `contact-<slug>.png` | one labelled contact sheet; a single PNG, not zipped |
| `smoke-<slug>` | everything the leg wrote: one folder per scenario with every screenshot (each with a platform caption), the logs and `results.txt` (a `PASS`, `FAIL` or `INFO` line per check), plus a copy of the contact sheet; `gh run download <run-id> -n smoke-<slug>` fetches it |

`gh run download <run-id>` with neither `-n` nor `-p` stops with `zip: not a valid zip file`, because the contact sheets are
uploaded unzipped. Use `-p 'smoke-*'` for the four evidence artifacts (each holds its own contact sheet), or open the
run page to see a contact sheet.

What each check means is in [the plan](plan/features/linux-install-smoke-test.md#62-what-a-leg-checks).

**Reproduce a leg locally.** Needs Docker and a logged-in `gh`. `fetch.sh` downloads and verifies the packages as the
workflow does; `leg.sh` runs one leg in a container and writes its evidence to the output directory:

```sh
export GITHUB_REPOSITORY=pavel-purma/dockering
smoke="$PWD/target/smoke"
bash scripts/linux-smoke/fetch.sh v0.2.0 "$smoke/pkg"       # <tag|latest> <dir>
bash scripts/linux-smoke/leg.sh fedora tar "$smoke/pkg" "$smoke/out" 0.2.0 quay.io/fedora/fedora:latest
```

`leg.sh <slug> <deb|tar> <pkg-dir> <out-dir> <version> <image>...` takes the version without the `v` and the directory
`fetch.sh` filled; a later image is a fallback if the one before it cannot be pulled. For a `.deb` leg of a release
that still declares plain `libc6` (0.2.0), set `SMOKE_LIBC_FLOOR=warn` as the workflow does; without it `libc_floor`
fails. The legs:

| Slug | Package | Primary image |
|---|---|---|
| `ubuntu-24.04` | `deb` | `mirror.gcr.io/library/ubuntu:24.04` |
| `ubuntu-26.04` | `deb` | `mirror.gcr.io/library/ubuntu:26.04` |
| `fedora` | `tar` | `quay.io/fedora/fedora:latest` |
| `arch` | `tar` | `ghcr.io/archlinux/archlinux:latest` |

**When it fails.** The release is already public, so a failing run does not change it: never edit, delete or
re-upload a published release (rule 2 of the [release skill](../.agents/skills/release/SKILL.md#rules)). The job summary
shows which leg and scenario failed and that scenario's `results.txt` names the failing check. Look at its screenshots
and logs, and report the check and the evidence. What happens to the published build is the maintainer's decision (step 8 of
[By hand](#by-hand-fallback)). A failing weekly run opens or updates one issue titled "Linux smoke test failing on the
latest release". Fedora and Arch run on `latest` on purpose, so a weekly failure without a new release can also be
drift in the distribution or in a harness tool.

**Limits.** The test starts the app with `--demo` or with no engine at all, so it does not prove the Docker connection
or any real engine; that stays a manual checklist item. It renders in software (Mesa llvmpipe), so GPU-driver problems
stay manual too. The published `.deb` declares plain `libc6` although the binary needs glibc 2.39
([finding F1](plan/features/linux-install-smoke-test.md#65-what-the-spike-showed), REL-073): until a release ships the
fix, the `libc_floor` check shows `INFO` and does not fail the run.

## First-release verification

| Evidence | Result |
|---|---|
| Release commit | [`041d8fac06f1a2d26a61ed5e13ad16f47371f628`](https://github.com/pavel-purma/dockering/commit/041d8fac06f1a2d26a61ed5e13ad16f47371f628). |
| Preparation and fixes | Merged [PR #24](https://github.com/pavel-purma/dockering/pull/24), [#25](https://github.com/pavel-purma/dockering/pull/25), and [#27](https://github.com/pavel-purma/dockering/pull/27). |
| Exact-commit CI | [37228704953](https://github.com/pavel-purma/dockering/actions/runs/37228704953), successful: six packages, three platform test jobs, lint, Docker integration, and eight packaging tests under native macOS Bash 3.2. |
| Tag build | [37228719100](https://github.com/pavel-purma/dockering/actions/runs/37228719100), successful at the exact release commit: six packages, assembly, provenance, and draft creation. |
| Downloaded Windows/Linux packages | Expected PE/ELF and Debian architectures/version; Windows setup/portable executables are `NotSigned`; portable archives contain license/notices; downloaded x64 `--version` prints `dockering 0.1.0`. |
| Complete assets, checksums and manifest | Downloaded all 15 files. All 14 [checksum entries](https://github.com/pavel-purma/dockering/releases/download/v0.1.0/SHA256SUMS) match. Manifest schema/version, six platform filenames/kinds/hashes/sizes/version URLs, and notes URL verified. No `.minisig`. |
| macOS and Debian format checks | Both DMGs are nonempty with a UDIF `koly` trailer. Linux Debian payload ELF architectures match their targets as well as the metadata. |
| Provenance | All 15 attestations verified with source digest pinned to the exact release SHA, ref `refs/tags/v0.1.0`, `release.yml` signer, and self-hosted runners denied. |
| Publication | [Dockering 0.1.0 (unsigned)](https://github.com/pavel-purma/dockering/releases/tag/v0.1.0), published `2026-10-04T20:09:38Z`, stable/latest, `draft=false`, `prerelease=false`; latest API returns this tag and all 15 files. |
| Public downloads | Anonymous requests follow redirects to HTTP 200 for all 15 version-pinned asset URLs and all 12 latest distribution links. |
| Channel configuration | Repository variable list remains empty; signing/updater/winget variables are unset. |

The initial unpublished-tag run failed after macOS Bash 3.2 rejected the packaging wrapper;
it did not produce a release. After native Bash 3.2 recovery/syntax tests and local actionlint
passed, the unpublished tag was repointed with a lease. Main CI and the replacement tag build
then ran in parallel; exact-commit CI was green before publication. No published release was changed.

Available verification currently covers automated installer tests and binary/archive inspection.
Real GUI, physical macOS/Linux launch, and WSL/WSLC walkthroughs remain unchecked in the
[release checklist](plan/release-checklist.md).

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

## First-release procedure

The user approved execution and publication on 2026-10-04. The preparation changes are merged,
and `v0.1.0` is published and verified. The commands below are the historical first-release
procedure. Later releases use their new workspace version and tag; never recreate or upload to
the published `v0.1.0` release.

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

   Verify the published assets and permanent download links. The workflow refuses tags that
   already have a published release; publish a new version for corrections. The release URL is
   `https://github.com/pavel-purma/dockering/releases/tag/v0.1.0`.

## Dry runs and retries

After the prepared workflow exists on the default branch, manual dispatch on a branch builds
all six targets and assembles a review artifact without creating a tag or GitHub Release:

```sh
gh workflow run release.yml --ref main -f unsigned=true
```

The review bundle has the same distributions, metadata, and notes as a tag release. Its manifest
URLs describe the eventual version tag and resolve only after publication. Dispatching on an
existing **unpublished** version tag creates or resumes its draft. Substitute that version's tag:

```sh
gh workflow run release.yml --ref vX.Y.Z -f unsigned=true
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
  FUSE support (without it, run the AppImage with `--appimage-extract-and-run`). `.deb` and `.tar.gz` are
  alternatives. Builds are made on Ubuntu 24.04, so the binaries need glibc 2.39 or newer, the runtime
  libraries (the `.deb` declares them; for the `.tar.gz` see the [README](../README.md#install-on-linux)),
  Wayland or X11, and a Vulkan driver.

Minimum systems: Windows 10 22H2, macOS 15, and Linux with glibc 2.39 or newer and Vulkan. Updates for this
unsigned release are manual downloads from GitHub Releases.

## release-plz (dormant)

`release-plz.yml` and `release-plz.toml` are kept but do nothing: without the repository secrets
`RELEASE_BOT_APP_ID` and `RELEASE_BOT_PRIVATE_KEY` both jobs print a notice and skip. **Don't enable
them next to the `/release` skill**: both open a `chore(release): vX.Y.Z` PR and would fight over the
version and the changelog. To switch back to bot-driven release PRs, create a GitHub App with
**Contents** and **Pull requests** read/write permissions and no webhook, add the two secrets, and stop
using `/release`. `GITHUB_TOKEN` cannot trigger other workflows by pushing tags, which is why the App
token is needed.

## Signed releases

The signing setup, the per-release routine and the operations (renewal, key rotation, incidents) are in
[signing.md](signing.md); `/release signing` walks through the one-time setup and `/release status` shows
how far it got. The channel is chosen by repository variables:

| Variable | Meaning |
|---|---|
| `PUBLIC_RELEASES=true` | the signed channel: tag builds are signed and contain the updater. Its historical name means the channel, not repository visibility. Manual `unsigned=true` overrides it. |
| `WINDOWS_SIGNING` | `signpath` \| `azure` \| `none` (default). |
| `MACOS_SIGNING` | `apple` \| `none` (default). Independent of Windows (REL-034): a signed release never requires it. |
| `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` | SignPath project. |
| `WINDOWS_SIGNER_SUBJECT` | the expected signer subject; **required** by signed tag builds (REL-032). |

Secrets live in the `release` environment (restricted to `main` and `v*` refs) and reach only the steps
that use them: `SIGNPATH_API_TOKEN`; `UPDATE_SIGNING_KEY` (+ optional `UPDATE_SIGNING_KEY_PASSWORD`);
Azure: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`,
`AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`; Apple: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`. Never commit a key.

A signed run fails early, in `verify`, when the configuration is incomplete, when `keys.rs` has no
public key, or when a tag has no `WINDOWS_SIGNER_SUBJECT`. Windows binaries are signed before
packaging, then the installers are signed and verified (Authenticode `Valid`, RFC 3161 timestamp,
SHA-256, one signer). Inno's generated uninstaller needs a local compile-time
`DOCKERING_INNO_SIGNTOOL`; remote signing alone does not cover it.

Updater keys: `cargo xtask gen-update-keys <private-directory>`; the current private key becomes
`UPDATE_SIGNING_KEY` and the reviewed **public** keys go into `crates/dk-update/src/keys.rs`
([key rotation](../crates/dk-update/keys/README.md)). The assembled manifest signature is verified
against those keys before the draft is created.

Submit the first signed winget version manually, configure `WINGET_TOKEN`, then set
`WINGET_ENABLED=true`. The workflow also requires published stable release metadata with a
signed update manifest, so an explicit unsigned override is skipped; see [winget setup](../packaging/winget/README.md).
