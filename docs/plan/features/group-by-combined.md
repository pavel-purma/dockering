# Plan: Group by *Combined* (Compose project, else a label); the label key lives in Settings

- **Slug:** `group-by-combined`
- **Status:** done
- **Spec:** [Containers](../../spec/features/containers.md) (CON-010/011/013 changed, CON-014 new), [Settings](../../spec/features/settings.md) (SET-020 changed), [Engine contract §4](../../spec/21-engine-api-contract.md#4-grouping-rules-con-010) (rules 2 and 3), [Product glossary](../../spec/00-product.md) (*Group*)
- **Milestone:** post-v1 UI polish (this checkout has no milestone README/table)
- **Requirement IDs:** CON-014 (new) · CON-010, CON-011, CON-013, SET-020 (changed) · KBD-039, NFR-001…005, ENG-030 (preserved)
- **Created:** 2026-10-11

## 1. Goal

Today the Containers page groups by Compose project, and an optional *Label…* mode asks for a label key in a prompt on the page itself. Containers that don't come from Compose (plain `docker run`, Helm-style or hand-labelled stacks) stay in a flat tail.

After this change:

- *Group by* has a first option, **Combined**, and it is the default. A container with a Compose project goes into that project's group. A container without one goes into a group named by the value of a configurable label. A container with neither stays ungrouped, after the groups.
- The label key is **configured only in Settings → Containers**, prefilled with a standard key (`app.kubernetes.io/part-of`). The Containers page never shows it or asks for it. The inline key prompt is removed.
- The existing pure *Label* mode stays as a fourth mode and uses the same Settings key.

## 2. Scope

**In:** `dk_core::grouping` (`GroupBy`, `group_key`, `group`, group metadata); `dk-hub` config (`ContainerSettings`, defaults, migration); the Containers page (menu, value element, removal of the key prompt, group-row tag, delete-all note); `nav.rs` mode strings; Settings → Containers (the *Group by* select and an always-shown key field); demo data; tests; the spec text above.

**Out:** Compose dependency ordering for *Start all* (unchanged, CON-013); volumes and networks (still a column and filter, 21 §4 rule 6); per-container label editing; multiple label keys or a precedence list; the *Group by* control's look (variant A, already built and kept); the `Engine` trait and every backend.

## 3. Assumptions & open questions

User answers, 2026-10-11: (1) *Combined* is first in the menu and the default after the app starts; (2) the label is set in Settings, not on the Containers screen; (3) keep *Label* as a fourth mode; (4) the label key default is a common, standard application identifier, prefilled in Settings after the first start.

| # | Assumption / decision | Default if unanswered |
|---|---|---|
| A1 | *(answer 3)* Menu order is *Combined*, *Compose project*, *None*, *Label*. The *Group by* caption never changes; the value shows the mode name only (the tag icon and mono key shown for a custom label in the earlier variant-A mock-up go away, because the key is no longer shown on the page) | Yes |
| A2 | *(answer 4)* Default key is **`app.kubernetes.io/part-of`**. Checked against the sources: kubernetes.io's recommended labels define it as "the name of a higher level application this one is part of", example `wordpress` for a `mysql` component. That is the same idea as a Compose project (components of one application), so groups come out right. `app.kubernetes.io/name` is the application *component's* own name (example `mysql`), which would group all MySQLs together across applications. The OCI image annotations (`org.opencontainers.image.title`, `.source`) describe an image, not the running application, and are not container-level app identifiers. I found no usage-frequency source (the web search returned nothing), so "most common in Docker" is a judgement from these definitions, not a measurement | Yes; veto and `app.kubernetes.io/name` or another key becomes the default (one constant) |
| A3 | The new default applies to a config that has no saved choice (first start, or a missing/corrupt file). When the app saves, it writes the whole config (`save_config` serialises every field), so a user whose config file already carries `group_by = compose` keeps *Compose project*; the app can't tell "never chose" from "chose Compose". ⚠ verify in task 2 that a saved file really contains `group_by`. They pick *Combined* once from the menu or Settings | Yes; veto and a one-time migration moves saved `compose` to `combined` (config version bump) |
| A4 | In *Combined* a Compose project always wins over the label. A Compose group and a label group with the same display name stay two groups (different keys: `compose:<p>` and `label:<key>=<v>`), each with its own tag. Merging them would mix Compose dependency ordering with label semantics | Yes |
| A5 | The key is matched exactly (case-sensitive), as Docker label keys are. The value is trimmed; an empty or whitespace-only value counts as *no value* | Yes |
| A6 | The key can't be empty: Settings rejects an empty field inline and keeps the previous key (same message style as today). A hand-edited empty key in the file loads as the default. So *Label* and *Combined* always have a key, and no mode needs a "set a key first" state | Yes |
| A7 | Saved `{type = "label", key = "app"}` configs (the old shape) migrate on load to mode *Label* with `group_label_key = "app"`, so nobody loses a key they typed | Yes |
| A8 | The page menu item for *Label* works as soon as it is picked (no prompt). The Settings description says which key it uses; there is no hint on the page | Yes |
| A9 | Group actions (CON-013): *Start all* orders by Compose `depends_on` only when members carry it, so label groups run in parallel (limit 4). The *Delete all* dialog keeps the "networks and volumes are kept; use `docker compose down`" note only for Compose groups | Yes |

## 4. Requirements (as written into the spec)

| ID | Requirement | New/Changed |
|---|---|---|
| CON-010 | *Group by*: *Combined* (default) / *Compose project* / *None* / *Label*; the control is the labelled field (variant A) opening a four-item menu; the label key is not part of it | Changed |
| CON-011 | A label group's row has a "label" tag (tooltip: `key=value`) where a Compose group has the "compose" tag | Changed |
| CON-013 | The compose-down note in *Delete all* applies to Compose groups only | Changed |
| CON-014 | The label key comes from `containers.group_label_key` (default `app.kubernetes.io/part-of`), is set only in Settings, and the Containers page never shows or asks for it; changing it regroups a mounted page; in *Combined* a Compose project wins | New |
| SET-020 | *Group by* default has four choices and defaults to *Combined*; *Group label key* is always shown, prefilled, required, and used by *Combined* and *Label* | Changed |

Contract text (21 §4 rules 2 and 3) and the glossary are edited to match.

## 5. Engine contract impact

None. No `Engine` method, DTO, capability or `EngineFactory` change; labels already arrive on `ContainerSummary.labels`. This is a pure `dk-core` and UI change, so no per-backend mapping applies (ENG-030: the UI doesn't branch on `EngineKind`).

## 6. Design

### 6.1 Data flow & threading

- `dk_core::grouping`: `GroupBy` becomes `{ Combined, Compose, None, Label }` (unit variants). The key is a parameter: `group_key(c, by, label_key)` and `group(containers, by, label_key, interleave)`. `ContainerGroup.is_compose` stops being `matches!(by, Compose)` and is set from how each group was resolved (its key prefix), so a *Combined* result can contain both kinds. `working_dir` is read only for Compose groups, as today.
- `dk-hub` config: `ContainerSettings` gains `group_label_key: String` (default `app.kubernetes.io/part-of`, exported as a `dk-core` constant so the UI and tests share it); `group_by` default becomes `Combined`. Loading normalises an empty key to the default and migrates the legacy `Label(key)` shape (A7). Config writes stay debounced through the existing path.
- Containers page: reads `group_by` and `group_label_key` from `AppState::config`. The grouping job already runs in `background_spawn` above 2,000 rows with a revision id (`BACKGROUND_THRESHOLD`); the key is part of that job's input, and a change of either value bumps the revision, so a stale result is dropped. The page keeps following Settings while mounted (as `set_020_mounted_containers_page_follows_settings` does). No new `Task`, no hub call, no blocking work.
- Expanded state persists per group key; label keys are `label:<key>=<value>`, so they are stable across restarts. Changing the label key makes new keys (groups start expanded by default, CON-011).

### 6.2 UI

Containers page: *Group by* stays the variant-A field. The menu lists four items, *Combined* first, with a check on the active one. Picking an item applies it at once. The inline key `Input`, `label_prompt` and `on_group_key_event` are deleted. `Mod+Shift+G` still opens the selector (KBD-039); the command-palette and menu plumbing keep working through `SetGroupBy`, whose `mode` strings become `combined`, `compose`, `none`, `label` (the old `label:<key>` still parses to *Label*).

```
Group by │ Combined ⌄        menu:  ✓ Combined
                                      Compose project
                                      None
                                      Label
```

Group rows: Compose groups unchanged. Label groups get a "label" tag (tooltip `app.kubernetes.io/part-of=wordpress`) and no working-dir tooltip. *Start/Stop/Restart/Delete all* are the same buttons.

Settings → Containers: *Group containers by* (select: Combined, Compose project, None, Label) and *Group label key* (text input, always visible, prefilled, required, inline error when empty). The select no longer writes the key, and the field no longer depends on the mode, so `label_pending` and `commit_label_key`'s mode-dependent logic go away. The row description says which modes use the key. No shortcut changes; both controls stay Tab reachable (KBD-072).

States: unchanged (loading, empty, error, data). No focus rings (KBD-003).

### 6.3 Errors

No engine errors. Settings: an empty key shows the existing inline "Enter a label key…" message and leaves the saved key alone. A corrupt config falls back to defaults as today (`back_up`).

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | `dk-core`: new `GroupBy`, key parameter, per-group `is_compose`, `DEFAULT_GROUP_LABEL_KEY`, trimmed/empty value handling; update callers | rust-core | CON-010, CON-011, CON-014 | `cargo nextest -p dk-core` grouping tests |
| 2 | `dk-hub` config: `group_label_key`, defaults (*Combined*, `app.kubernetes.io/part-of`), empty-key normalisation, legacy `Label(key)` migration | rust-core | CON-014, SET-020 | config round-trip, legacy-file and corrupt-file tests |
| 3 | Containers page: four-item menu, remove the key prompt and its input, plain mode-name value, `nav.rs` mode strings, label tag and tooltip on label groups, compose note for Compose groups only, drop the unused tag-icon/mono branch from the control | gpui-ui | CON-010, CON-011, CON-013, CON-014 | view tests; screenshot in demo mode (light and dark) |
| 4 | Settings → Containers: four-choice select, always-visible required key field writing `group_label_key`, remove `label_pending` and the mode-dependent commit logic | gpui-ui | SET-020, CON-014 | settings view tests; Tab walk |
| 5 | Demo data: add containers carrying `app.kubernetes.io/part-of` (and one with both labels) so *Combined* shows both kinds of group | gpui-ui | CON-010 | `--demo` screenshot |
| 6 | Tests and spec Verification tables (containers, settings, keyboard KBD-039 reference) | qa-engineer | all | ID→test table (§8) |
| 7 | Review (UI-thread rules, stale guards, spec conformance, migration safety) | reviewer | all | verdict: approve |

Order: 1 → 2 → (3 ∥ 4) → 5 → 6 → 7.

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| CON-010 | dk-core | `con_010_combined_prefers_compose_then_label`, `con_010_combined_ungrouped_without_either`, `con_010_label_mode_ignores_compose`, `con_010_empty_label_value_is_ungrouped`, existing `con_010_*` updated for the key parameter |
| CON-011 | dk-core | `con_011_group_is_compose_follows_resolution` |
| CON-014 | dk-hub | `con_014_default_is_combined_with_part_of_key`, `con_014_legacy_label_group_by_migrates`, `con_014_empty_label_key_normalised` |
| CON-010, CON-014 | view | `con_010_combined_default_groups_label_fallback`, `con_014_no_label_prompt_on_containers_page` (the menu has four items; no key input exists), `con_014_settings_key_change_regroups_mounted_page` |
| CON-011, CON-013 | view | `con_011_label_group_has_label_tag_and_no_working_dir`, `con_013_label_group_delete_dialog_has_no_compose_note` |
| SET-020 | view | `set_020_group_by_default_affects_containers_page` (rewritten for four choices), `set_020_label_key_prefilled_and_required` |
| KBD-039 | view | `kbd_039_group_by_none_flattens` still passes; the Tab walk `a11y_tab_walk_reaches_all_interactive_settings` covers the always-visible key field |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Changing `GroupBy`'s serde shape could drop a saved config key | Task 2 keeps a tolerant reader and a test with a real legacy `[containers.group_by] type = "label"` sample; a corrupt file is still backed up and defaulted |
| Existing users stay on *Compose project* (A3) and may think *Combined* is missing | The menu lists it first; the PR description says so; one-time migration is the veto path |
| Two groups with the same display name in *Combined* (A4) | Different tags (compose / label) and tooltips; revisit if users report confusion |
| Large lists regroup when the key changes | Same background path and revision guard as sorting/filtering above 2,000 rows (NFR-001…005) |
| The default key is a judgement (A2), not a measured norm | It is one constant and one Settings field; users can change it |

## 10. Revision log

- 2026-10-11: created from the user's request and answers; spec edited to `planned`; waiting for approval.
- 2026-10-11: approved by the user ("Approved"); implementation started.
- 2026-10-11: implemented; reviewed (no blockers); spec reconciled and status set to `done`. Deviations: none from the design. Mode strings are `combined|compose|none|label`; a saved config that already stores `group_by = compose` keeps *Compose project* (A3).
