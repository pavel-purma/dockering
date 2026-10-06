---
name: feature-planning
description: Spec-driven feature planning for Dockering. Use when the user asks to plan, design, add, or change a feature (e.g. "/feature-planning add container rename", "plan image search"), or to finalize a finished feature ("/feature-planning complete <slug>"). Reads docs/spec, writes a plan to docs/plan/features/<slug>.md, and updates the spec (requirements, status) so spec and code never drift. Never edits a changelog: the release skill writes both at release time.
---

# Feature planning (spec-driven)

The spec in `docs/spec/` is the source of truth. This skill turns a request into:
1. **Spec changes**: new or changed requirements with IDs, status `planned`.
2. **A plan**: `docs/plan/features/<slug>.md`, built from [plan-template.md](plan-template.md).

It also **closes the loop** when a feature is finished (`complete` mode).

## Modes

| Invocation | Mode |
|---|---|
| `/feature-planning <free-text request>` | **plan**: new feature or change |
| `/feature-planning update <slug> <change>` | **revise**: change an existing plan and spec |
| `/feature-planning complete <slug>` | **complete**: reconcile spec with the implementation |
| `/feature-planning status` | **status**: table of all feature specs and plans with statuses |

---

## Mode: plan

### Step 1: Load context (always, before asking anything)
Read, in this order:
1. `docs/spec/README.md` (conventions, ID prefixes, statuses, feature index)
2. `docs/spec/00-product.md` (scope and **non-goals**)
3. `docs/spec/10-architecture.md` §3 (threading rules) and `docs/spec/21-engine-api-contract.md` (`Engine` trait, capabilities)
4. Every feature spec in `docs/spec/features/` that the request touches, plus `docs/spec/30-ui-shell.md` for UI work
5. `docs/plan/README.md` (milestones), if present, and any existing plan in `docs/plan/features/` for the same area
6. Code: `Grep` for the related modules to learn what exists already (skip if the crate isn't there yet)

### Step 2: Classify and check scope
- Which spec area(s) and ID prefix(es)? (`ENG CON CDT LOG TRM STA IMG VOL NET SET SHL KBD NFR`)
- Is it a **non-goal** in `00-product.md`? If so, stop and tell the user. Proceed only if they explicitly want the scope changed, and record that as a spec change to `00-product.md`.
- Does it conflict with an existing requirement or ADR? If so, list the conflicts.

### Step 3: Clarify (only what changes the design)
Ask the user at most 3–4 focused questions (use AskUserQuestion in Claude Code or question in OpenCode), and only when the answer changes the
plan. Examples: which engines must support it, UI placement, destructive-action semantics. Use
sensible defaults for everything else and state them in the plan under *Assumptions*.

### Step 4: Design
Work out, delegating to the `architect` agent for non-trivial designs:
- **Requirements**: new IDs (next free number in the prefix; never reuse IDs), written with MUST/SHOULD/MAY.
- **Engine contract impact**: new or changed `Engine` methods or DTO fields, the new `Capability` if the op isn't universal, and the mapping for **every** backend (Docker API endpoint + bollard call; WSLC COM method; `wslc` CLI command; unsupported → `Unsupported`).
- **Threading**: which hub calls or streams, where tasks are stored, batching, stale guards.
- **UI**: route, GPUI Kit components, the four states, actions, confirmations. **Keyboard is mandatory**: every new command is a GPUI `Action` with a default binding (or a command-palette entry), Tab/arrow reachability, focus restoration, and the shortcut shown in its tooltip. Check `docs/spec/features/keyboard.md` for conflicts and add the new bindings to its tables.
- **Tasks**: ordered, small (≤ 1 day each), each with an owner agent (`rust-core`, `engine-integrator`, `windows-platform`, `gpui-ui`, `qa-engineer`, `release-engineer`), the requirement IDs it covers, and a verification step.
- **Tests**: which layer proves each requirement (see `docs/spec/60-quality.md`).
- **Risks / spikes**: especially anything that depends on unverified `wslc` behaviour.

### Step 5: Write the spec changes
- Edit or create `docs/spec/features/<area>.md`: add or modify requirement rows, set `Status: planned`, and link the plan.
- If new: add a row to the feature table in `docs/spec/README.md`.
- If the contract changed: update `docs/spec/21-engine-api-contract.md` (trait, DTOs, capability matrix, mapping table) and `docs/spec/20-engine-backends.md` §5.4 (WSLC COM) / §5.5 (WSLC CLI).
- Changed requirements: edit in place. Removed ones: ~~strike through~~ + `(removed YYYY-MM-DD)`.
- Do **not** add an entry to `docs/spec/CHANGELOG.md` or `CHANGELOG.md`: concurrent PRs conflict on their first lines. The `release` skill writes both from the merged commits (REL-019), so make the spec edit and the commit message say what changed.

### Step 6: Write the plan
Create `docs/plan/features/<slug>.md` from [plan-template.md](plan-template.md) (kebab-case slug,
status `draft`), and add it to `docs/plan/features/README.md` (create the index if absent). If the plan adds tasks to an existing milestone, also
update the milestone table in `docs/plan/README.md`, if present.

### Step 7: Present and stop
Show the user a short summary: the requirement IDs added or changed, contract changes, the task
list with owners, open questions, and risks. **Do not start implementing.** Wait for approval.
On approval, set the plan status to `approved`.

---

## Mode: revise (`update <slug> …`)
Load the plan and its spec, apply the change, keep IDs stable (add new IDs, strike removed
ones), and add a *Revision log* entry in the plan. Then present the diff summary and stop.

## Mode: complete (`complete <slug>`)
1. Read the plan and the spec. Inspect the implementation (`Grep` for the requirement IDs in code and tests, `git log --grep <ID>`).
2. For every requirement ID in the plan, verify that it's implemented and tested. Build a table: ID · implemented? · test(s) · notes.
3. **Reconcile the spec with reality.** Wherever the implementation deliberately differs (different UX, a skipped capability, an extra behaviour), update the spec text so it describes what was built. Requirements that weren't done either move to a follow-up plan or are marked `deferred` in the spec. Never leave the spec claiming something the code doesn't do.
4. Set the feature spec `Status: implemented` (or keep `in-progress` if items are deferred). Set the plan status to `done`.
5. Report the table and any spec edits to the user.

## Mode: status
Print a table: feature · spec status · plan · plan status · open requirement IDs.

---

## Rules
- Never write production code in this skill. The output is docs only.
- Never add entries to `CHANGELOG.md` or `docs/spec/CHANGELOG.md` here. They're written only at release time by the `release` skill (`.agents/skills/release/changelog.md`), so feature PRs don't conflict on them.
- Never invent `wslc` or Docker behaviour. Mark unverified items `⚠ verify (spike)` and add a spike task.
- Keep plans executable by the agents in `.claude/agents/`: every task has an owner, IDs, and a verification step.
- Use dates in absolute `YYYY-MM-DD` form.
