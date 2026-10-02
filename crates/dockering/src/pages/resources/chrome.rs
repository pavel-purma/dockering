//! Shared page chrome for the Images, Volumes and Networks lists (spec 30 §1 "Page header"):
//! title row, search box, a focusable segmented filter (KBD-039), the overflow trigger, the
//! bulk bar (SHL-005), the four list states (SHL-004), and menu items that show their key
//! hint (KBD-036). Pages compose these; the logic stays in the page.

use dk_core::EngineError;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, AnyElement, App, Entity, FocusHandle, IntoElement, SharedString, Window, div, px,
};

use crate::actions::{SetFilter, SortByColumn, list};
use crate::strings as s;
use crate::ui::list_table::{ColumnSpec, SortState};
use crate::ui::widgets::{focus_ring, focus_wrap};

/// The page title with a muted summary (`12 images · 1.2 GB`) and a refresh spinner.
pub fn title_row(
    title: &'static str,
    summary: impl Into<SharedString>,
    loading: bool,
    cx: &App,
) -> gpui_kit::Div {
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(summary.into()),
        )
        .when(loading, |this| this.child(Spinner::new().small()))
}

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
    window: &Window,
    cx: &App,
) -> AnyElement {
    let focused = focus.is_focused(window);
    div()
        .id(id)
        .track_focus(focus)
        .rounded(cx.theme().radius)
        .map(|el| focus_ring(el, focused, cx))
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
            ButtonGroup::new(SharedString::from(format!("{id}-group")))
                .small()
                .children(options.iter().map(|(value, label)| {
                    let value = *value;
                    Button::new(SharedString::from(format!("{id}-{value}")))
                        .label(*label)
                        .selected(value == current)
                        .tab_stop(false)
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(
                                Box::new(SetFilter {
                                    filter: value.into(),
                                }),
                                cx,
                            )
                        })
                })),
        )
        .into_any_element()
}

/// A labelled header button the page owns the focus of (so menus can restore to it).
pub fn header_button(
    id: &'static str,
    focus: &FocusHandle,
    button: Button,
    action: Box<dyn Action>,
    window: &Window,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let on_key = action.boxed_clone();
    focus_wrap(
        id,
        focus,
        button.on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx)),
        move |_, window, cx| window.dispatch_action(on_key.boxed_clone(), cx),
        window,
        cx,
    )
}

/// The `⋮` overflow trigger. `on_activate` opens the page's overflow menu.
pub fn overflow_trigger(
    id: &'static str,
    focus: &FocusHandle,
    on_activate: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
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
        window,
        cx,
    )
}

/// SHL-005: shown when ≥ 2 rows are selected. `delete_disabled` gates the action.
pub fn bulk_bar(selected: usize, delete_disabled: bool, cx: &App) -> Option<AnyElement> {
    if selected < 2 {
        return None;
    }
    Some(
        h_flex()
            .id("bulk-bar")
            .mx_4()
            .mb_2()
            .px_3()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().accent)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(s::selected_count(selected)),
            )
            .child(div().flex_1())
            .child(
                Button::new("bulk-delete")
                    .small()
                    .danger()
                    .label(s::ACTION_DELETE)
                    .disabled(delete_disabled)
                    .tooltip_with_action(s::CMD_BULK_DELETE, &list::BulkDelete, None)
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(list::BulkDelete), cx)),
            )
            .child(
                Button::new("bulk-clear")
                    .small()
                    .ghost()
                    .label(s::CLEAR_SELECTION)
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(list::ClearSelection), cx)),
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

/// A small icon button for row cells (not a Tab stop: the table is one stop, KBD-004).
pub fn row_button(
    id: impl Into<gpui_kit::ElementId>,
    icon: impl Into<Icon>,
    tooltip: &'static str,
    disabled: bool,
    action: Box<dyn Action>,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon.into())
        .tooltip(tooltip)
        .disabled(disabled)
        .tab_stop(false)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
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
