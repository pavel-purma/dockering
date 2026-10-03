//! Focusable key/value panels for the Overview, Mounts and Network tabs (KBD-044).
//!
//! The whole panel is **one** Tab stop. `↑`/`↓`/`Home`/`End`/`PgUp`/`PgDn` move a row cursor
//! across every section, `Mod+C` copies the focused value, `Enter` follows the row's link
//! (image, volume, network, port), and `Space` reveals or hides a masked value (CDT-010).
//!
//! Sections are rebuilt from the inspect data on every render (cheap) and rendered with
//! a [`crate::ui::section`] heading + `DescriptionList` (key/value) or the Kit `Table`.
//! The cursor and the revealed set are kept by row *key*, so a refresh never moves them
//! (KBD-007, SHL-004).

use std::cell::Cell as StdCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::description_list::{DescriptionItem, DescriptionList};
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, AnyElement, App, ClipboardItem, Context, FocusHandle, IntoElement, MouseButton, Pixels,
    ScrollHandle, SharedString, Window, div, point, px, relative,
};

use crate::actions::rows;
use crate::strings as s;
use crate::ui::menu::TrackBounds as _;
use crate::ui::notify;

/// Key context of a rows panel (bindings in `keymap.rs`).
pub const ROWS_CONTEXT: &str = "DetailRows";

/// One cell of a row.
pub enum Cell {
    Text(SharedString),
    /// Monospaced (ids, ports, paths).
    Mono(SharedString),
    /// A link: clicking (or `Enter` on the row) dispatches the row action.
    Link(SharedString),
    /// A value masked until revealed (CDT-010).
    Secret(SharedString),
    /// A pre-built element (chips).
    Element(AnyElement),
}

impl Cell {
    pub fn text(s: impl Into<SharedString>) -> Self {
        Cell::Text(s.into())
    }
    pub fn mono(s: impl Into<SharedString>) -> Self {
        Cell::Mono(s.into())
    }
}

/// One row: cells, the value `Mod+C` copies, and the `Enter` action.
pub struct Row {
    pub key: SharedString,
    pub cells: Vec<Cell>,
    pub copy: SharedString,
    pub action: Option<Box<dyn Action>>,
}

impl Row {
    pub fn new(
        key: impl Into<SharedString>,
        cells: Vec<Cell>,
        copy: impl Into<SharedString>,
    ) -> Self {
        Self {
            key: key.into(),
            cells,
            copy: copy.into(),
            action: None,
        }
    }

    /// A key/value row for a `List` section.
    pub fn kv(key: &str, label: &str, value: Cell, copy: impl Into<SharedString>) -> Self {
        Self::new(
            key.to_owned(),
            vec![Cell::text(label.to_owned()), value],
            copy,
        )
    }

    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    fn is_secret(&self) -> bool {
        self.cells.iter().any(|c| matches!(c, Cell::Secret(_)))
    }
}

pub enum Layout {
    /// Key/value pairs (`DescriptionList`).
    List,
    /// A table with these column headers (Kit `Table`) and optional relative column widths.
    Table {
        headers: Vec<SharedString>,
        weights: Option<Vec<f32>>,
    },
}

pub struct Section {
    pub id: &'static str,
    pub title: SharedString,
    pub layout: Layout,
    pub rows: Vec<Row>,
    /// Shown when `rows` is empty.
    pub empty: SharedString,
}

impl Section {
    pub fn list(id: &'static str, title: &str, rows: Vec<Row>) -> Self {
        Self {
            id,
            title: title.to_owned().into(),
            layout: Layout::List,
            rows,
            empty: s::NONE_VALUE.into(),
        }
    }

    pub fn table(
        id: &'static str,
        title: &str,
        headers: &[&str],
        rows: Vec<Row>,
        empty: &str,
    ) -> Self {
        Self {
            id,
            title: title.to_owned().into(),
            layout: Layout::Table {
                headers: headers
                    .iter()
                    .map(|h| SharedString::from(h.to_string()))
                    .collect(),
                weights: None,
            },
            rows,
            empty: empty.to_owned().into(),
        }
    }

    /// Relative column widths of a table (one per header; the Kit default is equal widths).
    pub fn weights(mut self, w: &[f32]) -> Self {
        if let Layout::Table { headers, weights } = &mut self.layout {
            debug_assert_eq!(headers.len(), w.len());
            *weights = Some(w.to_vec());
        }
        self
    }
}

/// Per-panel state that survives refreshes and tab switches (CDT-081).
pub struct RowsState {
    pub focus: FocusHandle,
    cursor: Option<SharedString>,
    revealed: HashSet<SharedString>,
    // Synced from the sections at render.
    keys: Vec<SharedString>,
    copies: Vec<SharedString>,
    actions: Vec<Option<Box<dyn Action>>>,
    secrets: Vec<bool>,
    scroll: ScrollHandle,
    /// Scroll the cursor row into view after the next layout.
    reveal_cursor: Rc<StdCell<bool>>,
}

impl RowsState {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            cursor: None,
            revealed: HashSet::new(),
            keys: Vec::new(),
            copies: Vec::new(),
            actions: Vec::new(),
            secrets: Vec::new(),
            scroll: ScrollHandle::new(),
            reveal_cursor: Rc::new(StdCell::new(false)),
        }
    }

    pub fn cursor(&self) -> Option<&SharedString> {
        self.cursor.as_ref()
    }

    pub fn set_cursor(&mut self, key: impl Into<SharedString>) {
        self.cursor = Some(key.into());
        self.reveal_cursor.set(true);
    }

    pub fn is_revealed(&self, key: &str) -> bool {
        self.revealed.contains(key)
    }

    pub fn keys(&self) -> &[SharedString] {
        &self.keys
    }

    /// Takes the row index data from the sections (keeps the cursor by key).
    fn sync(&mut self, sections: &[Section]) {
        self.keys.clear();
        self.copies.clear();
        self.actions.clear();
        self.secrets.clear();
        for row in sections.iter().flat_map(|s| s.rows.iter()) {
            self.keys.push(row.key.clone());
            self.copies.push(row.copy.clone());
            self.actions
                .push(row.action.as_ref().map(|a| a.boxed_clone()));
            self.secrets.push(row.is_secret());
        }
        if self.cursor.as_ref().is_none_or(|c| !self.keys.contains(c)) {
            self.cursor = self.keys.first().cloned();
        }
    }

    fn cursor_ix(&self) -> Option<usize> {
        let c = self.cursor.as_ref()?;
        self.keys.iter().position(|k| k == c)
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.keys.is_empty() {
            return;
        }
        let n = self.keys.len() as isize;
        let ix = self.cursor_ix().map_or(0, |i| i as isize);
        let next = (ix + delta).clamp(0, n - 1) as usize;
        self.cursor = Some(self.keys[next].clone());
        self.reveal_cursor.set(true);
    }

    pub fn move_to(&mut self, last: bool) {
        let key = if last {
            self.keys.last()
        } else {
            self.keys.first()
        };
        if let Some(k) = key.cloned() {
            self.cursor = Some(k);
            self.reveal_cursor.set(true);
        }
    }

    /// The focused row's value (unmasked: copying is an explicit user action).
    pub fn copy_value(&self) -> Option<SharedString> {
        self.cursor_ix().and_then(|i| self.copies.get(i).cloned())
    }

    fn cursor_action(&self) -> Option<Box<dyn Action>> {
        self.cursor_ix()
            .and_then(|i| self.actions.get(i))
            .and_then(|a| a.as_ref().map(|a| a.boxed_clone()))
    }

    fn cursor_is_secret(&self) -> bool {
        self.cursor_ix()
            .and_then(|i| self.secrets.get(i).copied())
            .unwrap_or(false)
    }

    pub fn toggle_reveal(&mut self, key: &str) {
        if !self.revealed.remove(key) {
            self.revealed.insert(key.to_owned().into());
        }
    }
}

const PAGE_ROWS: isize = 10;

/// Renders the panel. `access` maps the owning view to its `RowsState`; every handler is an
/// action (KBD-002), clicks dispatch through the panel's focus handle.
pub fn render<V: 'static>(
    id: &'static str,
    state: &mut RowsState,
    sections: Vec<Section>,
    window: &mut Window,
    cx: &mut Context<V>,
    access: fn(&mut V) -> &mut RowsState,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    state.sync(&sections);
    let focused = state.focus.is_focused(window);
    let cursor = if focused { state.cursor.clone() } else { None };
    let focus = state.focus.clone();
    let reveal = state.reveal_cursor.clone();
    let scroll = state.scroll.clone();

    let mv = |delta: isize| {
        move |v: &mut V, cx: &mut Context<V>| {
            access(v).move_by(delta);
            cx.notify();
        }
    };
    let up = mv(-1);
    let down = mv(1);
    let pg_up = mv(-PAGE_ROWS);
    let pg_down = mv(PAGE_ROWS);

    let body = v_flex()
        .gap_4()
        .children(sections.into_iter().map(|section| {
            render_section(
                section,
                state,
                cursor.as_ref(),
                &focus,
                &reveal,
                &scroll,
                cx,
                access,
            )
        }));

    div()
        .id(id)
        .key_context(format!("DetailHeader {ROWS_CONTEXT}").as_str())
        .track_focus(&state.focus)
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&state.scroll)
        .p_1()
        .rounded(cx.theme().radius)
        .on_action(cx.listener(move |v, _: &rows::Up, _, cx| up(v, cx)))
        .on_action(cx.listener(move |v, _: &rows::Down, _, cx| down(v, cx)))
        .on_action(cx.listener(move |v, _: &rows::PageUp, _, cx| pg_up(v, cx)))
        .on_action(cx.listener(move |v, _: &rows::PageDown, _, cx| pg_down(v, cx)))
        .on_action(cx.listener(move |v, _: &rows::First, _, cx| {
            access(v).move_to(false);
            cx.notify();
        }))
        .on_action(cx.listener(move |v, _: &rows::Last, _, cx| {
            access(v).move_to(true);
            cx.notify();
        }))
        .on_action(cx.listener(move |v, _: &rows::CopyValue, window, cx| {
            if let Some(text) = access(v).copy_value() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                notify::info(window, cx, s::COPIED);
            }
        }))
        .on_action(cx.listener(move |v, _: &rows::Activate, window, cx| {
            let st = access(v);
            if let Some(a) = st.cursor_action() {
                window.dispatch_action(a, cx);
            } else if st.cursor_is_secret()
                && let Some(k) = st.cursor.clone()
            {
                st.toggle_reveal(&k);
                cx.notify();
            }
        }))
        .on_action(cx.listener(move |v, _: &rows::ToggleReveal, _, cx| {
            let st = access(v);
            if st.cursor_is_secret()
                && let Some(k) = st.cursor.clone()
            {
                st.toggle_reveal(&k);
                cx.notify();
            }
        }))
        .on_action(cx.listener(move |v, a: &rows::RevealRow, _, cx| {
            access(v).toggle_reveal(&a.key);
            cx.notify();
        }))
        .child(body)
}

#[allow(clippy::too_many_arguments)]
fn render_section<V: 'static>(
    section: Section,
    state: &RowsState,
    cursor: Option<&SharedString>,
    focus: &FocusHandle,
    reveal: &Rc<StdCell<bool>>,
    scroll: &ScrollHandle,
    cx: &mut Context<V>,
    access: fn(&mut V) -> &mut RowsState,
) -> AnyElement {
    let body: AnyElement = if section.rows.is_empty() {
        div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(section.empty.clone())
            .into_any_element()
    } else {
        match section.layout {
            Layout::List => {
                let items: Vec<DescriptionItem> = section
                    .rows
                    .into_iter()
                    .map(|row| {
                        let is_cursor = cursor == Some(&row.key);
                        let mut cells = row.cells.into_iter();
                        let label = match cells.next() {
                            Some(Cell::Text(t)) | Some(Cell::Mono(t)) => t,
                            _ => SharedString::default(),
                        };
                        let value = cells.next().unwrap_or(Cell::Text(SharedString::default()));
                        let el = cell_element(
                            value, &row.key, is_cursor, focus, reveal, scroll, state, cx, access,
                        );
                        DescriptionItem::new(label).value(el)
                    })
                    .collect();
                DescriptionList::new()
                    .columns(1)
                    .small()
                    .label_width(px(160.))
                    .children(items)
                    .into_any_element()
            }
            Layout::Table { headers, weights } => {
                let basis = |ix: usize| weights.as_ref().and_then(|w| w.get(ix)).copied();
                let head = TableHeader::new().child(TableRow::new().children(
                    headers.into_iter().enumerate().map(|(ix, h)| {
                        TableHead::new()
                            .when_some(basis(ix), |el, w| el.flex_basis(relative(w)))
                            .child(h)
                    }),
                ));
                let body = TableBody::new().children(section.rows.into_iter().map(|row| {
                    let is_cursor = cursor == Some(&row.key);
                    let key = row.key.clone();
                    TableRow::new()
                        .when(is_cursor, |r| r.bg(cx.theme().accent))
                        .children(row.cells.into_iter().enumerate().map(|(ix, c)| {
                            TableCell::new()
                                .when_some(basis(ix), |el, w| el.flex_basis(relative(w)))
                                .child(cell_element(
                                    c, &key, false, focus, reveal, scroll, state, cx, access,
                                ))
                        }))
                }));
                Table::new()
                    .small()
                    .child(head)
                    .child(body)
                    .into_any_element()
            }
        }
    };
    crate::ui::section(section.title.clone(), body, cx)
        .id(section.id)
        .into_any_element()
}

/// One cell's content. Clicking selects the row (and focuses the panel); cursor rows of
/// key/value lists get the highlight here (tables highlight the whole row).
#[allow(clippy::too_many_arguments)]
fn cell_element<V: 'static>(
    cell: Cell,
    key: &SharedString,
    highlight: bool,
    focus: &FocusHandle,
    reveal: &Rc<StdCell<bool>>,
    scroll: &ScrollHandle,
    state: &RowsState,
    cx: &mut Context<V>,
    access: fn(&mut V) -> &mut RowsState,
) -> AnyElement {
    let theme = cx.theme();
    let row_key = key.clone();
    let select = cx.listener(move |v, _: &gpui_kit::MouseDownEvent, window, cx| {
        let st = access(v);
        st.cursor = Some(row_key.clone());
        let f = st.focus.clone();
        window.focus(&f, cx);
        cx.notify();
    });
    let content: AnyElement = match cell {
        Cell::Text(t) => div().child(t).into_any_element(),
        Cell::Mono(t) => div()
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .child(t)
            .into_any_element(),
        Cell::Link(t) => div()
            .text_color(theme.link)
            .cursor_pointer()
            .child(format!("{t} ↗"))
            .into_any_element(),
        Cell::Secret(value) => {
            let revealed = state.is_revealed(key);
            let action = rows::RevealRow { key: key.clone() };
            let focus = focus.clone();
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(if revealed {
                            value
                        } else {
                            SharedString::from(s::MASKED_VALUE)
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("reveal-{key}")))
                        .ghost()
                        .xsmall()
                        .tab_stop(false)
                        .icon(if revealed {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        })
                        .tooltip(if revealed {
                            s::HIDE_VALUE
                        } else {
                            s::REVEAL_VALUE
                        })
                        .on_click(move |_, window, cx| focus.dispatch_action(&action, window, cx)),
                )
                .into_any_element()
        }
        Cell::Element(e) => e,
    };
    let reveal = reveal.clone();
    let scroll = scroll.clone();
    div()
        .id(SharedString::from(format!("cell-{key}")))
        .w_full()
        .min_w_0()
        .px_1()
        .text_sm()
        .rounded(theme.radius)
        .when(highlight, |el| el.bg(theme.accent))
        .on_mouse_down(MouseButton::Left, select)
        .child(content)
        .when(highlight, move |el| {
            // Keep the cursor row in view (KBD-031-style) after keyboard moves.
            el.on_bounds(move |bounds, window, _| {
                if !reveal.replace(false) {
                    return;
                }
                scroll_into_view(&scroll, bounds.top(), bounds.bottom());
                window.refresh();
            })
        })
        .into_any_element()
}

fn scroll_into_view(scroll: &ScrollHandle, top: Pixels, bottom: Pixels) {
    let view = scroll.bounds();
    if view.size.height <= px(0.) {
        return;
    }
    let offset = scroll.offset();
    let margin = px(8.);
    if top < view.top() {
        scroll.set_offset(point(offset.x, offset.y + (view.top() - top) + margin));
    } else if bottom > view.bottom() {
        scroll.set_offset(point(
            offset.x,
            offset.y - (bottom - view.bottom()) - margin,
        ));
    }
}
