//! Container detail page (`docs/spec/features/container-detail.md`, CDT-001…082; logs,
//! terminal and stats specs; keyboard.md KBD-040…070).
//!
//! Structure:
//! - [`ContainerDetailPage`] (this file): the routed page. Header (breadcrumb, status, image
//!   link, short id, ports, health, actions), the `TabBar`, the removed banner, and the tab
//!   entities, which stay alive while switching tabs on the same container (CDT-081).
//! - [`state::ContainerDetailState`]: inspect data + engine events for this id (CDT-003/080).
//! - Tabs: [`overview`], [`mounts`], [`network`] (focusable [`rows`] panels, KBD-044),
//!   [`inspect`] (code editor), [`logs`], [`terminal`] (+ [`crate::state::TerminalRegistry`]),
//!   [`stats`].
//!
//! Header actions reuse `pages::containers::ops` and the shared confirmations (SHL-002).
//! The single-letter header keys live in the `DetailHeader` key context (KBD-041), which the
//! header, the tab bar, and the non-input tab bodies carry.

pub mod inspect;
pub mod logs;
pub mod mounts;
pub mod network;
pub mod overview;
pub mod rows;
pub mod state;
pub mod stats;
pub mod terminal;

use dk_core::{Capabilities, ContainerState, ContainerSummary, EngineError};
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Subscription, Task, Window, div,
};

use crate::actions::detail_tab::SelectTab;
use crate::actions::res::ReplaceRoute;
use crate::actions::{Navigate, OpenUrl, container, detail, list};
use crate::assets::Lucide;
use crate::keymap::ctx;
use crate::nav::{ContainerTab, ImageTab, Route};
use crate::pages::containers::ops::{self, Op};
use crate::pages::resources::detail::{TabSpec, segmented_tabs, tab_bar_frame};
use crate::state::{AppState, EngineStore};
use crate::strings as s;
use crate::ui::breadcrumb::{Crumb, breadcrumb};
use crate::ui::confirm::{ConfirmSpec, confirm_destructive, should_confirm_stopped_delete};
use crate::ui::dispatch;
use crate::ui::menu::TrackBounds as _;
use crate::ui::menu::{KeyMenu, MenuAnchor};
use crate::ui::notify;
use crate::ui::page::PageView;
use crate::ui::status_chip::{container_chip, health_chip};
use crate::ui::widgets::{focus_wrap, port_link, port_url};

pub use state::{ContainerDetailState, DetailEvent};

/// The tab entities (CDT-081): created lazily on first visit, kept while the page lives.
#[derive(Default)]
pub struct Tabs {
    pub overview: Option<Entity<overview::OverviewTab>>,
    pub logs: Option<Entity<logs::LogsView>>,
    pub terminal: Option<Entity<terminal::TerminalTab>>,
    pub stats: Option<Entity<stats::StatsTab>>,
    pub mounts: Option<Entity<mounts::MountsTab>>,
    pub network: Option<Entity<network::NetworkTab>>,
    pub inspect: Option<Entity<inspect::InspectTab>>,
}

pub struct ContainerDetailPage {
    id: String,
    tab: ContainerTab,
    state: Option<Entity<ContainerDetailState>>,
    store: Option<Entity<EngineStore>>,
    tabs: Tabs,
    /// Focus target for the tab bar (KBD-040/061, primary focus after navigation).
    tab_bar_focus: FocusHandle,
    breadcrumb_focus: FocusHandle,
    image_focus: FocusHandle,
    copy_focus: FocusHandle,
    more_focus: FocusHandle,
    more_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    menu: Option<KeyMenu>,
    /// In-flight header action (SHL-001: spinner within 100 ms).
    pending: Option<&'static str>,
    action_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

/// Creates the page for `Route::ContainerDetail { id, tab }`.
pub fn new(
    id: String,
    tab: ContainerTab,
    store: Option<Entity<EngineStore>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContainerDetailPage> {
    cx.new(|cx| ContainerDetailPage::new(id, tab, store, window, cx))
}

impl ContainerDetailPage {
    pub fn new(
        id: String,
        tab: ContainerTab,
        store: Option<Entity<EngineStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = AppState::hub(cx);
        let state = store.as_ref().map(|s| {
            let engine = s.read(cx).engine_id().clone();
            let store = s.clone();
            let id = id.clone();
            cx.new(|cx| ContainerDetailState::new(hub, engine, id, Some(store), cx))
        });
        let mut subs = Vec::new();
        if let Some(st) = &state {
            subs.push(cx.observe(st, |_, _, cx| cx.notify()));
            subs.push(cx.subscribe_in(st, window, Self::on_detail_event));
        }
        if let Some(store) = &store {
            subs.push(cx.observe(store, |_, _, cx| cx.notify()));
        }
        let mut this = Self {
            id,
            tab,
            state,
            store,
            tabs: Tabs::default(),
            tab_bar_focus: cx.focus_handle().tab_stop(true),
            breadcrumb_focus: cx.focus_handle().tab_stop(true),
            image_focus: cx.focus_handle().tab_stop(true),
            copy_focus: cx.focus_handle().tab_stop(true),
            more_focus: cx.focus_handle().tab_stop(true),
            more_bounds: Default::default(),
            menu: None,
            pending: None,
            action_task: None,
            _subs: subs,
        };
        this.ensure_tab(tab, window, cx);
        this
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn container_id(&self) -> &str {
        &self.id
    }

    pub fn tab(&self) -> ContainerTab {
        self.tab
    }

    pub fn detail_state(&self) -> Option<&Entity<ContainerDetailState>> {
        self.state.as_ref()
    }

    pub fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    pub fn tab_bar_focus(&self) -> &FocusHandle {
        &self.tab_bar_focus
    }

    /// The `EngineStore` the page was built on (the shell remounts on engine switch).
    pub fn store_id(&self) -> Option<gpui_kit::EntityId> {
        self.store.as_ref().map(|s| s.entity_id())
    }

    pub fn is_removed(&self, cx: &App) -> bool {
        self.state.as_ref().is_some_and(|s| s.read(cx).is_removed())
    }

    fn summary(&self, cx: &App) -> Option<ContainerSummary> {
        self.state
            .as_ref()
            .and_then(|s| s.read(cx).summary(cx).cloned())
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.store
            .as_ref()
            .map(|s| s.read(cx).capabilities())
            .unwrap_or_default()
    }

    /// Actions are disabled when removed (CDT-080) or the engine is degraded (SHL-013).
    fn read_only(&self, cx: &App) -> bool {
        self.is_removed(cx) || crate::shell::engine_read_only(cx)
    }

    // ── tabs (CDT-002, CDT-081) ─────────────────────────────────────────────────────────

    /// Called by the shell when only the tab of the route changed (keeps tab state).
    pub fn set_tab(&mut self, tab: ContainerTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.ensure_tab(tab, window, cx);
        self.visibility_changed(cx);
        cx.notify();
    }

    fn ensure_tab(&mut self, tab: ContainerTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let bar = self.tab_bar_focus.clone();
        match tab {
            ContainerTab::Overview if self.tabs.overview.is_none() => {
                self.tabs.overview = Some(cx.new(|cx| overview::OverviewTab::new(state, cx)));
            }
            ContainerTab::Mounts if self.tabs.mounts.is_none() => {
                self.tabs.mounts = Some(cx.new(|cx| mounts::MountsTab::new(state, cx)));
            }
            ContainerTab::Network if self.tabs.network.is_none() => {
                self.tabs.network = Some(cx.new(|cx| network::NetworkTab::new(state, cx)));
            }
            ContainerTab::Inspect if self.tabs.inspect.is_none() => {
                self.tabs.inspect = Some(cx.new(|cx| inspect::InspectTab::new(state, window, cx)));
            }
            ContainerTab::Logs if self.tabs.logs.is_none() => {
                self.tabs.logs = Some(cx.new(|cx| logs::LogsView::new(state, window, cx)));
            }
            ContainerTab::Terminal if self.tabs.terminal.is_none() => {
                self.tabs.terminal =
                    Some(cx.new(|cx| terminal::TerminalTab::new(state, bar, window, cx)));
            }
            ContainerTab::Stats if self.tabs.stats.is_none() => {
                self.tabs.stats = Some(cx.new(|cx| stats::StatsTab::new(state, cx)));
            }
            _ => {}
        }
    }

    /// STA-006: stats stream only while the Stats tab is visible; terminals learn whether
    /// they're shown (focus on open).
    fn visibility_changed(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab;
        if let Some(st) = &self.tabs.stats {
            st.update(cx, |t, cx| t.set_visible(tab == ContainerTab::Stats, cx));
        }
        if let Some(t) = &self.tabs.logs {
            t.update(cx, |t, _| t.set_visible(tab == ContainerTab::Logs));
        }
    }

    fn route_for(&self, tab: ContainerTab) -> Route {
        Route::ContainerDetail {
            id: self.id.clone(),
            tab,
        }
    }

    /// Switch tabs through the route (CDT-002): the shell replaces the history entry and
    /// calls back [`Self::set_tab`].
    fn go_tab(&mut self, tab: ContainerTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        let route = self.route_for(tab);
        window.dispatch_action(Box::new(ReplaceRoute { route }), cx);
        // Apply locally too, so the switch is visible within the frame (SHL-001).
        self.set_tab(tab, window, cx);
    }

    fn step_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let all = ContainerTab::ALL;
        let n = all.len() as isize;
        let ix = all.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        let next = all[((ix + delta).rem_euclid(n)) as usize];
        let bar_focused = self.tab_bar_focus.is_focused(window);
        self.go_tab(next, window, cx);
        if bar_focused {
            window.focus(&self.tab_bar_focus, cx);
            if let Some(t) = &self.tabs.terminal {
                t.update(cx, |t, _| t.keep_tab_bar_focus());
            }
        }
    }

    fn on_next_tab(&mut self, _: &detail::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step_tab(1, window, cx);
    }

    fn on_prev_tab(&mut self, _: &detail::PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step_tab(-1, window, cx);
    }

    fn on_select_tab(&mut self, a: &SelectTab, window: &mut Window, cx: &mut Context<Self>) {
        self.go_tab(a.tab, window, cx);
    }

    /// `Mod+Shift+F6` from the terminal (KBD-061).
    fn on_leave_terminal(
        &mut self,
        _: &dk_terminal::actions::LeaveTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.tab_bar_focus, cx);
        cx.notify();
    }

    // ── header actions (CDT-001 → CON-020, KBD-041) ────────────────────────────────────

    fn run_op(&mut self, op: Op, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let Some(summary) = self.summary(cx) else {
            return;
        };
        if matches!(op, Op::Pause | Op::Unpause) && !self.caps(cx).contains(Capabilities::PAUSE) {
            return; // capability-gated (ENG-030)
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let hub = AppState::hub(cx);
        let engine = store.read(cx).engine_id().clone();
        let verb = op.verb();
        let name = summary.name.clone();
        let is_remove = matches!(op, Op::Remove { .. });
        self.pending = Some(verb);
        cx.notify();
        let call = ops::run(&hub, &engine, op, vec![summary]);
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                this.pending = None;
                let err = match result {
                    Ok(outcome) => outcome.into_iter().find_map(|o| o.2.err()),
                    Err(e) => Some(e),
                };
                match err {
                    Some(e) => notify::engine_error(window, cx, s::action_failed(verb, &name), &e),
                    None if is_remove => {
                        // Back to the list, focusing the neighbour (KBD-007 via the list).
                        window.dispatch_action(
                            Box::new(Navigate {
                                route: Route::Containers,
                            }),
                            cx,
                        );
                    }
                    None => {}
                }
                if let Some(st) = &this.state {
                    st.update(cx, |s, cx| s.refresh(cx));
                }
                store.update(cx, |s, cx| {
                    s.refetch(crate::state::Collection::Containers, cx)
                });
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_start_stop(&mut self, _: &container::StartStop, w: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.summary(cx) else { return };
        let op = if c.state.is_running() || c.state == ContainerState::Paused {
            Op::Stop
        } else {
            Op::Start
        };
        self.run_op(op, w, cx);
    }

    fn on_restart(&mut self, _: &container::Restart, w: &mut Window, cx: &mut Context<Self>) {
        self.run_op(Op::Restart, w, cx);
    }

    fn on_pause(&mut self, _: &container::PauseToggle, w: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.summary(cx) else { return };
        match c.state {
            ContainerState::Paused => self.run_op(Op::Unpause, w, cx),
            s if s.is_running() => self.run_op(Op::Pause, w, cx),
            _ => {}
        }
    }

    fn on_kill(&mut self, _: &container::Kill, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.summary(cx) else { return };
        if self.read_only(cx) || !(c.state.is_running() || c.state == ContainerState::Paused) {
            return;
        }
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::CONFIRM_KILL_TITLE)
                .body(s::CONFIRM_KILL_BODY)
                .items([c.name.clone()])
                .confirm_label(s::ACTION_KILL),
            window,
            cx,
            move |_, window, cx| {
                page.update(cx, |p, cx| p.run_op(Op::Kill, window, cx)).ok();
            },
        );
    }

    /// `Del` (CON-021 rules): running needs Force; stopped confirms unless disabled.
    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.summary(cx) else { return };
        if self.read_only(cx) {
            return;
        }
        let page = cx.entity().downgrade();
        let running = c.state.is_running() || c.state == ContainerState::Paused;
        if running {
            confirm_destructive(
                ConfirmSpec::new(s::CONFIRM_FORCE_RUNNING_TITLE)
                    .body(s::confirm_force_running_body(1))
                    .items([c.name.clone()])
                    .confirm_label(s::FORCE_DELETE),
                window,
                cx,
                move |_, window, cx| {
                    page.update(cx, |p, cx| p.run_op(Op::Remove { force: true }, window, cx))
                        .ok();
                },
            );
            return;
        }
        if !should_confirm_stopped_delete(cx) {
            self.run_op(Op::Remove { force: false }, window, cx);
            return;
        }
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_title(1))
                .items([c.name.clone()])
                .option(s::DONT_ASK_AGAIN, false),
            window,
            cx,
            move |options, window, cx| {
                if options.first().copied().unwrap_or(false) {
                    AppState::update_config(cx, |c| c.general.confirm_delete_stopped = false);
                }
                page.update(cx, |p, cx| {
                    p.run_op(Op::Remove { force: false }, window, cx)
                })
                .ok();
            },
        );
    }

    fn on_logs(&mut self, _: &container::Logs, w: &mut Window, cx: &mut Context<Self>) {
        self.go_tab(ContainerTab::Logs, w, cx);
    }

    fn on_terminal(&mut self, _: &container::Terminal, w: &mut Window, cx: &mut Context<Self>) {
        if self.caps(cx).contains(Capabilities::EXEC_TTY) {
            self.go_tab(ContainerTab::Terminal, w, cx);
        }
    }

    fn on_inspect(&mut self, _: &container::Inspect, w: &mut Window, cx: &mut Context<Self>) {
        self.go_tab(ContainerTab::Inspect, w, cx);
    }

    fn on_open_port(&mut self, _: &container::OpenPort, w: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.summary(cx) else { return };
        match c.ports.iter().find_map(port_url) {
            Some(url) => w.dispatch_action(Box::new(OpenUrl { url: url.into() }), cx),
            None => notify::info(w, cx, s::NO_PUBLISHED_PORT),
        }
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .summary(cx)
            .map(|c| c.id)
            .unwrap_or_else(|| self.id.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(id));
        notify::info(window, cx, s::COPIED);
    }

    fn open_more_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let caps = self.caps(cx);
        let ro = self.read_only(cx);
        let state = self.summary(cx).map(|c| c.state);
        let running = state.is_some_and(|s| s.is_running());
        let paused = state == Some(ContainerState::Paused);
        let has_port = self
            .summary(cx)
            .is_some_and(|c| c.ports.iter().any(|p| port_url(p).is_some()));
        let external = self
            .detail_state()
            .is_some_and(|s| s.read(cx).external_terminal_available(cx));
        let pos = MenuAnchor::below_or(
            self.more_bounds,
            gpui_kit::point(gpui_kit::px(640.), gpui_kit::px(110.)),
        );
        let restore = self.more_focus.clone();
        self.menu = Some(KeyMenu::open(
            pos,
            restore,
            window,
            cx,
            move |menu, _, _| {
                let hint = |m: gpui_kit::component::menu::PopupMenu,
                            label: &'static str,
                            action: Box<dyn gpui_kit::Action>,
                            disabled: bool| {
                    let keys = crate::keymap::hint_for(action.name(), ctx::DETAIL_HEADER);
                    m.menu_element_with_disabled(action, disabled, move |_, cx| {
                        h_flex()
                            .w_full()
                            .gap_4()
                            .justify_between()
                            .child(label)
                            .when_some(keys.clone(), |this, k| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(k),
                                )
                            })
                    })
                };
                let mut m = menu;
                if caps.contains(Capabilities::PAUSE) {
                    let label = if paused {
                        s::ACTION_UNPAUSE
                    } else {
                        s::ACTION_PAUSE
                    };
                    m = hint(
                        m,
                        label,
                        Box::new(container::PauseToggle),
                        ro || !(running || paused),
                    );
                }
                m = hint(
                    m,
                    s::ACTION_KILL,
                    Box::new(container::Kill),
                    ro || !(running || paused),
                );
                m = hint(
                    m,
                    s::ACTION_OPEN_PORT,
                    Box::new(container::OpenPort),
                    !has_port,
                );
                m = hint(m, s::COPY_ID, Box::new(list::CopyId), false);
                if external {
                    m = hint(
                        m,
                        s::TERMINAL_EXTERNAL,
                        Box::new(crate::actions::term_ext::OpenExternal),
                        ro || !running,
                    );
                }
                m.separator()
                    .menu_with_disabled(s::ACTION_DELETE, Box::new(list::Delete), ro)
            },
            |this: &mut Self, _| this.menu = None,
        ));
        cx.notify();
    }

    // ── events ─────────────────────────────────────────────────────────────────────────

    fn on_detail_event(
        &mut self,
        _: &Entity<ContainerDetailState>,
        event: &DetailEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *event == DetailEvent::Removed {
            // CDT-080 / TRM-008: the container's terminals close.
            if let Some(store) = &self.store {
                let engine = store.read(cx).engine_id().clone();
                crate::state::TerminalRegistry::close_container(&engine, &self.id, cx);
            }
            cx.notify();
        }
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let summary = self.summary(cx);
        let removed = self.is_removed(cx);
        let ro = self.read_only(cx);
        let caps = self.caps(cx);
        let loading = self
            .state
            .as_ref()
            .is_some_and(|s| s.read(cx).details.is_loading());
        let name: SharedString = summary
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| dk_core::format::short_id(&self.id).to_owned())
            .into();
        let mut crumbs = vec![Crumb::link(s::PAGE_CONTAINERS, Route::Containers)];
        if let Some(project) = summary.as_ref().and_then(|c| c.compose.as_ref()) {
            crumbs.push(Crumb::here(project.project.clone()));
        }
        crumbs.push(Crumb::here(name.clone()));
        let running = summary.as_ref().is_some_and(|c| c.state.is_running());
        let paused = summary
            .as_ref()
            .is_some_and(|c| c.state == ContainerState::Paused);
        let dot_color = match summary.as_ref().map(|c| c.state) {
            Some(ContainerState::Running) => cx.theme().success,
            Some(ContainerState::Paused) => cx.theme().warning,
            Some(ContainerState::Restarting) => cx.theme().info,
            Some(ContainerState::Exited | ContainerState::Dead)
                if summary.as_ref().and_then(|c| c.exit_code).unwrap_or(0) != 0 =>
            {
                cx.theme().danger
            }
            _ => cx.theme().muted_foreground,
        };
        let pending = self.pending;
        let busy = |verb: &str| pending == Some(verb);

        let image_link = summary.as_ref().map(|c| {
            let route = Route::ImageDetail {
                id: c.image_id.clone(),
                tab: ImageTab::Overview,
            };
            let nav = Navigate { route };
            let nav2 = nav.clone();
            focus_wrap(
                "detail-image-wrap",
                &self.image_focus,
                Button::new("detail-image")
                    .link()
                    .small()
                    .label(format!("{} ↗", c.image))
                    .tooltip(s::OPEN_IMAGE)
                    .on_click(dispatch::on_click(&self.image_focus, Box::new(nav))),
                move |_, w, cx| w.dispatch_action(Box::new(nav2.clone()), cx),
                cx,
            )
        });
        let full_id = summary
            .as_ref()
            .map(|c| c.id.clone())
            .unwrap_or_else(|| self.id.clone());
        let ports: Vec<AnyElement> = summary
            .as_ref()
            .map(|c| {
                c.ports
                    .iter()
                    .filter(|p| p.public.is_some())
                    .enumerate()
                    .map(|(i, p)| port_link(("detail-port", i), p, cx))
                    .collect()
            })
            .unwrap_or_default();
        let copy_text: SharedString = full_id.clone().into();
        let copy = focus_wrap(
            "detail-copy-wrap",
            &self.copy_focus,
            Button::new("detail-copy-id")
                .ghost()
                .xsmall()
                .icon(IconName::Copy)
                .tooltip_with_action(s::COPY_ID, &list::CopyId, Some(ctx::DETAIL_HEADER))
                .on_click(dispatch::on_click(
                    &self.copy_focus,
                    Box::new(crate::actions::CopyText { text: copy_text }),
                )),
            |_, w, cx| w.dispatch_action(Box::new(list::CopyId), cx),
            cx,
        );
        // Header clicks dispatch from the breadcrumb's focus node, inside this page (KBD-002).
        let origin = self.breadcrumb_focus.clone();
        let action_btn = |id: &'static str,
                          icon: gpui_kit::component::Icon,
                          label: &'static str,
                          action: Box<dyn gpui_kit::Action>,
                          disabled: bool,
                          loading: bool| {
            let a = action.boxed_clone();
            Button::new(id)
                .small()
                .outline()
                .icon(icon)
                .label(label)
                .loading(loading)
                .disabled(disabled)
                .tooltip_with_action(label, action.as_ref(), Some(ctx::DETAIL_HEADER))
                .on_click(dispatch::on_click(&origin, a))
        };
        let more_focus_ring = self.more_focus.clone();
        let actions = h_flex()
            .gap_1()
            .items_center()
            .map(|this| {
                if running || paused {
                    this.child(action_btn(
                        "detail-stop",
                        Lucide::Square.into(),
                        s::ACTION_STOP,
                        Box::new(container::StartStop),
                        ro,
                        busy("stop"),
                    ))
                    .child(action_btn(
                        "detail-restart",
                        IconName::RotateCw.into(),
                        s::ACTION_RESTART,
                        Box::new(container::Restart),
                        ro,
                        busy("restart"),
                    ))
                } else {
                    this.child(action_btn(
                        "detail-start",
                        IconName::Play.into(),
                        s::ACTION_START,
                        Box::new(container::StartStop),
                        ro || summary.is_none(),
                        busy("start"),
                    ))
                }
            })
            .child(
                focus_wrap(
                    "detail-more-wrap",
                    &more_focus_ring,
                    Button::new("detail-more")
                        .small()
                        .ghost()
                        .icon(IconName::EllipsisVertical)
                        .tooltip(s::MORE_ACTIONS)
                        .disabled(summary.is_none())
                        .on_click(cx.listener(|this, _, w, cx| this.open_more_menu(w, cx))),
                    {
                        let this = cx.entity().downgrade();
                        move |_, w, cx| {
                            this.update(cx, |p, cx| p.open_more_menu(w, cx)).ok();
                        }
                    },
                    cx,
                )
                .on_bounds({
                    let this = cx.entity().downgrade();
                    move |b, _, cx| {
                        this.update(cx, |p, _| p.more_bounds = b).ok();
                    }
                }),
            )
            .child(
                Button::new("detail-delete")
                    .small()
                    .danger()
                    .icon(Lucide::Trash)
                    .tooltip_with_action(s::ACTION_DELETE, &list::Delete, Some(ctx::DETAIL_HEADER))
                    .loading(busy("delete"))
                    .disabled(ro || summary.is_none())
                    .on_click(dispatch::on_click(&origin, Box::new(list::Delete))),
            );
        let _ = caps;

        v_flex()
            .id("detail-header")
            .gap_2()
            .child(
                div()
                    .id("detail-breadcrumb")
                    .track_focus(&self.breadcrumb_focus)
                    .rounded(cx.theme().radius)
                    .px_1()
                    // KBD-042: Enter on the focused breadcrumb goes to the parent list.
                    .on_key_down(|e: &gpui_kit::KeyDownEvent, w, cx| {
                        if e.keystroke.key == "enter" && !e.keystroke.modifiers.modified() {
                            w.dispatch_action(Box::new(detail::ParentList), cx);
                            cx.stop_propagation();
                        }
                    })
                    .child(breadcrumb(crumbs, cx)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .flex_wrap()
                    .child(crate::ui::status_chip::dot(dot_color))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .children(image_link)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .child(dk_core::format::short_id(&full_id).to_owned()),
                            )
                            .child(copy),
                    )
                    .children(ports)
                    .when(loading, |this| this.child(Spinner::new().small()))
                    .child(div().flex_1())
                    .child(actions),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .when_some(summary.as_ref(), |this, c| {
                        this.child(container_chip(c.state, c.exit_code))
                            .when_some(c.health, |this, h| this.child(health_chip(h)))
                            .child(c.status_text.clone())
                    }),
            )
            .when(removed, |this| {
                this.child(
                    Alert::warning("detail-removed", s::CONTAINER_GONE)
                        .small()
                        .title(s::DETAIL_READ_ONLY),
                )
                .child(
                    h_flex().child(
                        Button::new("detail-back-to-list")
                            .small()
                            .label(s::BACK_TO_LIST)
                            .on_click(dispatch::on_click(
                                &self.breadcrumb_focus,
                                Box::new(Navigate {
                                    route: Route::Containers,
                                }),
                            )),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let selected = ContainerTab::ALL
            .iter()
            .position(|t| *t == self.tab)
            .unwrap_or(0);
        let caps = self.caps(cx);
        let tabs = ContainerTab::ALL
            .iter()
            .map(|t| {
                // Terminal is capability-gated (ENG-030).
                let disabled =
                    *t == ContainerTab::Terminal && !caps.contains(Capabilities::EXEC_TTY);
                tab_spec(*t).disabled(disabled)
            })
            .collect();
        let bar = segmented_tabs(
            "detail-tabs",
            tabs,
            selected,
            cx.listener(|this, ix: &usize, w, cx| {
                if let Some(tab) = ContainerTab::ALL.get(*ix).copied() {
                    this.go_tab(tab, w, cx);
                    w.focus(&this.tab_bar_focus, cx);
                }
            }),
        );
        tab_bar_frame(
            "detail-tab-bar",
            &format!("{} {}", ctx::DETAIL_HEADER, ctx::DETAIL_TABS),
            &self.tab_bar_focus,
            bar,
            cx,
        )
        .into_any_element()
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.state.clone() else {
            return crate::ui::skeleton_rows(6, 3);
        };
        let st = state.read(cx);
        // Four states (SHL-004): first load → skeleton; error without data → panel + Retry.
        let has_data = st.details.data().is_some();
        if !has_data && !st.is_removed() {
            if let Some(err) = st.details.error().cloned() {
                return detail_error(&err, cx);
            }
            if matches!(
                self.tab,
                ContainerTab::Overview
                    | ContainerTab::Mounts
                    | ContainerTab::Network
                    | ContainerTab::Inspect
            ) {
                return crate::ui::skeleton_rows(8, 3);
            }
        }
        match self.tab {
            ContainerTab::Overview => self.tabs.overview.clone().map(|e| e.into_any_element()),
            ContainerTab::Logs => self.tabs.logs.clone().map(|e| e.into_any_element()),
            ContainerTab::Terminal => self.tabs.terminal.clone().map(|e| e.into_any_element()),
            ContainerTab::Stats => self.tabs.stats.clone().map(|e| e.into_any_element()),
            ContainerTab::Mounts => self.tabs.mounts.clone().map(|e| e.into_any_element()),
            ContainerTab::Network => self.tabs.network.clone().map(|e| e.into_any_element()),
            ContainerTab::Inspect => self.tabs.inspect.clone().map(|e| e.into_any_element()),
        }
        .unwrap_or_else(|| crate::ui::skeleton_rows(6, 3))
    }
}

/// Icon + label of a container detail tab (SHL-025).
fn tab_spec(t: ContainerTab) -> TabSpec {
    let icon: gpui_kit::component::Icon = match t {
        ContainerTab::Overview => IconName::LayoutDashboard.into(),
        ContainerTab::Logs => Lucide::ScrollText.into(),
        ContainerTab::Terminal => IconName::SquareTerminal.into(),
        ContainerTab::Stats => Lucide::Activity.into(),
        ContainerTab::Mounts => IconName::HardDrive.into(),
        ContainerTab::Network => IconName::Network.into(),
        ContainerTab::Inspect => Lucide::Braces.into(),
    };
    TabSpec::new(t.label(), icon)
}

fn detail_error(err: &EngineError, cx: &App) -> AnyElement {
    crate::ui::error_panel(
        "detail-error",
        s::CONTAINER_DETAIL_LOAD_FAILED,
        err,
        Box::new(crate::actions::Refresh),
        cx,
    )
}

impl Focusable for ContainerDetailPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.tab_bar_focus.clone()
    }
}

impl PageView for ContainerDetailPage {
    /// The first tab stop of the page content (KBD-007): the tab bar.
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.tab_bar_focus.clone()
    }

    /// `Mod+F` (KBD-025): the in-view search in Logs and Inspect.
    fn focus_search(&mut self, window: &mut Window, cx: &mut App) {
        match self.tab {
            ContainerTab::Logs => {
                if let Some(l) = &self.tabs.logs {
                    l.update(cx, |l, cx| l.focus_search(window, cx));
                    return;
                }
            }
            ContainerTab::Inspect => {
                if let Some(i) = &self.tabs.inspect {
                    i.update(cx, |i, cx| i.open_search(window, cx));
                    return;
                }
            }
            _ => {}
        }
        window.focus(&self.tab_bar_focus, cx);
    }
}

impl Render for ContainerDetailPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let tab_bar = self.render_tab_bar(cx);
        let body = self.render_body(cx);
        v_flex()
            .id("container-detail")
            .size_full()
            .px_4()
            .pt_3()
            .gap_2()
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_select_tab))
            .on_action(cx.listener(Self::on_leave_terminal))
            .on_action(cx.listener(Self::on_start_stop))
            .on_action(cx.listener(Self::on_restart))
            .on_action(cx.listener(Self::on_pause))
            .on_action(cx.listener(Self::on_kill))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_logs))
            .on_action(cx.listener(Self::on_terminal))
            .on_action(cx.listener(Self::on_inspect))
            .on_action(cx.listener(Self::on_open_port))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(|this, _: &crate::actions::Refresh, _, cx| {
                // F5 / Mod+R (KBD-026): refetch inspect, then let the shell refresh lists.
                if let Some(st) = &this.state {
                    st.update(cx, |s, cx| s.refresh(cx));
                }
                cx.propagate();
            }))
            .child(
                div()
                    .id("detail-header-region")
                    .key_context(ctx::DETAIL_HEADER)
                    .child(header),
            )
            .child(tab_bar)
            .child(div().flex_1().min_h_0().pb_2().child(body))
            .when_some(self.menu.as_ref(), |this, m| this.child(m.render()))
    }
}
