//! Stats tab (stub; filled in by a later commit).

use gpui_kit::{App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window, div};

use gpui_kit::prelude::*;

use super::state::ContainerDetailState;

pub struct StatsTab {
    _state: Entity<ContainerDetailState>,
    focus: FocusHandle,
}

impl StatsTab {
    pub fn new(state: Entity<ContainerDetailState>, cx: &mut Context<Self>) -> Self {
        Self {
            _state: state,
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    pub fn set_visible(&mut self, _visible: bool, _cx: &mut Context<Self>) {}
}

impl Focusable for StatsTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StatsTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).child("Stats")
    }
}
