//! The Containers page view: toolbar (search, filter, group-by, overflow), bulk bar, the
//! `ListTable`, and every container command (keys, buttons, menus share the same actions).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use dk_core::grouping::GroupBy;
use dk_core::{Capabilities, ContainerState, ContainerSummary, EngineId, StatsSample};
use dk_hub::Feed;
use futures::StreamExt;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Selectable, Sizable, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Subscription, Task, Window, div, point, px,
};

use super::model::{self, BuildInput, StatusFilter};
use super::ops::{self, Op};
use super::row::{ContainersDelegate, RowStats, columns};
use crate::actions::{
    GoImages, Navigate, OnRow, OpenUrl, RowCommand, SetFilter, SetGroupBy, SortByColumn, container,
    list,
};
use crate::keymap::ctx;
use crate::nav::{ContainerTab, Route, group_by_from_mode, group_by_to_mode};
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::confirm::{ConfirmSpec, confirm_destructive, should_confirm_stopped_delete};
use crate::ui::list_table::{ListEvent, ListTable, RowKind, SortState};
use crate::ui::menu::TrackBounds as _;
use crate::ui::menu::{KeyMenu, MenuAnchor};
use crate::ui::notify;
use crate::ui::page::PageView;
use crate::ui::widgets::{focus_wrap, port_url};

/// Search debounce (SHL-006).
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// Above this many rows the tree build runs on a background thread (SHL-006, CON spec).
pub const BACKGROUND_THRESHOLD: usize = 2_000;

type Table = ListTable<ContainersDelegate>;

pub struct ContainersPage {
    store: Entity<EngineStore>,
    engine: EngineId,
    table: Entity<Table>,
    search: Entity<InputState>,
    group_key_input: Entity<InputState>,
    filter: StatusFilter,
    group_by: GroupBy,
    query: String,
    running: usize,
    stopped: usize,
    /// Rebuild revision: stale background builds are dropped (NFR-005).
    revision: u64,
    build_task: Option<Task<()>>,
    search_task: Option<Task<()>>,
    /// In-flight actions per container (CON-031).
    pending: HashSet<String>,
    action_tasks: Vec<Task<()>>,
    /// List stats subscriptions for visible running rows (STA-006).
    stats_tasks: HashMap<String, Task<()>>,
    stats: HashMap<String, RowStats>,
    stats_flush: Option<Task<()>>,
    menu: Option<KeyMenu>,
    filter_focus: FocusHandle,
    group_focus: FocusHandle,
    overflow_focus: FocusHandle,
    group_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    overflow_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    label_prompt: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<()> for ContainersPage {}

impl ContainersPage {
    pub fn new(store: Entity<EngineStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = store.read(cx).engine_id().clone();
        let config = AppState::config(cx).clone();
        let ui = AppState::ui_state(cx);
        let filter = ui
            .container_filter
            .get(engine.as_str())
            .map(|f| StatusFilter::parse(f))
            .unwrap_or_default();
        let group_by = config.containers.group_by.clone();
        let table = cx.new(|cx| {
            ListTable::new(
                ContainersDelegate::new(),
                Some("containers".into()),
                window,
                cx,
            )
        });
        let expanded: Vec<(String, bool)> = ui
            .expanded_groups
            .get(engine.as_str())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        table.update(cx, |t, cx| {
            t.update_model(cx, |m| m.set_expanded_state(expanded))
        });

        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::SEARCH)
                .clean_on_escape()
        });
        let group_key_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(s::GROUP_LABEL_KEY));
        let mut subs = vec![
            cx.subscribe_in(&table, window, Self::on_list_event),
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe_in(&group_key_input, window, Self::on_group_key_event),
            cx.subscribe_in(&store, window, Self::on_store_event),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        // M9 (SET-020): Settings apply live: group-by default, CPU/memory columns.
        subs.push(cx.observe_global::<AppState>(|this, cx| {
            let c = &AppState::config(cx).containers;
            if c.group_by != this.group_by {
                this.group_by = c.group_by.clone();
                this.rebuild(cx);
            }
            this.sync_delegate(cx);
        }));
        // SHL-007: one app-wide ticker refreshes relative times; repaint the table cells.
        if let Some(ticker) = crate::state::Ticker::global(cx) {
            let t = table.clone();
            subs.push(cx.observe(&ticker, move |_, _, cx| t.update(cx, |_, cx| cx.notify())));
        }
        let filter_focus = cx.focus_handle().tab_stop(true);
        let group_focus = cx.focus_handle().tab_stop(true);
        let overflow_focus = cx.focus_handle().tab_stop(true);
        let mut this = Self {
            store,
            engine,
            table,
            search,
            group_key_input,
            filter,
            group_by,
            query: String::new(),
            running: 0,
            stopped: 0,
            revision: 0,
            build_task: None,
            search_task: None,
            pending: HashSet::new(),
            action_tasks: Vec::new(),
            stats_tasks: HashMap::new(),
            stats: HashMap::new(),
            stats_flush: None,
            menu: None,
            filter_focus,
            group_focus,
            overflow_focus,
            group_bounds: Default::default(),
            overflow_bounds: Default::default(),
            label_prompt: false,
            _subs: subs,
        };
        this.sync_delegate(cx);
        this.rebuild(cx);
        this
    }

    pub fn store_entity(&self) -> &Entity<EngineStore> {
        &self.store
    }

    pub fn table(&self) -> &Entity<Table> {
        &self.table
    }

    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search
    }

    pub fn filter(&self) -> StatusFilter {
        self.filter
    }

    pub fn group_by(&self) -> &GroupBy {
        &self.group_by
    }

    pub fn pending(&self) -> &HashSet<String> {
        &self.pending
    }

    /// Puts the cursor on container `id`, expanding its group (KBD-042: `Alt+↑` from the
    /// container detail lands on this row). Focuses the table.
    pub fn reveal_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.table.update(cx, |t, cx| {
            t.update_model(cx, |m| {
                let full = m
                    .all_item_keys()
                    .into_iter()
                    .find(|k| k.as_ref() == id || k.starts_with(id.as_str()));
                let Some(full) = full else { return };
                let group = m.nodes().iter().find_map(|n| match n {
                    crate::ui::list_table::ListNode::Group { key, children, .. }
                        if children.iter().any(|(k, _)| *k == full) =>
                    {
                        Some(key.clone())
                    }
                    _ => None,
                });
                if let Some(g) = group {
                    m.set_expanded(&g, true);
                }
                m.set_cursor(Some(full));
            });
        });
        let h = self.table.focus_handle(cx);
        window.focus(&h, cx);
        cx.notify();
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.store.read(cx).capabilities()
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    fn containers<'a>(&self, cx: &'a App) -> &'a [ContainerSummary] {
        self.store
            .read(cx)
            .containers
            .data()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    // ── data → rows ────────────────────────────────────────────────────────────────────

    fn on_store_event(
        &mut self,
        _: &Entity<EngineStore>,
        event: &EngineStoreEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineStoreEvent::Changed(Collection::Containers) => {
                // Clear pending flags for rows whose refetch landed (CON-031: the final state
                // comes from the refetch, never guessed).
                if !self.store.read(cx).containers.is_loading() {
                    self.rebuild(cx);
                }
            }
            EngineStoreEvent::Changed(Collection::Images) => self.sync_delegate(cx),
            EngineStoreEvent::InfoChanged => {
                self.sync_delegate(cx);
                self.update_stats_subscriptions(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn stats_enabled(&self, cx: &App) -> bool {
        let limit = self
            .store
            .read(cx)
            .info()
            .map(|i| i.list_stats_limit)
            .unwrap_or(0);
        AppState::config(cx).containers.show_cpu_mem_columns && limit > 0
    }

    /// Pushes page state into the delegate (caps, read-only, pending, stats).
    fn sync_delegate(&mut self, cx: &mut Context<Self>) {
        let caps = self.caps(cx);
        let read_only = self.read_only(cx);
        let show_stats = self.stats_enabled(cx);
        let has_images = self
            .store
            .read(cx)
            .images
            .data()
            .is_some_and(|i| !i.is_empty());
        let pending = self.pending.clone();
        let stats = self.stats.clone();
        let filtered_out = !self.containers(cx).is_empty();
        let table = self.table.clone();
        let columns_changed = table.read(cx).delegate(cx).show_stats != show_stats;
        table.update(cx, |t, cx| {
            t.update_delegate(cx, |d| {
                d.caps = caps;
                d.read_only = read_only;
                d.show_stats = show_stats;
                d.pending = pending;
                d.stats = stats;
                d.has_images = has_images;
                d.filtered_out = filtered_out;
            });
            if columns_changed {
                t.set_columns(columns(show_stats), cx);
            }
        });
    }

    /// Rebuilds the tree from the store. Above `BACKGROUND_THRESHOLD` rows the work runs on
    /// `background_spawn` (NFR-003); a revision id drops stale results.
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let rev = self.revision;
        let containers = self.containers(cx).to_vec();
        // Pending rows whose state changed are done.
        let interleave = AppState::config(cx).containers.interleave_ungrouped;
        let group_by = self.group_by.clone();
        let filter = self.filter;
        let query = self.query.clone();
        let sort = self.table.read(cx).model(cx).sort.clone();
        let stats: HashMap<String, f64> =
            self.stats.iter().map(|(k, v)| (k.clone(), v.cpu)).collect();
        let job = move || {
            let cpu = |id: &str| stats.get(id).copied();
            model::build(BuildInput {
                containers: &containers,
                group_by: &group_by,
                interleave,
                filter,
                query: &query,
                sort: sort.as_ref(),
                cpu: &cpu,
            })
        };
        let n = self.containers(cx).len();
        if n > BACKGROUND_THRESHOLD {
            let task = cx.background_spawn(async move { job() });
            self.build_task = Some(cx.spawn(async move |this, cx| {
                let out = task.await;
                this.update(cx, |this, cx| {
                    if this.revision == rev {
                        this.apply_build(out, cx);
                    }
                })
                .ok();
            }));
        } else {
            let out = job();
            self.apply_build(out, cx);
        }
    }

    fn apply_build(&mut self, out: model::BuildOutput, cx: &mut Context<Self>) {
        self.running = out.running;
        self.stopped = out.stopped;
        let nodes = out.nodes;
        let force = out.force_expanded;
        self.table.update(cx, |t, cx| {
            t.update_model(cx, |m| m.set_force_expanded(force));
            t.set_nodes(nodes, cx);
        });
        self.sync_delegate(cx);
        self.update_stats_subscriptions(cx);
        cx.notify();
    }

    // ── list events ────────────────────────────────────────────────────────────────────

    fn on_list_event(
        &mut self,
        _: &Entity<Table>,
        event: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ListEvent::Open(key) => self.open_detail(key, ContainerTab::Overview, window, cx),
            ListEvent::Sort(_) => self.rebuild(cx),
            ListEvent::Expanded { group, expanded } => {
                let engine = self.engine.to_string();
                let (group, expanded) = (group.to_string(), *expanded);
                AppState::update_ui_state(cx, move |s| {
                    s.expanded_groups
                        .entry(engine)
                        .or_insert_with(BTreeMap::new)
                        .insert(group, expanded);
                });
            }
            ListEvent::VisibleRows(_) => self.update_stats_subscriptions(cx),
            ListEvent::SelectionChanged | ListEvent::ColumnWidths(_) => cx.notify(),
        }
    }

    fn open_detail(
        &mut self,
        key: &str,
        tab: ContainerTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.table.read(cx).model(cx).find_item(key) else {
            return;
        };
        let route = Route::ContainerDetail {
            id: c.id.clone(),
            tab,
        };
        window.dispatch_action(Box::new(Navigate { route }), cx);
    }

    // ── search & filters ───────────────────────────────────────────────────────────────

    fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = event {
            let value = self.search.read(cx).value().to_string();
            // Debounce 120 ms (SHL-006); replacing the task cancels the previous one.
            self.search_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                this.update(cx, |this, cx| {
                    this.query = value;
                    this.rebuild(cx);
                })
                .ok();
            }));
        }
    }

    fn set_filter(&mut self, filter: StatusFilter, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        let engine = self.engine.to_string();
        AppState::update_ui_state(cx, move |s| {
            s.container_filter
                .insert(engine, filter.as_str().to_owned());
        });
        self.rebuild(cx);
    }

    fn on_set_filter(&mut self, a: &SetFilter, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(StatusFilter::parse(&a.filter), cx);
    }

    fn set_group_by(&mut self, g: GroupBy, cx: &mut Context<Self>) {
        self.group_by = g.clone();
        AppState::update_config(cx, move |c| c.containers.group_by = g);
        self.rebuild(cx);
    }

    fn on_set_group_by(&mut self, a: &SetGroupBy, window: &mut Window, cx: &mut Context<Self>) {
        if a.mode.as_ref() == "label:" {
            // Ask for the key (CON-010 "Label…").
            self.label_prompt = true;
            self.group_key_input.update(cx, |i, cx| i.focus(window, cx));
            cx.notify();
            return;
        }
        self.set_group_by(group_by_from_mode(&a.mode), cx);
    }

    fn on_group_key_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                let key = self.group_key_input.read(cx).value().trim().to_owned();
                self.label_prompt = false;
                if !key.is_empty() {
                    self.set_group_by(GroupBy::Label(key), cx);
                }
                let h = self.table.focus_handle(cx);
                window.focus(&h, cx);
                cx.notify();
            }
            InputEvent::Blur => {
                self.label_prompt = false;
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_sort_by(&mut self, a: &SortByColumn, _: &mut Window, cx: &mut Context<Self>) {
        let key = a.key.clone();
        let current = self.table.read(cx).model(cx).sort.clone();
        let next = match &current {
            Some(s) if s.key == key && !s.descending => Some(SortState {
                key: key.clone(),
                descending: true,
            }),
            Some(s) if s.key == key => None,
            _ => Some(SortState {
                key: key.clone(),
                descending: false,
            }),
        };
        // The ListTable reports the change (ListEvent::Sort) and we rebuild there.
        self.table.update(cx, |t, cx| t.set_sort(next, cx));
    }

    // ── menus (KBD-039, Mod+Shift+O, overflow) ─────────────────────────────────────────

    /// Opens menus under their trigger (or under the group-by button for key-only menus).
    fn menu_anchor(&self, focus: &FocusHandle, _window: &Window, _cx: &App) -> MenuAnchor {
        let b = if *focus == self.overflow_focus {
            self.overflow_bounds
        } else {
            self.group_bounds
        };
        MenuAnchor::below_or(b, point(px(320.), px(110.)))
    }

    fn open_menu(
        &mut self,
        anchor: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) {
        let pos = self.menu_anchor(&anchor, window, cx);
        let restore = self.table.focus_handle(cx);
        self.menu = Some(KeyMenu::open(
            pos,
            restore,
            window,
            cx,
            build,
            |this: &mut Self, _| this.menu = None,
        ));
        cx.notify();
    }

    fn on_group_by_menu(&mut self, _: &list::GroupBy, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.group_by.clone();
        let anchor = self.group_focus.clone();
        self.open_menu(anchor, window, cx, move |menu, _, _| {
            let item = |m: PopupMenu, label: &'static str, mode: &str, checked: bool| {
                m.menu_with_check(
                    label,
                    checked,
                    Box::new(SetGroupBy {
                        mode: mode.to_owned().into(),
                    }),
                )
            };
            let m = item(
                menu,
                s::GROUP_BY_COMPOSE,
                "compose",
                current == GroupBy::Compose,
            );
            let m = item(m, s::GROUP_BY_NONE, "none", current == GroupBy::None);
            item(
                m,
                s::GROUP_BY_LABEL,
                "label:",
                matches!(current, GroupBy::Label(_)),
            )
        });
    }

    fn on_sort_menu(&mut self, _: &list::SortMenu, window: &mut Window, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).model(cx).sort.clone();
        let anchor = self.group_focus.clone();
        let show_stats = self.stats_enabled(cx);
        self.open_menu(anchor, window, cx, move |mut menu, _, _| {
            for c in columns(show_stats).into_iter().filter(|c| c.sortable) {
                let arrow = match &sort {
                    Some(st) if st.key == c.key => {
                        if st.descending {
                            " ↓"
                        } else {
                            " ↑"
                        }
                    }
                    _ => "",
                };
                menu = menu.menu(
                    format!("{}{arrow}", c.name),
                    Box::new(SortByColumn { key: c.key.into() }),
                );
            }
            menu
        });
    }

    fn on_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = self.overflow_focus.clone();
        let ro = self.read_only(cx);
        self.open_menu(anchor, window, cx, move |menu, _, _| {
            menu.menu_with_disabled(s::PRUNE_STOPPED, Box::new(list::Prune), ro)
                .separator()
                .menu(s::CMD_COLLAPSE_ALL, Box::new(list::CollapseAll))
                .menu(s::CMD_EXPAND_ALL, Box::new(list::ExpandAll))
        });
    }

    fn on_focus_filter(
        &mut self,
        _: &list::FocusFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.filter_focus, cx);
    }

    // ── container commands (KBD-030/038, CON-020…023) ──────────────────────────────────

    /// Containers the command applies to: the multi-selection (≥ 2) or the cursor row; a
    /// group row stands for its members.
    fn targets(&self, cx: &App) -> Vec<ContainerSummary> {
        let t = self.table.read(cx);
        let model = t.model(cx);
        model
            .targets()
            .iter()
            .filter_map(|k| model.find_item(k).cloned())
            .collect()
    }

    fn cursor_is_group(&self, cx: &App) -> bool {
        self.table
            .read(cx)
            .cursor_row(cx)
            .is_some_and(|r| r.is_group())
    }

    fn can_act(&self, cx: &App) -> bool {
        !self.read_only(cx)
    }

    fn start_op(
        &mut self,
        op: Op,
        targets: Vec<ContainerSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() || !self.can_act(cx) {
            return;
        }
        if let Some(cap) = match op {
            Op::Pause | Op::Unpause => Some(Capabilities::PAUSE),
            _ => None,
        } && !self.caps(cx).contains(cap)
        {
            return; // capability-gated (ENG-030)
        }
        let hub = AppState::hub(cx);
        let engine = self.engine.clone();
        for t in &targets {
            self.pending.insert(t.id.clone());
        }
        self.sync_delegate(cx);
        cx.notify();
        let verb = op.verb();
        let ids: Vec<String> = targets.iter().map(|t| t.id.clone()).collect();
        let single_name = (targets.len() == 1).then(|| targets[0].name.clone());
        let is_remove = matches!(op, Op::Remove { .. });
        if is_remove {
            let keys: Vec<SharedString> = ids.iter().map(|i| i.clone().into()).collect();
            self.table
                .update(cx, |t, cx| t.update_model(cx, |m| m.prepare_remove(&keys)));
        }
        let call = ops::run(&hub, &engine, op, targets);
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                for id in &ids {
                    this.pending.remove(id);
                }
                this.sync_delegate(cx);
                match result {
                    Ok(outcome) => {
                        let (ok, failed) = ops::summarize(&outcome);
                        if failed.is_empty() {
                            if ok > 1 {
                                notify::success(window, cx, s::action_done(verb, ok));
                            }
                        } else if ok == 0 && failed.len() == 1 {
                            let (name, err) = &failed[0];
                            notify::error(window, cx, s::action_failed(verb, name), err.clone());
                        } else {
                            let details = failed
                                .iter()
                                .map(|(n, e)| format!("{n}: {e}"))
                                .collect::<Vec<_>>()
                                .join("\n");
                            notify::error(
                                window,
                                cx,
                                s::action_partial(verb, ok, failed.len()),
                                details,
                            );
                        }
                    }
                    Err(err) => {
                        let name = single_name.unwrap_or_default();
                        notify::engine_error(window, cx, s::action_failed(verb, &name), &err);
                    }
                }
                // The final state comes from the refetch (CON-031); events usually beat us.
                this.store
                    .update(cx, |s, cx| s.refetch(Collection::Containers, cx));
                cx.notify();
            })
            .ok();
        });
        self.action_tasks.retain(|t| !t.is_ready());
        self.action_tasks.push(task);
    }

    fn on_start_stop(
        &mut self,
        _: &container::StartStop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.targets(cx);
        if targets.is_empty() {
            return;
        }
        // Multi-selection or group: stop if anything runs, else start.
        let any_running = targets
            .iter()
            .any(|c| c.state.is_running() || c.state == ContainerState::Paused);
        let op = if any_running { Op::Stop } else { Op::Start };
        let targets = if any_running {
            targets
                .into_iter()
                .filter(|c| c.state.is_running() || c.state == ContainerState::Paused)
                .collect()
        } else {
            targets
        };
        self.start_op(op, targets, window, cx);
    }

    fn on_restart(&mut self, _: &container::Restart, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets(cx);
        self.start_op(Op::Restart, targets, window, cx);
    }

    fn on_pause(
        &mut self,
        _: &container::PauseToggle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.caps(cx).contains(Capabilities::PAUSE) {
            return;
        }
        let targets = self.targets(cx);
        let any_paused = targets.iter().any(|c| c.state == ContainerState::Paused);
        let (op, targets): (Op, Vec<_>) = if any_paused {
            (
                Op::Unpause,
                targets
                    .into_iter()
                    .filter(|c| c.state == ContainerState::Paused)
                    .collect(),
            )
        } else {
            (
                Op::Pause,
                targets
                    .into_iter()
                    .filter(|c| c.state.is_running())
                    .collect(),
            )
        };
        self.start_op(op, targets, window, cx);
    }

    fn on_kill(&mut self, _: &container::Kill, window: &mut Window, cx: &mut Context<Self>) {
        let targets: Vec<_> = self
            .targets(cx)
            .into_iter()
            .filter(|c| c.state.is_running() || c.state == ContainerState::Paused)
            .collect();
        if targets.is_empty() || !self.can_act(cx) {
            return;
        }
        let names: Vec<String> = targets.iter().map(|c| c.name.clone()).collect();
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::CONFIRM_KILL_TITLE)
                .body(s::CONFIRM_KILL_BODY)
                .items(names)
                .confirm_label(s::ACTION_KILL),
            window,
            cx,
            move |_, window, cx| {
                let targets = targets.clone();
                page.update(cx, |p, cx| p.start_op(Op::Kill, targets, window, cx))
                    .ok();
            },
        );
    }

    fn on_logs(&mut self, _: &container::Logs, window: &mut Window, cx: &mut Context<Self>) {
        self.open_cursor_tab(ContainerTab::Logs, window, cx);
    }

    fn on_terminal(
        &mut self,
        _: &container::Terminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.caps(cx).contains(Capabilities::EXEC_TTY) {
            return;
        }
        let running = self.cursor_item(cx).is_some_and(|c| c.state.is_running());
        if running {
            self.open_cursor_tab(ContainerTab::Terminal, window, cx);
        }
    }

    fn on_inspect(&mut self, _: &container::Inspect, window: &mut Window, cx: &mut Context<Self>) {
        self.open_cursor_tab(ContainerTab::Inspect, window, cx);
    }

    fn cursor_item(&self, cx: &App) -> Option<ContainerSummary> {
        self.table
            .read(cx)
            .cursor_row(cx)
            .and_then(|r| r.item().cloned())
    }

    fn open_cursor_tab(&mut self, tab: ContainerTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = self.cursor_item(cx) {
            self.open_detail(&c.id, tab, window, cx);
        }
    }

    fn on_open_port(
        &mut self,
        _: &container::OpenPort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.cursor_item(cx) else {
            return;
        };
        match c.ports.iter().find_map(port_url) {
            Some(url) => window.dispatch_action(Box::new(OpenUrl { url: url.into() }), cx),
            None => notify::info(window, cx, s::NO_PUBLISHED_PORT),
        }
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = if self.cursor_is_group(cx) {
            Vec::new()
        } else {
            self.targets(cx).into_iter().map(|c| c.id).collect()
        };
        if ids.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(ids.join("\n")));
        notify::info(window, cx, s::COPIED);
    }

    /// `Del` (CON-021, KBD-030): running containers need Force; stopped ones confirm unless
    /// "Don't ask again" was chosen (persisted in `general.confirm_delete_stopped`).
    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_act(cx) {
            return;
        }
        let is_group =
            self.cursor_is_group(cx) && self.table.read(cx).model(cx).selected().len() < 2;
        let targets = self.targets(cx);
        if targets.is_empty() {
            return;
        }
        if is_group {
            self.confirm_group_delete(targets, window, cx);
            return;
        }
        self.delete_targets(targets, window, cx);
    }

    pub fn delete_targets(
        &mut self,
        targets: Vec<ContainerSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let running = targets
            .iter()
            .filter(|c| c.state.is_running() || c.state == ContainerState::Paused)
            .count();
        let names: Vec<String> = targets.iter().map(|c| c.name.clone()).collect();
        let page = cx.entity().downgrade();
        if running > 0 {
            confirm_destructive(
                ConfirmSpec::new(s::CONFIRM_FORCE_RUNNING_TITLE)
                    .body(s::confirm_force_running_body(running))
                    .items(names)
                    .confirm_label(s::FORCE_DELETE),
                window,
                cx,
                move |_, window, cx| {
                    let targets = targets.clone();
                    page.update(cx, |p, cx| {
                        p.start_op(Op::Remove { force: true }, targets, window, cx)
                    })
                    .ok();
                },
            );
            return;
        }
        if !should_confirm_stopped_delete(cx) {
            self.start_op(Op::Remove { force: false }, targets, window, cx);
            return;
        }
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_title(targets.len()))
                .items(names)
                .option(s::DONT_ASK_AGAIN, false),
            window,
            cx,
            move |options, window, cx| {
                if options.first().copied().unwrap_or(false) {
                    AppState::update_config(cx, |c| c.general.confirm_delete_stopped = false);
                }
                let targets = targets.clone();
                page.update(cx, |p, cx| {
                    p.start_op(Op::Remove { force: false }, targets, window, cx)
                })
                .ok();
            },
        );
    }

    /// CON-013 Delete all: lists members, Force option, networks/volumes kept.
    fn confirm_group_delete(
        &mut self,
        members: Vec<ContainerSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self
            .table
            .read(cx)
            .cursor_row(cx)
            .and_then(|r| r.group().map(|g| g.group.label.clone()))
            .unwrap_or_default();
        let any_running = members
            .iter()
            .any(|c| c.state.is_running() || c.state == ContainerState::Paused);
        let names: Vec<String> = members
            .iter()
            .map(|c| {
                if c.compose.as_ref().is_some_and(|ci| ci.oneoff) {
                    format!("{} ({})", c.name, s::ONEOFF_TAG)
                } else {
                    c.name.clone()
                }
            })
            .collect();
        let mut spec = ConfirmSpec::new(s::confirm_delete_group_title(&project))
            .items(names)
            .note(s::KEEP_NETWORKS_NOTE)
            .confirm_label(s::ACTION_DELETE_ALL);
        if any_running {
            spec = spec.option(s::FORCE_RUNNING, false);
        }
        let page = cx.entity().downgrade();
        confirm_destructive(spec, window, cx, move |options, window, cx| {
            let force = options.first().copied().unwrap_or(false);
            // Without Force, running members are skipped (they'd fail with a conflict).
            let targets: Vec<ContainerSummary> = members
                .iter()
                .filter(|c| force || !(c.state.is_running() || c.state == ContainerState::Paused))
                .cloned()
                .collect();
            page.update(cx, |p, cx| {
                p.start_op(Op::Remove { force }, targets, window, cx)
            })
            .ok();
        });
    }

    fn on_bulk_start(&mut self, _: &list::BulkStart, window: &mut Window, cx: &mut Context<Self>) {
        let t: Vec<_> = self
            .targets(cx)
            .into_iter()
            .filter(|c| !c.state.is_running())
            .collect();
        self.start_op(Op::Start, t, window, cx);
    }

    fn on_bulk_stop(&mut self, _: &list::BulkStop, window: &mut Window, cx: &mut Context<Self>) {
        let t: Vec<_> = self
            .targets(cx)
            .into_iter()
            .filter(|c| c.state.is_running() || c.state == ContainerState::Paused)
            .collect();
        self.start_op(Op::Stop, t, window, cx);
    }

    fn on_bulk_delete(
        &mut self,
        _: &list::BulkDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.targets(cx);
        if !t.is_empty() && self.can_act(cx) {
            self.delete_targets(t, window, cx);
        }
    }

    /// CON-023
    fn on_prune(&mut self, _: &list::Prune, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_act(cx) {
            return;
        }
        let stopped: Vec<String> = self
            .containers(cx)
            .iter()
            .filter(|c| c.state.is_stopped())
            .map(|c| c.name.clone())
            .collect();
        let hub = AppState::hub(cx);
        let engine = self.engine.clone();
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_prune_title(stopped.len()))
                .body(s::CONFIRM_PRUNE_BODY)
                .items(stopped)
                .confirm_label(s::PRUNE_STOPPED),
            window,
            cx,
            move |_, window, cx| {
                let call = ops::prune(&hub, &engine);
                let task = window.spawn(cx, async move |cx| {
                    let r = call.await;
                    cx.update(|window, cx| match r {
                        Ok(rep) => notify::success(
                            window,
                            cx,
                            s::pruned(
                                rep.deleted.len(),
                                &dk_core::format::format_size(rep.space_reclaimed),
                            ),
                        ),
                        Err(e) => notify::engine_error(window, cx, s::PRUNE_STOPPED, &e),
                    })
                    .ok();
                });
                page.update(cx, |p, _| p.action_tasks.push(task)).ok();
            },
        );
    }

    /// Row buttons and menus (`OnRow`) — the ListTable already moved the cursor.
    fn on_row(&mut self, a: &OnRow, window: &mut Window, cx: &mut Context<Self>) {
        match a.action {
            RowCommand::Start => {
                let t: Vec<_> = self
                    .targets(cx)
                    .into_iter()
                    .filter(|c| !c.state.is_running())
                    .collect();
                self.start_op(Op::Start, t, window, cx)
            }
            RowCommand::Stop => {
                let t: Vec<_> = self
                    .targets(cx)
                    .into_iter()
                    .filter(|c| c.state.is_running() || c.state == ContainerState::Paused)
                    .collect();
                self.start_op(Op::Stop, t, window, cx)
            }
            RowCommand::Restart => self.on_restart(&container::Restart, window, cx),
            RowCommand::Pause | RowCommand::Unpause => {
                self.on_pause(&container::PauseToggle, window, cx)
            }
            RowCommand::Kill => self.on_kill(&container::Kill, window, cx),
            RowCommand::Delete => self.on_delete(&list::Delete, window, cx),
            RowCommand::Logs => self.on_logs(&container::Logs, window, cx),
            RowCommand::Terminal => self.on_terminal(&container::Terminal, window, cx),
            RowCommand::Inspect => self.on_inspect(&container::Inspect, window, cx),
            RowCommand::CopyId => self.on_copy_id(&list::CopyId, window, cx),
            RowCommand::OpenPort => self.on_open_port(&container::OpenPort, window, cx),
            RowCommand::ContextMenu => {
                let h = self.table.focus_handle(cx);
                window.focus(&h, cx);
                h.dispatch_action(&list::ContextMenu, window, cx);
            }
            RowCommand::Open | RowCommand::ToggleGroup | RowCommand::ToggleSelected => {}
        }
    }

    // ── list stats (CON-002, STA-006) ──────────────────────────────────────────────────

    /// Subscribes to `hub.stats` for visible running rows only, capped at
    /// `EngineInfo.list_stats_limit`; hidden when that is 0.
    fn update_stats_subscriptions(&mut self, cx: &mut Context<Self>) {
        if !self.stats_enabled(cx) {
            self.stats_tasks.clear();
            return;
        }
        let limit = self
            .store
            .read(cx)
            .info()
            .map(|i| i.list_stats_limit as usize)
            .unwrap_or(0);
        let wanted: Vec<String> = {
            let t = self.table.read(cx);
            let m = t.model(cx);
            let range = m.visible.clone();
            m.rows()
                .get(range.start.min(m.rows().len())..range.end.min(m.rows().len()))
                .unwrap_or(&[])
                .iter()
                .flat_map(|r| match &r.kind {
                    RowKind::Item(c) if c.state.is_running() => vec![c.id.clone()],
                    RowKind::Group {
                        group,
                        expanded: false,
                        ..
                    } => group
                        .members
                        .iter()
                        .filter(|c| c.state.is_running())
                        .map(|c| c.id.clone())
                        .collect(),
                    _ => vec![],
                })
                .take(limit)
                .collect()
        };
        let wanted_set: HashSet<&String> = wanted.iter().collect();
        self.stats_tasks.retain(|id, _| wanted_set.contains(id));
        let hub = AppState::hub(cx);
        for id in wanted {
            if self.stats_tasks.contains_key(&id) {
                continue;
            }
            let mut stream = hub.stats(&self.engine, &id);
            let key = id.clone();
            let task = cx.spawn(async move |this, cx| {
                while let Some(item) = stream.next().await {
                    let Ok(Feed::Item(sample)) = item else {
                        continue;
                    };
                    let sample: StatsSample = sample;
                    let ok = this
                        .update(cx, |this, cx| {
                            this.stats.insert(
                                key.clone(),
                                RowStats {
                                    cpu: sample.cpu_percent,
                                    mem: sample.mem_used,
                                },
                            );
                            this.schedule_stats_flush(cx);
                        })
                        .is_ok();
                    if !ok {
                        break;
                    }
                }
            });
            self.stats_tasks.insert(id, task);
        }
    }

    /// Coalesces stats updates: one delegate sync + notify per 500 ms.
    fn schedule_stats_flush(&mut self, cx: &mut Context<Self>) {
        if self.stats_flush.is_some() {
            return;
        }
        self.stats_flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| {
                this.stats_flush = None;
                this.sync_delegate(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let loading = self.store.read(cx).containers.is_loading();
        let filter = self.filter;
        let filter_focused = self.filter_focus.is_focused(window);
        let group_label = match &self.group_by {
            GroupBy::Compose => s::GROUP_BY_COMPOSE.to_owned(),
            GroupBy::None => s::GROUP_BY_NONE.to_owned(),
            GroupBy::Label(k) => format!("label: {k}"),
        };
        v_flex()
            .id("containers-toolbar")
            .key_context(ctx::TOOLBAR)
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(s::PAGE_CONTAINERS),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(s::containers_counts(self.running, self.stopped)),
                    )
                    .when(loading, |this| this.child(Spinner::new().small()))
                    .child(div().flex_1())
                    .child(
                        div().w(px(240.)).child(
                            Input::new(&self.search)
                                .small()
                                .cleanable(true)
                                .prefix(gpui_kit::component::Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(
                        div()
                            .id("status-filter")
                            .track_focus(&self.filter_focus)
                            .rounded(cx.theme().radius)
                            .map(|el| crate::ui::focus_ring(el, filter_focused, cx))
                            .on_key_down(cx.listener(
                                move |this, e: &gpui_kit::KeyDownEvent, _, cx| {
                                    let ix = StatusFilter::ALL
                                        .iter()
                                        .position(|f| *f == this.filter)
                                        .unwrap_or(0);
                                    let next = match e.keystroke.key.as_str() {
                                        "left" => Some(ix.saturating_sub(1)),
                                        "right" => Some((ix + 1).min(2)),
                                        _ => None,
                                    };
                                    if let Some(n) = next {
                                        this.set_filter(StatusFilter::ALL[n], cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .child(ButtonGroup::new("filter-group").small().children(
                                StatusFilter::ALL.iter().map(|f| {
                                    let f = *f;
                                    Button::new(f.as_str())
                                        .label(f.label())
                                        .selected(f == filter)
                                        .tab_stop(false)
                                        .on_click(move |_, window, cx| {
                                            window.dispatch_action(
                                                Box::new(SetFilter {
                                                    filter: f.as_str().into(),
                                                }),
                                                cx,
                                            )
                                        })
                                }),
                            )),
                    )
                    .child(
                        focus_wrap(
                            "group-by-wrap",
                            &self.group_focus,
                            Button::new("group-by")
                                .small()
                                .outline()
                                .label(format!("{}: {group_label}", s::GROUP_BY))
                                .dropdown_caret(true)
                                .tooltip_with_action(s::CMD_GROUP_BY, &list::GroupBy, None)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(list::GroupBy), cx)
                                }),
                            |_, window, cx| window.dispatch_action(Box::new(list::GroupBy), cx),
                            window,
                            cx,
                        )
                        .on_bounds({
                            let this = cx.entity().downgrade();
                            move |b, _, cx| {
                                this.update(cx, |p, _| p.group_bounds = b).ok();
                            }
                        }),
                    )
                    .child(
                        focus_wrap(
                            "overflow-wrap",
                            &self.overflow_focus,
                            Button::new("containers-overflow")
                                .small()
                                .ghost()
                                .icon(IconName::EllipsisVertical)
                                .tooltip(s::MORE_ACTIONS)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.on_overflow(window, cx)),
                                ),
                            {
                                let this = cx.entity().downgrade();
                                move |_, window, cx| {
                                    this.update(cx, |p, cx| p.on_overflow(window, cx)).ok();
                                }
                            },
                            window,
                            cx,
                        )
                        .on_bounds({
                            let this = cx.entity().downgrade();
                            move |b, _, cx| {
                                this.update(cx, |p, _| p.overflow_bounds = b).ok();
                            }
                        }),
                    ),
            )
            .when(self.label_prompt, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(div().text_sm().child(s::GROUP_BY_LABEL))
                        .child(
                            div()
                                .w(px(240.))
                                .child(Input::new(&self.group_key_input).small()),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_bulk_bar(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let n = self.table.read(cx).model(cx).selected().len();
        if n < 2 {
            return None;
        }
        let ro = self.read_only(cx);
        Some(
            h_flex()
                .id("bulk-bar")
                .mx_4()
                .mb_2()
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .rounded(cx.theme().radius)
                .bg(cx.theme().accent)
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(s::selected_count(n)),
                )
                .child(div().flex_1())
                .child(
                    Button::new("bulk-start")
                        .small()
                        .label(s::ACTION_START)
                        .disabled(ro)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(list::BulkStart), cx)),
                )
                .child(
                    Button::new("bulk-stop")
                        .small()
                        .label(s::ACTION_STOP)
                        .disabled(ro)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(list::BulkStop), cx)),
                )
                .child(
                    Button::new("bulk-delete")
                        .small()
                        .danger()
                        .label(s::ACTION_DELETE)
                        .disabled(ro)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(list::BulkDelete), cx)),
                )
                .child(
                    Button::new("bulk-clear")
                        .small()
                        .ghost()
                        .label(s::CLEAR_SELECTION)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(list::ClearSelection), cx)),
                )
                .into_any_element(),
        )
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let res = self.store.read(cx).resource(Collection::Containers);
        let has_data = self.store.read(cx).containers.data().is_some();
        if res.first_load && !has_data {
            return crate::ui::skeleton_rows(8, 6);
        }
        if let (Some(err), false) = (res.error.as_ref(), has_data) {
            return crate::ui::error_panel(
                "containers-error",
                s::LIST_LOAD_FAILED,
                err,
                Box::new(crate::actions::Refresh),
                cx,
            );
        }
        div()
            .size_full()
            .child(self.table.clone())
            .into_any_element()
    }
}

impl Focusable for ContainersPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl PageView for ContainersPage {
    fn primary_focus(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |i, cx| i.focus(window, cx));
    }

    fn clear_search_if_focused(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let focused = self.search.focus_handle(cx).is_focused(window);
        if focused && !self.search.read(cx).value().is_empty() {
            self.search.update(cx, |i, cx| i.set_value("", window, cx));
            return true;
        }
        false
    }
}

impl Render for ContainersPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(window, cx);
        let bulk = self.render_bulk_bar(cx);
        let body = self.render_body(cx);
        // Tab order: toolbar controls → table (one stop) → out (KBD-004).
        let prev = self.overflow_focus.clone();
        self.table
            .update(cx, |t, _| t.set_tab_neighbours(Some(prev), None));
        v_flex()
            .id("containers-page")
            .size_full()
            .on_action(cx.listener(Self::on_start_stop))
            .on_action(cx.listener(Self::on_restart))
            .on_action(cx.listener(Self::on_pause))
            .on_action(cx.listener(Self::on_kill))
            .on_action(cx.listener(Self::on_logs))
            .on_action(cx.listener(Self::on_terminal))
            .on_action(cx.listener(Self::on_inspect))
            .on_action(cx.listener(Self::on_open_port))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_bulk_start))
            .on_action(cx.listener(Self::on_bulk_stop))
            .on_action(cx.listener(Self::on_bulk_delete))
            .on_action(cx.listener(Self::on_prune))
            .on_action(cx.listener(Self::on_row))
            .on_action(cx.listener(Self::on_set_filter))
            .on_action(cx.listener(Self::on_set_group_by))
            .on_action(cx.listener(Self::on_sort_by))
            .on_action(cx.listener(Self::on_group_by_menu))
            .on_action(cx.listener(Self::on_sort_menu))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(|_, _: &GoImages, _, cx| cx.propagate()))
            .child(toolbar)
            .children(bulk)
            .child(div().flex_1().min_h_0().child(body))
            .when_some(self.menu.as_ref(), |this, m| this.child(m.render()))
    }
}

#[allow(dead_code)]
fn _mode(g: &GroupBy) -> String {
    group_by_to_mode(g)
}
