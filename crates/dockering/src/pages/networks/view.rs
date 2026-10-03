//! The Networks page (NET-001…005): header (title, overflow; search, selection actions), the
//! `ListTable`, delete (confirm; `NETWORK_MGMT`; never the built-in networks) and prune. `C`
//! copies the id, `Del` deletes, `Enter` opens the detail. Creating/connecting networks is out
//! of scope (NET-005).

use std::collections::HashSet;
use std::time::Duration;

use dk_core::{Capabilities, ContainerSummary, EngineId, NetworkSummary};
use futures::FutureExt;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, point, px,
};

use super::model::{self, NetworkRow};
use super::row::{NetworksDelegate, columns};
use crate::actions::{Navigate, OnRow, RowCommand, SortByColumn, list};
use crate::nav::{NetworkTab, Route};
use crate::pages::resources::chrome;
use crate::pages::resources::ops::{self as rops, summarize};
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::list_table::{ListEvent, ListTable};
use crate::ui::menu::TrackBounds as _;
use crate::ui::menu::{KeyMenu, MenuAnchor};
use crate::ui::notify;
use crate::ui::page::{PageView, RoutedPage};

pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

type Table = ListTable<NetworksDelegate>;

pub struct NetworksPage {
    store: Entity<EngineStore>,
    engine: EngineId,
    table: Entity<Table>,
    search: Entity<InputState>,
    query: String,
    rows: Vec<NetworkRow>,
    search_task: Option<Task<()>>,
    action_tasks: Vec<Task<()>>,
    pending: HashSet<String>,
    menu: Option<KeyMenu>,
    overflow_focus: FocusHandle,
    overflow_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    _subs: Vec<Subscription>,
}

impl NetworksPage {
    pub fn new(store: Entity<EngineStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = store.read(cx).engine_id().clone();
        let table = cx.new(|cx| {
            ListTable::new(
                NetworksDelegate::default(),
                Some("networks".into()),
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
            query: String::new(),
            rows: Vec::new(),
            search_task: None,
            action_tasks: Vec::new(),
            pending: HashSet::new(),
            menu: None,
            overflow_focus: cx.focus_handle().tab_stop(true),
            overflow_bounds: Default::default(),
            _subs: subs,
        };
        this.rebuild(cx);
        this
    }

    pub fn table(&self) -> &Entity<Table> {
        &self.table
    }
    pub fn rows(&self) -> &[NetworkRow] {
        &self.rows
    }

    fn can_manage(&self, cx: &App) -> bool {
        self.store
            .read(cx)
            .capabilities()
            .contains(Capabilities::NETWORK_MGMT)
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    fn networks<'a>(&self, cx: &'a App) -> &'a [NetworkSummary] {
        self.store
            .read(cx)
            .networks
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

    fn on_store_event(
        &mut self,
        _: &Entity<EngineStore>,
        event: &EngineStoreEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineStoreEvent::Changed(Collection::Networks | Collection::Containers) => {
                self.rebuild(cx)
            }
            EngineStoreEvent::InfoChanged => self.sync_delegate(cx),
            _ => {}
        }
    }

    fn sync_delegate(&mut self, cx: &mut Context<Self>) {
        let read_only = self.read_only(cx);
        let can_manage = self.can_manage(cx);
        let pending = self.pending.clone();
        let filtered_out = !self.rows.is_empty();
        self.table.update(cx, |t, cx| {
            t.update_delegate(cx, |d| {
                d.read_only = read_only;
                d.can_manage = can_manage;
                d.pending = pending;
                d.filtered_out = filtered_out;
            })
        });
    }

    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.rows = model::rows(self.networks(cx), self.containers(cx));
        let sort = self.table.read(cx).model(cx).sort.clone();
        let nodes = model::build(&self.rows, &self.query, sort.as_ref());
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
            ListEvent::Open(key) => window.dispatch_action(
                Box::new(Navigate {
                    route: Route::NetworkDetail {
                        id: key.to_string(),
                        tab: NetworkTab::Overview,
                    },
                }),
                cx,
            ),
            ListEvent::Sort(_) => self.rebuild(cx),
            ListEvent::SelectionChanged => cx.notify(),
            _ => {}
        }
    }

    fn targets(&self, cx: &App) -> Vec<NetworkRow> {
        let t = self.table.read(cx);
        let m = t.model(cx);
        m.targets()
            .iter()
            .filter_map(|k| m.find_item(k).cloned())
            .collect()
    }

    /// The checked rows, for the selection actions (SHL-005).
    fn selection_targets(&self, cx: &App) -> Vec<NetworkRow> {
        let t = self.table.read(cx);
        let m = t.model(cx);
        m.selection_targets()
            .iter()
            .filter_map(|k| m.find_item(k).cloned())
            .collect()
    }

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
        let pos = MenuAnchor::below_or(self.overflow_bounds, point(px(640.), px(110.)));
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
        let disabled = self.read_only(cx) || !self.can_manage(cx);
        self.open_menu(window, cx, move |menu, _, _| {
            menu.menu_with_disabled(s::PRUNE_UNUSED_NETWORKS, Box::new(list::Prune), disabled)
        });
    }

    fn on_sort_menu(&mut self, _: &list::SortMenu, window: &mut Window, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).model(cx).sort.clone();
        self.open_menu(window, cx, move |menu, _, _| {
            chrome::sort_menu(menu, columns(), sort)
        });
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.targets(cx).into_iter().map(|r| r.id).collect();
        if ids.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(ids.join("\n")));
        notify::info(window, cx, s::COPIED);
    }

    /// NET-002: built-in networks and engines without `NETWORK_MGMT` can't delete.
    fn deletable(&self, targets: Vec<NetworkRow>, cx: &App) -> Vec<NetworkRow> {
        if self.read_only(cx) || !self.can_manage(cx) {
            return Vec::new();
        }
        targets.into_iter().filter(|r| !r.builtin).collect()
    }

    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets(cx);
        self.confirm_delete(targets, window, cx);
    }

    fn confirm_delete(
        &mut self,
        targets: Vec<NetworkRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_builtin = targets.iter().any(|r| r.builtin);
        let targets = self.deletable(targets, cx);
        if targets.is_empty() {
            if any_builtin {
                notify::info(window, cx, s::BUILTIN_NETWORK);
            }
            return;
        }
        let names: Vec<String> = targets.iter().map(|r| r.name.clone()).collect();
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_networks(targets.len())).items(names),
            window,
            cx,
            move |_, window, cx| {
                let targets = targets.clone();
                page.update(cx, |p, cx| p.delete(targets, window, cx)).ok();
            },
        );
    }

    fn delete(&mut self, targets: Vec<NetworkRow>, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let keys: Vec<SharedString> = targets.iter().map(|r| r.key.clone()).collect();
        for k in &keys {
            self.pending.insert(k.to_string());
        }
        self.table
            .update(cx, |t, cx| t.update_model(cx, |m| m.prepare_remove(&keys)));
        self.sync_delegate(cx);
        let call = rops::remove_many(
            &hub,
            &self.engine,
            targets
                .iter()
                .map(|r| (r.id.clone(), r.name.clone()))
                .collect(),
            |e, id| async move { e.remove_network(&id).await }.boxed(),
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
                        if ok > 1 {
                            notify::success(window, cx, s::deleted_n(s::NETWORK, ok));
                        }
                        for (name, err) in &failed {
                            notify::engine_error(
                                window,
                                cx,
                                s::delete_failed(s::NETWORK, name),
                                err,
                            );
                        }
                    }
                    Err(e) => {
                        notify::engine_error(window, cx, s::delete_failed(s::NETWORK, ""), &e)
                    }
                }
                this.store
                    .update(cx, |s, cx| s.refetch(Collection::Networks, cx));
            })
            .ok();
        });
        self.action_tasks.retain(|t| !t.is_ready());
        self.action_tasks.push(task);
    }

    /// NET-004
    fn on_prune(&mut self, _: &list::Prune, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || !self.can_manage(cx) {
            return;
        }
        let names = model::prune_candidates(&self.rows);
        let hub = AppState::hub(cx);
        let engine = self.engine.clone();
        let store = self.store.downgrade();
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_prune_networks(names.len()))
                .body(s::PRUNE_NETWORKS_BODY)
                .items(names)
                .confirm_label(s::PRUNE_UNUSED_NETWORKS),
            window,
            cx,
            move |_, window, cx| {
                let call = hub.call(&engine, |e| async move { e.prune_networks().await });
                let store = store.clone();
                let task = window.spawn(cx, async move |cx| {
                    let r = call.await;
                    cx.update(|window, cx| {
                        match r {
                            Ok(rep) => notify::success(
                                window,
                                cx,
                                s::pruned_kind(s::NETWORK, rep.deleted.len(), None),
                            ),
                            Err(e) => {
                                notify::engine_error(window, cx, s::PRUNE_UNUSED_NETWORKS, &e)
                            }
                        }
                        store
                            .update(cx, |s, cx| s.refetch(Collection::Networks, cx))
                            .ok();
                    })
                    .ok();
                });
                page.update(cx, |p, _| p.action_tasks.push(task)).ok();
            },
        );
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

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let res = self.store.read(cx).resource(Collection::Networks);
        let count = self.networks(cx).len();
        let this = cx.entity().downgrade();
        let overflow = chrome::overflow_trigger(
            "networks-overflow",
            &self.overflow_focus,
            move |window, cx| {
                this.update(cx, |p, cx| p.on_overflow(window, cx)).ok();
            },
            cx,
        )
        .on_bounds({
            let this = cx.entity().downgrade();
            move |b, _, cx| {
                this.update(cx, |p, _| p.overflow_bounds = b).ok();
            }
        })
        .into_any_element();
        let selected = self.table.read(cx).model(cx).selected().len();
        let delete_disabled = self.read_only(cx) || !self.can_manage(cx);
        chrome::page_header(
            "networks-toolbar",
            s::PAGE_NETWORKS,
            res.loading,
            [],
            &self.search,
            s::total_count(s::NETWORK, count),
            chrome::selection_actions(selected, Vec::new(), delete_disabled, cx),
            overflow,
            cx,
        )
        .into_any_element()
    }
}

impl Focusable for NetworksPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl PageView for NetworksPage {
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

impl RoutedPage for NetworksPage {}

impl Render for NetworksPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(cx);
        let res = self.store.read(cx).resource(Collection::Networks);
        let has_data = self.store.read(cx).networks.data().is_some();
        let body = chrome::list_body(
            res.first_load,
            has_data,
            res.error.as_ref(),
            "networks-error",
            self.table.clone().into_any_element(),
            cx,
        );
        // The page `⋮` ends the header (right of the search row): Shift+Tab lands on it.
        let prev = self.overflow_focus.clone();
        self.table
            .update(cx, |t, _| t.set_tab_neighbours(Some(prev), None));
        v_flex()
            .id("networks-page")
            .size_full()
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_prune))
            .on_action(cx.listener(|this, _: &list::BulkDelete, window, cx| {
                let targets = this.selection_targets(cx);
                this.confirm_delete(targets, window, cx)
            }))
            .on_action(cx.listener(Self::on_row))
            .on_action(cx.listener(Self::on_sort_by))
            .on_action(cx.listener(Self::on_sort_menu))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
            .when_some(self.menu.as_ref(), |this, m| this.child(m.render()))
    }
}
