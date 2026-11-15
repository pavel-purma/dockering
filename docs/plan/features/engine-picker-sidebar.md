# Plan: Engine picker moves to the sidebar and status bar; the switcher becomes a modal

- **Slug:** `engine-picker-sidebar`
- **Status:** in-progress
- **Spec:** [Engines](../../spec/features/engines.md) (ENG-100, ENG-101, ENG-108, ENG-165, ENG-166), [UI shell §1](../../spec/30-ui-shell.md), [Keyboard](../../spec/features/keyboard.md) (KBD-005, KBD-021)
- **Milestone:** post-v1 UI polish (this checkout has no milestone README/table)
- **Requirement IDs:** ENG-165, ENG-166 (new) · ENG-100, ENG-101, ENG-108, KBD-005, KBD-021 (changed) · ENG-030, KBD-001/002/003/007 (preserved)
- **Created:** 2026-11-15
- **Approval:** the user approved implementation on 2026-11-15.

## 1. Goal

Today the active engine is a bordered button in the centre of the title bar, and `Mod+K` opens a dropdown under it. The user wants the engine choice to live with the navigation:

- The bottom of the sidebar shows the active engine as a card (icon, name, status dot, switch icon). Collapsed, it's a single icon button.
- The status bar starts with the active engine's name, and clicking that section also opens the switcher.
- The switcher is a modal panel like the command palette (`Mod+Shift+P`): centred near the top, dimmed backdrop, fixed size.
- The title bar keeps only the app name and the action buttons.

## 2. Scope

**In:** `shell/app_shell.rs` (title bar, sidebar footer, status bar, overlay rendering, title-region default focus, sidebar cursor), `shell/sidebar_nav.rs` (footer element), `shell/switcher.rs` (sizing inside the modal), `strings.rs`, view tests.

**Out:** the switcher's contents and behaviour (grouping, filtering, *Start & connect*, *Manage engines…*, ENG-106/116 marks); the hub, the engine contract and every backend; the command palette itself; Settings → Engines.

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | Expanded sidebar footer content (answered 2026-11-15) | Engine card: kind icon, name, dot, switch icon |
| 2 | "Bottom panel" means the status bar (answered 2026-11-15) | Status bar engine section, clickable |
| 3 | Modal size | Width 560 px like the palette; height = the palette's cap (`overlay_cap`), fixed whatever the engine count or filter |
| 4 | Where the insecure-TCP warning tag goes now that the title bar loses the engine | On the sidebar card (expanded); collapsed, the tooltip carries it |
| 5 | Keyboard path to the card | Last entry of the sidebar's roving cursor; `Enter` opens the switcher. The status bar region opens it on `Enter` |
| 6 | Title-region default focus | The first title button (update button if offered, else *Refresh*) |

## 4. Requirements (as written into the spec)

| ID | Requirement (summary) | New/Changed |
|---|---|---|
| ENG-100 | Active engine shown in the sidebar footer and status bar; not in the title bar | Changed |
| ENG-101 | Switcher is a fixed-size modal panel like the palette; backdrop click / `Esc` closes; focus returns to the invoker | Changed |
| ENG-108 | Status bar begins with kind icon + engine name + state only while the sidebar is collapsed; expanded, it shows only the details | Changed |
| ENG-165 | Sidebar engine card (expanded) / icon button (collapsed); insecure tag; shown on Settings routes | New |
| ENG-166 | Status bar left section is one click target that opens the switcher; `Enter` from the status region | New |
| KBD-005 | Title region default focus is its first button | Changed |
| KBD-021 | Switcher modal keys; card on the sidebar cursor; status bar `Enter` | Changed |

## 5. Engine contract impact

None. No `Engine`, `HubHandle`, DTO or capability changes. The UI reads the same `EngineListStore` / `EngineStatus` and dispatches the same `EngineSwitcher`, `SwitchEngine` and `start_engine` calls.

## 6. Design

### 6.1 Data flow & threading

No new I/O or tasks. The sidebar card and status bar read `active_status(cx)` and the store's `info()` as `render_title_bar` does today. All three openers dispatch the existing `EngineSwitcher` action through `ui::dispatch::on_click` (KBD-002), so the open/close and focus-restore logic in `on_engine_switcher` / `close_overlays` is unchanged.

### 6.2 UI

```
┌─ TitleBar ─ ◧ Dockering ───────────────────────────────── ⟳ ☾ ⚙ ≡ ┐
├────────────────────┬─────────────────────────────────────────────┤
│ ▣ Containers    12 │        ┌────────────────────────────────┐    │
│ ◫ Images        34 │        │ [Filter engines…            ]  │    │
│ ⛁ Volumes        9 │        │ Local                          │    │
│ ⇄ Networks       5 │        │ ◧ Docker Desktop  27.3  ● Conn │    │
│                    │        │ WSL distros                    │    │
│                    │        │ ▣ Ubuntu-22.04    27.3  ● Conn │    │
│                    │        │ …   (scrolls; panel size fixed)│    │
│────────────────────│        │────────────────────────────────│    │
│ ◧ Ubuntu-22.04 ● ⇅ │        │ ⚙ Manage engines…              │    │
├────────────────────┴────────└────────────────────────────────┘────┤
│ Docker 27.3.1 · API 1.47 · linux/amd64 │ via unix               │ CPUs 8│
└───────────────────────────────────────────────────────────────────┘
```

- **Title bar** (`render_title_bar`): remove the `engine-switcher-button` element, its `switcher_btn_bounds` tracking and the insecure tag. The spacer layout becomes app name · flex · actions.
- **Sidebar footer** (`render_sidebar` → `Sidebar::footer`): a new `sidebar_nav::EngineCard` element (`debug_selector("sidebar-engine")`). Expanded: `h_flex` with `Icon(kind_icon)`, truncated name, optional `Tag::warning` *Insecure*, `dot`, `Icon(ChevronsUpDown)`; same 32 px height, radius and hover/cursor tint as `NavItem`. Collapsed: centred `ChevronsUpDown` icon with a small dot overlay. Click → `dispatch::on_click(…, EngineSwitcher)`; `on_mouse_down` prevents default so the sidebar doesn't take focus (same as nav items). Tooltip: *Switch engine* + `Mod+K` (`Tooltip::action`), and the state label.
- **Sidebar cursor**: `SidebarEntry::Engine` is appended to `sidebar_entries` (both normal and Settings modes); `on_sidebar_activate` dispatches `EngineSwitcher` for it. `active_entry` never returns it.
- **Status bar** (`render_status_bar`): prepend `Icon(kind_icon)` + name to the left section only while the sidebar is collapsed (expanded, the card already names the engine); wrap the whole left `h_flex` in an `.id("status-engine-button")` with `cursor_pointer`, hover tint (`theme.secondary_hover`), `on_click` dispatching `EngineSwitcher`, and a tooltip. The status region gets an `on_key_down` `Enter`/`Space` handler that dispatches `EngineSwitcher`.
- **Modal** (`render_overlay`): delete `switcher_dropdown`; render the switcher through the same `panel` closure as the palette (width 560). `EngineSwitcher` takes a fixed body height: `set_height(cap - chrome)`, mirroring `ShortcutList::set_max_h`, and its rows list uses `h(...)` (not `max_h`) so the panel never resizes with the row count or filter. Width drops its own `w(px(380.))` and fills the panel.
- **Focus**: `Regions` title default becomes `title_focus`'s first tab stop (the refresh button gets a focus handle `refresh_focus`; when the update button is present the existing `update_focus` comes first). Remove `switcher_btn_focus` and `switcher_button_focus()`.
- **Strings**: reuse `SWITCH_ENGINE`, `NO_ENGINE`, `INSECURE_TCP*`. No new strings except a tooltip helper `switch_engine_tooltip(state)` if needed.

No `EngineKind` branching beyond the existing `kind_icon` (ENG-030). No focus ring (KBD-003).

### 6.3 Errors

None new. The switcher's existing error paths (start failure toast via `hub_call`) are untouched.

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Remove the engine button, its bounds and the insecure tag from the title bar; new title-region default focus (update button, else *Refresh*); drop `switcher_btn_focus` | gpui-ui | ENG-100, KBD-005 | `eng_100_title_bar_has_no_engine_button`; update `kbd_076_title_update_button_in_tab_order_and_f6`, `kbd_005_f6_cycles_regions` |
| 2 | Sidebar footer engine card (expanded + collapsed), insecure tag, tooltip; `SidebarEntry::Engine` on the roving cursor | gpui-ui | ENG-165, KBD-021 | `eng_165_sidebar_card_opens_switcher`, `eng_165_collapsed_card_opens_switcher`, `eng_165_sidebar_cursor_enter_opens_switcher`, `eng_165_card_on_settings_routes` |
| 3 | Status bar: engine icon + name first; left section clickable; `Enter` in the status region | gpui-ui | ENG-108, ENG-166, KBD-021 | `eng_166_status_engine_click_opens_switcher`, `eng_166_status_region_enter_opens_switcher` |
| 4 | Switcher as a fixed-size modal through the palette panel; remove `switcher_dropdown` | gpui-ui | ENG-101, KBD-021 | `eng_101_switcher_is_fixed_size_modal` (panel bounds equal with 1 vs many engines and after filtering), existing `kbd_021_switcher_filter_arrows_enter` |
| 5 | Re-point the click-away regression test at the new openers; screenshot review light/dark, expanded/collapsed | qa-engineer | KBD-002, KBD-007 | `kbd_007_header_menus_open_after_switcher_dismissed_by_title_click` renamed `…_by_backdrop_click`, uses `sidebar-engine` and a backdrop click |
| 6 | Release checklist §3: replace "title bar engine" steps with the sidebar card / status bar | qa-engineer | ENG-100 | checklist diff |
| 7 | Review | reviewer | all | verdict: approve |

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| ENG-100 | GPUI view | `eng_100_title_bar_has_no_engine_button` |
| ENG-101 | GPUI view | `eng_101_switcher_is_fixed_size_modal`, `kbd_021_switcher_filter_arrows_enter` |
| ENG-108 | GPUI view | `eng_166_status_engine_click_opens_switcher` (no name while expanded), `eng_165_collapsed_card_opens_switcher` (name first while collapsed) |
| ENG-165 | GPUI view | `eng_165_sidebar_card_opens_switcher`, `eng_165_collapsed_card_opens_switcher`, `eng_165_sidebar_cursor_enter_opens_switcher`, `eng_165_card_on_settings_routes` |
| ENG-166 | GPUI view | `eng_166_status_engine_click_opens_switcher`, `eng_166_status_region_enter_opens_switcher` |
| KBD-005 | GPUI view | `kbd_005_f6_cycles_regions`, `kbd_076_title_update_button_in_tab_order_and_f6` |
| KBD-007 | GPUI view | `kbd_007_header_menus_open_after_switcher_dismissed_by_backdrop_click` |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| `Sidebar::footer` padding | Checked 2026-11-15 in `gpui-component-0.8.0/src/sidebar/mod.rs`: the footer is an `h_flex` (`px_3 pb_3`, `px_2 pt_2` collapsed) under a `flex_1` content list, so it pins to the bottom. The 1 px divider is drawn by the card (`border_t_1`), since the kit adds none |
| A sidebar entry that isn't a route complicates `active_entry`/`sidebar_set_cursor` (Settings mode auto-navigates on cursor move) | `SidebarEntry::Engine` never navigates on cursor move; covered by `eng_165_sidebar_cursor_enter_opens_switcher` |
| Fixed-height modal shows empty space with 1–2 engines | Accepted by the user ("not necessary to change the size"); the list area is plain background |
| Status bar text may get long with icon + name added | Name truncates (max ~200 px) before facts do |

## 10. Revision log

- 2026-11-15: created (user answers: engine card in the sidebar footer; "bottom panel" = status bar).
- 2026-11-15: approved and implemented (tasks 1–6). `cargo test -p dockering` 366 passed; clippy, fmt and the blocking check clean. Screenshot review on Windows, `--demo`: dark and light, sidebar expanded and collapsed, switcher open. Manual checks on real engines are in the release checklist.
- 2026-11-15: revised at the user's request: the status bar names the engine only while the sidebar is collapsed; expanded, the sidebar card names it and the status bar shows the details (ENG-108/166).
- 2026-11-15: revised at the user's request: with the sidebar expanded, the status bar also drops the state dot and label.
- 2026-11-15: fix from the user's testing: with the sidebar expanded and the engine not connected, the status bar went empty. The state (or *No engine*) now stays until details are shown, and the bar keeps a minimum height (`eng_108_status_bar_shows_state_while_not_connected`).
- 2026-11-15: revised again at the user's request: the state appearing and then disappearing on connect was distracting. With the sidebar expanded the status bar never shows the connection state; it is always shown at its minimum height and fills in the details when they arrive (`eng_108_status_bar_keeps_its_height_without_state`).
- 2026-11-15: revised at the user's request: the status bar never draws a status dot (the sidebar card and the switcher rows carry it); collapsed, it shows the state as text only (ENG-100/108).
