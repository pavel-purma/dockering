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
//! - id-stable refresh and focus-to-next-row after deletes (KBD-007).
//!
//! Pages implement [`ListDelegate`] (cells, columns, menus) and handle the [`ListEvent`]s.
//! Page-specific single letters (`S`, `R`, …) are plain actions bound in the `ListTable`
//! context; the page's element handles them and reads [`ListTable::targets`].

mod model;

pub use model::{ListModel, ListNode, ListRow, RowKind, SortState};

use std::ops::Range;

use gpui_kit::base::ElementExt as _;
use gpui_kit::base::actions::{SelectDown, SelectPageDown, SelectPageUp, SelectUp};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Size, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Pixels, Render, SharedString, Subscription, Window, div, px,
};

use crate::actions::{OnRow, RowCommand, list};
use crate::keymap::ctx;
use crate::state::AppState;
use crate::strings as s;
use crate::ui::menu::KeyMenu;

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
}

/// Supplied by the page: rendering and row text. `G` = group payload, `I` = item payload.
pub trait ListDelegate: 'static {
    type Group: Clone + 'static;
    type Item: Clone + 'static;

    fn columns(&self) -> Vec<ColumnSpec>;

    /// Renders one cell. `row_ix` lets cells build element ids.
    fn render_cell(
        &self,
        row: &ListRow<Self::Group, Self::Item>,
        row_ix: usize,
        col: &ColumnSpec,
        selected: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;

    /// Text quick find matches against (KBD-037), usually the name.
    fn row_text(&self, row: &ListRow<Self::Group, Self::Item>) -> String;

    /// The row context menu (KBD-036). Items dispatch [`OnRow`] actions.
    fn context_menu(
        &self,
        row: &ListRow<Self::Group, Self::Item>,
        menu: PopupMenu,
        window: &Window,
        cx: &App,
    ) -> PopupMenu {
        let _ = (row, window, cx);
        menu
    }

    /// Rendered when there are no rows.
    fn render_empty(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let _ = window;
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
    Sort(SortState),
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
    columns: Vec<ColumnSpec>,
    loading: bool,
    /// Pending visible range to report (set during render, read by the owner).
    pending_visible: Option<Range<usize>>,
}

impl<D: ListDelegate> Adapter<D> {
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

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("row", row_ix))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (Some(row), Some(col)) = (self.model.row(row_ix), self.columns.get(col_ix)) else {
            return div().into_any_element();
        };
        let selected = self.model.row_selected(row);
        self.delegate
            .render_cell(row, row_ix, col, selected, window, cx)
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        match self.model.row(row_ix) {
            Some(row) => self.delegate.context_menu(row, menu, window, cx),
            None => menu,
        }
    }

    fn render_empty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.delegate.render_empty(window, cx)
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
        let mut columns = delegate.columns();
        if let Some(key) = widths_key.as_ref()
            && cx.try_global::<AppState>().is_some()
        {
            let saved = AppState::ui_state(cx).column_widths.get(key).cloned();
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
            model: ListModel::new(false),
            columns,
            loading: false,
            pending_visible: None,
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
        ];
        Self {
            table,
            find_input,
            find_open: false,
            next_focus: None,
            prev_focus: None,
            widths_key,
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
        self.table.update(cx, |t, cx| {
            t.delegate_mut().columns = columns;
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
        window: &mut Window,
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
            TableEvent::DoubleClickedRow(ix) => {
                self.open_row(*ix, window, cx);
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
        // Sort changes happen inside the delegate; report them.
        let sort = self.model(cx).sort.clone();
        if let Some(sort) = sort
            && matches!(event, TableEvent::SelectColumn(_))
        {
            cx.emit(ListEvent::Sort(sort));
        }
        let visible = self
            .table
            .update(cx, |t, _| t.delegate_mut().pending_visible.take());
        if let Some(range) = visible {
            cx.emit(ListEvent::VisibleRows(range));
        }
    }

    fn open_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
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

    fn on_open(&mut self, _: &list::OpenDetail, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.model(cx).cursor_ix() {
            self.open_row(ix, window, cx);
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

    fn on_row(&mut self, action: &OnRow, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_row(&action.row, cx);
        match action.action {
            RowCommand::Open => {
                if let Some(ix) = self.model(cx).cursor_ix() {
                    self.open_row(ix, window, cx);
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
        // Position: left edge of the table, at the cursor row (36 px rows + header).
        let row_h = Size::Small.table_row_height();
        let first = self.model(cx).visible.start;
        let y = self.bounds.origin.y + row_h * (row_ix.saturating_sub(first) as f32 + 1.5);
        let pos = gpui_kit::point(self.bounds.origin.x + px(56.), y);
        self.key_menu = Some(KeyMenu::open(
            pos,
            table_focus,
            window,
            cx,
            move |menu, window, cx| {
                let t = table.read(cx);
                t.delegate().delegate.context_menu(&row, menu, window, cx)
            },
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_prepaint({
                let this = cx.entity().downgrade();
                move |bounds, _, cx| {
                    this.update(cx, |t, _| t.bounds = bounds).ok();
                }
            })
            .when_some(self.key_menu.as_ref(), |this, m| this.child(m.render()))
    }
}
