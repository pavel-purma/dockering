//! Terminal tab (stub; filled in by a later commit).

use gpui_kit::{App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window, div};

use gpui_kit::prelude::*;

use super::state::ContainerDetailState;

pub struct TerminalTab {
    _state: Entity<ContainerDetailState>,
    focus: FocusHandle,
}

impl TerminalTab {
    pub fn new(
        state: Entity<ContainerDetailState>,
        _tab_bar: FocusHandle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _state: state,
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}

impl Focusable for TerminalTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).child("Terminal")
    }
}
