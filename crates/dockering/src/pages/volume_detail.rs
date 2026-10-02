//! TODO(phase-2): Volume detail page (`docs/spec/features/volumes.md`, VOL-*). The route
//! `Route::VolumeDetail { name, tab }` already works (palette "Go to", back/forward); this
//! placeholder shows a breadcrumb back to the list.

use gpui_kit::{AppContext, Entity, SharedString, Window};

use crate::nav::Route;
use crate::pages::placeholder::PlaceholderPage;
use crate::strings as s;

pub fn new(name: &str, _window: &mut Window, cx: &mut gpui_kit::App) -> Entity<PlaceholderPage> {
    let label: SharedString = name.to_owned().into();
    cx.new(|cx| {
        PlaceholderPage::new(
            label.clone(),
            vec![
                (s::PAGE_VOLUMES.into(), Some(Route::Volumes)),
                (label.clone(), None),
            ],
            s::COMING_SOON,
            cx,
        )
    })
}
