//! Networks page (NET-001…005): pure `model`, the `row` delegate and the `view`. The
//! sidebar entry can be hidden in Settings (NET-001, `general.show_networks_page`).

pub mod model;
mod row;
mod view;

pub use model::NetworkRow;
pub use view::NetworksPage;
