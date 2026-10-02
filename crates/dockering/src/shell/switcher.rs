//! Engine switcher popover (`Mod+K`, ENG-101/KBD-021/075): grouped Local / WSL distros /
//! WSL containers / Remote; status dot, version, OS/arch tooltip, "also reachable via"
//! (ENG-009), unsupported entries greyed with the reason (ENG-112), stopped WSL distros
//! offer *Start & connect* (ENG-106); filter as you type; arrows + Enter; Rescan and
//! Manage engines… in the footer.

use dk_core::{EngineGroup, EngineKind, EngineState, EngineStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Window, div, px,
};

use crate::actions::{ManageEngines, Rescan, switcher};
use crate::keymap::ctx;
use crate::state::EngineListStore;
use crate::strings as s;
use crate::ui::status_chip::{dot, engine_dot_color, engine_state_label};

/// Icon per kind — labels/icons are the only place `EngineKind` is matched (ENG-030).
pub fn kind_icon(kind: Option<EngineKind>) -> IconName {
    match kind {
        Some(EngineKind::Docker) => IconName::Frame,
        Some(EngineKind::WslDistro) => IconName::SquareTerminal,
        Some(EngineKind::Wslc) => IconName::LayoutDashboard,
        _ => IconName::Globe,
    }
}

pub fn group_label(g: EngineGroup) -> &'static str {
    match g {
        EngineGroup::Local => s::GROUP_LOCAL,
        EngineGroup::WslDistros => s::GROUP_WSL_DISTROS,
        EngineGroup::WslContainers => s::GROUP_WSL_CONTAINERS,
        EngineGroup::Remote => s::GROUP_REMOTE,
        EngineGroup::Other => s::GROUP_OTHER,
    }
}

/// What choosing an entry does.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Switch(SharedString),
    /// Boot a stopped WSL distro, then connect (ENG-106).
    StartAndConnect(SharedString),
    /// Unsupported: not selectable.
    None,
}

pub fn choice_for(e: &EngineStatus) -> Choice {
    let id: SharedString = e.id().to_string().into();
    match &e.state {
        EngineState::Unsupported { .. } | EngineState::Disabled => Choice::None,
        EngineState::Stopped => Choice::StartAndConnect(id),
        _ => Choice::Switch(id),
    }
}

/// Emitted to the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum SwitcherEvent {
    Chosen(Choice),
    Dismissed,
}

pub struct EngineSwitcher {
    list: Entity<EngineListStore>,
    filter: Entity<InputState>,
    query: String,
    cursor: usize,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SwitcherEvent> for EngineSwitcher {}

impl EngineSwitcher {
    pub fn new(list: Entity<EngineListStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(s::FILTER_ENGINES));
        let subs = vec![
            cx.subscribe_in(&filter, window, |this, input, e: &InputEvent, _, cx| {
                if let InputEvent::Change = e {
                    this.query = input.read(cx).value().to_lowercase();
                    this.cursor = 0;
                    cx.notify();
                }
            }),
            cx.observe(&list, |_, _, cx| cx.notify()),
        ];
        // Start on the active engine.
        let mut this = Self {
            list,
            filter,
            query: String::new(),
            cursor: 0,
            focus: cx.focus_handle(),
            _subs: subs,
        };
        let active = this.entries(cx).iter().position(|e| e.active);
        this.cursor = active.unwrap_or(0);
        this
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |i, cx| i.focus(window, cx));
    }

    /// Visible entries in group order, filtered.
    pub fn entries(&self, cx: &App) -> Vec<EngineStatus> {
        let mut v: Vec<EngineStatus> = self
            .list
            .read(cx)
            .engines()
            .iter()
            .filter(|e| !e.config.hidden)
            .filter(|e| {
                self.query.is_empty()
                    || e.config.name.to_lowercase().contains(&self.query)
                    || e.config
                        .endpoint
                        .display()
                        .to_lowercase()
                        .contains(&self.query)
            })
            .cloned()
            .collect();
        v.sort_by_key(|e| e.config.group());
        v
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(e) = self.entries(cx).get(ix).cloned() else {
            return;
        };
        let c = choice_for(&e);
        if c != Choice::None {
            cx.emit(SwitcherEvent::Chosen(c));
        }
    }

    fn on_up(&mut self, _: &switcher::Up, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.cursor.saturating_sub(1);
        cx.notify();
    }
    fn on_down(&mut self, _: &switcher::Down, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.entries(cx).len();
        if n > 0 {
            self.cursor = (self.cursor + 1).min(n - 1);
        }
        cx.notify();
    }
    fn on_choose(&mut self, _: &switcher::Choose, _: &mut Window, cx: &mut Context<Self>) {
        self.choose(self.cursor, cx);
    }
    fn on_close(&mut self, _: &switcher::Close, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SwitcherEvent::Dismissed);
    }

    #[allow(dead_code)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

impl Focusable for EngineSwitcher {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.focus_handle(cx)
    }
}

impl Render for EngineSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries(cx);
        let mut last_group = None;
        let mut children = Vec::new();
        for (ix, e) in entries.iter().enumerate() {
            let g = e.config.group();
            if last_group != Some(g) {
                last_group = Some(g);
                children.push(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child(group_label(g))
                        .into_any_element(),
                );
            }
            let selected = ix == self.cursor;
            let choice = choice_for(e);
            let disabled = choice == Choice::None;
            let version = e
                .info
                .as_ref()
                .map(|i| format!("{} {}", i.kind.label(), i.server_version))
                .unwrap_or_default();
            let tooltip = {
                let mut t = e.config.endpoint.display();
                if let Some(i) = &e.info {
                    t.push_str(&format!("\n{}/{}", i.os, i.arch));
                    if let Some(tr) = &i.transport {
                        t.push_str(&format!(" · {}", s::transport_label(tr)));
                    }
                }
                if !e.also_reachable_via.is_empty() {
                    t.push('\n');
                    t.push_str(&s::also_reachable_via(&e.also_reachable_via.join(", ")));
                }
                if let EngineState::Unsupported { reason } = &e.state {
                    t.push('\n');
                    t.push_str(reason);
                }
                t
            };
            let state_label = engine_state_label(&e.state);
            let dot_color = engine_dot_color(&e.state, cx);
            let kind = e.config.endpoint.kind();
            children.push(
                h_flex()
                    .id(("engine-row", ix))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .items_center()
                    .rounded(cx.theme().radius)
                    .when(selected, |this| this.bg(cx.theme().accent))
                    .when(disabled, |this| this.opacity(0.5))
                    .when(!disabled, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose(ix, cx);
                            }))
                    })
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                            .build(window, cx)
                    })
                    .child(Icon::new(kind_icon(kind)).small())
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_sm().child(e.config.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(version),
                            ),
                    )
                    .when(matches!(choice, Choice::StartAndConnect(_)), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().link)
                                .child(s::START_AND_CONNECT),
                        )
                    })
                    .child(dot(dot_color))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(state_label),
                    )
                    .into_any_element(),
            );
        }
        if entries.is_empty() {
            children.push(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::NO_MATCHING_ENGINES)
                    .into_any_element(),
            );
        }
        v_flex()
            .id("engine-switcher")
            .key_context(ctx::SWITCHER)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_choose))
            .on_action(cx.listener(Self::on_close))
            .w(px(380.))
            .gap_1()
            .child(Input::new(&self.filter).small())
            .child(
                v_flex()
                    .id("engine-rows")
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .children(children),
            )
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("switcher-rescan")
                            .small()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label(s::RESCAN)
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(Rescan), cx)),
                    )
                    .child(
                        Button::new("switcher-manage")
                            .small()
                            .ghost()
                            .icon(IconName::Settings)
                            .label(s::MANAGE_ENGINES)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ManageEngines), cx)
                            }),
                    ),
            )
    }
}
