//! Notifications (SHL-003): success/info toasts, and errors with a *Copy details* action.

use dk_core::EngineError;
use gpui_kit::{App, ClipboardItem, SharedString, Window};

use super::toast::{self, Toast, ToastAction};
use crate::strings as s;

pub fn success(_: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    toast::push(Toast::success(message), cx);
}

pub fn info(_: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    toast::push(Toast::info(message), cx);
}

/// Error toast with the engine message and *Copy details* (doesn't auto-hide, KBD-074).
pub fn error(
    _: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    details: impl Into<SharedString>,
) {
    let details: SharedString = details.into();
    let copy = details.clone();
    let toast = Toast::error(details).title(title).action(
        ToastAction::new("copy-details", s::COPY_DETAILS, move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()))
        })
        .ghost(),
    );
    toast::push(toast, cx);
}

pub fn engine_error(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    err: &EngineError,
) {
    let mut details = err.to_string();
    if let Some(hint) = err.hint() {
        details.push('\n');
        details.push_str(hint);
    }
    error(window, cx, title, details);
}
