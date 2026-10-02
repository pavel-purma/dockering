//! Reusable UI building blocks for every page (phase 2 builds on these).
//!
//! | Module | Use |
//! |---|---|
//! | [`list_table`] | Generic `ListTable<D>` over GPUI Kit `DataTable` with roving focus, groups, checkbox selection, quick find |
//! | [`confirm`] | Destructive confirmation dialog (KBD-071, SHL-002) |
//! | [`notify`] | Success/error notifications with *Copy details* (SHL-003) |
//! | [`status_chip`] | Container/engine status chips with text labels (spec 30 §4) |
//! | [`states`] | Empty state, error panel with Retry, skeleton rows |
//! | [`widgets`] | Copy-id, port link, relative time, focus ring, keyboard hint tooltips |
//! | [`page`] | The `Page` trait (`primary_focus`, search focus, refresh) |
//! | [`breadcrumb`] | Detail-page breadcrumb |

pub mod breadcrumb;
pub mod confirm;
pub mod list_table;
pub mod menu;
pub mod notify;
pub mod page;
pub mod states;
pub mod status_chip;
pub mod widgets;

pub use states::{empty_state, error_panel, skeleton_rows};
pub use widgets::{copy_id, focus_ring, port_link, relative_time};

// ── M6 additions ────────────────────────────────────────────────────────────────────────
/// Form dialogs (Pull/Run/Tag image, Create volume; KBD-071/072).
pub mod form_dialog;
/// Repeating key/value form rows (KBD-072).
pub mod kv_rows;
/// Cross-links to resource detail pages (spec 30 §2).
pub mod links;
