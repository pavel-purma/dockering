# Spike S-8: focus APIs, DataTable key overrides, physical-key bindings (2026-10-02)

Scope: keyboard plan task 1 (KBD-004, KBD-010, KBD-085). Read against the sources of
`gpui-pre 0.3.7`, `gpui-pre-{windows,macos,linux} 0.3.7`, `gpui-component 0.7.0` and
`gpui-base 0.7.0`, then implemented in `crates/dockering` (`keymap.rs`,
`ui/list_table/`, `ui/confirm.rs`) and covered by view tests.

## Summary

| # | Question | Result |
|---|---|---|
| S-8.1 | Do bindings match logical or physical keys? | **Logical** on all OSes. A typed keystroke matches a binding by `key` (the layout's character) or `key_char`, never by scancode (`Keystroke::should_match`). |
| S-8.2 | Can a binding be pinned to the US-QWERTY *position*? | **Windows: yes**, via `KeyBinding::load(.., use_key_equivalents = true, .., cx.keyboard_mapper())`; the Windows mapper translates the binding's key through a fixed US vkey table (`get_vkey_from_key_with_us_layout`) to the current layout's character on that key. **macOS: partially**: the mapper applies the layout's key-equivalent table (only a few layouts remap). **Linux: no**: the platform uses `DummyKeyboardMapper`. |
| S-8.3 | Does GPUI shift-normalise digit/punctuation keys? | Windows converts `Shift+<OEM/digit>` into the shifted character and clears `shift` (`get_keystroke_key`), so `ctrl-+` and `ctrl-shift-=` are different strings for the same physical chord. Bind both forms when it matters (we bind `secondary-=` and `secondary-+` for zoom in). |
| S-8.4 | How to override the GPUI Kit `DataTable` bindings (`tab`/`shift-tab` → columns, `left`/`right` → columns, `escape` → clear)? | Bind the same keys with predicate **`ListTable > DataTable`**. Precedence is by context depth first, then by insertion order (later wins); both bindings match at the `DataTable` depth, ours are installed after `gpui_kit::init`, so ours win. A plain `ListTable` predicate would match one level *shallower* and lose. |
| S-8.5 | Is there a declarative "initial focus" for GPUI Kit dialogs? | No. `WindowState::open_dialog` focuses the dialog host. To put focus on *Cancel* (KBD-071) the body must own the focus handle (component `Button` creates its own keyed handle and ignores `InteractiveElement::track_focus`), so `ui/confirm.rs` wraps each button in a focusable `div` with our handle, a focus ring, and Enter/Space activation; the body focuses Cancel on its first render. `on_next_frame` does not run under `TestAppContext`, so focus changes are applied during render. |
| S-8.6 | `Kbd::binding_for_action` / tooltips | Works from the live keymap. `Button::tooltip_with_action(label, &action, context)` resolves the highest-precedence binding for the action in that context, so hints follow the OS table automatically (KBD-010). The `Command` palette resolves hints per item the same way. |
| S-8.7 | Tab order / roving focus | `FocusHandle::tab_stop(true)` + `track_focus` puts an element in the Tab order (`TabStopMap`, sorted by `tab_index` path then paint order). `TableState`'s handle is a tab stop, so the table is **one** stop; arrows move the cursor inside. `Tab`/`Shift+Tab` in the table are rebound (`list::FocusNext/FocusPrev`) to leave it. Dialog focus traps are provided by `gpui-base` (`focus_trap` + `Root` `Tab` handling). |
| S-8.8 | Keyboard context menus | GPUI Kit `context_menu()` opens only on a right mouse-down at a position. For `Shift+F10`/`Menu` we own a `PopupMenu` (`ui/menu.rs` `KeyMenu`) anchored at the cursor row, focused on open, `action_context` = the table so Esc restores focus. |

## KBD-085 decision

`keymap::install` loads every binding once logically and, for digit/punctuation keys
(`Mod+1…4`, `Mod+/`, `Mod+,`, `Mod+[`/`]`, zoom), a second time with
`use_key_equivalents = true`. Effects:

- **Windows** (Czech, German, …): `Ctrl+2` matches the top-row key that types `ě` on
  Czech; `Ctrl+/` matches the key left of right-Shift. Both the physical and (where the
  layout produces it) the logical form work. On US layouts the twin is identical and
  harmless.
- **macOS**: the twin uses the system key-equivalent table, which covers the Cmd-shortcut
  conventions Apple itself applies; Czech `Cmd+1…4` already deliver digits via
  `key_char`-insensitive Cmd handling.
- **Linux**: no mapper, so only logical matching. With Czech the top row yields `+ľščť…`;
  `Ctrl+2` therefore needs the user to use the numpad or a US layout. Follow-up: generate
  layout-specific twins at startup from xkb (`keystroke_from_xkb` has the keymap) when GPUI
  exposes it, or upstream a Linux `PlatformKeyboardMapper`. Tracked as a known gap; the
  palette (`Mod+Shift+P`, a letter chord) always works.

## Tests that pin the behaviour

`keymap_no_conflicts_{windows,macos,linux}`, `keymap_no_altgr_chords`,
`keymap_no_reserved_chords`, `keymap_single_letters_only_in_list_contexts`,
`a11y_every_action_bound_or_in_palette`, and the view tests
`kbd_005_f6_cycles_regions`, `kbd_006_escape_priority_order`,
`kbd_033_left_right_collapse_group` (proves the `ListTable > DataTable` override beats the
built-in `left`/`right` column bindings), `con_021_running_delete_needs_force` (initial
focus on Cancel, `Mod+Enter` confirms), `kbd_023_mod_digits_navigate_and_back_forward`.
