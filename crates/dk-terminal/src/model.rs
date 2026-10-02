//! Terminal state: an `alacritty_terminal::Term` plus its VT parser, owned by the view on the
//! foreground thread (no locks; spec: container-terminal implementation notes, TRM-002).
//!
//! The model is a plain struct: [`TerminalModel::advance`] parses PTY output in place and
//! [`TerminalModel::take_events`] drains what the terminal asked the UI to do (replies to write
//! back to the PTY, title changes, bell).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};

use crate::keys::KeyModes;

/// Smallest grid the model accepts.
pub const MIN_COLS: u16 = 2;
pub const MIN_ROWS: u16 = 1;

/// What the terminal asked the embedding UI to do. Drained by [`TerminalModel::take_events`].
#[derive(Clone)]
pub enum ModelEvent {
    /// Bytes to write back to the PTY (DSR/DA replies, …).
    PtyWrite(Vec<u8>),
    /// OSC 0/2 title; `None` resets it.
    Title(Option<String>),
    Bell,
    /// OSC 4/10/11/12 colour query: answer with `format(rgb)` written to the PTY.
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Send + Sync>),
    /// XTWINOPS text-area size query.
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
}

impl std::fmt::Debug for ModelEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PtyWrite(bytes) => write!(f, "PtyWrite({:?})", String::from_utf8_lossy(bytes)),
            Self::Title(title) => write!(f, "Title({title:?})"),
            Self::Bell => write!(f, "Bell"),
            Self::ColorRequest(index, _) => write!(f, "ColorRequest({index})"),
            Self::TextAreaSizeRequest(_) => write!(f, "TextAreaSizeRequest"),
        }
    }
}

/// Collects `Term` events on the foreground thread.
#[derive(Clone, Default)]
pub struct Listener {
    events: Rc<RefCell<Vec<ModelEvent>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let event = match event {
            Event::PtyWrite(text) => ModelEvent::PtyWrite(text.into_bytes()),
            Event::Title(title) => ModelEvent::Title(Some(title)),
            Event::ResetTitle => ModelEvent::Title(None),
            Event::Bell => ModelEvent::Bell,
            Event::ColorRequest(index, format) => ModelEvent::ColorRequest(index, format),
            Event::TextAreaSizeRequest(format) => ModelEvent::TextAreaSizeRequest(format),
            // OSC 52 is disabled; cursor/wakeup/exit events don't apply to an embedded view.
            _ => return,
        };
        self.events.borrow_mut().push(event);
    }
}

/// Grid size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Mouse reporting requested by the application (TRM-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseModes {
    /// Any of the reporting modes (1000/1002/1003) is on.
    pub report: bool,
    /// 1002: report motion while a button is held.
    pub drag: bool,
    /// 1003: report all motion.
    pub motion: bool,
    /// 1006: SGR encoding.
    pub sgr: bool,
}

/// `alacritty_terminal::Term` + VT parser.
pub struct TerminalModel {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    listener: Listener,
    scrollback: usize,
    /// Cell size in pixels, for text-area size replies.
    cell_px: (u16, u16),
}

impl TerminalModel {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let listener = Listener::default();
        let size = grid_size(cols, rows);
        let term = Term::new(term_config(scrollback), &size, listener.clone());
        Self {
            term,
            parser: Processor::new(),
            listener,
            scrollback,
            cell_px: (8, 16),
        }
    }

    /// Parse PTY output.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// When a synchronized update (`CSI ? 2026 h`) is buffering output, the time at which it must
    /// be flushed with [`Self::flush_sync`] even if the application never ends it.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// End a pending synchronized update and apply its buffered output.
    pub fn flush_sync(&mut self) {
        if self.sync_deadline().is_some() {
            self.parser.stop_sync(&mut self.term);
        }
    }

    /// Resize the grid (clamped to `MIN_COLS × MIN_ROWS`). Returns whether the size changed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let size = grid_size(cols, rows);
        if size.cols == self.term.columns() && size.rows == self.term.screen_lines() {
            return false;
        }
        self.term.resize(size);
        true
    }

    /// `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        (
            u16::try_from(self.term.columns()).unwrap_or(u16::MAX),
            u16::try_from(self.term.screen_lines()).unwrap_or(u16::MAX),
        )
    }

    pub fn set_cell_px(&mut self, width: u16, height: u16) {
        self.cell_px = (width.max(1), height.max(1));
    }

    pub fn window_size(&self) -> WindowSize {
        let (cols, rows) = self.size();
        WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: self.cell_px.0,
            cell_height: self.cell_px.1,
        }
    }

    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// Change the scrollback limit (TRM-002, SET-040).
    pub fn set_scrollback(&mut self, lines: usize) {
        if lines != self.scrollback {
            self.scrollback = lines;
            self.term.set_options(term_config(lines));
            // `set_options` reports the title again; that's noise here.
            self.listener
                .events
                .borrow_mut()
                .retain(|e| !matches!(e, ModelEvent::Title(_)));
        }
    }

    /// Number of lines currently in the scrollback history.
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Drain pending terminal events.
    pub fn take_events(&mut self) -> Vec<ModelEvent> {
        std::mem::take(&mut *self.listener.events.borrow_mut())
    }

    // ── modes ──────────────────────────────────────────────────────────────────────────────

    fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn app_cursor(&self) -> bool {
        self.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn app_keypad(&self) -> bool {
        self.mode().contains(TermMode::APP_KEYPAD)
    }

    pub fn key_modes(&self) -> KeyModes {
        KeyModes {
            app_cursor: self.app_cursor(),
            app_keypad: self.app_keypad(),
        }
    }

    pub fn bracketed_paste(&self) -> bool {
        self.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn alt_screen(&self) -> bool {
        self.mode().contains(TermMode::ALT_SCREEN)
    }

    /// `CSI ? 1007 h`: wheel sends arrow keys in the alternate screen.
    pub fn alternate_scroll(&self) -> bool {
        self.mode().contains(TermMode::ALTERNATE_SCROLL)
    }

    pub fn focus_reporting(&self) -> bool {
        self.mode().contains(TermMode::FOCUS_IN_OUT)
    }

    pub fn mouse_modes(&self) -> MouseModes {
        let mode = self.mode();
        MouseModes {
            report: mode.intersects(TermMode::MOUSE_MODE),
            drag: mode.contains(TermMode::MOUSE_DRAG),
            motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr: mode.contains(TermMode::SGR_MOUSE),
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.term.is_focused = focused;
    }

    // ── scrolling ──────────────────────────────────────────────────────────────────────────

    /// Lines scrolled back into history (0 = live screen).
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll the viewport; positive `lines` scroll into the history.
    pub fn scroll_lines(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_page_up(&mut self) {
        self.term.scroll_display(Scroll::PageUp);
    }

    pub fn scroll_page_down(&mut self) {
        self.term.scroll_display(Scroll::PageDown);
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
        }
    }

    // ── selection ──────────────────────────────────────────────────────────────────────────

    /// Convert a viewport cell (row from the top of the view) into a grid point.
    pub fn viewport_point(&self, row: usize, col: usize) -> Point {
        let rows = self.term.screen_lines().saturating_sub(1);
        let cols = self.term.columns().saturating_sub(1);
        let row = row.min(rows);
        let col = col.min(cols);
        Point::new(Line(row as i32 - self.display_offset() as i32), Column(col))
    }

    /// Start a selection at a viewport cell. `clicks`: 1 simple, 2 word, 3+ line.
    pub fn start_selection(&mut self, row: usize, col: usize, side: Side, clicks: usize) {
        let ty = match clicks {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let point = self.viewport_point(row, col);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, row: usize, col: usize, side: Side) {
        let point = self.viewport_point(row, col);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    /// Drop an empty selection (a plain click).
    pub fn finish_selection(&mut self) {
        if self
            .term
            .selection
            .as_ref()
            .is_some_and(Selection::is_empty)
        {
            self.term.selection = None;
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.is_some()
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    // ── read access for the painter ───────────────────────────────────────────────────────

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    /// The visible screen as text, one line per row, trailing blanks trimmed (tests).
    #[doc(hidden)]
    pub fn grid_text(&self) -> String {
        let grid = self.term.grid();
        let offset = self.display_offset() as i32;
        let mut out = Vec::with_capacity(self.term.screen_lines());
        for row in 0..self.term.screen_lines() {
            let line = &grid[Line(row as i32 - offset)];
            let mut text = String::new();
            for col in 0..self.term.columns() {
                let cell = &line[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                text.push(cell.c);
                if let Some(zw) = cell.zerowidth() {
                    text.extend(zw);
                }
            }
            out.push(text.trim_end().to_string());
        }
        out.join("\n")
    }
}

fn grid_size(cols: u16, rows: u16) -> GridSize {
    GridSize {
        cols: usize::from(cols.max(MIN_COLS)),
        rows: usize::from(rows.max(MIN_ROWS)),
    }
}

fn term_config(scrollback: usize) -> Config {
    Config {
        scrolling_history: scrollback,
        // A container must not be able to write to the user's clipboard.
        osc52: Osc52::Disabled,
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{Color, NamedColor};

    fn cell(model: &TerminalModel, row: i32, col: usize) -> alacritty_terminal::term::cell::Cell {
        model.term().grid()[Line(row)][Column(col)].clone()
    }

    #[test]
    fn trm_002_sgr_red_cell() {
        let mut m = TerminalModel::new(80, 24, 100);
        m.advance(b"\x1b[31mX\x1b[0mY\x1b[1;4;3mZ");
        let x = cell(&m, 0, 0);
        assert_eq!(x.c, 'X');
        assert_eq!(x.fg, Color::Named(NamedColor::Red));
        let y = cell(&m, 0, 1);
        assert_eq!(y.fg, Color::Named(NamedColor::Foreground));
        let z = cell(&m, 0, 2);
        assert!(
            z.flags
                .contains(Flags::BOLD | Flags::UNDERLINE | Flags::ITALIC)
        );
    }

    #[test]
    fn trm_002_truecolor() {
        let mut m = TerminalModel::new(80, 24, 100);
        m.advance(b"\x1b[38;2;10;20;30;48;5;196mX");
        let x = cell(&m, 0, 0);
        assert_eq!(
            x.fg,
            Color::Spec(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        assert_eq!(x.bg, Color::Indexed(196));
    }

    #[test]
    fn trm_002_alt_screen() {
        let mut m = TerminalModel::new(80, 24, 100);
        m.advance(b"primary");
        assert!(!m.alt_screen());
        m.advance(b"\x1b[?1049h\x1b[2J\x1b[Halternate");
        assert!(m.alt_screen());
        assert!(m.grid_text().starts_with("alternate"));
        m.advance(b"\x1b[?1049l");
        assert!(!m.alt_screen());
        assert!(m.grid_text().starts_with("primary"));
    }

    #[test]
    fn trm_002_scrollback_capped() {
        let mut m = TerminalModel::new(80, 24, 100);
        for i in 0..500 {
            m.advance(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(m.history_size(), 100);
        // Default is 10,000 lines.
        let mut big = TerminalModel::new(80, 24, crate::TerminalConfig::default().scrollback_lines);
        for i in 0..10_500 {
            big.advance(format!("{i}\r\n").as_bytes());
        }
        assert_eq!(big.history_size(), 10_000);
        // Shrinking the limit drops history.
        big.set_scrollback(50);
        assert_eq!(big.history_size(), 50);
    }

    #[test]
    fn trm_005_resize() {
        let mut m = TerminalModel::new(80, 24, 100);
        assert_eq!(m.size(), (80, 24));
        assert!(m.resize(100, 30));
        assert_eq!(m.size(), (100, 30));
        assert!(!m.resize(100, 30));
        // Clamped to the minimum.
        m.resize(0, 0);
        assert_eq!(m.size(), (MIN_COLS, MIN_ROWS));
        // Text-area size replies use the new size.
        m.resize(120, 40);
        m.set_cell_px(8, 16);
        let ws = m.window_size();
        assert_eq!((ws.num_cols, ws.num_lines), (120, 40));
    }

    #[test]
    fn bracketed_paste_mode_tracked() {
        let mut m = TerminalModel::new(80, 24, 100);
        assert!(!m.bracketed_paste());
        m.advance(b"\x1b[?2004h");
        assert!(m.bracketed_paste());
        m.advance(b"\x1b[?2004l");
        assert!(!m.bracketed_paste());
        // DECCKM and mouse modes too.
        m.advance(b"\x1b[?1h\x1b[?1002h\x1b[?1006h");
        assert!(m.app_cursor());
        assert!(m.key_modes().app_cursor);
        let mouse = m.mouse_modes();
        assert!(mouse.report && mouse.drag && mouse.sgr && !mouse.motion);
    }

    #[test]
    fn dsr_reply_emitted_as_pty_write() {
        let mut m = TerminalModel::new(80, 24, 100);
        m.advance(b"ab\x1b[6n");
        let events = m.take_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ModelEvent::PtyWrite(b) if b == b"\x1b[1;3R")),
            "{events:?}"
        );
        assert!(m.take_events().is_empty());
        m.advance(b"\x1b]0;my title\x07\x07");
        let events = m.take_events();
        assert!(matches!(&events[0], ModelEvent::Title(Some(t)) if t == "my title"));
        assert!(matches!(events[1], ModelEvent::Bell));
    }

    #[test]
    fn selection_round_trip() {
        let mut m = TerminalModel::new(20, 5, 100);
        m.advance(b"hello world\r\nsecond");
        m.start_selection(0, 0, Side::Left, 1);
        m.update_selection(0, 4, Side::Right);
        assert_eq!(m.selection_text().as_deref(), Some("hello"));
        m.start_selection(0, 7, Side::Left, 2);
        assert_eq!(m.selection_text().as_deref(), Some("world"));
        m.start_selection(1, 2, Side::Left, 3);
        assert_eq!(m.selection_text().as_deref(), Some("second\n"));
        m.start_selection(0, 3, Side::Left, 1);
        m.finish_selection();
        assert!(!m.has_selection());
    }

    #[test]
    fn synchronized_update_flushes() {
        let mut m = TerminalModel::new(20, 5, 100);
        m.advance(b"\x1b[?2026hbuffered");
        assert!(m.sync_deadline().is_some());
        assert!(!m.grid_text().contains("buffered"));
        m.flush_sync();
        assert!(m.sync_deadline().is_none());
        assert!(m.grid_text().contains("buffered"));
    }
}
