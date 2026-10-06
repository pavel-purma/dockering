# Plan: `/release` skill, release-time changelogs, Windows signing (SignPath Foundation)

- **Slug:** `release-skill-and-signing`
- **Status:** in-progress (built 2026-10-06; the signing setup is the maintainer's, see [signing.md](../../signing.md))
- **Spec:** [Distribution](../../spec/features/distribution.md)
- **Milestone:** after the unsigned v0.1.0/v0.2.0 releases
- **Requirement IDs:** REL-018, REL-019, REL-033, REL-034 (new); REL-010, REL-011, REL-031, REL-032, REL-060 (changed)
- **Created:** 2026-10-06

## 1. Goal

One `/release` command does the whole release: it writes the changelogs from the merged commits, picks
the version from their severity, builds and tests locally, opens and merges the release PR, tags,
watches the build, verifies the draft and publishes it after the maintainer's approval. The changelogs
are written only at release time, so feature PRs stop conflicting on them. Windows downloads can be
signed by SignPath Foundation, and the signed channel no longer waits for macOS signing.

## 2. Scope

**In:** the `release` skill (Claude Code and OpenCode adapters), three `xtask` helpers, a committed
draft verifier, `release.yml` changes, SignPath artifact configurations, the README code signing
policy, the signing guide, the `feature-planning` change and the agent/spec text that mandated a
changelog line per PR.

**Out:** applying to SignPath, creating the SignPath configuration, generating and storing the real
update keys, the signed rehearsal and the channel switch (maintainer steps, [signing.md](../../signing.md));
Apple signing and notarization; retiring release-plz; a CI launch test; an app profile override;
fewer SignPath approvals (restructuring the Windows jobs).

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | Windows provider: SignPath Foundation (free for open source, files show *SignPath Foundation* as publisher, manual approval per request). Azure Artifact Signing stays supported. | Decided by the maintainer. |
| 2 | macOS signing is independent of the signed channel (`MACOS_SIGNING`, default `none`). | Decided by the maintainer. |
| 3 | The skill stops twice: before the release PR and before publishing. `auto` skips a stop only when the channel is unchanged, nothing warned and every check passed. | Decided by the maintainer. |
| 4 | "The change log" in the request means `docs/spec/CHANGELOG.md`; the root `CHANGELOG.md` is only ever written in release PRs. Both are release-time now. | Both release-time. |
| 5 | SignPath's exact signer subject, quotas, solo-maintainer acceptance, branch-rehearsal policy and `parameters` syntax could not be confirmed from its documentation. | The signed rehearsal confirms them; the channel stays unsigned until it passes. |

## 4. Requirements

| ID | Requirement | New/Changed |
|---|---|---|
| REL-018 | `/release` runs REL-010 end to end with two approval stops, a local gate, draft verification and public URL checks; hard prohibitions on pushing to `main`, touching tags and published releases, and handling secrets. | New |
| REL-019 | Both changelogs are written only by the release skill, in the release PR; other PRs and `feature-planning` never edit them. | New |
| REL-033 | README *Code signing policy* and the signed notes' policy link and SignPath attribution. | New |
| REL-034 | `MACOS_SIGNING` (`apple`\|`none`) is independent of Windows; a signed release never requires macOS signing. | New |
| REL-010, REL-011 | The flow runs through the skill; bumps come from `cargo xtask release-plan`; release-plz is dormant. | Changed |
| REL-031, REL-032 | Signed mode needs a compiled-in public key; signed tags require `WINDOWS_SIGNER_SUBJECT`; timestamp and digest are checked; the manifest signature is checked against the app's keys. | Changed |
| REL-060 | The channel switch follows the documented setup, with a verified rehearsal and a pinned signer. | Changed |

## 5. Engine contract impact

None. Engine APIs, capabilities, hub threading, application UI and keyboard bindings don't change.

## 6. Design

```
/release → preflight → release-plan → changelogs → local gate → bump → [Gate 1] → release PR
        → merge → tag → release.yml (signed: 4 SignPath approvals) → draft → verify-release.ps1
        → [Gate 2] → publish → record PR
```

- **`cargo xtask release-plan`** lists the first-parent (squash-merge) commits since the last stable tag as JSON: type, scope, PR number, requirement IDs, files, whether a user sees it, and the level each implies, then the bump and next version (REL-011). `--bump` accepts `patch|minor|major` or an explicit version, which must exceed the last tag.
- **`cargo xtask release-verify`** mirrors the `verify` job of `release.yml` locally.
- **`cargo xtask verify-manifest`** checks the `.minisig` against the keys in `keys.rs`: the one check that proves the CI secret matches the compiled-in public keys.
- **`scripts/verify-release.ps1`** replaces the uncommitted 114-check script of 0.2.0 and adds signature, attestation, public URL and launch checks.
- **`release.yml`:** per-platform signing (`macos_signed` output), early failure on an incomplete signed configuration, `actions: read` for SignPath, the `version` artifact parameter, a one-hour approval wait, timestamp/digest/subject checks, notes for the signed channel.
- **Signing flow:** see [signing.md](../../signing.md) phases 1–7. A trusted certificate can't be generated locally; SignPath Foundation holds the key and signs after the maintainer approves.

## 7. Tasks

| # | Task | Owner | IDs | Verification |
|---|---|---|---|---|
| 1 | Spec, plan, index row. | architect | all | review |
| 2 | `release-plan`, `release-verify`, `verify-manifest` in `xtask`. | release-engineer | REL-011, REL-018, UPD-003 | `rel_018_*` and `upd_003_*` tests; real-repo runs over `v0.1.0..v0.2.0` |
| 3 | `scripts/verify-release.ps1`. | release-engineer | REL-018, REL-032 | passes on public v0.2.0 as unsigned, fails only signature checks as signed, thirteen mutations each fail the right check |
| 4 | `release.yml` per-platform signing, early failures, SignPath inputs, signed notes, verification of timestamp/digest/subject/manifest. | release-engineer | REL-031…034 | actionlint 1.7.12; the `verify` step run in 13 configurations (`scripts/tests/test_release_verify.py`); digest function exercised on signed binaries |
| 5 | Skill, adapters, `AGENTS.md`, agent prompts, quality DoD, `feature-planning` change. | coordinator | REL-018, REL-019 | grep shows no per-PR changelog duty left |
| 6 | SignPath configurations, signing guide, README policy, runbook. | coordinator | REL-030…034 | links resolve |
| 7 | Review. | reviewer | all | verdict |

## 8. Test plan

| Requirement | Evidence |
|---|---|
| REL-011, REL-018 | `xtask` unit tests; `release-plan --since v0.1.0 --to v0.2.0` gives `minor` driven by the one `feat`; `--since v0.2.0 --to origin/main` gives `none` (docs only) |
| REL-018 | `verify-release.ps1` self-tests (see above), plus attestation and published-URL runs against v0.2.0 |
| REL-019 | `grep -rn CHANGELOG` over the skills, `AGENTS.md`, `docs/spec/60-quality.md`, `.claude/agents` |
| REL-031…034 | actionlint; local reproduction of the key gate and digest check; the signed rehearsal after setup |

## 9. Risks and limits

| Risk | Mitigation |
|---|---|
| SignPath's behaviour differs from its documentation (subject, origin policy for branch runs, parameter syntax). | The channel stays unsigned until a rehearsal passes; the rehearsal is a dispatch, so nothing is tagged or published. |
| Renewal changes the signer's subject or issuer, which the updater pins (UPD-003). | The verifier prints both; the signing guide says to check at each renewal. |
| `release.yml` is edited but only runs on tags and dispatch. | After merge, dispatch `-f unsigned=true` on `main` and require the unsigned behaviour to be unchanged. |
| The skill is autonomous over a long, expensive flow. | Two approval stops, resume rules, prohibitions on tags, `main` and secrets, and every step re-checks state first. |
| Inno's uninstaller stays unsigned with remote signing. | Known gap, unchanged. |

## 10. Revision log

- 2026-10-06: Created and built after the maintainer approved the plan (SignPath Foundation, Windows first, two approvals).
