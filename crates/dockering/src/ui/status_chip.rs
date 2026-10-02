//! Container and engine status chips (spec 30 §4). Colour is never the only signal: every
//! chip carries a text label (spec 30 §5).

use dk_core::{ContainerState, EngineState, Health};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Hsla, IntoElement, div, px};

use crate::strings as s;

/// `Running` = green, `Paused` = amber, `Exited(0)` = grey, `Exited(≠0)`/`Dead` = red,
/// `Restarting` = blue + spinner, `Created` = grey outline.
pub fn container_chip(state: ContainerState, exit_code: Option<i64>) -> impl IntoElement {
    let label = match (state, exit_code) {
        (ContainerState::Exited, Some(code)) => format!("Exited ({code})"),
        (state, _) => state.label().to_owned(),
    };
    let tag = match state {
        ContainerState::Running => Tag::success(),
        ContainerState::Paused => Tag::warning(),
        ContainerState::Restarting => Tag::info(),
        ContainerState::Exited if exit_code.unwrap_or(0) != 0 => Tag::danger(),
        ContainerState::Dead => Tag::danger(),
        ContainerState::Created => Tag::secondary().outline(),
        _ => Tag::secondary(),
    };
    tag.small().child(
        h_flex()
            .gap_1()
            .items_center()
            .when(state == ContainerState::Restarting, |this| {
                this.child(Spinner::new().xsmall())
            })
            .child(label),
    )
}

/// Health as a second chip.
pub fn health_chip(health: Health) -> impl IntoElement {
    let tag = match health {
        Health::Healthy => Tag::success().outline(),
        Health::Unhealthy => Tag::danger().outline(),
        Health::Starting => Tag::info().outline(),
    };
    tag.small().child(health.label())
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
pub fn dot(color: Hsla) -> impl IntoElement {
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
