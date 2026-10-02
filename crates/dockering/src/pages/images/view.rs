//! The Images page (IMG-001…007): toolbar (search, filter, Pull, overflow), bulk bar, the
//! `ListTable`, and every image command. Keys, buttons and menus dispatch the same actions
//! (KBD-002): `U` run, `G` pull, `C` copy id, `Del` delete, `Enter` detail.

use std::collections::HashSet;
use std::time::Duration;

use dk_core::format::format_size;
use dk_core::{Capabilities, ContainerSummary, EngineId, ImageSummary};
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, point, px,
};

use super::dialogs::{PullDialog, RunDialog};
use super::model::{self, ImageFilter, ImageRow};
use super::ops;
use super::row::{ImagesDelegate, columns};
use crate::actions::res::{PruneDangling, PruneUnused, RunRow};
use crate::actions::{Navigate, OnRow, RowCommand, SetFilter, SortByColumn, image, list};
use crate::keymap::ctx;
use crate::nav::{ImageTab, Route};
use crate::pages::resources::chrome;
use crate::pages::resources::ops::{is_in_use, summarize};
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::list_table::{ListEvent, ListTable};
use crate::ui::menu::KeyMenu;
use crate::ui::notify;
use crate::ui::page::{PageView, RoutedPage};

/// Search debounce (SHL-006).
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

type Table = ListTable<ImagesDelegate>;

pub struct ImagesPage {
    store: Entity<EngineStore>,
    engine: EngineId,
    table: Entity<Table>,
    search: Entity<InputState>,
    filter: ImageFilter,
    query: String,
    rows: Vec<ImageRow>,
    search_task: Option<Task<()>>,
    action_tasks: Vec<Task<()>>,
    /// Row keys with an operation in flight (SHL-001).
    pending: HashSet<String>,
    menu: Option<KeyMenu>,
    filter_focus: FocusHandle,
    pull_focus: FocusHandle,
    overflow_focus: FocusHandle,
    overflow_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    /// The last Run dialog this page opened (tests).
    last_run: Option<gpui_kit::WeakEntity<RunDialog>>,
    _subs: Vec<Subscription>,
}

impl ImagesPage {
    pub fn new(store: Entity<EngineStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = store.read(cx).engine_id().clone();
        let table = cx
            .new(|cx| ListTable::new(ImagesDelegate::default(), Some("images".into()), window, cx));
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
            filter: ImageFilter::All,
            query: String::new(),
            rows: Vec::new(),
            search_task: None,
            action_tasks: Vec::new(),
            pending: HashSet::new(),
            menu: None,
            filter_focus: cx.focus_handle().tab_stop(true),
            pull_focus: cx.focus_handle().tab_stop(true),
            overflow_focus: cx.focus_handle().tab_stop(true),
            overflow_bounds: Default::default(),
            last_run: None,
            _subs: subs,
        };
        this.rebuild(cx);
        this
    }

    pub fn table(&self) -> &Entity<Table> {
        &self.table
    }
    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search
    }
    pub fn filter(&self) -> ImageFilter {
        self.filter
    }
    pub fn rows(&self) -> &[ImageRow] {
        &self.rows
    }
    pub fn last_run_dialog(&self) -> Option<Entity<RunDialog>> {
        self.last_run.as_ref().and_then(|w| w.upgrade())
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.store.read(cx).capabilities()
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    fn images<'a>(&self, cx: &'a App) -> &'a [ImageSummary] {
        self.store
            .read(cx)
            .images
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

    // ── data → rows ────────────────────────────────────────────────────────────────────

    fn on_store_event(
        &mut self,
        _: &Entity<EngineStore>,
        event: &EngineStoreEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineStoreEvent::Changed(Collection::Images | Collection::Containers) => {
                self.rebuild(cx)
            }
            EngineStoreEvent::InfoChanged => self.sync_delegate(cx),
            _ => {}
        }
    }

    fn sync_delegate(&mut self, cx: &mut Context<Self>) {
        let read_only = self.read_only(cx);
        let pending = self.pending.clone();
        let filtered_out = !self.rows.is_empty();
        self.table.update(cx, |t, cx| {
            t.update_delegate(cx, |d| {
                d.read_only = read_only;
                d.pending = pending;
                d.filtered_out = filtered_out;
            })
        });
    }

    /// Rebuilds the rows from the store (cheap; image lists are small, SHL-006 threshold is
    /// for 2,000+ rows and a per-tag flatten stays well below the frame budget).
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.rows = model::rows(self.images(cx), self.containers(cx));
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
            ListEvent::Open(key) => self.open_detail(key, window, cx),
            ListEvent::Sort(_) => self.rebuild(cx),
            ListEvent::SelectionChanged => cx.notify(),
            _ => {}
        }
    }

    fn row(&self, key: &str, cx: &App) -> Option<ImageRow> {
        self.table.read(cx).model(cx).find_item(key).cloned()
    }

    fn cursor_row(&self, cx: &App) -> Option<ImageRow> {
        self.table
            .read(cx)
            .cursor_row(cx)
            .and_then(|r| r.item().cloned())
    }

    /// Rows a command applies to: the multi-selection (≥ 2) or the cursor row (KBD-038).
    fn targets(&self, cx: &App) -> Vec<ImageRow> {
        let t = self.table.read(cx);
        let m = t.model(cx);
        m.targets()
            .iter()
            .filter_map(|k| m.find_item(k).cloned())
            .collect()
    }

    fn open_detail(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.row(key, cx) else {
            return;
        };
        window.dispatch_action(
            Box::new(Navigate {
                route: Route::ImageDetail {
                    id: r.image_id,
                    tab: ImageTab::Overview,
                },
            }),
            cx,
        );
    }

    // ── search, filter, sort ───────────────────────────────────────────────────────────

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

    pub fn set_filter(&mut self, filter: ImageFilter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.rebuild(cx);
        }
    }

    fn on_set_filter(&mut self, a: &SetFilter, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(ImageFilter::parse(&a.filter), cx);
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

    // ── menus ──────────────────────────────────────────────────────────────────────────

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
            menu.menu_with_disabled(s::PRUNE_DANGLING, Box::new(PruneDangling), ro)
                .menu_with_disabled(s::PRUNE_UNUSED_IMAGES, Box::new(PruneUnused), ro)
        });
    }

    fn on_sort_menu(&mut self, _: &list::SortMenu, window: &mut Window, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).model(cx).sort.clone();
        self.open_menu(window, cx, move |menu, _, _| {
            chrome::sort_menu(menu, columns(), sort)
        });
    }

    // ── commands ───────────────────────────────────────────────────────────────────────

    /// `G` / Pull button (IMG-004).
    fn on_pull(&mut self, _: &image::Pull, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        PullDialog::open(
            self.engine.clone(),
            self.caps(cx),
            self.store.downgrade(),
            None,
            window,
            cx,
        );
    }

    /// `U` / Run button (IMG-005).
    fn on_run(&mut self, _: &image::Run, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let Some(r) = self.cursor_row(cx) else {
            return;
        };
        let dialog = RunDialog::open(
            self.engine.clone(),
            r.reference(),
            self.store.downgrade(),
            window,
            cx,
        );
        self.last_run = Some(dialog.downgrade());
    }

    fn on_run_row(&mut self, a: &RunRow, window: &mut Window, cx: &mut Context<Self>) {
        let key = a.row.to_string();
        self.table.update(cx, |t, cx| t.focus_row(&key, cx));
        self.on_run(&image::Run, window, cx);
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let mut ids: Vec<String> = self.targets(cx).into_iter().map(|r| r.image_id).collect();
        ids.dedup();
        if ids.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(ids.join("\n")));
        notify::info(window, cx, s::COPIED);
    }

    /// `Del` (IMG-006): confirm, then untag/delete. In-use failures get a second dialog that
    /// lists the containers and offers *Force*.
    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets(cx);
        self.confirm_delete(targets, window, cx);
    }

    fn on_bulk_delete(
        &mut self,
        _: &list::BulkDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.targets(cx);
        self.confirm_delete(targets, window, cx);
    }

    pub fn confirm_delete(
        &mut self,
        targets: Vec<ImageRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() || self.read_only(cx) {
            return;
        }
        let labels: Vec<String> = targets.iter().map(|r| r.label()).collect();
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_images(targets.len())).items(labels),
            window,
            cx,
            move |_, window, cx| {
                let targets = targets.clone();
                page.update(cx, |p, cx| p.delete(targets, false, window, cx))
                    .ok();
            },
        );
    }

    fn delete(
        &mut self,
        targets: Vec<ImageRow>,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hub = AppState::hub(cx);
        let keys: Vec<SharedString> = targets.iter().map(|r| r.key.clone()).collect();
        for r in &targets {
            self.pending.insert(r.key.to_string());
        }
        self.table
            .update(cx, |t, cx| t.update_model(cx, |m| m.prepare_remove(&keys)));
        self.sync_delegate(cx);
        let call = ops::remove(
            &hub,
            &self.engine,
            targets.iter().map(|r| (r.reference(), r.label())).collect(),
            force,
        );
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                for k in &keys {
                    this.pending.remove(k.as_ref());
                }
                this.sync_delegate(cx);
                match result {
                    Ok(outcome) => {
                        let (ok, failed) = summarize(&outcome);
                        let in_use: Vec<ImageRow> = failed
                            .iter()
                            .filter(|(_, e)| is_in_use(e))
                            .filter_map(|(label, _)| {
                                targets.iter().find(|r| r.label() == *label).cloned()
                            })
                            .collect();
                        let other: Vec<&(String, dk_core::EngineError)> =
                            failed.iter().filter(|(_, e)| !is_in_use(e)).collect();
                        if ok > 1 || (ok == 1 && targets.len() > 1) {
                            notify::success(window, cx, s::deleted_n(s::IMAGE, ok));
                        }
                        if let [(label, err)] = other.as_slice() {
                            notify::engine_error(
                                window,
                                cx,
                                s::delete_failed(s::IMAGE, label),
                                err,
                            );
                        } else if !other.is_empty() {
                            let details = other
                                .iter()
                                .map(|(n, e)| format!("{n}: {e}"))
                                .collect::<Vec<_>>()
                                .join("\n");
                            notify::error(
                                window,
                                cx,
                                s::delete_partial(s::IMAGE, ok, other.len()),
                                details,
                            );
                        }
                        if !in_use.is_empty() && !force {
                            this.confirm_force(in_use, window, cx);
                        }
                    }
                    Err(e) => notify::engine_error(window, cx, s::delete_failed(s::IMAGE, ""), &e),
                }
                this.store
                    .update(cx, |s, cx| s.refetch(Collection::Images, cx));
            })
            .ok();
        });
        self.action_tasks.retain(|t| !t.is_ready());
        self.action_tasks.push(task);
    }

    /// IMG-006: "Image is in use" with the containers listed, and a *Force* button.
    pub fn confirm_force(
        &mut self,
        rows: Vec<ImageRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut users: Vec<String> = rows.iter().flat_map(|r| r.used_by.clone()).collect();
        users.sort();
        users.dedup();
        if users.is_empty() {
            users = rows.iter().map(|r| r.label()).collect();
        }
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::IMAGE_IN_USE_TITLE)
                .body(s::IMAGE_IN_USE_BODY)
                .items(users)
                .confirm_label(s::FORCE_DELETE),
            window,
            cx,
            move |_, window, cx| {
                let rows = rows.clone();
                page.update(cx, |p, cx| p.delete(rows, true, window, cx))
                    .ok();
            },
        );
    }

    /// IMG-003: prune with the reclaimable size from `disk_usage` (when `DISK_USAGE`),
    /// otherwise the sum of the candidates' sizes.
    fn prune(&mut self, dangling_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let candidates = model::prune_candidates(&self.rows, dangling_only);
        let local: u64 = candidates.iter().map(|c| c.2).sum();
        let labels: Vec<String> = candidates.iter().map(|c| c.1.clone()).collect();
        let n = candidates.len();
        let hub = AppState::hub(cx);
        let engine = self.engine.clone();
        let has_du = self.caps(cx).contains(Capabilities::DISK_USAGE);
        let page = cx.entity().downgrade();
        // Ask the engine for the reclaimable size first (fast; a df call), then confirm.
        let du = (has_du && !dangling_only).then(|| ops::reclaimable(&hub, &engine));
        let task = cx.spawn_in(window, async move |_, cx| {
            let size = match du {
                Some(call) => call.await.ok().flatten().unwrap_or(local),
                None => local,
            };
            cx.update(|window, cx| {
                let (title, body, label) = if dangling_only {
                    (
                        s::confirm_prune_images(n),
                        s::PRUNE_DANGLING_BODY,
                        s::PRUNE_DANGLING,
                    )
                } else {
                    (
                        s::confirm_prune_images(n),
                        s::PRUNE_UNUSED_BODY,
                        s::PRUNE_UNUSED_IMAGES,
                    )
                };
                let page = page.clone();
                confirm_destructive(
                    ConfirmSpec::new(title)
                        .body(body)
                        .items(labels.clone())
                        .reclaimable(format_size(size))
                        .confirm_label(label),
                    window,
                    cx,
                    move |_, window, cx| {
                        page.update(cx, |p, cx| p.run_prune(dangling_only, window, cx))
                            .ok();
                    },
                );
            })
            .ok();
        });
        self.action_tasks.push(task);
    }

    fn run_prune(&mut self, dangling_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let call = ops::prune(&hub, &self.engine, dangling_only);
        let store = self.store.downgrade();
        let task = cx.spawn_in(window, async move |_, cx| {
            let r = call.await;
            cx.update(|window, cx| {
                match r {
                    Ok(rep) => notify::success(
                        window,
                        cx,
                        s::pruned_kind(
                            s::IMAGE,
                            rep.deleted.len(),
                            Some(&format_size(rep.space_reclaimed)),
                        ),
                    ),
                    Err(e) => notify::engine_error(window, cx, s::PRUNE_UNUSED_IMAGES, &e),
                }
                store
                    .update(cx, |s, cx| s.refetch(Collection::Images, cx))
                    .ok();
            })
            .ok();
        });
        self.action_tasks.push(task);
    }

    fn on_prune_dangling(
        &mut self,
        _: &PruneDangling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prune(true, window, cx);
    }

    fn on_prune_unused(&mut self, _: &PruneUnused, window: &mut Window, cx: &mut Context<Self>) {
        self.prune(false, window, cx);
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
        let res = self.store.read(cx).resource(Collection::Images);
        let (count, size) = model::totals(self.images(cx));
        let ro = self.read_only(cx);
        let this = cx.entity().downgrade();
        v_flex()
            .id("images-toolbar")
            .key_context(ctx::TOOLBAR)
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(chrome::title_row(
                        s::PAGE_IMAGES,
                        s::total_count_size(s::IMAGE, count, &format_size(size)),
                        res.loading,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(chrome::search_box(&self.search))
                    .child(chrome::filter_segment(
                        "image-filter",
                        &self.filter_focus,
                        ImageFilter::OPTIONS,
                        self.filter.as_str(),
                        window,
                        cx,
                    ))
                    .child(
                        chrome::header_button(
                            "pull-wrap",
                            &self.pull_focus,
                            Button::new("pull-image")
                                .small()
                                .primary()
                                .icon(IconName::ArrowDown)
                                .label(s::PULL)
                                .disabled(ro)
                                .tooltip(crate::keymap::tooltip_for(
                                    s::PULL_IMAGE,
                                    &image::Pull,
                                    ctx::LIST_KEYS,
                                )),
                            Box::new(image::Pull),
                            window,
                            cx,
                        )
                        .debug_selector(|| "pull-image-trigger".into()),
                    )
                    .child(
                        chrome::overflow_trigger(
                            "images-overflow",
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

impl Focusable for ImagesPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl PageView for ImagesPage {
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

impl RoutedPage for ImagesPage {}

impl Render for ImagesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(window, cx);
        let selected = self.table.read(cx).model(cx).selected().len();
        let bulk = chrome::bulk_bar(selected, self.read_only(cx), cx);
        let res = self.store.read(cx).resource(Collection::Images);
        let has_data = self.store.read(cx).images.data().is_some();
        let body = chrome::list_body(
            res.first_load,
            has_data,
            res.error.as_ref(),
            "images-error",
            self.table.clone().into_any_element(),
            cx,
        );
        let prev = self.overflow_focus.clone();
        self.table
            .update(cx, |t, _| t.set_tab_neighbours(Some(prev), None));
        v_flex()
            .id("images-page")
            .size_full()
            .on_action(cx.listener(Self::on_pull))
            .on_action(cx.listener(Self::on_run))
            .on_action(cx.listener(Self::on_run_row))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_bulk_delete))
            .on_action(cx.listener(Self::on_prune_dangling))
            .on_action(cx.listener(Self::on_prune_unused))
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

/// Keeps trait imports used regardless of cfg.
#[allow(dead_code)]
fn _traits(cx: &App) -> gpui_kit::Hsla {
    let _ = Sizable::small(Button::new("x"));
    cx.theme().border
}
