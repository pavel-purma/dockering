//! Shared placeholder page for phase-2 routes: title, breadcrumb, a short note, and a
//! focusable body so navigation focus (KBD-007) and region cycling work today.

use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, IntoElement, Render, SharedString, Window, div,
};

use crate::ui::breadcrumb::{Crumb, breadcrumb};
use crate::ui::page::PageView;

pub struct PlaceholderPage {
    title: SharedString,
    crumbs: Vec<(SharedString, Option<crate::nav::Route>)>,
    body: SharedString,
    focus: FocusHandle,
}

impl PlaceholderPage {
    pub fn new(
        title: impl Into<SharedString>,
        crumbs: Vec<(SharedString, Option<crate::nav::Route>)>,
        body: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            title: title.into(),
            crumbs,
            body: body.into(),
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}

impl Focusable for PlaceholderPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PageView for PlaceholderPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PlaceholderPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("placeholder-page")
            .size_full()
            .p_4()
            .gap_3()
            .when(!self.crumbs.is_empty(), |this| {
                this.child(breadcrumb(
                    self.crumbs
                        .iter()
                        .map(|(l, r)| match r {
                            Some(r) => Crumb::link(l.clone(), r.clone()),
                            None => Crumb::here(l.clone()),
                        })
                        .collect(),
                ))
            })
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id("placeholder-body")
                    .track_focus(&self.focus)
                    .p_3()
                    .rounded(cx.theme().radius)
                    .text_color(cx.theme().muted_foreground)
                    .child(self.body.clone()),
            )
    }
}
