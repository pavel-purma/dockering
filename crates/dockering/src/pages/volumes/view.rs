//! The Volumes page (VOL-001…005): toolbar (search, filter, Create, overflow), bulk bar, the
//! `ListTable`, and the volume commands. `N` creates, `C` copies the name, `Del` deletes,
//! `Enter` opens the detail.
//!
//! VOL-002: sizes and in-use counts come from `disk_usage()`, fetched lazily **on this page
//! only**, after the list has data; cells show skeletons until it arrives. Without
//! `DISK_USAGE`, sizes are "—" and in-use is derived from container mounts.

use std::collections::HashSet;
use std::time::Duration;

use dk_core::format::format_size;
use dk_core::{Capabilities, ContainerSummary, EngineId, VolumeSummary};
use futures::FutureExt;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, point, px,
};

use super::dialog::CreateVolumeDialog;
use super::model::{self, UsageState, VolumeFilter, VolumeRow};
use super::row::{VolumesDelegate, columns};
use crate::actions::{Navigate, OnRow, RowCommand, SetFilter, SortByColumn, list, volume};
use crate::keymap::ctx;
use crate::nav::{Route, VolumeTab};
use crate::pages::resources::chrome;
use crate::pages::resources::ops::{self as rops, is_in_use, summarize};
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::list_table::{ListEvent, ListTable};
use crate::ui::menu::KeyMenu;
use crate::ui::notify;
use crate::ui::page::{PageView, RoutedPage};

pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

type Table = ListTable<VolumesDelegate>;

pub struct VolumesPage {
    store: Entity<EngineStore>,
    engine: EngineId,
    table: Entity<Table>,
    search: Entity<InputState>,
    filter: VolumeFilter,
    query: String,
    rows: Vec<VolumeRow>,
    usage: UsageState,
    usage_rev: u64,
    usage_task: Option<Task<()>>,
    search_task: Option<Task<()>>,
    action_tasks: Vec<Task<()>>,
    pending: HashSet<String>,
    menu: Option<KeyMenu>,
    filter_focus: FocusHandle,
    create_focus: FocusHandle,
    overflow_focus: FocusHandle,
    overflow_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    last_create: Option<gpui_kit::WeakEntity<CreateVolumeDialog>>,
    _subs: Vec<Subscription>,
}

impl VolumesPage {
    pub fn new(store: Entity<EngineStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = store.read(cx).engine_id().clone();
        let table = cx.new(|cx| {
            ListTable::new(
                VolumesDelegate::default(),
                Some("volumes".into()),
                window,
                cx,
            )
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::SEARCH)
                .clean_on_escape()
        });
        let mut subs = vec![
            cx.subscribe_in(&table, window, Self::on_list_event),
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe_in(&store, window, Self::on_store_event),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        if let Some(ticker) = crate::state::Ticker::global(cx) {
            let t = table.clone();
            subs.push(cx.observe(&ticker, move |_, _, cx| t.update(cx, |_, cx| cx.notify())));
        }
        let mut this = Self {
            store,
            engine,
            table,
            search,
            filter: VolumeFilter::All,
            query: String::new(),
            rows: Vec::new(),
            usage: UsageState::Loading,
            usage_rev: 0,
            usage_task: None,
            search_task: None,
            action_tasks: Vec::new(),
            pending: HashSet::new(),
            menu: None,
            filter_focus: cx.focus_handle().tab_stop(true),
            create_focus: cx.focus_handle().tab_stop(true),
            overflow_focus: cx.focus_handle().tab_stop(true),
            overflow_bounds: Default::default(),
            last_create: None,
            _subs: subs,
        };
        this.rebuild(cx);
        this.load_usage(cx);
        this
    }

    pub fn table(&self) -> &Entity<Table> {
        &self.table
    }
    pub fn rows(&self) -> &[VolumeRow] {
        &self.rows
    }
    pub fn usage(&self) -> &UsageState {
        &self.usage
    }
    pub fn filter(&self) -> VolumeFilter {
        self.filter
    }
    pub fn last_create_dialog(&self) -> Option<Entity<CreateVolumeDialog>> {
        self.last_create.as_ref().and_then(|w| w.upgrade())
    }

    fn caps(&self, cx: &App) -> Option<Capabilities> {
        self.store.read(cx).info().map(|i| i.capabilities)
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    fn volumes<'a>(&self, cx: &'a App) -> &'a [VolumeSummary] {
        self.store
            .read(cx)
            .volumes
            .data()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn containers<'a>(&self, cx: &'a App) -> &'a [ContainerSummary] {
        self.store
            .read(cx)
            .containers
            .data()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    // ── VOL-002: lazy disk usage ───────────────────────────────────────────────────────

    /// Fetches `disk_usage()` once the list has data (and again after the volume list
    /// changes). Revision-guarded; previous sizes stay visible while refreshing (SHL-004).
    fn load_usage(&mut self, cx: &mut Context<Self>) {
        let Some(caps) = self.caps(cx) else {
            return; // wait for EngineInfo (InfoChanged)
        };
        if !caps.contains(Capabilities::DISK_USAGE) {
            self.usage_task = None;
            if self.usage != UsageState::Unavailable {
                self.usage = UsageState::Unavailable;
                self.rebuild(cx);
            }
            return;
        }
        if self.store.read(cx).volumes.data().is_none() {
            return; // after the list renders
        }
        self.usage_rev += 1;
        let rev = self.usage_rev;
        let hub = AppState::hub(cx);
        let call = hub.call(&self.engine, |e| async move { e.disk_usage().await });
        self.usage_task = Some(cx.spawn(async move |this, cx| {
            let r = call.await;
            this.update(cx, |this, cx| {
                if this.usage_rev != rev {
                    return;
                }
                this.usage = match r {
                    Ok(du) => UsageState::Known(du),
                    Err(e) => {
                        tracing::debug!(%e, "disk_usage failed; sizes unavailable");
                        UsageState::Unavailable
                    }
                };
                this.rebuild(cx);
            })
            .ok();
        }));
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
            EngineStoreEvent::Changed(Collection::Volumes) => {
                self.rebuild(cx);
                if !self.store.read(cx).volumes.is_loading() {
                    self.load_usage(cx);
                }
            }
            EngineStoreEvent::Changed(Collection::Containers) => self.rebuild(cx),
            EngineStoreEvent::InfoChanged => {
                self.sync_delegate(cx);
                if matches!(self.usage, UsageState::Loading) || self.usage_task.is_none() {
                    self.load_usage(cx);
                }
            }
            _ => {}
        }
    }

    fn sync_delegate(&mut self, cx: &mut Context<Self>) {
        let read_only = self.read_only(cx);
        let pending = self.pending.clone();
        let filtered_out = !self.rows.is_empty();
        let sizes_loading = matches!(self.usage, UsageState::Loading);
        self.table.update(cx, |t, cx| {
            t.update_delegate(cx, |d| {
                d.read_only = read_only;
                d.pending = pending;
                d.filtered_out = filtered_out;
                d.sizes_loading = sizes_loading;
            })
        });
    }

    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.rows = model::rows(self.volumes(cx), self.containers(cx), &self.usage);
        let sort = self.table.read(cx).model(cx).sort.clone();
        let nodes = model::build(&self.rows, self.filter, &self.query, sort.as_ref());
        self.table.update(cx, |t, cx| t.set_nodes(nodes, cx));
        self.sync_delegate(cx);
        cx.notify();
    }

    fn on_list_event(
        &mut self,
        _: &Entity<Table>,
        event: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ListEvent::Open(key) => {
                window.dispatch_action(
                    Box::new(Navigate {
                        route: Route::VolumeDetail {
                            name: key.to_string(),
                            tab: VolumeTab::Overview,
                        },
                    }),
                    cx,
                );
            }
            ListEvent::Sort(_) => self.rebuild(cx),
            ListEvent::SelectionChanged => cx.notify(),
            _ => {}
        }
    }

    fn targets(&self, cx: &App) -> Vec<VolumeRow> {
        let t = self.table.read(cx);
        let m = t.model(cx);
        m.targets()
            .iter()
            .filter_map(|k| m.find_item(k).cloned())
            .collect()
    }

    // ── search, filter, sort, menus ────────────────────────────────────────────────────

    fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = event {
            let value = self.search.read(cx).value().to_string();
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

    pub fn set_filter(&mut self, filter: VolumeFilter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.rebuild(cx);
        }
    }

    fn on_set_filter(&mut self, a: &SetFilter, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(VolumeFilter::parse(&a.filter), cx);
    }

    fn on_focus_filter(
        &mut self,
        _: &list::FocusFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.filter_focus, cx);
    }

    fn on_sort_by(&mut self, a: &SortByColumn, _: &mut Window, cx: &mut Context<Self>) {
        let current = self.table.read(cx).model(cx).sort.clone();
        let next = chrome::next_sort(current.as_ref(), a.key.clone());
        self.table.update(cx, |t, cx| t.set_sort(next, cx));
    }

    fn open_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) {
        let b = self.overflow_bounds;
        let pos = if b.size.width > px(0.) {
            point(b.origin.x - px(180.), b.origin.y + b.size.height + px(4.))
        } else {
            point(px(640.), px(110.))
        };
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

    fn on_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ro = self.read_only(cx);
        self.open_menu(window, cx, move |menu, _, _| {
            menu.menu_with_disabled(s::PRUNE_UNUSED_VOLUMES, Box::new(list::Prune), ro)
        });
    }

    fn on_sort_menu(&mut self, _: &list::SortMenu, window: &mut Window, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).model(cx).sort.clone();
        self.open_menu(window, cx, move |menu, _, _| {
            chrome::sort_menu(menu, columns(), sort)
        });
    }

    // ── commands ───────────────────────────────────────────────────────────────────────

    /// `N` / Create button (VOL-004).
    fn on_create(&mut self, _: &volume::Create, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let d = CreateVolumeDialog::open(self.engine.clone(), self.store.downgrade(), window, cx);
        self.last_create = Some(d.downgrade());
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self.targets(cx).into_iter().map(|r| r.name).collect();
        if names.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
        notify::info(window, cx, s::COPIED);
    }

    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.targets(cx);
        self.confirm_delete(t, window, cx);
    }

    fn on_bulk_delete(
        &mut self,
        _: &list::BulkDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.targets(cx);
        self.confirm_delete(t, window, cx);
    }

    /// VOL-005: confirm (lists the volumes and their sizes when known).
    fn confirm_delete(
        &mut self,
        targets: Vec<VolumeRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() || self.read_only(cx) {
            return;
        }
        let items: Vec<String> = targets
            .iter()
            .map(|r| match r.size {
                Some(sz) => format!("{} ({})", r.name, format_size(sz)),
                None => r.name.clone(),
            })
            .collect();
        let total = targets
            .iter()
            .map(|r| r.size)
            .try_fold(0u64, |acc, s| s.map(|s| acc + s));
        let mut spec = ConfirmSpec::new(s::confirm_delete_volumes(targets.len()))
            .body(s::CONFIRM_DELETE_VOLUMES_BODY)
            .items(items);
        if let Some(total) = total {
            spec = spec.reclaimable(format_size(total));
        }
        let page = cx.entity().downgrade();
        confirm_destructive(spec, window, cx, move |_, window, cx| {
            let targets = targets.clone();
            page.update(cx, |p, cx| p.delete(targets, window, cx)).ok();
        });
    }

    fn delete(&mut self, targets: Vec<VolumeRow>, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let keys: Vec<SharedString> = targets.iter().map(|r| r.key.clone()).collect();
        for r in &targets {
            self.pending.insert(r.name.clone());
        }
        self.table
            .update(cx, |t, cx| t.update_model(cx, |m| m.prepare_remove(&keys)));
        self.sync_delegate(cx);
        let call = rops::remove_many(
            &hub,
            &self.engine,
            targets
                .iter()
                .map(|r| (r.name.clone(), r.name.clone()))
                .collect(),
            |e, name| async move { e.remove_volume(&name, false).await }.boxed(),
        );
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                for t in &targets {
                    this.pending.remove(&t.name);
                }
                this.sync_delegate(cx);
                match result {
                    Ok(outcome) => {
                        let (ok, failed) = summarize(&outcome);
                        if ok > 1 {
                            notify::success(window, cx, s::deleted_n(s::VOLUME, ok));
                        }
                        for (name, err) in &failed {
                            if is_in_use(err) {
                                // VOL-005: explain which containers use it.
                                let users = targets
                                    .iter()
                                    .find(|r| r.name == *name)
                                    .map(|r| r.used_by.join(", "))
                                    .filter(|u| !u.is_empty())
                                    .unwrap_or_else(|| err.to_string());
                                notify::error(
                                    window,
                                    cx,
                                    format!("{}: {name}", s::VOLUME_IN_USE_TITLE),
                                    format!("{}\n{users}", s::VOLUME_IN_USE_BODY),
                                );
                            } else {
                                notify::engine_error(
                                    window,
                                    cx,
                                    s::delete_failed(s::VOLUME, name),
                                    err,
                                );
                            }
                        }
                    }
                    Err(e) => notify::engine_error(window, cx, s::delete_failed(s::VOLUME, ""), &e),
                }
                this.store
                    .update(cx, |s, cx| s.refetch(Collection::Volumes, cx));
            })
            .ok();
        });
        self.action_tasks.retain(|t| !t.is_ready());
        self.action_tasks.push(task);
    }

    /// VOL-005: prune unused volumes (confirm with the reclaimable size when known).
    fn on_prune(&mut self, _: &list::Prune, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let (names, size) = model::prune_candidates(&self.rows);
        let mut spec = ConfirmSpec::new(s::confirm_prune_volumes(names.len()))
            .body(s::PRUNE_VOLUMES_BODY)
            .items(names)
            .confirm_label(s::PRUNE_UNUSED_VOLUMES);
        if let Some(size) = size {
            spec = spec.reclaimable(format_size(size));
        }
        let hub = AppState::hub(cx);
        let engine = self.engine.clone();
        let store = self.store.downgrade();
        let page = cx.entity().downgrade();
        confirm_destructive(spec, window, cx, move |_, window, cx| {
            let call = hub.call(&engine, |e| async move { e.prune_volumes().await });
            let store = store.clone();
            let task = window.spawn(cx, async move |cx| {
                let r = call.await;
                cx.update(|window, cx| {
                    match r {
                        Ok(rep) => notify::success(
                            window,
                            cx,
                            s::pruned_kind(
                                s::VOLUME,
                                rep.deleted.len(),
                                Some(&format_size(rep.space_reclaimed)),
                            ),
                        ),
                        Err(e) => notify::engine_error(window, cx, s::PRUNE_UNUSED_VOLUMES, &e),
                    }
                    store
                        .update(cx, |s, cx| s.refetch(Collection::Volumes, cx))
                        .ok();
                })
                .ok();
            });
            page.update(cx, |p, _| p.action_tasks.push(task)).ok();
        });
    }

    fn on_row(&mut self, a: &OnRow, window: &mut Window, cx: &mut Context<Self>) {
        match a.action {
            RowCommand::Delete => self.on_delete(&list::Delete, window, cx),
            RowCommand::CopyId => self.on_copy_id(&list::CopyId, window, cx),
            RowCommand::ContextMenu => {
                let h = self.table.focus_handle(cx);
                window.focus(&h, cx);
                h.dispatch_action(&list::ContextMenu, window, cx);
            }
            _ => {}
        }
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let res = self.store.read(cx).resource(Collection::Volumes);
        let count = self.volumes(cx).len();
        let summary = match &self.usage {
            UsageState::Known(du) => {
                let total: u64 = du.volumes.iter().map(|(_, s)| *s).sum();
                s::total_count_size(s::VOLUME, count, &format_size(total))
            }
            _ => s::total_count(s::VOLUME, count),
        };
        let ro = self.read_only(cx);
        let this = cx.entity().downgrade();
        v_flex()
            .id("volumes-toolbar")
            .key_context(ctx::TOOLBAR)
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(chrome::title_row(s::PAGE_VOLUMES, summary, res.loading, cx))
                    .child(div().flex_1())
                    .child(chrome::search_box(&self.search))
                    .child(chrome::filter_segment(
                        "volume-filter",
                        &self.filter_focus,
                        VolumeFilter::OPTIONS,
                        self.filter.as_str(),
                        window,
                        cx,
                    ))
                    .child(chrome::header_button(
                        "create-volume-wrap",
                        &self.create_focus,
                        Button::new("create-volume")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label(s::CREATE)
                            .disabled(ro)
                            .tooltip_with_action(
                                s::CREATE_VOLUME,
                                &volume::Create,
                                Some(ctx::LIST_KEYS),
                            ),
                        Box::new(volume::Create),
                        window,
                        cx,
                    ))
                    .child(
                        chrome::overflow_trigger(
                            "volumes-overflow",
                            &self.overflow_focus,
                            move |window, cx| {
                                this.update(cx, |p, cx| p.on_overflow(window, cx)).ok();
                            },
                            window,
                            cx,
                        )
                        .on_prepaint({
                            let this = cx.entity().downgrade();
                            move |b, _, cx| {
                                this.update(cx, |p, _| p.overflow_bounds = b).ok();
                            }
                        }),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for VolumesPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl PageView for VolumesPage {
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

impl RoutedPage for VolumesPage {}

impl Render for VolumesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(window, cx);
        let selected = self.table.read(cx).model(cx).selected().len();
        let bulk = chrome::bulk_bar(selected, self.read_only(cx), cx);
        let res = self.store.read(cx).resource(Collection::Volumes);
        let has_data = self.store.read(cx).volumes.data().is_some();
        let body = chrome::list_body(
            res.first_load,
            has_data,
            res.error.as_ref(),
            "volumes-error",
            self.table.clone().into_any_element(),
            cx,
        );
        let prev = self.overflow_focus.clone();
        self.table
            .update(cx, |t, _| t.set_tab_neighbours(Some(prev), None));
        v_flex()
            .id("volumes-page")
            .size_full()
            .on_action(cx.listener(Self::on_create))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_bulk_delete))
            .on_action(cx.listener(Self::on_prune))
            .on_action(cx.listener(Self::on_row))
            .on_action(cx.listener(Self::on_set_filter))
            .on_action(cx.listener(Self::on_sort_by))
            .on_action(cx.listener(Self::on_sort_menu))
            .on_action(cx.listener(Self::on_focus_filter))
            .child(toolbar)
            .children(bulk)
            .child(div().flex_1().min_h_0().child(body))
            .when_some(self.menu.as_ref(), |this, m| this.child(m.render()))
    }
}
