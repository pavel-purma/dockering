//! TerminalView tests in a headless GPUI Kit window (TRM-002, TRM-005, TRM-006, TRM-010,
//! KBD-060, KBD-061).
//!
//! Imports are explicit: with `test-support`, `use gpui_kit::*` would shadow `#[test]`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use dk_terminal::actions::{Copy, Paste, ScrollPageUp};
use dk_terminal::{TerminalConfig, TerminalEvent, TerminalView};
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ClipboardItem, Entity, Focusable, Point,
    TestAppContext, WindowBounds, WindowOptions, px, size,
};

struct Harness {
    window: AnyWindowHandle,
    view: Entity<TerminalView>,
    events: UnboundedReceiver<TerminalEvent>,
}

fn open(cx: &mut TestAppContext) -> Harness {
    cx.update(gpui_kit::init);
    cx.update(|cx| cx.bind_keys(dk_terminal::default_key_bindings()));
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.), px(480.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| TerminalView::new(TerminalConfig::default(), window, cx)),
        )
        .expect("open test window")
    });
    let events = cx.events(&view);
    cx.update_window(window, |_, window, cx| {
        let focus = view.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        window.render_frame(cx);
    })
    .expect("window open");
    cx.run_until_parked();
    Harness {
        window,
        view,
        events,
    }
}

impl Harness {
    fn drain(&mut self) -> Vec<TerminalEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    /// All `Input` bytes, concatenated; other events are dropped.
    fn input(&mut self) -> Vec<u8> {
        self.drain()
            .into_iter()
            .filter_map(|e| match e {
                TerminalEvent::Input(bytes) => Some(bytes.to_vec()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn render(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .expect("window open");
    }

    fn feed(&self, cx: &mut TestAppContext, bytes: &[u8]) {
        self.view.update(cx, |view, cx| view.feed(bytes, cx));
    }
}

fn settle(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn trm_006_exit_line_and_reconnect_on_enter(cx: &mut TestAppContext) {
    let mut h = open(cx);
    h.feed(cx, b"$ exit\r\n");
    cx.simulate_keystrokes(h.window, "enter");
    assert_eq!(h.input(), b"\r");

    h.view.update(cx, |view, cx| view.set_exited(Some(130), cx));
    h.render(cx);
    let text = h.view.read_with(cx, |view, _| view.grid_text());
    assert!(
        text.contains("[process exited with code 130] \u{2014} press Enter to reconnect"),
        "{text}"
    );
    assert!(h.view.read_with(cx, |view, _| view.is_exited()));

    // Typing doesn't reach the dead process; Enter asks to reconnect (once).
    cx.simulate_keystrokes(h.window, "ctrl-c");
    cx.simulate_input(h.window, "ls");
    cx.simulate_keystrokes(h.window, "enter enter");
    let events = h.drain();
    assert_eq!(events, vec![TerminalEvent::ReconnectRequested]);

    // A new session clears the exited state.
    h.view.update(cx, |view, cx| view.reset(cx));
    assert!(!h.view.read_with(cx, |view, _| view.is_exited()));
    assert_eq!(h.view.read_with(cx, |view, _| view.grid_text()).trim(), "");
    cx.simulate_keystrokes(h.window, "enter");
    assert_eq!(h.input(), b"\r");
}

#[gpui_kit::test]
fn feed_coalesces_notifications(cx: &mut TestAppContext) {
    let h = open(cx);
    let notified = Rc::new(Cell::new(0usize));
    let _sub = cx.update(|cx| {
        let notified = notified.clone();
        cx.observe(&h.view, move |_, _| notified.set(notified.get() + 1))
    });

    for i in 0..200 {
        h.feed(cx, format!("chunk {i}\r\n").as_bytes());
    }
    // Parsed immediately, painted later.
    let text = h.view.read_with(cx, |view, _| view.grid_text());
    assert!(text.contains("chunk 199"), "{text}");
    assert_eq!(notified.get(), 0);

    settle(cx, 3);
    assert_eq!(notified.get(), 0, "no repaint before the 4 ms window ends");
    settle(cx, 2);
    assert_eq!(notified.get(), 1, "one repaint per 4 ms window (TRM-010)");

    h.feed(cx, b"more");
    settle(cx, 5);
    assert_eq!(notified.get(), 2);
}

#[gpui_kit::test]
fn paste_wraps_bracketed(cx: &mut TestAppContext) {
    let mut h = open(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("echo hi\nls".into()));

    cx.dispatch_action(h.window, Paste);
    assert_eq!(h.input(), b"echo hi\rls");

    h.feed(cx, b"\x1b[?2004h");
    cx.dispatch_action(h.window, Paste);
    assert_eq!(h.input(), b"\x1b[200~echo hi\rls\x1b[201~");

    // An embedded end marker can't terminate the paste early.
    cx.write_to_clipboard(ClipboardItem::new_string("a\x1b[201~b".into()));
    cx.dispatch_action(h.window, Paste);
    assert_eq!(h.input(), b"\x1b[200~ab\x1b[201~");

    // The paste chord is bound in the terminal context (TRM-003).
    let chord = if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-shift-v"
    };
    cx.simulate_keystrokes(h.window, chord);
    assert_eq!(h.input(), b"\x1b[200~ab\x1b[201~");
}

#[gpui_kit::test]
fn read_only_emits_no_input(cx: &mut TestAppContext) {
    let mut h = open(cx);
    h.view.update(cx, |view, cx| view.set_read_only(true, cx));
    h.render(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("x".into()));

    cx.simulate_keystrokes(h.window, "ctrl-c enter tab up");
    cx.simulate_input(h.window, "abc");
    cx.dispatch_action(h.window, Paste);
    // Terminal replies (DSR) are dropped too.
    h.feed(cx, b"\x1b[6n");
    assert_eq!(h.input(), b"");

    h.view.update(cx, |view, cx| view.set_read_only(false, cx));
    h.render(cx);
    cx.simulate_keystrokes(h.window, "ctrl-c");
    assert_eq!(h.input(), [0x03]);
}

#[gpui_kit::test]
fn resize_event_debounced(cx: &mut TestAppContext) {
    let mut h = open(cx);
    let resizes = |h: &mut Harness| -> Vec<(u16, u16)> {
        h.drain()
            .into_iter()
            .filter_map(|e| match e {
                TerminalEvent::Resize { cols, rows } => Some((cols, rows)),
                _ => None,
            })
            .collect()
    };

    // First layout: reported once the size was stable for 50 ms (TRM-005).
    settle(cx, 40);
    assert!(resizes(&mut h).is_empty());
    settle(cx, 20);
    let first = h.view.read_with(cx, |view, _| view.size());
    assert_ne!(first, (80, 24), "the grid follows the window size");
    assert_eq!(resizes(&mut h), vec![first]);

    // A burst of resizes produces one event with the final size.
    for width in [400., 500., 600.] {
        cx.simulate_window_resize(h.window, size(px(width), px(300.)));
        h.render(cx);
        settle(cx, 20);
    }
    // The model follows immediately; the event waits for the debounce.
    let last = h.view.read_with(cx, |view, _| view.size());
    assert_ne!(last, first);
    assert!(resizes(&mut h).is_empty());
    settle(cx, 50);
    assert_eq!(resizes(&mut h), vec![last]);

    // Returning to the reported size before the debounce fires reports nothing.
    cx.simulate_window_resize(h.window, size(px(300.), px(300.)));
    h.render(cx);
    settle(cx, 10);
    cx.simulate_window_resize(h.window, size(px(600.), px(300.)));
    h.render(cx);
    settle(cx, 100);
    assert!(resizes(&mut h).is_empty());
}

#[gpui_kit::test]
fn kbd_060_terminal_keys_win_over_app_bindings(cx: &mut TestAppContext) {
    let mut h = open(cx);
    // Root binds tab/shift-tab to focus navigation; in the terminal they go to the PTY.
    cx.simulate_keystrokes(h.window, "tab shift-tab escape f6 ctrl-r up");
    assert_eq!(h.input(), b"\t\x1b[Z\x1b\x1b[17~\x12\x1b[A");
    let focused = cx.update_window(h.window, |_, window, cx| {
        h.view.read(cx).focus_handle(cx).is_focused(window)
    });
    assert_eq!(focused.ok(), Some(true), "focus stays in the terminal");

    // DECCKM switches arrows to SS3.
    h.feed(cx, b"\x1b[?1h");
    cx.simulate_keystrokes(h.window, "up");
    assert_eq!(h.input(), b"\x1bOA");

    // Text arrives through the input handler (KBD-083).
    cx.simulate_input(h.window, "ls -la");
    assert_eq!(h.input(), b"ls -la");
}

#[gpui_kit::test]
fn kbd_061_reserved_chords_are_not_sent(cx: &mut TestAppContext) {
    let mut h = open(cx);
    let chords = if cfg!(target_os = "macos") {
        "cmd-shift-f6 cmd-escape ctrl-tab cmd-shift-t cmd-shift-w ctrl-pageup"
    } else {
        "ctrl-shift-f6 ctrl-tab ctrl-shift-tab ctrl-shift-t ctrl-shift-w ctrl-pageup ctrl-pagedown"
    };
    cx.simulate_keystrokes(h.window, chords);
    assert_eq!(h.input(), b"");
}

#[gpui_kit::test]
fn copy_selection_and_scrollback(cx: &mut TestAppContext) {
    let mut h = open(cx);
    for i in 0..200 {
        h.feed(cx, format!("line {i}\r\n").as_bytes());
    }
    h.render(cx);

    // Shift+PgUp (KBD-063) passes through the encoder to the keymap binding.
    cx.simulate_keystrokes(h.window, "shift-pageup");
    assert!(
        h.view
            .read_with(cx, |view, _| view.model().display_offset())
            > 0
    );
    assert_eq!(h.input(), b"");
    cx.simulate_keystrokes(h.window, "shift-pagedown");
    assert_eq!(
        h.view
            .read_with(cx, |view, _| view.model().display_offset()),
        0
    );

    // Page up scrolls into history; typing jumps back to the live screen.
    cx.dispatch_action(h.window, ScrollPageUp);
    let offset = h
        .view
        .read_with(cx, |view, _| view.model().display_offset());
    assert!(offset > 0);
    cx.simulate_input(h.window, "x");
    assert_eq!(h.input(), b"x");
    assert_eq!(
        h.view
            .read_with(cx, |view, _| view.model().display_offset()),
        0
    );

    // Copy writes the selection to the clipboard.
    h.view.update(cx, |view, cx| {
        view.feed(b"\x1b[2J\x1b[Hhello world", cx);
    });
    h.render(cx);
    select(cx, &h, 0, 0, 0, 4);
    assert_eq!(
        h.view.read_with(cx, |view, _| view.selection_text()),
        Some("hello".to_string())
    );
    cx.dispatch_action(h.window, Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("hello".to_string())
    );
}

/// Drag-select from (row, col) to (row, col) with the mouse.
fn select(
    cx: &mut TestAppContext,
    h: &Harness,
    row0: usize,
    col0: usize,
    row1: usize,
    col1: usize,
) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    // Cell geometry from the test text system: 0.6 em wide, 1.2 em high, 4 px padding,
    // 1 px border.
    let cell_w = 13.0 * 0.6;
    let line_h = (13.0f32 * 1.2).round();
    let at = |row: usize, col: usize| {
        gpui_kit::point(
            px(5.0 + cell_w * col as f32 + 1.0),
            px(5.0 + line_h * row as f32 + 2.0),
        )
    };
    let to = gpui_kit::point(
        px(5.0 + cell_w * (col1 + 1) as f32 - 1.0),
        px(5.0 + line_h * row1 as f32 + 2.0),
    );
    let from = at(row0, col0);
    cx.update_window(h.window, |_, window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(MouseMoveEvent {
                position: to,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: to,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    })
    .expect("window open");
}

#[gpui_kit::test]
fn trm_002_mouse_reporting_and_wheel(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase};
    let mut h = open(cx);
    let wheel = |cx: &mut TestAppContext, h: &Harness, lines: f32| {
        cx.update_window(h.window, |_, window, cx| {
            window.dispatch_event(
                gpui_kit::PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: gpui_kit::point(px(20.), px(20.)),
                    delta: ScrollDelta::Lines(gpui_kit::point(0., lines)),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        })
        .unwrap();
    };

    // Alternate screen without mouse mode: the wheel sends arrow keys.
    h.feed(cx, b"\x1b[?1049h");
    h.render(cx);
    wheel(cx, &h, 2.0);
    assert_eq!(h.input(), b"\x1b[A\x1b[A");

    // SGR mouse mode: wheel and clicks are reported.
    h.feed(cx, b"\x1b[?1000h\x1b[?1006h");
    h.render(cx);
    wheel(cx, &h, -1.0);
    let bytes = h.input();
    assert!(bytes.starts_with(b"\x1b[<65;"), "{bytes:?}");
    select(cx, &h, 0, 0, 0, 0);
    let bytes = String::from_utf8(h.input()).unwrap();
    assert!(
        bytes.starts_with("\x1b[<0;1;1M") && bytes.ends_with('m'),
        "{bytes:?}"
    );
}

#[gpui_kit::test]
fn unchanged_rows_are_not_reshaped(cx: &mut TestAppContext) {
    let h = open(cx);
    h.feed(cx, b"one\r\ntwo\r\nthree");
    h.render(cx);
    let (_, rows) = h.view.read_with(cx, |view, _| view.size());
    // Redraw without changes: everything comes from the cache.
    cx.update_window(h.window, |_, window, cx| {
        window.refresh();
        window.render_frame(cx);
    })
    .expect("window open");
    assert_eq!(
        h.view
            .read_with(cx, |view, _| view.rows_shaped_last_frame()),
        0
    );

    // Typing on one line reshapes only that row.
    h.feed(cx, b"!");
    h.render(cx);
    let shaped = h
        .view
        .read_with(cx, |view, _| view.rows_shaped_last_frame());
    assert_eq!(shaped, 1, "only the edited row is reshaped (of {rows})");
}

#[gpui_kit::test]
fn ime_composition_owns_keys(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    let mut h = open(cx);
    cx.update_window(h.window, |_, window, cx| {
        h.view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "ka", None, window, cx)
        });
    })
    .expect("window open");
    // Navigation keys belong to the IME while composing; they aren't encoded for the PTY.
    cx.simulate_keystrokes(h.window, "left backspace");
    assert_eq!(h.input(), b"");
    cx.update_window(h.window, |_, window, cx| {
        h.view.update(cx, |view, cx| {
            view.replace_text_in_range(None, "\u{304b}", window, cx)
        });
    })
    .expect("window open");
    assert_eq!(h.input(), "\u{304b}".as_bytes());
    cx.simulate_keystrokes(h.window, "enter");
    assert_eq!(h.input(), b"\r");
}
