//! Inspect tab (CDT-040): the raw inspect JSON, pretty-printed on `background_spawn`
//! (NFR-003), in the read-only GPUI Kit code editor with JSON highlighting, *Copy*, and the
//! editor's own search (`Mod+F`, KBD-025). A warning says env values are not masked here.
//!
//! Reuses the shared [`InspectView`] of the resource detail pages.

use gpui_kit::component::alert::Alert;
use gpui_kit::component::{Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription,
    Window, div,
};

use super::state::ContainerDetailState;
use crate::pages::resources::detail::InspectView;
use crate::strings as s;

pub struct InspectTab {
    state: Entity<ContainerDetailState>,
    view: Entity<InspectView>,
    /// Pointer identity of the last JSON shown (avoid reformatting unchanged data).
    shown: Option<serde_json::Value>,
    _sub: Subscription,
}

impl InspectTab {
    pub fn new(
        state: Entity<ContainerDetailState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = cx.new(|cx| InspectView::new(window, cx));
        let sub = cx.observe_in(&state, window, |this, _, window, cx| this.sync(window, cx));
        let mut this = Self {
            state,
            view,
            shown: None,
            _sub: sub,
        };
        this.sync(window, cx);
        this
    }

    pub fn view(&self) -> &Entity<InspectView> {
        &self.view
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(raw) = self.state.read(cx).details.data().map(|d| d.raw.clone()) else {
            return;
        };
        if self.shown.as_ref() == Some(&raw) {
            return;
        }
        self.shown = Some(raw.clone());
        self.view.update(cx, |v, cx| v.set_value(raw, window, cx));
        cx.notify();
    }

    /// `Mod+F` from the page (KBD-025): focus the editor and open its search.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.view.read(cx).editor().clone();
        let h = editor.focus_handle(cx);
        window.focus(&h, cx);
        editor.update(cx, |e, cx| e.open_search(false, cx));
    }
}

impl Focusable for InspectTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.focus_handle(cx)
    }
}

impl Render for InspectTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("inspect-tab")
            .size_full()
            .gap_2()
            .child(Alert::warning("inspect-unmasked", s::INSPECT_UNMASKED).small())
            .child(div().flex_1().min_h_0().child(self.view.clone()))
    }
}
