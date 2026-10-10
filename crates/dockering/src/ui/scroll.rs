//! Paging helpers for plain scroll containers (KBD-096).

use gpui_kit::{Pixels, ScrollHandle, point, px};

/// Scrolls `scroll` so a row spanning `top..bottom` is visible, with a small margin.
pub fn scroll_into_view(scroll: &ScrollHandle, top: Pixels, bottom: Pixels) {
    let view = scroll.bounds();
    if view.size.height <= px(0.) {
        return;
    }
    let offset = scroll.offset();
    let margin = px(8.);
    if top < view.top() {
        scroll.set_offset(point(offset.x, offset.y + (view.top() - top) + margin));
    } else if bottom > view.bottom() {
        scroll.set_offset(point(
            offset.x,
            offset.y - (bottom - view.bottom()) - margin,
        ));
    }
}

/// One line of overlap between pages.
const OVERLAP: Pixels = px(24.);

/// Scrolls `handle` by one viewport page, up (`up`) or down. Offsets are negative while
/// scrolled, so paging down makes the offset more negative; GPUI clamps it at layout.
pub fn scroll_page(handle: &ScrollHandle, up: bool) {
    let page = (handle.bounds().size.height - OVERLAP).max(px(40.));
    let offset = handle.offset();
    let max = handle.max_offset().y;
    let y = if up {
        (offset.y + page).min(px(0.))
    } else {
        (offset.y - page).max(-max)
    };
    handle.set_offset(point(offset.x, y));
}
