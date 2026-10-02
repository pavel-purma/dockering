//! Shortcut reference (KBD-022): searchable, generated from the keymap for the current OS,
//! grouped by context. Reused by Settings › Keyboard (SET-070).

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription,
    Window, div, px,
};

use crate::keymap::{Group, Os, ReferenceRow, reference_rows};
use crate::strings as s;

pub struct ShortcutList {
    rows: Vec<ReferenceRow>,
    filter: Entity<InputState>,
    query: String,
    _sub: Subscription,
}

impl ShortcutList {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(s::SHORTCUTS_FILTER));
        let sub = cx.subscribe_in(&filter, window, |this, input, e: &InputEvent, _, cx| {
            if let InputEvent::Change = e {
                this.query = input.read(cx).value().to_lowercase();
                cx.notify();
            }
        });
        Self {
            rows: reference_rows(Os::current()),
            filter,
            query: String::new(),
            _sub: sub,
        }
    }

    pub fn rows(&self) -> &[ReferenceRow] {
        &self.rows
    }

    /// Rows matching the filter (label, keys, or group).
    pub fn visible_rows(&self) -> Vec<&ReferenceRow> {
        self.rows
            .iter()
            .filter(|r| {
                self.query.is_empty()
                    || r.label.to_lowercase().contains(&self.query)
                    || r.keys.to_lowercase().contains(&self.query)
                    || r.group.label().to_lowercase().contains(&self.query)
            })
            .collect()
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |i, cx| i.focus(window, cx));
    }
}

impl Focusable for ShortcutList {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.focus_handle(cx)
    }
}

impl Render for ShortcutList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.visible_rows();
        let mut groups: Vec<(Group, Vec<&ReferenceRow>)> = Vec::new();
        for r in rows {
            match groups.last_mut() {
                Some((g, v)) if *g == r.group => v.push(r),
                _ => groups.push((r.group, vec![r])),
            }
        }
        v_flex()
            .id("shortcut-list")
            .gap_3()
            .size_full()
            .child(
                Input::new(&self.filter)
                    .small()
                    .prefix(gpui_kit::component::Icon::new(IconName::Search).small()),
            )
            .child(
                v_flex()
                    .id("shortcut-rows")
                    .gap_3()
                    .max_h(px(460.))
                    .overflow_y_scroll()
                    .children(groups.into_iter().map(|(g, rows)| {
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(g.label()),
                            )
                            .children(rows.into_iter().map(|r| {
                                h_flex()
                                    .justify_between()
                                    .gap_4()
                                    .text_sm()
                                    .child(div().child(r.label))
                                    .child(
                                        div()
                                            .px_1()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().muted)
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_xs()
                                            .child(r.keys.clone()),
                                    )
                            }))
                    })),
            )
    }
}
