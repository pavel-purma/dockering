//! `TerminalView`: the terminal entity embedded by the app (TRM-002…006, TRM-010…012,
//! KBD-060…063, KBD-083).
//!
//! Threading: everything here runs on the foreground executor and does no I/O (NFR-001). PTY
//! bytes come in through [`TerminalView::feed`]; keystrokes, pastes and mouse reports go out as
//! [`TerminalEvent::Input`]. Repaints are coalesced at 4 ms and resize events debounced at 50 ms
//! with tasks stored on the entity (NFR-004).
//!
//! Keyboard routing: GPUI resolves key bindings *before* key-down listeners run, so a plain
//! `on_key_down` handler would lose `Tab`, `Ctrl+R`, `F6`, … to app-wide bindings. The view
//! therefore registers a keystroke interceptor that, while the terminal is focused, encodes
//! every key with [`keys::encode`] and stops propagation for bytes bound for the PTY. Reserved
//! chords ([`KeyAction::PassToApp`]) and plain text ([`KeyAction::Ignore`]) continue through
//! the normal dispatch: the app keymap, then GPUI's input handler (IME, dead keys, AltGr).

mod element;
mod mouse;
pub mod palette;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bytes::Bytes;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Theme};
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeystrokeEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollWheelEvent, SharedString, Styled,
    Subscription, Task, UTF16Selection, Window, div, prelude::FluentBuilder, px,
};

use crate::actions::{Copy, Paste, ScrollPageDown, ScrollPageUp};
use crate::keys::{self, KeyAction};
use crate::model::{ModelEvent, TerminalModel};
use crate::{KEY_CONTEXT, TerminalConfig, TerminalEvent};

use element::{CellMetrics, GridGeometry, RowCache, TerminalElement};
use mouse::{MouseReport, ReportModifiers, button};
use palette::Palette;

/// Output repaint coalescing window (TRM-010, spec 10 §3.3).
pub const OUTPUT_COALESCE: Duration = Duration::from_millis(4);
/// PTY resize debounce (TRM-005).
pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(50);

/// Default monospace families, in order of preference (TRM-011).
const DEFAULT_FONTS: &[&str] = if cfg!(target_os = "windows") {
    &["Cascadia Mono", "Consolas"]
} else if cfg!(target_os = "macos") {
    &["Menlo", "Monaco"]
} else {
    &["DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono"]
};

/// The terminal view entity. Owns the `alacritty_terminal::Term` on the foreground thread.
pub struct TerminalView {
    focus: FocusHandle,
    pub(crate) config: TerminalConfig,
    pub(crate) model: TerminalModel,
    /// `Some(code)` after the process exited (TRM-006).
    exited: Option<Option<i64>>,
    reconnect_requested: bool,
    read_only: bool,

    // Layout (written in prepaint).
    pub(crate) geometry: GridGeometry,
    pub(crate) row_cache: RowCache,
    pub(crate) cell_metrics: Option<CellMetrics>,
    /// IME composition text.
    pub(crate) marked_text: Option<String>,

    // Tasks (NFR-004): replacing one cancels the previous.
    notify_task: Option<Task<()>>,
    resize_task: Option<Task<()>>,
    sync_task: Option<Task<()>>,
    reported_size: Option<(u16, u16)>,
    pending_size: Option<(u16, u16)>,

    // Pointer state.
    selecting: bool,
    report_button: Option<u8>,
    last_report_cell: Option<(usize, usize)>,
    scroll_remainder: f32,
    pub(crate) last_prepaint: Duration,

    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(config: TerminalConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let model = TerminalModel::new(80, 24, config.scrollback_lines);

        let this = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if let Some(view) = this.upgrade() {
                view.update(cx, |view, cx| view.intercept_keystroke(event, window, cx));
            }
        });
        let focus_in = cx.on_focus(&focus, window, |this, _, cx| this.focus_changed(true, cx));
        let focus_out = cx.on_blur(&focus, window, |this, _, cx| this.focus_changed(false, cx));

        Self {
            focus,
            config,
            model,
            exited: None,
            reconnect_requested: false,
            read_only: false,
            geometry: GridGeometry::default(),
            row_cache: RowCache::default(),
            cell_metrics: None,
            marked_text: None,
            notify_task: None,
            resize_task: None,
            sync_task: None,
            reported_size: None,
            pending_size: None,
            selecting: false,
            report_button: None,
            last_report_cell: None,
            scroll_remainder: 0.0,
            last_prepaint: Duration::ZERO,
            _subscriptions: vec![intercept, focus_in, focus_out],
        }
    }

    /// Feed PTY output (parsed in place; repaint coalesced at ~4 ms, TRM-010).
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.model.advance(bytes);
        self.drain_model_events(cx);
        self.schedule_sync_flush(cx);
        self.schedule_notify(cx);
    }

    /// Show "[process exited with code N] — press Enter to reconnect" (TRM-006) and stop
    /// emitting `Input` until reset.
    pub fn set_exited(&mut self, code: Option<i64>, cx: &mut Context<Self>) {
        self.model.flush_sync();
        // Leave the alternate screen so the message lands in the scrollback-backed screen.
        let leave_alt = if self.model.alt_screen() {
            "\x1b[?1049l"
        } else {
            ""
        };
        let status = match code {
            Some(code) => format!("[process exited with code {code}]"),
            None => "[process exited]".to_string(),
        };
        let text = format!("{leave_alt}\x1b[0m\r\n{status} \u{2014} press Enter to reconnect\r\n");
        self.model.advance(text.as_bytes());
        // Replies to a dead process are dropped.
        let _ = self.model.take_events();
        self.model.scroll_to_bottom();
        self.exited = Some(code);
        self.reconnect_requested = false;
        self.marked_text = None;
        self.report_button = None;
        self.notify_task = None;
        self.sync_task = None;
        cx.notify();
    }

    /// Clear the screen and exited state for a new session.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let (cols, rows) = self.model.size();
        self.model = TerminalModel::new(cols, rows, self.config.scrollback_lines);
        if let Some(metrics) = &self.cell_metrics {
            self.model.set_cell_px(
                metrics.cell_width.as_f32().round() as u16,
                metrics.line_height.as_f32().round() as u16,
            );
        }
        self.exited = None;
        self.reconnect_requested = false;
        self.marked_text = None;
        self.selecting = false;
        self.report_button = None;
        self.scroll_remainder = 0.0;
        self.sync_task = None;
        self.notify_task = None;
        cx.notify();
    }

    /// Read-only mode: the container was removed or the engine disconnected (CDT-080, SHL-013).
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            self.marked_text = None;
            self.report_button = None;
            cx.notify();
        }
    }

    pub fn set_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        if config.scrollback_lines != self.config.scrollback_lines {
            self.model.set_scrollback(config.scrollback_lines);
        }
        self.config = config;
        cx.notify();
    }

    /// Current grid size `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        self.model.size()
    }

    /// Selected text, if any.
    pub fn selection_text(&self) -> Option<String> {
        self.model.selection_text()
    }

    /// Whether the process exited (TRM-006).
    pub fn is_exited(&self) -> bool {
        self.exited.is_some()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The visible screen as text (tests).
    #[doc(hidden)]
    pub fn grid_text(&self) -> String {
        self.model.grid_text()
    }

    /// The underlying model (tests and diagnostics).
    #[doc(hidden)]
    pub fn model(&self) -> &TerminalModel {
        &self.model
    }

    // ── internals ──────────────────────────────────────────────────────────────────────────

    /// No PTY input is produced while exited or read-only.
    pub(crate) fn inert(&self) -> bool {
        self.exited.is_some() || self.read_only
    }

    fn emit_input(&mut self, bytes: impl Into<Bytes>, cx: &mut Context<Self>) {
        if !self.inert() {
            cx.emit(TerminalEvent::Input(bytes.into()));
        }
    }

    /// Input typed by the user: jump back to the live screen first.
    fn user_input(&mut self, bytes: impl Into<Bytes>, cx: &mut Context<Self>) {
        if self.inert() {
            return;
        }
        if self.model.display_offset() != 0 {
            self.model.scroll_to_bottom();
            cx.notify();
        }
        self.emit_input(bytes, cx);
    }

    fn drain_model_events(&mut self, cx: &mut Context<Self>) {
        let events = self.model.take_events();
        if events.is_empty() {
            return;
        }
        let palette = palette_for(cx);
        for event in events {
            match event {
                ModelEvent::PtyWrite(bytes) => self.emit_input(bytes, cx),
                ModelEvent::Title(title) => {
                    cx.emit(TerminalEvent::Title(title.unwrap_or_default()))
                }
                ModelEvent::Bell => cx.emit(TerminalEvent::Bell),
                ModelEvent::ColorRequest(index, format) => {
                    let rgb = palette.query(index, self.model.term().colors());
                    self.emit_input(format(rgb).into_bytes(), cx);
                }
                ModelEvent::TextAreaSizeRequest(format) => {
                    let reply = format(self.model.window_size());
                    self.emit_input(reply.into_bytes(), cx);
                }
            }
        }
    }

    /// One `cx.notify()` per 4 ms window, however many chunks arrive (TRM-010).
    fn schedule_notify(&mut self, cx: &mut Context<Self>) {
        if self.notify_task.is_some() {
            return;
        }
        self.notify_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUTPUT_COALESCE).await;
            this.update(cx, |this, cx| {
                this.notify_task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Flush a synchronized update (`CSI ? 2026 h`) the application never ended.
    fn schedule_sync_flush(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.model.sync_deadline() else {
            self.sync_task = None;
            return;
        };
        if self.sync_task.is_some() {
            return;
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.sync_task = None;
                this.model.flush_sync();
                this.drain_model_events(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Called from prepaint with the size that fits the bounds: resize the model now and report
    /// the new size after it has been stable for 50 ms (TRM-005).
    pub(crate) fn grid_resized(
        &mut self,
        cols: u16,
        rows: u16,
        metrics: &CellMetrics,
        cx: &mut Context<Self>,
    ) {
        self.model.set_cell_px(
            metrics.cell_width.as_f32().round() as u16,
            metrics.line_height.as_f32().round() as u16,
        );
        if self.model.resize(cols, rows) {
            self.selecting = false;
        }
        let size = self.model.size();
        if self.pending_size == Some(size) {
            return;
        }
        if self.reported_size == Some(size) {
            // Back to the size the PTY already has: drop the pending report.
            self.pending_size = None;
            self.resize_task = None;
            return;
        }
        self.pending_size = Some(size);
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESIZE_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.resize_task = None;
                if let Some((cols, rows)) = this.pending_size.take() {
                    this.reported_size = Some((cols, rows));
                    cx.emit(TerminalEvent::Resize { cols, rows });
                }
            })
            .ok();
        }));
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.model.set_focused(focused);
        if self.model.focus_reporting() {
            let report: &'static [u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.emit_input(report, cx);
        }
        cx.notify();
    }

    // ── keyboard (KBD-060…063) ─────────────────────────────────────────────────────────────

    fn intercept_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        if self.exited.is_some() {
            // TRM-006: Enter reconnects; everything else keeps its normal meaning.
            if keystroke.key == "enter" && !keystroke.modifiers.modified() {
                if !self.reconnect_requested {
                    self.reconnect_requested = true;
                    cx.emit(TerminalEvent::ReconnectRequested);
                }
                cx.stop_propagation();
            }
            return;
        }
        // While an IME composition is open, keys belong to the IME (KBD-083).
        if self.read_only || self.marked_text.is_some() {
            return;
        }
        if let KeyAction::Pty(bytes) = keys::encode(keystroke, self.model.key_modes()) {
            self.marked_text = None;
            self.user_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.model.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    /// Write pasted text, bracketed when the application asked for it (TRM-002).
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.inert() || text.is_empty() {
            return;
        }
        let bytes = encode_paste(text, self.model.bracketed_paste());
        self.user_input(bytes, cx);
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.model.scroll_page_up();
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.model.scroll_page_down();
        cx.notify();
    }

    // ── pointer (TRM-002) ──────────────────────────────────────────────────────────────────

    /// The application wants mouse reports (Shift bypasses reporting for local selection).
    pub(crate) fn mouse_reporting(&self, shift: bool) -> bool {
        !self.inert() && !shift && self.model.mouse_modes().report
    }

    fn report(
        &mut self,
        kind: MouseReport,
        button: u8,
        cell: (usize, usize),
        mods: ReportModifiers,
        cx: &mut Context<Self>,
    ) {
        let sgr = self.model.mouse_modes().sgr;
        if let Some(bytes) = mouse::encode(kind, button, cell.1, cell.0, mods, sgr) {
            self.emit_input(bytes, cx);
        }
    }

    /// Capture phase: send a press report when the application enabled mouse reporting.
    /// Returns whether the event was consumed.
    pub(crate) fn mouse_down_report(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.mouse_reporting(event.modifiers.shift) {
            return false;
        }
        let code = match event.button {
            MouseButton::Left => button::LEFT,
            MouseButton::Middle => button::MIDDLE,
            MouseButton::Right => button::RIGHT,
            MouseButton::Navigate(_) => return false,
        };
        window.focus(&self.focus, cx);
        let (row, col, _) = self.cell_at(event.position);
        self.report_button = Some(code);
        self.last_report_cell = Some((row, col));
        self.report(
            MouseReport::Press,
            code,
            (row, col),
            report_mods(&event.modifiers),
            cx,
        );
        true
    }

    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        window.focus(&self.focus, cx);
        let (row, col, side) = self.cell_at(event.position);
        self.model
            .start_selection(row, col, side, event.click_count.max(1));
        self.selecting = true;
        cx.notify();
    }

    pub(crate) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if self.selecting && event.dragging() {
            // Auto-scroll when dragging past the top/bottom edge.
            let g = self.geometry;
            let bottom = g.origin.y + g.line_height * g.rows as f32;
            if event.position.y < g.origin.y {
                self.model.scroll_lines(1);
            } else if event.position.y > bottom {
                self.model.scroll_lines(-1);
            }
            let (row, col, side) = self.cell_at(event.position);
            self.model.update_selection(row, col, side);
            cx.notify();
            return;
        }
        if !self.mouse_reporting(event.modifiers.shift) {
            return;
        }
        let modes = self.model.mouse_modes();
        let held = self.report_button;
        let wants = match held {
            Some(_) => modes.drag || modes.motion,
            None => modes.motion && hovered,
        };
        if !wants {
            return;
        }
        let (row, col, _) = self.cell_at(event.position);
        if self.last_report_cell == Some((row, col)) {
            return;
        }
        self.last_report_cell = Some((row, col));
        self.report(
            MouseReport::Motion,
            held.unwrap_or(button::NONE),
            (row, col),
            report_mods(&event.modifiers),
            cx,
        );
    }

    pub(crate) fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting && event.button == MouseButton::Left {
            self.selecting = false;
            self.model.finish_selection();
            cx.notify();
        }
        if let Some(code) = self.report_button.take() {
            let (row, col, _) = self.cell_at(event.position);
            self.report(
                MouseReport::Release,
                code,
                (row, col),
                report_mods(&event.modifiers),
                cx,
            );
        }
    }

    pub(crate) fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line_height = self.geometry.line_height;
        let delta = event.delta.pixel_delta(line_height).y / line_height;
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc();
        self.scroll_remainder -= lines;
        let lines = lines as i32;
        if lines == 0 {
            return;
        }
        let count = lines.unsigned_abs().min(64) as usize;
        if self.mouse_reporting(event.modifiers.shift) {
            let code = if lines > 0 {
                button::WHEEL_UP
            } else {
                button::WHEEL_DOWN
            };
            let (row, col, _) = self.cell_at(event.position);
            for _ in 0..count {
                self.report(
                    MouseReport::Press,
                    code,
                    (row, col),
                    report_mods(&event.modifiers),
                    cx,
                );
            }
        } else if self.model.alt_screen() && self.model.alternate_scroll() && !self.inert() {
            // Full-screen apps without mouse mode (less, man): wheel sends arrow keys.
            let arrow: &[u8] = match (lines > 0, self.model.app_cursor()) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.emit_input(arrow.repeat(count), cx);
        } else {
            self.model.scroll_lines(lines);
            cx.notify();
        }
    }
}

fn report_mods(m: &gpui_kit::Modifiers) -> ReportModifiers {
    ReportModifiers {
        shift: m.shift,
        alt: m.alt,
        ctrl: m.control,
    }
}

/// Paste encoding: newlines become CR; with bracketed paste the text is wrapped in
/// `ESC [200~ … ESC [201~` and embedded end markers are removed so pasted text can't end the
/// paste early.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let inner = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

fn palette_for(cx: &App) -> Palette {
    if cx.try_global::<Theme>().is_some() {
        Palette::from_theme(cx)
    } else {
        let bg: gpui_kit::Hsla = gpui_kit::rgb(0x1e1e1e).into();
        let fg: gpui_kit::Hsla = gpui_kit::rgb(0xd4d4d4).into();
        Palette::new(bg, fg, fg, gpui_kit::rgba(0x264f78ff).into(), true)
    }
}

/// The configured family, or the platform default monospace font (TRM-011).
pub(crate) fn font_family(config: &TerminalConfig, cx: &App) -> SharedString {
    let configured = config.font_family.trim();
    if !configured.is_empty() {
        return SharedString::from(configured.to_string());
    }
    static DEFAULT: OnceLock<String> = OnceLock::new();
    let family = DEFAULT.get_or_init(|| {
        let installed = cx.text_system().all_font_names();
        DEFAULT_FONTS
            .iter()
            .find(|candidate| installed.iter().any(|name| name == *candidate))
            .map(|family| (*family).to_string())
            .or_else(|| {
                cx.try_global::<Theme>()
                    .map(|theme| theme.mono_font_family.to_string())
            })
            .unwrap_or_else(|| DEFAULT_FONTS[0].to_string())
    });
    SharedString::from(family.clone())
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette_for(cx);
        let focused = self.focus.is_focused(window);
        let ring = if cx.try_global::<Theme>().is_some() {
            cx.theme().ring
        } else {
            palette.cursor
        };
        let badge: Option<SharedString> = if self.read_only {
            Some("Read-only".into())
        } else if self.exited.is_some() {
            Some("Exited".into())
        } else {
            None
        };
        let menu_focus = self.focus.clone();
        let has_selection = self.model.has_selection();
        let inert = self.inert();

        div()
            .id(("terminal", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette.background)
            .border_1()
            .border_color(if focused {
                ring
            } else {
                gpui_kit::transparent_black()
            })
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .child(TerminalElement::new(
                cx.entity(),
                self.focus.clone(),
                focused,
                palette,
            ))
            .when_some(badge, |this, badge| {
                this.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(10.))
                        .child(Tag::secondary().child(badge)),
                )
            })
            .context_menu(move |menu, _, _| {
                menu.action_context(menu_focus.clone())
                    .menu_with_disabled("Copy", Box::new(Copy), !has_selection)
                    .menu_with_disabled("Paste", Box::new(Paste), inert)
            })
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked_text
            .as_ref()
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
        if text.is_empty() || self.inert() {
            return;
        }
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.user_input(text.into_bytes(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty() && !self.inert()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds())
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        !self.inert()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_encoding() {
        assert_eq!(encode_paste("a\nb\r\nc", false), b"a\rb\rc".to_vec());
        assert_eq!(
            encode_paste("ls\n", true),
            b"\x1b[200~ls\r\x1b[201~".to_vec()
        );
        assert_eq!(
            encode_paste("evil\x1b[201~rm -rf /\n", true),
            b"\x1b[200~evilrm -rf /\r\x1b[201~".to_vec()
        );
    }
}
