# Feature: Settings

- **Status:** planned
- **Requirement prefix:** SET
- **Plan:** [docs/plan/features/settings.md](../../plan/features/settings.md) (to be written)

Settings is a full page (route `Settings { section }`) built with GPUI Kit `Settings` / `Form`
components. Changes apply immediately and persist via `dk-hub::config` (debounced save).

| Section | ID | Settings |
|---|---|---|
| General | SET-001 | Theme (System/Light/Dark) · Start page (Containers/Images/Volumes) · Confirm before deleting stopped containers · Show Networks page |
| Engines | SET-010 | See ENG-104/105 · "Show all WSL distros" · "Show all WSLC sessions" · Rescan |
| Containers | SET-020 | Group by default (Compose/None/Label) · Custom group label key · Show CPU/Memory columns · Polling interval fallback (s) |
| Logs | SET-030 | Initial tail lines (default 1000) · Max buffer lines (default 50k) · Timestamps default · Wrap default |
| Terminal | SET-040 | Font family · Font size · Default shell · Scrollback lines · External terminal command (TRM-009) |
| Stats | SET-050 | History window (default 15 min) · CPU % relative to all cores |
| Diagnostics | SET-060 | Log level · Open logs folder · Copy diagnostics · Version / licences |
| Keyboard | SET-070 | Read-only keymap view (the same as the shortcut reference, KBD-022). Rebinding is reserved for post-v1 (KBD-081). |
