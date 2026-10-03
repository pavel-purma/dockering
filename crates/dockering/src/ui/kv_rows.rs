//! Repeating key/value form rows (KBD-072): ports, env vars, mounts (Run image), driver
//! options and labels (Create volume). Each row is two text fields, an optional checkbox,
//! and a Tab-reachable *Remove* button; the section ends with an *Add* button.
//!
//! Keyboard: `Mod+Shift+Enter` adds a row (`form::AddRow`), `Mod+Shift+Backspace` removes the
//! focused row (`form::RemoveRow`); both are bound in the `Dialog` context and handled by the
//! section that holds focus. Enter on a focused Add/Remove button is routed here by the form
//! ([`KvRows::activate_focused`]) because the dialog's own `enter` binding runs first.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Window, div, px,
};

use crate::actions::form::{AddRow, RemoveRow};
use crate::keymap::ctx;
use crate::strings as s;
use crate::ui::widgets::focus_wrap;

/// What a section looks like.
#[derive(Debug, Clone, Copy)]
pub struct KvRowsSpec {
    /// Element id prefix (unique per form).
    pub id: &'static str,
    pub title: &'static str,
    pub key_placeholder: &'static str,
    pub value_placeholder: &'static str,
    pub separator: &'static str,
    pub add_label: &'static str,
    /// Optional per-row checkbox (e.g. *Read-only* for mounts).
    pub flag: Option<&'static str>,
}

/// One row's values: `(key, value, flag)`, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvValue {
    pub key: String,
    pub value: String,
    pub flag: bool,
}

pub struct KvRow {
    pub key: Entity<InputState>,
    pub value: Entity<InputState>,
    pub flag: bool,
    pub error: Option<SharedString>,
    remove_focus: FocusHandle,
    uid: usize,
}

pub struct KvRows {
    spec: KvRowsSpec,
    rows: Vec<KvRow>,
    add_focus: FocusHandle,
    next_uid: usize,
}

impl KvRows {
    pub fn new(spec: KvRowsSpec, cx: &mut Context<Self>) -> Self {
        Self {
            spec,
            rows: Vec::new(),
            add_focus: cx.focus_handle().tab_stop(true),
            next_uid: 0,
        }
    }

    pub fn spec(&self) -> &KvRowsSpec {
        &self.spec
    }

    pub fn rows(&self) -> &[KvRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn add_focus(&self) -> &FocusHandle {
        &self.add_focus
    }

    /// Appends a row and focuses its first field.
    pub fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row("", "", false, window, cx);
        if let Some(row) = self.rows.last() {
            let h = row.key.focus_handle(cx);
            window.focus(&h, cx);
        }
        cx.notify();
    }

    /// Appends a row with values (prefill, tests).
    pub fn push_row(
        &mut self,
        key: &str,
        value: &str,
        flag: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kp, vp) = (self.spec.key_placeholder, self.spec.value_placeholder);
        let key_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(kp)
                .default_value(key.to_owned())
        });
        let value_in = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(vp)
                .default_value(value.to_owned())
        });
        self.next_uid += 1;
        self.rows.push(KvRow {
            key: key_in,
            value: value_in,
            flag,
            error: None,
            remove_focus: cx.focus_handle().tab_stop(true),
            uid: self.next_uid,
        });
        cx.notify();
    }

    /// Removes row `ix`; focus moves to the next row, else the previous one, else *Add*
    /// (KBD-007).
    pub fn remove_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.rows.len() {
            return;
        }
        self.rows.remove(ix);
        let target = self
            .rows
            .get(ix)
            .or_else(|| ix.checked_sub(1).and_then(|p| self.rows.get(p)))
            .map(|r| r.key.focus_handle(cx))
            .unwrap_or_else(|| self.add_focus.clone());
        window.focus(&target, cx);
        cx.notify();
    }

    /// The row holding focus (either field or its Remove button).
    pub fn focused_row(&self, window: &Window, cx: &App) -> Option<usize> {
        self.rows.iter().position(|r| {
            r.key.focus_handle(cx).is_focused(window)
                || r.value.focus_handle(cx).is_focused(window)
                || r.remove_focus.is_focused(window)
        })
    }

    /// Whether focus is anywhere in this section.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.add_focus.is_focused(window) || self.focused_row(window, cx).is_some()
    }

    /// Enter on a focused Add/Remove button (see the module docs). Returns true if handled.
    pub fn activate_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.add_focus.is_focused(window) {
            self.add_row(window, cx);
            return true;
        }
        if let Some(ix) = self
            .rows
            .iter()
            .position(|r| r.remove_focus.is_focused(window))
        {
            self.remove_row(ix, window, cx);
            return true;
        }
        false
    }

    /// Current values, trimmed; fully empty rows are skipped by callers via
    /// [`KvValue::is_blank`].
    pub fn values(&self, cx: &App) -> Vec<KvValue> {
        self.rows
            .iter()
            .map(|r| KvValue {
                key: r.key.read(cx).value().trim().to_owned(),
                value: r.value.read(cx).value().trim().to_owned(),
                flag: r.flag,
            })
            .collect()
    }

    /// Per-row validation messages (same order as [`Self::values`]).
    pub fn set_errors(&mut self, errors: Vec<Option<SharedString>>, cx: &mut Context<Self>) {
        for (row, err) in self.rows.iter_mut().zip(errors) {
            row.error = err;
        }
        cx.notify();
    }

    fn on_add(&mut self, _: &AddRow, window: &mut Window, cx: &mut Context<Self>) {
        if self.contains_focus(window, cx) {
            self.add_row(window, cx);
        } else {
            cx.propagate();
        }
    }

    fn on_remove(&mut self, _: &RemoveRow, window: &mut Window, cx: &mut Context<Self>) {
        match self.focused_row(window, cx) {
            Some(ix) => self.remove_row(ix, window, cx),
            None => cx.propagate(),
        }
    }
}

impl KvValue {
    pub fn is_blank(&self) -> bool {
        self.key.is_empty() && self.value.is_empty()
    }
}

impl Render for KvRows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = self.spec;
        let entity = cx.entity().downgrade();
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let uid = row.uid;
            let flag = row.flag;
            let remove = {
                let entity = entity.clone();
                focus_wrap(
                    "kv-remove-wrap",
                    &row.remove_focus,
                    Button::new(("kv-remove", uid))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Minus)
                        .tooltip_with_action(s::REMOVE_ROW, &RemoveRow, Some(ctx::DIALOG))
                        .on_click({
                            let entity = entity.clone();
                            move |_, window, cx| {
                                entity
                                    .update(cx, |this, cx| {
                                        if let Some(ix) =
                                            this.rows.iter().position(|r| r.uid == uid)
                                        {
                                            this.remove_row(ix, window, cx)
                                        }
                                    })
                                    .ok();
                            }
                        }),
                    move |_, window, cx| {
                        entity
                            .update(cx, |this, cx| {
                                if let Some(ix) = this.rows.iter().position(|r| r.uid == uid) {
                                    this.remove_row(ix, window, cx)
                                }
                            })
                            .ok();
                    },
                    cx,
                )
            };
            v_flex()
                .id(("kv-row", uid))
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().child(Input::new(&row.key).small()))
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(spec.separator),
                        )
                        .child(div().flex_1().child(Input::new(&row.value).small()))
                        .when_some(spec.flag, |this, label| {
                            let entity = entity.clone();
                            this.child(
                                Checkbox::new(("kv-flag", uid))
                                    .label(label)
                                    .checked(flag)
                                    .on_click(move |v: &bool, _, cx| {
                                        let v = *v;
                                        entity
                                            .update(cx, |this, cx| {
                                                if let Some(r) =
                                                    this.rows.iter_mut().find(|r| r.uid == uid)
                                                {
                                                    r.flag = v;
                                                }
                                                cx.notify();
                                            })
                                            .ok();
                                    }),
                            )
                        })
                        .child(remove),
                )
                .when_some(row.error.clone(), |this, err| {
                    this.child(
                        div()
                            .id(("kv-error", ix))
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(err),
                    )
                })
        });
        let add = {
            let entity = entity.clone();
            focus_wrap(
                "kv-add-wrap",
                &self.add_focus,
                Button::new(SharedString::from(format!("{}-add", spec.id)))
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(spec.add_label)
                    .tooltip_with_action(s::ADD_ROW, &AddRow, Some(ctx::DIALOG))
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| this.add_row(window, cx)).ok();
                        }
                    }),
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.add_row(window, cx)).ok();
                },
                cx,
            )
        };
        v_flex()
            .id(spec.id)
            .gap_1()
            .on_action(cx.listener(Self::on_add))
            .on_action(cx.listener(Self::on_remove))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(spec.title),
            )
            .children(rows)
            .child(h_flex().child(add))
            .min_w(px(0.))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_values() {
        let v = KvValue {
            key: String::new(),
            value: String::new(),
            flag: true,
        };
        assert!(v.is_blank());
    }
}
