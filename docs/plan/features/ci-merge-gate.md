# Plan: CI as the merge gate for pull requests

- **Slug:** `ci-merge-gate`
- **Status:** done (2026-10-11)
- **Spec:** [Build & Release, Merge gate](../../spec/50-build-and-release.md#merge-gate-rel-080082)
- **Milestone:** after the unsigned v0.1.0/v0.2.0 releases (release tooling)
- **Requirement IDs:** REL-080, REL-081, REL-082 (new); REL-010 (clarified)
- **Created:** 2026-10-11

## 1. Goal

A pull request cannot merge into `main` until CI is green and its title is a conventional commit.
Today the workflows exist and run, but nothing enforces them: the `Protect main` ruleset requires a
pull request and squash merges, and no status check. #38 and #39 merged with a red `lint` job, so
`main` has failed `cargo fmt --check` since `87b450b`, and every later PR's CI inherits it.

## 2. Scope

**In:** format `crates/dockering/src/view_tests_kbd.rs` so `main` is green; a `gate` aggregate job in
`ci.yml` and a script test that keeps it complete; required status checks in the `Protect main`
ruleset; the spec, release skill and release guide text that says GitHub does not require checks.

**Out:** merge queue; requiring the Linux/macOS smoke tests (they are path-triggered and not always
reported, REL-082); self-hosted `wsl-integration` (manual, never on PR code); auto-merge; CODEOWNERS
or required approvals (a solo maintainer; the ruleset keeps `required_approving_review_count: 0`);
fixing flaky tests beyond the rule that a flake is read, rerun and reported (see risks).

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | **Which checks gate.** `CI gate` (aggregates `lint`, `test` ×3, `integration-docker`, `build` ×6) and `conventional commit title`. | Both, as above. |
| 2 | **Up-to-date branch** (`strict_required_status_checks_policy`). On, every merge reruns CI after a rebase (the Windows legs take about 12 minutes) and `allow_update_branch` is off. Off, a semantic conflict between two green PRs can turn `main` red; the post-merge `push` run still reports it. | **Off.** |
| 3 | **Bypass actors.** Decided 2026-10-11: only the maintainer may bypass, and only from the PR UI. A personal repository cannot name a user in a ruleset, so the Admin repository role (`actor_id` 5) is listed with `bypass_mode: pull_request`; `pavel-purma` is its only member. The button lets a PR merge past a red check; a direct push to `main` stays blocked. Applied to the live ruleset 2026-10-11. | **Admin role, pull-request mode.** |
| 4 | **Two PRs.** The formatting fix lands first and alone, so the gate PR is itself green and `main` is green before anything is required. | **Two PRs.** |
| 5 | The ruleset is a repository setting, not a file. The change is made with `gh api` after PR 2 merges, with the maintainer's go-ahead, and is verified, not committed. | As described in §6.3. |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| REL-080 | The `Protect main` ruleset requires `CI gate` (`ci.yml`) and `conventional commit title` (`pr-title.yml`), from the GitHub Actions app; squash-only and pull-request rules stay; the only bypass is the Admin role in pull-request mode (the maintainer, from the PR UI); branches need not be up to date. No other workflow gates a PR. | New |
| REL-081 | `ci.yml` has one job `gate` (`CI gate`) that `needs` every other job, runs `if: always()` and fails unless every needed job succeeded (failed, cancelled and skipped all fail). Matrix changes never need a ruleset change. | New |
| REL-082 | `ci.yml` and `pr-title.yml` run on every pull request: no `paths`/`paths-ignore` on `pull_request`, no job-level `if` in `ci.yml`. | New |
| REL-010 | "enforced by a PR-title check" becomes "enforced by a required PR-title check" (REL-080). | Clarified |

## 5. Engine contract impact

None. No engine, capability, hub, UI or keyboard change.

## 6. Design

### 6.1 The `gate` job

Appended to `ci.yml` (away from the `lint` steps that #37 edits):

```yaml
  gate:
    name: CI gate
    if: always()
    needs: [lint, test, integration-docker, build]
    runs-on: ubuntu-24.04
    steps:
      - name: Every CI job succeeded
        env:
          RESULTS: ${{ join(needs.*.result, ' ') }}
        run: |
          echo "needs: $RESULTS"
          for r in $RESULTS; do [ "$r" = success ] || { echo "::error::a CI job did not succeed"; exit 1; }; done
```

Notes: `always()` is needed because a skipped required check counts as passing, and a plain `needs`
would skip the gate when a job fails. The loop is plain POSIX (no `jq`) so the step runs unchanged
in the script test on every OS. A comment above the job says new jobs MUST be added to `needs`.
`scripts/tests/test_ci_gate.py` (already discovered by the `lint` and macOS `test` steps) enforces that:

- `gate` exists, is named `CI gate`, has `if: always()`, and its `needs` equals every other job in `ci.yml`;
- the step's script (extracted as `test_release_verify.py` does) exits 0 for `success success` and non-zero
  for `success failure`, `success cancelled`, `success skipped` and an empty list;
- `ci.yml` has no job-level `if` and neither workflow filters `pull_request` by `paths`/`paths-ignore`;
- the required names in the spec (`CI gate`, `conventional commit title`) are the workflows' job names.

### 6.2 The formatting fix

`cargo fmt --all` rewrites seven hunks in `crates/dockering/src/view_tests_kbd.rs` and nothing else
(checked 2026-10-11 against `112ecff`, toolchain 1.98). No behaviour change. Title
`style(ui): format the keyboard view tests`.

### 6.3 The ruleset

`PUT` replaces the whole ruleset, so build the body from the live one and add a rule:

```sh
gh api repos/pavel-purma/dockering/rulesets/24414791 \
  | jq '{name, target, enforcement, conditions, bypass_actors,
         rules: (.rules + [{type: "required_status_checks", parameters: {
           strict_required_status_checks_policy: false,
           do_not_enforce_on_create: false,
           required_status_checks: [
             {context: "CI gate", integration_id: 15368},
             {context: "conventional commit title", integration_id: 15368}]}}])}' > protect-main.json
# review protect-main.json, then:
gh api --method PUT repos/pavel-purma/dockering/rulesets/24414791 --input protect-main.json
```

`15368` is the GitHub Actions app (read from the check runs of #39), so another app cannot satisfy
the check. Rollback: the same call without the `required_status_checks` rule (the other three rules are
unchanged). Run only after PR 2 has merged, so `CI gate` has reported on `main`, and only with the
maintainer's go-ahead.

### 6.4 Text that changes

- `docs/spec/50-build-and-release.md`: a `gate` row and a *Merge gate* section (written now, status planned).
- `docs/spec/60-quality.md`: Definition of Done item 3 names the gate.
- `.agents/skills/release/SKILL.md` rule 10 and `docs/release.md` steps 2–3: `main` requires `CI gate` and
  `conventional commit title`, GitHub refuses the merge until they pass, only the maintainer can bypass (from the PR UI), and the skill
  still reads a failed check's log (rule 8) before rerunning it.

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | PR 1: `cargo fmt --all`, commit only `view_tests_kbd.rs`. | release-engineer | — | `cargo fmt --all --check`; CI `lint` green on the PR; merge |
| 2 | PR 2: add the `gate` job to `ci.yml`. | release-engineer | REL-081, REL-082 | `actionlint`; the PR's own run shows `CI gate` green and red when a job is made to fail locally in a scratch branch |
| 3 | PR 2: `scripts/tests/test_ci_gate.py` (§6.1). | qa-engineer | REL-081, REL-082 | `python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'` on Windows and in CI |
| 4 | PR 2: the §6.4 text changes (spec status stays `planned` until task 5). | release-engineer | REL-080, REL-010 | reread against the spec; `grep -rn "does not require green"` finds nothing |
| 5 | After PR 2 merges and the maintainer agrees: apply §6.3. | release-engineer | REL-080 | `gh api repos/pavel-purma/dockering/rules/branches/main` lists both contexts; a throwaway draft PR with a formatting error shows `mergeStateStatus: BLOCKED` and `gh pr merge` refuses; close it unmerged |
| 6 | Review | reviewer | all | verdict: approve |
| 7 | `/feature-planning complete ci-merge-gate` (spec rows to `implemented`, with the evidence) | architect | REL-080…082 | status table |

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| REL-081 | script test | `test_rel_081_gate_needs_every_job`, `test_rel_081_gate_runs_always`, `test_rel_081_gate_step_requires_success` |
| REL-082 | script test | `test_rel_082_no_job_level_if`, `test_rel_082_no_pull_request_path_filters` |
| REL-080 | script test + live | `test_rel_080_required_names_are_job_names`; task 5's `gh api` listing and the blocked canary PR |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| A flaky test now blocks merges. Recent history on `main` has one non-formatting failure, `net_004_prune_unused_networks` on ubuntu (2026-10-06). | Read the log, `gh run rerun --failed`, and fix the test in its own PR; `docs/release.md` already forbids waving a red test through. |
| A runner is not acquired (`windows-11-arm`, `ubuntu-24.04-arm`, `macos-15-intel`): `CI gate` stays red. | Existing "CI incidents" step: check githubstatus.com, rerun the failed jobs; the gate reruns with them. |
| A new `ci.yml` job is forgotten in `needs`. | `test_rel_081_gate_needs_every_job` fails in `lint`. |
| A workflow-level `paths` filter is added later, so a required check never reports. | `test_rel_082_*`. |
| Open PRs (#37) change under their feet: the rule applies to them at once. | Their CI already runs on the merge ref; they need `main` green (task 1) and the gate job on their merge ref, which it is once PR 2 is on `main`. |
| Textual overlap with #37 (`ci.yml`, spec 50, 60, `release.md`, release skill). | The edits sit in different hunks (end of `ci.yml`, after the `build` row, DoD item 3, rule 10, steps 2–3); rebase whichever merges second. |
| Mid-flight, the release skill text is true only once task 5 is done. | Tasks 4 and 5 were done in one sitting (PR #41, then the ruleset), so the text was true within minutes. |

## 10. Revision log

- 2026-10-11: created
- 2026-10-11: implemented and completed. PR #40 (format fix), PR #41 (`gate` job, test, docs), then the
  `required_status_checks` rule on `Protect main` with the Admin pull-request bypass kept. Canary PR #42
  (deliberate `cargo fmt` error) failed `CI gate` and `gh pr merge` was refused; closed unmerged. The bypass
  button was not exercised. Spec rows moved from planned to implemented.
