# Feature: Container terminal (exec)

- **Status:** implemented (2026-10-02)
- **Requirement prefix:** TRM
- **Plan:** no per-feature plan; built in milestone M4 of the [v1 plan](../../plan/README.md) (spike S-1)

## Requirements

| ID | Requirement |
|---|---|
| TRM-001 | The Terminal tab of a **running** container opens an interactive shell inside the container (`exec`, TTY). When the container isn't running, the tab shows "Start the container to open a terminal" with a *Start* button. Without `EXEC_TTY`, the tab says the engine doesn't support terminals. |
| TRM-002 | The terminal is an embedded emulator (`dk-terminal`: `alacritty_terminal` grid + custom GPUI element). It supports 256-colour and truecolour, bold/italic/underline, cursor shapes, alternate screen (vim, htop), mouse reporting, bracketed paste, and scrollback of 10,000 lines (configurable, SET-040). |
| TRM-003 | Keyboard: all keys are forwarded through the keystroke interceptor (see *Implementation notes*), including Ctrl-combos, arrows, Home/End, and function keys. Copy: `Ctrl+Shift+C` on Windows and Linux, `Cmd+C` on macOS, plus the selection's context menu. Paste: `Ctrl+Shift+V` / `Cmd+V`. |
| TRM-004 | The shell is auto-detected (bash, falling back to sh, per [21 §5](../21-engine-api-contract.md#5-terminal-session-contract)). A dropdown offers auto, sh, bash, zsh, ash, or a custom command. The user field is optional. The default comes from Settings (SET-040). |
| TRM-005 | Resizing the view resizes the PTY (debounced 50 ms) when the engine has `EXEC_RESIZE`. |
| TRM-006 | When the process exits, the terminal shows "[process exited with code N] — press Enter to reconnect". |
| TRM-007 | Multiple terminal sessions per container are allowed, as sub-tabs inside the Terminal tab ("+" button). |
| TRM-008 | Terminal sessions survive switching to another tab or page *within the same engine* for up to 10 minutes. The session is kept in `TerminalRegistry` and closed when the engine switches or the container is removed. |
| TRM-009 | *Open in external terminal* (detail header overflow menu, Terminal tab toolbar, and the command palette): launches the configured terminal app (Settings, SET-040) with `docker exec -it <id> sh`, run on the hub as an argv (NFR-022). The template is split with simple quoting (`"…"` / `'…'` keep spaces; no escapes, variables, or globbing). An unquoted `{cmd}` word is replaced by the exec argv; without one, the exec argv is appended. Settings rejects a non-empty template without `{cmd}`. **v1 scope:** offered only when a template is configured, the engine has `EXEC_TTY`, and the active engine's transport is `npipe`, `unix`, `tcp`, or `tls`, where `docker exec` on the host reaches the same daemon. It's hidden for `bridge` (WSL distro), `com`/`cli` (WSLC), and unknown transports. The decision uses `EngineInfo.transport`, never `EngineKind` (ENG-030). From the palette where it's unavailable, it shows an info notification. *Follow-up:* a per-engine exec-command hint in `EngineInfo` (`wsl -d <distro> docker exec …`, `wslc container exec …`). |
| TRM-010 | Input latency (keypress → echo rendered) SHOULD be < 30 ms for a local engine. Output is coalesced at 4 ms. *Measured 2026-10-02 (debug build):* 20–32 ms, made up of the 4 ms batch plus waiting for the next frame. |
| TRM-011 | Font family and size are configurable in Settings. Default: platform monospace at 13 px. |
| TRM-012 | Keyboard focus: the terminal captures all keys. The reserved escape hatch `Mod+Shift+F6` leaves it, and sub-tab shortcuts apply, per [keyboard.md](keyboard.md) KBD-060…063. |

## Implementation notes

- `dk-terminal::TerminalModel` owns `alacritty_terminal::Term` **on the foreground thread** (no shared lock). It's fed bytes from `TerminalHandle.output` (10 §3.3) in 4 ms batches. VT parsing is cheap, and doing it in place avoids locking `Term` between parser and painter. Only damaged lines are re-shaped.
- **Text input** goes through GPUI's `InputHandler` (IME, dead keys, AltGr on Czech and similar layouts). Control/navigation keys and app chords go through the keystroke interceptor (next item).
- **Key capture (KBD-060).** The terminal reads keys through a GPUI **keystroke interceptor** that is active only while the terminal is focused, not through `on_key_down` handlers (GPUI resolves bindings before key-down listeners run). The interceptor encodes PTY-bound keys and stops propagation, so app bindings such as `Tab` and `F6` can't take keys meant for the shell. Reserved app chords (KBD-061/062) and plain text continue through normal dispatch: the keymap, then the `InputHandler`.
- `TerminalView` (GPUI `Element`) paints the cell grid with `ShapedLine`s per row, the cursor, and the selection. Intercepted keys → escape-sequence encoding (`dk-terminal::keys`) → `session.write()`.
- **Render cost (spike S-1):** a full reshape of a 209×52 grid takes 2.2–2.5 ms per frame; a frame with no changed rows takes about 0.3 ms.
- Zed's terminal crate (`crates/terminal`, `crates/terminal_view`) is the reference design: it uses alacritty_terminal + GPUI. We reimplement a minimal version because it isn't published on crates.io (both Zed `terminal` and `terminal_view` are **GPL**, so they're reference only and nothing is copied; REL-003).

## Verification (2026-10-02)

| ID | Tests |
|---|---|
| TRM-001 | `trm_001_not_running_shows_start`, `trm_001_opens_session_and_typing_echoes` |
| TRM-002 | `trm_002_*` (SGR, truecolour, alt screen, scrollback cap, 256 colours, mouse reporting) |
| TRM-003 | `trm_003_ctrl_c_is_etx`, `kbd_060_tab_and_esc_sent_to_pty`, `kbd_060_terminal_keys_win_over_app_bindings` |
| TRM-004 | `trm_004_shell_argv`, `trm_004_shell_and_user_in_exec_request` |
| TRM-005 | `trm_005_resize` |
| TRM-006 | `trm_006_exit_line_and_reconnect_on_enter`, `trm_006_exit_then_reconnect_in_same_sub_tab` |
| TRM-007 | `trm_007_sub_tabs_new_switch_close` |
| TRM-008 | `trm_008_*` (hub actor, view: navigation, engine switch, container removal, stuck writes) |
| TRM-009 | `trm_009_external_argv`, `trm_009_external_argv_with_quotes`, `trm_009_template_quoting`, `trm_009_offered_only_where_host_docker_exec_works`, `trm_009_external_terminal_hidden_for_unsupported_transport` |
| TRM-010 | `trm_010_parse_full_screen_timing`; latency measured manually (20–32 ms, debug build) |
| TRM-011 | `set_030_040_050_numbers_text_and_switches_persist` (settings); render checked manually |
| TRM-012 | `kbd_061_leave_terminal_focuses_tab_bar`, `kbd_061_escape_hatch_not_sent_to_pty`, `kbd_062_reserved_chords_pass_to_app` |

## Known gaps (v1)

- TRM-009 is unavailable for WSL distro and WSLC engines (see TRM-009).
- TRM-010 has been measured on a debug build only. A release-build measurement is in the [release checklist](../../plan/release-checklist.md).
- The 10-minute TRM-008 idle timeout isn't covered by a test that waits out the timer. The tests cover survival and the engine-switch and removal closes.
