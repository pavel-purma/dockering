//! Pages. Containers is the M2 page; Images, Volumes and Networks (with their detail pages)
//! are M6 and are mounted by the shell through [`mount_resource_page`].

pub mod container_detail;
pub mod containers;
pub mod image_detail;
pub mod images;
pub mod network_detail;
pub mod networks;
pub mod placeholder;
pub mod resources;
pub mod settings;
pub mod volume_detail;
pub mod volumes;

use gpui_kit::{App, AppContext, Entity, Window};

use crate::nav::Route;
use crate::state::EngineStore;
use crate::ui::page::DynPage;

/// Creates the page for an M6 route (Images, Volumes, Networks and their details).
pub fn mount_resource_page(
    route: &Route,
    store: Entity<EngineStore>,
    window: &mut Window,
    cx: &mut App,
) -> DynPage {
    let sid = Some(store.entity_id());
    match route {
        Route::Images => DynPage::new(cx.new(|cx| images::ImagesPage::new(store, window, cx)), sid),
        Route::ImageDetail { id, tab } => DynPage::new(
            cx.new(|cx| image_detail::ImageDetailPage::new(id.clone(), *tab, store, window, cx)),
            sid,
        ),
        Route::Volumes => DynPage::new(
            cx.new(|cx| volumes::VolumesPage::new(store, window, cx)),
            sid,
        ),
        Route::VolumeDetail { name, tab } => DynPage::new(
            cx.new(|cx| {
                volume_detail::VolumeDetailPage::new(name.clone(), *tab, store, window, cx)
            }),
            sid,
        ),
        Route::NetworkDetail { id, tab } => DynPage::new(
            cx.new(|cx| {
                network_detail::NetworkDetailPage::new(id.clone(), *tab, store, window, cx)
            }),
            sid,
        ),
        _ => DynPage::new(
            cx.new(|cx| networks::NetworksPage::new(store, window, cx)),
            sid,
        ),
    }
}
