//! dk-terminal: terminal emulator model (`alacritty_terminal`) + GPUI `TerminalView`
//! element (TRM-002…005, 010…012, KBD-060…063).
//!
//! The UI owns the hub `TerminalHandle`; it feeds PTY bytes into the view with
//! [`TerminalView::feed`] and forwards [`TerminalEvent::Input`] / [`TerminalEvent::Resize`] to
//! the hub. The view never does I/O itself (NFR-001).
//!
//! Modules:
//! - [`keys`]: keystroke → PTY byte encoding and the reserved app chords (KBD-060…062).
//! - [`model`]: the `alacritty_terminal::Term` + VT parser, owned on the foreground thread.
//! - `view`: the `TerminalView` entity and its custom grid element.
//!
//! Zed's `terminal`/`terminal_view` crates (GPL) were not used or copied (REL-003).

pub mod keys;
pub mod model;
mod view;

use bytes::Bytes;

pub use view::palette::Palette;
pub use view::{OUTPUT_COALESCE, RESIZE_DEBOUNCE, TerminalView, encode_paste};

/// Appearance & behaviour (SET-040, TRM-011).
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalConfig {
    /// Empty = platform monospace.
    pub font_family: String,
    /// Pixels; default 13.
    pub font_size: f32,
    /// Default 10,000 (TRM-002).
    pub scrollback_lines: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 13.0,
            scrollback_lines: 10_000,
        }
    }
}

/// Events emitted by `TerminalView` (subscribe with `cx.subscribe`).
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent {
    /// Encoded keystrokes / paste / mouse reports to write to the PTY.
    Input(Bytes),
    /// The grid size changed (already debounced 50 ms, TRM-005).
    Resize {
        cols: u16,
        rows: u16,
    },
    /// OSC title.
    Title(String),
    Bell,
    /// The user pressed Enter after the process exited (TRM-006 reconnect).
    ReconnectRequested,
}

/// Actions handled by the terminal or passed to the app (KBD-061/062).
pub mod actions {
    gpui_kit::actions!(
        terminal,
        [
            /// `Mod+Shift+F6` (macOS also `Cmd+Esc`): move focus to the detail tab bar.
            LeaveTerminal,
            /// `Ctrl+Shift+C` / `Cmd+C`
            Copy,
            /// `Ctrl+Shift+V` / `Cmd+V`
            Paste,
            /// `Shift+PgUp`
            ScrollPageUp,
            /// `Shift+PgDn`
            ScrollPageDown,
        ]
    );
}

/// Key context of the terminal view (bindings in the app's `keymap.rs`).
pub const KEY_CONTEXT: &str = "Terminal";

/// Default key bindings for the terminal's own actions (KBD-062/063, TRM-003). The app's
/// `keymap.rs` is the single source of truth; it may use these or define its own.
pub fn default_key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use actions::{Copy, Paste, ScrollPageDown, ScrollPageUp};
    use gpui_kit::KeyBinding;
    let ctx = Some(KEY_CONTEXT);
    let (copy, paste) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-v")
    } else {
        ("ctrl-shift-c", "ctrl-shift-v")
    };
    vec![
        KeyBinding::new(copy, Copy, ctx),
        KeyBinding::new(paste, Paste, ctx),
        KeyBinding::new("shift-pageup", ScrollPageUp, ctx),
        KeyBinding::new("shift-pagedown", ScrollPageDown, ctx),
    ]
}
