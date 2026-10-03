//! The app's segmented control (SHL-025): detail tabs, inner tab bars (Stats window, terminal
//! sessions and shell) and list filters all use it, so a selection reads the same everywhere.
//!
//! Custom element on the unstyled GPUI Kit base `Tabs`/`Tab` (tab-list semantics, pointer
//! activation): the styled `TabBar` paints its selection with the window background, which
//! can't be themed apart from it. Light mode inverts the selected segment (dark pill, light
//! text); dark mode lifts it to a lighter pill. The pill slides between segments.
//!
//! Keyboard focus stays with the caller's wrapper: one Tab stop, arrows move (KBD-040).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::{Tab, Tabs, Transition, spring, transition};
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, ElementId, Hsla, Pixels, Point, SharedString, Window, canvas, div, px,
};

type OnSelect = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// One segment: label, optional icon, disabled.
pub struct Segment {
    label: SharedString,
    icon: Option<Icon>,
    disabled: bool,
    selector: Option<SharedString>,
}

impl Segment {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            disabled: false,
            selector: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A `debug_selector` for view tests.
    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    segments: Vec<Segment>,
    selected: usize,
    size: gpui_kit::component::Size,
    on_select: Option<OnSelect>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: 0,
            size: gpui_kit::component::Size::Medium,
            on_select: None,
        }
    }

    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    pub fn segments(mut self, segments: impl IntoIterator<Item = Segment>) -> Self {
        self.segments.extend(segments);
        self
    }

    pub fn on_select(mut self, f: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl Sizable for Segmented {
    fn with_size(mut self, size: impl Into<gpui_kit::component::Size>) -> Self {
        self.size = size.into();
        self
    }
}

/// Segment bounds from the last prepaint, relative to the control's padding box.
#[derive(Default)]
struct Measure {
    origin: Point<Pixels>,
    segments: Vec<Option<Bounds<Pixels>>>,
    /// Where this frame's render put the pill (`None`: painted inline, not yet measured).
    shown: Option<Bounds<Pixels>>,
    refresh_queued: bool,
}

struct Palette {
    trough: Hsla,
    pill: Hsla,
    /// Light mode only: the white pill needs an edge and a lift to read on the trough.
    pill_border: Option<Hsla>,
    pill_fg: Hsla,
    fg: Hsla,
    hover_bg: Hsla,
    hover_fg: Hsla,
    disabled_fg: Hsla,
}

fn palette(cx: &App) -> Palette {
    let t = cx.theme();
    let trough = t.tab_bar_segmented;
    if t.is_dark() {
        Palette {
            trough,
            pill: trough.blend(Hsla::white().opacity(0.16)),
            pill_border: None,
            pill_fg: t.foreground,
            fg: t.foreground.opacity(0.68),
            hover_bg: Hsla::white().opacity(0.06),
            hover_fg: t.foreground,
            disabled_fg: t.muted_foreground.opacity(0.6),
        }
    } else {
        // A raised white pill on a slightly deeper trough.
        Palette {
            trough: trough.blend(Hsla::black().opacity(0.04)),
            pill: Hsla::white(),
            pill_border: Some(Hsla::black().opacity(0.08)),
            pill_fg: t.foreground,
            fg: t.foreground.opacity(0.62),
            hover_bg: Hsla::black().opacity(0.05),
            hover_fg: t.foreground,
            disabled_fg: t.muted_foreground.opacity(0.6),
        }
    }
}

fn key(id: &ElementId, part: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), part.into())
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui_kit::component::Size;
        let small = matches!(self.size, Size::XSmall | Size::Small);
        // Same footprint as the GPUI Kit segmented bar: 24 px small, 32 px medium.
        let (pad, seg_h, seg_px, radius) = if small {
            (px(3.), px(18.), px(10.), cx.theme().radius)
        } else {
            (px(4.), px(24.), px(12.), cx.theme().radius_lg)
        };
        let seg_radius = (radius - px(2.)).max(px(0.));
        let pal = palette(cx);
        let motion = cx.theme().motion_tokens();
        let (spring_move, fade) = (motion.spring_move, motion.duration_normal);

        let id = self.id;
        let selected = self.selected;
        let n = self.segments.len();
        let measure = window
            .use_keyed_state(key(&id, "measure"), cx, |_, _| {
                Rc::new(RefCell::new(Measure::default()))
            })
            .read(cx)
            .clone();
        let target = {
            let mut m = measure.borrow_mut();
            m.segments.resize(n, None);
            m.refresh_queued = false;
            m.shown = m.segments.get(selected).copied().flatten();
            m.shown
        };

        let pill = target.map(|b| {
            let left = spring(key(&id, "pill-left"), b.origin.x, spring_move, window, cx);
            let width = spring(
                key(&id, "pill-width"),
                b.size.width,
                spring_move,
                window,
                cx,
            );
            div()
                .absolute()
                .top(b.origin.y)
                .h(b.size.height)
                .left(left)
                .w(width)
                .rounded(seg_radius)
                .bg(pal.pill)
                .when_some(pal.pill_border, |d, c| {
                    d.border_1().border_color(c).shadow_xs()
                })
        });

        let on_select = self.on_select;
        let segments: Vec<_> = self
            .segments
            .into_iter()
            .enumerate()
            .map(|(ix, seg)| {
                let is_selected = ix == selected;
                let hoverable = !is_selected && !seg.disabled;
                let fg_target = if seg.disabled {
                    pal.disabled_fg
                } else if is_selected {
                    pal.pill_fg
                } else {
                    pal.fg
                };
                // The text fades with the pill instead of flipping before it arrives.
                let fg = transition(
                    key(&id, format!("fg-{ix}")),
                    fg_target,
                    Transition::new(fade),
                    window,
                    cx,
                );
                let measure = measure.clone();
                let (hover_bg, hover_fg) = (pal.hover_bg, pal.hover_fg);
                Tab::new(ix)
                    .selected(is_selected)
                    .disabled(seg.disabled)
                    .accessibility_label(seg.label.clone())
                    .set_position(ix + 1, n)
                    .relative()
                    .flex_shrink_0()
                    .h(seg_h)
                    .px(seg_px)
                    .rounded(seg_radius)
                    .text_color(fg)
                    // Before the first measurement the pill can't slide in; paint it inline.
                    .when(is_selected && target.is_none(), |t| {
                        t.bg(pal.pill).when_some(pal.pill_border, |t, c| {
                            t.border_1().border_color(c).shadow_xs()
                        })
                    })
                    .when(hoverable, |t| t.cursor_pointer())
                    // Always registered: GPUI only refreshes hover state while one is present.
                    .hover(move |s| {
                        if hoverable {
                            s.bg(hover_bg).text_color(hover_fg)
                        } else {
                            s
                        }
                    })
                    .when_some(seg.selector, |t, sel| {
                        t.debug_selector(move || sel.to_string())
                    })
                    .when_some(on_select.clone(), |t, f| {
                        t.on_click(move |_, window, cx| f(&ix, window, cx))
                    })
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .when_some(seg.icon, |el, icon| el.child(icon.small()))
                            .child(seg.label),
                    )
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                let mut m = measure.borrow_mut();
                                let rel = Bounds {
                                    origin: bounds.origin - m.origin,
                                    size: bounds.size,
                                };
                                if let Some(slot) = m.segments.get_mut(ix) {
                                    *slot = Some(rel);
                                }
                                // Layout moved under the pill (first frame, zoom, labels):
                                // render once more with the new bounds.
                                if ix == selected && m.shown != Some(rel) && !m.refresh_queued {
                                    m.refresh_queued = true;
                                    window.on_next_frame(|window, _| window.refresh());
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
            })
            .collect();

        Tabs::new(id)
            .relative()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(pad)
            .rounded(radius)
            .bg(pal.trough)
            .text_sm()
            // First child, so the origin is known before the segments measure themselves.
            .child(
                canvas(
                    move |bounds, _, _| measure.borrow_mut().origin = bounds.origin,
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(pill)
            .children(segments)
    }
}
