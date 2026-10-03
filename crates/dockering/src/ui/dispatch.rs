//! Pointer commands dispatch from the clicked control, not from focus (KBD-002, KBD-007).
//!
//! `Window::dispatch_action` routes along the *focused* element. That's right for keys, but
//! GPUI Kit buttons don't take focus on mouse down, so a click routed that way runs from
//! wherever focus was left. Once focus sat outside the view that handles the button's action
//! (back on the engine switcher's title-bar button after the switcher closed, or on a detail
//! page's tab bar while the Logs toolbar is clicked), the click silently did nothing.
//!
//! So clicks dispatch from an *origin*: a focus handle tracked inside the view that handles
//! the action (GPUI Kit's dialog buttons and Zed do the same). The action then walks the
//! control's own ancestors (that view, the page, the shell) whatever holds focus. Focus
//! doesn't move, and keys keep dispatching from focus.
//!
//! A view whose root already tracks a focus handle uses that as the origin. Otherwise it
//! owns a [`DispatchAnchor`] and renders [`DispatchAnchor::element`] inside the element its
//! `on_action`s are on. Shared builders take the origin as a `&FocusHandle`.

use gpui_kit::prelude::*;
use gpui_kit::{Action, App, ClickEvent, Div, FocusHandle, Window, div};

/// A focus handle that exists only to give a view an origin for click dispatch.
pub struct DispatchAnchor {
    handle: FocusHandle,
}

impl DispatchAnchor {
    pub fn new(cx: &App) -> Self {
        Self {
            handle: cx.focus_handle(),
        }
    }

    pub fn handle(&self) -> &FocusHandle {
        &self.handle
    }

    /// The node that places the anchor in the tree. It has no size, so it's never hovered,
    /// never takes focus, and isn't a Tab stop.
    pub fn element(&self) -> Div {
        div().absolute().size_0().track_focus(&self.handle)
    }
}

/// Dispatches `action` from `origin`'s node. Like `Window::dispatch_action` it runs on the next
/// effect cycle, so the triggering event finishes its own propagation first. Falls back to
/// focus when nothing on `origin`'s path handles the action (e.g. it isn't rendered).
pub fn dispatch_from(origin: &FocusHandle, action: &dyn Action, window: &mut Window, cx: &mut App) {
    let origin = origin.clone();
    let action = action.boxed_clone();
    window.defer(cx, move |window, cx| {
        if window.is_action_available_in(action.as_ref(), &origin) {
            origin.dispatch_action(action.as_ref(), window, cx);
        } else {
            window.dispatch_action(action, cx);
        }
    });
}

/// A click handler that dispatches `action` from `origin`.
pub fn on_click(
    origin: &FocusHandle,
    action: Box<dyn Action>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let origin = origin.clone();
    move |_, window, cx| dispatch_from(&origin, action.as_ref(), window, cx)
}

/// Like [`on_click`], and the click stops there (row buttons mustn't open the row too).
pub fn on_click_stop(
    origin: &FocusHandle,
    action: Box<dyn Action>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let origin = origin.clone();
    move |_, window, cx| {
        cx.stop_propagation();
        dispatch_from(&origin, action.as_ref(), window, cx)
    }
}

/// `build`'s control in a wrapper that carries its own origin, for controls whose builder has
/// no origin at hand (shared widgets, table cells). The origin sits right at the control, so
/// the action walks everything the control is inside of.
pub fn anchored<E: IntoElement>(cx: &App, build: impl FnOnce(&FocusHandle) -> E) -> Div {
    let anchor = DispatchAnchor::new(cx);
    let child = build(anchor.handle());
    div()
        .flex()
        .flex_none()
        .child(anchor.element())
        .child(child)
}
