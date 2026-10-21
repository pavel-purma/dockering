//! The *Set up Docker in WSL* dialog (ENG-151, 154, 162): choice and consent, progress, error.
//!
//! The choices come from the offer (`SetupChoice` data) as a radio group with nothing selected
//! and no default: *Set up* stays off until the user picks. Closing the dialog in any stage
//! drops the stream task, which cancels the setup (ENG-154).

use dk_core::format::format_size;
use dk_core::{EngineError, EngineResult, SetupOffer, SetupPhase, SetupProgress};
use dk_hub::Feed;
use futures::StreamExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AppContext, Context, Entity, EventEmitter, FocusHandle, IntoElement, Render, SharedString,
    Task, Window, div, px,
};

use crate::actions::choice;
use crate::keymap::ctx;
use crate::state::{AppState, SetupStore};
use crate::strings as s;
use crate::ui::notify;
use crate::ui::widgets::focus_wrap;

/// What the dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Choice and consent.
    Choose,
    Running {
        phase: SetupPhase,
        fraction: Option<f32>,
        detail: String,
    },
    Failed {
        message: String,
        hint: Option<String>,
        /// An info message (another setup is running), not a failure: no *Try again*.
        info: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupDialogEvent {
    /// The dialog is gone (closed, cancelled, or done).
    Closed,
}

impl EventEmitter<SetupDialogEvent> for SetupDialog {}

pub struct SetupDialog {
    offer: SetupOffer,
    selected: Option<usize>,
    stage: Stage,
    /// Id of the current run; events carrying another id are dropped (stale guard).
    setup_id: u64,
    task: Option<Task<()>>,
    group_focus: FocusHandle,
    setup_focus: FocusHandle,
    cancel_focus: FocusHandle,
    retry_focus: FocusHandle,
    close_focus: FocusHandle,
    focus_pending: bool,
}

impl SetupDialog {
    /// Opens the dialog over the current window. Focus returns to the invoker on close.
    pub fn open(offer: SetupOffer, window: &mut Window, cx: &mut gpui_kit::App) -> Entity<Self> {
        let title: SharedString = offer.title.clone().into();
        let view = cx.new(|cx| Self {
            offer,
            selected: None,
            stage: Stage::Choose,
            setup_id: 0,
            task: None,
            group_focus: cx.focus_handle().tab_stop(true),
            setup_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            retry_focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            focus_pending: true,
        });
        let (on_ok, on_cancel, body) = (view.clone(), view.clone(), view.clone());
        window.open_dialog(cx, move |dialog, _, _| {
            let (on_ok, on_cancel) = (on_ok.clone(), on_cancel.clone());
            dialog
                .title(title.clone())
                .w(px(520.))
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    on_ok.update(cx, |d, cx| d.submit_key(window, cx));
                    false
                })
                .on_cancel(move |_, _, cx| {
                    on_cancel.update(cx, |d, cx| d.stop(cx));
                    true
                })
                .child(body.clone())
        });
        view
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn stage(&self) -> &Stage {
        &self.stage
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn setup_id(&self) -> u64 {
        self.setup_id
    }
    pub fn group_focus(&self) -> &FocusHandle {
        &self.group_focus
    }
    pub fn setup_focus(&self) -> &FocusHandle {
        &self.setup_focus
    }
    pub fn cancel_focus(&self) -> &FocusHandle {
        &self.cancel_focus
    }
    pub fn offer(&self) -> &SetupOffer {
        &self.offer
    }

    /// The download and disk size lines of the selected choice; none until one is picked.
    pub fn consent_lines(&self) -> Option<(String, String)> {
        let c = self.selected.and_then(|i| self.offer.choices.get(i))?;
        Some((
            s::setup_download_size(&format_size(c.download_bytes)),
            s::setup_disk_size(&format_size(c.disk_bytes)),
        ))
    }

    /// *Set up* is enabled: a choice is picked, or the offer has none to pick.
    pub fn can_run(&self) -> bool {
        matches!(self.stage, Stage::Choose)
            && (self.offer.choices.is_empty() || self.selected.is_some())
    }

    // ── state changes ──────────────────────────────────────────────────────────────────

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if matches!(self.stage, Stage::Choose) && ix < self.offer.choices.len() {
            self.selected = Some(ix);
            cx.notify();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.offer.choices.len();
        if n == 0 {
            return;
        }
        let ix = match (self.selected, delta > 0) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), _) => (i as isize + delta).clamp(0, n as isize - 1) as usize,
        };
        self.select(ix, cx);
    }

    /// Drops the stream (cancelling the setup) and tells the owner the dialog is gone.
    fn stop(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.setup_id += 1;
        cx.emit(SetupDialogEvent::Closed);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.stop(cx);
    }

    /// *Try again*: back to the unselected choice screen.
    pub fn try_again(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.setup_id += 1;
        self.selected = None;
        self.stage = Stage::Choose;
        self.focus_pending = true;
        cx.notify();
    }

    /// *Set up*: nothing is started before this (ENG-151).
    pub fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_run() {
            return;
        }
        let choice = self
            .selected
            .and_then(|i| self.offer.choices.get(i))
            .map(|c| c.id.clone())
            .unwrap_or_default();
        self.setup_id += 1;
        let id = self.setup_id;
        self.stage = Stage::Running {
            phase: SetupPhase::Checking,
            fraction: None,
            detail: String::new(),
        };
        self.focus_pending = true;
        let mut stream = AppState::hub(cx).run_setup(&self.offer.id, &choice);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(item) = stream.next().await {
                // A failed update means the dialog is gone: treat it as a cancel.
                if this
                    .update_in(cx, |this, window, cx| this.apply(id, item, window, cx))
                    .is_err()
                {
                    return;
                }
            }
            this.update_in(cx, |this, _, cx| this.ended(id, cx)).ok();
        }));
        cx.notify();
    }

    /// One event of the run `id`; an older run's events are dropped.
    pub fn apply(
        &mut self,
        id: u64,
        item: EngineResult<Feed<SetupProgress>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id != self.setup_id || !matches!(self.stage, Stage::Running { .. }) {
            return;
        }
        match item {
            Ok(Feed::Item(SetupProgress::Phase {
                phase,
                fraction,
                detail,
            })) => {
                self.stage = Stage::Running {
                    phase,
                    fraction,
                    detail,
                };
            }
            Ok(Feed::Item(SetupProgress::Done(cfg))) => {
                self.task = None;
                window.close_dialog(cx);
                notify::success(window, cx, s::setup_done(&cfg.name));
                if let Some(store) = SetupStore::global(cx) {
                    store.update(cx, |st, cx| st.refresh(cx));
                }
                cx.emit(SetupDialogEvent::Closed);
                return;
            }
            Ok(Feed::Lagged { .. }) => return,
            Err(EngineError::Cancelled) => return,
            Err(EngineError::Conflict(_)) => self.fail(s::SETUP_ALREADY_RUNNING.into(), None, true),
            Err(e) => self.fail(e.to_string(), e.hint().map(str::to_owned), false),
        }
        cx.notify();
    }

    /// The stream ended; fine after `Done`, a failure while still running.
    fn ended(&mut self, id: u64, cx: &mut Context<Self>) {
        if id == self.setup_id && matches!(self.stage, Stage::Running { .. }) {
            self.fail(s::SETUP_ENDED.into(), None, false);
            cx.notify();
        }
    }

    fn fail(&mut self, message: String, hint: Option<String>, info: bool) {
        self.task = None;
        self.focus_pending = true;
        self.stage = Stage::Failed {
            message,
            hint,
            info,
        };
    }

    /// `Enter` (the dialog's confirm): activates the focused footer button.
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.stage {
            Stage::Choose if self.setup_focus.is_focused(window) => self.run(window, cx),
            Stage::Failed { info: false, .. } if self.retry_focus.is_focused(window) => {
                self.try_again(cx)
            }
            _ if self.cancel_focus.is_focused(window) || self.close_focus.is_focused(window) => {
                self.close(window, cx)
            }
            _ => {}
        }
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn close_button(
        id: &'static str,
        wrap: &'static str,
        label: &'static str,
        handle: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        focus_wrap(
            wrap,
            handle,
            Button::new(id)
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            cx.listener(|this, _: &gpui_kit::KeyDownEvent, window, cx| this.close(window, cx)),
            cx,
        )
    }

    fn render_choose(&mut self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let t = cx.theme();
        let muted = t.muted_foreground;
        let mut out: Vec<gpui_kit::AnyElement> = Vec::new();
        if !self.offer.choices.is_empty() {
            let radios = self.offer.choices.iter().map(|c| {
                Radio::new(SharedString::from(c.id.clone()))
                    .label(c.label.clone())
                    .tab_stop(false)
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(c.description.clone()),
                    )
            });
            let group = RadioGroup::new("setup-choices")
                .selected_index(self.selected)
                .on_click(cx.listener(|this, ix: &usize, _, cx| this.select(*ix, cx)))
                .children(radios);
            out.push(
                v_flex()
                    .id("setup-choice-group")
                    .debug_selector(|| "setup-choice-group".into())
                    .key_context(ctx::CHOICE_GROUP)
                    .track_focus(&self.group_focus)
                    .on_action(cx.listener(|this, _: &choice::Prev, _, cx| this.step(-1, cx)))
                    .on_action(cx.listener(|this, _: &choice::Next, _, cx| this.step(1, cx)))
                    .on_action(cx.listener(|this, _: &choice::Select, _, cx| {
                        // Nothing is picked implicitly: `Space` confirms the current pick.
                        if let Some(ix) = this.selected {
                            this.select(ix, cx)
                        }
                    }))
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(s::SETUP_CHOOSE_TITLE),
                    )
                    .child(group)
                    .into_any_element(),
            );
        }
        match self.consent_lines() {
            Some((download, disk)) => out.push(
                v_flex()
                    .id("setup-consent")
                    .gap_1()
                    .child(div().text_sm().child(self.offer.body.clone()))
                    .child(
                        div()
                            .debug_selector(|| "setup-download-size".into())
                            .text_sm()
                            .text_color(muted)
                            .child(download),
                    )
                    .child(
                        div()
                            .debug_selector(|| "setup-disk-size".into())
                            .text_sm()
                            .text_color(muted)
                            .child(disk),
                    )
                    .into_any_element(),
            ),
            None if !self.offer.choices.is_empty() => out.push(
                div()
                    .id("setup-hint")
                    .debug_selector(|| "setup-hint".into())
                    .text_sm()
                    .text_color(muted)
                    .child(s::SETUP_CHOOSE_HINT)
                    .into_any_element(),
            ),
            None => out.push(
                div()
                    .text_sm()
                    .child(self.offer.body.clone())
                    .into_any_element(),
            ),
        }
        out
    }

    fn render_running(
        &self,
        phase: SetupPhase,
        fraction: Option<f32>,
        detail: &str,
        cx: &Context<Self>,
    ) -> Vec<gpui_kit::AnyElement> {
        let muted = cx.theme().muted_foreground;
        vec![
            v_flex()
                .id("setup-progress")
                .debug_selector(|| "setup-progress".into())
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(fraction.is_none(), |this| {
                            this.child(Spinner::new().small())
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(s::setup_phase(phase, fraction)),
                        ),
                )
                .when_some(fraction, |this, f| {
                    this.child(
                        Progress::new("setup-bar")
                            .value(f.clamp(0.0, 1.0) * 100.)
                            .accessibility_label(s::SETUP_PROGRESS),
                    )
                })
                .when(!detail.is_empty(), |this| {
                    this.child(div().text_sm().text_color(muted).child(detail.to_owned()))
                })
                .into_any_element(),
        ]
    }

    fn render_failed(
        &self,
        message: &str,
        hint: Option<&str>,
        info: bool,
        cx: &Context<Self>,
    ) -> Vec<gpui_kit::AnyElement> {
        let alert = if info {
            Alert::info("setup-alert", message.to_owned())
        } else {
            Alert::error("setup-alert", message.to_owned()).title(s::SETUP_FAILED_TITLE)
        };
        let mut out = vec![alert.into_any_element()];
        if let Some(h) = hint {
            out.push(
                div()
                    .id("setup-hint-text")
                    .debug_selector(|| "setup-hint-text".into())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(h.to_owned())
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for SetupDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_pending) {
            let target = match &self.stage {
                Stage::Choose if !self.offer.choices.is_empty() => self.group_focus.clone(),
                Stage::Choose => self.setup_focus.clone(),
                Stage::Running { .. } => self.cancel_focus.clone(),
                Stage::Failed { info: false, .. } => self.retry_focus.clone(),
                Stage::Failed { .. } => self.close_focus.clone(),
            };
            window.focus(&target, cx);
        }
        let stage = self.stage.clone();
        let body = match &stage {
            Stage::Choose => self.render_choose(cx),
            Stage::Running {
                phase,
                fraction,
                detail,
            } => self.render_running(*phase, *fraction, detail, cx),
            Stage::Failed {
                message,
                hint,
                info,
            } => self.render_failed(message, hint.as_deref(), *info, cx),
        };
        // DOM order is Tab order (primary first); the row is reversed so *Cancel* sits left.
        let footer = h_flex().flex_row_reverse().gap_2().items_center();
        let footer = match &stage {
            Stage::Choose => {
                let run = if self.can_run() {
                    focus_wrap(
                        "setup-run-wrap",
                        &self.setup_focus,
                        Button::new("setup-run")
                            .primary()
                            .label(s::SETUP_BUTTON)
                            .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
                        cx.listener(|this, _: &gpui_kit::KeyDownEvent, window, cx| {
                            this.run(window, cx)
                        }),
                        cx,
                    )
                    .into_any_element()
                } else {
                    Button::new("setup-run")
                        .primary()
                        .label(s::SETUP_BUTTON)
                        .disabled(true)
                        .into_any_element()
                };
                footer.child(run).child(Self::close_button(
                    "setup-cancel",
                    "setup-cancel-wrap",
                    s::CANCEL,
                    &self.cancel_focus,
                    cx,
                ))
            }
            Stage::Running { .. } => footer.child(Self::close_button(
                "setup-cancel",
                "setup-cancel-wrap",
                s::CANCEL,
                &self.cancel_focus,
                cx,
            )),
            Stage::Failed { info, .. } => footer
                .when(!info, |f| {
                    f.child(focus_wrap(
                        "setup-retry-wrap",
                        &self.retry_focus,
                        Button::new("setup-retry")
                            .primary()
                            .label(s::SETUP_TRY_AGAIN)
                            .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
                        cx.listener(|this, _: &gpui_kit::KeyDownEvent, _, cx| this.try_again(cx)),
                        cx,
                    ))
                })
                .child(Self::close_button(
                    "setup-close",
                    "setup-close-wrap",
                    s::SETUP_CLOSE,
                    &self.close_focus,
                    cx,
                )),
        };
        v_flex()
            .id("setup-dialog")
            .key_context(ctx::DIALOG)
            .gap_3()
            .children(body)
            .child(footer)
    }
}
