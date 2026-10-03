//! The page pattern. Every routed page implements [`PageView`] so the shell can focus its
//! primary control after navigation (KBD-007), forward `Mod+F` (KBD-025), and refresh.

use gpui_kit::{App, FocusHandle, Window};

pub trait PageView {
    /// The control that gets focus after navigation (the list, or the first tab).
    fn primary_focus(&self, cx: &App) -> FocusHandle;

    /// `Mod+F`: focus the page search field. Default: the primary control.
    fn focus_search(&mut self, window: &mut Window, cx: &mut App) {
        let handle = self.primary_focus(cx);
        window.focus(&handle, cx);
    }

    /// Escape chain step 3 (KBD-006): clear a focused, non-empty search. Returns true if handled.
    fn clear_search_if_focused(&mut self, _: &mut Window, _: &mut App) -> bool {
        false
    }
}

// ── M6: type-erased pages for the shell (Images, Volumes, Networks and their details) ───────

use std::rc::Rc;

use gpui_kit::{AnyView, Entity, Render};

use crate::nav::Route;

/// Extra hooks for pages mounted through [`DynPage`].
pub trait RoutedPage: PageView + Render + Sized + 'static {
    /// The route changed but stays on this page kind (e.g. a detail tab switch): update in
    /// place and return true, or return false to have the shell mount a fresh page.
    fn accept_route(
        &mut self,
        route: &Route,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        let _ = (route, window, cx);
        false
    }
}

trait DynOps {
    fn primary_focus(&self, cx: &App) -> FocusHandle;
    fn focus_search(&self, window: &mut Window, cx: &mut App);
    fn clear_search(&self, window: &mut Window, cx: &mut App) -> bool;
    fn accept_route(&self, route: &Route, window: &mut Window, cx: &mut App) -> bool;
}

impl<V: RoutedPage> DynOps for Entity<V> {
    fn primary_focus(&self, cx: &App) -> FocusHandle {
        self.read(cx).primary_focus(cx)
    }
    fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |p, cx| p.focus_search(window, cx));
    }
    fn clear_search(&self, window: &mut Window, cx: &mut App) -> bool {
        self.update(cx, |p, cx| p.clear_search_if_focused(window, cx))
    }
    fn accept_route(&self, route: &Route, window: &mut Window, cx: &mut App) -> bool {
        self.update(cx, |p, cx| {
            let ok = p.accept_route(route, window, cx);
            if ok {
                cx.notify();
            }
            ok
        })
    }
}

/// A mounted page of any type (the shell keeps one of these per route).
#[derive(Clone)]
pub struct DynPage {
    view: AnyView,
    ops: Rc<dyn DynOps>,
    store: Option<gpui_kit::EntityId>,
}

impl DynPage {
    pub fn new<V: RoutedPage>(entity: Entity<V>, store: Option<gpui_kit::EntityId>) -> Self {
        Self {
            view: entity.clone().into(),
            ops: Rc::new(entity),
            store,
        }
    }
    /// The `EngineStore` the page was built on (pages never outlive an engine switch).
    pub fn store_id(&self) -> Option<gpui_kit::EntityId> {
        self.store
    }
    pub fn view(&self) -> AnyView {
        self.view.clone()
    }
    /// The concrete page entity, if it is a `V` (tests, shell lookups).
    pub fn downcast<V: 'static>(&self) -> Option<Entity<V>> {
        self.view.clone().downcast::<V>().ok()
    }
    pub fn primary_focus(&self, cx: &App) -> FocusHandle {
        self.ops.primary_focus(cx)
    }
    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.ops.focus_search(window, cx)
    }
    pub fn clear_search_if_focused(&self, window: &mut Window, cx: &mut App) -> bool {
        self.ops.clear_search(window, cx)
    }
    pub fn accept_route(&self, route: &Route, window: &mut Window, cx: &mut App) -> bool {
        self.ops.accept_route(route, window, cx)
    }
}
