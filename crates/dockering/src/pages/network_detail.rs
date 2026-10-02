//! TODO(phase-2): Network detail page (`docs/spec/features/networks.md`, NET-*). The route
//! `Route::NetworkDetail { id, tab }` already works (palette "Go to", back/forward); this
//! placeholder shows a breadcrumb back to the list.

use gpui_kit::{AppContext, Entity, SharedString, Window};

use crate::nav::Route;
use crate::pages::placeholder::PlaceholderPage;
use crate::strings as s;

pub fn new(id: &str, _window: &mut Window, cx: &mut gpui_kit::App) -> Entity<PlaceholderPage> {
    let label: SharedString = dk_core::format::short_id(id).to_owned().into();
    cx.new(|cx| {
        PlaceholderPage::new(
            label.clone(),
            vec![
                (s::PAGE_NETWORKS.into(), Some(Route::Networks)),
                (label.clone(), None),
            ],
            s::COMING_SOON,
            cx,
        )
    })
}
