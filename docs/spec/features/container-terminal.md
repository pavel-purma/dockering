# Feature: Container terminal (exec)

- **Status:** planned
- **Requirement prefix:** TRM
- **Plan:** [docs/plan/features/container-terminal.md](../../plan/features/container-terminal.md) (to be written)

## Requirements

| ID | Requirement |
|---|---|
| TRM-001 | The Terminal tab of a **running** container opens an interactive shell inside the container (`exec`, TTY). When the container isn't running, the tab shows "Start the container to open a terminal" with a *Start* button. |
| TRM-002 | The terminal is an embedded emulator (`dk-terminal`: `alacritty_terminal` grid + custom GPUI element). It supports 256-colour and truecolour, bold/italic/underline, cursor shapes, alternate screen (vim, htop), mouse reporting, bracketed paste, and scrollback of 10,000 lines. |
| TRM-003 | Keyboard: all keys are forwarded, including Ctrl-combos, arrows, Home/End, and function keys. Copy: `Ctrl+Shift+C` on Windows and Linux, `Cmd+C` on macOS, plus the selection's context menu. Paste: `Ctrl+Shift+V` / `Cmd+V`. |
| TRM-004 | The shell is auto-detected (bash, falling back to sh, per [21 §5](../21-engine-api-contract.md#5-terminal-session-contract)). A dropdown offers sh, bash, zsh, ash, or a custom command. The user field is optional. |
| TRM-005 | Resizing the view resizes the PTY (debounced 50 ms) when the engine has `EXEC_RESIZE`. |
| TRM-006 | When the process exits, the terminal shows "[process exited with code N] — press Enter to reconnect". |
| TRM-007 | Multiple terminal sessions per container are allowed, as sub-tabs inside the Terminal tab ("+" button). |
| TRM-008 | Terminal sessions survive switching to another tab or page *within the same engine* for up to 10 minutes. The session is kept in `TerminalRegistry` and closed when the engine switches or the container is removed. |
| TRM-009 | *Open in external terminal* (overflow menu): launches the OS terminal with `docker exec -it <id> sh` (or `wsl -d <distro> docker exec …` / `wslc container exec …`), using the configured terminal app in Settings. Optional; it MAY be deferred. |
| TRM-010 | Input latency (keypress → echo rendered) SHOULD be < 30 ms for a local engine. Output is coalesced at 4 ms. |
| TRM-011 | Font family and size are configurable in Settings. Default: platform monospace at 13 px. |
| TRM-012 | Keyboard focus: the terminal captures all keys. The reserved escape hatch `Mod+Shift+F6` leaves it, and sub-tab shortcuts apply, per [keyboard.md](keyboard.md) KBD-060…063. |

## Implementation notes

- `dk-terminal::TerminalModel` owns `alacritty_terminal::Term` **on the foreground thread** (no shared lock). It's fed bytes from `TerminalHandle.output` (10 §3.3) in 4 ms batches. VT parsing is cheap, and doing it in place avoids locking `Term` between parser and painter. Only damaged lines are re-shaped.
- **Text input** goes through GPUI's `InputHandler` (IME, dead keys, AltGr on Czech and similar layouts). `KeyDownEvent` handles only control/navigation keys and app chords.
- `TerminalView` (GPUI `Element`) paints the cell grid with `ShapedLine`s per row, the cursor, and the selection. It handles `KeyDownEvent` → escape-sequence encoding (`dk-terminal::keys`) → `session.write()`.
- Zed's terminal crate (`crates/terminal`, `crates/terminal_view`) is the reference design: it uses alacritty_terminal + GPUI. We reimplement a minimal version because it isn't published on crates.io (both Zed `terminal` and `terminal_view` are **GPL**, so they're reference only and nothing is copied; REL-003).
