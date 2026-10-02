//! Shared pieces of the Image, Volume and Network detail pages (spec 30 §2, IMG-010,
//! VOL-010, NET-003):
//!
//! - [`DetailTabs`]: a GPUI Kit `TabBar` that is one focusable stop with arrow keys
//!   (`DetailTabs` key context, KBD-040) and `Ctrl+Tab` handled by the page;
//! - [`InspectView`]: read-only JSON in the GPUI Kit code editor, with *Copy* and the
//!   editor's own search (`Mod+F` inside it, KBD-025). Pretty-printing runs on
//!   `background_spawn` (NFR-003);
//! - [`Loaded`]: the detail fetch state with a revision guard (NFR-005);
//! - small renderers: key/value sections (`DescriptionList`), gone banner, links.

use std::collections::BTreeMap;

use dk_core::EngineError;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::description_list::{DescriptionItem, DescriptionList};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Task, Window, div, px,
};

use crate::actions::{Back, Navigate};
use crate::keymap::ctx;
use crate::nav::Route;
use crate::strings as s;
use crate::ui::notify;
use crate::ui::widgets::focus_ring;

/// A detail fetch: `None` before the first result.
#[derive(Debug, Clone)]
pub enum Loaded<T> {
    Loading,
    Ready(T),
    Failed(EngineError),
}

impl<T> Loaded<T> {
    pub fn data(&self) -> Option<&T> {
        match self {
            Loaded::Ready(d) => Some(d),
            _ => None,
        }
    }
    pub fn is_loading(&self) -> bool {
        matches!(self, Loaded::Loading)
    }
    pub fn error(&self) -> Option<&EngineError> {
        match self {
            Loaded::Failed(e) => Some(e),
            _ => None,
        }
    }
    /// A NotFound error: the resource is gone (banner instead of an error panel).
    pub fn is_gone(&self) -> bool {
        matches!(self, Loaded::Failed(EngineError::NotFound { .. }))
    }
}

/// The tab bar of a detail page: one Tab stop, `←/→` switch tabs (bound in the
/// `DetailTabs` context to `detail::PrevTab/NextTab`, which the page handles).
pub fn tab_bar(
    focus: &FocusHandle,
    labels: &[(&'static str, bool)],
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let focused = focus.is_focused(window);
    div()
        .id("detail-tabs")
        .key_context(ctx::DETAIL_TABS)
        .track_focus(focus)
        .rounded(cx.theme().radius)
        .map(|el| focus_ring(el, focused, cx))
        .child(
            TabBar::new("detail-tab-bar")
                .underline()
                .small()
                .selected_index(selected)
                .on_click(move |ix, window, cx| on_select(*ix, window, cx))
                .children(
                    labels
                        .iter()
                        .map(|(label, disabled)| Tab::new().label(*label).disabled(*disabled)),
                ),
        )
        .into_any_element()
}

/// Next enabled tab index after `current` in direction `dir` (wraps).
pub fn step_tab(enabled: &[bool], current: usize, forward: bool) -> usize {
    let n = enabled.len();
    if n == 0 {
        return 0;
    }
    let mut ix = current;
    for _ in 0..n {
        ix = if forward {
            (ix + 1) % n
        } else {
            (ix + n - 1) % n
        };
        if enabled[ix] {
            return ix;
        }
    }
    current
}

/// Header row: back button, title, extra chips, then action buttons.
pub fn header(
    focus: &FocusHandle,
    title: impl Into<SharedString>,
    chips: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let focused = focus.is_focused(window);
    h_flex()
        .id("detail-header")
        .key_context(ctx::DETAIL_HEADER)
        .track_focus(focus)
        .gap_3()
        .items_center()
        .p_1()
        .rounded(cx.theme().radius)
        .map(|el| focus_ring(el, focused, cx))
        .child(
            Button::new("detail-back")
                .ghost()
                .icon(IconName::ArrowLeft)
                .tooltip_with_action(s::BACK, &Back, None)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Back), cx)),
        )
        .child(
            div()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .truncate()
                .child(title.into()),
        )
        .children(chips)
        .child(div().flex_1())
        .children(actions)
        .into_any_element()
}

/// A header action button that dispatches `action` and shows its binding.
pub fn header_action(
    id: &'static str,
    label: &'static str,
    icon: Option<IconName>,
    action: Box<dyn Action>,
    disabled: bool,
) -> AnyElement {
    let hint = action.boxed_clone();
    Button::new(id)
        .small()
        .outline()
        .label(label)
        .when_some(icon, |b, i| b.icon(i))
        .disabled(disabled)
        .tooltip_with_action(label, hint.as_ref(), Some(ctx::DETAIL_HEADER))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        .into_any_element()
}

/// "This image no longer exists" + *Back to list* (spec 30 §2).
pub fn gone_banner(kind: &str, parent: Route, cx: &App) -> AnyElement {
    h_flex()
        .id("gone-banner")
        .gap_3()
        .items_center()
        .p_2()
        .rounded(cx.theme().radius)
        .bg(cx.theme().warning.opacity(0.15))
        .child(div().text_sm().child(s::resource_gone(kind)))
        .child(
            Button::new("back-to-list")
                .small()
                .label(s::BACK_TO_LIST)
                .on_click(move |_, window, cx| {
                    window.dispatch_action(
                        Box::new(Navigate {
                            route: parent.clone(),
                        }),
                        cx,
                    )
                }),
        )
        .into_any_element()
}

/// A key/value section built on GPUI Kit `DescriptionList`.
pub fn kv_section(
    title: Option<&'static str>,
    items: Vec<(&'static str, AnyElement)>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .when_some(title, |this, t| {
            this.child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(t),
            )
        })
        .child(
            DescriptionList::new()
                .columns(1)
                .label_width(px(160.))
                .small()
                .children(
                    items
                        .into_iter()
                        .map(|(label, value)| DescriptionItem::new(label).value(value)),
                ),
        )
        .into_any_element()
}

/// Plain text value (or "—").
pub fn text_value(v: Option<impl Into<SharedString>>, cx: &App) -> AnyElement {
    match v {
        Some(v) => {
            let v: SharedString = v.into();
            if v.is_empty() {
                crate::pages::resources::chrome::dash(cx)
            } else {
                div().text_sm().child(v).into_any_element()
            }
        }
        None => crate::pages::resources::chrome::dash(cx),
    }
}

/// Monospace value (ids, digests, commands).
pub fn mono_value(v: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .child(v.into())
        .into_any_element()
}

/// A list of strings, one per line (or "—").
pub fn lines_value(lines: &[String], cx: &App) -> AnyElement {
    if lines.is_empty() {
        return crate::pages::resources::chrome::dash(cx);
    }
    v_flex()
        .gap_0p5()
        .children(lines.iter().map(|l| mono_value(l.clone(), cx)))
        .into_any_element()
}

/// A map as `key=value` lines.
pub fn map_value(m: &BTreeMap<String, String>, cx: &App) -> AnyElement {
    let lines: Vec<String> = m.iter().map(|(k, v)| format!("{k}={v}")).collect();
    lines_value(&lines, cx)
}

/// Yes/No.
pub fn bool_value(v: bool) -> AnyElement {
    div()
        .text_sm()
        .child(if v { s::YES } else { s::NO })
        .into_any_element()
}

/// A link to a route (container names in "Used by", KBD-043). Not a Tab stop on its own:
/// the list it lives in is focusable as one stop and `Enter` follows the cursor link.
pub fn route_link(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    route: Route,
) -> AnyElement {
    Button::new(id)
        .link()
        .xsmall()
        .label(label.into())
        .on_click(move |_, window, cx| {
            window.dispatch_action(
                Box::new(Navigate {
                    route: route.clone(),
                }),
                cx,
            )
        })
        .into_any_element()
}

/// A focusable list of link rows (Used by / Containers tabs): one Tab stop, `↑/↓` move a
/// cursor, `Enter` follows the link (KBD-043/044).
pub struct LinkList {
    pub focus: FocusHandle,
    pub cursor: usize,
}

impl LinkList {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            cursor: 0,
        }
    }

    /// Renders `rows` (each row: route + cells). `on_key` handles up/down/enter.
    #[allow(clippy::too_many_arguments)]
    pub fn render<V: 'static>(
        &self,
        id: &'static str,
        headers: &[&'static str],
        rows: Vec<(Route, Vec<AnyElement>)>,
        empty: &'static str,
        window: &Window,
        cx: &mut Context<V>,
        get: fn(&mut V) -> &mut LinkList,
    ) -> AnyElement {
        let focused = self.focus.is_focused(window);
        if rows.is_empty() {
            return div()
                .id(id)
                .track_focus(&self.focus)
                .p_2()
                .rounded(cx.theme().radius)
                .map(|el| focus_ring(el, focused, cx))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(empty)
                .into_any_element();
        }
        let n = rows.len();
        let cursor = self.cursor.min(n - 1);
        let routes: Vec<Route> = rows.iter().map(|(r, _)| r.clone()).collect();
        let cols = headers.len();
        v_flex()
            .id(id)
            .track_focus(&self.focus)
            .rounded(cx.theme().radius)
            .map(|el| focus_ring(el, focused, cx))
            .on_key_down(
                cx.listener(move |this, e: &gpui_kit::KeyDownEvent, window, cx| {
                    if e.keystroke.modifiers.modified() {
                        return;
                    }
                    let list = get(this);
                    match e.keystroke.key.as_str() {
                        "down" => list.cursor = (list.cursor + 1).min(n - 1),
                        "up" => list.cursor = list.cursor.saturating_sub(1),
                        "home" => list.cursor = 0,
                        "end" => list.cursor = n - 1,
                        "enter" => {
                            if let Some(route) = routes.get(list.cursor.min(n - 1)).cloned() {
                                window.dispatch_action(Box::new(Navigate { route }), cx);
                            }
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .children(headers.iter().map(|h| div().flex_1().child(*h))),
            )
            .children(rows.into_iter().enumerate().map(|(ix, (_, cells))| {
                h_flex()
                    .id(("link-row", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .when(focused && ix == cursor, |this| this.bg(cx.theme().accent))
                    .children(
                        cells
                            .into_iter()
                            .take(cols)
                            .map(|c| div().flex_1().min_w_0().child(c)),
                    )
            }))
            .into_any_element()
    }
}

/// The *Inspect* tab: pretty JSON in a read-only code editor (spec 30 §1 "Raw JSON").
pub struct InspectView {
    editor: Entity<EditorState>,
    json: SharedString,
    revision: u64,
    format_task: Option<Task<()>>,
    copy_focus: FocusHandle,
}

impl InspectView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
                .searchable(true)
        });
        editor.update(cx, |e, cx| e.set_readonly(true, cx));
        Self {
            editor,
            json: SharedString::default(),
            revision: 0,
            format_task: None,
            copy_focus: cx.focus_handle().tab_stop(true),
        }
    }

    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    pub fn text(&self) -> &SharedString {
        &self.json
    }

    /// Formats `raw` on a background thread, then shows it (stale results dropped).
    pub fn set_value(
        &mut self,
        raw: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.revision += 1;
        let rev = self.revision;
        let job =
            cx.background_spawn(
                async move { serde_json::to_string_pretty(&raw).unwrap_or_default() },
            );
        self.format_task = Some(cx.spawn_in(window, async move |this, cx| {
            let text = job.await;
            this.update_in(cx, |this, window, cx| {
                if this.revision != rev {
                    return;
                }
                this.json = text.clone().into();
                this.editor
                    .update(cx, |e, cx| e.set_value(text, window, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.json.to_string()));
        notify::info(window, cx, s::COPIED);
    }
}

impl Focusable for InspectView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for InspectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let this2 = this.clone();
        v_flex()
            .id("inspect-view")
            .size_full()
            .gap_2()
            .child(
                h_flex().gap_2().child(crate::ui::widgets::focus_wrap(
                    "inspect-copy-wrap",
                    &self.copy_focus,
                    Button::new("inspect-copy")
                        .small()
                        .outline()
                        .icon(IconName::Copy)
                        .label(s::COPY)
                        .on_click(move |_, window, cx| {
                            this.update(cx, |v, cx| v.copy(window, cx)).ok();
                        }),
                    move |_, window, cx| {
                        this2.update(cx, |v, cx| v.copy(window, cx)).ok();
                    },
                    window,
                    cx,
                )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.editor).readonly(true).h_full()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kbd_040_step_tab_skips_disabled() {
        let enabled = [true, false, true, true];
        assert_eq!(step_tab(&enabled, 0, true), 2);
        assert_eq!(step_tab(&enabled, 2, false), 0);
        assert_eq!(step_tab(&enabled, 3, true), 0);
        assert_eq!(step_tab(&enabled, 0, false), 3);
    }
}
