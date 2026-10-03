# Releasing Dockering

The normative requirements are in [spec `features/distribution.md`](spec/features/distribution.md)
(REL-010…060, UPD-001…012), and the reasoning is in
[ADR-0006](plan/adr/0006-windows-installer-and-updates.md). This page is the maintainer's runbook.

## Now: private phase (internal builds)

The repo is private and nothing is published to users yet, but versioning and the release flow
already work. Every release is an **internal build**:

- unsigned (`WINDOWS_SIGNING` unset), and without the in-app updater (`PUBLIC_RELEASES` unset);
- a **draft** GitHub Release in this private repo. Only collaborators can see it, and you can
  leave it a draft forever (*latest* only moves when a release is *published*);
- winget, signing, attestations, and the update manifest signature are all skipped.

What you need for it (one time):

1. The release bot App (below) and its two secrets, so release-plz opens release PRs and tags.
   Without it, use the manual tag (below). Everything else still works.
2. Nothing else. No environments, no signing, no keys.

Per release:

1. Merge the release PR `chore(release): vX.Y.Z` (or push a tag by hand).
2. Wait for `release.yml` (about 30–40 min for 6 targets) → a draft release with all installers,
   `dockering-update.json` (unsigned), and `SHA256SUMS`.
3. Download and try the Windows installer. Expect a SmartScreen warning: *More info → Run anyway*.
4. Leave it as a draft, or delete it. Don't publish while the repo is private: there's no
   one to publish to, and published releases can't be immutable or attested on GitHub Free.

> **GitHub Free and private repos.** Environments (`release`, `release-plz`), environment secrets,
> required reviewers, rulesets, and artifact attestations aren't available for private repos on
> GitHub Free. So during this phase: the workflows use **repository** secrets, the `environment:`
> lines in `release.yml` have no protection rules, and attestations are skipped
> (`PUBLIC_RELEASES` unset). Branch protection and the tag ruleset come with the public launch (or
> a paid plan).

Note: the first release is **0.1.0**. With no `v*` tag yet, release-plz treats the current version
as the initial release, so the first release PR is `chore(release): v0.1.0`.

## The flow (REL-010)

```
feature PRs (squash merge; conventional-commit PR titles) ──▶ main
      │ every push to main
      ▼
release-plz.yml / release-pr ──▶ keeps ONE PR open: "chore(release): vX.Y.Z"
      │                          (workspace version, Cargo.lock, CHANGELOG.md section)
      │ you: review, polish the changelog wording, run the release checklist, merge
      ▼
release-plz.yml / release ──▶ tag vX.Y.Z (release-bot App token → starts release.yml)
      ▼
release.yml: verify → package ×6 (sign) → manifest + minisign → SHA256SUMS → attest → DRAFT release
      │ you: download Dockering-Setup-x64.exe from the draft, install it, sanity-check it
      ▼
Publish ──▶ "latest" moves ──▶ the in-app updater sees it (≤ 24 h) ──▶ winget.yml opens a winget-pkgs PR
```

Per release you do two things: **merge the release PR**, then **publish the draft**. You never
type a version number or a tag.

Before merging the release PR:

- run the [release checklist](plan/release-checklist.md) on a build of the PR (CI's `installer`
  artifact for Windows);
- if dependencies changed since the last release, regenerate the third-party notices
  (`cargo about generate about.hbs -o THIRD_PARTY_LICENSES.html`). CI's `lint` job fails when
  they're stale, so the release PR can't be merged otherwise;
- if the release-plz PR moved hand-written changelog bullets below the new section, move them
  back into it.

### Versions (REL-011)

release-plz works out the next version from the commit messages (squash merges, so the PR titles):

| Commit | 0.x | ≥ 1.0 |
|---|---|---|
| `feat:` | minor | minor |
| `fix:`, `perf:`, `refactor:`, others | patch | patch |
| breaking (`feat!:` or `BREAKING CHANGE:`) | minor | major |

`docs`, `test`, `chore`, `ci`, `style`, and `build` commits are left out of the changelog. The
changelog keeps the Keep a Changelog headings (*Added*, *Changed*, *Fixed*). Edit the wording in
the release PR before you merge it; release-plz keeps manual edits.

### Pre-releases

To ship `-alpha.N`, `-beta.N`, or `-rc.N`, set the version on the release PR branch before merging.
Edit `[workspace.package] version` in `Cargo.toml` and the top `## [x.y.z]` heading in
`CHANGELOG.md` to `x.y.z-rc.1`, then run `cargo update --workspace` to refresh `Cargo.lock`.

Don't use `release-plz set-version` here. In this workspace (checked with 0.3.169) it either fails
(it looks for per-crate changelogs) or, with `dockering@…`, replaces `version.workspace = true` in
`crates/dockering/Cargo.toml` with a literal version, which splits the app from the workspace
version.

Pre-releases become GitHub *pre-releases*. They are never "latest", so neither the updater nor
winget sees them. They may be unsigned.

### Hotfixes

Normally, fix the bug on `main` and let the next release PR ship a patch.

If `main` holds work that isn't ready to release:

1. Branch `release/X.Y` from the last tag.
2. Cherry-pick the fix onto it.
3. release-plz runs on `release/*` too, so a release PR opens against that branch.

### Manual fallback

You can always tag by hand; the tag ruleset lets the maintainer bypass it:

```sh
git tag -a v0.2.0 -m "Dockering 0.2.0" && git push origin v0.2.0
```

`release.yml` checks a hand-made tag the same way as a bot tag:

- the tag equals the workspace version;
- `CHANGELOG.md` has a `## [0.2.0]` section;
- the tagged commit is on `main` or a `release/*` branch.

### Yanking a bad release

The preferred fix is to publish a patch right away.

If it's urgent, edit the bad release and mark it as a pre-release. That moves *latest* back to the
previous version, which stops the updater from offering it. Then open a winget-pkgs PR that removes
the version.

## One-time setup

### Release bot (GitHub App), needed by release-plz

The default `GITHUB_TOKEN` can't start other workflows. Without this App, CI wouldn't run on the
release PR and the tag wouldn't start `release.yml`.

1. Go to *Settings › Developer settings › GitHub Apps › New GitHub App* and create
   `dockering-release-bot`. It needs no webhook. Repository permissions: **Contents** read/write
   and **Pull requests** read/write.
2. Install it on `pavel-purma/dockering` only.
3. Generate a private key for the App (*App settings › Private keys*). Add two **repository**
   secrets (*Settings › Secrets and variables › Actions*): `RELEASE_BOT_APP_ID` (the App ID or
   Client ID) and `RELEASE_BOT_PRIVATE_KEY` (the whole `.pem`).
4. Repository settings:
   - allow **squash merging** only, and set the default commit message to *Pull request title*;
   - at the public launch (GitHub Free has no rulesets or protection for private repos):
     branch protection on `main` requiring a PR, linear history, and these checks (the names
     GitHub shows): `lint`, `test (ubuntu-24.04)`, `test (windows-2025)`, `test (macos-15)`,
     `build (…)` ×6 (the two Windows ones also build and smoke-test the installer),
     `conventional commit title`; and a **tag ruleset** for `v*` restricting creation to the
     release bot App, with a bypass for you.

Until the secrets exist, `release-plz.yml` only prints a notice. Releases then need the manual tag.

### Windows code signing (REL-030…032)

`release.yml` reads the repository variable `WINDOWS_SIGNING`:

| Value | Provider | Needs |
|---|---|---|
| `none` (default) | unsigned. While `PUBLIC_RELEASES` is unset, any tag builds unsigned (internal builds). Once it's `true`, only pre-release tags may be unsigned | — |
| `signpath` | SignPath Foundation (free for OSS; the publisher shown is "SignPath Foundation") | secret `SIGNPATH_API_TOKEN`; vars `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`; artifact configurations `exe` and `installer`; signing policy `release-signing` |
| `azure` | Azure Artifact Signing (individuals only in the US/CA; EU organisations are eligible) | secrets `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` |

Signing has two stages. First `dockering.exe` is signed, and only then is it packed into the
installer and the zip. After packaging, the setup `.exe` is signed. A PowerShell step then checks
that every file's signature is `Valid` and that all files have the same signer.

The step-by-step enrolment for each provider (SignPath application, Certum fallback, Azure
identity validation) is in [plan §A2](plan/features/windows-distribution.md#a2-windows-code-signing).

⚠ Inno writes the uninstaller at install time. With remote signing (SignPath or Azure) it isn't
signed yet. Inno's `SignedUninstaller` needs a `SignTool` at compile time
(`DOCKERING_INNO_SIGNTOOL`), which only works with a local or CLI signer. This is tracked in the
plan's risks.

### Update signing key (UPD-003)

The installed app only accepts an update manifest that a key compiled into it has signed.

```sh
cargo xtask gen-update-keys ~/dockering-keys     # writes current.key/.pub and next.key/.pub
```

1. Paste both printed public keys into `crates/dk-update/src/keys.rs`, `current` first, and merge.
2. Store the contents of `current.key` as the secret `UPDATE_SIGNING_KEY`, in the environment
   `release` once the repo is public (repository secret before that).
3. Keep `next.key` offline, in a password manager. Delete the key files from disk.

Rotation works by making `next` current and generating a new `next`; see
[`crates/dk-update/keys/README.md`](../crates/dk-update/keys/README.md). Without
`UPDATE_SIGNING_KEY` the manifest ships unsigned and installed apps ignore it.

### winget (REL-040…042)

See [`packaging/winget/README.md`](../packaging/winget/README.md). In short:

1. Submit the first version by hand with `komac new` after the first signed public stable release.
2. Fork `microsoft/winget-pkgs`.
3. Store a classic PAT with scope `public_repo` as the secret `WINGET_TOKEN`.
4. Set `WINGET_ENABLED=true`.

## Going public (REL-060)

Until the repo is public, releases are unsigned internal builds (see *Now: private phase*), and
the updater is compiled out of them (no `updater` feature). Checklist, in order:

- [ ] Secret scan of the full history (`gitleaks detect --log-opts=--all`), with no hits; no private paths or tokens in fixtures.
- [ ] Make the repo public. Enable **immutable releases** (*Settings › General › Releases*).
- [ ] Branch protection and the tag ruleset (above). Create the environment `release` with you as
      required reviewer, and move the signing secrets and `UPDATE_SIGNING_KEY` from repository
      secrets into it (environments now work).
- [ ] Dependabot alerts and secret scanning turned on.
- [ ] Repo *About*: the description, topics, and social preview `assets/brand/social-preview.png`
      (see [plan §A4](plan/features/windows-distribution.md#a4-home-page-copy)):
      ```sh
      gh repo edit pavel-purma/dockering \
        --description "Fast, native desktop client for Docker, Docker in WSL, and WSL containers. Rust + GPUI. Windows · macOS · Linux." \
        --add-topic docker,containers,wsl,wslc,desktop-app,rust,gpui,docker-desktop-alternative,devtools
      ```
      The social preview can only be uploaded in the web UI (*Settings › General*).
- [ ] Apply to SignPath Foundation, set it up, then set `WINDOWS_SIGNING=signpath`.
- [ ] Generate the update keys (above), commit the public keys, and set `PUBLIC_RELEASES=true`.
      This turns on, for release builds: the updater, attestations, the "stable must be signed"
      guard, and the "update key must be set" check.
- [ ] Set `WINDOWS_SIGNER_SUBJECT` to the signer DN shown by the first signed build.
- [ ] Delete the old internal draft releases (or keep them as drafts). The first *published*
      release becomes *latest*.
- [ ] Uncomment the shields.io badges in `README.md`.
- [ ] First signed stable release → manual winget submission → `WINGET_ENABLED=true`.

## Secrets and variables

| Name | Kind | Where | Used by |
|---|---|---|---|
| `RELEASE_BOT_APP_ID`, `RELEASE_BOT_PRIVATE_KEY` | secret | repo | release-plz.yml |
| `SIGNPATH_API_TOKEN` | secret | env `release` (repo while private) | release.yml (`signpath`) |
| `AZURE_*` (6) | secret | env `release` | release.yml (`azure`) |
| `UPDATE_SIGNING_KEY` (+ `UPDATE_SIGNING_KEY_PASSWORD`) | secret | env `release` (repo while private) | release.yml manifest signature |
| `APPLE_*` (6) | secret | env `release` | release.yml (macOS) |
| `WINGET_TOKEN` | secret | repo | winget.yml |
| `WINDOWS_SIGNING` | variable | repo | release.yml |
| `WINDOWS_SIGNER_SUBJECT` | variable | repo | release.yml (REL-032 expected signer, e.g. `CN=SignPath Foundation, …`) |
| `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` | variable | repo | release.yml |
| `PUBLIC_RELEASES` | variable | repo | release.yml: `--features updater`, attestations, signed-stable and update-key guards |
| `WINGET_ENABLED` | variable | repo | winget.yml |

## Local dry runs

```sh
cargo build --release -p dockering
cargo xtask package --formats inno                  # target/dist/<triple>/Dockering-Setup-x64.exe + zip
pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup target/dist/x86_64-pc-windows-msvc/Dockering-Setup-x64.exe -Version 0.1.0
cargo xtask update-manifest --assets target/dist/x86_64-pc-windows-msvc --version 0.1.0
cargo xtask checksums target/dist/x86_64-pc-windows-msvc
release-plz update --dry-run                        # preview the next version + changelog (cargo install release-plz)
```
