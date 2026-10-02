//! Image pulls (IMG-004, IMG-007) run in an app-wide [`PullManager`] global entity, so closing
//! the dialog or leaving the page never cancels them (SHL-012). Each pull is a
//! `hub.subscribe(engine, |e| e.pull_image(..))` stream consumed by a task stored here;
//! dropping the task (Cancel) drops the stream, which cancels the producer on the hub.
//!
//! Progress shows in one notification per pull (`Notification::id1::<PullNote>(id)`, pushed
//! again to update in place): per-layer bars when the engine has `PULL_PROGRESS`, otherwise a
//! status line. Updates are batched (at most every 100 ms). Registry credentials are resolved
//! by the backend; nothing here logs or stores them (NFR-020).

use std::collections::BTreeMap;
use std::time::Duration;

use dk_core::{Capabilities, EngineError, EngineId, PullProgress};
use futures::StreamExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, Global, IntoElement, SharedString, Task, div,
};

use super::ops::pull_error_message;
use crate::state::AppState;
use crate::strings as s;
use crate::ui::notify;

/// How often a running pull repaints its notification.
pub const PULL_FLUSH: Duration = Duration::from_millis(100);
/// At most this many layer bars are shown (the rest are summarised).
pub const MAX_LAYERS: usize = 8;

/// Notification id type for pull toasts.
pub struct PullNote;

/// One layer's latest state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerState {
    pub status: String,
    pub current: Option<u64>,
    pub total: Option<u64>,
}

impl LayerState {
    /// 0–100, if the layer reported sizes.
    pub fn percent(&self) -> Option<f32> {
        match (self.current, self.total) {
            (Some(c), Some(t)) if t > 0 => Some((c.min(t) as f32 / t as f32) * 100.0),
            _ if self.status.contains("complete") || self.status.contains("exists") => Some(100.0),
            _ => None,
        }
    }
}

/// Phase of a pull.
#[derive(Debug, Clone, PartialEq)]
pub enum PullPhase {
    Running,
    Done { digest: Option<String> },
    Failed(String),
    Cancelled,
}

/// Snapshot of a pull, shown in its notification.
#[derive(Debug, Clone, PartialEq)]
pub struct PullState {
    pub id: u64,
    pub engine: EngineId,
    pub reference: String,
    pub structured: bool,
    pub status: String,
    pub layers: BTreeMap<String, LayerState>,
    pub layer_order: Vec<String>,
    pub phase: PullPhase,
}

impl PullState {
    fn new(id: u64, engine: EngineId, reference: String, structured: bool) -> Self {
        Self {
            id,
            engine,
            status: s::pulling(&reference),
            reference,
            structured,
            layers: BTreeMap::new(),
            layer_order: Vec::new(),
            phase: PullPhase::Running,
        }
    }

    /// Folds one progress item in.
    pub fn apply(&mut self, p: PullProgress) {
        match p {
            PullProgress::Status(text) => self.status = text,
            PullProgress::Layer {
                id,
                status,
                current,
                total,
            } => {
                if !self.layers.contains_key(&id) {
                    self.layer_order.push(id.clone());
                }
                if !self.structured {
                    self.status = s::layer_status(&id, &status);
                }
                self.layers.insert(
                    id,
                    LayerState {
                        status,
                        current,
                        total,
                    },
                );
            }
            PullProgress::Done { digest } => self.phase = PullPhase::Done { digest },
        }
    }
}

struct PullJob {
    state: PullState,
    window: AnyWindowHandle,
    task: Option<Task<()>>,
    dirty: bool,
}

/// App-wide owner of running pulls.
pub struct PullManager {
    next_id: u64,
    jobs: BTreeMap<u64, PullJob>,
    flush: Option<Task<()>>,
    /// Finished pulls (tests, diagnostics).
    finished: Vec<PullState>,
}

struct GlobalPulls(Entity<PullManager>);
impl Global for GlobalPulls {}

impl PullManager {
    /// The app-wide manager, created on first use.
    pub fn global(cx: &mut App) -> Entity<PullManager> {
        if let Some(g) = cx.try_global::<GlobalPulls>() {
            return g.0.clone();
        }
        let e = cx.new(|_| PullManager {
            next_id: 0,
            jobs: BTreeMap::new(),
            flush: None,
            finished: Vec::new(),
        });
        cx.set_global(GlobalPulls(e.clone()));
        e
    }

    pub fn try_global(cx: &App) -> Option<Entity<PullManager>> {
        cx.try_global::<GlobalPulls>().map(|g| g.0.clone())
    }

    pub fn running(&self) -> Vec<&PullState> {
        self.jobs.values().map(|j| &j.state).collect()
    }

    pub fn finished(&self) -> &[PullState] {
        &self.finished
    }

    /// Starts pulling `reference` on `engine`; progress goes to `window`'s notifications.
    /// `on_done(ok)` runs on the main thread when the pull ends (e.g. refetch images).
    pub fn start(
        &mut self,
        engine: EngineId,
        reference: String,
        caps: Capabilities,
        window: AnyWindowHandle,
        on_done: impl FnOnce(bool, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let structured = caps.contains(Capabilities::PULL_PROGRESS);
        let hub = AppState::hub(cx);
        let r = reference.clone();
        let mut stream = hub.subscribe(&engine, move |e| e.pull_image(&r, None));
        let state = PullState::new(id, engine, reference, structured);
        let task = cx.spawn(async move |this, cx| {
            let mut outcome: Option<Result<(), EngineError>> = None;
            while let Some(item) = stream.next().await {
                let keep = this
                    .update(cx, |this, cx| {
                        let Some(job) = this.jobs.get_mut(&id) else {
                            return false;
                        };
                        match item {
                            Ok(p) => {
                                job.state.apply(p);
                                job.dirty = true;
                                this.schedule_flush(cx);
                                true
                            }
                            Err(e) => {
                                outcome = Some(Err(e));
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                let ok = match outcome {
                    Some(Err(e)) => {
                        this.finish(id, Some(e), cx);
                        false
                    }
                    _ => {
                        this.finish(id, None, cx);
                        true
                    }
                };
                on_done(ok, cx);
            })
            .ok();
        });
        self.jobs.insert(
            id,
            PullJob {
                state,
                window,
                task: Some(task),
                dirty: false,
            },
        );
        let manager = cx.entity().downgrade();
        window
            .update(cx, |_, window, cx| {
                window.push_notification(progress_note(id, manager), cx);
            })
            .ok();
        cx.notify();
        id
    }

    /// *Cancel*: drops the stream (cancels the hub producer) and reports it.
    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(mut job) = self.jobs.remove(&id) else {
            return;
        };
        job.task = None;
        job.state.phase = PullPhase::Cancelled;
        let window = job.window;
        let state = job.state.clone();
        self.finished.push(state);
        window
            .update(cx, |_, window, cx| {
                window.remove_notification1::<PullNote>(id as usize, cx);
                notify::info(window, cx, s::PULL_CANCELLED);
            })
            .ok();
    }

    fn finish(&mut self, id: u64, error: Option<EngineError>, cx: &mut Context<Self>) {
        let Some(mut job) = self.jobs.remove(&id) else {
            return;
        };
        let reference = job.state.reference.clone();
        job.state.phase = match &error {
            Some(e) => PullPhase::Failed(pull_error_message(&reference, e)),
            None => match &job.state.phase {
                PullPhase::Done { digest } => PullPhase::Done {
                    digest: digest.clone(),
                },
                _ => PullPhase::Done { digest: None },
            },
        };
        let window = job.window;
        let phase = job.state.phase.clone();
        self.finished.push(job.state);
        window
            .update(cx, |_, window, cx| {
                window.remove_notification1::<PullNote>(id as usize, cx);
                match phase {
                    PullPhase::Failed(msg) => {
                        notify::error(window, cx, s::pull_failed(&reference), msg)
                    }
                    _ => notify::success(window, cx, s::pulled(&reference)),
                }
            })
            .ok();
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if self.flush.is_some() {
            return;
        }
        self.flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PULL_FLUSH).await;
            this.update(cx, |this, cx| {
                this.flush = None;
                this.flush_now(cx);
            })
            .ok();
        }));
    }

    /// Repaints the progress notifications: their content reads this entity, so one
    /// `notify` per batch updates every bar in place (no re-push, no re-animation).
    fn flush_now(&mut self, cx: &mut Context<Self>) {
        for job in self.jobs.values_mut() {
            job.dirty = false;
        }
        cx.notify();
    }

    /// Live state of a running pull.
    pub fn state(&self, id: u64) -> Option<&PullState> {
        self.jobs.get(&id).map(|j| &j.state)
    }
}

/// The progress notification of one pull: title, status line, per-layer bars (structured
/// progress only), and a *Cancel* action.
fn progress_note(id: u64, manager: gpui_kit::WeakEntity<PullManager>) -> Notification {
    let content_manager = manager.clone();
    Notification::new()
        .id1::<PullNote>(id as usize)
        .autohide(false)
        .content(move |_, _, cx| {
            let Some(state) = content_manager
                .upgrade()
                .and_then(|m| m.read(cx).state(id).cloned())
            else {
                return div().into_any_element();
            };
            let muted = cx.theme().muted_foreground;
            let layers: Vec<(String, LayerState)> = state
                .layer_order
                .iter()
                .filter_map(|l| state.layers.get(l).map(|s| (l.clone(), s.clone())))
                .collect();
            let hidden = layers.len().saturating_sub(MAX_LAYERS);
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .truncate()
                        .child(s::pulling(&state.reference)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(state.status.clone()),
                )
                .when(state.structured, |this| {
                    this.children(layers.into_iter().take(MAX_LAYERS).map(|(lid, l)| {
                        let pct = l.percent();
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .truncate()
                                    .child(s::layer_status(&lid, &l.status)),
                            )
                            .child(
                                Progress::new(SharedString::from(format!("pull-{id}-{lid}")))
                                    .xsmall()
                                    .loading(pct.is_none())
                                    .value(pct.unwrap_or(0.0))
                                    .accessibility_label(s::layer_status(&lid, &l.status)),
                            )
                    }))
                    .when(hidden > 0, |this| {
                        this.child(div().text_xs().text_color(muted).child(s::and_more(hidden)))
                    })
                })
                .into_any_element()
        })
        .action(move |_, _, _| {
            let manager = manager.clone();
            Button::new(SharedString::from(format!("pull-cancel-{id}")))
                .small()
                .ghost()
                .label(s::CANCEL)
                .tooltip(s::PULL_CANCEL)
                .on_click(move |_, _, cx| {
                    manager.update(cx, |m, cx| m.cancel(id, cx)).ok();
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_004_progress_folds_layers_and_done() {
        let mut st = PullState::new(1, EngineId::new("e"), "alpine:3.20".into(), true);
        st.apply(PullProgress::Status("Pulling from library/alpine".into()));
        st.apply(PullProgress::Layer {
            id: "a1".into(),
            status: "Downloading".into(),
            current: Some(25),
            total: Some(100),
        });
        st.apply(PullProgress::Layer {
            id: "b2".into(),
            status: "Waiting".into(),
            current: None,
            total: None,
        });
        assert_eq!(st.layer_order, ["a1", "b2"]);
        assert_eq!(st.layers["a1"].percent(), Some(25.0));
        assert_eq!(st.layers["b2"].percent(), None);
        st.apply(PullProgress::Layer {
            id: "a1".into(),
            status: "Pull complete".into(),
            current: None,
            total: None,
        });
        assert_eq!(st.layers["a1"].percent(), Some(100.0));
        st.apply(PullProgress::Done {
            digest: Some("sha256:x".into()),
        });
        assert_eq!(
            st.phase,
            PullPhase::Done {
                digest: Some("sha256:x".into())
            }
        );
    }

    #[test]
    fn img_004_unstructured_progress_is_a_status_line() {
        let mut st = PullState::new(1, EngineId::new("e"), "alpine".into(), false);
        st.apply(PullProgress::Layer {
            id: "a1".into(),
            status: "Downloading".into(),
            current: None,
            total: None,
        });
        assert_eq!(st.status, "a1: Downloading");
    }
}
