//! Cross-links to resource detail pages (spec 30 §2): container detail → image, volume and
//! network detail; image/volume detail → containers. Links dispatch [`Navigate`] (KBD-002)
//! and `Enter` follows a focused link (KBD-043).

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::{App, ElementId, IntoElement, SharedString};

use crate::actions::Navigate;
use crate::nav::{ImageTab, NetworkTab, Route, VolumeTab};
use crate::ui::dispatch;

/// Route for an image id (or reference): container detail → image detail.
pub fn image_route(image_id: &str) -> Route {
    Route::ImageDetail {
        id: image_id.to_owned(),
        tab: ImageTab::Overview,
    }
}

/// Route for a volume name: container detail → volume detail.
pub fn volume_route(name: &str) -> Route {
    Route::VolumeDetail {
        name: name.to_owned(),
        tab: VolumeTab::Overview,
    }
}

/// Route for a network id or name (the detail page resolves either through
/// `inspect_network`).
pub fn network_route(id_or_name: &str) -> Route {
    Route::NetworkDetail {
        id: id_or_name.to_owned(),
        tab: NetworkTab::Overview,
    }
}

/// A link that navigates to `route`.
pub fn resource_link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    route: Route,
    cx: &App,
) -> gpui_kit::AnyElement {
    dispatch::anchored(cx, |origin| {
        Button::new(id)
            .link()
            .xsmall()
            .label(label.into())
            .on_click(dispatch::on_click(origin, Box::new(Navigate { route })))
    })
    .into_any_element()
}
