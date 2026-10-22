//! Engine connection UX (SHL-013, ENG-107, ENG-111): the full-page disconnected/failed
//! alert, the degraded banner, the first-run screen, and the no-active-engine screen.

use dk_core::{EngineId, EngineState, EngineStatus, SetupOffer, StartPrompt};
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, FocusHandle, IntoElement, SharedString, div, px};

use crate::actions::{
    EngineSwitcher, Rescan, RetryEngine, SetUpEngine, StartEngine, StartListedEngine,
};
use crate::strings as s;
use crate::ui::dispatch;
use crate::ui::widgets::focus_wrap;

/// Full-page alert for Disconnected/Failed/Stopped (SHL-013, ENG-107). Last-known data is
/// not shown so the user can't act on stale ids.
///
/// `restart` is the prompt of an engine the user already started: when that start failed, the
/// page offers *Start* again next to *Retry* (ENG-147).
pub fn disconnected_page(
    status: &EngineStatus,
    restart: Option<&StartPrompt>,
    origin: &FocusHandle,
    start_focus: &FocusHandle,
    cx: &App,
) -> AnyElement {
    if matches!(status.state, EngineState::Stopped)
        && let Some(prompt) = &status.start_prompt
    {
        return start_page(prompt, false, origin, start_focus, cx);
    }
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
    let can_start =
        stopped || (restart.is_some() && matches!(status.state, EngineState::Failed { .. }));
    let start_text = match restart {
        Some(p) if !stopped => p.action_label.clone(),
        _ => start_label(status),
    };
    v_flex()
        .id("engine-unreachable")
        .debug_selector(|| "engine-unreachable".into())
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
                        .when(can_start, |this| {
                            this.child(
                                Button::new("start-engine")
                                    .primary()
                                    .debug_selector(|| "failed-start-button".into())
                                    .label(start_text)
                                    .on_click(dispatch::on_click(origin, Box::new(StartEngine))),
                            )
                        })
                        .child(
                            Button::new("retry-engine")
                                .when(!can_start, |b| b.primary())
                                .icon(IconName::RefreshCw)
                                .label(s::RETRY)
                                .on_click(dispatch::on_click(origin, Box::new(RetryEngine))),
                        )
                        .child(
                            Button::new("switch-engine")
                                .label(s::SWITCH_ENGINE)
                                .tooltip_with_action(s::SWITCH_ENGINE, &EngineSwitcher, None)
                                .on_click(dispatch::on_click(origin, Box::new(EngineSwitcher))),
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

/// A listed `Stopped` engine the backend can start (ENG-158).
#[derive(Debug, Clone, PartialEq)]
pub struct StartRow {
    pub id: EngineId,
    pub name: String,
    pub prompt: StartPrompt,
}

/// What the first-run and no-engine pages list before the generic guidance (ENG-158).
pub struct Landing<'a> {
    pub starts: &'a [StartRow],
    pub offers: &'a [SetupOffer],
}

/// A row's button. The page's primary one takes `primary` (initial focus) and activates on
/// Enter/Space; every button is Tab-reachable.
fn row_button(
    id: SharedString,
    label: String,
    action: Box<dyn gpui_kit::Action>,
    primary: Option<&FocusHandle>,
    origin: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let button = Button::new(id)
        .when(primary.is_some(), |b| b.primary())
        .label(label)
        .on_click(dispatch::on_click(origin, action.boxed_clone()));
    match primary {
        Some(handle) => primary_wrap(handle, button, action, origin, cx),
        None => button.into_any_element(),
    }
}

/// The page's primary control: owns `handle` (initial focus) and runs `action` on Enter/Space.
fn primary_wrap(
    handle: &FocusHandle,
    button: Button,
    action: Box<dyn gpui_kit::Action>,
    origin: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let origin = origin.clone();
    focus_wrap(
        "landing-primary-wrap",
        handle,
        button,
        move |_: &gpui_kit::KeyDownEvent, window: &mut gpui_kit::Window, cx: &mut App| {
            dispatch::dispatch_from(&origin, action.as_ref(), window, cx)
        },
        cx,
    )
    .into_any_element()
}

fn row_shell(
    id: SharedString,
    title: String,
    subtitle: String,
    button: AnyElement,
    cx: &App,
) -> AnyElement {
    let selector = id.to_string();
    h_flex()
        .id(id)
        .debug_selector(move || selector.clone())
        .w_full()
        .gap_3()
        .p_3()
        .items_center()
        .justify_between()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle),
                ),
        )
        .child(div().flex_none().child(button))
        .into_any_element()
}

fn section_title(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// The start and set-up rows. The first button is the page's primary action and takes
/// `primary`; the flag says whether it was used.
fn landing_rows(
    landing: &Landing,
    primary: &FocusHandle,
    origin: &FocusHandle,
    cx: &App,
) -> (Vec<AnyElement>, bool) {
    let mut out = Vec::new();
    let mut used = false;
    let mut take = || {
        let first = !used;
        used = true;
        first.then_some(primary)
    };
    if !landing.starts.is_empty() {
        out.push(section_title(s::START_SECTION, cx));
        for r in landing.starts {
            let button = row_button(
                format!("landing-start-{}", r.id).into(),
                r.prompt.action_label.clone(),
                Box::new(StartListedEngine {
                    id: r.id.to_string().into(),
                }),
                take(),
                origin,
                cx,
            );
            out.push(row_shell(
                format!("landing-start-row-{}", r.id).into(),
                r.name.clone(),
                r.prompt.title.clone(),
                button,
                cx,
            ));
        }
    }
    if !landing.offers.is_empty() {
        out.push(section_title(s::SETUP_SECTION, cx));
        for o in landing.offers {
            let button = row_button(
                format!("landing-setup-{}", o.id).into(),
                o.action_label.clone(),
                Box::new(SetUpEngine {
                    offer: o.id.clone().into(),
                }),
                take(),
                origin,
                cx,
            );
            out.push(row_shell(
                format!("landing-setup-row-{}", o.id).into(),
                o.title.clone(),
                o.body.clone(),
                button,
                cx,
            ));
        }
    }
    (out, used)
}

fn rows_block(rows: Vec<AnyElement>) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id("landing-rows")
        .debug_selector(|| "landing-rows".into())
        .w(px(560.))
        .max_w_full()
        .gap_2()
        .children(rows)
}

/// First run / no engine (ENG-111, ENG-158): startable engines and setup offers, then
/// per-OS guidance, Rescan, Add engine….
pub fn first_run(
    landing: &Landing,
    primary: &FocusHandle,
    origin: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let (rows, used) = landing_rows(landing, primary, origin, cx);
    let rescan = Button::new("first-run-rescan")
        .when(!used, |b| b.primary())
        .icon(IconName::RefreshCw)
        .label(s::RESCAN)
        .on_click(dispatch::on_click(origin, Box::new(Rescan)));
    let rescan = if used {
        rescan.into_any_element()
    } else {
        primary_wrap(primary, rescan, Box::new(Rescan), origin, cx)
    };
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
        .when(!rows.is_empty(), |this| this.child(rows_block(rows)))
        .child(
            v_flex().gap_1().text_sm().children(
                s::FIRST_RUN_GUIDANCE
                    .iter()
                    .map(|l| div().child(format!("• {l}"))),
            ),
        )
        .child(
            h_flex().gap_2().child(rescan).child(
                Button::new("first-run-add")
                    .label(s::ADD_ENGINE)
                    .on_click(dispatch::on_click(
                        origin,
                        Box::new(crate::actions::settings::AddEngine),
                    )),
            ),
        )
        .into_any_element()
}

/// Engines exist but none is active (ENG-158): the same rows, then *Switch engine*.
pub fn no_active_engine(
    landing: &Landing,
    primary: &FocusHandle,
    origin: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let (rows, used) = landing_rows(landing, primary, origin, cx);
    let pick = Button::new("pick-engine")
        .when(!used, |b| b.primary())
        .label(s::SWITCH_ENGINE)
        .on_click(dispatch::on_click(origin, Box::new(EngineSwitcher)));
    let pick = if used {
        pick.into_any_element()
    } else {
        primary_wrap(primary, pick, Box::new(EngineSwitcher), origin, cx)
    };
    v_flex()
        .id("no-active-engine")
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(
            div()
                .text_lg()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(s::NO_ACTIVE_ENGINE_TITLE),
        )
        .child(div().text_sm().child(s::NO_ACTIVE_ENGINE_BODY))
        .when(!rows.is_empty(), |this| this.child(rows_block(rows)))
        .child(pick)
        .into_any_element()
}

/// The start button's label: the backend's own wording, else the generic one (ENG-139).
pub fn start_label(status: &EngineStatus) -> String {
    status.start_prompt.as_ref().map_or_else(
        || s::START_AND_CONNECT.to_owned(),
        |p| p.action_label.clone(),
    )
}

/// A stopped engine the backend can start: a calm empty state, not an error (ENG-139).
/// `starting` disables the button and shows its progress.
pub fn start_page(
    prompt: &StartPrompt,
    starting: bool,
    origin: &FocusHandle,
    start_focus: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let activate = {
        let origin = origin.clone();
        move |_: &gpui_kit::KeyDownEvent, window: &mut gpui_kit::Window, cx: &mut App| {
            dispatch::dispatch_from(&origin, &StartEngine, window, cx)
        }
    };
    v_flex()
        .id("engine-stopped")
        .debug_selector(|| "engine-stopped".into())
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(crate::assets::Lucide::Plug).size(px(48.))),
        )
        .child(
            div()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(prompt.title.clone()),
        )
        .child(
            div()
                .max_w(px(480.))
                .text_sm()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(prompt.body.clone()),
        )
        .child(
            h_flex()
                .gap_2()
                .child(focus_wrap(
                    "start-engine-wrap",
                    start_focus,
                    Button::new("start-engine")
                        .primary()
                        .label(if starting {
                            s::STARTING.to_owned()
                        } else {
                            prompt.action_label.clone()
                        })
                        .loading(starting)
                        .disabled(starting)
                        .on_click(dispatch::on_click(origin, Box::new(StartEngine))),
                    activate,
                    cx,
                ))
                .child(
                    Button::new("switch-engine")
                        .label(s::SWITCH_ENGINE)
                        .tooltip_with_action(s::SWITCH_ENGINE, &EngineSwitcher, None)
                        .on_click(dispatch::on_click(origin, Box::new(EngineSwitcher))),
                ),
        )
        .into_any_element()
}
