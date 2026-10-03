//! Form dialogs (KBD-071/072) on GPUI Kit `Dialog`: Pull image, Run image, Tag image,
//! Create volume.
//!
//! - Initial focus goes to the form's first field (the form focuses it on its first render,
//!   S-8.5).
//! - `Enter` (the dialog's own `Confirm` binding, which a single-line `Input` lets through)
//!   and `Mod+Enter` (`dialog::ConfirmDestructive`) both call [`FormView::submit_key`]. Forms
//!   use it to submit, or to activate a focused row button / Cancel.
//! - `Esc` cancels (GPUI Kit), and closing restores focus to the invoker (GPUI Kit root).

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Entity, FocusHandle, IntoElement, Render, SharedString, Window, div, px,
};

use crate::actions::dialog::ConfirmDestructive;
use crate::keymap::ctx;
use crate::strings as s;
use crate::ui::widgets::focus_wrap;

/// A form shown in a dialog.
pub trait FormView: Render + Sized + 'static {
    /// `Enter` / `Mod+Enter` anywhere in the dialog.
    fn submit_key(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<Self>);
}

/// Opens `view` in a dialog titled `title`.
pub fn open_form_dialog<V: FormView>(
    title: impl Into<SharedString>,
    width: f32,
    view: Entity<V>,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = title.into();
    window.open_dialog(cx, move |dialog, _, _| {
        let on_ok = view.clone();
        dialog
            .title(title.clone())
            .w(px(width))
            .overlay_closable(false)
            .on_ok(move |_, window, cx| {
                on_ok.update(cx, |f, cx| f.submit_key(window, cx));
                false
            })
            .child(view.clone())
    });
}

/// The form body wrapper: the `Dialog` key context and `Mod+Enter` routing.
pub fn form_root<V: FormView>(
    id: &'static str,
    cx: &mut gpui_kit::Context<V>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex().id(id).key_context(ctx::DIALOG).gap_3().on_action(
        cx.listener(|this, _: &ConfirmDestructive, window, cx| this.submit_key(window, cx)),
    )
}

/// A labelled field: label, control, optional hint and error.
pub fn field(
    label: &'static str,
    control: impl IntoElement,
    hint: Option<&'static str>,
    error: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(label),
        )
        .child(control)
        .when_some(hint, |this, h| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(h),
            )
        })
        .when_some(error, |this, e| {
            this.child(div().text_xs().text_color(cx.theme().danger).child(e))
        })
        .into_any_element()
}

/// An inline error line (submit failures).
pub fn form_error(error: Option<SharedString>, cx: &App) -> Option<AnyElement> {
    error.map(|e| {
        div()
            .id("form-error")
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().danger.opacity(0.1))
            .text_sm()
            .text_color(cx.theme().danger)
            .child(e)
            .into_any_element()
    })
}

/// Footer with *Cancel* and the primary button, both focusable through handles the form owns.
#[allow(clippy::too_many_arguments)]
pub fn footer(
    cancel_focus: &FocusHandle,
    ok_focus: &FocusHandle,
    ok_label: &'static str,
    busy: bool,
    on_cancel: impl Fn(&mut Window, &mut App) + Clone + 'static,
    on_ok: impl Fn(&mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let (c1, c2) = (on_cancel.clone(), on_cancel);
    let (o1, o2) = (on_ok.clone(), on_ok);
    h_flex()
        .gap_2()
        .justify_end()
        .child(focus_wrap(
            "form-cancel-wrap",
            cancel_focus,
            Button::new("form-cancel")
                .label(s::CANCEL)
                .on_click(move |_, window, cx| c1(window, cx)),
            move |_, window, cx| c2(window, cx),
            cx,
        ))
        .child(focus_wrap(
            "form-ok-wrap",
            ok_focus,
            Button::new("form-ok")
                .primary()
                .label(ok_label)
                .loading(busy)
                .tooltip_with_action(ok_label, &ConfirmDestructive, Some(ctx::DIALOG))
                .on_click(move |_, window, cx| o1(window, cx)),
            move |_, window, cx| o2(window, cx),
            cx,
        ))
        .into_any_element()
}

/// Closes the dialog (focus returns to the invoker).
pub fn close(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
}
