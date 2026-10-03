//! `ListTable<D>`: the generic list used by every list page (spec 30 §1 "Lists"). A thin
//! wrapper over GPUI Kit `DataTable`/`TableDelegate` that adds:
//!
//! - rows that are groups or items, with depth/expanded state (CON-011);
//! - a **cursor** (GPUI Kit row selection) separate from the **checkbox multi-selection**
//!   (KBD-034/035);
//! - roving focus: the table is one Tab stop, `Tab`/`Shift+Tab` leave it (KBD-004, S-8);
//! - `←/→` collapse/expand, `Mod+←/→` all, `Home/End`, `Space`, `Shift+↑/↓`, `Mod+A`, `Esc`,
//!   `Enter`, `/` quick find, `Shift+F10` context menu (KBD-031…037);
//! - sorting by header click / `Mod+Shift+O` and persisted column widths (CON-002/003);
//! - an optional column pinned to the right edge (`ColumnSpec::pin_right`, the row
//!   actions), always visible while the other columns scroll horizontally;
//! - a click anywhere on a row opens it (item → detail, group → expand/collapse, CON-033).
//!   Controls inside cells (checkbox, buttons, links) stop propagation so they don't;
//! - id-stable refresh and focus-to-next-row after deletes (KBD-007).
//!
//! Pages implement [`ListDelegate`] (cells, columns, menus) and handle the [`ListEvent`]s.
//! Page-specific single letters (`S`, `R`, …) are plain actions bound in the `ListTable`
//! context; the page's element handles them and reads [`ListTable::targets`].

mod model;

pub use model::{ListModel, ListNode, ListRow, RowKind, SortState};

use std::ops::Range;

use gpui_kit::base::actions::{SelectDown, SelectPageDown, SelectPageUp, SelectUp};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Size, StyleSized, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Pixels, Render, SharedString, Subscription, Window, div, px,
};

use crate::actions::{OnRow, RowCommand, list};
use crate::keymap::ctx;
use crate::state::AppState;
use crate::strings as s;
use crate::ui::menu::{KeyMenu, MenuAnchor, TrackBounds as _};

/// Key of the leading checkbox column; its header renders a select-all checkbox.
pub const SELECT_COLUMN: &str = "select";

/// A column definition.
#[derive(Debug, Clone)]
pub struct ColumnSpec {
    pub key: &'static str,
    pub name: SharedString,
    pub width: Pixels,
    pub min_width: Pixels,
    pub sortable: bool,
    pub resizable: bool,
    pub right: bool,
    /// Pinned to the right edge of the table, outside the horizontal scroll (at most one).
    pub pinned_right: bool,
}

impl ColumnSpec {
    pub fn new(key: &'static str, name: impl Into<SharedString>, width: f32) -> Self {
        Self {
            key,
            name: name.into(),
            width: px(width),
            min_width: px(40.),
            sortable: false,
            resizable: true,
            right: false,
            pinned_right: false,
        }
    }
    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }
    pub fn fixed(mut self) -> Self {
        self.resizable = false;
        self
    }
    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }
    /// Pin to the right edge, always visible (implies a fixed width).
    pub fn pin_right(mut self) -> Self {
        self.pinned_right = true;
        self.resizable = false;
        self
    }
}

/// Splits off the right-pinned column. GPUI Kit `DataTable` only pins columns on the left,
/// so the pinned column isn't a table column: rows and the header reserve its width as
/// right padding and draw it there (see `Adapter::render_tr`).
fn split_pinned(columns: Vec<ColumnSpec>) -> (Vec<ColumnSpec>, Option<ColumnSpec>) {
    let mut pinned = None;
    let rest = columns
        .into_iter()
        .filter_map(|c| {
            if c.pinned_right && pinned.is_none() {
                pinned = Some(c);
                None
            } else {
                Some(c)
            }
        })
        .collect();
    (rest, pinned)
}

/// Supplied by the page: rendering and row text. `G` = group payload, `I` = item payload.
pub trait ListDelegate: 'static {
    type Group: Clone + 'static;
    type Item: Clone + 'static;

    fn columns(&self) -> Vec<ColumnSpec>;

    /// Renders one cell. `row_ix` lets cells build element ids. The `select` column is
    /// rendered by the table itself.
    fn render_cell(
        &self,
        row: &ListRow<Self::Group, Self::Item>,
        row_ix: usize,
        col: &ColumnSpec,
        cx: &mut App,
    ) -> AnyElement;

    /// Text quick find matches against (KBD-037), usually the name.
    fn row_text(&self, row: &ListRow<Self::Group, Self::Item>) -> String;

    /// The row context menu (KBD-036). Items dispatch [`OnRow`] actions.
    fn context_menu(&self, row: &ListRow<Self::Group, Self::Item>, menu: PopupMenu) -> PopupMenu {
        let _ = row;
        menu
    }

    /// Rendered when there are no rows.
    fn render_empty(&self, cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Inbox))
            .into_any_element()
    }

    /// Whether a row is busy (spinner instead of actions, CON-031).
    fn is_pending(&self, key: &str) -> bool {
        let _ = key;
        false
    }

    /// The visible row range changed (STA-006 stats for visible rows).
    fn visible_rows_changed(&mut self, range: Range<usize>) {
        let _ = range;
    }
}

/// Emitted to the owning page.
#[derive(Debug, Clone, PartialEq)]
pub enum ListEvent {
    /// `Enter` / double-click on an item row (CON-033).
    Open(SharedString),
    /// Sort changed by header click or the sort menu (CON-003).
    Sort(Option<SortState>),
    /// A group was expanded/collapsed (persist, CON-011).
    Expanded { group: SharedString, expanded: bool },
    /// Cursor or multi-selection changed.
    SelectionChanged,
    /// Column widths changed (persist in `UiState.column_widths`).
    ColumnWidths(Vec<f32>),
    /// The visible row range changed.
    VisibleRows(Range<usize>),
}

/// Adapter implementing GPUI Kit's `TableDelegate` on top of the model + page delegate.
pub struct Adapter<D: ListDelegate> {
    pub delegate: D,
    pub model: ListModel<D::Group, D::Item>,
    /// The table's (scrolling) columns.
    columns: Vec<ColumnSpec>,
    /// The right-pinned column, drawn in each row's right padding.
    pinned: Option<ColumnSpec>,
    loading: bool,
    /// Pending visible range to report (set during render, read by the owner).
    pending_visible: Option<Range<usize>>,
    /// Painted bounds of the cursor row (anchors the keyboard row menu, KBD-036).
    cursor_bounds: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<Pixels>>>>,
}

/// Bounds of the row ⋮ button that was just clicked: the row menu opens under it instead
/// of at the cursor row (one menu open at a time, so one slot per window is enough).
#[derive(Default)]
pub struct RowMenuTrigger(std::cell::Cell<Option<gpui_kit::Bounds<Pixels>>>);

impl gpui_kit::Global for RowMenuTrigger {}

impl RowMenuTrigger {
    /// Records the ⋮ button under the pointer; call from its click handler.
    pub fn set(bounds: gpui_kit::Bounds<Pixels>, cx: &mut App) {
        cx.default_global::<RowMenuTrigger>().0.set(Some(bounds));
    }

    fn take(cx: &mut App) -> Option<gpui_kit::Bounds<Pixels>> {
        cx.default_global::<RowMenuTrigger>().0.take()
    }
}

impl<D: ListDelegate> Adapter<D> {
    /// A row's selection checkbox. It updates the model directly: a dispatched action goes
    /// to the focused element, which isn't the table when the mouse is used without
    /// focusing it first.
    fn select_cell(
        &self,
        row_ix: usize,
        key: SharedString,
        selected: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let table = cx.entity().downgrade();
        h_flex()
            .size_full()
            .items_center()
            .child(
                Checkbox::new(("row-check", row_ix))
                    .debug_selector(move || format!("row-check-{row_ix}"))
                    .checked(selected)
                    .tab_stop(false)
                    .on_click(move |_, _, cx| {
                        // Don't let the click open the row too.
                        cx.stop_propagation();
                        table
                            .update(cx, |t, cx| {
                                t.delegate_mut().model.toggle_selected(&key);
                                cx.notify();
                            })
                            .ok();
                    }),
            )
            .into_any_element()
    }

    fn sort_of(&self, key: &str) -> Option<ColumnSort> {
        match &self.model.sort {
            Some(s) if s.key == key => Some(if s.descending {
                ColumnSort::Descending
            } else {
                ColumnSort::Ascending
            }),
            _ => Some(ColumnSort::Default),
        }
    }
}

impl<D: ListDelegate> TableDelegate for Adapter<D> {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.model.rows().len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let spec = &self.columns[col_ix];
        let mut c = Column::new(spec.key, spec.name.clone())
            .width(spec.width)
            .min_width(spec.min_width)
            .resizable(spec.resizable)
            .movable(false)
            .selectable(false);
        if spec.sortable {
            c = match self.sort_of(spec.key) {
                Some(sort) => c.sort(sort),
                None => c.sortable(),
            };
        }
        if spec.right {
            c = c.text_right();
        }
        c
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(spec) = self.columns.get(col_ix) else {
            return;
        };
        self.model.sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some(SortState {
                key: spec.key.into(),
                descending: false,
            }),
            ColumnSort::Descending => Some(SortState {
                key: spec.key.into(),
                descending: true,
            }),
        };
        cx.notify();
    }

    /// The `select` column's header is a select-all checkbox (SHL-005): checked when every
    /// visible item is selected. A click clears a non-empty selection, else selects every
    /// visible row (KBD-038).
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let spec = &self.columns[col_ix];
        if spec.key != SELECT_COLUMN {
            return div()
                .size_full()
                .child(spec.name.clone())
                .into_any_element();
        }
        let keys: Vec<SharedString> = self
            .model
            .rows()
            .iter()
            .flat_map(|r| r.item_keys())
            .collect();
        let all = !keys.is_empty() && keys.iter().all(|k| self.model.is_selected(k));
        let any = !self.model.selected().is_empty();
        let table = cx.entity().downgrade();
        Checkbox::new("select-all")
            .debug_selector(|| "select-all".into())
            .checked(all)
            .tooltip(s::CMD_SELECT_ALL)
            .tab_stop(false)
            .on_click(move |_, _, cx| {
                // Don't let the header click sort or select the column.
                cx.stop_propagation();
                table
                    .update(cx, |t, cx| {
                        let m = &mut t.delegate_mut().model;
                        if any {
                            m.clear_selection();
                        } else {
                            m.select_all();
                        }
                        // `ListTable` sees the change through its observer and reports it.
                        cx.notify();
                    })
                    .ok();
            })
            .into_any_element()
    }

    fn render_header(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let header = div().id("header");
        let Some(pinned) = self.pinned.as_ref() else {
            return header;
        };
        header.pr(pinned.width).relative().child(
            h_flex()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(pinned.width)
                .items_center()
                .table_cell_size(Size::Small)
                .border_b_1()
                .border_color(cx.theme().border)
                .child(pinned.name.clone()),
        )
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let key = self.model.row(row_ix).map(|r| r.key.clone());
        // The pinned cell (also reserves its width on the filler rows below the data).
        let pinned = self.pinned.as_ref().map(|col| {
            let cell = self
                .model
                .row(row_ix)
                .map(|row| self.delegate.render_cell(row, row_ix, col, cx));
            (col.width, cell)
        });
        div()
            .id(("row", row_ix))
            .cursor_pointer()
            .when_some(pinned, |this, (width, cell)| {
                this.pr(width).relative().when_some(cell, |this, cell| {
                    this.child(
                        h_flex()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(width)
                            .items_center()
                            .overflow_hidden()
                            .table_cell_size(Size::Small)
                            .child(cell),
                    )
                })
            })
            .when_some(key, |this, key| {
                // Runs before the GPUI Kit row handler on the same element, which moves the
                // cursor. A double click only counts once (groups would toggle back), and a
                // modified click only moves the cursor.
                this.on_click(move |e, window, cx| {
                    let m = e.modifiers();
                    if e.click_count() != 1 || m.secondary() || m.shift || m.alt {
                        return;
                    }
                    window.dispatch_action(
                        Box::new(OnRow {
                            row: key.clone(),
                            action: RowCommand::Open,
                        }),
                        cx,
                    )
                })
            })
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (Some(row), Some(col)) = (self.model.row(row_ix), self.columns.get(col_ix)) else {
            return div().into_any_element();
        };
        let selected = self.model.row_selected(row);
        if col.key == SELECT_COLUMN {
            return self.select_cell(row_ix, row.key.clone(), selected, cx);
        }
        let is_cursor = col_ix == 0 && self.model.cursor_ix() == Some(row_ix);
        let slot = self.cursor_bounds.clone();
        // Cells are rows of inline content, vertically centred (tags don't stretch).
        h_flex()
            .size_full()
            .items_center()
            .overflow_hidden()
            .when(is_cursor, move |this| {
                this.on_bounds(move |b, _, _| slot.set(Some(b)))
            })
            .child(self.delegate.render_cell(row, row_ix, col, cx))
            .into_any_element()
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        match self.model.row(row_ix) {
            Some(row) => self.delegate.context_menu(row, menu),
            None => menu,
        }
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.delegate.render_empty(cx)
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.model.visible = visible_range.clone();
        self.delegate.visible_rows_changed(visible_range.clone());
        self.pending_visible = Some(visible_range);
    }

    fn cell_text(&self, row_ix: usize, _col_ix: usize, _: &App) -> String {
        self.model
            .row(row_ix)
            .map(|r| self.delegate.row_text(r))
            .unwrap_or_default()
    }
}

/// The list view entity.
pub struct ListTable<D: ListDelegate> {
    table: Entity<TableState<Adapter<D>>>,
    /// Quick-find field (KBD-037), shown while open.
    find_input: Entity<InputState>,
    find_open: bool,
    /// Where focus goes on `Tab`/`Shift+Tab` out of the table (set by the page).
    next_focus: Option<FocusHandle>,
    prev_focus: Option<FocusHandle>,
    /// Persisted column widths key (`UiState.column_widths`).
    widths_key: Option<String>,
    /// The last sort reported to the page.
    reported_sort: Option<SortState>,
    /// The selection size last seen by the observer (header checkbox changes).
    reported_selected: usize,
    /// Keyboard-opened row menu (KBD-036): menu, anchor position, dismiss subscription.
    key_menu: Option<KeyMenu>,
    /// Painted bounds of the list (for anchoring the keyboard menu).
    bounds: gpui_kit::Bounds<Pixels>,
    _subs: Vec<Subscription>,
}

impl<D: ListDelegate> EventEmitter<ListEvent> for ListTable<D> {}

impl<D: ListDelegate> ListTable<D> {
    pub fn new(
        delegate: D,
        widths_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mut columns, pinned) = split_pinned(delegate.columns());
        if let Some(key) = widths_key.as_ref()
            && cx.try_global::<AppState>().is_some()
        {
            // Widths are saved for the table's columns (the pinned one isn't resizable).
            let mut saved = AppState::ui_state(cx).column_widths.get(key).cloned();
            // Widths saved before the column was pinned still include it as the last entry.
            if let Some(w) = saved.as_mut()
                && pinned.is_some()
                && w.len() == columns.len() + 1
            {
                w.pop();
            }
            if let Some(saved) = saved
                && saved.len() == columns.len()
            {
                for (c, w) in columns.iter_mut().zip(saved) {
                    if w.is_finite() && w > 10.0 {
                        c.width = px(w);
                    }
                }
            }
        }
        let adapter = Adapter {
            delegate,
            // Groups start expanded: every container is visible (CON-011).
            model: ListModel::new(true),
            columns,
            pinned,
            loading: false,
            pending_visible: None,
            cursor_bounds: Default::default(),
        };
        let table = cx.new(|cx| {
            TableState::new(adapter, window, cx)
                .row_selectable(true)
                .col_selectable(false)
                .col_movable(false)
                .loop_selection(false)
                .sortable(true)
        });
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder(s::QUICK_FIND));
        let subs = vec![
            cx.subscribe_in(&table, window, Self::on_table_event),
            cx.subscribe_in(&find_input, window, Self::on_find_event),
            // Header-click sorting happens inside the GPUI Kit table (perform_sort) without an
            // event; detect sort changes on notify and report them (CON-003).
            // The header select-all checkbox updates the model the same way.
            cx.observe(&table, |this, _, cx| {
                this.check_sort_changed(cx);
                this.check_selection_changed(cx);
            }),
        ];
        Self {
            table,
            find_input,
            find_open: false,
            next_focus: None,
            prev_focus: None,
            widths_key,
            reported_sort: None,
            reported_selected: 0,
            key_menu: None,
            bounds: gpui_kit::Bounds::default(),
            _subs: subs,
        }
    }

    pub fn table(&self) -> &Entity<TableState<Adapter<D>>> {
        &self.table
    }

    pub fn delegate<'a>(&self, cx: &'a App) -> &'a D {
        &self.table.read(cx).delegate().delegate
    }

    pub fn update_delegate<R>(&self, cx: &mut App, f: impl FnOnce(&mut D) -> R) -> R {
        self.table.update(cx, |t, cx| {
            let r = f(&mut t.delegate_mut().delegate);
            cx.notify();
            r
        })
    }

    pub fn model<'a>(&self, cx: &'a App) -> &'a ListModel<D::Group, D::Item> {
        &self.table.read(cx).delegate().model
    }

    /// Mutates the model and re-syncs the GPUI Kit cursor with the model cursor.
    pub fn update_model<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ListModel<D::Group, D::Item>) -> R,
    ) -> R {
        let r = self.table.update(cx, |t, _| f(&mut t.delegate_mut().model));
        self.sync_cursor(cx);
        cx.notify();
        r
    }

    /// Focus targets for leaving the table with Tab / Shift+Tab (KBD-004).
    pub fn set_tab_neighbours(&mut self, prev: Option<FocusHandle>, next: Option<FocusHandle>) {
        self.prev_focus = prev;
        self.next_focus = next;
    }

    /// Replaces the rows (cursor/selection preserved by key, SHL-004).
    pub fn set_nodes(&mut self, nodes: Vec<ListNode<D::Group, D::Item>>, cx: &mut Context<Self>) {
        self.update_model(cx, |m| m.set_nodes(nodes));
    }

    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            t.delegate_mut().loading = loading;
            cx.notify();
        });
    }

    /// Columns changed (e.g. CPU/Mem columns toggled).
    pub fn set_columns(&mut self, columns: Vec<ColumnSpec>, cx: &mut Context<Self>) {
        let (columns, pinned) = split_pinned(columns);
        self.table.update(cx, |t, cx| {
            t.delegate_mut().columns = columns;
            t.delegate_mut().pinned = pinned;
            t.refresh(cx);
            cx.notify();
        });
    }

    pub fn targets(&self, cx: &App) -> Vec<SharedString> {
        self.model(cx).targets()
    }

    pub fn cursor_row(&self, cx: &App) -> Option<ListRow<D::Group, D::Item>> {
        self.model(cx).cursor_row().cloned()
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.table.focus_handle(cx).contains_focused(window, cx)
            || self.find_input.focus_handle(cx).is_focused(window)
    }

    /// Model cursor → GPUI Kit row selection (scrolls into view).
    fn sync_cursor(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let ix = t.delegate().model.cursor_ix();
            match ix {
                Some(ix) if t.selected_row() != Some(ix) => t.set_selected_row(ix, cx),
                None if t.selected_row().is_some() => t.clear_selection(cx),
                _ => {}
            }
        });
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<Adapter<D>>>,
        event: &TableEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::SelectRow(ix) => {
                let ix = *ix;
                let changed = self.table.update(cx, |t, _| {
                    let m = &mut t.delegate_mut().model;
                    let before = m.cursor_ix();
                    m.set_cursor_ix(ix);
                    before != Some(ix)
                });
                if changed {
                    cx.emit(ListEvent::SelectionChanged);
                }
            }
            TableEvent::ColumnWidthsChanged(widths) => {
                let widths: Vec<f32> = widths.iter().map(|w| w.as_f32()).collect();
                if let Some(key) = self.widths_key.clone() {
                    let w = widths.clone();
                    AppState::update_ui_state(cx, move |s| {
                        s.column_widths.insert(key, w);
                    });
                }
                cx.emit(ListEvent::ColumnWidths(widths));
            }
            TableEvent::RightClickedRow(Some(ix)) => {
                let ix = *ix;
                self.table
                    .update(cx, |t, _| t.delegate_mut().model.set_cursor_ix(ix));
                self.sync_cursor(cx);
            }
            _ => {}
        }
        let visible = self
            .table
            .update(cx, |t, _| t.delegate_mut().pending_visible.take());
        if let Some(range) = visible {
            cx.emit(ListEvent::VisibleRows(range));
        }
    }

    fn check_selection_changed(&mut self, cx: &mut Context<Self>) {
        let n = self.model(cx).selected().len();
        if n != self.reported_selected {
            self.reported_selected = n;
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn check_sort_changed(&mut self, cx: &mut Context<Self>) {
        let sort = self.model(cx).sort.clone();
        if sort != self.reported_sort {
            self.reported_sort = sort.clone();
            cx.emit(ListEvent::Sort(sort));
        }
    }

    /// Sets the sort (sort menu, palette) and syncs the header arrows.
    pub fn set_sort(&mut self, sort: Option<SortState>, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            t.delegate_mut().model.sort = sort;
            t.refresh(cx);
            cx.notify();
        });
    }

    fn open_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.model(cx).row(ix).cloned() else {
            return;
        };
        if row.is_group() {
            self.toggle(&row.key, cx);
        } else {
            cx.emit(ListEvent::Open(row.key));
        }
    }

    fn toggle(&mut self, group: &SharedString, cx: &mut Context<Self>) {
        let changed = self.update_model(cx, |m| m.toggle_group(group));
        if changed {
            let expanded = self.model(cx).is_expanded(group);
            cx.emit(ListEvent::Expanded {
                group: group.clone(),
                expanded,
            });
        }
    }

    fn set_group_expanded(&mut self, group: &SharedString, expanded: bool, cx: &mut Context<Self>) {
        if self.update_model(cx, |m| m.set_expanded(group, expanded)) {
            cx.emit(ListEvent::Expanded {
                group: group.clone(),
                expanded,
            });
        }
    }

    // ── key actions ────────────────────────────────────────────────────────────────────

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }
    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, cx);
    }
    fn on_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page_size(cx);
        self.move_cursor(-(page as isize), cx);
    }
    fn on_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page_size(cx);
        self.move_cursor(page as isize, cx);
    }

    fn page_size(&self, cx: &App) -> usize {
        let v = &self.model(cx).visible;
        v.len().saturating_sub(1).max(1)
    }

    /// Moves the cursor by `delta`, clamped (no wrap: arrows stop at the ends).
    pub fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.model(cx).rows().len();
        if n == 0 {
            return;
        }
        let cur = self.model(cx).cursor_ix();
        let next = match cur {
            None => {
                if delta >= 0 {
                    0
                } else {
                    n - 1
                }
            }
            Some(c) => (c as isize + delta).clamp(0, n as isize - 1) as usize,
        };
        self.update_model(cx, |m| m.set_cursor_ix(next));
        cx.emit(ListEvent::SelectionChanged);
    }

    fn on_first(&mut self, _: &list::First, _: &mut Window, cx: &mut Context<Self>) {
        if !self.model(cx).rows().is_empty() {
            self.update_model(cx, |m| m.set_cursor_ix(0));
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn on_last(&mut self, _: &list::Last, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.model(cx).rows().len();
        if n > 0 {
            self.update_model(cx, |m| m.set_cursor_ix(n - 1));
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn on_open(&mut self, _: &list::OpenDetail, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.model(cx).cursor_ix() {
            self.open_row(ix, cx);
        }
    }

    fn on_toggle_group(&mut self, _: &list::ToggleGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.model(cx).cursor_row().cloned()
            && row.is_group()
        {
            self.toggle(&row.key, cx);
        }
    }

    fn on_collapse(&mut self, _: &list::CollapseOrParent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.model(cx).cursor_row().cloned() else {
            return;
        };
        match &row.kind {
            RowKind::Group { expanded: true, .. } => self.set_group_expanded(&row.key, false, cx),
            RowKind::Item(_) if row.parent.is_some() => {
                self.update_model(cx, |m| m.cursor_to_parent());
                cx.emit(ListEvent::SelectionChanged);
            }
            _ => {}
        }
    }

    fn on_expand(&mut self, _: &list::Expand, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.model(cx).cursor_row().cloned()
            && let RowKind::Group {
                expanded: false, ..
            } = row.kind
        {
            self.set_group_expanded(&row.key, true, cx);
        }
    }

    fn set_all(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let groups: Vec<SharedString> = self
            .model(cx)
            .nodes()
            .iter()
            .filter(|n| matches!(n, ListNode::Group { .. }))
            .map(|n| n.key().clone())
            .collect();
        self.update_model(cx, |m| m.set_all_expanded(expanded));
        for g in groups {
            cx.emit(ListEvent::Expanded { group: g, expanded });
        }
    }

    fn on_collapse_all(&mut self, _: &list::CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all(false, cx);
    }

    fn on_expand_all(&mut self, _: &list::ExpandAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all(true, cx);
    }

    fn on_toggle_selected(
        &mut self,
        _: &list::ToggleRowSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.model(cx).cursor().cloned() {
            self.update_model(cx, |m| m.toggle_selected(&key));
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn on_extend_up(&mut self, _: &list::ExtendUp, _: &mut Window, cx: &mut Context<Self>) {
        self.update_model(cx, |m| m.extend_selection(false));
        cx.emit(ListEvent::SelectionChanged);
    }

    fn on_extend_down(&mut self, _: &list::ExtendDown, _: &mut Window, cx: &mut Context<Self>) {
        self.update_model(cx, |m| m.extend_selection(true));
        cx.emit(ListEvent::SelectionChanged);
    }

    fn on_select_all(&mut self, _: &list::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.update_model(cx, |m| m.select_all());
        cx.emit(ListEvent::SelectionChanged);
    }

    /// `Esc` in the table: clear the multi-selection, else let it bubble (KBD-006).
    fn on_escape(&mut self, _: &list::Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.update_model(cx, |m| m.clear_selection()) {
            cx.emit(ListEvent::SelectionChanged);
        } else {
            cx.propagate();
        }
    }

    fn on_clear_selection(
        &mut self,
        _: &list::ClearSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.update_model(cx, |m| m.clear_selection()) {
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn on_focus_next(&mut self, _: &list::FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        match self.next_focus.clone() {
            Some(h) => window.focus(&h, cx),
            None => window.focus_next(cx),
        }
    }

    fn on_focus_prev(&mut self, _: &list::FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        match self.prev_focus.clone() {
            Some(h) => window.focus(&h, cx),
            None => window.focus_prev(cx),
        }
    }

    /// Dispatches row-scoped commands from cell buttons/menus (`OnRow`). Moves the cursor
    /// to the row first so the page's normal command handlers apply to it.
    pub fn focus_row(&mut self, key: &str, cx: &mut Context<Self>) {
        let key: SharedString = key.to_owned().into();
        self.update_model(cx, |m| m.set_cursor(Some(key)));
    }

    fn on_row(&mut self, action: &OnRow, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_row(&action.row, cx);
        match action.action {
            RowCommand::Open => {
                if let Some(ix) = self.model(cx).cursor_ix() {
                    self.open_row(ix, cx);
                }
            }
            RowCommand::ToggleGroup => {
                let key = action.row.clone();
                self.toggle(&key, cx);
            }
            RowCommand::ToggleSelected => {
                let key = action.row.clone();
                self.update_model(cx, |m| m.toggle_selected(&key));
                cx.emit(ListEvent::SelectionChanged);
            }
            // Everything else is a page command: let it bubble to the page.
            _ => cx.propagate(),
        }
    }

    // ── quick find (KBD-037) ───────────────────────────────────────────────────────────

    fn on_quick_find(&mut self, _: &list::QuickFind, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.find_input.update(cx, |i, cx| {
            i.set_value("", window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    fn on_find_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = event {
            self.find_jump(false, cx);
        }
    }

    fn find_jump(&mut self, next: bool, cx: &mut Context<Self>) {
        let query = self.find_input.read(cx).value().to_string();
        let start = match (self.model(cx).cursor_ix(), next) {
            (Some(c), true) => c + 1,
            (Some(c), false) => c,
            (None, _) => 0,
        };
        let found = {
            let t = self.table.read(cx);
            let a = t.delegate();
            a.model.find(&query, start, |r| a.delegate.row_text(r))
        };
        if let Some(ix) = found {
            self.update_model(cx, |m| m.set_cursor_ix(ix));
            cx.emit(ListEvent::SelectionChanged);
        }
    }

    fn on_find_next(&mut self, _: &list::QuickFindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.find_jump(true, cx);
    }

    fn on_find_close(
        &mut self,
        _: &list::QuickFindClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_find(window, cx);
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        let h = self.table.focus_handle(cx);
        window.focus(&h, cx);
        cx.notify();
    }

    pub fn find_open(&self) -> bool {
        self.find_open
    }

    /// `Shift+F10` / `Menu` (KBD-036): opens the row menu for the cursor row, anchored at
    /// the row, focused, and navigable with arrows/Enter/Esc. Esc returns focus to the table
    /// (GPUI Kit `PopupMenu` restores `action_context`). Mouse right-click keeps using the
    /// GPUI Kit table context menu (same delegate builder).
    fn on_context_menu(
        &mut self,
        _: &list::ContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row_ix) = self.model(cx).cursor_ix() else {
            return;
        };
        let Some(row) = self.model(cx).row(row_ix).cloned() else {
            return;
        };
        let table_focus = self.table.focus_handle(cx);
        let table = self.table.clone();
        // Under the row's ⋮ button when it was clicked; from the keyboard, below the cursor
        // row's first cell, else the table top.
        let pos = if let Some(b) = RowMenuTrigger::take(cx) {
            MenuAnchor::Below(b)
        } else {
            match self.table.read(cx).delegate().cursor_bounds.get() {
                Some(b) => gpui_kit::point(b.origin.x + px(48.), b.origin.y + b.size.height).into(),
                None => gpui_kit::point(
                    self.bounds.origin.x + px(56.),
                    self.bounds.origin.y + px(40.),
                )
                .into(),
            }
        };
        self.key_menu = Some(KeyMenu::open(
            pos,
            table_focus,
            window,
            cx,
            move |menu, _, cx| table.read(cx).delegate().delegate.context_menu(&row, menu),
            |this: &mut Self, _| this.key_menu = None,
        ));
        cx.notify();
    }

    pub fn key_menu_open(&self) -> bool {
        self.key_menu.is_some()
    }
}

impl<D: ListDelegate> Focusable for ListTable<D> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl<D: ListDelegate> Render for ListTable<D> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Report visible-row changes that happened during the last table render.
        let visible = self
            .table
            .update(cx, |t, _| t.delegate_mut().pending_visible.take());
        if let Some(range) = visible {
            cx.emit(ListEvent::VisibleRows(range));
        }
        div()
            .id("list-table")
            .key_context(ctx::LIST_TABLE)
            .size_full()
            .relative()
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_first))
            .on_action(cx.listener(Self::on_last))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_toggle_group))
            .on_action(cx.listener(Self::on_collapse))
            .on_action(cx.listener(Self::on_expand))
            .on_action(cx.listener(Self::on_collapse_all))
            .on_action(cx.listener(Self::on_expand_all))
            .on_action(cx.listener(Self::on_toggle_selected))
            .on_action(cx.listener(Self::on_extend_up))
            .on_action(cx.listener(Self::on_extend_down))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_clear_selection))
            .on_action(cx.listener(Self::on_focus_next))
            .on_action(cx.listener(Self::on_focus_prev))
            .on_action(cx.listener(Self::on_context_menu))
            .on_action(cx.listener(Self::on_row))
            .on_action(cx.listener(Self::on_quick_find))
            .child(
                DataTable::new(&self.table)
                    .with_size(Size::Small)
                    .bordered(false)
                    .stripe(false),
            )
            .when(self.find_open, |this| {
                this.child(
                    h_flex()
                        .id("quick-find")
                        .key_context(ctx::QUICK_FIND)
                        .on_action(cx.listener(Self::on_find_next))
                        .on_action(cx.listener(Self::on_find_close))
                        .absolute()
                        .top_1()
                        .right_4()
                        .w(px(260.))
                        .p_1()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow_md()
                        .child(
                            Input::new(&self.find_input)
                                .small()
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                )
            })
            .on_bounds({
                let this = cx.entity().downgrade();
                move |bounds, _, cx| {
                    this.update(cx, |t, _| t.bounds = bounds).ok();
                }
            })
            .when_some(self.key_menu.as_ref(), |this, m| this.child(m.render()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn con_002_actions_column_is_pinned_out_of_the_table_columns() {
        let (cols, pinned) = split_pinned(vec![
            ColumnSpec::new("name", "Name", 200.),
            ColumnSpec::new("actions", "Actions", 80.).pin_right(),
            ColumnSpec::new("created", "Created", 100.),
        ]);
        let pinned = pinned.expect("actions pinned");
        assert_eq!(pinned.key, "actions");
        assert!(!pinned.resizable);
        let keys: Vec<_> = cols.iter().map(|c| c.key).collect();
        assert_eq!(keys, ["name", "created"]);
    }
}
