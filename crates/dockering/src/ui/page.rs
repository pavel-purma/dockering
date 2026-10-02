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
    fn clear_search_if_focused(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        false
    }
}
