# 30 — UI Shell

All UI is built with **GPUI Kit** (`gpui-kit` 0.7: `gpui` + `gpui-component`). Custom
elements are allowed only where GPUI Kit has no equivalent. The terminal grid is the only
planned one. Prefer GPUI Kit components and theme tokens over hand-styled `div()`s.

## 1. Window layout

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ TitleBar:  ◧ Dockering          [● Ubuntu-22.04 (WSL) ▾]          ⟳   ☾   ⚙        │
├───────────────┬──────────────────────────────────────────────────────────────────────┤
│ Sidebar       │ Containers                                        [Filter] [Group▾] │
│               │ [Search…        ]                       12 running · 3 stopped  [⋮] │
│ ▣ Containers 12├──────────────────────────────────────────────────────────────────────┤
│ ◫ Images     34│ DataTable / Tree-table                                              │
│ ⛁ Volumes     9│ ☐  Name ▴          Image         Status      CPU   Ports      Actions│
│ ⇄ Networks    5│ ▾  myshop (compose) 3/3 running              4.1%            ■ ⟳ ⋮ │
│               │ ☐   ├ web-1        nginx:1.27    Up 2h       0.3%  8080:80 ↗ ■ ⟳ ⋮ │
│               │ ☐   ├ api-1        myshop/api    Up 2h       3.7%  3000      ■ ⟳ ⋮ │
│               │ ☐   └ db-1         postgres:16   Up 2h       0.1%  5432      ■ ⟳ ⋮ │
│               │ ☐  redis           redis:7       Exited (0)                   ▶ ⟳ ⋮ │
│               │                                                                      │
├───────────────┴──────────────────────────────────────────────────────────────────────┤
│ ● Connected │ Docker 27.3.1 · API 1.47 · linux/amd64 │ via unix  CPUs 8 · RAM 16 GB │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

| Region | GPUI Kit component | Notes |
|---|---|---|
| Title bar | `TitleBar` (custom-drawn on Windows/Linux, native traffic lights on macOS) | Engine switcher in the centre, plus refresh, theme toggle, and settings |
| Engine switcher | `Popover` + `List` (or `Select`) | Status dot, kind icon, name, version; groups: *Local*, *WSL*, *WSLC*, *Remote*; footer: "Manage engines…" (Rescan lives in Settings → Engines, ENG-113) |
| Sidebar | `Sidebar` with the app's `NavMenu` items (icon, label, count badge) | Collapsible to icons (`SidebarToggleButton`). On Settings routes it switches to Settings mode: *Back to <Page>* plus the settings sections, same item component (SET-080) |
| Page header | `v_flex` of two rows: the title on the left and the view controls (filters, group-by, page buttons) on the right; then the search `Input` on the left and, on the right, the summary counts (or the selection actions while rows are checked, SHL-005) followed by the page `⋮`. A small gap separates the header from the table | `chrome::page_header` |
| Lists | `DataTable` (`TableState` + `TableDelegate`) | Grouped containers: the delegate flattens a tree into rows with `depth` and `expanded`, and renders a disclosure chevron in column 0 (CON-011) |
| Detail pages | `ui::segmented::Segmented` (icon + label), built on GPUI Kit base `Tabs`/`Tab` | SHL-025 |
| Key/value panels | `DescriptionList` under a section heading | `ui::section` |
| Charts | `AreaChart`, `LineChart` | |
| Raw JSON | `Input` in multi-line code-editor mode, read-only, with JSON syntax highlighting (`highlighter`) | |
| Confirmations | `Dialog` / `AlertDialog` | |
| Toasts | `Notification` via `window.push_notification` | |
| Empty states | `empty` component + illustration icon + CTA | |
| Loading | `Skeleton` rows (first load), `Spinner` in header (refresh) | |
| Status bar | `StatusBar` | Left: state dot and label │ engine kind label (foreground) + version · API · OS/arch (muted) │ `via <transport>` tag, separated by thin dividers. Right: CPUs/RAM, update item (UPD-008). The state dot is the only colour: never a route note or coloured chip (ENG-108, ENG-110) |

## 1a. Native menus & window chrome (SHL-020…)

- **SHL-020** macOS: a native app menu via GPUI `cx.set_menus`. *Dockering* (About, Settings… `Cmd+,`, Hide `Cmd+H`, Hide Others, Quit `Cmd+Q`), *Edit* (Undo/Redo/Cut/Copy/Paste/Select All, needed for text fields), *View* (pages `Cmd+1…4`, Toggle Sidebar, Command Palette, Theme), *Window* (Minimize `Cmd+M`, Zoom), *Help* (Keyboard Shortcuts, Open Logs Folder). Menu items dispatch the same actions as the keymap (KBD-002).
- **SHL-021** Windows/Linux: no menu bar. The same commands are in the title-bar overflow menu (`Alt` focuses it) and in the command palette.
- **SHL-022** Single instance per user. A second launch focuses the existing window and exits (10 §7).
- **SHL-023** Linux: a `.desktop` file and Wayland `app_id` = `dev.dockering.Dockering`, so the dock/taskbar icon matches.
- **SHL-025** Detail tabs are a segmented control (`ui::segmented::Segmented`): each tab shows an icon and a label, the selection slides between tabs, and the control hugs its content. Every segmented control in the app uses the same component and style: detail tabs, inner tab bars (Stats window, terminal sessions, shell picker) and list filters (CON-004, KBD-039). The selected segment must stand out clearly. In light mode it is a raised white pill (thin border, soft shadow) on a slightly darker trough. In dark mode it is a lighter pill on the trough.
- **SHL-024** UI language is **English only** in v1. All strings go through one module (`strings.rs`) so they can be localised later. OS high-contrast settings: the System theme follows the OS light/dark setting. A dedicated high-contrast theme is post-v1. UI zoom is `Mod+=` / `Mod+-` / `Mod+0` (scales the rem size).

## 2. Navigation model

```rust
pub enum Route {
    Containers,
    ContainerDetail { id: String, tab: ContainerTab },  // Overview|Logs|Terminal|Stats|Mounts|Network|Inspect
    Images,
    ImageDetail { id: String, tab: ImageTab },          // Overview|Layers|UsedBy|Inspect
    Volumes,
    VolumeDetail { name: String, tab: VolumeTab },      // Overview|UsedBy|Inspect
    Networks,
    NetworkDetail { id: String, tab: NetworkTab },      // Overview|Containers|Inspect
    Settings { section: SettingsSection },
}
```

- `Navigator` keeps back and forward stacks, bound to `Alt+←/→` (`Cmd+[`/`]` on macOS) and the mouse back/forward buttons.
- Opening a row (a click anywhere on the row, or `Enter`) pushes the detail route; on a group row the same click or `Enter` expands/collapses it. Controls inside a row (checkbox, row buttons, port links, copy id) act on their own and don't open the row. A modified click (`Mod`/`Shift`/`Alt`) only moves the cursor. Rows show a hover tint and a pointer cursor; names are plain text, not links. The *Actions* column is pinned to the right edge of every list and stays visible while the other columns scroll horizontally. On navigation, focus moves to the new page's primary control (KBD-007). A breadcrumb in the detail header shows `Containers › myshop › web-1`.
- Detail pages for a resource that disappears show a banner, "This container no longer exists", with a *Back to list* button. Logs and terminal content stays visible.
- Cross-links: container detail → image detail, volume detail, and network detail. Image detail → "Used by" containers. Volume detail → containers.

## 3. Global UX rules (SHL-xxx)

- **SHL-001** Every action that calls the engine shows progress within 100 ms. A row action replaces its button with a `Spinner`; a page action shows a header spinner.
- **SHL-002** Destructive actions (delete, prune, force-remove, kill) need a confirmation `Dialog`. The dialog says what will be removed and reclaimed. "Don't ask again" is offered only for delete of a *stopped* container.
- **SHL-003** Errors from actions → error `Notification` with the engine message and a *Copy details* action. Errors from list loads → inline panel with *Retry*.
- **SHL-004** Lists keep the previous data while refreshing. Rows never flicker or reorder unless the sort key changed.
- **SHL-005** Selection: checkbox column plus `Shift`/`Ctrl`-click. The checkbox column's header is a select-all checkbox: checked when every visible row is; a click selects every visible row, or clears a non-empty selection. While any row is checked (even one), the selection actions (count, page-specific actions such as Start/Stop, Delete, Clear) replace the summary counts in the page header's second row, left of the page `⋮`, and act on exactly the checked rows. Networks has no checkbox column.
- **SHL-026** Row actions column: fixed icon slots in the same position on every row: start/stop (toggles with state), restart, then `⋮` (the row menu with every other action, including Delete). Pages without start/restart show only the slots they have, ending with `⋮`. Delete is never an inline row button: it's reached through `⋮`, the context menu, `Del`, or the selection actions.
- **SHL-006** Search is a case-insensitive substring match over name, image, id prefix, and compose project. It is debounced by 120 ms and runs on `background_spawn` above 2,000 rows.
- **SHL-007** Relative times ("2 hours ago") refresh every 30 s through a single app-wide ticker, not per row.
- **SHL-008** Ids are shortened to 12 characters with a copy-to-clipboard button. Sizes use SI units (`1.2 GB`), as Docker Desktop does.
- **SHL-009** Theme: Light, Dark, or System. The default is System. GPUI Kit `Theme` is used with a violet accent (`#6E56CF`), also used for the row cursor, text selection, and the active sidebar page. Links and accent text are lifted in dark mode for contrast. Theme switching takes effect live.
- **SHL-010** Keyboard: the **whole UI is keyboard-operable**. Requirements, the full default keymap, focus regions, the command palette (`Mod+Shift+P`), and the shortcut reference (`Mod+/`, `F1`) are in [features/keyboard.md](features/keyboard.md) (KBD-*). Summary: `Mod+1..4` pages, `Mod+F` search, `Mod+R`/`F5` refresh, `Del` delete (confirm), `Mod+,` settings, `Mod+K` engine switcher, `F6` region cycling, single-letter row actions in lists.
- **SHL-011** Window state (size, position, sidebar collapsed, column widths, group expand state per engine) persists in `state.json`.
- **SHL-012** No modal blocks the app while an operation runs. Dialogs close immediately, and progress continues in the row or a notification.
- **SHL-013** Disconnected engine: list pages show a full-page `Alert` with the error, the hint, and *Retry* / *Switch engine*. Last known list data isn't shown, so the user can't act on stale ids. While **Degraded** (ping failing, reconnecting), data stays visible read-only with a banner and actions disabled. Open detail pages keep their logs and terminal scrollback visible, read-only.

## 4. Visual language

- Docker-Desktop-like density: 36 px rows, 13 px UI font, 12 px monospaced font for ids and ports.
- Status chips (`Tag::color`, soft: tinted background, coloured label, leading dot): Running = green, Paused = amber, Exited(0) = grey, Exited(≠0)/Dead = red, Restarting = sky with a spinner, Created = grey outline. Health shows as a second, outlined chip. *In use* chips use the same soft green.
- Sidebar: hover is a neutral tint, the active page an accent tint with an accent icon. While the sidebar region has keyboard focus, the cursor item gets the hover tint. Nothing draws a focus ring (KBD-003).
- Groups of content (detail sections, settings groups) have no surrounding border: a muted semibold title followed by a 1 px `border`-coloured line that runs to the right edge, then the content. Tables and lists inside keep their own border (`ui::section`).
- Ports render as links (`8080:80 ↗`). Clicking one opens `http://localhost:8080` in the default browser via `cx.open_url`.
- Icons come from the bundled GPUI Kit icon set (Lucide). App-specific icons go in `assets/icons/`.

## 5. Accessibility

- All interactive elements are focusable and have tooltips. Icon-only buttons have an accessible label (`.tooltip()`). Tooltips include the shortcut (KBD-010).
- Keyboard-only operation, visible focus, and focus management follow [features/keyboard.md](features/keyboard.md) (KBD-001…010).
- Colour is never the only status signal. Every status also has a text label.
- Minimum contrast follows the GPUI Kit theme defaults. Both themes are verified by screenshot review.
