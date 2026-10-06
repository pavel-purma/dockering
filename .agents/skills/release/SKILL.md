---
name: release
description: Cut a Dockering release end to end. Writes the changelogs from the merged commits, picks the version, builds and tests locally, opens and merges the release PR, tags, watches the (signed) build, verifies the draft release and publishes it after approval. Also reports release status and guides the one-time Windows signing setup. Use when the user asks to release, ship or cut a version, prepare the changelog, bump the version, or set up code signing ("/release", "/release minor", "/release status", "/release signing").
---

# Release

One command runs the whole release flow of spec `docs/spec/features/distribution.md`
(REL-010…019, REL-030…034). The runbook for humans is `docs/release.md`; signing is
`docs/signing.md`. Supporting files, relative to this directory:
[changelog.md](changelog.md) · [signing.md](signing.md) · [checklist-template.md](checklist-template.md).

| Invocation | Mode |
|---|---|
| `/release [major\|minor\|patch\|X.Y.Z[-pre]] [--dry-run] [auto]` | **release**: cut the next version |
| `/release status` | **status**: unreleased changes, computed bump, channel, signing readiness |
| `/release signing` | **signing**: guided one-time Windows signing setup |

Host tools: `AskUserQuestion` in Claude Code is `question` in OpenCode; `Task` is `task`.

## Rules

1. Never push to `main`. The only force-push allowed is `--force-with-lease` on your own `chore/release-*` or `chore/record-*` branch.
2. Never move, delete or re-create a tag. Never edit, delete or re-upload a published release. If a **published** build is broken, stop and ask (`docs/release.md`, "If a published build is broken").
3. Never print, read back or store a secret or key. Pass secrets to `gh secret set NAME --env release < file`; never `cat` a key or token file, and never paste one into a command line.
4. Never run `scripts/installer-smoke.ps1` on this machine: it uses the real installer `AppId`, and its uninstall step removes the maintainer's own installation. CI runs it.
5. Never tick a checklist item that was not verified in this run. Leave unperformed manual checks unchecked.
6. Never change `WINDOWS_SIGNING`, `MACOS_SIGNING`, `PUBLIC_RELEASES`, `WINDOWS_SIGNER_SUBJECT` or a secret while a release PR is open, a release run is active, or a draft exists.
7. Never choose `1.0.0` yourself. Leaving `0.x` is the maintainer's explicit decision (`/release 1.0.0`).
8. Never rerun a failed job or continue past a failed check without reading its log.
9. Work in the scratch worktree (step 1). Never modify the maintainer's current checkout.
10. Squash merges only; auto-merge is off. GitHub does not require green checks on `main`, so **you** wait for them.

## Mode: release

### Where a run starts (resume)

Releases are long (the checks and the build take about an hour). Find out what already happened and continue from there:

| Found | Continue at |
|---|---|
| Nothing user-visible since the last tag | stop: nothing to release |
| Open PR from `chore/release-vX.Y.Z` | step 7 (wait, merge) |
| `Cargo.toml` version is higher than the last tag, merged, no tag | step 8 (tag) |
| Tag, no release run or no draft | step 9 (build) |
| Draft release | step 10 (verify) |
| Published, README/checklist not updated | step 12 (record) |

Each step first checks whether it is already done. `cargo xtask release-plan` shows `last_tag`, `workspace_version` and the commits; `gh pr list`, `gh run list --workflow release.yml`, `gh release view` show the rest.

### 0. Preflight

- `git fetch origin --tags --prune`. Tools: `git`, `gh` (scopes `repo`, `workflow`), `cargo`, PowerShell 7 (`pwsh`), Python 3. Optional: `cargo-nextest`, `cargo-deny`, `cargo-about`, Inno Setup.
- CI on `origin/main` HEAD is green: `gh run list --workflow ci.yml --branch main --limit 1 --json conclusion,status,headSha,databaseId`. If it is still running, wait. If it failed, read the failing job's log (`gh run view <id> --log-failed`) before deciding. A runner-acquisition failure ("The job was not acquired by Runner of type hosted even after multiple attempts") or the known flaky test `net_004_prune_unused_networks` failing **alone** allows one `gh run rerun <id> --failed`; report it as a flake in the PR body and the checklist, never as a pass. Any other failure: stop.
- No open `chore(release)` PR, no active `release.yml` run, no draft release (unless resuming it).
- **Channel**: signed when the repository variable `PUBLIC_RELEASES` is `true`, otherwise unsigned. `gh variable list` (variables) and `gh secret list --env release` (names only). A signed channel needs `WINDOWS_SIGNING` = `signpath` or `azure`, its secrets and variables, `UPDATE_SIGNING_KEY`, a public key in `crates/dk-update/src/keys.rs`, and for a tag, `WINDOWS_SIGNER_SUBJECT`. `MACOS_SIGNING=apple` additionally needs the Apple secrets. Stop if the configuration is incomplete, because the tag run would fail in `verify` after the PR is merged. Compare with the previous release's channel: it was signed when `gh release view <last-tag> --json assets` lists `dockering-update.json.minisig`.

### 1. Scratch worktree

```sh
git worktree add -b chore/release-tmp target/release-wt origin/main
```

`target/` is ignored by git. Run xtask only as `cargo xtask <command>` inside the scratch worktree: the built binary remembers the workspace root it was compiled in, so a binary from another checkout would read the wrong files. Reusing the main checkout's target directory (`CARGO_TARGET_DIR=<main checkout>/target`) saves compiling the dependencies again; the workspace crates still compile once. On Windows run cargo as `pwsh -NoProfile scripts/dev.ps1 <cargo args>`, and use `python` where `python3` is only the Microsoft Store stub. A cold build takes 10–20 minutes: run it in the background and poll. The version is known only after step 2: create the worktree on `chore/release-tmp` and rename the branch (`git branch -m chore/release-vX.Y.Z`) once it is.

### 2. Plan the release

`cargo xtask release-plan [--bump <arg>] [--bodies]` prints JSON: the commits since the last stable tag (`date`, type, scope, PR, requirement IDs, `files`, `spec_files`, `plans`, `user_visible`, `level`), the `bump` and `next_version`, the `reason`, `internal_only` and `warnings`. Rules are REL-011. To replay an old range, add `--since <tag> --to <rev>` (a replay warns that the version isn't greater than the latest tag; that is expected). `bump: none` means nothing user-visible: stop and tell the maintainer (an explicit `--bump patch` is allowed after they confirm). Show the warnings.

### 3. Changelogs

Write both with the rules in [changelog.md](changelog.md): the `CHANGELOG.md` section (user-facing prose) and the `docs/spec/CHANGELOG.md` lines for merged changes under `docs/spec/**`. Build the coverage table: every merged PR → its entry, or the reason it has none. Then derive the bump the written sections imply (Added, Removed or a breaking mark → minor in 0.x; only Changed/Deprecated/Fixed/Security → patch). It must equal `bump`; if it differs, fix the sections or the commit classification before going on.

**First signed release** (the channel is signed and the previous release was unsigned): the signed notes link to the README's *Code signing policy*, so this release's PR also rewrites the README: the Download notice (`> Downloads are unsigned…`), the *Updates* paragraph and the status paragraph of *Code signing policy*, which then carries the attribution sentence SignPath requires ([signing.md](signing.md) phase 7). Show that diff at Gate 1. The changelog intro says that Windows files are signed, that updates are available, and that macOS stays unsigned while `MACOS_SIGNING` is `none`.

### 4. Local gate (pre-bump)

Run what CI's `lint` and `test` jobs run, on the scratch worktree: `cargo fmt --all --check`; `cargo xtask check-blocking`; `cargo xtask icons --check`; clippy with `-D warnings`, also with `--features dockering/updater`; `cargo nextest run --workspace --locked` (else `cargo test --workspace --locked`) and `cargo nextest run -p dk-update -p dk-hub --features dk-hub/updater --locked`; `cargo deny check advisories bans licenses sources`; the `cargo about generate` diff; `python3 -B -m unittest discover -s scripts/tests -p "test_*.py"`. A missing optional tool: say so and rely on CI for it. A failure stops the release. A failed test may be rerun once; read the failure and report a flake as a flake (`net_004_prune_unused_networks` is known to flake), never as a pass.

### 5. Bump and local release build

- Set `[workspace.package] version` in `Cargo.toml` to `next_version`, then `cargo update --workspace`. Only `version = ` lines of workspace crates may change in `Cargo.lock`: `git diff -U0 Cargo.lock | grep -E '^[+-][^+-]' | grep -vE '^[+-]version = '` must print nothing.
- `cargo xtask release-verify --version X.Y.Z --remote` (version, lockfile, changelog heading, tag absent locally and on origin, version order).
- `cargo build --release --locked -p dockering` (add `--features updater` on the signed channel), then require that `target/release/dockering --version` prints `dockering X.Y.Z` (the exe has the GUI subsystem: on Windows read it through a pipe, `& $exe --version | Out-String`). Then, without `--target` so that it packages that build for the host: `cargo xtask package` and `cargo xtask verify-assets --assets target/dist/<host triple> --target <host triple>`, when the platform tools exist (Inno Setup on Windows; `cargo-packager` on macOS and Linux).

### 6. Gate 1, before anything is pushed

Show: the version and why (`reason`), the channel, the changelog diff (both files), the coverage table, the local results. `--dry-run` ends here: print the diff stat, remove the worktree, push nothing. Otherwise ask: open the release PR / edit the changelog first / cancel. With `auto` skip the question when the conditions in "Auto" hold.

### 7. Release PR

Commit `Cargo.toml`, `Cargo.lock` and the two changelogs (nothing else) as `chore(release): vX.Y.Z [REL-010, REL-011, REL-018, REL-019]`, push, and `gh pr create` with the same title. The body has: summary and the reason for the bump, the channel, the coverage table, the local results, requirement IDs. Add the host's attribution lines when the host requires them.

Wait for the checks: poll `gh pr checks <n> --json name,bucket` until nothing is `pending`; green means every bucket is `pass` or `skipping`. A failure: read the log; a runner-acquisition failure ("The job was not acquired by Runner of type hosted even after multiple attempts") is cancelled before any step ran, so after checking [githubstatus.com](https://www.githubstatus.com) use `gh run rerun <run-id> --failed`. Any other failure is real: fix the cause in a separate PR or stop.

Before merging: `git fetch origin`. If `origin/main` moved, rebase the branch (`--force-with-lease`), rerun step 2. When the set of user-visible commits changed, return to step 3 and Gate 1. Otherwise wait for the checks again. Then `gh pr merge <n> --squash`.

### 8. Tag

`git fetch origin --tags`. Check that `refs/tags/vX.Y.Z` is absent locally and on origin (`git ls-remote --tags origin refs/tags/vX.Y.Z`), then tag the PR's merge commit (`gh pr view <n> --json mergeCommit`), not a later `main` commit:

```sh
git tag -a vX.Y.Z -m "Dockering X.Y.Z (unsigned)" <merge-commit>    # signed channel: "Dockering X.Y.Z"
git push origin vX.Y.Z
```

### 9. Build

`gh run list --workflow release.yml --branch vX.Y.Z --limit 1 --json databaseId,status,conclusion,url`, then poll `gh run view <id> --json status,conclusion,jobs`. Never block longer than the host's command timeout; use a background command or wake-ups. It ends with a draft; a failed matrix job blocks assembly.

On the signed channel each Windows job submits two SignPath signing requests (`dockering.exe`, then the installer): four per release, each needs the maintainer's approval in SignPath (Projects → Dockering → Signing requests; SignPath also sends an e-mail). Tell them when the run reaches the signing steps; the action waits up to an hour. A **rejected** or **timed out** request: stop and ask. Rerunning creates new requests that need approval again.

### 10. Verify the draft

```sh
gh release download vX.Y.Z --dir target/release-review/vX.Y.Z
pwsh -NoProfile -File scripts/verify-release.ps1 -Dir target/release-review/vX.Y.Z -Version X.Y.Z \
  -Expect Unsigned|Signed -Changelog CHANGELOG.md -Attest -Commit <full 40-character merge-commit SHA> [-SignerSubject "<WINDOWS_SIGNER_SUBJECT>"] [-LaunchSmoke]
cargo xtask verify-manifest --assets target/release-review/vX.Y.Z      # signed channel only
```

- A signed draft can only be verified on Windows (Authenticode). Elsewhere stop and ask the maintainer to run the script on Windows.
- `-LaunchSmoke` starts the downloaded app on this machine's real profile (it backs the profile files up and restores them). Windows only, and only when no Dockering is running. Ask once before running it (unless `auto`). It is the check that would have caught the first 0.2.0 build.
- Exact-commit CI must be green: `gh run list --workflow ci.yml --commit <merge-commit>`.
- Any `[FAIL]` stops the release. Fix `main` through a PR and ship the next patch; never repair a tag.

### 11. Gate 2, before publishing

Show: the `RESULT` line, the attestation count, the signer subject (signed), the launch check, exact-commit CI, the notes, and the **manual checks not performed** (real-machine install and upgrade, macOS and Linux launch, WSL/WSLC walkthroughs). Ask: publish / stop and keep the draft. Then:

```sh
gh release edit vX.Y.Z --title "Dockering X.Y.Z (unsigned)" --draft=false --latest    # signed: "Dockering X.Y.Z"
gh release edit vX.Y.Z --draft=false                                                  # pre-release: no --latest
pwsh -NoProfile -File scripts/verify-release.ps1 -Dir target/release-review/vX.Y.Z -Version X.Y.Z -Expect <…> -Published
```

Publishing starts the `winget` workflow; report its result (it is skipped while `WINGET_ENABLED` or `PUBLIC_RELEASES` is unset).

### 12. Record

A docs PR `chore/record-vX.Y.Z`, titled `docs(release): record vX.Y.Z [REL-010, REL-018]`: the README download text and links, the `docs/release.md` table row, the new `docs/plan/release-checklist.md` section from [checklist-template.md](checklist-template.md) (ticked only for what ran), the `CHANGELOG.md` heading date if the UTC publication date differs, and spec status lines that name the latest release or the signing state. Wait for green checks, merge, then `git worktree remove target/release-wt` and delete the local branches. Finish with a short report: version, links (PR, run, release), evidence, what is still manual.

### Auto

`auto` skips Gate 1 and Gate 2 only when all of these hold; otherwise ask as usual: the channel equals the previous release's, `release-plan` gave no warnings, every check passed, the launch check ran and passed, and the previous release was not withdrawn. It never skips a failure.

## Mode: status

Print a table: last release (tag, date, channel); unreleased commits (total, user-visible) and the computed `bump` and `next_version` from `cargo xtask release-plan`; open release PR, active run, draft; channel configuration (variables and secret **names** from `gh variable list` and `gh secret list --env release`; whether `keys.rs` holds a public key); and the signing phase reached ([signing.md](signing.md)). Change nothing.

## Mode: signing

Follow [signing.md](signing.md): detect the next phase of the one-time Windows signing setup, do the parts an agent may do, and tell the maintainer exactly what only they can do.
