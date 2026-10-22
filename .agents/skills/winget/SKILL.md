---
name: winget
description: Publish a released Dockering version to winget. Reads the latest GitHub release, renders the winget manifests from the templates in packaging/winget/, checks them against the release, and opens the pull request in microsoft/winget-pkgs from the maintainer's fork. Also reports where winget publishing stands and follows a pull request through validation. Use when the user asks to publish, submit or release to winget ("/winget", "/winget v0.3.0", "/winget status", "/winget pr 123456").
---

# winget

One command takes a **published** GitHub release of Dockering to a pull request in
[`microsoft/winget-pkgs`](https://github.com/microsoft/winget-pkgs) (REL-040, REL-041). The scripts do the steps, so a run is the same
every time; this skill adds the order, the gates and the judgement. The manifest templates are `packaging/winget/manifests/`
(package `Dockering.Dockering`); background and the CI automation for later releases: `packaging/winget/README.md`.

| Invocation | Mode |
|---|---|
| `/winget [vX.Y.Z]` | **submit**: the latest published release, or the tag, to winget-pkgs |
| `/winget status` | **status**: the release, winget-pkgs, the clone, access. Changes nothing |
| `/winget pr <number>` | **follow**: state, labels and comments of a winget-pkgs pull request |

Host tools: `AskUserQuestion` in Claude Code is `question` in OpenCode.

## The scripts

All are PowerShell 7 (`pwsh -NoProfile -File <script>`), print one `[PASS|WARN|INFO|SKIP|FAIL] <id>  <detail>` line per check and end with
`KEY value` lines. Exit 0 means done; 1 means a check failed (read the `[FAIL]` line, never work around it).

| Script | What it does |
|---|---|
| `scripts/winget/Get-Status.ps1 [-Tag] [-Pr <n>]` | Read-only. Latest release, what winget-pkgs holds, open pull requests, `gh` access, the clone. Ends with `NEXT submit-new-package \| submit-new-version \| wait-for-pr \| none`; with `-Pr`, `PR_STATE merged \| closed \| needs-author-feedback \| open` |
| `scripts/winget/New-Manifest.ps1 [-Tag] [-OutDir]` | Renders the three manifests into `target/winget/<version>/manifests/d/Dockering/Dockering/<version>/`. Checks that the release is published and stable, that `SHA256SUMS` equals GitHub's own digest of each installer, that each installer downloads from the public URL with that SHA-256, and `winget validate`. Ends with `VERSION`, `CHANNEL`, `MANIFEST_DIR` |
| `scripts/winget/Submit-Manifest.ps1 [-Submit] [-InstallTested] [-Attribution] [-CommitAttribution]` | Creates or refreshes the clone, branches from the tip of winget-pkgs, copies the manifests in, commits and writes the pull request body. Only with `-Submit`: forks, pushes the branch to the fork and opens the pull request. Ends with `BRANCH`, `COMMIT`, `TITLE`, `BODY_FILE`, `PR_URL` |

Every script takes `-Repo owner/name` (default `pavel-purma/dockering`), and the first two take `-Tag`. They never run an installer.

## Rules

1. Never push to `microsoft/winget-pkgs`. Only to the maintainer's fork, and never with a force. A pull request is opened only after Gate 1.
2. Never edit rendered manifests by hand. A wrong manifest is fixed in the template (`packaging/winget/manifests/`), through a normal PR in this repository, and rendered again. The exception is a moderator's change request on an open pull request: see "Follow".
3. Never install or run the Dockering installer, and never `winget install --manifest`, on this machine: it replaces the maintainer's own installation, and `winget settings --enable LocalManifestFiles` needs an elevated prompt. An install test is done in Windows Sandbox or a VM by the maintainer, never by you.
4. Never tick a pull request box that was not verified. The scripts tick what they checked. "Tested manifest locally" stays open unless the maintainer reports an install test (`-InstallTested`). The Contributor License Agreement box is never ticked: only the maintainer can sign it.
5. Never print, read back or store a token. `gh` uses the maintainer's own login; `WINGET_TOKEN` belongs to the CI workflow, not to this skill.
6. Never open a second pull request for a package or version that already has one open, and never resubmit a version winget-pkgs already holds. `Get-Status.ps1` and `Submit-Manifest.ps1` stop on both.
7. Never submit a draft or a pre-release. Stable `vX.Y.Z` tags only.
8. An **unsigned** release (the notes say *Unsigned release*) is submitted only after the maintainer has said so at Gate 1. REL-041 allows no automatic submission of unsigned builds and asks for the first submission after a signed release; a maintainer-run submission with explicit approval is the exception this skill implements.
9. The winget-pkgs clone lives in one place (below) and is touched by nothing but these scripts. Never delete it, and never put other repositories in it. Look at what is there before removing anything.
10. Anything you would write to winget-pkgs in the maintainer's name beyond the pull request itself (a reply to a moderator, a comment) is shown to the maintainer first.

## Where the clone goes

The winget-pkgs clone is **`<root of the drive that holds this worktree>\repos\winget-pkgs`**: a worktree in `D:\worktrees\dockering\<name>` uses
`D:\repos\winget-pkgs`, one on `E:` uses `E:\repos\winget-pkgs`, so the clone follows the worktree when the machine has several disks. Off Windows
it is `~/repos/winget-pkgs`. `Get-DefaultCloneDir` in `scripts/winget/Lib.ps1` computes it; `-CloneDir` overrides it.

The clone is **blobless, depth 1 and sparse** (`manifests/d/Dockering`, `.github`, `Tools`): the full repository has more than 700,000 files and
a plain clone takes many minutes and gigabytes. The remote `upstream` is `microsoft/winget-pkgs`; the remote `fork` is added when submitting.
Don't run `git fetch --unshallow`, `sparse-checkout disable` or a full `git clone` there.

## Mode: submit

### 0. Preflight and status

Run `Get-Status.ps1` (add `-Tag` for an older release). Tools: `gh` (logged in, scopes `repo`), `git`, `pwsh` 7.3+, and `winget` (for `winget validate`).

Act on the `NEXT` line:

| NEXT | Do |
|---|---|
| `submit-new-package` | the first submission: continue |
| `submit-new-version` | the package exists, this version doesn't: continue |
| `wait-for-pr` | stop. Show the open pull request and switch to **follow** |
| `none` | stop: winget-pkgs already has this version |

Read the `WARN` lines. `upstream.access` is about **SAML SSO**: the Microsoft organisation blocks every `gh` call to winget-pkgs until the maintainer
authorises the token for it, in a browser (the line prints the link; it is the maintainer's account, so only they can). The rendering and the local
commit don't need it; only `-Submit` does. Tell the maintainer now so it is done before Gate 1.

### 1. Render

`New-Manifest.ps1`. A `[FAIL]` stops the run: a checksum that differs between `SHA256SUMS` and GitHub's digest, an installer that downloads with
another hash, a missing asset, a `winget validate` error. Report it; don't patch the manifest (rule 2).

Read the `release.channel` line: **unsigned** is a `WARN` the maintainer decides on at Gate 1 (rule 8).

### 2. Install test (the maintainer's, optional)

Dockering's own installer is Inno Setup, so the interesting test is `winget install --manifest <dir>` in a clean Windows Sandbox or VM, for both scopes
(`--scope user`, then `--scope machine`), then `winget uninstall Dockering.Dockering`. The clone has Microsoft's helper:
`<clone>\Tools\SandboxTest.ps1 <manifest dir>` (needs the Windows Sandbox feature; this is the maintainer's to run). Ask whether it was done. If yes,
pass `-InstallTested` so the pull request says so; if no, the box stays open and the moderators' own pipeline tests the install anyway.

### 3. Local commit and Gate 1

`Submit-Manifest.ps1` **without** `-Submit` (add `-InstallTested` per step 2). It creates or refreshes the clone, branches from the tip of winget-pkgs
as `Dockering.Dockering-<version>`, commits, and writes `target/winget/<version>/pr-body.md`. Nothing leaves the machine.

**Gate 1.** Show: the identifier and version, the release link, the channel (and, for an unsigned one, that REL-041 asks for a signed release and this
is the maintainer's explicit exception), the three rendered manifests, the commit title, the pull request body with its ticked and open boxes, and
the `gh` access state. Ask: submit / stop. `AskUserQuestion` for this; never assume yes.

### 4. Submit

`Submit-Manifest.ps1 -Submit`, with the same flags, plus `-CommitAttribution` and `-Attribution` set to the host's attribution lines when the host
requires them (they end the commit message and the pull request body). It creates the fork if there is none, brings the fork's `master` up to date,
pushes the branch and opens the pull request. Use the printed `PR_URL`.

On failure: `gh.access` is the SSO case above (authorise, run again). `push` with *shallow update not allowed* means the fork's `master` is behind:
`gh repo sync <login>/winget-pkgs --branch master`, then run again. A branch left by an earlier attempt: pass another `-Branch`; never force.

### 5. Report

Pull request link, branch, commit, title, which boxes are ticked and which are open (the CLA, always), and what happens next: the validation pipeline
runs by itself within minutes to hours, a moderator reviews, and a merge makes `winget install Dockering.Dockering` work after the source refreshes.
Say that the maintainer has to **sign the CLA** at https://cla.opensource.microsoft.com if they haven't (the bot adds `Needs-CLA` until then).

## Mode: follow

`Get-Status.ps1 -Pr <number>` shows state, labels and comment count; read the comments with `gh pr view <number> --repo microsoft/winget-pkgs --comments`
(needs the SSO authorisation) or in the browser. Labels (Microsoft's `doc/ValidationFailureGuide.md` has the full list):

| Label / state | Meaning and action |
|---|---|
| `Azure-Pipeline-Passed`, `Validation-Completed` | passed; wait for a moderator or the merge |
| `Needs-CLA` | the maintainer signs the CLA; nothing to change |
| `Needs-Author-Feedback` | something needs the author within 5 days (then `No-Recent-Activity`, and a close 3 days later). Read the comments, report, and fix as below |
| `Manifest-Validation-Error`, `Validation-*-Error`, `URL-*` | the pipeline found a fault: read the bot's comment and the check's log, report the cause |
| `Blocking-Issue`, `Policy-*`, `Validation-Domain`, `Validation-Executable-Error` | needs a Windows Package Manager administrator; waiting and answering questions is all that can be done |
| merged | go to "After the merge" |
| closed, not merged | report why; a resubmission is a new run of this skill |

**Fixing a pull request.** If the fault is in the manifest content, fix the template here (rule 2), then in the clone commit the corrected files
on the same branch and push it normally: `git -C <clone> switch <branch>`, edit or copy the rendered files, `git -C <clone> add -- <dir>`, `git -C <clone> commit -m "<what>"`,
`git -C <clone> push fork <branch>` (a new commit, never a force). A reply to a moderator is shown to the maintainer first (rule 10).

## After the merge

winget-pkgs updates its index shortly after the merge. Check `winget search Dockering.Dockering` (or `winget show Dockering.Dockering`) once. Then, only if
the maintainer asks, a separate docs PR in this repository (conventional commit, `docs(...)`) turns the "winget (planned)" text into an install command:
`README.md` (the commented badge, the Download table cell, the Install on Windows bullet), and `docs/release.md` / `packaging/winget/README.md` where
they say the first submission is still to do. A spec change goes through the feature-planning skill. Later stable releases can then use the
`winget` workflow (`WINGET_ENABLED`, `WINGET_TOKEN`; `packaging/winget/README.md`): that is the maintainer's switch.

## Mode: status

`Get-Status.ps1`, shown as a short table: latest release (tag, date, signed or not, assets present), what winget-pkgs holds (package, versions, open pull
requests), `gh` access to the Microsoft organisation, the clone (path, exists), and the `NEXT` action. Change nothing.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `Resource protected by organization SAML enforcement` | `gh` is not authorised for the `microsoft` organisation: the maintainer opens the link `Get-Status.ps1` prints. The unauthenticated reads the scripts do themselves still work |
| HTTP 403 from the scripts' own API calls | the unauthenticated rate limit (60 requests an hour, 10 searches a minute): wait; the message names the reset |
| `shallow update not allowed` on push | the fork's `master` is behind: `gh repo sync <login>/winget-pkgs --branch master` |
| `clone ... has uncommitted changes` | someone worked in the clone: look at `git -C <clone> status` and clean it up yourself; the script never discards changes |
| `SHA256SUMS says X, GitHub's digest says Y` | the release asset was replaced after the checksums were written: stop and report; never edit or re-upload a published release (release skill, rule 2) |
