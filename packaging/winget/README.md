# winget package `Dockering.Dockering`

This folder holds the winget manifest template for Dockering (REL-040) and the steps to submit it
to [`microsoft/winget-pkgs`](https://github.com/microsoft/winget-pkgs) (REL-041). The plan is in
[`docs/plan/features/windows-distribution.md` §A3](../../docs/plan/features/windows-distribution.md#a3-winget).

```
manifests/
  Dockering.Dockering.yaml                 # version manifest
  Dockering.Dockering.locale.en-US.yaml    # default locale (publisher, licence, description, tags)
  Dockering.Dockering.installer.yaml       # Inno installers: x64 + arm64, user + machine scope
```

The manifests use schema **1.12.0**. Placeholders:

| Placeholder | Value |
|---|---|
| `{{VERSION}}` | Release version without the leading `v`, e.g. `0.2.0` |
| `{{X64_SHA256}}` | SHA-256 of `Dockering-Setup-x64.exe` (from the release's `SHA256SUMS`) |
| `{{ARM64_SHA256}}` | SHA-256 of `Dockering-Setup-arm64.exe` |

Key choices:

- `InstallerType: inno` and `UpgradeBehavior: install`.
- `ProductCode` (and the same value in `AppsAndFeaturesEntries`) is the Inno uninstall key, `AppId` of
  `packaging/windows/dockering.iss` plus `_is1`. winget matches the installed *Installed apps* entry to this
  package by it, so `winget list` and `winget upgrade` recognise a Dockering installed by hand. Without it the
  match falls back to the name and publisher, and the publisher in the locale file differs from the one the
  installer writes (`Dockering contributors`). A test keeps it equal to `AppId`.
- Each architecture has a `Scope: user` entry (`/CURRENTUSER`, the default) and a `Scope: machine`
  entry (`/ALLUSERS`) (REL-021).
- `RequireExplicitUpgrade` is **not** set. The self-updater updates the same *Installed apps* entry,
  so `winget upgrade` stays correct (REL-042).

The template is the reference for what the published manifest must contain. Komac generates
the real submission from the release assets and should produce the same fields.

## Submitting a release with `/winget`

The `winget` skill (`.agents/skills/winget/SKILL.md`, run as `/winget`) does the steps below for a published release, using three scripts in
`scripts/winget/`: `Get-Status.ps1`, `New-Manifest.ps1` and `Submit-Manifest.ps1`. It takes the latest GitHub release, renders these templates with
the version and the SHA-256 values of the release, checks them (the checksums against `SHA256SUMS` and GitHub's digest, a fresh download of each
installer, `winget validate`) and opens the pull request in `microsoft/winget-pkgs` from your fork. It asks before it pushes anything.

The winget-pkgs clone it works in is `<root of the drive that holds the worktree>\repos\winget-pkgs` (`D:\repos\winget-pkgs` for a worktree on `D:`).
It is a blobless, depth-1, sparse clone, because the full repository has more than 700,000 files.

You need `gh` logged in, `winget`, and PowerShell 7. The Microsoft organisation enforces SAML SSO, so authorise the `gh` token for it once
(`Get-Status.ps1` prints the link) and sign the [Contributor License Agreement](https://cla.opensource.microsoft.com).

The sections below are the same process by hand, and the setup for the automated workflow.

## First submission (manual, once)

Do this after the first **signed, public, stable** release (REL-060). Confirm the identifier
`Dockering.Dockering` first, because it can't be changed later.

1. Install Komac: `winget install Komac`. `wingetcreate` also works.
2. Create the manifests from the release assets:

   ```sh
   komac new Dockering.Dockering --version 0.2.0 --urls \
     https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-x64.exe \
     https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-arm64.exe
   ```

   Komac detects `inno` and both scopes. Fill in the prompts to match
   `manifests/Dockering.Dockering.locale.en-US.yaml`:
   - Publisher `Pavel Purma`
   - PackageName `Dockering`
   - License `MIT`
   - ShortDescription "Fast, native desktop client for Docker, Docker in WSL, and WSL containers."
   - Tags `docker`, `containers`, `wsl`, `devtools`
   - Moniker `dockering`
   - `ReleaseNotesUrl`
3. Compare the generated manifests with the templates here. Check that:
   - `UpgradeBehavior: install` is set;
   - each architecture has both a `Scope: user` entry (`/CURRENTUSER`) and a `Scope: machine` entry (`/ALLUSERS`);
   - the SHA-256 values match `SHA256SUMS`.
4. Validate, then test the install in a clean Windows Sandbox:

   ```powershell
   winget validate <manifest-dir>
   winget settings --enable LocalManifestFiles   # once, from an elevated prompt
   winget install --manifest <manifest-dir>
   winget uninstall Dockering.Dockering
   ```

   Test both scopes: `winget install --manifest <dir> --scope user`, then `--scope machine`.
5. Submit with `komac submit`. This opens a PR in `microsoft/winget-pkgs`. Watch the validation
   bot and answer the moderators.

## Automation (every later stable release)

`.github/workflows/winget.yml` runs on `release: published` and opens the winget-pkgs PR with
[`vedantmgoyal9/winget-releaser`](https://github.com/vedantmgoyal9/winget-releaser), which uses Komac.
It skips pre-releases. To resubmit a release by hand, run the workflow with `workflow_dispatch` and a `tag` input.

The action fails until the package exists in winget-pkgs, so do the first submission above
before you enable the workflow.

One-time setup:

1. Fork `microsoft/winget-pkgs` to the `pavel-purma` account. The action pushes branches to this fork.
2. Create a **classic** personal access token with the `public_repo` scope. Add the `workflow` scope
   too if the fork's workflows need updating. Store it as the repository secret `WINGET_TOKEN`.
3. Set the repository variable `WINGET_ENABLED` to `true`. While it is unset, the workflow does nothing.
