//! dk-terminal: terminal emulator model (`alacritty_terminal`) + GPUI `TerminalView`
//! element (TRM-002…005, 010…012, KBD-060…063).
//!
//! The UI owns the hub `TerminalHandle`; it feeds PTY bytes into the view with
//! [`TerminalView::feed`] and forwards [`TerminalEvent::Input`] / [`TerminalEvent::Resize`] to
//! the hub. The view never does I/O itself (NFR-001).
//!
//! PUBLIC API FIXED — implemented by `gpui-ui` (terminal worktree).

pub mod keys;

use bytes::Bytes;
use gpui_kit::{Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window, div};

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

/// The terminal view entity. Owns the `alacritty_terminal::Term` on the foreground thread.
pub struct TerminalView {
    focus: FocusHandle,
}

impl TerminalView {
    pub fn new(config: TerminalConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = (config, window);
        Self {
            focus: cx.focus_handle(),
        }
    }

    /// Feed PTY output (parsed in place; repaint coalesced at ~4 ms, TRM-010).
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let _ = (bytes, cx);
    }

    /// Show "[process exited with code N] — press Enter to reconnect" (TRM-006) and stop
    /// emitting `Input` until reset.
    pub fn set_exited(&mut self, code: Option<i64>, cx: &mut Context<Self>) {
        let _ = (code, cx);
    }

    /// Clear the screen and exited state for a new session.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
    }

    /// Read-only mode: the container was removed or the engine disconnected (CDT-080, SHL-013).
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        let _ = (read_only, cx);
    }

    pub fn set_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        let _ = (config, cx);
    }

    /// Current grid size `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        (80, 24)
    }

    /// Selected text, if any.
    pub fn selection_text(&self) -> Option<String> {
        None
    }
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
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
