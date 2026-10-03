//! Settings › Engines (ENG-104, ENG-109, ENG-110, SET-010, KBD-075).
//!
//! Every engine the hub lists, hidden ones included (marked), with: inline-editable name
//! (`update_engine`), endpoint, origin, kind label, state dot, version and transport (WSLC:
//! transport + fallback note, ENG-110), *Enabled* (ENG-025), *Test connection*, *Start &
//! connect* (stopped WSL distros, ENG-106), *Hide*/*Unhide* (discovered), *Remove* (manual,
//! confirmed), and "also reachable via" with *Un-merge* (ENG-009). The discovery toggles
//! (ENG-109), *Rescan*, and *Add engine…* sit on top. Every control is a Tab stop in visual
//! order; buttons dispatch [`EngineOp`] so clicks and keys share one handler.

use std::collections::HashMap;

use dk_core::{EngineError, EngineId, EngineOrigin, EngineState, EngineStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement,
    SharedString, Subscription, Task, Window, div, px,
};

use super::SettingsPage;
use super::controls::BoolKey;
use crate::actions::Rescan;
use crate::actions::settings::{AddEngine, EngineOp, EngineOpKind};
use crate::shell::switcher::kind_icon;
use crate::state::{AppState, EngineListStore};
use crate::strings as s;
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::notify;
use crate::ui::status_chip::{dot, engine_dot_color, engine_state_label};

/// Result of *Test connection* for one engine.
#[derive(Debug, Clone, PartialEq)]
pub enum TestState {
    Running,
    Ok(SharedString),
    Failed {
        message: SharedString,
        hint: Option<SharedString>,
    },
}

impl TestState {
    pub fn from_result(name: &str, r: &Result<dk_core::EngineInfo, EngineError>) -> Self {
        match r {
            Ok(i) => TestState::Ok(
                s::test_ok(
                    name,
                    &i.server_version,
                    i.api_version.as_deref(),
                    &i.os,
                    &i.arch,
                )
                .into(),
            ),
            Err(e) => TestState::Failed {
                message: e.to_string().into(),
                hint: e.hint().map(|h| h.to_owned().into()),
            },
        }
    }
}

/// UI state of the Engines section (lives on the page so it survives section switches).
pub struct EnginesUi {
    name_inputs: HashMap<EngineId, Entity<InputState>>,
    name_subs: HashMap<EngineId, Subscription>,
    tests: HashMap<EngineId, TestState>,
    /// Revision per engine: a newer test replaces the older result (stale guard).
    test_revs: HashMap<EngineId, u64>,
    test_tasks: HashMap<EngineId, Task<()>>,
    /// Optimistic *Enabled* value until the hub reports it (SHL-001: feedback < 100 ms).
    pending_enabled: HashMap<EngineId, bool>,
    op_tasks: HashMap<EngineId, Task<()>>,
    rescan_task: Option<Task<()>>,
    rescanning: bool,
    /// Focus target after a dialog (Add engine) or a removal (KBD-007).
    add_focus: FocusHandle,
    next_rev: u64,
}

impl EnginesUi {
    pub fn new(cx: &mut Context<SettingsPage>) -> Self {
        Self {
            name_inputs: HashMap::new(),
            name_subs: HashMap::new(),
            tests: HashMap::new(),
            test_revs: HashMap::new(),
            test_tasks: HashMap::new(),
            pending_enabled: HashMap::new(),
            op_tasks: HashMap::new(),
            rescan_task: None,
            rescanning: false,
            add_focus: cx.focus_handle().tab_stop(true),
            next_rev: 0,
        }
    }

    pub fn name_input(&self, id: &EngineId) -> Option<&Entity<InputState>> {
        self.name_inputs.get(id)
    }
    pub fn test_state(&self, id: &EngineId) -> Option<&TestState> {
        self.tests.get(id)
    }
    pub fn add_focus(&self) -> &FocusHandle {
        &self.add_focus
    }
    pub fn rescanning(&self) -> bool {
        self.rescanning
    }

    /// One name input per listed engine; follows renames made elsewhere unless focused.
    pub fn sync_name_inputs(
        &mut self,
        list: &Entity<EngineListStore>,
        window: &mut Window,
        cx: &mut Context<SettingsPage>,
    ) {
        let engines: Vec<(EngineId, String)> = list
            .read(cx)
            .engines()
            .iter()
            .map(|e| (e.id().clone(), e.config.name.clone()))
            .collect();
        for (id, name) in &engines {
            match self.name_inputs.get(id) {
                Some(input) => {
                    let focused = input.focus_handle(cx).is_focused(window);
                    if !focused && input.read(cx).value().as_ref() != name.as_str() {
                        let name = name.clone();
                        input.update(cx, |i, cx| i.set_value(name, window, cx));
                    }
                }
                None => {
                    let name = name.clone();
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(name)
                            .placeholder(s::ENGINE_NAME)
                    });
                    let eid = id.clone();
                    let sub = cx.subscribe_in(&input, window, move |this, input, e, window, cx| {
                        if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            let v = input.read(cx).value().to_string();
                            rename(this, &eid, &v, window, cx);
                        }
                    });
                    self.name_inputs.insert(id.clone(), input);
                    self.name_subs.insert(id.clone(), sub);
                }
            }
        }
        self.name_inputs
            .retain(|id, _| engines.iter().any(|(e, _)| e == id));
        self.name_subs
            .retain(|id, _| engines.iter().any(|(e, _)| e == id));
        // The hub caught up with an optimistic toggle.
        let list = list.read(cx);
        self.pending_enabled
            .retain(|id, v| list.get(id).is_some_and(|e| e.config.enabled != *v));
    }

    fn enabled(&self, e: &EngineStatus) -> bool {
        self.pending_enabled
            .get(e.id())
            .copied()
            .unwrap_or(e.config.enabled)
    }
}

// ── ops ─────────────────────────────────────────────────────────────────────────────────

fn status_of(this: &SettingsPage, id: &EngineId, cx: &App) -> Option<EngineStatus> {
    this.engine_list.read(cx).get(id).cloned()
}

/// Runs a hub call for an engine; errors become a notification (SHL-003).
fn run_op(
    this: &mut SettingsPage,
    id: &EngineId,
    title: &'static str,
    call: dk_hub::HubCall<()>,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    let eid = id.clone();
    let task = cx.spawn_in(window, async move |this, cx| {
        let r = call.await;
        this.update_in(cx, |this, window, cx| {
            if let Err(e) = r {
                this.engines.pending_enabled.remove(&eid);
                notify::engine_error(window, cx, title, &e);
            }
            cx.notify();
        })
        .ok();
    });
    this.engines.op_tasks.insert(id.clone(), task);
}

fn rename(
    this: &mut SettingsPage,
    id: &EngineId,
    value: &str,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    let Some(status) = status_of(this, id, cx) else {
        return;
    };
    let name = value.trim();
    if name.is_empty() || name == status.config.name {
        // Nothing to save: show the current name again.
        if let Some(i) = this.engines.name_inputs.get(id) {
            let current = status.config.name.clone();
            i.update(cx, |i, cx| i.set_value(current, window, cx));
        }
        return;
    }
    let mut cfg = status.config;
    cfg.name = name.to_owned();
    let call = AppState::hub(cx).update_engine(cfg);
    run_op(this, id, s::RENAME_ENGINE, call, window, cx);
}

fn test(
    this: &mut SettingsPage,
    status: EngineStatus,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    let id = status.id().clone();
    this.engines.next_rev += 1;
    let rev = this.engines.next_rev;
    this.engines.test_revs.insert(id.clone(), rev);
    this.engines.tests.insert(id.clone(), TestState::Running);
    let name = status.config.name.clone();
    let call = AppState::hub(cx).test_engine(status.config);
    let eid = id.clone();
    let task = cx.spawn_in(window, async move |this, cx| {
        let r = call.await;
        this.update(cx, |this, cx| {
            if this.engines.test_revs.get(&eid) != Some(&rev) {
                return;
            }
            this.engines
                .tests
                .insert(eid.clone(), TestState::from_result(&name, &r));
            cx.notify();
        })
        .ok();
    });
    this.engines.test_tasks.insert(id, task);
    cx.notify();
}

fn remove(
    _this: &mut SettingsPage,
    status: EngineStatus,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    let page = cx.entity().downgrade();
    let name = status.config.name.clone();
    let id = status.id().clone();
    let spec = ConfirmSpec::new(s::remove_engine_title(&name))
        .body(s::REMOVE_ENGINE_BODY)
        .items([status.config.endpoint.display()])
        .confirm_label(s::REMOVE);
    confirm_destructive(spec, window, cx, move |_, window, cx| {
        let id = id.clone();
        let name = name.clone();
        page.update(cx, |this, cx| {
            // Focus target once the row is gone (KBD-007): the next engine's name, or
            // *Add engine…*.
            let list = this.engine_list.read(cx).engines().to_vec();
            let ix = list.iter().position(|e| e.id() == &id);
            let next = ix
                .and_then(|ix| {
                    list.get(ix + 1)
                        .or_else(|| ix.checked_sub(1).and_then(|p| list.get(p)))
                })
                .map(|e| e.id().clone());
            let call = AppState::hub(cx).remove_engine(&id);
            let eid = id.clone();
            let task = cx.spawn_in(window, async move |this, cx| {
                let r = call.await;
                this.update_in(cx, |this, window, cx| {
                    match r {
                        Ok(()) => {
                            let target = next
                                .and_then(|n| this.engines.name_inputs.get(&n))
                                .map(|i| i.focus_handle(cx))
                                .unwrap_or_else(|| this.engines.add_focus.clone());
                            window.focus(&target, cx);
                            notify::success(window, cx, s::engine_removed(&name));
                        }
                        Err(e) => notify::engine_error(window, cx, s::REMOVE_ENGINE, &e),
                    }
                    cx.notify();
                })
                .ok();
            });
            this.engines.op_tasks.insert(eid, task);
        })
        .ok();
    });
}

/// [`EngineOp`] handler (row buttons, KBD-075).
pub(super) fn on_engine_op(
    this: &mut SettingsPage,
    a: &EngineOp,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    let id = EngineId::new(a.id.to_string());
    if a.op == EngineOpKind::Unmerge {
        AppState::update_config(cx, |c| {
            if !c.engines.unmerged.contains(&id) {
                c.engines.unmerged.push(id.clone());
            }
        });
        cx.notify();
        return;
    }
    let Some(status) = status_of(this, &id, cx) else {
        return;
    };
    let hub = AppState::hub(cx);
    match a.op {
        EngineOpKind::Test => test(this, status, window, cx),
        EngineOpKind::ToggleEnabled => {
            let mut cfg = status.config.clone();
            cfg.enabled = !this.engines.enabled(&status);
            this.engines.pending_enabled.insert(id.clone(), cfg.enabled);
            let call = hub.update_engine(cfg);
            run_op(this, &id, s::UPDATE_ENGINE, call, window, cx);
        }
        EngineOpKind::ToggleHidden => {
            let mut cfg = status.config;
            cfg.hidden = !cfg.hidden;
            let call = hub.update_engine(cfg);
            run_op(this, &id, s::UPDATE_ENGINE, call, window, cx);
        }
        EngineOpKind::Remove => remove(this, status, window, cx),
        EngineOpKind::StartAndConnect => {
            let call = hub.start_wsl_distro(&id);
            run_op(this, &id, s::START_AND_CONNECT, call, window, cx);
        }
        EngineOpKind::Unmerge => {}
    }
    cx.notify();
}

/// *Rescan* while the page is open: shows progress on the button.
pub(super) fn on_rescan(
    this: &mut SettingsPage,
    _: &Rescan,
    window: &mut Window,
    cx: &mut Context<SettingsPage>,
) {
    if this.engines.rescanning {
        return;
    }
    this.engines.rescanning = true;
    let call = AppState::hub(cx).rescan();
    this.engines.rescan_task = Some(cx.spawn_in(window, async move |this, cx| {
        let r = call.await;
        this.update_in(cx, |this, window, cx| {
            this.engines.rescanning = false;
            if let Err(e) = r {
                notify::engine_error(window, cx, s::RESCAN, &e);
            }
            cx.notify();
        })
        .ok();
    }));
    cx.notify();
}

// ── rendering ───────────────────────────────────────────────────────────────────────────

fn dispatch_op(
    nav: &FocusHandle,
    id: &EngineId,
    op: EngineOpKind,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let id: SharedString = id.to_string().into();
    super::dispatch_here(nav, EngineOp { id, op })
}

fn eid(prefix: &str, id: &EngineId) -> SharedString {
    format!("{prefix}-{id}").into()
}

fn engine_row(
    this: &SettingsPage,
    e: &EngineStatus,
    merged: &[(EngineId, String)],
    cx: &App,
) -> AnyElement {
    let id = e.id();
    let t = cx.theme();
    let enabled = this.engines.enabled(e);
    let manual = e.config.origin == EngineOrigin::Manual;
    let unsupported = matches!(e.state, EngineState::Unsupported { .. });
    let kind = e.config.endpoint.kind();
    let kind_label = kind.map(|k| k.label()).unwrap_or(s::GROUP_OTHER);
    let test = this.engines.tests.get(id).cloned();
    let testing = matches!(test, Some(TestState::Running));
    let name_input = this.engines.name_inputs.get(id).cloned();
    let nav = this.focus.clone();

    let header = h_flex()
        .gap_2()
        .items_center()
        .child(dot(engine_dot_color(&e.state, cx)))
        .child(Icon::new(kind_icon(kind)).small())
        .child(match name_input {
            Some(i) => div()
                .w(px(240.))
                .child(Input::new(&i).small())
                .into_any_element(),
            None => div().child(e.config.name.clone()).into_any_element(),
        })
        .child(Tag::secondary().small().child(kind_label))
        .child(Tag::secondary().outline().small().child(if manual {
            s::ORIGIN_MANUAL
        } else {
            s::ORIGIN_DISCOVERED
        }))
        .when(e.active, |this| {
            this.child(Tag::primary().small().child(s::TAG_ACTIVE))
        })
        .when(e.config.hidden, |this| {
            this.child(Tag::warning().small().child(s::TAG_HIDDEN))
        })
        .child(div().flex_1())
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child(s::ENABLED),
                )
                .child(
                    Switch::new(eid("eng-enabled", id))
                        .checked(enabled)
                        .small()
                        .accessibility_label(s::ENABLED)
                        .on_click({
                            let f = dispatch_op(&nav, id, EngineOpKind::ToggleEnabled);
                            move |_, window, cx| f(&gpui_kit::ClickEvent::default(), window, cx)
                        }),
                ),
        );

    // Endpoint, state, version, and transport (ENG-110).
    let mut facts = h_flex()
        .gap_2()
        .flex_wrap()
        .items_center()
        .text_xs()
        .text_color(t.muted_foreground)
        .child(
            div()
                .font_family(t.mono_font_family.clone())
                .child(e.config.endpoint.display()),
        )
        .child("·")
        .child(engine_state_label(&e.state));
    if let EngineState::Unsupported { reason } = &e.state {
        facts = facts.child("·").child(reason.clone());
    }
    if let Some(i) = &e.info {
        facts = facts
            .child("·")
            .child(s::engine_version(
                &i.server_version,
                i.api_version.as_deref(),
                &i.os,
                &i.arch,
            ))
            .when_some(i.transport.as_ref(), |this, tr| {
                this.child(Tag::secondary().small().child(s::transport_label(tr)))
            })
            .when_some(i.transport_note.as_ref(), |this, n| {
                this.child(Tag::info().small().child(n.clone()))
            });
    }

    let test_line = test.and_then(|ts| match ts {
        TestState::Running => None,
        TestState::Ok(msg) => Some(
            h_flex()
                .id(eid("eng-test-result", id))
                .gap_1()
                .items_center()
                .text_sm()
                .text_color(t.success)
                .child(Icon::new(IconName::CircleCheck).small())
                .child(msg)
                .into_any_element(),
        ),
        TestState::Failed { message, hint } => Some(
            v_flex()
                .id(eid("eng-test-result", id))
                .gap_0p5()
                .text_sm()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_color(t.danger)
                        .child(Icon::new(IconName::CircleX).small())
                        .child(format!("{}: {message}", s::TEST_FAILED)),
                )
                .when_some(hint, |this, h| {
                    this.child(div().text_color(t.muted_foreground).child(h))
                })
                .into_any_element(),
        ),
    });

    let reach = (!e.also_reachable_via.is_empty()).then(|| {
        h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .text_xs()
            .text_color(t.muted_foreground)
            .child(s::also_reachable_via(&e.also_reachable_via.join(", ")))
            .children(merged.iter().map(|(mid, name)| {
                Button::new(eid("eng-unmerge", mid))
                    .xsmall()
                    .outline()
                    .label(format!("{} {name}", s::UNMERGE))
                    .on_click(dispatch_op(&nav, mid, EngineOpKind::Unmerge))
            }))
    });

    let actions = h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new(eid("eng-test", id))
                .small()
                .outline()
                .icon(crate::assets::Lucide::Plug)
                .label(if testing {
                    s::TESTING
                } else {
                    s::TEST_CONNECTION
                })
                .loading(testing)
                .disabled(unsupported)
                .on_click(dispatch_op(&nav, id, EngineOpKind::Test)),
        )
        .when(matches!(e.state, EngineState::Stopped), |this| {
            this.child(
                Button::new(eid("eng-start", id))
                    .small()
                    .outline()
                    .icon(IconName::Play)
                    .label(s::START_AND_CONNECT)
                    .on_click(dispatch_op(&nav, id, EngineOpKind::StartAndConnect)),
            )
        })
        .when(!manual, |this| {
            this.child(
                Button::new(eid("eng-hide", id))
                    .small()
                    .ghost()
                    .icon(if e.config.hidden {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .label(if e.config.hidden { s::UNHIDE } else { s::HIDE })
                    .on_click(dispatch_op(&nav, id, EngineOpKind::ToggleHidden)),
            )
        })
        .when(manual, |this| {
            this.child(
                Button::new(eid("eng-remove", id))
                    .small()
                    .ghost()
                    .icon(IconName::Delete)
                    .label(s::REMOVE)
                    .on_click(dispatch_op(&nav, id, EngineOpKind::Remove)),
            )
        });

    GroupBox::new()
        .outline()
        .id(eid("eng-row", id))
        .child(
            v_flex()
                .gap_2()
                .when(unsupported || !enabled, |this| this.opacity(0.75))
                .child(header)
                .child(facts)
                .children(reach)
                .children(test_line)
                .child(actions),
        )
        .into_any_element()
}

/// The section's content blocks (one per engine, so the focused one scrolls into view).
pub(super) fn blocks(this: &mut SettingsPage, cx: &mut Context<SettingsPage>) -> Vec<AnyElement> {
    let rescanning = this.engines.rescanning;
    let discovery = crate::ui::section(
        s::ENGINES_DISCOVERY,
        v_flex()
            .gap_3()
            .child(this.bool_row(
                BoolKey::ShowAllDistros,
                s::SET_SHOW_ALL_DISTROS,
                s::SET_SHOW_ALL_DISTROS_DESC,
                cx,
            ))
            .child(this.bool_row(
                BoolKey::ShowAllSessions,
                s::SET_SHOW_ALL_SESSIONS,
                s::SET_SHOW_ALL_SESSIONS_DESC,
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("eng-rescan")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(s::RESCAN)
                            .loading(rescanning)
                            .tooltip_with_action(s::RESCAN, &Rescan, None)
                            .on_click(super::dispatch_here(&this.focus, Rescan)),
                    )
                    .child(crate::ui::widgets::focus_wrap(
                        "eng-add-wrap",
                        &this.engines.add_focus,
                        Button::new("eng-add")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label(s::ADD_ENGINE)
                            .on_click(super::dispatch_here(&this.focus, AddEngine)),
                        |_, w, cx| w.dispatch_action(Box::new(AddEngine), cx),
                        cx,
                    )),
            ),
        cx,
    )
    .into_any_element();
    let list = this.engine_list.read(cx);
    let engines: Vec<EngineStatus> = list.engines().to_vec();
    let merged: Vec<Vec<(EngineId, String)>> =
        engines.iter().map(|e| list.merged_into(e)).collect();
    let mut out = vec![discovery];
    if engines.is_empty() {
        out.push(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(s::NO_ENGINES)
                .into_any_element(),
        );
    }
    for (e, m) in engines.iter().zip(merged.iter()) {
        out.push(engine_row(this, e, m, cx));
    }
    out
}
