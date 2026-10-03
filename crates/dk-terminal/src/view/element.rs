//! `TerminalElement`: the custom GPUI element that lays out and paints the cell grid
//! (TRM-002, TRM-005, TRM-010).
//!
//! Prepaint computes the grid size from the bounds, resizes the model, and builds one
//! `ShapedLine` per run of cells (rows only break into several lines after wide characters).
//! Shaped rows are cached on the view by a hash of their content, so unchanged rows (and rows
//! that merely scrolled) are never reshaped. Paint draws backgrounds, selection, text, cursor and
//! IME composition, registers the input handler and the mouse listeners.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use alacritty_terminal::index::{Column, Line, Side};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui_kit::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    FocusHandle, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, Length, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Size,
    StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point,
    px, size,
};

use super::TerminalView;
use super::palette::{Palette, hash_color};

/// Inner padding between the element's bounds and the first cell.
pub(crate) const PADDING: Pixels = px(4.);

/// Font and cell measurements.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CellMetrics {
    pub font: Font,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    /// Requested family + size, to know when to re-measure.
    pub key: (String, u32),
}

impl CellMetrics {
    pub fn measure(family: SharedString, font_size: f32, window: &Window) -> Self {
        let font_size = px(font_size.max(4.0));
        let font = Font {
            family: family.clone(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system
            .advance(font_id, font_size, 'M')
            .map(|advance| advance.width)
            .unwrap_or(font_size * 0.6);
        let cell_width = if cell_width > px(0.) {
            cell_width
        } else {
            font_size * 0.6
        };
        Self {
            font,
            font_size,
            cell_width,
            line_height: (font_size * 1.2).round(),
            key: (family.to_string(), font_size.as_f32().to_bits()),
        }
    }

    fn font_for(&self, bold: bool, italic: bool) -> Font {
        let mut font = self.font.clone();
        if bold {
            font.weight = FontWeight::BOLD;
        }
        if italic {
            font.style = FontStyle::Italic;
        }
        font
    }
}

/// Where the grid sits in the window, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GridGeometry {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub cols: usize,
    pub rows: usize,
}

impl Default for GridGeometry {
    fn default() -> Self {
        Self {
            origin: Point::default(),
            cell_width: px(8.),
            line_height: px(16.),
            cols: 80,
            rows: 24,
        }
    }
}

impl GridGeometry {
    /// The viewport cell under `position`, clamped to the grid, and the side of the cell.
    pub fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let x = ((position.x - self.origin.x) / self.cell_width).max(0.0);
        let y = ((position.y - self.origin.y) / self.line_height).max(0.0);
        let col = (x.floor() as usize).min(self.cols.saturating_sub(1));
        let row = (y.floor() as usize).min(self.rows.saturating_sub(1));
        let side = if x.fract() > 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (row, col, side)
    }

    pub fn cell_bounds(&self, row: usize, col: usize, width_cells: usize) -> Bounds<Pixels> {
        Bounds::new(
            point(
                self.origin.x + self.cell_width * col as f32,
                self.origin.y + self.line_height * row as f32,
            ),
            size(self.cell_width * width_cells as f32, self.line_height),
        )
    }
}

/// One shaped viewport row.
pub(crate) struct ShapedRow {
    /// `(start column, line)`.
    segments: Vec<(usize, ShapedLine)>,
    /// `[start, end)` columns with a non-default background.
    backgrounds: Vec<(usize, usize, Hsla)>,
}

/// Shaped rows keyed by content hash, kept for one frame after their last use.
#[derive(Default)]
pub(crate) struct RowCache {
    current: HashMap<u64, Rc<ShapedRow>>,
    previous: HashMap<u64, Rc<ShapedRow>>,
    /// Rows shaped in the last frame (for tests and perf logging).
    pub shaped_last_frame: usize,
}

impl RowCache {
    fn begin_frame(&mut self) {
        self.previous = std::mem::take(&mut self.current);
        self.shaped_last_frame = 0;
    }

    fn end_frame(&mut self) {
        self.previous.clear();
    }

    fn get_or_shape(&mut self, hash: u64, shape: impl FnOnce() -> ShapedRow) -> Rc<ShapedRow> {
        if let Some(row) = self.current.get(&hash) {
            return row.clone();
        }
        let row = match self.previous.remove(&hash) {
            Some(row) => row,
            None => {
                self.shaped_last_frame += 1;
                Rc::new(shape())
            }
        };
        self.current.insert(hash, row.clone());
        row
    }

    pub fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
    }
}

/// Resolved style of one cell.
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

fn is_default_bg(color: Color, overrides: &Colors) -> bool {
    matches!(color, Color::Named(NamedColor::Background))
        && overrides[NamedColor::Background].is_none()
}

fn cell_style(cell: &Cell, palette: &Palette, overrides: &Colors) -> CellStyle {
    let flags = cell.flags;
    let mut fg = palette.resolve(cell.fg, overrides);
    let mut bg = (!is_default_bg(cell.bg, overrides)).then(|| palette.resolve(cell.bg, overrides));
    if flags.contains(Flags::DIM) {
        fg = palette.dim(fg);
    }
    if flags.contains(Flags::INVERSE) {
        let old_bg = bg.unwrap_or(palette.background);
        bg = Some(fg);
        fg = old_bg;
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg.unwrap_or(palette.background);
    }
    CellStyle {
        fg,
        bg,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        undercurl: flags.contains(Flags::UNDERCURL),
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

fn hash_cell(cell: &Cell, h: &mut impl Hasher) {
    cell.c.hash(h);
    hash_color(cell.fg, h);
    hash_color(cell.bg, h);
    cell.flags.bits().hash(h);
    if let Some(zw) = cell.zerowidth() {
        zw.hash(h);
    }
}

fn printable(c: char) -> char {
    if c.is_control() { ' ' } else { c }
}

/// Shape one grid row.
fn shape_row(
    cells: &[Cell],
    palette: &Palette,
    overrides: &Colors,
    metrics: &CellMetrics,
    window: &Window,
) -> ShapedRow {
    let mut backgrounds: Vec<(usize, usize, Hsla)> = Vec::new();
    let mut segments = Vec::new();

    // Last cell that draws something (text or decoration); blank tails aren't shaped.
    let last_visible = cells
        .iter()
        .rposition(|cell| {
            let blank = matches!(cell.c, ' ' | '\t' | '\0') && cell.zerowidth().is_none();
            !blank
                || cell
                    .flags
                    .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT | Flags::INVERSE)
        })
        .map_or(0, |ix| ix + 1);

    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut run_style: Option<CellStyle> = None;
    let mut segment_start = 0usize;

    let flush_run = |runs: &mut Vec<TextRun>, style: Option<CellStyle>, len: usize| {
        let (Some(style), true) = (style, len > 0) else {
            return;
        };
        runs.push(TextRun {
            len,
            font: metrics.font_for(style.bold, style.italic),
            color: style.fg,
            background_color: None,
            underline: style.underline.then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(style.fg),
                wavy: style.undercurl,
            }),
            strikethrough: style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(style.fg),
            }),
        });
    };

    let mut run_len = 0usize;
    let mut col = 0usize;
    while col < cells.len() {
        let cell = &cells[col];
        let style = cell_style(cell, palette, overrides);
        if let Some(bg) = style.bg {
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            match backgrounds.last_mut() {
                Some((_, end, color)) if *end == col && *color == bg => *end = col + width,
                _ => backgrounds.push((col, col + width, bg)),
            }
        }

        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            || col >= last_visible
        {
            col += 1;
            continue;
        }

        if run_style != Some(style) {
            flush_run(&mut runs, run_style, run_len);
            run_style = Some(style);
            run_len = 0;
        }
        let before = text.len();
        text.push(printable(cell.c));
        if let Some(zw) = cell.zerowidth() {
            text.extend(zw.iter().copied());
        }
        run_len += text.len() - before;

        if cell.flags.contains(Flags::WIDE_CHAR) {
            // Glyphs after a wide char would be placed one cell too early by `force_width`;
            // end the segment here and start a new one after the spacer.
            flush_run(&mut runs, run_style, run_len);
            run_len = 0;
            push_segment(
                &mut segments,
                segment_start,
                &mut text,
                &mut runs,
                metrics,
                window,
            );
            segment_start = col + 2;
            col += 2;
            continue;
        }
        col += 1;
    }
    flush_run(&mut runs, run_style, run_len);
    push_segment(
        &mut segments,
        segment_start,
        &mut text,
        &mut runs,
        metrics,
        window,
    );

    ShapedRow {
        segments,
        backgrounds,
    }
}

fn push_segment(
    segments: &mut Vec<(usize, ShapedLine)>,
    start: usize,
    text: &mut String,
    runs: &mut Vec<TextRun>,
    metrics: &CellMetrics,
    window: &Window,
) {
    if text.is_empty() {
        runs.clear();
        return;
    }
    let line = window.text_system().shape_line(
        SharedString::from(std::mem::take(text)),
        metrics.font_size,
        runs,
        Some(metrics.cell_width),
    );
    runs.clear();
    segments.push((start, line));
}

enum CursorPaint {
    Block {
        bounds: Bounds<Pixels>,
        color: Hsla,
        glyph: Option<Box<ShapedLine>>,
    },
    Hollow {
        bounds: Bounds<Pixels>,
        color: Hsla,
    },
    Bar {
        bounds: Bounds<Pixels>,
        color: Hsla,
    },
}

/// Everything paint needs, built in prepaint.
pub(crate) struct Frame {
    geometry: GridGeometry,
    rows: Vec<Rc<ShapedRow>>,
    selection: Vec<Bounds<Pixels>>,
    selection_color: Hsla,
    cursor: Option<CursorPaint>,
    marked: Option<(Bounds<Pixels>, ShapedLine)>,
    marked_bg: Hsla,
    hitbox: Hitbox,
    mouse_reporting: bool,
}

/// The grid element. Created by `TerminalView::render`.
pub(crate) struct TerminalElement {
    view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    palette: Palette,
}

impl TerminalElement {
    pub fn new(
        view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        palette: Palette,
    ) -> Self {
        Self {
            view,
            focus,
            focused,
            palette,
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: Size::<Length>::full(),
            flex_grow: 1.0,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let palette = self.palette.clone();
        let focused = self.focused;
        self.view.update(cx, |view, cx| {
            view.prepaint_frame(bounds, hitbox, palette, focused, window, cx)
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        frame: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let geometry = frame.geometry;
        window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
            for (row, shaped) in frame.rows.iter().enumerate() {
                for &(start, end, color) in &shaped.backgrounds {
                    window.paint_quad(fill(geometry.cell_bounds(row, start, end - start), color));
                }
            }
            for rect in &frame.selection {
                window.paint_quad(fill(*rect, frame.selection_color));
            }
            for (row, shaped) in frame.rows.iter().enumerate() {
                for (col, line) in &shaped.segments {
                    let origin = geometry.cell_bounds(row, *col, 1).origin;
                    let _ = line.paint(
                        origin,
                        geometry.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            match &frame.cursor {
                Some(CursorPaint::Block {
                    bounds,
                    color,
                    glyph,
                }) => {
                    window.paint_quad(fill(*bounds, *color));
                    if let Some(glyph) = glyph {
                        let _ = glyph.paint(
                            bounds.origin,
                            geometry.line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                }
                Some(CursorPaint::Hollow { bounds, color }) => {
                    window.paint_quad(outline(*bounds, *color, Default::default()));
                }
                Some(CursorPaint::Bar { bounds, color }) => {
                    window.paint_quad(fill(*bounds, *color));
                }
                None => {}
            }
            if let Some((rect, line)) = &frame.marked {
                window.paint_quad(fill(*rect, frame.marked_bg));
                let _ = line.paint(
                    rect.origin,
                    geometry.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });

        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        window.set_cursor_style(
            if frame.mouse_reporting {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            },
            &frame.hitbox,
        );
        register_mouse_listeners(&self.view, &frame.hitbox, window);
    }
}

fn register_mouse_listeners(view: &Entity<TerminalView>, hitbox: &Hitbox, window: &mut Window) {
    // Capture phase: mouse reporting must win over the context menu's right-click handler.
    let (v, hb) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture && hb.is_hovered(window) {
            let consumed = v.update(cx, |view, cx| view.mouse_down_report(event, window, cx));
            if consumed {
                cx.stop_propagation();
            }
        }
    });
    let (v, hb) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
            v.update(cx, |view, cx| view.mouse_down(event, window, cx));
        }
    });
    let (v, hb) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            let hovered = hb.is_hovered(window);
            v.update(cx, |view, cx| view.mouse_move(event, hovered, cx));
        }
    });
    let v = view.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            v.update(cx, |view, cx| view.mouse_up(event, cx));
        }
    });
    let (v, hb) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hb.should_handle_scroll(window) {
            v.update(cx, |view, cx| view.scroll_wheel(event, cx));
            cx.stop_propagation();
        }
    });
}

impl TerminalView {
    /// Lay out the grid for `bounds` and build the frame (called from the element's prepaint).
    pub(crate) fn prepaint_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        hitbox: Hitbox,
        palette: Palette,
        focused: bool,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Frame {
        let started = std::time::Instant::now();
        let metrics = self.metrics(window, cx);
        let origin = point(bounds.origin.x + PADDING, bounds.origin.y + PADDING);
        let avail_w = (bounds.size.width - PADDING * 2.0).max(px(0.));
        let avail_h = (bounds.size.height - PADDING * 2.0).max(px(0.));
        let cols = ((avail_w / metrics.cell_width).floor() as u16).max(crate::model::MIN_COLS);
        let rows = ((avail_h / metrics.line_height).floor() as u16).max(crate::model::MIN_ROWS);
        self.grid_resized(cols, rows, &metrics, cx);

        let (cols, rows) = self.model.size();
        let geometry = GridGeometry {
            origin,
            cell_width: metrics.cell_width,
            line_height: metrics.line_height,
            cols: usize::from(cols),
            rows: usize::from(rows),
        };
        self.geometry = geometry;

        let term = self.model.term();
        let overrides = *term.colors();
        let grid = term.grid();
        let display_offset = grid.display_offset();

        // Content hash seed: everything besides the cells that changes how a row looks.
        let mut seed = std::hash::DefaultHasher::new();
        palette.key(&overrides).hash(&mut seed);
        metrics.key.hash(&mut seed);
        metrics.cell_width.as_f32().to_bits().hash(&mut seed);
        let seed = seed.finish();

        self.row_cache.begin_frame();
        let mut shaped_rows = Vec::with_capacity(geometry.rows);
        for row in 0..geometry.rows {
            let line = &grid[Line(row as i32 - display_offset as i32)];
            let cells: Vec<Cell> = (0..geometry.cols)
                .map(|col| line[Column(col)].clone())
                .collect();
            let mut h = std::hash::DefaultHasher::new();
            seed.hash(&mut h);
            for cell in &cells {
                hash_cell(cell, &mut h);
            }
            let hash = h.finish();
            let shaped = self.row_cache.get_or_shape(hash, || {
                shape_row(&cells, &palette, &overrides, &metrics, window)
            });
            shaped_rows.push(shaped);
        }
        self.row_cache.end_frame();

        let content = term.renderable_content();

        // Selection rectangles, per viewport row.
        let mut selection = Vec::new();
        if let Some(range) = content.selection {
            for row in 0..geometry.rows {
                let line = Line(row as i32 - display_offset as i32);
                if line < range.start.line || line > range.end.line {
                    continue;
                }
                let (start, end) = if range.is_block {
                    (range.start.column.0, range.end.column.0)
                } else {
                    let start = if line == range.start.line {
                        range.start.column.0
                    } else {
                        0
                    };
                    let end = if line == range.end.line {
                        range.end.column.0
                    } else {
                        geometry.cols - 1
                    };
                    (start, end)
                };
                if end >= start {
                    selection.push(geometry.cell_bounds(row, start, end - start + 1));
                }
            }
        }

        // Cursor.
        let mut cursor = None;
        let inert = self.inert();
        if content.cursor.shape != CursorShape::Hidden
            && !inert
            && let Some(vp) = point_to_viewport(display_offset, content.cursor.point)
            && vp.line < geometry.rows
            && vp.column.0 < geometry.cols
        {
            let cell = &grid[content.cursor.point];
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let cell_bounds = geometry.cell_bounds(vp.line, vp.column.0, if wide { 2 } else { 1 });
            let color = palette.cursor;
            let bar = px(2.);
            cursor = Some(match (focused, content.cursor.shape) {
                (false, _) | (_, CursorShape::HollowBlock) => CursorPaint::Hollow {
                    bounds: cell_bounds,
                    color,
                },
                (true, CursorShape::Beam) => CursorPaint::Bar {
                    bounds: Bounds::new(cell_bounds.origin, size(bar, cell_bounds.size.height)),
                    color,
                },
                (true, CursorShape::Underline) => CursorPaint::Bar {
                    bounds: Bounds::new(
                        point(
                            cell_bounds.origin.x,
                            cell_bounds.origin.y + cell_bounds.size.height - bar,
                        ),
                        size(cell_bounds.size.width, bar),
                    ),
                    color,
                },
                (true, _) => {
                    let glyph = (!matches!(cell.c, ' ' | '\t' | '\0')).then(|| {
                        let text: SharedString = printable(cell.c).to_string().into();
                        let style = cell_style(cell, &palette, &overrides);
                        window.text_system().shape_line(
                            text.clone(),
                            metrics.font_size,
                            &[TextRun {
                                len: text.len(),
                                font: metrics.font_for(style.bold, style.italic),
                                color: palette.background,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            Some(metrics.cell_width),
                        )
                    });
                    CursorPaint::Block {
                        bounds: cell_bounds,
                        color,
                        glyph: glyph.map(Box::new),
                    }
                }
            });
        }

        // IME composition at the cursor.
        let marked = self
            .marked_text
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|text| {
                let text: SharedString = text.clone().into();
                let line = window.text_system().shape_line(
                    text.clone(),
                    metrics.font_size,
                    &[TextRun {
                        len: text.len(),
                        font: metrics.font.clone(),
                        color: palette.foreground,
                        background_color: None,
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(palette.foreground),
                            wavy: false,
                        }),
                        strikethrough: None,
                    }],
                    None,
                );
                let vp = point_to_viewport(display_offset, content.cursor.point);
                let (row, col) = vp.map_or((0, 0), |p| (p.line, p.column.0));
                let origin = geometry.cell_bounds(row, col, 1).origin;
                (
                    Bounds::new(origin, size(line.width(), geometry.line_height)),
                    line,
                )
            });

        self.last_prepaint = started.elapsed();
        Frame {
            geometry,
            rows: shaped_rows,
            selection,
            selection_color: palette.selection,
            cursor,
            marked,
            marked_bg: palette.background,
            hitbox,
            mouse_reporting: self.mouse_reporting(false),
        }
    }

    /// Time spent building and shaping the grid in the last frame (TRM-010 diagnostics).
    #[doc(hidden)]
    pub fn last_prepaint_time(&self) -> std::time::Duration {
        self.last_prepaint
    }

    /// Number of rows reshaped in the last frame (cache misses).
    #[doc(hidden)]
    pub fn rows_shaped_last_frame(&self) -> usize {
        self.row_cache.shaped_last_frame
    }

    pub(crate) fn metrics(&mut self, window: &Window, cx: &App) -> CellMetrics {
        let family = super::font_family(&self.config, cx);
        let key = (family.to_string(), self.config.font_size.max(4.0).to_bits());
        if let Some(metrics) = &self.cell_metrics
            && metrics.key == key
        {
            return metrics.clone();
        }
        let metrics = CellMetrics::measure(family, self.config.font_size, window);
        self.row_cache.clear();
        self.cell_metrics = Some(metrics.clone());
        metrics
    }

    pub(crate) fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        self.geometry.cell_at(position)
    }

    pub(crate) fn cursor_bounds(&self) -> Bounds<Pixels> {
        let term = self.model.term();
        let display_offset = term.grid().display_offset();
        let cursor = term.grid().cursor.point;
        let (row, col) =
            point_to_viewport(display_offset, cursor).map_or((0, 0), |p| (p.line, p.column.0));
        self.geometry.cell_bounds(row, col, 1)
    }
}
