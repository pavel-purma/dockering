//! Destructive confirmation (SHL-002, KBD-071).
//!
//! Built on GPUI Kit `Dialog` (`window.open_dialog`), which provides the modal layer, focus
//! trap, Escape-to-cancel, and invoker focus restore on close. This module adds:
//! - initial focus on **Cancel** (the safe choice), via a focus handle we own;
//! - `Mod+Enter` = confirm (`dialog::ConfirmDestructive`) — plain Enter only activates the
//!   focused button, so a destructive confirm needs an explicit Tab or the chord;
//! - an item list (truncated), reclaimable size, a note, and optional checkboxes
//!   (*Force*, *Don't ask again*), whose values are passed to `on_confirm`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, Render, SharedString, Window, div,
};

use crate::actions::dialog::ConfirmDestructive;
use crate::keymap::ctx;
use crate::strings as s;
use crate::ui::widgets::focus_wrap;

/// Max items listed before "…and N more".
pub const MAX_LISTED: usize = 12;

/// A checkbox option in the dialog.
#[derive(Clone)]
pub struct ConfirmOption {
    pub label: SharedString,
    pub checked: bool,
}

/// What the dialog shows.
#[derive(Clone)]
pub struct ConfirmSpec {
    pub title: SharedString,
    pub body: Option<SharedString>,
    pub items: Vec<SharedString>,
    pub reclaimable: Option<SharedString>,
    pub note: Option<SharedString>,
    pub confirm_label: SharedString,
    pub options: Vec<ConfirmOption>,
}

impl ConfirmSpec {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            body: None,
            items: Vec::new(),
            reclaimable: None,
            note: None,
            confirm_label: s::DELETE.into(),
            options: Vec::new(),
        }
    }
    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }
    pub fn reclaimable(mut self, size: impl Into<SharedString>) -> Self {
        self.reclaimable = Some(size.into());
        self
    }
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }
    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = label.into();
        self
    }
    pub fn option(mut self, label: impl Into<SharedString>, checked: bool) -> Self {
        self.options.push(ConfirmOption {
            label: label.into(),
            checked,
        });
        self
    }
}

type OnConfirm = Rc<dyn Fn(Vec<bool>, &mut Window, &mut App)>;

/// The dialog body entity: owns focus handles and option state.
pub struct ConfirmView {
    spec: ConfirmSpec,
    options: Vec<bool>,
    cancel_focus: FocusHandle,
    confirm_focus: FocusHandle,
    on_confirm: OnConfirm,
    done: Rc<RefCell<bool>>,
    /// Focus Cancel on the first render (KBD-071).
    focus_cancel: bool,
}

impl ConfirmView {
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut *self.done.borrow_mut(), true) {
            return;
        }
        let options = self.options.clone();
        let on_confirm = self.on_confirm.clone();
        // Close first so focus returns to the invoker (KBD-007), then act (SHL-012).
        window.close_dialog(cx);
        on_confirm(options, window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        *self.done.borrow_mut() = true;
        window.close_dialog(cx);
    }

    pub fn options(&self) -> &[bool] {
        &self.options
    }

    pub fn cancel_focus(&self) -> &FocusHandle {
        &self.cancel_focus
    }
}

impl Render for ConfirmView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_cancel) {
            window.focus(&self.cancel_focus, cx);
        }
        let spec = &self.spec;
        let extra = spec.items.len().saturating_sub(MAX_LISTED);
        v_flex()
            .id("confirm-body")
            .key_context(ctx::DIALOG)
            .gap_3()
            .on_action(
                cx.listener(|this, _: &ConfirmDestructive, window, cx| this.confirm(window, cx)),
            )
            .when_some(spec.body.clone(), |this, body| {
                this.child(div().text_sm().child(body))
            })
            .when(!spec.items.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("confirm-items")
                        .max_h(gpui_kit::px(180.))
                        .overflow_y_scroll()
                        .gap_0p5()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().muted)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .children(
                            spec.items
                                .iter()
                                .take(MAX_LISTED)
                                .map(|i| div().child(i.clone())),
                        )
                        .when(extra > 0, |this| {
                            this.child(div().child(s::and_more(extra)))
                        }),
                )
            })
            .when_some(spec.reclaimable.clone(), |this, size| {
                this.child(div().text_sm().child(s::reclaimable(&size)))
            })
            .when_some(spec.note.clone(), |this, note| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
            })
            .children(spec.options.iter().enumerate().map(|(ix, opt)| {
                let checked = self.options.get(ix).copied().unwrap_or(false);
                Checkbox::new(("confirm-opt", ix))
                    .label(opt.label.clone())
                    .checked(checked)
                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                        if let Some(slot) = this.options.get_mut(ix) {
                            *slot = *value;
                        }
                        cx.notify();
                    }))
            }))
            .child(
                DialogFooter::new().child(
                    h_flex()
                        .gap_2()
                        .child(focus_wrap(
                            "confirm-cancel-wrap",
                            &self.cancel_focus,
                            Button::new("confirm-cancel")
                                .label(s::CANCEL)
                                .tab_stop(false)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                                ),
                            cx.listener(|this, _: &gpui_kit::KeyDownEvent, window, cx| {
                                this.cancel(window, cx)
                            }),
                            window,
                            cx,
                        ))
                        .child(focus_wrap(
                            "confirm-ok-wrap",
                            &self.confirm_focus,
                            Button::new("confirm-ok")
                                .danger()
                                .label(spec.confirm_label.clone())
                                .tab_stop(false)
                                .tooltip_with_action(
                                    spec.confirm_label.clone(),
                                    &ConfirmDestructive,
                                    Some(ctx::DIALOG),
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                                ),
                            cx.listener(|this, _: &gpui_kit::KeyDownEvent, window, cx| {
                                this.confirm(window, cx)
                            }),
                            window,
                            cx,
                        )),
                ),
            )
    }
}

/// Opens the confirmation. `on_confirm(options)` runs after the dialog closed. Returns the
/// body entity (tests inspect it).
pub fn confirm_destructive(
    spec: ConfirmSpec,
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(Vec<bool>, &mut Window, &mut App) + 'static,
) -> Entity<ConfirmView> {
    let options = spec.options.iter().map(|o| o.checked).collect();
    let title = spec.title.clone();
    let view = cx.new(|cx| ConfirmView {
        spec,
        options,
        cancel_focus: cx.focus_handle(),
        confirm_focus: cx.focus_handle(),
        on_confirm: Rc::new(on_confirm),
        done: Rc::new(RefCell::new(false)),
        focus_cancel: true,
    });
    let body = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(title.clone())
            .w(gpui_kit::px(460.))
            .overlay_closable(true)
            .child(body.clone())
    });
    // Initial focus on Cancel (KBD-071): the dialog host focuses itself on open; Cancel is
    // inside it, so focusing it keeps the trap and the invoker restore intact.
    view
}

/// Whether a confirmation for deleting stopped containers should be shown (CON-021).
pub fn should_confirm_stopped_delete(cx: &App) -> bool {
    crate::state::AppState::config(cx)
        .general
        .confirm_delete_stopped
}
