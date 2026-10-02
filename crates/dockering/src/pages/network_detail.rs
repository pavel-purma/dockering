//! Stub (replaced below in this milestone).
use crate::nav::NetworkTab;
use crate::state::EngineStore;
use crate::ui::page::{PageView, RoutedPage};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, FocusHandle, IntoElement, Render, Window, div};
pub struct NetworkDetailPage {
    focus: FocusHandle,
}
impl NetworkDetailPage {
    pub fn new(
        _id: String,
        _tab: NetworkTab,
        _store: Entity<EngineStore>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}
impl PageView for NetworkDetailPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl RoutedPage for NetworkDetailPage {}
impl Render for NetworkDetailPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("stub").track_focus(&self.focus)
    }
}
