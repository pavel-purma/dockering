//! Stub (replaced below in this milestone).
use crate::state::EngineStore;
use crate::ui::page::{PageView, RoutedPage};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, FocusHandle, IntoElement, Render, Window, div};
pub struct NetworksPage {
    focus: FocusHandle,
}
impl NetworksPage {
    pub fn new(_store: Entity<EngineStore>, _w: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}
impl PageView for NetworksPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl RoutedPage for NetworksPage {}
impl Render for NetworksPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("stub").track_focus(&self.focus)
    }
}
