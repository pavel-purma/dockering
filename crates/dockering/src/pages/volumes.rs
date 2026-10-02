//! TODO(phase-2): Volumes list page (`docs/spec/features/volumes.md`, VOL-*).
//!
//! Build it like `pages/containers`: a `ListTable<…Delegate>` over
//! `EngineStore.volumes` (`Resource<Vec<…>>`), a toolbar (search, filters), the four
//! states (`ui::states`), confirmations via `ui::confirm`, notifications via `ui::notify`,
//! single-letter actions bound in `keymap.rs` (`ListTable` context), and `PageView`.

use gpui_kit::{AppContext, Entity, Window};

use crate::pages::placeholder::PlaceholderPage;
use crate::strings as s;

pub fn new(_window: &mut Window, cx: &mut gpui_kit::App) -> Entity<PlaceholderPage> {
    cx.new(|cx| PlaceholderPage::new(s::PAGE_VOLUMES, vec![], s::COMING_SOON, cx))
}
