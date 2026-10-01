# 30 — UI Shell

All UI is built with **GPUI Kit** (`gpui-kit` 0.7: `gpui` + `gpui-component`). Custom
elements are allowed only where GPUI Kit has no equivalent. The terminal grid is the only
planned one. Prefer GPUI Kit components and theme tokens over hand-styled `div()`s.

## 1. Window layout

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ TitleBar:  ◧ Dockering          [● Ubuntu-22.04 (WSL) ▾]          ⟳   ☾   ⚙        │
├───────────────┬──────────────────────────────────────────────────────────────────────┤
│ Sidebar       │ Page header:  Containers                     [Search…    ] [Filter▾]│
│               │               12 running · 3 stopped          [Group ▾] [⋮ Prune ] │
│ ▣ Containers 12├──────────────────────────────────────────────────────────────────────┤
│ ◫ Images     34│ DataTable / Tree-table                                              │
│ ⛁ Volumes     9│ ☐  Name ▴          Image         Status      CPU   Ports      Actions│
│ ⇄ Networks    5│ ▾  myshop (compose) 3/3 running              4.1%            ▶ ■ 🗑 │
│               │ ☐   ├ web-1        nginx:1.27    Up 2h       0.3%  8080:80 ↗ ■ ⟳ 🗑 │
│               │ ☐   ├ api-1        myshop/api    Up 2h       3.7%  3000      ■ ⟳ 🗑 │
│               │ ☐   └ db-1         postgres:16   Up 2h       0.1%  5432      ■ ⟳ 🗑 │
│               │ ☐  redis           redis:7       Exited (0)                   ▶ 🗑 │
│               │                                                                      │
├───────────────┴──────────────────────────────────────────────────────────────────────┤
│ StatusBar: ● Connected · Docker 27.3.1 · API 1.47 · linux/amd64        RAM —  CPU — │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

| Region | GPUI Kit component | Notes |
|---|---|---|
| Title bar | `TitleBar` (custom-drawn on Windows/Linux, native traffic lights on macOS) | Engine switcher in the centre, plus refresh, theme toggle, and settings |
| Engine switcher | `Popover` + `List` (or `Select`) | Status dot, kind icon, name, version; groups: *Local*, *WSL*, *WSLC*, *Remote*; footer: "Manage engines…", "Rescan" |
| Sidebar | `Sidebar`, `SidebarGroup`, `SidebarMenu`, `SidebarMenuItem` with `.suffix(Badge)` counts | Collapsible to icons (`SidebarToggleButton`) |
| Page header | `h_flex` + `Input` (search) + `DropdownButton`/`Select` (filters, group-by) + `Button`s | |
| Lists | `DataTable` (`TableState` + `TableDelegate`) | Grouped containers: the delegate flattens a tree into rows with `depth` and `expanded`, and renders a disclosure chevron in column 0 (CON-011) |
| Detail pages | `TabBar` + `Tab` | |
| Key/value panels | `DescriptionList`, `GroupBox` | |
| Charts | `AreaChart`, `LineChart` | |
| Raw JSON | `Input` in multi-line code-editor mode, read-only, with JSON syntax highlighting (`highlighter`) | |
| Confirmations | `Dialog` / `AlertDialog` | |
| Toasts | `Notification` via `window.push_notification` | |
| Empty states | `empty` component + illustration icon + CTA | |
| Loading | `Skeleton` rows (first load), `Spinner` in header (refresh) | |
| Status bar | `StatusBar` | |

## 1a. Native menus & window chrome (SHL-020…)

- **SHL-020** macOS: a native app menu via GPUI `cx.set_menus`. *Dockering* (About, Settings… `Cmd+,`, Hide `Cmd+H`, Hide Others, Quit `Cmd+Q`), *Edit* (Undo/Redo/Cut/Copy/Paste/Select All, needed for text fields), *View* (pages `Cmd+1…4`, Toggle Sidebar, Command Palette, Theme), *Window* (Minimize `Cmd+M`, Zoom), *Help* (Keyboard Shortcuts, Open Logs Folder). Menu items dispatch the same actions as the keymap (KBD-002).
- **SHL-021** Windows/Linux: no menu bar. The same commands are in the title-bar overflow menu (`Alt` focuses it) and in the command palette.
- **SHL-022** Single instance per user. A second launch focuses the existing window and exits (10 §7).
- **SHL-023** Linux: a `.desktop` file and Wayland `app_id` = `dev.dockering.Dockering`, so the dock/taskbar icon matches.
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
- Opening a row (double-click, `Enter`, or clicking the name link) pushes the detail route. On navigation, focus moves to the new page's primary control (KBD-007). A breadcrumb in the detail header shows `Containers › myshop › web-1`.
- Detail pages for a resource that disappears show a banner, "This container no longer exists", with a *Back to list* button. Logs and terminal content stays visible.
- Cross-links: container detail → image detail, volume detail, and network detail. Image detail → "Used by" containers. Volume detail → containers.

## 3. Global UX rules (SHL-xxx)

- **SHL-001** Every action that calls the engine shows progress within 100 ms. A row action replaces its button with a `Spinner`; a page action shows a header spinner.
- **SHL-002** Destructive actions (delete, prune, force-remove, kill) need a confirmation `Dialog`. The dialog says what will be removed and reclaimed. "Don't ask again" is offered only for delete of a *stopped* container.
- **SHL-003** Errors from actions → error `Notification` with the engine message and a *Copy details* action. Errors from list loads → inline panel with *Retry*.
- **SHL-004** Lists keep the previous data while refreshing. Rows never flicker or reorder unless the sort key changed.
- **SHL-005** Selection: checkbox column plus `Shift`/`Ctrl`-click. A bulk action bar appears when 2 or more rows are selected (Start, Stop, Delete).
- **SHL-006** Search is a case-insensitive substring match over name, image, id prefix, and compose project. It is debounced by 120 ms and runs on `background_spawn` above 2,000 rows.
- **SHL-007** Relative times ("2 hours ago") refresh every 30 s through a single app-wide ticker, not per row.
- **SHL-008** Ids are shortened to 12 characters with a copy-to-clipboard button. Sizes use SI units (`1.2 GB`), as Docker Desktop does.
- **SHL-009** Theme: Light, Dark, or System. The default is System. GPUI Kit `Theme` is used with a Docker-like accent (`#1D63ED`). Theme switching takes effect live.
- **SHL-010** Keyboard: the **whole UI is keyboard-operable**. Requirements, the full default keymap, focus regions, the command palette (`Mod+Shift+P`), and the shortcut reference (`Mod+/`, `F1`) are in [features/keyboard.md](features/keyboard.md) (KBD-*). Summary: `Mod+1..4` pages, `Mod+F` search, `Mod+R`/`F5` refresh, `Del` delete (confirm), `Mod+,` settings, `Mod+K` engine switcher, `F6` region cycling, single-letter row actions in lists.
- **SHL-011** Window state (size, position, sidebar collapsed, column widths, group expand state per engine) persists in `state.json`.
- **SHL-012** No modal blocks the app while an operation runs. Dialogs close immediately, and progress continues in the row or a notification.
- **SHL-013** Disconnected engine: list pages show a full-page `Alert` with the error, the hint, and *Retry* / *Switch engine*. Last known list data isn't shown, so the user can't act on stale ids. While **Degraded** (ping failing, reconnecting), data stays visible read-only with a banner and actions disabled. Open detail pages keep their logs and terminal scrollback visible, read-only.

## 4. Visual language

- Docker-Desktop-like density: 36 px rows, 13 px UI font, 12 px monospaced font for ids and ports.
- Status chips (`Tag`): Running = green, Paused = amber, Exited(0) = grey, Exited(≠0)/Dead = red, Restarting = blue with a spinner, Created = grey outline. Health shows as a second chip.
- Ports render as links (`8080:80 ↗`). Clicking one opens `http://localhost:8080` in the default browser via `cx.open_url`.
- Icons come from the bundled GPUI Kit icon set (Lucide). App-specific icons go in `assets/icons/`.

## 5. Accessibility

- All interactive elements are focusable and have tooltips. Icon-only buttons have an accessible label (`.tooltip()`). Tooltips include the shortcut (KBD-010).
- Keyboard-only operation, visible focus, and focus management follow [features/keyboard.md](features/keyboard.md) (KBD-001…010).
- Colour is never the only status signal. Every status also has a text label.
- Minimum contrast follows the GPUI Kit theme defaults. Both themes are verified by screenshot review.
