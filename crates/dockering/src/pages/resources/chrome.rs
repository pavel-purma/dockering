//! Shared page chrome for the list pages (spec 30 §1 "Page header"): the header layout
//! (title + view controls, search + selection actions), a focusable segmented filter
//! (KBD-039), the overflow trigger, the selection actions (SHL-005), the four list states
//! (SHL-004), and menu items that show their key hint (KBD-036). Pages compose these; the
//! logic stays in the page.

use dk_core::EngineError;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, AnyElement, App, Entity, FocusHandle, IntoElement, SharedString, Window, div, px,
};

use crate::actions::{SetFilter, SortByColumn, list};
use crate::strings as s;
use crate::ui::dispatch;
use crate::ui::list_table::{ColumnSpec, RowMenuTrigger, SortState};
use crate::ui::menu::TrackBounds as _;
use crate::ui::segmented::{Segment, Segmented};
use crate::ui::widgets::focus_wrap;

/// The search field (SHL-006).
pub fn search_box(state: &Entity<InputState>) -> AnyElement {
    div()
        .w(px(240.))
        .child(
            Input::new(state)
                .small()
                .cleanable(true)
                .prefix(Icon::new(IconName::Search).small()),
        )
        .into_any_element()
}

/// A segmented filter control (`All / In use / Unused …`) as one Tab stop: arrows move the
/// selection, clicks and keys dispatch [`SetFilter`] (KBD-039, KBD-002).
pub fn filter_segment(
    id: &'static str,
    focus: &FocusHandle,
    options: &'static [(&'static str, &'static str)],
    current: &'static str,
    cx: &App,
) -> AnyElement {
    let origin = focus.clone();
    div()
        .id(id)
        .track_focus(focus)
        .rounded(cx.theme().radius)
        .on_key_down(move |e: &gpui_kit::KeyDownEvent, window, cx| {
            let ix = options.iter().position(|(v, _)| *v == current).unwrap_or(0);
            let next = match e.keystroke.key.as_str() {
                "left" => Some(ix.saturating_sub(1)),
                "right" => Some((ix + 1).min(options.len().saturating_sub(1))),
                "home" => Some(0),
                "end" => Some(options.len().saturating_sub(1)),
                _ => None,
            };
            if let Some(n) = next
                && !e.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                window.dispatch_action(
                    Box::new(SetFilter {
                        filter: options[n].0.into(),
                    }),
                    cx,
                );
            }
        })
        .child(
            Segmented::new(SharedString::from(format!("{id}-group")))
                .small()
                .selected(options.iter().position(|(v, _)| *v == current).unwrap_or(0))
                .on_select({
                    move |ix, window, cx| {
                        // Segments swallow the mouse-down; focus the control so arrows
                        // work next (CON-004), and dispatch from it (KBD-002).
                        window.focus(&origin, cx);
                        let set = SetFilter {
                            filter: options[*ix].0.into(),
                        };
                        dispatch::dispatch_from(&origin, &set, window, cx)
                    }
                })
                .segments(
                    options.iter().map(|(value, label)| {
                        Segment::new(*label).selector(format!("{id}-{value}"))
                    }),
                ),
        )
        .into_any_element()
}

/// A labelled header button the page owns the focus of (so menus can restore to it).
pub fn header_button(
    id: &'static str,
    focus: &FocusHandle,
    button: Button,
    action: Box<dyn Action>,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let on_key = action.boxed_clone();
    focus_wrap(
        id,
        focus,
        button.on_click(dispatch::on_click(focus, action)),
        move |_, window, cx| window.dispatch_action(on_key.boxed_clone(), cx),
        cx,
    )
}

/// The `⋮` overflow trigger. `on_activate` opens the page's overflow menu.
pub fn overflow_trigger(
    id: &'static str,
    focus: &FocusHandle,
    on_activate: impl Fn(&mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let click = on_activate.clone();
    focus_wrap(
        id,
        focus,
        Button::new(SharedString::from(format!("{id}-button")))
            .small()
            .ghost()
            .icon(IconName::EllipsisVertical)
            .tooltip(s::MORE_ACTIONS)
            .on_click(move |_, window, cx| click(window, cx)),
        move |_, window, cx| on_activate(window, cx),
        cx,
    )
}

/// The page header (spec 30 §1). Top row: the title (and a refresh spinner) on the left,
/// the view controls (filters, page buttons) on the right. Second row: the search on the
/// left; on the right the muted summary (`12 images · 1.2 GB`), replaced by the selection
/// actions while any row is checked (SHL-005), then the page `⋮` overflow.
#[allow(clippy::too_many_arguments)]
pub fn page_header(
    id: &'static str,
    title: &'static str,
    loading: bool,
    controls: impl IntoIterator<Item = AnyElement>,
    search: &Entity<InputState>,
    summary: impl Into<SharedString>,
    selection: Option<AnyElement>,
    overflow: AnyElement,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let summary = selection.unwrap_or_else(|| {
        div()
            .debug_selector(|| "page-summary".into())
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(summary.into())
            .into_any_element()
    });
    v_flex()
        .id(id)
        .debug_selector(|| "page-header".into())
        .key_context(crate::keymap::ctx::TOOLBAR)
        .gap_2()
        .px_4()
        .pt_3()
        .pb_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .when(loading, |this| this.child(Spinner::new().small()))
                .child(div().flex_1())
                .children(controls),
        )
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(search_box(search))
                .child(div().flex_1())
                .child(summary)
                .child(overflow),
        )
}

/// SHL-005: the selection actions, shown while any row is checked. `leading` are page
/// specific buttons placed before Delete; `delete_disabled` gates the delete action.
pub fn selection_actions(
    selected: usize,
    leading: Vec<Button>,
    delete_disabled: bool,
    origin: &FocusHandle,
    cx: &App,
) -> Option<AnyElement> {
    if selected == 0 {
        return None;
    }
    Some(
        h_flex()
            .id("bulk-bar")
            .debug_selector(|| "selection-actions".into())
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::selected_count(selected)),
            )
            .children(leading)
            .child(
                Button::new("bulk-delete")
                    .small()
                    .danger()
                    .label(s::ACTION_DELETE)
                    .disabled(delete_disabled)
                    .tooltip_with_action(s::CMD_BULK_DELETE, &list::BulkDelete, None)
                    .on_click(dispatch::on_click(origin, Box::new(list::BulkDelete))),
            )
            .child(
                Button::new("bulk-clear")
                    .small()
                    .ghost()
                    .label(s::CLEAR_SELECTION)
                    .on_click(dispatch::on_click(origin, Box::new(list::ClearSelection))),
            )
            .into_any_element(),
    )
}

/// The list body in its loading / error / data states (empty is the table's own state).
pub fn list_body(
    first_load: bool,
    has_data: bool,
    error: Option<&EngineError>,
    error_id: &'static str,
    table: AnyElement,
    cx: &App,
) -> AnyElement {
    if first_load && !has_data {
        return crate::ui::skeleton_rows(8, 6);
    }
    if let (Some(err), false) = (error, has_data) {
        return crate::ui::error_panel(
            error_id,
            s::LIST_LOAD_FAILED,
            err,
            Box::new(crate::actions::Refresh),
            cx,
        );
    }
    div().size_full().child(table).into_any_element()
}

/// The `Mod+Shift+O` sort menu over the sortable columns.
pub fn sort_menu(menu: PopupMenu, columns: Vec<ColumnSpec>, sort: Option<SortState>) -> PopupMenu {
    let mut menu = menu;
    for c in columns.into_iter().filter(|c| c.sortable) {
        let arrow = match &sort {
            Some(st) if st.key == c.key => {
                if st.descending {
                    " ↓"
                } else {
                    " ↑"
                }
            }
            _ => "",
        };
        menu = menu.menu(
            format!("{}{arrow}", c.name),
            Box::new(SortByColumn { key: c.key.into() }),
        );
    }
    menu
}

/// Cycles the sort for `key`: ascending → descending → none.
pub fn next_sort(current: Option<&SortState>, key: SharedString) -> Option<SortState> {
    match current {
        Some(s) if s.key == key && !s.descending => Some(SortState {
            key,
            descending: true,
        }),
        Some(s) if s.key == key => None,
        _ => Some(SortState {
            key,
            descending: false,
        }),
    }
}

/// A menu item that dispatches `action` and shows the binding of `hint` in `context`
/// (KBD-010/036: shortcuts shown in menus).
pub fn hinted(
    menu: PopupMenu,
    label: &'static str,
    action: Box<dyn Action>,
    hint: &dyn Action,
    context: &str,
    disabled: bool,
) -> PopupMenu {
    let keys = crate::keymap::hint_for(hint.name(), context);
    menu.menu_element_with_disabled(action, disabled, move |_, cx| {
        h_flex()
            .w_full()
            .gap_4()
            .justify_between()
            .child(label)
            .when_some(keys.clone(), |this, k| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(k),
                )
            })
    })
}

/// The row ⋮ button (`OnRow` → `ContextMenu`). The row menu it opens is anchored under it
/// ([`RowMenuTrigger`]); `Button` can't report its bounds, so a wrapper measures it.
pub fn row_menu_button(
    id: impl Into<gpui_kit::ElementId>,
    action: Box<dyn Action>,
    cx: &App,
) -> AnyElement {
    let id = id.into();
    let anchor = dispatch::DispatchAnchor::new(cx);
    let origin = anchor.handle().clone();
    let selector = format!("row-{id}");
    let bounds = std::rc::Rc::new(std::cell::Cell::new(gpui_kit::Bounds::default()));
    let slot = bounds.clone();
    div()
        .relative()
        .child(anchor.element())
        .child(
            Button::new(id)
                .debug_selector(move || selector)
                .ghost()
                .xsmall()
                .icon(IconName::EllipsisVertical)
                .tooltip(s::MORE_ACTIONS)
                .tab_stop(false)
                .on_click(move |_, window, cx| {
                    // Don't let the click open the row too.
                    cx.stop_propagation();
                    RowMenuTrigger::set(bounds.get(), cx);
                    dispatch::dispatch_from(&origin, action.as_ref(), window, cx)
                }),
        )
        .on_bounds(move |b, _, _| slot.set(b))
        .into_any_element()
}

/// The primary text of a row (its name). The whole row opens the detail (CON-033), so
/// the name is plain text rather than a link.
pub fn name_cell(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_sm()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .truncate()
        .child(text.into())
        .into_any_element()
}

/// A muted "—" cell.
pub fn dash(cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(s::DASH)
        .into_any_element()
}

/// A small text cell.
pub fn text_cell(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_xs()
        .truncate()
        .child(text.into())
        .into_any_element()
}

/// A mono text cell (ids, subnets).
pub fn mono_cell(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .truncate()
        .font_family(cx.theme().mono_font_family.clone())
        .child(text.into())
        .into_any_element()
}

/// A skeleton placeholder cell (VOL-002 lazy sizes).
pub fn skeleton_cell(width: f32) -> AnyElement {
    gpui_kit::component::skeleton::Skeleton::new()
        .h(px(12.))
        .w(px(width))
        .into_any_element()
}

/// Keeps `Disableable`/`Selectable` imports honest across cfgs.
#[allow(dead_code)]
fn _traits(b: Button) -> Button {
    b.disabled(false).selected(false)
}
