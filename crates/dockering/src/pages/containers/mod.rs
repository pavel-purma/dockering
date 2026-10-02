//! Containers page (CON-001…034, KBD-030…039). Rows are `dk_core::grouping` output
//! flattened by `ListTable`; actions go through `ops` on the hub.

pub mod model;
pub mod ops;
mod row;
mod view;

pub use model::{GroupInfo, StatusFilter};
pub use view::ContainersPage;
