//! Engine connection UX (SHL-013, ENG-107, ENG-111): the full-page disconnected/failed
//! alert, the degraded banner, the first-run screen, and the no-active-engine screen.

use dk_core::{EngineState, EngineStatus};
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, div, px};

use crate::actions::{EngineSwitcher, ManageEngines, Rescan, RetryEngine, StartEngine};
use crate::strings as s;

/// Full-page alert for Disconnected/Failed/Stopped (SHL-013, ENG-107). Last-known data is
/// not shown so the user can't act on stale ids.
pub fn disconnected_page(status: &EngineStatus, cx: &App) -> AnyElement {
    let (message, hint, retry_in) = match &status.state {
        EngineState::Failed { error, retry_in_ms } => (
            error.to_string(),
            error.hint().map(str::to_owned),
            retry_in_ms.map(|ms| ms.div_ceil(1000)),
        ),
        EngineState::Unsupported { reason } => (reason.clone(), None, None),
        other => (
            format!(
                "{} — {}",
                crate::ui::status_chip::engine_state_label(other),
                s::ENGINE_DISCONNECTED_BODY
            ),
            None,
            None,
        ),
    };
    let stopped = matches!(status.state, EngineState::Stopped);
    v_flex()
        .id("engine-unreachable")
        .size_full()
        .items_center()
        .justify_center()
        .p_8()
        .child(
            v_flex()
                .max_w(px(640.))
                .gap_3()
                .child(Alert::error("engine-alert", message).title(format!(
                    "{} · {}",
                    s::ENGINE_UNREACHABLE_TITLE,
                    status.config.name
                )))
                .when_some(hint, |this, hint| this.child(div().text_sm().child(hint)))
                .when_some(retry_in, |this, secs| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(s::retry_in(secs)),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .when(stopped, |this| {
                            this.child(
                                Button::new("start-engine")
                                    .primary()
                                    .label(s::START_AND_CONNECT)
                                    .on_click(|_, w, cx| {
                                        w.dispatch_action(Box::new(StartEngine), cx)
                                    }),
                            )
                        })
                        .child(
                            Button::new("retry-engine")
                                .when(!stopped, |b| b.primary())
                                .icon(IconName::RefreshCw)
                                .label(s::RETRY)
                                .on_click(|_, w, cx| w.dispatch_action(Box::new(RetryEngine), cx)),
                        )
                        .child(
                            Button::new("switch-engine")
                                .label(s::SWITCH_ENGINE)
                                .tooltip_with_action(s::SWITCH_ENGINE, &EngineSwitcher, None)
                                .on_click(|_, w, cx| {
                                    w.dispatch_action(Box::new(EngineSwitcher), cx)
                                }),
                        ),
                ),
        )
        .into_any_element()
}

/// Degraded banner (SHL-013): data stays visible read-only.
pub fn degraded_banner() -> AnyElement {
    Alert::warning("degraded-banner", s::DEGRADED_BANNER)
        .banner()
        .into_any_element()
}

/// First run / no engine (ENG-111): per-OS guidance, Rescan, Add engine….
pub fn first_run(cx: &App) -> AnyElement {
    v_flex()
        .id("first-run")
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Frame).size(px(48.))),
        )
        .child(
            div()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(s::FIRST_RUN_TITLE),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(s::FIRST_RUN_BODY),
        )
        .child(
            v_flex().gap_1().text_sm().children(
                s::FIRST_RUN_GUIDANCE
                    .iter()
                    .map(|l| div().child(format!("• {l}"))),
            ),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("first-run-rescan")
                        .primary()
                        .icon(IconName::RefreshCw)
                        .label(s::RESCAN)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(Rescan), cx)),
                )
                .child(
                    Button::new("first-run-add")
                        .label(s::ADD_ENGINE)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(ManageEngines), cx)),
                ),
        )
        .into_any_element()
}

/// Engines exist but none is active.
pub fn no_active_engine() -> AnyElement {
    v_flex()
        .id("no-active-engine")
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(s::NO_ACTIVE_ENGINE_TITLE),
        )
        .child(div().text_sm().child(s::NO_ACTIVE_ENGINE_BODY))
        .child(
            Button::new("pick-engine")
                .primary()
                .label(s::SWITCH_ENGINE)
                .on_click(|_, w, cx| w.dispatch_action(Box::new(EngineSwitcher), cx)),
        )
        .into_any_element()
}
