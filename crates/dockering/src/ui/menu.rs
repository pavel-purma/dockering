//! Keyboard-openable popup menus (KBD-036, KBD-039, KBD-073): an owned GPUI Kit `PopupMenu`
//! anchored at a position, focused on open, closed on dismiss. Used for the row menu
//! (`Shift+F10`), the sort menu (`Mod+Shift+O`), the group-by menu (`Mod+Shift+G`) and
//! the header overflow. GPUI Kit's `dropdown_menu`/`context_menu` only open from pointer
//! events; this gives the same menu a keyboard entry point.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, App, Bounds, Context, DismissEvent, Entity, FocusHandle, Focusable, Pixels,
    Point, Subscription, Window, anchored, canvas, deferred, point, px,
};

/// Gap between a trigger and the menu it opens.
const TRIGGER_GAP: Pixels = px(4.);

pub struct KeyMenu {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    /// The menu corner placed at `position`.
    anchor: Anchor,
    _sub: Subscription,
}

/// Where a menu opens: at a point (its top-left corner there), or under a trigger button,
/// right edges aligned like GPUI Kit's dropdown menus.
#[derive(Debug, Clone, Copy)]
pub enum MenuAnchor {
    At(Point<Pixels>),
    Below(Bounds<Pixels>),
}

impl MenuAnchor {
    /// Below `trigger` once it has painted bounds, else at `fallback`.
    pub fn below_or(trigger: Bounds<Pixels>, fallback: Point<Pixels>) -> Self {
        if trigger.size.width > px(0.) {
            MenuAnchor::Below(trigger)
        } else {
            MenuAnchor::At(fallback)
        }
    }

    fn resolve(self) -> (Point<Pixels>, Anchor) {
        match self {
            MenuAnchor::At(p) => (p, Anchor::TopLeft),
            MenuAnchor::Below(b) => (
                point(
                    b.origin.x + b.size.width,
                    b.origin.y + b.size.height + TRIGGER_GAP,
                ),
                Anchor::TopRight,
            ),
        }
    }
}

/// Reports an element's painted bounds after layout, for anchoring popups to it.
///
/// GPUI Kit's `ElementExt::on_prepaint` measures with an absolute canvas that has no insets,
/// so inside a block `div` the canvas sits at its static position *below* the content and
/// reports bounds shifted down by the element's height. This pins the canvas to the
/// element's top-left corner.
pub trait TrackBounds: ParentElement + Sized {
    fn on_bounds(
        self,
        callback: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.child(
            canvas(
                move |b, window, cx| callback(b, window, cx),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}

impl<T: ParentElement> TrackBounds for T {}

impl From<Point<Pixels>> for MenuAnchor {
    fn from(p: Point<Pixels>) -> Self {
        MenuAnchor::At(p)
    }
}

impl KeyMenu {
    /// Builds and focuses a menu at `at` (a point, or [`MenuAnchor::Below`] a trigger).
    /// `restore` gets focus back on dismiss and is where item actions dispatch (`PopupMenu::action_context`). `on_close` runs on dismiss.
    pub fn open<V: 'static>(
        at: impl Into<MenuAnchor>,
        restore: FocusHandle,
        window: &mut Window,
        cx: &mut Context<V>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
        on_close: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Self {
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            build(menu.action_context(restore), window, cx)
        });
        let sub = cx.subscribe_in(&menu, window, move |this, _, _: &DismissEvent, _, cx| {
            on_close(this, cx);
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        let (position, anchor) = at.into().resolve();
        Self {
            menu,
            position,
            anchor,
            _sub: sub,
        }
    }

    pub fn menu(&self) -> &Entity<PopupMenu> {
        &self.menu
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.menu.focus_handle(cx).contains_focused(window, cx)
    }

    /// The deferred, anchored overlay element.
    pub fn render(&self) -> AnyElement {
        deferred(
            anchored()
                .anchor(self.anchor)
                .position(self.position)
                .snap_to_window_with_margin(px(8.))
                .child(self.menu.clone()),
        )
        .with_priority(1)
        .into_any_element()
    }
}
