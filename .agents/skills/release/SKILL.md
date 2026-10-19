---
name: release
description: Cut a Dockering release end to end. Writes the changelogs from the merged commits, picks the version, builds and tests locally, opens and merges the release PR, tags, watches the (signed) build, verifies the draft release and publishes it after approval. Also reports release status and guides the one-time Windows signing setup. Use when the user asks to release, ship or cut a version, prepare the changelog, bump the version, or set up code signing ("/release", "/release minor", "/release status", "/release signing").
---

# Release

One command runs the whole release flow of spec `docs/spec/features/distribution.md`
(REL-010…019, REL-030…034, REL-083, REL-084). The runbook for humans is `docs/release.md`; signing is
`docs/signing.md`. Supporting files, relative to this directory:
[changelog.md](changelog.md) · [signing.md](signing.md) · [checklist-template.md](checklist-template.md).

This file describes the current behaviour only. The channel has two independent parts: OS signing
(`PUBLIC_RELEASES`, `-Expect Signed|Unsigned`) and the updater (`UPDATER=none|all`, `--updater` / `-Updater`). Neither is
inferred from the other, and neither from whether a `.minisig` exists.

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
6. Never change `WINDOWS_SIGNING`, `MACOS_SIGNING`, `PUBLIC_RELEASES`, `UPDATER`, `WINDOWS_SIGNER_SUBJECT` or a secret while a release PR is open, a release run is active, or a draft exists.
7. Never choose `1.0.0` yourself. Leaving `0.x` is the maintainer's explicit decision (`/release 1.0.0`).
8. Never rerun a failed job or continue past a failed check without reading its log.
9. Work in the scratch worktree (step 1). Never modify the maintainer's current checkout.
10. Squash merges only; auto-merge is off. `main` requires the `CI gate` and `conventional commit title` checks (REL-080), so GitHub refuses a merge until both pass. Only the maintainer can bypass that, from the PR UI: never use `gh pr merge --admin`, and still read a failed check's log (rule 8) rather than rerunning blindly.

## Mode: release

### Where a run starts (resume)

Releases are long (the checks and the build take about an hour). Find out what already happened and continue from there:

| Found | Continue at |
|---|---|
| Nothing user-visible since the last tag | stop: nothing to release |
| Open PR from `chore/release-vX.Y.Z` | step 7 (wait, merge) |
| `Cargo.toml` version is higher than the last tag, merged, no tag | step 8 (tag) |
| Tag, no release run or no draft | step 9 (build) |
| Draft release | step 10 (verify, then the draft gate) |
| Published, README/checklist not updated | step 11's `Linux smoke` and `macOS smoke` runs (find, wait, report), then step 12 (record) |

Each step first checks whether it is already done. `cargo xtask release-plan` shows `last_tag`, `workspace_version` and the commits; `gh pr list`, `gh run list --workflow release.yml`, `gh release view` show the rest.

### 0. Preflight

- `git fetch origin --tags --prune`. Tools: `git`, `gh` (scopes `repo`, `workflow`), `cargo`, PowerShell 7 (`pwsh`), Python 3. Optional: `cargo-nextest`, `cargo-deny`, `cargo-about`, Inno Setup.
- CI on `origin/main` HEAD is green: `gh run list --workflow ci.yml --branch main --limit 1 --json conclusion,status,headSha,databaseId`. If it is still running, wait. If it failed, read the failing job's log (`gh run view <id> --log-failed`) before deciding. A runner-acquisition failure ("The job was not acquired by Runner of type hosted even after multiple attempts") or the known flaky test `net_004_prune_unused_networks` failing **alone** allows one `gh run rerun <id> --failed`; report it as a flake in the PR body and the checklist, never as a pass. Any other failure: stop.
- No open `chore(release)` PR, no active `release.yml` run, no draft release (unless resuming it).
- **Channel** has two independent parts (REL-083). `gh variable list` (variables) and `gh secret list --env release` (names only):
  - **OS signing**: signed when the repository variable `PUBLIC_RELEASES` is `true`, otherwise unsigned (this is what `-Expect Signed|Unsigned` means). A signed build needs `WINDOWS_SIGNING` = `signpath` or `azure`, its secrets and variables, and for a tag, `WINDOWS_SIGNER_SUBJECT`. `MACOS_SIGNING=apple` additionally needs the Apple secrets.
  - **Updater**: `UPDATER` = `none` (default, also when unset) or `all` (this is what `--updater none|all` and `-Updater none|all` mean). A tag run uses the variable, nothing else. `all` builds every target with `--features updater`, **whether or not the build is OS-signed**, and signs the manifest.
  - The manifest is signed (`sign_manifest`) when `UPDATER=all` **or** the build is OS-signed, so a `dockering-update.json.minisig` no longer says which part is on. Whenever it is signed the run needs `UPDATE_SIGNING_KEY` and a public key in `crates/dk-update/src/keys.rs`.

  Stop if the configuration is incomplete, because the tag run would fail in `verify` after the PR is merged. Compare with the previous release's channel from its notes, never from its files: `gh release view <last-tag> --json body --jq .body` says whether it was OS-signed (**Signed release** or **Unsigned release**) and carries one sentence for the update channel (*Dockering checks for updates in these builds…* is `all`; *Automatic updates are not included in these builds; the update manifest is signed…* or *…has no signature* is `none`). Releases up to v0.3.0 predate this: unsigned, no updater, and the sentence *Automatic updates are disabled*.

### 1. Scratch worktree

```sh
git worktree add -b chore/release-tmp target/release-wt origin/main
```

`target/` is ignored by git. Run xtask only as `cargo xtask <command>` inside the scratch worktree: the built binary remembers the workspace root it was compiled in, so a binary from another checkout would read the wrong files. Reusing the main checkout's target directory (`CARGO_TARGET_DIR=<main checkout>/target`) saves compiling the dependencies again; the workspace crates still compile once. On Windows run cargo as `pwsh -NoProfile scripts/dev.ps1 <cargo args>`, and use `python` where `python3` is only the Microsoft Store stub. A cold build takes 10–20 minutes: run it in the background and poll. The version is known only after step 2: create the worktree on `chore/release-tmp` and rename the branch (`git branch -m chore/release-vX.Y.Z`) once it is.

### 2. Plan the release

`cargo xtask release-plan [--bump <arg>] [--bodies]` prints JSON: the commits since the last stable tag (`date`, type, scope, PR, requirement IDs, `files`, `spec_files`, `plans`, `user_visible`, `level`), the `bump` and `next_version`, the `reason`, `internal_only` and `warnings`. Rules are REL-011. To replay an old range, add `--since <tag> --to <rev>` (a replay warns that the version isn't greater than the latest tag; that is expected). `bump: none` means nothing user-visible: stop and tell the maintainer (an explicit `--bump patch` is allowed after they confirm). Show the warnings.

### 3. Changelogs

Write both with the rules in [changelog.md](changelog.md): the `CHANGELOG.md` section (user-facing prose) and the `docs/spec/CHANGELOG.md` lines for merged changes under `docs/spec/**`. Build the coverage table: every merged PR → its entry, or the reason it has none. Then derive the bump the written sections imply (Added, Removed or a breaking mark → minor in 0.x; only Changed/Deprecated/Fixed/Security → patch). It must equal `bump`; if it differs, fix the sections or the commit classification before going on.

**First OS-signed release** (`PUBLIC_RELEASES=true` and the previous release was unsigned): the signed notes link to the README's *Code signing policy*, so this release's PR also rewrites the README: the Download notice (`> Downloads are unsigned…`), the *Updates* paragraph and the status paragraph of *Code signing policy*, which then carries the attribution sentence SignPath requires ([signing.md](signing.md) phase 7). Show that diff at Gate 1. The changelog intro says that Windows files are signed and that macOS stays unsigned while `MACOS_SIGNING` is `none`. It says nothing about updates unless the updater channel also changes (below): OS signing no longer turns the updater on.

**First updater release** (`UPDATER=all` and the previous release was `none`): the notes and the README's *Privacy & network* text must say that this build checks GitHub Releases for updates and how to turn that off; the changelog intro says that earlier releases have no updater, so the installed copy has to be updated by hand once. Show that diff at Gate 1. A release that switches the updater **off** again says so in the intro.

### 4. Local gate (pre-bump)

Run what CI's `lint` and `test` jobs run, on the scratch worktree: `cargo fmt --all --check`; `cargo xtask check-blocking`; `cargo xtask icons --check`; clippy with `-D warnings`, also with `--features dockering/updater`; `cargo nextest run --workspace --locked` (else `cargo test --workspace --locked`) and `cargo nextest run -p dk-update -p dk-hub --features dk-hub/updater --locked`; `cargo deny check advisories bans licenses sources`; the `cargo about generate` diff; `python3 -B -m unittest discover -s scripts/tests -p "test_*.py"`. A missing optional tool: say so and rely on CI for it. A failure stops the release. A failed test may be rerun once; read the failure and report a flake as a flake (`net_004_prune_unused_networks` is known to flake), never as a pass.

### 5. Bump and local release build

- Set `[workspace.package] version` in `Cargo.toml` to `next_version`, then `cargo update --workspace`. Only `version = ` lines of workspace crates may change in `Cargo.lock`: `git diff -U0 Cargo.lock | grep -E '^[+-][^+-]' | grep -vE '^[+-]version = '` must print nothing.
- `cargo xtask release-verify --version X.Y.Z --remote` (version, lockfile, changelog heading, tag absent locally and on origin, version order).
- `cargo build --release --locked -p dockering` (add `--features updater` exactly when the channel's `UPDATER` is `all`, with or without OS signing), then require that `target/release/dockering --version` prints `dockering X.Y.Z` (the exe has the GUI subsystem: on Windows read it through a pipe, `& $exe --version | Out-String`). Never add `--features update-e2e`: the test seam never ships (REL-086). Then, without `--target` so that it packages that build for the host: `cargo xtask package` and `cargo xtask verify-assets --assets target/dist/<host triple> --target <host triple>`, when the platform tools exist (Inno Setup on Windows; `cargo-packager` on macOS and Linux).

### 6. Gate 1, before anything is pushed

Show: the version and why (`reason`), the channel, the changelog diff (both files), the coverage table, the local results. `--dry-run` ends here: print the diff stat, remove the worktree, push nothing. The changelogs are always derived by the skill from the merged commits and PRs ([changelog.md](changelog.md)); never ask the maintainer to write or supply them, only to review the result. Otherwise ask: open the release PR / edit the changelog first / cancel. With `auto` skip the question when the conditions in "Auto" hold.

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

On an OS-signed build each Windows job submits two SignPath signing requests (`dockering.exe`, then the installer): four per release, each needs the maintainer's approval in SignPath (Projects → Dockering → Signing requests; SignPath also sends an e-mail). Tell them when the run reaches the signing steps; the action waits up to an hour. A **rejected** or **timed out** request: stop and ask. Rerunning creates new requests that need approval again.

### 10. Verify the draft

Read the channel **from the draft**, not from the files in it (`dockering-update.json.minisig` exists when `sign_manifest` is true: updater `all` or an OS-signed build). The tag's `release.yml` run uploads the artifact `updater-evidence-X.Y.Z` (six `updater-evidence-<target>.txt` files, one per target, written from the unpackaged binaries; they are workflow artifacts, never release assets), and the draft's `RELEASE_NOTES.md` carries the sentence for the channel:

- `--updater all` / `-Updater all`: all six files say `updater=true`, and the notes say *Dockering checks for updates in these builds…*.
- `--updater none` / `-Updater none`: all six say `updater=false`, and the notes say *Automatic updates are not included in these builds; the update manifest is signed…* or *…has no signature*.
- `-Expect Signed|Unsigned` is the OS signing mode: **Signed release** or **Unsigned release** in the notes.
- A mixed or incomplete set of evidence files, or a channel that disagrees with the notes or with the `UPDATER` value you recorded in step 0, is a failed gate: stop. (`assemble` rejects a mixed set, so this means the draft isn't the build step 9 reported.)

```sh
run=$(gh run list --workflow release.yml --branch vX.Y.Z --limit 1 --json databaseId --jq '.[0].databaseId')
gh release download vX.Y.Z --dir target/release-review/vX.Y.Z
gh run download "$run" -n updater-evidence-X.Y.Z --dir target/release-review/evidence-X.Y.Z     # not into the release directory
grep -H '^updater=\|^seam=' target/release-review/evidence-X.Y.Z/*/updater-evidence-*.txt        # six updater=<same>, six seam=false
pwsh -NoProfile -File scripts/verify-release.ps1 -Dir target/release-review/vX.Y.Z -Version X.Y.Z \
  -Expect Unsigned|Signed -Updater none|all -Changelog CHANGELOG.md -Attest -Commit <full 40-character merge-commit SHA> [-SignerSubject "<WINDOWS_SIGNER_SUBJECT>"] [-LaunchSmoke]
cargo xtask verify-update-artifacts --assets target/release-review/vX.Y.Z --version X.Y.Z --updater none|all
```

- `verify-release.ps1` has no cargo and no minisign: it checks the file set (the `.minisig` is required when `-Updater all` or `-Expect Signed`, forbidden otherwise) and that the notes carry the channel's sentence. `cargo xtask verify-update-artifacts` is the other half and **always runs next to it**, from the scratch worktree at the release commit so that `crates/dk-update/src/keys.rs` is the one the build embedded. It checks that the `.minisig` verifies against those keys **and was made by the first (current) key** (a secret set to the rotation key fails with "the secret `UPDATE_SIGNING_KEY` is not the current key"), that the manifest `version` is X.Y.Z, and that each of the six platform entries equals its asset in kind, URL, size and SHA-256. With `--updater none` and no `.minisig` it checks the entries and the version only. It supersedes `verify-manifest` for the draft.
- An **OS-signed** draft (`-Expect Signed`) can only be verified on Windows (Authenticode). Elsewhere stop and ask the maintainer to run the script on Windows. An unsigned draft, with or without the updater, verifies anywhere with `pwsh`.
- `-LaunchSmoke` starts the downloaded app on this machine's real profile (it backs the profile files up and restores them). Windows only, and only when no Dockering is running. Ask once before running it (unless `auto`). It is the check that would have caught the first 0.2.0 build.
- Exact-commit CI must be green: `gh run list --workflow ci.yml --commit <merge-commit>`.
- Any `[FAIL]`, and any failure of `verify-update-artifacts`, stops the release. The draft is unpublished, so nothing is public yet: report the failed check and ask. A wrong update-key secret is the maintainer's to correct (rule 3); fix `main` through a PR and ship the next patch, and never repair a tag.

**Draft gate (REL-079).** `verify-release.ps1` cannot start the Linux or macOS builds, and the first 0.2.0 build passed every check it has. The `Linux smoke` and `macOS smoke` workflows can, on the draft's own files, before anyone can download them. They need the id of the tag's `release.yml` run (step 9):

```sh
run=$(gh run list --workflow release.yml --branch vX.Y.Z --limit 1 --json databaseId --jq '.[0].databaseId')
gh workflow run linux-smoke.yml -f run="$run"
gh workflow run macos-smoke.yml -f run="$run"
```

Find both runs with `gh run list --workflow <file> --limit 3` (the title reads `draft, run <id>`), follow each with `gh run watch <run-id> --exit-status` (never block longer than the host's command timeout), and keep the links and results for Gate 2. Review the evidence as in step 11 (`gh run download <run-id> -p 'smoke-*'` for Linux, `-p 'macos-smoke-*'` for macOS, each into its own directory under `target/smoke-review/vX.Y.Z`). A failed leg stops the release exactly like a `[FAIL]` above: the build is still a draft, so nothing is public yet. Read the log, report the failing check and the screenshots, and stop to ask. Two known states are not failures: `libc_floor` as `INFO` (REL-073) and `bundle_seal` as `INFO` (finding F9). If a run cannot start, because a workflow is missing at the tag's commit or the dispatch is refused, say so at Gate 2 and do not report it as a pass.

### 11. Gate 2, before publishing

Show: the `RESULT` line, the **channel** read from the draft (OS signed or not, `updater` none or all, and that the six evidence files and the notes agree), the `verify-update-artifacts` result (which public key signed, the version, six entries), the attestation count, the signer subject (signed), the launch check, exact-commit CI, the notes, the **draft gate** results (both workflows, run links, which legs passed), and the **manual checks not performed** (real-machine install and upgrade, WSL/WSLC walkthroughs, a real Docker connection on every platform). Say what the smoke tests cover and what they do not: clean containers on Linux and hosted runners on macOS, `--demo` or no engine, software rendering on Linux and the runner's GPU on macOS; a physical Linux or Mac machine stays manual. Ask: publish / stop and keep the draft. Then:

```sh
gh release edit vX.Y.Z --title "Dockering X.Y.Z (unsigned)" --draft=false --latest    # signed: "Dockering X.Y.Z"
gh release edit vX.Y.Z --draft=false                                                  # pre-release: no --latest
pwsh -NoProfile -File scripts/verify-release.ps1 -Dir target/release-review/vX.Y.Z -Version X.Y.Z -Expect <…> -Updater <none|all> -Published
```

Publishing starts the `winget` workflow; report its result (it is skipped while `WINGET_ENABLED` or `PUBLIC_RELEASES` is unset).

Publishing also starts the `Linux smoke` and `macOS smoke` workflows (REL-070, REL-078), this time on the published files. They install the packages in clean Ubuntu, Fedora and Arch containers and on hosted macOS runners, start the app and keep screenshots. Wait for both, then record them in step 12:

- Find each run with `gh run list --workflow linux-smoke.yml --limit 3` and `gh run list --workflow macos-smoke.yml --limit 3` (the title names the tag) and follow it with `gh run watch <run-id> --exit-status` (as in step 9, never block longer than the host's command timeout). If no run for this tag appears within a minute, dispatch one: `gh workflow run <file> -f tag=vX.Y.Z`.
- Keep their links and results for step 12. To review the screenshots, download the evidence into its own directory (`gh run download <run-id> -p 'smoke-*' --dir target/smoke-review/vX.Y.Z` for Linux, `-p 'macos-smoke-*'` for macOS) and look at each leg's `contact-<slug>.png` (Linux) and the first frame and page screenshots (macOS). Tick "reviewed" only if you did.
- A failing run falls under rule 2: the release is already public. Read the log (`gh run view <run-id> --log-failed`), report the failing check with the evidence, never edit, delete or re-upload the release, and stop to ask the maintainer. A failure here after a green draft gate means the published files differ from the draft or the runner changed: say which.

How to read the results: `docs/release.md`, "Linux smoke test", "macOS smoke test" and "Draft gate".

### 12. Record

A docs PR `chore/record-vX.Y.Z`, titled `docs(release): record vX.Y.Z [REL-010, REL-018]`: the README download text and links, the `docs/release.md` table row, the new `docs/plan/release-checklist.md` section from [checklist-template.md](checklist-template.md) (ticked only for what ran, including the draft gate of step 10 and the `Linux smoke` and `macOS smoke` runs of step 11: link, result, screenshots reviewed; after a resume, find the runs with `gh run list --workflow linux-smoke.yml` and `--workflow macos-smoke.yml`), the `CHANGELOG.md` heading date if the UTC publication date differs, and spec status lines that name the latest release or the signing state. Wait for green checks, merge, then `git worktree remove target/release-wt` and delete the local branches. Finish with a short report: version, links (PR, run, release, the draft-gate and published smoke runs for both platforms), evidence, what is still manual.

### Auto

`auto` skips Gate 1 and Gate 2 only when all of these hold; otherwise ask as usual: the channel (OS signing **and** `UPDATER`) equals the previous release's, `release-plan` gave no warnings, every check passed, `verify-update-artifacts` passed, the launch check ran and passed, the draft gate passed on every leg of both platforms, and the previous release was not withdrawn. It never skips a failure.

## Mode: status

Print a table: last release (tag, date, channel: OS signed or not, updater `none` or `all`, read from its notes as in step 0); unreleased commits (total, user-visible) and the computed `bump` and `next_version` from `cargo xtask release-plan`; open release PR, active run, draft; channel configuration (variables, including `UPDATER`, and secret **names** from `gh variable list` and `gh secret list --env release`; whether `keys.rs` holds a public key); and the signing phase reached ([signing.md](signing.md)). Change nothing.

## Mode: signing

Follow [signing.md](signing.md): detect the next phase of the one-time Windows signing setup, do the parts an agent may do, and tell the maintainer exactly what only they can do.
