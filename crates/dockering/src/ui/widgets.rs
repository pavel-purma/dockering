//! Small shared widgets: section heading, copy-id, port link, relative time, action tooltips.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use dk_core::PortMapping;
use dk_core::format::{format_port, format_relative, short_id};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, ElementId, IntoElement, SharedString, Styled, canvas, div, px};
use time::OffsetDateTime;

use crate::actions::{CopyText, OpenUrl};
use crate::state::Ticker;
use crate::strings as s;
use crate::ui::dispatch;

/// A titled group of content without a surrounding border (spec 30 §4): the title, then a
/// hairline that runs to the right edge, then the content. The content (a table, a list)
/// keeps its own border; the group itself only marks where it starts.
pub fn section(title: impl Into<SharedString>, body: impl IntoElement, cx: &App) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .gap_3()
        // Space above the heading separates it from the previous group's content.
        .pt_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child(title.into()),
                )
                .child(div().flex_1().h(gpui_kit::px(1.)).bg(cx.theme().border)),
        )
        .child(body)
}

/// `abcdef123456` + copy button (SHL-008). The button dispatches [`CopyText`].
pub fn copy_id(id_prefix: impl Into<ElementId>, full_id: &str, cx: &App) -> impl IntoElement {
    let text: SharedString = full_id.to_owned().into();
    let anchor = dispatch::DispatchAnchor::new(cx);
    h_flex()
        .relative()
        .gap_1()
        .items_center()
        .child(anchor.element())
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .child(short_id(full_id).to_owned()),
        )
        .child(
            Button::new(id_prefix)
                .ghost()
                .xsmall()
                .icon(IconName::Copy)
                .tooltip(s::COPY_ID)
                .on_click(dispatch::on_click_stop(
                    anchor.handle(),
                    Box::new(CopyText { text }),
                )),
        )
}

/// URL for a published port (spec 30 §4): `http://localhost:<public>`.
pub fn port_url(p: &PortMapping) -> Option<String> {
    let public = p.public?;
    let host = match p.ip {
        Some(ip) if !ip.is_unspecified() => {
            if ip.is_ipv6() {
                format!("[{ip}]")
            } else {
                ip.to_string()
            }
        }
        _ => "localhost".to_owned(),
    };
    Some(format!("http://{host}:{public}"))
}

/// A port as a link (`8080:80 ↗`) when published, plain text otherwise. Clicking dispatches
/// [`OpenUrl`] (handled with `cx.open_url`).
pub fn port_link(id: impl Into<ElementId>, p: &PortMapping, cx: &App) -> gpui_kit::AnyElement {
    let label = format_port(p);
    match port_url(p) {
        Some(url) => {
            let url: SharedString = url.into();
            dispatch::anchored(cx, |origin| {
                Button::new(id)
                    .link()
                    .xsmall()
                    .label(format!("{label} ↗"))
                    .tooltip(url.clone())
                    .on_click(dispatch::on_click_stop(origin, Box::new(OpenUrl { url })))
            })
            .into_any_element()
        }
        None => div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(label)
            .into_any_element(),
    }
}

/// "2 hours ago", refreshed by the app-wide [`Ticker`] (SHL-007).
pub fn relative_time(t: OffsetDateTime, cx: &App) -> String {
    format_relative(t, Ticker::now_in(cx))
}

/// Which cells overflowed their column when last painted (truncated text, clipped port
/// links), so the next render can add a tooltip or a "show all" control. Cells measure
/// themselves at prepaint; a change schedules one more frame.
#[derive(Clone, Default)]
pub struct OverflowSet(Rc<RefCell<HashSet<SharedString>>>);

impl OverflowSet {
    pub fn contains(&self, key: &str) -> bool {
        self.0.borrow().contains(key)
    }

    fn set(&self, key: &SharedString, overflows: bool, window: &mut gpui_kit::Window) {
        let changed = if overflows {
            self.0.borrow_mut().insert(key.clone())
        } else {
            self.0.borrow_mut().remove(key)
        };
        if changed {
            // Prepaint can't invalidate the frame being drawn; redraw once more.
            window.on_next_frame(|window, _| window.refresh());
        }
    }

    /// An invisible child that records whether `text`, in the inherited text style, is
    /// wider than its parent. The parent must be `relative`.
    pub fn probe_text(&self, key: SharedString, text: SharedString) -> gpui_kit::AnyElement {
        let this = self.clone();
        canvas(
            move |bounds, window, _| {
                let style = window.text_style();
                let size = style.font_size.to_pixels(window.rem_size());
                let run = style.to_run(text.len());
                let width = window
                    .text_system()
                    .shape_line(text.clone(), size, &[run], None)
                    .width;
                this.set(&key, width > bounds.size.width + px(0.5), window);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// A clipping box around `content` laid out at its natural width; records whether the
    /// content is wider than the box.
    pub fn clip(&self, key: SharedString, content: impl IntoElement) -> gpui_kit::Div {
        let this = self.clone();
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .on_children_prepainted(move |bounds, window, _| {
                if let [clip, content] = bounds.as_slice() {
                    this.set(&key, content.size.width > clip.size.width + px(0.5), window);
                }
            })
            .child(
                canvas(|_, _, _| {}, |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(h_flex().flex_none().h_full().items_center().child(content))
    }
}

/// A focusable wrapper around a GPUI Kit `Button`, so the caller owns
/// the focus handle (S-8.5: the component `Button` keeps its own keyed handle). Use it when
/// focus must be set programmatically (initial dialog focus, F6 region defaults, menu
/// restore targets). Enter/Space on the wrapper run `on_activate`; the inner button is
/// removed from the Tab order. Records its bounds into `bounds` for anchoring popups.
pub fn focus_wrap(
    id: &'static str,
    handle: &gpui_kit::FocusHandle,
    button: Button,
    on_activate: impl Fn(&gpui_kit::KeyDownEvent, &mut gpui_kit::Window, &mut App) + 'static,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .track_focus(&handle.clone().tab_stop(true))
        .rounded(cx.theme().radius)
        .on_key_down(move |e: &gpui_kit::KeyDownEvent, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space")
                && !e.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                on_activate(e, window, cx);
            }
        })
        .child(button.tab_stop(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::Proto;

    #[test]
    fn port_urls() {
        let mut p = PortMapping {
            ip: None,
            private: 80,
            public: Some(8080),
            proto: Proto::Tcp,
        };
        assert_eq!(port_url(&p).as_deref(), Some("http://localhost:8080"));
        p.ip = Some("0.0.0.0".parse().unwrap());
        assert_eq!(port_url(&p).as_deref(), Some("http://localhost:8080"));
        p.ip = Some("127.0.0.2".parse().unwrap());
        assert_eq!(port_url(&p).as_deref(), Some("http://127.0.0.2:8080"));
        p.public = None;
        assert_eq!(port_url(&p), None);
    }
}
