//! Small shared widgets: copy-id, port link, relative time, focus ring, action tooltips.

use dk_core::PortMapping;
use dk_core::format::{format_port, format_relative, short_id};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, ElementId, IntoElement, SharedString, Styled, div};
use time::OffsetDateTime;

use crate::actions::{CopyText, OpenUrl};
use crate::state::Ticker;
use crate::strings as s;

/// The theme focus ring on custom focusable elements (KBD-003). GPUI Kit controls draw
/// their own; use this for `div`s with `track_focus`.
pub fn focus_ring<E: Styled>(el: E, focused: bool, cx: &App) -> E {
    if focused {
        el.border_1().border_color(cx.theme().ring)
    } else {
        el.border_1().border_color(gpui_kit::transparent_black())
    }
}

/// `abcdef123456` + copy button (SHL-008). The button dispatches [`CopyText`].
pub fn copy_id(id_prefix: impl Into<ElementId>, full_id: &str, cx: &App) -> impl IntoElement {
    let text: SharedString = full_id.to_owned().into();
    h_flex()
        .gap_1()
        .items_center()
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
                .on_click(move |_, window, cx| {
                    window.dispatch_action(Box::new(CopyText { text: text.clone() }), cx)
                }),
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
            Button::new(id)
                .link()
                .xsmall()
                .label(format!("{label} ↗"))
                .tooltip(url.clone())
                .on_click(move |_, window, cx| {
                    window.dispatch_action(Box::new(OpenUrl { url: url.clone() }), cx)
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
