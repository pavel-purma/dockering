//! App assets: the GPUI Kit default icon bundle plus the extra Lucide icons Dockering uses
//! (spec 30 §4: icons come from the bundled Lucide set).

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

pub use gpui_kit::assets::IconName as Lucide;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Container, Square, Trash, Layers, Boxes, Keyboard, Server, Plug, Unplug
    ]
);

/// Default component assets + [`ExtraIcons`].
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
