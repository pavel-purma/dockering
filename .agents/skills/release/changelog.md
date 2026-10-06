# Writing the changelogs (REL-011, REL-019)

Both changelogs are written only by the release skill, in the release PR (step 3). Feature PRs
never touch them. The input is `cargo xtask release-plan` (add `--bodies` for the commit bodies)
plus, for each commit with a PR, `gh pr view <n> --json title,body,files`. Read the PR text and, if
it is unclear what a user sees, the diff.

## `CHANGELOG.md`

Keep a Changelog. The new section goes **above** the newest `## [` heading, with a blank line
before and after:

```
## [X.Y.Z] - YYYY-MM-DD

<optional intro: at most three short sentences>

### Added

- …

### Changed

- …

### Fixed

- …
```

- **Date:** the current UTC date. The record PR (step 12) corrects it if the publication lands on a later UTC date.
- **Intro:** only when users need it: the channel (*Downloads are unsigned, as in <previous release>.*, repeating *Automatic updates and winget remain deferred.* while that is true; the first signed release says that Windows files are signed and that macOS stays unsigned), a withdrawn build this one replaces, or an upgrade note.
- **Sections** (use only those that have entries, in this order): *Added* (`feat`), *Changed* (`perf`, `refactor`, `revert`, other user-visible commits), *Deprecated*, *Removed*, *Fixed* (`fix`), *Security*. A breaking change starts with `**Breaking:**` in the section it belongs to.
- **Entries:** one per user-visible change, in the user's words ("The status bar names the active engine"), not the commit's. Describe the difference from the previous *published* release, never text that no release showed. Present tense. One commit can give several entries (a repair PR often fixes several visible things); several commits that make one change give one. Name the platform or engine when it matters (*Windows:*, *WSLC:*). Hardening users can't observe (cancellation, cleanup) gets one combined entry per PR. Don't claim more than the spec does: read the requirement before listing what is or isn't retried. No commit hashes, PR numbers or requirement IDs. Wrap at about 100 columns with two-space continuation indents, like the existing sections.
- **Skip** `docs`, `test`, `chore`, `ci`, `style` and `build` commits and commits scoped `ci`, `release`, `docs`, `spec` or `deps`, unless the change is visible in the app or the downloads (then it is a normal entry).
- **Skip fixes for defects users never had:** a `fix` for something introduced after the last release tag (the range's own `feat` or `fix` commits, or a build that was withdrawn) gets no entry, because no published version has the bug. If a withdrawn build shipped it, say so in the intro instead ("This build replaces a first 0.2.0 build that was withdrawn: it crashed at launch after an upgrade from 0.1.0").
- Never add an *Unreleased* section.

### Bump implied by the sections

| Sections | Bump while `0.x` | From `1.0` |
|---|---|---|
| a **Breaking** mark, or *Removed* | minor | major |
| *Added* | minor | minor |
| only *Changed*, *Deprecated*, *Fixed*, *Security* | patch | patch |

This must equal the `bump` that `release-plan` computed from the commits. A mismatch means an entry
was put in the wrong section or a commit was classified wrongly: fix that, don't override the bump.

## `docs/spec/CHANGELOG.md`

One line per merged change that touched the spec (`spec_files` is not empty: any file under
`docs/spec/` except the changelog itself), whether or not users see it (a `fix(ci)` that edits
`50-build-and-release.md` gets a line), newest first,
in the format already used at the top of the file:

```
- YYYY-MM-DD · <area> · <summary> · [plan](../plan/features/<slug>.md)
```

- **Date:** the commit date of the change, so lines stay in chronological order. A new line goes above older ones, by date.
- **Area:** the feature spec's name (`containers`, `engines`, `distribution`, …), `a/b` (at most three) when several are touched, `release` for `distribution.md`, `ci` for `50-build-and-release.md`, `architecture` for `10-architecture.md`, `engines` for specs 20 and 21, `ui` for `30-ui-shell.md`. Ignore `docs/spec/README.md`, which only carries status lines.
- **Summary:** what the requirement text now says: requirement IDs added, changed or struck, status changes, contract changes. Read the diff of the spec files and the PR text. Say what changed in the spec, not how the code was written.
- **Link:** `[plan](../plan/features/<slug>.md)` for a slug in the commit's `plans` (with two, the one the commit created; `release-checklist.md` is not a plan); otherwise `[PR #n](https://github.com/pavel-purma/dockering/pull/n)`.
- Several commits of one plan become one line. Never duplicate a line that is already in the file (search for the PR number, the plan slug and the requirement IDs first: older hand-written lines may carry none of the first two).
- A release that only adds docs outside `docs/spec/**` adds nothing here.

## Coverage table (in the PR body)

| PR | Commit | `CHANGELOG.md` | Spec changelog |
|---|---|---|---|
| #26 | fix(ci): retry macOS DMG packaging… | skipped: scope `ci` | line: ci · 50-build-and-release |
| #35 | fix(ui): push the post-update notice… | no entry: fixes a withdrawn build (intro) | line: release · distribution |
| #28 | docs(release): record v0.1.0… | skipped: internal (`docs`) | line: release · distribution |
| #29 | fix(wslc): repair COM/CLI fallback… | Fixed ×4, Changed ×1 | line: engines/WSLC/images/container-detail |
| #31 | feat(ui): name engines… | Added ×1 | line: engines/ui |
| #32 | test(wslc): gate ConPTY fixtures… | skipped: internal (`test`) | — (no spec files) |

Every commit since the last release tag appears exactly once. A user-visible commit without an
entry, or a commit that touched the spec without a line, is a defect.
