# Feature: Settings

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** SET
- **Plan:** no per-feature plan; built in milestone M9 of the [v1 plan](../../plan/README.md)

Settings is a full page (route `Settings { section }`, so deep links and back/forward work). The
section nav is the **app sidebar in Settings mode** (SET-080): the page draws no column of its own.
The content column holds GPUI Kit primitives: `GroupBox` groups with
`Switch`, `NumberInput`, `Input`, and `Select` controls, plus inline validation messages.

The GPUI Kit `Settings` container is **not** used. Its virtualised page list drops off-screen
controls from the Tab order (KBD-072/092), its section can't follow the route, and it has no
inline validation. Changes apply immediately (the theme in place, everything else through the
shell's config observer) and persist via `dk-hub::config` (debounced save). Exceptions are noted
per row.

| Section | ID | Settings |
|---|---|---|
| General | SET-001 | Theme (System/Light/Dark) · Start page (Containers/Images/Volumes) · Confirm before deleting stopped containers · Show Networks page |
| Engines | SET-010 | See ENG-104/105 · "Show all WSL distros" · "Show all WSLC sessions" · Rescan · *Add engine…* · per engine: inline rename, *Enabled*, *Test connection*, *Start & connect* (stopped WSL distros), *Hide*/*Unhide*, *Remove* (manual, confirmed), transport + note (ENG-110), and *Un-merge* for merged engines (ENG-009; see Known gaps) |
| Containers | SET-020 | Group by default (Compose/None/Label) · Custom group label key · Show CPU/Memory columns · Polling interval fallback (s, 1–300, validated inline) |
| Logs | SET-030 | Initial tail lines (default 1000) · Max buffer lines (default 50k) · Timestamps default · Wrap default |
| Terminal | SET-040 | Font family · Font size · Default shell · Scrollback lines · External terminal command (TRM-009; must contain `{cmd}`, validated inline) |
| Stats | SET-050 | History window (default 15 min, 1–60) · CPU % relative to all cores |
| Diagnostics | SET-060 | Log level (Info/Debug; **applies on restart**, because the tracing filter is installed once at startup; `RUST_LOG` overrides it) · Open logs folder · Copy diagnostics · per-engine transport, version, and note (ENG-110) · Version / licences (REL-002) |
| Navigation | SET-080 | *(planned, [plan](../../plan/features/ui-tabs-settings-nav.md))* On a Settings route the app sidebar shows a *Back to <Page>* item (returns to the last non-Settings route; the start page if none) followed by one item per section, rendered with the same nav item component as the main menu (hover tint, accent active item, count-less). Opening Settings focuses the sidebar with the cursor on the active section. `↑/↓/Home/End` move the cursor and show that section in place (no history entry); `Enter`/`Space` on a section enter its first control, on *Back* they navigate. A click on a section pushes its route. |
| Updates | SET-090 | *(planned, [plan](../../plan/features/windows-distribution.md))* *Check for updates automatically* (default on) · current version · last checked + result · *Check now* (inline result) · *View release notes*. Disabled with a note when a policy or `DOCKERING_DISABLE_UPDATES` turns updates off. Hidden in builds without the `updater` feature. See [distribution.md](distribution.md) UPD-009. |
| Keyboard | SET-070 | Read-only keymap view (the same as the shortcut reference, KBD-022). Rebinding is reserved for post-v1 (KBD-081). |

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| SET-001 | `set_001_theme_applies_live_and_persists`, `set_001_networks_page_hidden_from_sidebar`, `set_001_start_page_and_confirm_stopped_persist` |
| SET-010 | `eng_104_*`, `eng_105_*`, `eng_109_show_all_toggles_persist`, `eng_025_enable_toggle_calls_hub_update`, `eng_009_unmerge_adds_id_to_config` |
| SET-020 | `set_020_group_by_default_affects_containers_page`, `set_020_mounted_containers_page_follows_settings`, `set_020_cpu_columns_toggle_live`, `set_020_polling_interval_validated_inline`, `set_020_polling_interval_at_least_one_second` |
| SET-030/040/050 | `set_030_040_050_numbers_text_and_switches_persist`, `set_040_font_size_accepts_decimals`, `set_050_history_window_1_to_60_minutes` |
| SET-060 | `set_060_copy_diagnostics_to_clipboard`, `set_060_log_level_persists`, `rel_002_licences_are_embedded` |
| SET-070 | `set_070_keyboard_section_lists_bindings` |
| Route, Tab order | `set_route_section_arrows_and_back_forward`, `kbd_024_mod_comma_opens_settings_with_nav_focus`, `a11y_tab_walk_reaches_all_interactive_settings` |

## Known gaps (v1)

- SET-060: changing the log level takes effect only after a restart. The control's description says so.
- SET-010 / ENG-009: *Un-merge* appears only for merged engines whose name was seen in the current session.
- SET-060 *Open logs folder* has no automated test (it opens the OS file manager).
- The WSLC transport (*Auto*/*COM*/*CLI*) is chosen in the *Add engine* dialog only. It isn't editable for existing engines (20 §5.3).
