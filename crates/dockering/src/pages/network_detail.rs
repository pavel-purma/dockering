//! Network detail (NET-003): tabs Overview (id, driver, scope, internal, attachable, IPv6,
//! IPAM config, options, labels) / Containers (name link, IPv4, IPv6, MAC) / Inspect.
//! Header: Copy id, Delete (`NETWORK_MGMT`, never the built-in networks).

use dk_core::format::format_timestamp;
use dk_core::{Capabilities, EngineId, NetworkDetails};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{IconName, Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, Subscription, Task, Window, div,
};

use crate::actions::res::ReplaceRoute;
use crate::actions::{Navigate, detail, list};
use crate::assets::Lucide;
use crate::nav::{ContainerTab, NetworkTab, Route};
use crate::pages::resources::detail::{
    InspectView, LinkList, Loaded, TabSpec, bool_value, gone_banner, header, header_action,
    kv_section, lines_value, map_value, mono_value, route_link, step_tab, tab_bar, text_value,
};
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::breadcrumb::{Crumb, breadcrumb};
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::notify;
use crate::ui::page::{PageView, RoutedPage};

pub const TABS: [NetworkTab; 3] = [
    NetworkTab::Overview,
    NetworkTab::Containers,
    NetworkTab::Inspect,
];

fn tab_spec(t: NetworkTab) -> TabSpec {
    match t {
        NetworkTab::Overview => TabSpec::new(s::TAB_OVERVIEW, IconName::LayoutDashboard),
        NetworkTab::Containers => TabSpec::new(s::TAB_CONTAINERS, Lucide::Boxes),
        NetworkTab::Inspect => TabSpec::new(s::TAB_INSPECT, Lucide::Braces),
    }
}

pub struct NetworkDetailPage {
    id: String,
    tab: NetworkTab,
    store: Entity<EngineStore>,
    engine: EngineId,
    details: Loaded<NetworkDetails>,
    revision: u64,
    fetch: Option<Task<()>>,
    action_task: Option<Task<()>>,
    inspect: Entity<InspectView>,
    containers: LinkList,
    header_focus: FocusHandle,
    tabs_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl NetworkDetailPage {
    pub fn new(
        id: String,
        tab: NetworkTab,
        store: Entity<EngineStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let engine = store.read(cx).engine_id().clone();
        let subs = vec![
            cx.subscribe_in(
                &store,
                window,
                |this, _, e: &EngineStoreEvent, window, cx| {
                    let refetch = match e {
                        EngineStoreEvent::Changed(Collection::Networks) => {
                            !this.store.read(cx).networks.is_loading()
                        }
                        EngineStoreEvent::Changed(Collection::Containers) => {
                            !this.store.read(cx).containers.is_loading()
                        }
                        _ => false,
                    };
                    if refetch {
                        this.refresh(window, cx);
                    }
                },
            ),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        let inspect = cx.new(|cx| InspectView::new(window, cx));
        let mut this = Self {
            id,
            tab,
            store,
            engine,
            details: Loaded::Loading,
            revision: 0,
            fetch: None,
            action_task: None,
            inspect,
            containers: LinkList::new(cx),
            header_focus: cx.focus_handle().tab_stop(true),
            tabs_focus: cx.focus_handle().tab_stop(true),
            _subs: subs,
        };
        this.refresh(window, cx);
        this
    }

    pub fn tab(&self) -> NetworkTab {
        self.tab
    }
    pub fn details(&self) -> &Loaded<NetworkDetails> {
        &self.details
    }

    fn can_delete(&self, cx: &App) -> bool {
        !crate::shell::engine_read_only(cx)
            && self
                .store
                .read(cx)
                .capabilities()
                .contains(Capabilities::NETWORK_MGMT)
            && self.details.data().is_some_and(|d| !d.summary.is_builtin())
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        let rev = self.revision;
        let hub = AppState::hub(cx);
        let id = self.id.clone();
        let call = hub.call(&self.engine, move |e| async move {
            e.inspect_network(&id).await
        });
        self.fetch = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| {
                if this.revision != rev {
                    return;
                }
                match r {
                    Ok(d) => {
                        let raw = d.raw.clone();
                        this.details = Loaded::Ready(d);
                        this.inspect
                            .update(cx, |v, cx| v.set_value(raw, window, cx));
                    }
                    Err(e) => this.details = Loaded::Failed(e),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn name(&self) -> String {
        self.details
            .data()
            .map(|d| d.summary.name.clone())
            .unwrap_or_else(|| dk_core::format::short_id(&self.id).to_owned())
    }

    pub fn set_tab(&mut self, tab: NetworkTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        window.dispatch_action(
            Box::new(ReplaceRoute {
                route: Route::NetworkDetail {
                    id: self.id.clone(),
                    tab,
                },
            }),
            cx,
        );
        cx.notify();
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cur = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = step_tab(&[true; 3], cur, forward);
        self.set_tab(TABS[next], window, cx);
    }

    fn on_next_tab(&mut self, _: &detail::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }
    fn on_prev_tab(&mut self, _: &detail::PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .details
            .data()
            .map(|d| d.summary.id.clone())
            .unwrap_or_else(|| self.id.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(id));
        notify::info(window, cx, s::COPIED);
    }

    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_delete(cx) {
            if self.details.data().is_some_and(|d| d.summary.is_builtin()) {
                notify::info(window, cx, s::BUILTIN_NETWORK);
            }
            return;
        }
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_networks(1)).items([self.name()]),
            window,
            cx,
            move |_, window, cx| {
                page.update(cx, |p, cx| p.delete(window, cx)).ok();
            },
        );
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let id = self.id.clone();
        let call = hub.call(
            &self.engine,
            move |e| async move { e.remove_network(&id).await },
        );
        let name = self.name();
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| match r {
                Ok(()) => {
                    notify::success(window, cx, s::deleted_n(s::NETWORK, 1));
                    this.store
                        .update(cx, |s, cx| s.refetch(Collection::Networks, cx));
                    window.dispatch_action(
                        Box::new(Navigate {
                            route: Route::Networks,
                        }),
                        cx,
                    );
                }
                Err(e) => notify::engine_error(window, cx, s::delete_failed(s::NETWORK, &name), &e),
            })
            .ok();
        }));
    }

    fn render_state(&self, cx: &App) -> AnyElement {
        match &self.details {
            Loaded::Loading => crate::ui::skeleton_rows(6, 2),
            Loaded::Failed(e) if !self.details.is_gone() => crate::ui::error_panel(
                "network-detail-error",
                s::DETAIL_LOAD_FAILED,
                e,
                Box::new(crate::actions::Refresh),
                cx,
            ),
            _ => div().into_any_element(),
        }
    }

    fn render_overview(&self, cx: &App) -> AnyElement {
        let Some(d) = self.details.data() else {
            return self.render_state(cx);
        };
        let n = &d.summary;
        let ipam: Vec<String> = n
            .subnets
            .iter()
            .map(|c| {
                let mut parts = Vec::new();
                if let Some(s_) = &c.subnet {
                    parts.push(format!("{} {s_}", s::SUBNET));
                }
                if let Some(g) = &c.gateway {
                    parts.push(format!("{} {g}", s::GATEWAY));
                }
                if let Some(r) = &c.ip_range {
                    parts.push(format!("{} {r}", s::IP_RANGE));
                }
                parts.join(" · ")
            })
            .collect();
        kv_section(
            None,
            vec![
                (s::ID, mono_value(n.id.clone(), cx)),
                (s::NAME, text_value(Some(n.name.clone()), cx)),
                (s::DRIVER, text_value(Some(n.driver.clone()), cx)),
                (s::COL_SCOPE, text_value(Some(n.scope.clone()), cx)),
                (
                    s::CREATED,
                    text_value(Some(format_timestamp(n.created)), cx),
                ),
                (s::INTERNAL, bool_value(n.internal)),
                (s::ATTACHABLE, bool_value(n.attachable)),
                (s::IPV6, bool_value(n.ipv6)),
                (s::IPAM, lines_value(&ipam, cx)),
                (
                    s::COL_COMPOSE,
                    text_value(n.compose.as_ref().map(|c| c.project.clone()), cx),
                ),
                (s::OPTIONS, map_value(&d.options, cx)),
                (s::LABELS, map_value(&n.labels, cx)),
            ],
            cx,
        )
    }

    fn render_containers(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.details.data() else {
            return self.render_state(cx);
        };
        let rows: Vec<(Route, Vec<AnyElement>)> = d
            .containers
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let route = Route::ContainerDetail {
                    id: c.id.clone(),
                    tab: ContainerTab::Network,
                };
                let opt = |v: &Option<String>| {
                    mono_value(v.clone().unwrap_or_else(|| s::DASH.into()), cx)
                };
                (
                    route.clone(),
                    vec![
                        route_link(("net-container", ix), c.name.clone(), route),
                        opt(&c.ipv4),
                        opt(&c.ipv6),
                        opt(&c.mac),
                    ],
                )
            })
            .collect();
        self.containers.render(
            "network-containers",
            &[s::COL_NAME, s::COL_IPV4, s::COL_IPV6, s::COL_MAC],
            rows,
            s::NO_ATTACHED,
            window,
            cx,
            |p: &mut NetworkDetailPage| &mut p.containers,
        )
    }
}

impl Focusable for NetworkDetailPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.header_focus.clone()
    }
}

impl PageView for NetworkDetailPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.tabs_focus.clone()
    }
}

impl RoutedPage for NetworkDetailPage {
    fn accept_route(&mut self, route: &Route, _: &mut Window, _: &mut Context<Self>) -> bool {
        match route {
            Route::NetworkDetail { id, tab } if *id == self.id => {
                self.tab = *tab;
                true
            }
            _ => false,
        }
    }
}

impl Render for NetworkDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gone = self.details.is_gone();
        let name = self.name();
        let builtin = self.details.data().is_some_and(|d| d.summary.is_builtin());
        let can_delete = self.can_delete(cx);
        let tabs: Vec<TabSpec> = TABS.iter().map(|t| tab_spec(*t)).collect();
        let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let this = cx.entity().downgrade();
        let body = match self.tab {
            NetworkTab::Overview => self.render_overview(cx),
            NetworkTab::Containers => self.render_containers(window, cx),
            NetworkTab::Inspect => self.inspect.clone().into_any_element(),
        };
        v_flex()
            .id("network-detail")
            .size_full()
            .p_4()
            .gap_3()
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .child(breadcrumb(vec![
                Crumb::link(s::PAGE_NETWORKS, Route::Networks),
                Crumb::here(name.clone()),
            ]))
            .child(header(
                &self.header_focus,
                name,
                if builtin {
                    vec![
                        Tag::secondary()
                            .outline()
                            .small()
                            .child("built-in")
                            .into_any_element(),
                    ]
                } else {
                    vec![]
                },
                vec![
                    header_action(
                        "net-copy",
                        s::COPY_ID,
                        Some(IconName::Copy),
                        Box::new(list::CopyId),
                        false,
                    ),
                    header_action(
                        "net-delete",
                        s::DELETE,
                        None,
                        Box::new(list::Delete),
                        !can_delete,
                    ),
                ],
                cx,
            ))
            .when(gone, |this| {
                this.child(gone_banner(s::NETWORK, Route::Networks, cx))
            })
            .child(tab_bar(
                &self.tabs_focus,
                tabs,
                selected,
                move |ix, window, cx| {
                    this.update(cx, |p, cx| p.set_tab(TABS[ix], window, cx))
                        .ok();
                },
                cx,
            ))
            .child(
                div()
                    .id("network-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
