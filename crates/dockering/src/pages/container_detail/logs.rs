//! Logs tab (stub; filled in by a later commit).

use gpui_kit::{App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window, div};

use gpui_kit::prelude::*;

use super::state::ContainerDetailState;

pub struct LogsView {
    _state: Entity<ContainerDetailState>,
    focus: FocusHandle,
}

impl LogsView {
    pub fn new(
        state: Entity<ContainerDetailState>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _state: state,
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    pub fn set_visible(&mut self, _visible: bool, _cx: &mut Context<Self>) {}

    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
}

impl Focusable for LogsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).child("Logs")
    }
}
