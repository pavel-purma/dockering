//! Volumes page (VOL-001…005): pure `model`, the `row` delegate, the `dialog` (Create
//! volume) and the `view`. Sizes come lazily from `disk_usage()` (VOL-002).

pub mod dialog;
pub mod model;
mod row;
mod view;

pub use model::{UsageState, VolumeFilter, VolumeRow};
pub use view::VolumesPage;
