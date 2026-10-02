//! TODO(phase-2): Images list page (`docs/spec/features/images.md`, IMG-*).
//!
//! Build it like `pages/containers`: a `ListTable<…Delegate>` over
//! `EngineStore.images` (`Resource<Vec<…>>`), a toolbar (search, filters), the four
//! states (`ui::states`), confirmations via `ui::confirm`, notifications via `ui::notify`,
//! single-letter actions bound in `keymap.rs` (`ListTable` context), and `PageView`.

use gpui_kit::{AppContext, Entity, Window};

use crate::pages::placeholder::PlaceholderPage;
use crate::strings as s;

pub fn new(_window: &mut Window, cx: &mut gpui_kit::App) -> Entity<PlaceholderPage> {
    cx.new(|cx| PlaceholderPage::new(s::PAGE_IMAGES, vec![], s::COMING_SOON, cx))
}
