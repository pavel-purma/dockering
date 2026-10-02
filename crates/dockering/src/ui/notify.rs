//! Notifications (SHL-003): success/info toasts, and errors with a *Copy details* action.

use dk_core::EngineError;
use gpui_kit::component::Sizable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, ClipboardItem, SharedString, Window};

use crate::strings as s;

pub fn success(window: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    window.push_notification(Notification::success(message), cx);
}

pub fn info(window: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    window.push_notification(Notification::info(message), cx);
}

/// Error toast with the engine message and *Copy details* (doesn't auto-hide, KBD-074).
pub fn error(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    details: impl Into<SharedString>,
) {
    let details: SharedString = details.into();
    let note = Notification::error(details.clone())
        .title(title)
        .action(move |_, _, _| {
            let details = details.clone();
            Button::new("copy-details")
                .small()
                .ghost()
                .label(s::COPY_DETAILS)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(details.to_string()))
                })
        });
    window.push_notification(note, cx);
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
