# Plan: Segmented detail tabs · Settings in the app sidebar

- **Slug:** `ui-tabs-settings-nav`
- **Status:** done
- **Spec:** [30-ui-shell.md](../../spec/30-ui-shell.md) §1/§4 · [settings.md](../../spec/features/settings.md) · [keyboard.md](../../spec/features/keyboard.md)
- **Milestone:** post-v1 UI polish
- **Requirement IDs:** SET-080 (new) · SHL-025 (new) · KBD-005, KBD-024 (clarified)
- **Created:** 2026-10-02

## 1. Goal
Detail pages (container, image, volume, network) get a modern, app-style tab control: a
segmented control with an icon per tab and a sliding selection, instead of underlined text
inside a full-width focus box. Settings stops drawing its own section column; the app sidebar
switches to a *Settings mode* that renders the sections with exactly the same component
(`NavMenu`/`NavItem`) as Containers/Images/Volumes/Networks, plus a *Back* item that returns
to the app page the user came from.

## 2. Scope
**In:** `resources::detail::tab_bar`, the container detail tab bar, `shell::sidebar_nav`
(generalised items), `AppShell` sidebar/cursor/focus for Settings, `SettingsPage` layout and
focus, view tests, spec text.
**Out:** inner tab bars (Stats time window, terminal sessions/shell picker) keep their current
style; no new actions or bindings.

## 3. Assumptions & open questions
| # | Assumption / question | Default if unanswered |
|---|---|---|
| 1 | Tab style | GPUI Kit `TabBar::segmented()` (raised pill on a tinted track, animated), icon + label |
| 2 | Back target | The last non-Settings route (list or detail), mapped to its parent list after an engine switch; the start page if Settings was the first route. *Back* navigates (pushes history). |
| 3 | Back label | `Back to <Page>` (e.g. *Back to Containers*); collapsed sidebar shows the arrow icon with that tooltip |
| 4 | Keys in Settings mode | Unchanged from today's settings nav: `↑/↓/Home/End` move the cursor and show that section in place (no history entry); `Enter`/`Space` on a section enter its first control; on *Back* they navigate back. Mouse click on a section pushes the route (deep link), as today. |

## 4. Requirements (as written into the spec)
| ID | Requirement | New/Changed |
|---|---|---|
| SHL-025 | Detail pages MUST use a segmented `TabBar` (icon + label per tab, sliding selection). The bar hugs its content; its focus ring follows the control's rounded shape. | New |
| SET-080 | On a Settings route the app sidebar MUST switch to Settings mode: a *Back to <Page>* item, then one item per section, rendered with the same nav item component as the main menu. The settings page has no section column of its own. Opening Settings focuses the sidebar with the cursor on the active section. | New |
| KBD-005 | In Settings the *Sidebar* region holds the section nav. | Clarified |
| KBD-024 | `Mod+,` focuses the sidebar's section nav. | Clarified |

## 5. Engine contract impact
None.

## 6. Design
### 6.1 Data flow & threading
UI only. No hub calls, tasks, or background work.

### 6.2 UI
- `sidebar_nav::NavItem` gets `label`, `route`, `icon`, `count`, `active`, `cursor` (instead of
  `page`). `NavMenu` takes an optional heading (`Settings`) rendered like a `SidebarGroup` label.
- `AppShell`:
  - `app_route: Route` remembers the last non-Settings route (updated on every route change;
    mapped through `for_engine_switch` on engine switch).
  - `sidebar_entries()` → main: visible pages; Settings: `[Back(app_route), sections…]`. The roving
    cursor, `sidebar::{Prev,Next,First,Last,Activate}` and click handling work over entries.
  - Settings mode: cursor moves replace the route to that section; Activate on a section →
    `focus_next` into the content; on Back → `navigate(app_route)`.
  - Page primary focus on Settings = the sidebar region (cursor on the active section).
- `SettingsPage`: drops `render_nav` and the nav handlers; keeps one non-tab-stop root focus
  handle (`focus`) for `dispatch_here`, `Focusable`, and the Content region default.
- Detail tabs: `TabBar::segmented()` default size; each `Tab` gets a child row (icon + label)
  and `aria_label`. Wrapper `self_start`, rounded like the bar, `focus_ring` on it.
  Icons: Overview `LayoutDashboard`, Logs `ScrollText`, Terminal `SquareTerminal`, Stats
  `Activity`, Mounts `HardDrive`, Network/Containers `Network`/`Boxes`, Layers `Layers`,
  Used by `Boxes`, Inspect `Braces` (new Lucide extras: `ScrollText`, `Activity`, `Braces`).

### 6.3 Errors
None.

## 7. Tasks
| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 1 | Segmented tab bar with icons (resource detail + container detail) | gpui-ui | SHL-025 | screenshot; `cdt_*`, `kbd_040_*` pass |
| 2 | Generalise `NavItem`/`NavMenu`; shell Settings mode, `app_route`, cursor/activate over entries | gpui-ui | SET-080, KBD-005 | view tests |
| 3 | `SettingsPage` without its own nav; focus handle rename | gpui-ui | SET-080, KBD-024 | settings view tests |
| 4 | Update tests: `kbd_024_*`, `set_route_section_*`, `set_070_*`, tab walk; add `set_080_settings_sidebar_back_returns_to_app_page` | qa-engineer | SET-080, KBD-024 | `cargo nextest run -p dockering` |
| 5 | Review | reviewer | all | approve |

## 8. Test plan
| ID | Layer | Test name(s) |
|---|---|---|
| SET-080 | GPUI view | `set_080_settings_sidebar_back_returns_to_app_page`, `set_route_section_arrows_and_back_forward` |
| KBD-024 | GPUI view | `kbd_024_mod_comma_opens_settings_with_nav_focus` |
| SHL-025 | manual | screenshot review (light/dark) in the release checklist |

## 9. Risks & spikes
| Risk | Mitigation / spike |
|---|---|
| Segmented `Tab` with an icon renders icon-only (`Tab::icon`) | Put icon + label in the tab's children instead of `icon()` |
| Tab walk now starts in the shell sidebar | Walk asserts return-to-start, not region confinement; counts are lower bounds |

## 10. Revision log
- 2026-10-02: created
- 2026-10-02: approved and implemented. Focus rings on the tab bar and the sidebar cursor show only after keyboard input (`Window::last_input_was_keyboard`), because navigation focuses them programmatically.
