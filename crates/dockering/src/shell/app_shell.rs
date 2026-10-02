//! `AppShell`: the window root view (spec 30 §1). Owns the engine list store, the active
//! `EngineStore`, the navigator history, the current page, focus regions, and the global
//! actions (KBD-017…029).

use std::cell::Cell;
use std::rc::Rc;

use dk_core::{EngineId, EngineState};
use dk_hub::ThemeMode;
use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::CommandState;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
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
};
use crate::strings as s;
use crate::theme;
use crate::ui::menu::KeyMenu;
use crate::ui::notify;
use crate::ui::page::PageView;
use crate::ui::status_chip::{dot, engine_dot_color, engine_state_label};

/// The mounted page.
pub enum ShellPage {
    Containers(Entity<ContainersPage>),
    ContainerDetail(Entity<pages::container_detail::ContainerDetailPage>),
    Settings(Entity<pages::settings::SettingsPage>),
    Placeholder(Entity<pages::placeholder::PlaceholderPage>),
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
            ShellPage::None => None,
        }
    }

    pub fn primary_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self {
            ShellPage::Containers(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::ContainerDetail(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::Settings(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::Placeholder(e) => Some(e.read(cx).primary_focus(cx)),
            ShellPage::None => None,
        }
    }
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
    /// Sidebar roving cursor (index into visible pages).
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
    store_subs: Vec<Subscription>,
    _subs: Vec<Subscription>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = AppState::hub(cx);
        let engines = cx.new(|cx| EngineListStore::new(hub, cx));
        let ui = AppState::ui_state(cx);
        let start = ui
            .last_route
            .as_deref()
            .and_then(Route::from_state)
            .unwrap_or_else(|| match AppState::config(cx).general.start_page {
                dk_hub::config::StartPage::Images => Route::Images,
                dk_hub::config::StartPage::Volumes => Route::Volumes,
                _ => Route::Containers,
            });
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
            sidebar_cursor: 0,
            switcher: None,
            palette: None,
            shortcuts: None,
            overflow_menu: None,
            restore_focus: None,
            focus_page_pending: Rc::new(Cell::new(true)),
            last_focus: None,
            action_task: None,
            store_subs: Vec::new(),
            _subs: subs,
        };
        let active = this.engines.read(cx).active_id().cloned();
        this.set_engine(active, window, cx);
        this
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

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
            (Route::ContainerDetail { id, tab }, _) => ShellPage::ContainerDetail(
                pages::container_detail::new(id.clone(), *tab, self.store.clone(), window, cx),
            ),
            (Route::Images, _) => ShellPage::Placeholder(pages::images::new(window, cx)),
            (Route::Volumes, _) => ShellPage::Placeholder(pages::volumes::new(window, cx)),
            (Route::Networks, _) => ShellPage::Placeholder(pages::networks::new(window, cx)),
            (Route::ImageDetail { id, .. }, _) => {
                ShellPage::Placeholder(pages::image_detail::new(id, window, cx))
            }
            (Route::VolumeDetail { name, .. }, _) => {
                ShellPage::Placeholder(pages::volume_detail::new(name, window, cx))
            }
            (Route::NetworkDetail { id, .. }, _) => {
                ShellPage::Placeholder(pages::network_detail::new(id, window, cx))
            }
            (Route::Settings { section }, _) => {
                ShellPage::Settings(pages::settings::new(*section, window, cx))
            }
        };
        if let Some(h) = self.page.primary_focus(cx) {
            self.regions.set_default(Region::Content, h);
        }
        self.sidebar_cursor = self
            .visible_pages(cx)
            .iter()
            .position(|p| Some(*p) == route.page())
            .unwrap_or(0);
    }

    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page.primary_focus(cx) {
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

    fn sidebar_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.visible_pages(cx).len() as isize;
        if n > 0 {
            self.sidebar_cursor = (self.sidebar_cursor as isize + delta).clamp(0, n - 1) as usize;
            cx.notify();
        }
    }

    fn on_sidebar_prev(
        &mut self,
        _: &crate::actions::sidebar::Prev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_move(-1, cx);
    }
    fn on_sidebar_next(
        &mut self,
        _: &crate::actions::sidebar::Next,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_move(1, cx);
    }
    fn on_sidebar_first(
        &mut self,
        _: &crate::actions::sidebar::First,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_cursor = 0;
        cx.notify();
    }
    fn on_sidebar_last(
        &mut self,
        _: &crate::actions::sidebar::Last,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_cursor = self.visible_pages(cx).len().saturating_sub(1);
        cx.notify();
    }
    fn on_sidebar_activate(
        &mut self,
        _: &crate::actions::sidebar::Activate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.visible_pages(cx).get(self.sidebar_cursor).copied() {
            self.go(p.route(), window, cx);
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

    fn open_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.overflow_focus.clone();
        let pos = gpui_kit::point(window.viewport_size().width - px(260.), px(36.));
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
        // Return focus to the primary list if focus is elsewhere in content.
        if let Some(h) = self.page.primary_focus(cx)
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

    fn render_title_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
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
        let focused = self.switcher_btn_focus.is_focused(window);
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
                            .track_focus(&self.switcher_btn_focus)
                            .gap_2()
                            .px_2()
                            .py_0p5()
                            .items_center()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(if focused {
                                cx.theme().ring
                            } else {
                                cx.theme().border
                            })
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
                            .child(Icon::new(IconName::ChevronDown).small()),
                    )
                    .when(insecure, |this| {
                        this.child(
                            div()
                                .id("insecure-tcp")
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
                            .on_click(|_, w, cx| w.dispatch_action(Box::new(ToggleTheme), cx)),
                    )
                    .child(
                        Button::new("title-settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .tooltip_with_action(s::OPEN_SETTINGS, &OpenSettings, None)
                            .on_click(|_, w, cx| w.dispatch_action(Box::new(OpenSettings), cx)),
                    )
                    .when(!cfg!(target_os = "macos"), |this| {
                        this.child(
                            Button::new("title-overflow")
                                .ghost()
                                .small()
                                .icon(IconName::Menu)
                                .track_focus(&self.overflow_focus)
                                .tooltip(s::MORE_COMMANDS)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_overflow(window, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let route = self.history.current().clone();
        let current = route.page();
        let pages = self.visible_pages(cx);
        let focused = self.sidebar_focus.is_focused(window);
        let counts = self.store.as_ref().map(|s| {
            let s = s.read(cx);
            [
                s.count(Collection::Containers),
                s.count(Collection::Images),
                s.count(Collection::Volumes),
                s.count(Collection::Networks),
            ]
        });
        let items: Vec<SidebarMenuItem> = pages
            .iter()
            .enumerate()
            .map(|(ix, p)| {
                let p = *p;
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
                let cursor = focused && ix == self.sidebar_cursor;
                SidebarMenuItem::new(p.label())
                    .icon(icon)
                    .active(Some(p) == current)
                    .when(cursor, |this| {
                        this.label_style(gpui_kit::StyleRefinement::default().underline())
                    })
                    .when_some(count, |this, n| {
                        this.suffix(move |_, cx| {
                            div()
                                .text_xs()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().muted)
                                .child(n.to_string())
                        })
                    })
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(Navigate { route: p.route() }), cx)
                    })
            })
            .collect();
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
            .when(focused, |this| {
                this.border_1().border_color(cx.theme().ring)
            })
            .child(
                Sidebar::new("sidebar")
                    .collapsed(self.sidebar_collapsed)
                    .w(px(220.))
                    .child(SidebarMenu::new().children(items)),
            )
            .into_any_element()
    }

    fn render_status_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let status = self.active_status(cx);
        let info = self
            .store
            .as_ref()
            .and_then(|s| s.read(cx).info().cloned())
            .or_else(|| status.as_ref().and_then(|s| s.info.clone()));
        let state = status.as_ref().map(|s| s.state.clone());
        let focused = self.status_focus.is_focused(window);
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
        let right = info.as_ref().map(|i| {
            s::status_bar_resources(i.cpus, i.mem_total.map(dk_core::format::format_size))
        });
        div()
            .id("status-region")
            .track_focus(&self.status_focus)
            .when(focused, |this| {
                this.border_1().border_color(cx.theme().ring)
            })
            .child(
                StatusBar::new()
                    .left(left)
                    .when_some(right, |this, r| this.right(div().child(r))),
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

    fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
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
            return Some(panel(sw.clone().into_any_element(), 404., cx));
        }
        if let Some(state) = &self.palette {
            let ctx = CommandContext {
                has_engine: self.store.is_some() && self.connected(cx),
                on_containers: matches!(self.page, ShellPage::Containers(_)),
            };
            let shell = cx.entity().downgrade();
            let el = palette::element(state, ctx, self.store.as_ref(), window, cx)
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
        let title = self.render_title_bar(window, cx);
        let sidebar = self.render_sidebar(window, cx);
        let content = self.render_content(cx);
        let status = self.render_status_bar(window, cx);
        let overlay = self.render_overlay(window, cx);
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
                let parent = this.history.current().parent_list();
                this.go(parent, w, cx)
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
            .on_action(cx.listener(Self::on_manage_engines))
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
