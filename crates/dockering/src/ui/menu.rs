//! Keyboard-openable popup menus (KBD-036, KBD-039, KBD-073): an owned GPUI Kit `PopupMenu`
//! anchored at a position, focused on open, closed on dismiss. Used for the row menu
//! (`Shift+F10`), the sort menu (`Mod+Shift+O`), the group-by menu (`Mod+Shift+G`) and
//! the header overflow. GPUI Kit's `dropdown_menu`/`context_menu` only open from pointer
//! events; this gives the same menu a keyboard entry point.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, DismissEvent, Entity, FocusHandle, Focusable, Pixels, Point,
    Subscription, Window, anchored, deferred, px,
};

pub struct KeyMenu {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _sub: Subscription,
}

impl KeyMenu {
    /// Builds and focuses a menu at `position`. `restore` gets focus back on dismiss and is
    /// where item actions dispatch (`PopupMenu::action_context`). `on_close` runs on dismiss.
    pub fn open<V: 'static>(
        position: Point<Pixels>,
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
        Self {
            menu,
            position,
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
                .position(self.position)
                .snap_to_window_with_margin(px(8.))
                .child(self.menu.clone()),
        )
        .with_priority(1)
        .into_any_element()
    }
}
