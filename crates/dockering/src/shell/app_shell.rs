//! `AppShell`: the window root view (spec 30 §1). Owns the engine list store, the active
//! `EngineStore`, the navigator history, the current page, focus regions, and the global
//! actions (KBD-017…029).

use std::cell::Cell;
use std::rc::Rc;

use dk_core::{EngineId, EngineState};
use dk_hub::{ThemeMode, UpdateStatus};
use gpui_kit::component::ThemeStyled as _;
use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::CommandState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::sidebar::Sidebar;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, Theme, TitleBar, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyView, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement,
    MouseButton, NavigationDirection, Render, SharedString, Subscription, Task, Window, div, px,
};

use super::engine_views;
use super::menus;
use super::regions::{Region, RegionSlot, Regions};
use super::shortcuts::ShortcutList;
use super::sidebar_nav::{NavItem, NavMenu};
use super::switcher::{Choice, EngineSwitcher as SwitcherView, SwitcherEvent, kind_icon};
use super::{ShellState, palette};
use crate::actions::*;
use crate::commands::CommandContext;
use crate::keymap::ctx;
use crate::nav::{History, Page, Route, SettingsSection};
use crate::pages;
use crate::pages::containers::ContainersPage;
use crate::state::{
    AppState, Collection, EngineListEvent, EngineListStore, EngineStore, EngineStoreEvent,
    ManualCheck, UpdateStore,
};
use crate::strings as s;
use crate::theme;
use crate::ui::menu::TrackBounds as _;
use crate::ui::menu::{KeyMenu, MenuAnchor};
use crate::ui::notify;
use crate::ui::page::PageView;
use crate::ui::status_chip::{dot, engine_dot_color, engine_state_label};

/// The mounted page.
pub enum ShellPage {
    Containers(Entity<ContainersPage>),
    ContainerDetail(Entity<pages::container_detail::ContainerDetailPage>),
    Settings(Entity<pages::settings::SettingsPage>),
    Placeholder(Entity<pages::placeholder::PlaceholderPage>),
    /// M6 pages (Images, Volumes, Networks and their details).
    Dyn(crate::ui::page::DynPage),
    /// Nothing to show (no engine / disconnected): the shell renders a state screen.
    None,
}

impl ShellPage {
    fn view(&self) -> Option<AnyView> {
        match self {
            ShellPage::Containers(e) => Some(e.clone().into()),
            ShellPage::ContainerDetail(e) => Some(e.clone().into()),
            ShellPage::Settings(e) => Some(e.clone().into()),
            ShellPage::Placeholder(e) => Some(e.clone().into()),
            ShellPage::Dyn(p) => Some(p.view()),
            ShellPage::None => None,
        }
    }

    pub fn primary_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self {
            ShellPage::Containers(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::ContainerDetail(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::Settings(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::Placeholder(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::Dyn(p) => Some(p.primary_focus(cx)),
            ShellPage::None => None,
        }
    }
}

/// One sidebar entry: the main pages, or on Settings routes *Back* plus the sections
/// (SET-080).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEntry {
    Page(Page),
    Back(Route),
    Section(SettingsSection),
}

/// Which overlay is open (for the Escape chain, KBD-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Switcher,
    Palette,
    Shortcuts,
}

pub struct AppShell {
    engines: Entity<EngineListStore>,
    store: Option<Entity<EngineStore>>,
    history: History,
    /// The last non-Settings route: where Settings' *Back* item returns (SET-080).
    app_route: Route,
    page: ShellPage,
    sidebar_collapsed: bool,
    regions: Regions,
    root_focus: FocusHandle,
    title_focus: FocusHandle,
    sidebar_focus: FocusHandle,
    content_focus: FocusHandle,
    status_focus: FocusHandle,
    switcher_btn_focus: FocusHandle,
    overflow_focus: FocusHandle,
    /// Painted bounds of the title-bar overflow button (its menu opens below it).
    overflow_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    /// Painted bounds of the engine switcher button (the switcher opens below it).
    switcher_btn_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    /// Sidebar roving cursor (index into [`AppShell::sidebar_entries`]).
    sidebar_cursor: usize,
    switcher: Option<Entity<SwitcherView>>,
    palette: Option<Entity<CommandState>>,
    shortcuts: Option<Entity<ShortcutList>>,
    overflow_menu: Option<KeyMenu>,
    /// Invoker to restore after an overlay closes (KBD-007).
    restore_focus: Option<FocusHandle>,
    /// Focus the page's primary control after the next render (navigation, KBD-007).
    focus_page_pending: Rc<Cell<bool>>,
    /// Focus at the previous render (see `ensure_focus_rendered`).
    last_focus: Option<FocusHandle>,
    action_task: Option<Task<()>>,
    /// *Restart to update* (UPD-007); separate so a container action can't cancel the quit.
    update_task: Option<Task<()>>,
    store_subs: Vec<Subscription>,
    _subs: Vec<Subscription>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = AppState::hub(cx);
        let engines = cx.new(|cx| EngineListStore::new(hub, cx));
        let ui = AppState::ui_state(cx);
        // SET-001: the configured start page decides where the app opens (`last_route` is
        // still recorded in state.json, spec 10 §6).
        let start = match AppState::config(cx).general.start_page {
            dk_hub::config::StartPage::Images => Route::Images,
            dk_hub::config::StartPage::Volumes => Route::Volumes,
            _ => Route::Containers,
        };
        let root_focus = cx.focus_handle();
        let title_focus = cx.focus_handle();
        let sidebar_focus = cx.focus_handle().tab_stop(true);
        let content_focus = cx.focus_handle();
        let status_focus = cx.focus_handle().tab_stop(true);
        let switcher_btn_focus = cx.focus_handle().tab_stop(true);
        let overflow_focus = cx.focus_handle().tab_stop(true);
        let regions = Regions::new(vec![
            RegionSlot {
                region: Region::TitleBar,
                container: title_focus.clone(),
                default: switcher_btn_focus.clone(),
                last: None,
            },
            RegionSlot {
                region: Region::Sidebar,
                container: sidebar_focus.clone(),
                default: sidebar_focus.clone(),
                last: None,
            },
            RegionSlot {
                region: Region::Content,
                container: content_focus.clone(),
                default: content_focus.clone(),
                last: None,
            },
            RegionSlot {
                region: Region::StatusBar,
                container: status_focus.clone(),
                default: status_focus.clone(),
                last: None,
            },
        ]);
        let subs = vec![
            cx.subscribe_in(&engines, window, Self::on_engine_list_event),
            cx.observe_in(&engines, window, |this, _, window, cx| {
                this.sync_engine_state(window, cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                let cfg = AppState::config(cx).general.clone();
                if cfg.theme == ThemeMode::System {
                    theme::apply(cfg.theme, cfg.ui_scale, Some(window), cx);
                }
            }),
            // M9: settings apply live (sidebar pages, Networks route).
            cx.observe_global_in::<AppState>(window, |this, window, cx| {
                this.on_config_changed(window, cx)
            }),
            cx.on_focus_in(&root_focus, window, |this, window, cx| {
                this.regions.remember(window, cx);
            }),
            // The focused element vanished (page swap, row removed): never leave focus nowhere
            // (KBD-007).
            cx.on_focus_lost(window, |this, window, cx| {
                if this.overlay() == Overlay::None && !window.has_active_dialog(cx) {
                    this.focus_page(window, cx);
                }
            }),
        ];
        let mut this = Self {
            engines,
            store: None,
            app_route: start.clone(),
            history: History::new(start),
            page: ShellPage::None,
            sidebar_collapsed: ui.sidebar_collapsed,
            regions,
            root_focus,
            title_focus,
            sidebar_focus,
            content_focus,
            status_focus,
            switcher_btn_focus,
            overflow_focus,
            overflow_bounds: Default::default(),
            switcher_btn_bounds: Default::default(),
            sidebar_cursor: 0,
            switcher: None,
            palette: None,
            shortcuts: None,
            overflow_menu: None,
            restore_focus: None,
            focus_page_pending: Rc::new(Cell::new(true)),
            last_focus: None,
            action_task: None,
            update_task: None,
            store_subs: Vec::new(),
            _subs: subs,
        };
        if let Some(updates) = UpdateStore::global(cx) {
            this._subs
                .push(cx.observe_in(&updates, window, Self::on_update_status));
        }
        // UPD-008: first launch after an update.
        if let Some(_previous) = AppState::hub(cx).take_previous_version() {
            let version = env!("CARGO_PKG_VERSION");
            let url = s::release_notes_url(version);
            let note = Notification::info(s::upd_updated(version)).action(move |_, _, _| {
                let url = url.clone();
                Button::new("upd-whats-new")
                    .small()
                    .ghost()
                    .label(s::UPD_WHATS_NEW)
                    .on_click(move |_, _, cx| cx.open_url(&url))
            });
            window.push_notification(note, cx);
        }
        let active = this.engines.read(cx).active_id().cloned();
        this.set_engine(active, window, cx);
        this
    }

    // ── updates (UPD-007, UPD-008, KBD-076) ────────────────────────────────────────────

    /// Status changed: re-render the status bar; once per version, announce a ready update.
    fn on_update_status(
        &mut self,
        updates: Entity<UpdateStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ready = updates.read(cx).ready_version().map(str::to_owned);
        if let Some(version) = ready
            && AppState::hub(cx).mark_update_notified(&version)
        {
            // One action button (GPUI Kit); clicking the toast body opens the release notes.
            let note = Notification::info(s::upd_ready(&version))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ViewReleaseNotes), cx))
                .action(|_, _, _| {
                    Button::new("upd-restart-now")
                        .small()
                        .primary()
                        .label(s::UPD_RESTART_NOW)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(RestartToUpdate), cx)
                        })
                });
            window.push_notification(note, cx);
        }
        cx.notify();
    }

    fn on_check_for_updates(
        &mut self,
        _: &CheckForUpdates,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(updates) = UpdateStore::global(cx) {
            updates.update(cx, |u, cx| u.check_now(cx));
        }
    }

    fn on_restart_to_update(
        &mut self,
        _: &RestartToUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ready =
            UpdateStore::global(cx).and_then(|u| u.read(cx).ready_version().map(str::to_owned));
        if ready.is_none() {
            notify::info(window, cx, s::UPD_NOTHING_READY);
            return;
        }
        let call = AppState::hub(cx).apply_update();
        self.update_task = Some(cx.spawn_in(window, async move |_, cx| {
            let result = call.await;
            cx.update(|window, cx| match result {
                // The installer waits for us to exit (AppMutex), then relaunches Dockering.
                Ok(()) => {
                    crate::app::save_window_bounds(window, cx);
                    cx.quit();
                }
                Err(e) => notify::engine_error(window, cx, s::UPD_APPLY_FAILED, &e),
            })
            .ok();
        }));
    }

    fn on_view_release_notes(
        &mut self,
        _: &ViewReleaseNotes,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = UpdateStore::global(cx)
            .and_then(|u| u.read(cx).notes_url().map(str::to_owned))
            .unwrap_or_else(|| s::release_notes_url(env!("CARGO_PKG_VERSION")));
        cx.open_url(&url);
    }

    /// Right-hand status-bar item (UPD-008). No focus ring (user preference).
    fn render_update_item(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        let store = UpdateStore::global(cx)?;
        let store = store.read(cx);
        let manual_running = matches!(store.manual(), Some(ManualCheck::Running));
        Some(match store.status() {
            UpdateStatus::Downloading { done, total, .. } => {
                let percent = if *total == 0 { 0 } else { done * 100 / total };
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Spinner::new().xsmall())
                    .child(div().text_xs().child(s::upd_downloading(percent)))
                    .into_any_element()
            }
            UpdateStatus::Ready {
                version,
                needs_elevation,
                ..
            } => Button::new("status-restart-to-update")
                .xsmall()
                .primary()
                .icon(IconName::Redo)
                .label(s::upd_restart(version, *needs_elevation))
                .tooltip_with_action(s::CMD_RESTART_TO_UPDATE, &RestartToUpdate, None)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(RestartToUpdate), cx))
                .into_any_element(),
            UpdateStatus::Available {
                version,
                notes_url,
                notify_only: true,
            } => {
                let url = notes_url.clone();
                Button::new("status-update-available")
                    .xsmall()
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .label(s::upd_available_short(version))
                    .tooltip_with_action(s::CMD_VIEW_RELEASE_NOTES, &ViewReleaseNotes, None)
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element()
            }
            UpdateStatus::Checking if manual_running => div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(s::UPD_CHECKING)
                .into_any_element(),
            UpdateStatus::Error { message } if store.manual().is_some() => {
                Button::new("status-update-error")
                    .xsmall()
                    .ghost()
                    .icon(IconName::TriangleAlert)
                    .label(s::UPD_CHECK_FAILED.trim_end_matches(':').to_owned())
                    .tooltip(message.clone())
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(CheckForUpdates), cx))
                    .into_any_element()
            }
            _ => return None,
        })
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    /// Whether the status bar currently shows an update item (UPD-008).
    pub fn has_update_item(&self, cx: &App) -> bool {
        self.render_update_item(cx).is_some()
    }

    pub fn route(&self) -> &Route {
        self.history.current()
    }
    pub fn store(&self) -> Option<&Entity<EngineStore>> {
        self.store.as_ref()
    }
    pub fn engines(&self) -> &Entity<EngineListStore> {
        &self.engines
    }
    pub fn page(&self) -> &ShellPage {
        &self.page
    }
    pub fn containers_page(&self) -> Option<&Entity<ContainersPage>> {
        match &self.page {
            ShellPage::Containers(p) => Some(p),
            _ => None,
        }
    }
    pub fn overlay(&self) -> Overlay {
        if self.switcher.is_some() {
            Overlay::Switcher
        } else if self.palette.is_some() {
            Overlay::Palette
        } else if self.shortcuts.is_some() {
            Overlay::Shortcuts
        } else {
            Overlay::None
        }
    }
    pub fn shortcuts(&self) -> Option<&Entity<ShortcutList>> {
        self.shortcuts.as_ref()
    }
    pub fn palette_state(&self) -> Option<&Entity<CommandState>> {
        self.palette.as_ref()
    }
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }
    /// Pages listed in the sidebar (SET-001 hides Networks).
    pub fn sidebar_pages(&self, cx: &App) -> Vec<Page> {
        self.visible_pages(cx)
    }
    /// What the sidebar lists for the current route (SET-080).
    pub fn sidebar_entries(&self, cx: &App) -> Vec<SidebarEntry> {
        if matches!(self.history.current(), Route::Settings { .. }) {
            std::iter::once(SidebarEntry::Back(self.app_route.clone()))
                .chain(
                    SettingsSection::ALL
                        .iter()
                        .map(|s| SidebarEntry::Section(*s)),
                )
                .collect()
        } else {
            self.visible_pages(cx)
                .into_iter()
                .map(SidebarEntry::Page)
                .collect()
        }
    }
    pub fn sidebar_cursor(&self) -> usize {
        self.sidebar_cursor
    }
    pub fn sidebar_focus(&self) -> &FocusHandle {
        &self.sidebar_focus
    }
    pub fn region_container(&self, r: Region) -> Option<&FocusHandle> {
        self.regions.container(r)
    }
    pub fn current_region(&self, window: &Window, cx: &App) -> Option<Region> {
        self.regions.current(window, cx)
    }

    // ── engines ────────────────────────────────────────────────────────────────────────

    fn active_status(&self, cx: &App) -> Option<dk_core::EngineStatus> {
        self.engines.read(cx).active().cloned()
    }

    fn sync_engine_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.active_status(cx).map(|s| s.state);
        let was = cx
            .try_global::<ShellState>()
            .and_then(|s| s.engine_state.clone());
        if was != state {
            cx.set_global(ShellState {
                engine_state: state.clone(),
            });
            let connected = matches!(state, Some(EngineState::Connected | EngineState::Degraded));
            if let Some(store) = &self.store {
                store.update(cx, |s, cx| s.set_connected(connected, cx));
            }
            // Pages appear/disappear with connection state (SHL-013).
            let had_page = !matches!(self.page, ShellPage::None);
            self.mount_page(window, cx);
            if !had_page && !matches!(self.page, ShellPage::None) {
                // The page just appeared (first connect, reconnect): give it focus unless the
                // user is elsewhere (KBD-007).
                let elsewhere = window.focused(cx).is_some_and(|f| {
                    !self.content_focus.contains(&f, window) && f != self.root_focus
                });
                if !elsewhere {
                    self.focus_page_pending.set(true);
                }
            }
        }
    }

    fn on_engine_list_event(
        &mut self,
        _: &Entity<EngineListStore>,
        event: &EngineListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineListEvent::ActiveChanged(id) => {
                let id = id.clone();
                self.set_engine(id, window, cx);
            }
            EngineListEvent::Reconnected(id) => {
                if let Some(store) = &self.store
                    && store.read(cx).engine_id() == id
                {
                    store.update(cx, |s, cx| s.reconnected(cx));
                }
            }
            EngineListEvent::CapabilitiesChanged(id) => {
                if let Some(store) = &self.store
                    && store.read(cx).engine_id() == id
                {
                    store.update(cx, |s, cx| s.fetch_info(cx));
                }
            }
        }
    }

    /// Engine switch (ENG-102, spec 10 §4.3): drop the old store (cancels its tasks), create
    /// a new one, keep list routes, map details to their parent list.
    fn set_engine(&mut self, id: Option<EngineId>, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.as_ref().map(|s| s.read(cx).engine_id()) == id.as_ref() {
            return;
        }
        let had_engine = self.store.is_some();
        self.store = None;
        self.store_subs.clear();
        if had_engine {
            self.history.engine_switched();
            self.app_route = self.app_route.for_engine_switch();
        }
        if let Some(id) = id {
            let hub = AppState::hub(cx);
            let interval = AppState::config(cx).containers.polling_interval_s;
            let store = cx.new(|cx| EngineStore::new(hub, id, interval, cx));
            self.store_subs = vec![
                cx.observe(&store, |_, _, cx| cx.notify()),
                cx.subscribe(&store, |_, _, _: &EngineStoreEvent, cx| cx.notify()),
            ];
            self.store = Some(store);
        }
        cx.set_global(ShellState {
            engine_state: self.active_status(cx).map(|s| s.state),
        });
        // TRM-008: terminal sessions don't survive an engine switch.
        let keep = self.store.as_ref().map(|s| s.read(cx).engine_id().clone());
        crate::state::TerminalRegistry::close_other_engines(keep.as_ref(), cx);
        self.mount_page(window, cx);
        self.focus_page_pending.set(true);
        cx.notify();
    }

    fn on_switch_engine(&mut self, a: &SwitchEngine, window: &mut Window, cx: &mut Context<Self>) {
        let id = EngineId::new(a.id.to_string());
        self.close_overlays(window, cx);
        // Switch the window within one frame (ENG-102); data loads async.
        self.engines
            .update(cx, |e, cx| e.mark_switching(id.clone(), cx));
        let hub = AppState::hub(cx);
        let call = hub.set_active(&id);
        self.action_task = Some(cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = call.await {
                cx.update(|window, cx| notify::engine_error(window, cx, s::SWITCH_ENGINE, &err))
                    .ok();
            }
        }));
    }

    fn hub_call(
        &mut self,
        title: &'static str,
        call: dk_hub::HubCall<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.action_task = Some(cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = call.await {
                cx.update(|window, cx| notify::engine_error(window, cx, title, &err))
                    .ok();
            }
        }));
    }

    fn on_retry(&mut self, _: &RetryEngine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.engines.read(cx).active_id().cloned() else {
            return;
        };
        let call = AppState::hub(cx).retry(&id);
        self.hub_call(s::RETRY, call, window, cx);
    }

    fn on_start_engine(&mut self, _: &StartEngine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.engines.read(cx).active_id().cloned() else {
            return;
        };
        let call = AppState::hub(cx).start_wsl_distro(&id);
        self.hub_call(s::START_AND_CONNECT, call, window, cx);
    }

    fn on_rescan(&mut self, _: &Rescan, window: &mut Window, cx: &mut Context<Self>) {
        let call = AppState::hub(cx).rescan();
        self.hub_call(s::RESCAN, call, window, cx);
    }

    // ── navigation ─────────────────────────────────────────────────────────────────────

    /// Navigate to `route` (push). Focus moves to the new page's primary control (KBD-007).
    pub fn navigate(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        if self.history.push(route) {
            self.after_route_change(window, cx);
        } else {
            self.focus_page(window, cx);
        }
    }

    fn after_route_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.history.current().to_state() {
            AppState::update_ui_state(cx, move |s| s.last_route = Some(state));
        }
        self.mount_page(window, cx);
        self.focus_page_pending.set(true);
        cx.notify();
    }

    fn connected(&self, cx: &App) -> bool {
        self.active_status(cx).is_some_and(|s| s.state.is_usable())
    }

    /// (Re)creates the page view for the current route.
    fn mount_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.history.current().clone();
        let needs_engine = !matches!(route, Route::Settings { .. });
        if needs_engine {
            self.app_route = route.clone();
        }
        self.sidebar_cursor = self.active_entry(cx);
        if needs_engine && (self.store.is_none() || !self.connected(cx)) {
            self.page = ShellPage::None;
            return;
        }
        // Keep the same page entity when the route didn't change kind (tab switches etc.).
        self.page = match (&route, &self.page) {
            // Same store: keep the page (search, selection, scroll survive reconnects).
            (Route::Containers, ShellPage::Containers(p))
                if self
                    .store
                    .as_ref()
                    .is_some_and(|s| p.read(cx).store_entity().entity_id() == s.entity_id()) =>
            {
                ShellPage::Containers(p.clone())
            }
            (Route::Containers, _) => match &self.store {
                Some(store) => {
                    let store = store.clone();
                    ShellPage::Containers(cx.new(|cx| ContainersPage::new(store, window, cx)))
                }
                None => ShellPage::None,
            },
            // Same container on the same store: keep the page and its tab state, switch the
            // tab in place (CDT-002/081).
            (Route::ContainerDetail { id, tab }, ShellPage::ContainerDetail(p))
                if p.read(cx).container_id() == id
                    && p.read(cx).store_id() == self.store.as_ref().map(|s| s.entity_id()) =>
            {
                let tab = *tab;
                p.update(cx, |p, cx| p.set_tab(tab, window, cx));
                ShellPage::ContainerDetail(p.clone())
            }
            (Route::ContainerDetail { id, tab }, _) => ShellPage::ContainerDetail(
                pages::container_detail::new(id.clone(), *tab, self.store.clone(), window, cx),
            ),
            // M6: resource pages (keep the same entity when it accepts the route, e.g. a tab).
            (
                Route::Images
                | Route::Volumes
                | Route::Networks
                | Route::ImageDetail { .. }
                | Route::VolumeDetail { .. }
                | Route::NetworkDetail { .. },
                current,
            ) => match (&self.store, current) {
                (Some(store), current) => {
                    let keep = match current {
                        ShellPage::Dyn(p) if p.store_id() == Some(store.entity_id()) => {
                            p.accept_route(&route, window, cx).then(|| p.clone())
                        }
                        _ => None,
                    };
                    ShellPage::Dyn(match keep {
                        Some(p) => p,
                        None => pages::mount_resource_page(&route, store.clone(), window, cx),
                    })
                }
                (None, _) => ShellPage::None,
            },
            // M9: one Settings entity across sections (route = section; back/forward).
            (Route::Settings { section }, ShellPage::Settings(p)) => {
                let section = *section;
                p.update(cx, |p, cx| p.set_section(section, cx));
                ShellPage::Settings(p.clone())
            }
            (Route::Settings { section }, _) => ShellPage::Settings(pages::settings::new(
                *section,
                self.engines.clone(),
                window,
                cx,
            )),
        };
        if let Some(h) = self.page.primary_focus(cx) {
            self.regions.set_default(Region::Content, h);
        }
    }

    /// The sidebar entry of the current route.
    fn active_entry(&self, cx: &App) -> usize {
        let route = self.history.current();
        self.sidebar_entries(cx)
            .iter()
            .position(|e| match (e, route) {
                (SidebarEntry::Section(s), Route::Settings { section }) => s == section,
                (SidebarEntry::Page(p), r) => Some(*p) == r.page(),
                _ => false,
            })
            .unwrap_or(0)
    }

    /// Where focus goes after a navigation (KBD-007): the page's primary control, or on
    /// Settings the sidebar's section nav (SET-080, KBD-024).
    fn primary_focus(&self, cx: &App) -> Option<FocusHandle> {
        if matches!(self.history.current(), Route::Settings { .. }) {
            return Some(self.sidebar_focus.clone());
        }
        self.page.primary_focus(cx)
    }

    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.primary_focus(cx) {
            Some(h) => window.focus(&h, cx),
            None => window.focus(&self.content_focus, cx),
        }
    }

    /// After a render: if focus points at an element that isn't in the frame (e.g. the table
    /// was replaced by an error panel), move it to the content region so keys and actions
    /// still reach the shell and page (KBD-007: no focus loss).
    fn ensure_focus_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = window.focused(cx) else {
            return;
        };
        // Only judge a handle that was already focused when the last frame was drawn; a
        // handle focused since then (a popup or input opening) has no node yet.
        let seen = self.last_focus.replace(f.clone());
        if seen.as_ref() != Some(&f) {
            return;
        }
        // `context_stack` is empty only when the focused handle has no node in the rendered
        // frame (deferred popups and dialogs still have one).
        if window.context_stack().is_empty()
            && self.overlay() == Overlay::None
            && !window.has_active_dialog(cx)
            && self.root_focus.contains(&self.content_focus, window)
            && !self.root_focus.contains(&f, window)
        {
            window.focus(&self.content_focus, cx);
        }
    }

    fn go(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(route, window, cx);
    }

    /// Replaces the current route without a history entry (detail tabs, M6).
    pub fn replace_route(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if *self.history.current() == route {
            return;
        }
        self.history.replace(route);
        self.mount_page(window, cx);
        cx.notify();
    }

    fn on_navigate(&mut self, a: &Navigate, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(a.route.clone(), window, cx);
    }

    fn on_back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.back() {
            self.after_route_change(window, cx);
        }
    }

    fn on_forward(&mut self, _: &Forward, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.forward() {
            self.after_route_change(window, cx);
        }
    }

    fn visible_pages(&self, cx: &App) -> Vec<Page> {
        let show_networks = AppState::config(cx).general.show_networks_page;
        Page::ALL
            .iter()
            .copied()
            .filter(|p| *p != Page::Networks || show_networks)
            .collect()
    }

    // ── global actions ─────────────────────────────────────────────────────────────────

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let v = self.sidebar_collapsed;
        AppState::update_ui_state(cx, move |s| s.sidebar_collapsed = v);
        cx.notify();
    }

    fn on_toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        let mode = theme::toggled(cx.theme().is_dark());
        let scale = AppState::config(cx).general.ui_scale;
        AppState::update_config(cx, |c| c.general.theme = mode);
        theme::apply(mode, scale, Some(window), cx);
    }

    fn set_zoom(&mut self, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        let scale = theme::clamp_scale(scale);
        let mode = AppState::config(cx).general.theme;
        AppState::update_config(cx, |c| c.general.ui_scale = scale);
        theme::apply(mode, scale, Some(window), cx);
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let s = theme::step_scale(AppState::config(cx).general.ui_scale, theme::SCALE_STEP);
        self.set_zoom(s, window, cx);
    }
    fn on_zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let s = theme::step_scale(AppState::config(cx).general.ui_scale, -theme::SCALE_STEP);
        self.set_zoom(s, window, cx);
    }
    fn on_zoom_reset(&mut self, _: &ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, window, cx);
    }

    fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.update(cx, |s, cx| {
                s.refresh_all(cx);
                s.fetch_info(cx);
            });
        }
    }

    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        match &self.page {
            ShellPage::Containers(p) => p.update(cx, |p, cx| p.focus_search(window, cx)),
            ShellPage::Dyn(p) => p.focus_search(window, cx),
            ShellPage::Settings(p) => p.update(cx, |p, cx| p.focus_search(window, cx)),
            ShellPage::ContainerDetail(p) => p.update(cx, |p, cx| p.focus_search(window, cx)),
            _ => self.focus_page(window, cx),
        }
    }

    fn on_next_region(&mut self, _: &NextRegion, window: &mut Window, cx: &mut Context<Self>) {
        self.regions.cycle(true, window, cx);
        cx.notify();
    }

    fn on_prev_region(&mut self, _: &PrevRegion, window: &mut Window, cx: &mut Context<Self>) {
        self.regions.cycle(false, window, cx);
        cx.notify();
    }

    fn on_focus_notifications(
        &mut self,
        _: &FocusNotifications,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // KBD-074: focus the newest toast (its action buttons are Tab-reachable).
        // GPUI Kit keeps the toast stack focus handle private; it is a Tab stop rendered
        // outside our root, so walk the Tab order until focus leaves the shell.
        if window.notifications(cx).is_empty() {
            return;
        }
        let start = window.focused(cx);
        for _ in 0..256 {
            window.focus_next(cx);
            match window.focused(cx) {
                Some(_) if !self.root_focus.contains_focused(window, cx) => return,
                f if f == start => break,
                _ => {}
            }
        }
        if let Some(h) = start {
            window.focus(&h, cx);
        }
    }

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn on_close_window(&mut self, _: &CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn on_minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    fn on_zoom_window(&mut self, _: &ZoomWindow, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    fn on_hide(&mut self, _: &Hide, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide();
    }

    fn on_hide_others(&mut self, _: &HideOthers, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide_other_apps();
    }

    fn on_about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        notify::info(
            window,
            cx,
            format!("{} {}", s::APP_NAME, env!("CARGO_PKG_VERSION")),
        );
    }

    fn on_open_logs(&mut self, _: &OpenLogsFolder, _: &mut Window, cx: &mut Context<Self>) {
        let dir = AppState::hub(cx).paths().log_dir.clone();
        cx.open_with_system(&dir);
    }

    // ── M9: Settings commands (SET-060, ENG-105, REL-002) ─────────────────────────────

    /// *Add engine…* from anywhere (first-run screen ENG-111, palette, Settings › Engines):
    /// show Settings › Engines, then open the dialog. Focus returns to the Engines section's
    /// *Add engine…* button when it closes (KBD-071).
    fn on_add_engine(
        &mut self,
        _: &crate::actions::settings::AddEngine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = Route::Settings {
            section: SettingsSection::Engines,
        };
        if *self.history.current() != target {
            self.go(target, window, cx);
        }
        self.focus_page_pending.set(false);
        if let ShellPage::Settings(p) = &self.page {
            p.update(cx, |p, cx| p.open_add_engine(window, cx));
        }
    }

    fn on_copy_diagnostics(
        &mut self,
        _: &crate::actions::settings::CopyDiagnostics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        pages::settings::copy_diagnostics(window, cx);
    }

    fn on_view_licenses(
        &mut self,
        _: &crate::actions::settings::ViewLicenses,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        pages::settings::view_licenses(window, cx);
    }

    /// Settings changed (SET-001…060 apply live): sidebar pages, start page; the containers
    /// page re-reads its defaults on its own.
    fn on_config_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = AppState::config(cx);
        let show_networks = config.general.show_networks_page;
        let polling = config.containers.polling_interval_s;
        if !show_networks && self.history.current().page() == Some(Page::Networks) {
            self.go(Route::Containers, window, cx);
        }
        if !show_networks && self.app_route.page() == Some(Page::Networks) {
            self.app_route = Route::Containers;
        }
        if let Some(store) = &self.store {
            store.update(cx, |s, cx| s.set_polling_interval(polling, cx));
        }
        let n = self.sidebar_entries(cx).len();
        self.sidebar_cursor = self.sidebar_cursor.min(n.saturating_sub(1));
        cx.notify();
    }

    fn on_manage_engines(
        &mut self,
        _: &ManageEngines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go(
            Route::Settings {
                section: SettingsSection::Engines,
            },
            window,
            cx,
        );
    }

    fn on_copy_text(&mut self, a: &CopyText, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(a.text.to_string()));
        notify::info(window, cx, s::COPIED);
    }

    fn on_open_url(&mut self, a: &OpenUrl, _: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(&a.url);
    }

    // ── sidebar ────────────────────────────────────────────────────────────────────────

    /// Moves the roving cursor. On Settings, landing on a section shows it in place (like
    /// tabs: no history entry, SET-080); the main pages wait for `Enter`.
    fn sidebar_set_cursor(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.sidebar_entries(cx);
        let Some(last) = entries.len().checked_sub(1) else {
            return;
        };
        self.sidebar_cursor = ix.min(last);
        if let SidebarEntry::Section(section) = entries[self.sidebar_cursor] {
            self.replace_route(Route::Settings { section }, window, cx);
        }
        cx.notify();
    }

    fn sidebar_move(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = (self.sidebar_cursor as isize + delta).max(0) as usize;
        self.sidebar_set_cursor(ix, window, cx);
    }

    fn on_sidebar_prev(
        &mut self,
        _: &crate::actions::sidebar::Prev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_move(-1, window, cx);
    }
    fn on_sidebar_next(
        &mut self,
        _: &crate::actions::sidebar::Next,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_move(1, window, cx);
    }
    fn on_sidebar_first(
        &mut self,
        _: &crate::actions::sidebar::First,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_set_cursor(0, window, cx);
    }
    fn on_sidebar_last(
        &mut self,
        _: &crate::actions::sidebar::Last,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_set_cursor(usize::MAX, window, cx);
    }
    /// `Enter` / `Space`: open the page, go *Back*, or step into the section's first control.
    fn on_sidebar_activate(
        &mut self,
        _: &crate::actions::sidebar::Activate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.sidebar_entries(cx).get(self.sidebar_cursor).cloned() {
            Some(SidebarEntry::Page(p)) => self.go(p.route(), window, cx),
            Some(SidebarEntry::Back(route)) => self.go(route, window, cx),
            Some(SidebarEntry::Section(section)) => {
                self.replace_route(Route::Settings { section }, window, cx);
                // The content column follows the sidebar in the Tab order.
                window.focus_next(cx);
            }
            None => {}
        }
    }

    // ── overlays ───────────────────────────────────────────────────────────────────────

    fn remember_invoker(&mut self, window: &Window, cx: &App) {
        if self.overlay() == Overlay::None {
            self.restore_focus = window.focused(cx);
        }
    }

    fn restore_invoker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.restore_focus.take() {
            Some(h) => window.focus(&h, cx),
            None => self.focus_page(window, cx),
        }
    }

    pub fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let had = self.overlay() != Overlay::None;
        if had && window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        self.switcher = None;
        self.palette = None;
        self.shortcuts = None;
        if had {
            self.restore_invoker(window, cx);
        }
        cx.notify();
        had
    }

    fn on_engine_switcher(
        &mut self,
        _: &EngineSwitcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switcher.is_some() {
            self.close_overlays(window, cx);
            return;
        }
        self.close_overlays(window, cx);
        self.remember_invoker(window, cx);
        let list = self.engines.clone();
        let view = cx.new(|cx| SwitcherView::new(list, window, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            |this, _, e: &SwitcherEvent, window, cx| match e {
                SwitcherEvent::Chosen(Choice::Switch(id)) => {
                    let id = id.clone();
                    this.on_switch_engine(&SwitchEngine { id }, window, cx)
                }
                SwitcherEvent::Chosen(Choice::StartAndConnect(id)) => {
                    let id = EngineId::new(id.to_string());
                    this.close_overlays(window, cx);
                    let hub = AppState::hub(cx);
                    let call = hub.start_wsl_distro(&id);
                    this.hub_call(s::START_AND_CONNECT, call, window, cx);
                }
                SwitcherEvent::Chosen(Choice::None) => {}
                SwitcherEvent::Dismissed => {
                    this.close_overlays(window, cx);
                }
            },
        );
        self.store_subs.push(sub);
        view.update(cx, |v, cx| v.focus_filter(window, cx));
        self.switcher = Some(view);
        cx.notify();
    }

    fn on_command_palette(
        &mut self,
        _: &CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_overlays(window, cx);
            return;
        }
        self.close_overlays(window, cx);
        self.remember_invoker(window, cx);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.palette = Some(state);
        cx.notify();
    }

    fn on_shortcuts(&mut self, _: &ShortcutReference, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcuts.is_some() {
            self.close_overlays(window, cx);
            return;
        }
        self.close_overlays(window, cx);
        self.remember_invoker(window, cx);
        let list = cx.new(|cx| ShortcutList::new(window, cx));
        list.update(cx, |l, cx| l.focus_filter(window, cx));
        self.shortcuts = Some(list);
        cx.notify();
    }

    fn on_run_command(
        &mut self,
        a: &palette::RunCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlays(window, cx);
        let action = a.action.0.boxed_clone();
        // Dispatch from the restored focus on the next effect cycle (window.dispatch_action
        // defers and resolves the focused element then).
        window.dispatch_action(action, cx);
    }

    fn on_palette_dismiss(
        &mut self,
        _: &crate::actions::palette::Dismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlays(window, cx);
    }

    fn on_focus_menu_bar(&mut self, _: &FocusMenuBar, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(target_os = "macos") {
            return;
        }
        if self.overflow_focus.is_focused(window) {
            self.open_overflow(window, cx);
        } else {
            window.focus(&self.overflow_focus, cx);
        }
    }

    fn open_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.overflow_focus.clone();
        let fallback = gpui_kit::point(window.viewport_size().width - px(260.), px(36.));
        let pos = MenuAnchor::below_or(self.overflow_bounds, fallback);
        self.overflow_menu = Some(KeyMenu::open(
            pos,
            restore,
            window,
            cx,
            |menu, _, _| menus::overflow_menu(menu),
            |this: &mut Self, _| this.overflow_menu = None,
        ));
        cx.notify();
    }

    /// The Escape chain (KBD-006): popup/menu → dialog → (search clear, handled by the input)
    /// → multi-selection (handled by the list) → return focus to the page's primary list.
    fn on_escape_root(
        &mut self,
        _: &gpui_kit::base::actions::Cancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overflow_menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.close_overlays(window, cx) {
            return;
        }
        // Clear a focused, non-empty page search.
        if let ShellPage::Containers(p) = &self.page {
            let cleared = p.update(cx, |p, cx| p.clear_search_if_focused(window, cx));
            if cleared {
                return;
            }
        }
        if let ShellPage::Dyn(p) = &self.page
            && p.clear_search_if_focused(window, cx)
        {
            return;
        }
        if let ShellPage::ContainerDetail(p) = &self.page
            && p.update(cx, |p, cx| p.clear_search_if_focused(window, cx))
        {
            return;
        }
        // Return focus to the primary list if focus is elsewhere in content.
        if let Some(h) = self.primary_focus(cx)
            && !h.is_focused(window)
        {
            window.focus(&h, cx);
        }
    }

    fn on_list_escape(&mut self, _: &list::Escape, window: &mut Window, cx: &mut Context<Self>) {
        // Reaches the shell only when the list had no selection to clear.
        self.on_escape_root(&gpui_kit::base::actions::Cancel, window, cx);
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_title_bar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let status = self.active_status(cx);
        let name: SharedString = status
            .as_ref()
            .map(|s| s.config.name.clone().into())
            .unwrap_or_else(|| s::NO_ENGINE.into());
        let dot_color = status
            .as_ref()
            .map(|s| engine_dot_color(&s.state, cx))
            .unwrap_or(cx.theme().muted_foreground);
        let state_label = status
            .as_ref()
            .map(|s| engine_state_label(&s.state))
            .unwrap_or("");
        let kind = status.as_ref().and_then(|s| s.config.endpoint.kind());
        let insecure = status
            .as_ref()
            .is_some_and(|s| s.config.endpoint.is_insecure_tcp());
        // The GPUI Kit `TitleBar` marks the whole bar as a window drag area. On Windows a
        // drag area under the pointer answers `WM_NCHITTEST` with `HTCAPTION`, which turns
        // the click into a window drag and never reaches our controls. Every interactive
        // element in the bar therefore `occlude()`s the drag area beneath it; the empty
        // space between them still moves the window.
        TitleBar::new()
            .child(
                h_flex()
                    .id("title-region")
                    .track_focus(&self.title_focus)
                    .key_context(ctx::TOOLBAR)
                    .w_full()
                    .pr_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(s::APP_NAME),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .id("engine-switcher-button")
                            .occlude()
                            .track_focus(&self.switcher_btn_focus)
                            .gap_2()
                            .px_2()
                            .py_0p5()
                            .items_center()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .cursor_pointer()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(EngineSwitcher), cx)
                            })
                            .on_key_down(cx.listener(
                                |_, e: &gpui_kit::KeyDownEvent, window, cx| {
                                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                        window.dispatch_action(Box::new(EngineSwitcher), cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(s::SWITCH_ENGINE)
                                    .action(&EngineSwitcher, None)
                                    .build(window, cx)
                            })
                            .child(Icon::new(kind_icon(kind)).small())
                            .child(div().text_sm().child(name))
                            .child(dot(dot_color))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(state_label),
                            )
                            .child(Icon::new(IconName::ChevronDown).small())
                            .on_bounds({
                                let this = cx.entity().downgrade();
                                move |b, _, cx| {
                                    this.update(cx, |s, _| s.switcher_btn_bounds = b).ok();
                                }
                            }),
                    )
                    .when(insecure, |this| {
                        this.child(
                            div()
                                .id("insecure-tcp")
                                .occlude()
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        s::INSECURE_TCP_TOOLTIP,
                                    )
                                    .build(window, cx)
                                })
                                .child(Tag::warning().small().child(s::INSECURE_TCP)),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .id("title-actions")
                            .occlude()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("title-refresh")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .tooltip_with_action(s::REFRESH, &Refresh, None)
                                    .on_click(|_, w, cx| w.dispatch_action(Box::new(Refresh), cx)),
                            )
                            .child(
                                Button::new("title-theme")
                                    .ghost()
                                    .small()
                                    .icon(if cx.theme().is_dark() {
                                        IconName::Sun
                                    } else {
                                        IconName::Moon
                                    })
                                    .tooltip_with_action(s::TOGGLE_THEME, &ToggleTheme, None)
                                    .on_click(|_, w, cx| {
                                        w.dispatch_action(Box::new(ToggleTheme), cx)
                                    }),
                            )
                            .child(
                                Button::new("title-settings")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Settings)
                                    .tooltip_with_action(s::OPEN_SETTINGS, &OpenSettings, None)
                                    .on_click(|_, w, cx| {
                                        w.dispatch_action(Box::new(OpenSettings), cx)
                                    }),
                            )
                            .when(!cfg!(target_os = "macos"), |this| {
                                // SHL-021: Alt focuses the overflow menu button on Windows/Linux.
                                this.child(
                                    crate::ui::widgets::focus_wrap(
                                        "title-overflow-wrap",
                                        &self.overflow_focus,
                                        Button::new("title-overflow")
                                            .ghost()
                                            .small()
                                            .icon(IconName::Menu)
                                            .tooltip(s::MORE_COMMANDS)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_overflow(window, cx)
                                            })),
                                        {
                                            let this = cx.entity().downgrade();
                                            move |_, window, cx| {
                                                this.update(cx, |s, cx| {
                                                    s.open_overflow(window, cx)
                                                })
                                                .ok();
                                            }
                                        },
                                        cx,
                                    )
                                    .on_bounds({
                                        let this = cx.entity().downgrade();
                                        move |b, _, cx| {
                                            this.update(cx, |s, _| s.overflow_bounds = b).ok();
                                        }
                                    }),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let route = self.history.current().clone();
        // The cursor item is tinted while the sidebar has keyboard focus. Keyboard only: a
        // click on a settings section focuses the sidebar (SET-080) without showing a cursor.
        let focused = self.sidebar_focus.is_focused(window) && window.last_input_was_keyboard();
        let cursor = self.sidebar_cursor;
        let counts = self.store.as_ref().map(|s| {
            let s = s.read(cx);
            [
                s.count(Collection::Containers),
                s.count(Collection::Images),
                s.count(Collection::Volumes),
                s.count(Collection::Networks),
            ]
        });
        let items: Vec<NavItem> = self
            .sidebar_entries(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, entry)| {
                let cursor = focused && ix == cursor;
                match entry {
                    SidebarEntry::Page(p) => {
                        let count = counts.and_then(|c| {
                            c[match p {
                                Page::Containers => 0,
                                Page::Images => 1,
                                Page::Volumes => 2,
                                Page::Networks => 3,
                            }]
                        });
                        let icon = match p {
                            Page::Containers => IconName::Inspector,
                            Page::Images => IconName::GalleryVerticalEnd,
                            Page::Volumes => IconName::HardDrive,
                            Page::Networks => IconName::Network,
                        };
                        NavItem {
                            label: p.label().into(),
                            route: p.route(),
                            icon: Icon::new(icon),
                            count,
                            active: Some(p) == route.page(),
                            cursor,
                        }
                    }
                    SidebarEntry::Back(target) => NavItem {
                        label: s::back_to(target.page().unwrap_or(Page::Containers).label()).into(),
                        route: target,
                        icon: Icon::new(IconName::ArrowLeft),
                        count: None,
                        active: false,
                        cursor,
                    },
                    SidebarEntry::Section(section) => NavItem {
                        label: section.label().into(),
                        route: Route::Settings { section },
                        icon: Icon::new(section.icon()),
                        count: None,
                        active: route == Route::Settings { section },
                        cursor,
                    },
                }
            })
            .collect();
        // Settings: *Back* on its own, then the sections under a heading.
        let menu = if matches!(route, Route::Settings { .. }) {
            let mut items = items;
            let sections = items.split_off(1);
            NavMenu::new(items).group(s::PAGE_SETTINGS, sections)
        } else {
            NavMenu::new(items)
        };
        div()
            .id("sidebar-region")
            .key_context(ctx::SIDEBAR)
            .track_focus(&self.sidebar_focus)
            .h_full()
            .on_action(cx.listener(Self::on_sidebar_prev))
            .on_action(cx.listener(Self::on_sidebar_next))
            .on_action(cx.listener(Self::on_sidebar_first))
            .on_action(cx.listener(Self::on_sidebar_last))
            .on_action(cx.listener(Self::on_sidebar_activate))
            .child(
                Sidebar::new("sidebar")
                    .collapsed(self.sidebar_collapsed)
                    .w(px(220.))
                    .child(menu),
            )
            .into_any_element()
    }

    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let status = self.active_status(cx);
        let info = self
            .store
            .as_ref()
            .and_then(|s| s.read(cx).info().cloned())
            .or_else(|| status.as_ref().and_then(|s| s.info.clone()));
        let state = status.as_ref().map(|s| s.state.clone());
        let left = h_flex()
            .gap_2()
            .items_center()
            .when_some(state.as_ref(), |this, st| {
                this.child(dot(engine_dot_color(st, cx)))
                    .child(engine_state_label(st))
            })
            .when_some(info.as_ref(), |this, i| {
                this.child(s::status_bar_engine(
                    &i.server_version,
                    i.api_version.as_deref(),
                    &i.os,
                    &i.arch,
                ))
                .when_some(i.transport.as_ref(), |this, t| {
                    // ENG-110: active transport (+ fallback reason chip).
                    this.child(Tag::secondary().small().child(s::transport_label(t)))
                })
                .when_some(i.transport_note.as_ref(), |this, n| {
                    this.child(Tag::info().small().child(n.clone()))
                })
            });
        let resources = info.as_ref().map(|i| {
            s::status_bar_resources(i.cpus, i.mem_total.map(dk_core::format::format_size))
        });
        let update = self.render_update_item(cx);
        let right = (resources.is_some() || update.is_some()).then(|| {
            h_flex()
                .gap_3()
                .items_center()
                .when_some(resources, |this, r| this.child(r))
                .when_some(update, |this, u| this.child(u))
        });
        div()
            .id("status-region")
            .track_focus(&self.status_focus)
            .child(
                StatusBar::new()
                    .left(left)
                    .when_some(right, |this, r| this.right(r)),
            )
            .into_any_element()
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let list = self.engines.read(cx);
        if !list.loaded() {
            return crate::ui::skeleton_rows(8, 5);
        }
        let route = self.history.current().clone();
        if let Route::Settings { .. } = route
            && let Some(v) = self.page.view()
        {
            return v.into_any_element();
        }
        if list.is_empty() {
            return engine_views::first_run(cx);
        }
        let Some(status) = list.active().cloned() else {
            if list.none_usable() {
                return engine_views::first_run(cx);
            }
            return engine_views::no_active_engine();
        };
        match &status.state {
            EngineState::Connected | EngineState::Degraded => {
                let degraded = matches!(status.state, EngineState::Degraded);
                v_flex()
                    .size_full()
                    .when(degraded, |this| this.child(engine_views::degraded_banner()))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .children(self.page.view().map(|v| v.into_any_element())),
                    )
                    .into_any_element()
            }
            EngineState::Connecting => crate::ui::skeleton_rows(8, 5),
            _ => engine_views::disconnected_page(&status, cx),
        }
    }

    /// The engine switcher as a dropdown under its title-bar button (centred on it, kept
    /// inside the window), over a click-to-dismiss backdrop like the other overlays.
    fn switcher_dropdown(&self, child: gpui_kit::AnyElement, cx: &App) -> gpui_kit::AnyElement {
        const WIDTH: f32 = 404.;
        let b = self.switcher_btn_bounds;
        let (position, anchor) = if b.size.width > px(0.) {
            (
                gpui_kit::point(b.center().x, b.bottom() + px(4.)),
                gpui_kit::Anchor::TopCenter,
            )
        } else {
            (gpui_kit::point(px(0.), px(44.)), gpui_kit::Anchor::TopLeft)
        };
        gpui_kit::deferred(
            div()
                .id("overlay-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::palette::Dismiss), cx)
                })
                .child(
                    gpui_kit::anchored()
                        .anchor(anchor)
                        .position(position)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .id("overlay-panel")
                                .key_context(ctx::PALETTE)
                                .occlude()
                                .w(px(WIDTH))
                                .max_h(px(560.))
                                .p_2()
                                .popover_style(cx)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(child),
                        ),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }

    fn render_overlay(&mut self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let panel = |child: gpui_kit::AnyElement, width: f32, cx: &App| {
            gpui_kit::deferred(
                div()
                    .id("overlay-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .pt(px(64.))
                    .bg(cx.theme().overlay)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::palette::Dismiss), cx)
                    })
                    .child(
                        div()
                            .id("overlay-panel")
                            .key_context(ctx::PALETTE)
                            .occlude()
                            .w(px(width))
                            .max_h(px(560.))
                            .p_3()
                            .rounded(cx.theme().radius_lg)
                            .bg(cx.theme().popover)
                            .border_1()
                            .border_color(cx.theme().border)
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(child),
                    ),
            )
            .with_priority(2)
            .into_any_element()
        };
        if let Some(sw) = &self.switcher {
            return Some(self.switcher_dropdown(sw.clone().into_any_element(), cx));
        }
        if let Some(state) = &self.palette {
            let ctx = CommandContext {
                has_engine: self.store.is_some() && self.connected(cx),
                on_containers: matches!(self.page, ShellPage::Containers(_)),
                page: self.history.current().page(),
                on_detail: self.history.current().is_detail(),
                update_ready: UpdateStore::global(cx)
                    .is_some_and(|u| u.read(cx).ready_version().is_some()),
            };
            let shell = cx.entity().downgrade();
            let el = palette::element(state, ctx, self.store.as_ref(), cx)
                // The item's action was dispatched; close (and restore focus unless the
                // action moved it, e.g. navigation).
                .on_confirm(move |_, window, cx| {
                    // Navigation entries ("Go to …") already ran; close if still open.
                    shell
                        .update(cx, |this, cx| {
                            if this.palette.is_some() {
                                this.close_overlays(window, cx);
                            }
                        })
                        .ok();
                })
                .into_any_element();
            return Some(panel(el, 560., cx));
        }
        if let Some(list) = &self.shortcuts {
            let el = v_flex()
                .gap_2()
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(s::SHORTCUTS_TITLE),
                )
                .child(list.clone())
                .into_any_element();
            return Some(panel(el, 560., cx));
        }
        None
    }
}

impl Focusable for AppShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_page_pending.replace(false)
            && self.overlay() == Overlay::None
            && !window.has_active_dialog(cx)
        {
            // Focus the new page's primary control (KBD-007). Handles are valid before their
            // element renders; the key dispatch path is resolved on the next frame.
            self.focus_page(window, cx);
        } else {
            self.ensure_focus_rendered(window, cx);
        }
        let title = self.render_title_bar(cx);
        let sidebar = self.render_sidebar(window, cx);
        let content = self.render_content(cx);
        let status = self.render_status_bar(cx);
        let overlay = self.render_overlay(cx);
        // Palette confirm closes the palette (the item action was already dispatched).
        if let Some(state) = self.palette.clone()
            && !state.focus_handle(cx).contains_focused(window, cx)
            && !self.root_focus.contains_focused(window, cx)
        {
            let _ = state;
        }
        v_flex()
            .id("app-shell")
            .key_context(ctx::WORKSPACE)
            .track_focus(&self.root_focus)
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::on_navigate))
            // M6: detail tab switches replace the route (no history entry).
            .on_action(
                cx.listener(|this, a: &crate::actions::res::ReplaceRoute, w, cx| {
                    this.replace_route(a.route.clone(), w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &GoContainers, w, cx| this.go(Route::Containers, w, cx)),
            )
            .on_action(cx.listener(|this, _: &GoImages, w, cx| this.go(Route::Images, w, cx)))
            .on_action(cx.listener(|this, _: &GoVolumes, w, cx| this.go(Route::Volumes, w, cx)))
            .on_action(cx.listener(|this, _: &GoNetworks, w, cx| {
                if AppState::config(cx).general.show_networks_page {
                    this.go(Route::Networks, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, w, cx| {
                this.go(
                    Route::Settings {
                        section: SettingsSection::General,
                    },
                    w,
                    cx,
                )
            }))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_forward))
            .on_action(cx.listener(|this, _: &detail::ParentList, w, cx| {
                // KBD-042: from a container detail, the list's cursor lands on its row.
                let from = match this.history.current() {
                    Route::ContainerDetail { id, .. } => Some(id.clone()),
                    _ => None,
                };
                let parent = this.history.current().parent_list();
                this.go(parent, w, cx);
                if let (Some(id), Some(page)) = (from, this.containers_page().cloned()) {
                    page.update(cx, |p, cx| p.reveal_row(&id, w, cx));
                }
            }))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_reset))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_next_region))
            .on_action(cx.listener(Self::on_prev_region))
            .on_action(cx.listener(Self::on_focus_notifications))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_close_window))
            .on_action(cx.listener(Self::on_minimize))
            .on_action(cx.listener(Self::on_zoom_window))
            .on_action(cx.listener(Self::on_hide))
            .on_action(cx.listener(Self::on_hide_others))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_open_logs))
            .on_action(cx.listener(Self::on_check_for_updates))
            .on_action(cx.listener(Self::on_restart_to_update))
            .on_action(cx.listener(Self::on_view_release_notes))
            .on_action(cx.listener(Self::on_manage_engines))
            .on_action(cx.listener(Self::on_add_engine))
            .on_action(cx.listener(Self::on_copy_diagnostics))
            .on_action(cx.listener(Self::on_view_licenses))
            .on_action(cx.listener(Self::on_copy_text))
            .on_action(cx.listener(Self::on_open_url))
            .on_action(cx.listener(Self::on_switch_engine))
            .on_action(cx.listener(Self::on_retry))
            .on_action(cx.listener(Self::on_start_engine))
            .on_action(cx.listener(Self::on_rescan))
            .on_action(cx.listener(Self::on_engine_switcher))
            .on_action(cx.listener(Self::on_command_palette))
            .on_action(cx.listener(Self::on_shortcuts))
            .on_action(cx.listener(Self::on_palette_dismiss))
            .on_action(cx.listener(Self::on_run_command))
            .on_action(cx.listener(Self::on_focus_menu_bar))
            .on_action(cx.listener(Self::on_escape_root))
            .on_action(cx.listener(Self::on_list_escape))
            .on_action(cx.listener(|_, _: &list::GroupBy, _, _| {}))
            .on_action(cx.listener(|_, _: &list::SortMenu, _, _| {}))
            .on_action(cx.listener(|_, _: &list::FocusFilter, _, _| {}))
            // Mouse back/forward buttons (spec 30 §2).
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, w, cx| this.on_back(&Back, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, w, cx| this.on_forward(&Forward, w, cx)),
            )
            .child(title)
            .child(
                h_flex().flex_1().min_h_0().child(sidebar).child(
                    div()
                        .id("content-region")
                        .track_focus(&self.content_focus)
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(content),
                ),
            )
            .child(status)
            .children(overlay)
            .when_some(self.overflow_menu.as_ref(), |this, m| {
                this.child(m.render())
            })
    }
}

/// Keeps the `Theme` import used on platforms where the compiler can't see it.
#[allow(dead_code)]
fn _theme(cx: &App) -> bool {
    Theme::global(cx).is_dark()
}

#[allow(dead_code)]
fn _badge() -> Badge {
    Badge::new()
}
