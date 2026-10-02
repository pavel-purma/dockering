# Feature Plans

One file per feature or change, created by the `feature-planning` skill
(`.claude/skills/feature-planning/`). The template is
[`.claude/skills/feature-planning/plan-template.md`](../../../.claude/skills/feature-planning/plan-template.md).

| Plan | Spec | Milestone | Status |
|---|---|---|---|
| [keyboard-navigation](keyboard-navigation.md) | [keyboard.md](../../spec/features/keyboard.md) | M2 foundation, then every UI milestone | in-progress (open: KBD-090 release walkthrough, KBD-092 Tab-walk beyond Settings) |
| [windows-distribution](windows-distribution.md) | [distribution.md](../../spec/features/distribution.md), [50-build-and-release.md](../../spec/50-build-and-release.md) | M10 Distribution | approved |
| [ui-tabs-settings-nav](ui-tabs-settings-nav.md) | [30-ui-shell.md](../../spec/30-ui-shell.md), [settings.md](../../spec/features/settings.md) | post-v1 UI polish | draft |

The other v1 features (engines, containers, container detail/logs/terminal/stats, images, volumes,
networks, settings) were built straight from the milestone tables in [../README.md](../README.md)
without a per-feature plan. Their completion record is the *Verification* and *Known gaps (v1)*
sections in each feature spec.

Statuses: `draft` → `approved` → `in-progress` → `done` (or `abandoned`).
