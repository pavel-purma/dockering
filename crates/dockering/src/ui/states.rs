//! Empty / error / loading states (every data view renders all four, SHL-004).

use dk_core::EngineError;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{ActiveTheme, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Action, AnyElement, App, ElementId, IntoElement, SharedString, div, px};

use crate::strings as s;

/// Empty state with an icon, title, description, and an optional CTA that dispatches
/// `action` (CON-032).
pub fn empty_state(
    icon: impl Into<Icon>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cta: Option<(SharedString, Box<dyn Action>)>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .id("empty-state")
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(icon.into().size(px(40.))),
        )
        .child(div().text_lg().font_semibold().child(title.into()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(description.into()),
        )
        .when_some(cta, |this, (label, action)| {
            this.child(
                Button::new("empty-cta")
                    .primary()
                    .label(label)
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    }),
            )
        })
        .into_any_element()
}

/// Inline error panel with *Retry* (SHL-003 list loads). `retry` is dispatched on click.
pub fn error_panel(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    error: &EngineError,
    retry: Box<dyn Action>,
    cx: &App,
) -> AnyElement {
    let hint = error.hint().map(|h| SharedString::from(h.to_owned()));
    v_flex()
        .id(id)
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .child(
            div()
                .text_color(cx.theme().danger)
                .child(Icon::new(IconName::CircleAlert).size(px(36.))),
        )
        .child(div().text_lg().font_semibold().child(title.into()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .max_w(px(560.))
                .child(error.to_string()),
        )
        .when_some(hint, |this, hint| {
            this.child(div().text_sm().max_w(px(560.)).child(hint))
        })
        .child(
            Button::new("retry")
                .label(s::RETRY)
                .icon(IconName::RefreshCw)
                .on_click(move |_, window, cx| window.dispatch_action(retry.boxed_clone(), cx)),
        )
        .into_any_element()
}

/// Skeleton rows for a first load (spec 30 §1 "Loading").
pub fn skeleton_rows(rows: usize, cols: usize) -> AnyElement {
    v_flex()
        .id("skeleton-rows")
        .w_full()
        .gap_3()
        .p_3()
        .children((0..rows).map(|r| {
            h_flex().gap_4().children((0..cols).map(move |c| {
                let w = match (r + c) % 3 {
                    0 => px(160.),
                    1 => px(110.),
                    _ => px(70.),
                };
                Skeleton::new().h(px(14.)).w(w)
            }))
        }))
        .into_any_element()
}
