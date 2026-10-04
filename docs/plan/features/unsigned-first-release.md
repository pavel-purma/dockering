# Plan: First unsigned cross-platform release

- **Slug:** `unsigned-first-release`
- **Status:** in-progress (release execution approved on 2026-10-04)
- **Spec:** [Distribution](../../spec/features/distribution.md)
- **Milestone:** First GitHub Release, `v0.1.0`
- **Requirement IDs:** REL-016, REL-017 (new); REL-010, REL-013, REL-030, REL-031, REL-041, REL-060, UPD-005 (changed)
- **Created:** 2026-10-04

## 1. Goal

Prepare and verify a GitHub Actions release pipeline that delivers unsigned installers and
archives for Windows, macOS, and Linux on x86_64 and ARM64. Obtain the user's approval before
pushing release changes, creating a tag, starting release builds, or publishing the first release.

## 2. Scope

Preparation includes the six-target workflow, explicit unsigned mode, complete asset validation,
checksums, update metadata, draft release notes, documentation, and local verification. Release
execution includes merging the prepared changes, tagging `v0.1.0`, monitoring builds, verifying
the draft assets, and publishing the GitHub Release after approval. Signing-provider enrollment,
key generation, winget submission, and enabling the updater are deferred.

## 3. Assumptions & open questions

| Assumption | Decision |
|---|---|
| First version | Keep the workspace's existing `0.1.0`; no releases or `v*` tags exist. |
| Platforms | All six supported OS/architecture combinations; 12 distribution files. |
| GitHub access | Repository is public; the current `gh` account has ADMIN permission and Actions are enabled. |
| Approval boundary | The user approved pushing, merging, tagging, building, verifying, and publishing v0.1.0 on 2026-10-04. |
| Unsigned stable release | User explicitly requested unsigned files; `0.1.0` may be published with unsigned-release notes. |
| Signing channel | Leave `PUBLIC_RELEASES`, signing configuration, updater keys, and winget disabled for this release. |

## 4. Requirements

| ID | Requirement | Change |
|---|---|---|
| REL-016 | Explicit unsigned releases MAY be published after maintainer approval. Unsigned mode MUST skip signing, notarization, and the updater regardless of available secrets. Branch dispatches MUST produce review artifacts only; tag builds MUST create a draft. | New |
| REL-017 | Release assembly MUST reject missing, empty, or duplicate distribution files and require all 12 stable names before producing checksums, metadata, and a release. | New |
| REL-010 | Preserve release-plz and manual-tag entry points; support a full branch dry run. | Changed |
| REL-013 | Public-repository build provenance is independent of code signing. | Changed |
| REL-030, REL-031 | Signed-release requirements continue to apply to the signed channel; REL-016 is the explicit unsigned exception. | Changed |
| REL-041, REL-060, UPD-005 | Public repository visibility alone does not enable signing, winget, or the updater. | Changed |

## 5. Engine contract impact

None. Engine APIs, capabilities, hub threading, application UI, and keyboard bindings do not change.

## 6. Design

Tag push or manual dispatch → verify version/changelog and select unsigned/signed mode → package
all six targets → validate and collect all distribution files → generate update metadata and
SHA256SUMS → attest assets in the public repository → save a review bundle. A tag run also creates
a draft release. Publication remains a separate, explicit action.

`PUBLIC_RELEASES=true` remains the existing switch for the configured signed/updater channel.
An unsigned dispatch overrides that configuration. The unset default supports this first release
without repository secrets or a release-bot App; the maintainer can push the tag using `gh`/Git.

## 7. Tasks

| # | Task | Owner | IDs | Verification |
|---|---|---|---|---|
| 1 | Record the unsigned exception and update the runbook, README, first-release notes, and checklist. | release-engineer / coordinator | REL-016, REL-060 | Review links and exact commands. |
| 2 | Prepare unsigned mode and draft/dry-run assembly in release.yml; guard winget. | release-engineer | REL-010, REL-013, REL-016, REL-041 | actionlint, YAML parsing, inspect all signing conditions. |
| 3 | Require the complete distribution asset set. | release-engineer | REL-017 | xtask tests for complete, missing, empty, and invalid-target inputs. |
| 4 | Verify locally and inspect existing six-platform CI. | release-engineer / coordinator | All | fmt, clippy for xtask, relevant tests; record limits. |
| 5 | After approval, commit/PR/merge, tag, monitor, inspect draft, and publish. | release-engineer / coordinator | REL-010, REL-016 | Green six-target release run, 12 distributions, valid SHA256SUMS, release URL. |

## 8. Test plan

| Requirement | Evidence |
|---|---|
| REL-016 | Workflow syntax and condition review; GitHub branch dry run/tag run after execution approval. |
| REL-017 | Unit tests for each target and the combined set; missing and empty file rejection. |
| REL-010, REL-013 | Existing six-target CI and post-approval release run; draft assets and provenance. |

## 9. Risks and limits

| Risk | Handling |
|---|---|
| Unsigned executables trigger OS trust prompts. | Label the release and document Windows/macOS installation behavior. |
| Local Windows cannot prove macOS/Linux packaging. | Preceding CI passed all six builds/packages. Latest CI failed only at Intel DMG eject with Resource busy; a narrow bounded retry is prepared and mocked locally. Real runner verification awaits approval. |
| No release bot App or signing secrets are configured. | Use the maintainer's manual tag for the first release; keep these optional. |
| Rerun could target an existing published release. | The verify stage checks release state and refuses already-published tags. |
| First release has no production manual smoke-test history. | Use installer CI smoke tests and the concrete release checklist; report remaining manual checks. |

## 10. Revision log

- 2026-10-04: Created for the user's authorized preparation request; release execution remains pending.

## Preparation verification

- actionlint passes all repository workflows; Rust formatting and xtask clippy pass.
- Seven mocked macOS recovery tests pass: success, owned busy-image retry, unrelated/different-disk
  refusal, ordinary failure, cleanup failure, and retry exhaustion. CI runs these checks.
- All 20 xtask tests pass, including four complete-asset validation tests.
- Nine local embedded verify-script cases cover branch/tag success, version/changelog errors,
  missing signed configuration, configured signed mode, and explicit unsigned overrides.
- Exact workflow assembly with 12 package fixtures produces six manifest platforms and 15 files;
  checksums and unsigned notes verify, and duplicate collection fails as required.
- No preparation changes, tags, workflow runs, or releases were pushed or published.
- GitHub build/provenance/publication verification is in progress under the user's execution approval.

- 2026-10-04: User approved execution and publication of the first `v0.1.0` release.
