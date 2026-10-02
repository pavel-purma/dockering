//! TODO(phase-2): Container detail (`docs/spec/features/container-detail.md`, CDT-*; logs,
//! terminal, stats specs). Minimal header for now: breadcrumb, name, status chip, back.
//!
//! Phase 2 adds `ContainerDetailState` (inspect + subscriptions, spec 10 §3.3 ownership),
//! the `TabBar` (tab is part of the route; `Ctrl+Tab` = `detail::NextTab`), the
//! `DetailHeader` key context (single letters S/R/P/T/L/I/C/Del, KBD-041) and the removed
//! banner (CDT-080). Header actions should reuse `pages::containers::ops`.

use dk_core::ContainerSummary;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription,
    Window, div,
};

use crate::actions::{Back, detail};
use crate::keymap::ctx;
use crate::nav::{ContainerTab, Route};
use crate::state::EngineStore;
use crate::strings as s;
use crate::ui::breadcrumb::{Crumb, breadcrumb};
use crate::ui::page::PageView;
use crate::ui::status_chip::{container_chip, health_chip};

pub struct ContainerDetailPage {
    id: String,
    tab: ContainerTab,
    store: Option<Entity<EngineStore>>,
    focus: FocusHandle,
    _sub: Option<Subscription>,
}

pub fn new(
    id: String,
    tab: ContainerTab,
    store: Option<Entity<EngineStore>>,
    _window: &mut Window,
    cx: &mut App,
) -> Entity<ContainerDetailPage> {
    cx.new(|cx| {
        let sub = store
            .as_ref()
            .map(|s| cx.observe(s, |_, _, cx| cx.notify()));
        ContainerDetailPage {
            id,
            tab,
            store,
            focus: cx.focus_handle().tab_stop(true),
            _sub: sub,
        }
    })
}

impl ContainerDetailPage {
    fn summary<'a>(&self, cx: &'a App) -> Option<&'a ContainerSummary> {
        let store = self.store.as_ref()?.read(cx);
        store
            .containers
            .data()?
            .iter()
            .find(|c| c.id == self.id || c.name == self.id)
    }

    pub fn container_id(&self) -> &str {
        &self.id
    }

    pub fn tab(&self) -> ContainerTab {
        self.tab
    }
}

impl Focusable for ContainerDetailPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PageView for ContainerDetailPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ContainerDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary(cx).cloned();
        let name = summary
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| dk_core::format::short_id(&self.id).to_owned());
        let mut crumbs = vec![Crumb::link(s::PAGE_CONTAINERS, Route::Containers)];
        if let Some(project) = summary.as_ref().and_then(|c| c.compose.as_ref()) {
            crumbs.push(Crumb::here(project.project.clone()));
        }
        crumbs.push(Crumb::here(name.clone()));
        let focused = self.focus.is_focused(window);
        v_flex()
            .id("container-detail")
            .key_context(ctx::DETAIL_HEADER)
            .size_full()
            .p_4()
            .gap_3()
            .on_action(cx.listener(|_, _: &detail::ParentList, window, cx| {
                window.dispatch_action(Box::new(Back), cx);
            }))
            .child(breadcrumb(crumbs))
            .child(
                h_flex()
                    .id("detail-header")
                    .track_focus(&self.focus)
                    .gap_3()
                    .items_center()
                    .p_1()
                    .rounded(cx.theme().radius)
                    .map(|el| crate::ui::focus_ring(el, focused, cx))
                    .child(
                        Button::new("detail-back")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip_with_action(s::BACK, &Back, None)
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(Back), cx)),
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .when_some(summary.as_ref(), |this, c| {
                        this.child(container_chip(c.state, c.exit_code))
                            .when_some(c.health, |this, h| this.child(health_chip(h)))
                    }),
            )
            .when(summary.is_none(), |this| {
                this.child(
                    div()
                        .text_color(cx.theme().warning)
                        .child(s::CONTAINER_GONE),
                )
            })
            .child(div().text_color(cx.theme().muted_foreground).child(format!(
                "{} — {}",
                self.tab.label(),
                s::DETAIL_COMING
            )))
    }
}
