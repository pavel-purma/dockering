# Plan: First unsigned cross-platform release

- **Slug:** `unsigned-first-release`
- **Status:** done (v0.1.0 published and verified on 2026-10-04)
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
| First version | Keep the workspace's existing `0.1.0`; `v0.1.0` targets the corrected release commit recorded below. |
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
| Local Windows cannot prove macOS/Linux application launch. | Exact-commit CI passed all six package jobs, including macOS; eight recovery/parse tests pass under native macOS Bash 3.2. Physical macOS/Linux GUI checks remain unverified. |
| No release bot App or signing secrets are configured. | Use the maintainer's manual tag for the first release; keep these optional. |
| Rerun could target an existing published release. | The verify stage checks release state and refuses already-published tags. |
| First release has no production manual smoke-test history. | Use installer CI smoke tests and the concrete release checklist; report remaining manual checks. |

## 10. Revision log

- 2026-10-04: Created for the user's authorized preparation request, before execution approval.
- 2026-10-04: User approved execution and publication. Merged preparation [PR #24](https://github.com/pavel-purma/dockering/pull/24), test-readiness fix [PR #25](https://github.com/pavel-purma/dockering/pull/25), and native Bash compatibility/asset-validation fix [PR #27](https://github.com/pavel-purma/dockering/pull/27).
- 2026-10-04: Initial tag run [37227501895](https://github.com/pavel-purma/dockering/actions/runs/37227501895) caught a missing macOS DMG after Bash 3.2 rejected the packaging wrapper. The obsolete Intel build was canceled. After native Bash 3.2 recovery/syntax tests and local actionlint passed, the unpublished tag was repointed with a lease to the corrected commit. Main CI and the replacement tag build then ran in parallel; exact-commit CI was green before publication.
- 2026-10-04: Verified the complete release and published [v0.1.0](https://github.com/pavel-purma/dockering/releases/tag/v0.1.0) as stable/latest at `2026-10-04T20:09:38Z`. Scoped plan complete; signed-channel activation remains deferred.

## Preparation verification

- actionlint passes all repository workflows; Rust formatting and xtask clippy pass.
- Eight macOS packaging tests pass: parsing, success, owned busy-image retry, unrelated/different-disk
  refusal, ordinary failure, cleanup failure, and retry exhaustion. Exact-commit CI runs them under
  native macOS `/bin/bash` 3.2 and also validates all generated target package files.
- All 20 xtask tests pass, including four complete-asset validation tests.
- Nine local embedded verify-script cases cover branch/tag success, version/changelog errors,
  missing signed configuration, configured signed mode, and explicit unsigned overrides.
- Exact workflow assembly with 12 package fixtures produces six manifest platforms and 15 files;
  checksums and unsigned notes verify, and duplicate collection fails as required.
- Preparation and fixes are merged. GitHub release verification and publication completed
  under the user's execution approval, with the evidence below.

## Release evidence

| Check | Result / evidence |
|---|---|
| Exact release commit | [`041d8fac06f1a2d26a61ed5e13ad16f47371f628`](https://github.com/pavel-purma/dockering/commit/041d8fac06f1a2d26a61ed5e13ad16f47371f628). |
| Exact-commit CI | [37228704953](https://github.com/pavel-purma/dockering/actions/runs/37228704953), successful: six builds/packages, tests on Windows/macOS/Linux, lint, and Docker integration. |
| Native macOS shell compatibility | CI syntax check plus all eight recovery tests passed under `/bin/bash` 3.2. |
| Tag release run | [37228719100](https://github.com/pavel-purma/dockering/actions/runs/37228719100), successful at the exact release commit: all six package jobs, assembly/provenance, and draft creation. |
| Downloaded Windows packages | Both portable PE architectures match their targets. Installers and portable executables report `NotSigned`; portable packages include license/notices. Downloaded x64 `--version` prints `dockering 0.1.0`. |
| Downloaded Linux packages | ELF architectures match their targets, including the payload inside each Debian package; Debian metadata has version `0.1.0` and the correct architecture; tar archives include license/notices. |
| Downloaded macOS packages | Both DMGs are nonempty and have the UDIF `koly` trailer. Actual mount/application launch remains unverified. |
| Complete published asset set | Downloaded exact set of 12 distributions plus manifest, notes, and `SHA256SUMS` (15 files). No `.minisig` is present. |
| Checksums and manifest | All 14 `SHA256SUMS` entries match. Manifest schema/version and all six platform entries have the correct asset filename, kind, hash, size, version-pinned URL, and notes URL. |
| Build provenance | All 15 `gh attestation verify` checks passed with source digest pinned to the release SHA, source ref `refs/tags/v0.1.0`, `release.yml` signer, and self-hosted runners denied. |
| Publication | [Dockering 0.1.0 (unsigned)](https://github.com/pavel-purma/dockering/releases/tag/v0.1.0), published `2026-10-04T20:09:38Z`; `draft=false`, `prerelease=false`, latest API returns this tag and all 15 files. |
| Public download links | Anonymous requests follow redirects to HTTP 200 for all 15 version-pinned asset URLs and all 12 permanent latest distribution links. |
| Channel configuration | Repository variable list remains empty; signing/updater/winget channel variables are unset. |
| Manual application walkthrough | Not performed: Windows GUI/engine lifecycle, physical macOS/Linux launch, and WSL/WSLC integration. See [checklist](../release-checklist.md). |

## Requirement reconciliation

| ID | Implemented? | Verification | Remaining work |
|---|---|---|---|
| REL-010 | Yes, manual-tag and review/draft paths | Version/ref tests, successful tag build, explicit stable/latest publication | Release-bot App setup deferred. |
| REL-013 | Yes, secret isolation and public provenance | Workflow review; all 15 attestations verified with source/tag/workflow constraints | No scoped release work remaining. |
| REL-016 | Yes, explicit unsigned mode | Nine verify-script cases; Windows executables `NotSigned`; no `.minisig`; updater/signing gates; published unsigned notes | No scoped release work remaining. |
| REL-017 | Yes, complete-asset validation | Four xtask tests, duplicate rehearsal, six CI package validations; actual 15-file set, 14 checksums and six-platform manifest verified | No scoped release work remaining. |
| REL-030, REL-031 | Yes for unsigned exemption and preserved signed-channel guards | Condition review and signed-config failure cases | Live signing/notarization setup and execution deferred. |
| REL-041 | Yes for unsigned-submission guards | Both variable gates and signed-manifest eligibility check; variables unset and release has no `.minisig` | First signed winget submission/token/fork deferred. |
| REL-060 | Yes for independent channel opt-ins | Public release verified with signing/updater/winget variables unset | Signing, updater keys, release-bot App and winget activation deferred. |
| UPD-005 | Yes, unsigned build gate | Release build feature conditions omit `updater`; feature tests in CI | Updater keys and production-channel activation deferred. |

The scoped plan is **done**: all six targets were built, the complete assets were verified,
and the unsigned release was published under the user's approval.
The broader distribution feature remains **in-progress** because the signed channel is deferred.
