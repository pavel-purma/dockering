//! Container and engine status chips (spec 30 §4). Colour is never the only signal: every
//! chip carries a text label (spec 30 §5).
//!
//! Chips are *soft*: a tinted background, a coloured label and a leading dot, built on GPUI
//! Kit `Tag::color` palette scales. The kit's solid `success`/`warning` variants put a
//! saturated fill behind text of the same hue, which fails contrast in the dark theme.

use dk_core::{ContainerState, EngineState, Health};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, ColorName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Hsla, IntoElement, SharedString, div, px};

use crate::strings as s;

/// The semantic hue of a chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Info,
    Neutral,
}

impl Tone {
    fn color(self) -> ColorName {
        match self {
            Tone::Success => ColorName::Emerald,
            Tone::Warning => ColorName::Amber,
            Tone::Danger => ColorName::Red,
            Tone::Info => ColorName::Sky,
            Tone::Neutral => ColorName::Neutral,
        }
    }
}

/// A soft chip with a leading status dot: `● Running`, `● In use`.
pub fn tone_tag(tone: Tone, label: impl Into<SharedString>) -> Tag {
    let color = tone.color();
    Tag::color(color).small().child(
        h_flex()
            .gap_1p5()
            .items_center()
            .child(dot(color.scale(500)).size(px(6.)))
            .child(label.into()),
    )
}

/// `Running` = green, `Paused` = amber, `Exited(0)` = grey, `Exited(≠0)`/`Dead` = red,
/// `Restarting` = blue + spinner, `Created` = grey outline.
pub fn container_chip(state: ContainerState, exit_code: Option<i64>) -> impl IntoElement {
    let label = match (state, exit_code) {
        (ContainerState::Exited, Some(code)) => format!("Exited ({code})"),
        (state, _) => state.label().to_owned(),
    };
    let tone = match state {
        ContainerState::Running => Tone::Success,
        ContainerState::Paused => Tone::Warning,
        ContainerState::Restarting => Tone::Info,
        ContainerState::Exited if exit_code.unwrap_or(0) != 0 => Tone::Danger,
        ContainerState::Dead => Tone::Danger,
        _ => Tone::Neutral,
    };
    if state == ContainerState::Restarting {
        return Tag::color(tone.color())
            .small()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Spinner::new().xsmall())
                    .child(label),
            )
            .into_any_element();
    }
    tone_tag(tone, label)
        .when(state == ContainerState::Created, |this| this.outline())
        .into_any_element()
}

/// Health as a second chip (outlined, so it reads as secondary to the state chip).
pub fn health_chip(health: Health) -> impl IntoElement {
    let tone = match health {
        Health::Healthy => Tone::Success,
        Health::Unhealthy => Tone::Danger,
        Health::Starting => Tone::Info,
    };
    Tag::color(tone.color())
        .outline()
        .small()
        .child(health.label())
}

/// Status dot colour for engines (ENG-100): green connected, amber connecting/degraded,
/// red failed, grey disabled/stopped/unsupported.
pub fn engine_dot_color(state: &EngineState, cx: &App) -> Hsla {
    let t = cx.theme();
    match state {
        EngineState::Connected => t.success,
        EngineState::Connecting | EngineState::Degraded => t.warning,
        EngineState::Failed { .. } => t.danger,
        EngineState::Disconnected
        | EngineState::Disabled
        | EngineState::Stopped
        | EngineState::Unsupported { .. } => t.muted_foreground,
    }
}

pub fn engine_state_label(state: &EngineState) -> &'static str {
    match state {
        EngineState::Connected => s::STATE_CONNECTED,
        EngineState::Connecting => s::STATE_CONNECTING,
        EngineState::Degraded => s::STATE_DEGRADED,
        EngineState::Failed { .. } => s::STATE_FAILED,
        EngineState::Disconnected => s::STATE_DISCONNECTED,
        EngineState::Disabled => s::STATE_DISABLED,
        EngineState::Stopped => s::STATE_STOPPED,
        EngineState::Unsupported { .. } => s::STATE_UNSUPPORTED,
    }
}

/// A small coloured dot.
pub fn dot(color: Hsla) -> gpui_kit::Div {
    div().size(px(8.)).rounded_full().bg(color).flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_labels_cover_all_states() {
        assert_eq!(
            engine_state_label(&EngineState::Connected),
            s::STATE_CONNECTED
        );
        assert_eq!(
            engine_state_label(&EngineState::Unsupported {
                reason: "ssh".into()
            }),
            s::STATE_UNSUPPORTED
        );
    }
}
