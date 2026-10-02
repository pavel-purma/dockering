//! Images page (IMG-001…007). Mirrors `pages/containers`: pure `model`, hub `ops`, the
//! `row` delegate, and the `view`; `dialogs` holds Pull/Run/Tag and `forms` their parsing.

pub mod dialogs;
pub mod forms;
pub mod model;
pub mod ops;
mod row;
mod view;

pub use model::{ImageFilter, ImageRow};
pub use view::ImagesPage;
