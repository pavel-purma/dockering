//! Volume detail (VOL-010): tabs Overview (name, driver, mountpoint, scope, created, size,
//! labels, options, status) / Used by (container links with destination and RW) / Inspect
//! (raw JSON). Browsing volume contents is a non-goal (VOL-011). Header: Copy name, Delete.

use dk_core::format::{format_size, format_timestamp};
use dk_core::{Capabilities, EngineId, VolumeDetails};
use gpui_kit::component::{IconName, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, Subscription, Task, Window, div,
};

use crate::actions::res::ReplaceRoute;
use crate::actions::{Navigate, detail, list};
use crate::assets::Lucide;
use crate::nav::{ContainerTab, Route, VolumeTab};
use crate::pages::resources::detail::{
    InspectView, LinkList, Loaded, TabSpec, gone_banner, header, header_action, kv_section,
    map_value, mono_value, route_link, step_tab, tab_bar, text_value,
};
use crate::pages::resources::ops::is_in_use;
use crate::state::{AppState, Collection, EngineStore, EngineStoreEvent};
use crate::strings as s;
use crate::ui::breadcrumb::{Crumb, breadcrumb};
use crate::ui::confirm::{ConfirmSpec, confirm_destructive};
use crate::ui::notify;
use crate::ui::page::{PageView, RoutedPage};

pub const TABS: [VolumeTab; 3] = [VolumeTab::Overview, VolumeTab::UsedBy, VolumeTab::Inspect];

fn tab_spec(t: VolumeTab) -> TabSpec {
    match t {
        VolumeTab::Overview => TabSpec::new(s::TAB_OVERVIEW, IconName::LayoutDashboard),
        VolumeTab::UsedBy => TabSpec::new(s::TAB_USED_BY, Lucide::Boxes),
        VolumeTab::Inspect => TabSpec::new(s::TAB_INSPECT, Lucide::Braces),
    }
}

pub struct VolumeDetailPage {
    name: String,
    tab: VolumeTab,
    store: Entity<EngineStore>,
    engine: EngineId,
    details: Loaded<VolumeDetails>,
    size: Option<u64>,
    revision: u64,
    fetch: Option<Task<()>>,
    size_fetch: Option<Task<()>>,
    action_task: Option<Task<()>>,
    inspect: Entity<InspectView>,
    used_by: LinkList,
    header_focus: FocusHandle,
    tabs_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl VolumeDetailPage {
    pub fn new(
        name: String,
        tab: VolumeTab,
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
                        EngineStoreEvent::Changed(Collection::Volumes) => {
                            !this.store.read(cx).volumes.is_loading()
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
            name,
            tab,
            store,
            engine,
            details: Loaded::Loading,
            size: None,
            revision: 0,
            fetch: None,
            size_fetch: None,
            action_task: None,
            inspect,
            used_by: LinkList::new(cx),
            header_focus: cx.focus_handle().tab_stop(true),
            tabs_focus: cx.focus_handle().tab_stop(true),
            _subs: subs,
        };
        this.refresh(window, cx);
        this
    }

    pub fn tab(&self) -> VolumeTab {
        self.tab
    }
    pub fn details(&self) -> &Loaded<VolumeDetails> {
        &self.details
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        let rev = self.revision;
        let hub = AppState::hub(cx);
        let name = self.name.clone();
        let call = hub.call(&self.engine, move |e| async move {
            e.inspect_volume(&name).await
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
        // Size from disk usage (one df call, only when supported; VOL-002).
        let caps = self.store.read(cx).capabilities();
        if caps.contains(Capabilities::DISK_USAGE) && self.size.is_none() {
            let call = hub.call(&self.engine, |e| async move { e.disk_usage().await });
            let name = self.name.clone();
            self.size_fetch = Some(cx.spawn(async move |this, cx| {
                if let Ok(du) = call.await {
                    this.update(cx, |this, cx| {
                        this.size = du.volumes.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
                        cx.notify();
                    })
                    .ok();
                }
            }));
        }
    }

    pub fn set_tab(&mut self, tab: VolumeTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        window.dispatch_action(
            Box::new(ReplaceRoute {
                route: Route::VolumeDetail {
                    name: self.name.clone(),
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
        cx.write_to_clipboard(ClipboardItem::new_string(self.name.clone()));
        notify::info(window, cx, s::COPIED);
    }

    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || self.details.is_gone() {
            return;
        }
        let mut spec = ConfirmSpec::new(s::confirm_delete_volumes(1))
            .body(s::CONFIRM_DELETE_VOLUMES_BODY)
            .items([self.name.clone()]);
        if let Some(sz) = self.size {
            spec = spec.reclaimable(format_size(sz));
        }
        let page = cx.entity().downgrade();
        confirm_destructive(spec, window, cx, move |_, window, cx| {
            page.update(cx, |p, cx| p.delete(window, cx)).ok();
        });
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let name = self.name.clone();
        let call = hub.call(&self.engine, move |e| async move {
            e.remove_volume(&name, false).await
        });
        let name = self.name.clone();
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| match r {
                Ok(()) => {
                    notify::success(window, cx, s::deleted_n(s::VOLUME, 1));
                    this.store
                        .update(cx, |s, cx| s.refetch(Collection::Volumes, cx));
                    window.dispatch_action(
                        Box::new(Navigate {
                            route: Route::Volumes,
                        }),
                        cx,
                    );
                }
                Err(e) if is_in_use(&e) => {
                    let users: Vec<String> = this
                        .details
                        .data()
                        .map(|d| d.used_by.iter().map(|c| c.name.clone()).collect())
                        .unwrap_or_default();
                    notify::error(
                        window,
                        cx,
                        format!("{}: {name}", s::VOLUME_IN_USE_TITLE),
                        format!("{}\n{}", s::VOLUME_IN_USE_BODY, users.join(", ")),
                    );
                }
                Err(e) => notify::engine_error(window, cx, s::delete_failed(s::VOLUME, &name), &e),
            })
            .ok();
        }));
    }

    fn render_overview(&self, cx: &App) -> AnyElement {
        let Some(d) = self.details.data() else {
            return self.render_state(cx);
        };
        let v = &d.summary;
        let n = d.used_by.len();
        let status = d
            .status
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default());
        kv_section(
            None,
            vec![
                (s::NAME, mono_value(v.name.clone(), cx)),
                (s::DRIVER, text_value(Some(v.driver.clone()), cx)),
                (s::MOUNTPOINT, mono_value(v.mountpoint.clone(), cx)),
                (s::COL_SCOPE, text_value(Some(v.scope.clone()), cx)),
                (s::CREATED, text_value(v.created.map(format_timestamp), cx)),
                (s::SIZE, text_value(self.size.map(format_size), cx)),
                (
                    s::COL_STATUS,
                    if n > 0 {
                        crate::pages::resources::detail::chip_value(
                            crate::ui::status_chip::tone_tag(
                                crate::ui::status_chip::Tone::Success,
                                s::in_use_by(n),
                            ),
                        )
                    } else {
                        text_value(Some(s::NOT_USED), cx)
                    },
                ),
                (
                    s::COL_COMPOSE,
                    text_value(v.compose.as_ref().map(|c| c.project.clone()), cx),
                ),
                (s::LABELS, map_value(&v.labels, cx)),
                (s::OPTIONS, map_value(&d.options, cx)),
                (s::DRIVER_STATUS, text_value(status, cx)),
            ],
            cx,
        )
    }

    fn render_state(&self, cx: &App) -> AnyElement {
        match &self.details {
            Loaded::Loading => crate::ui::skeleton_rows(6, 2),
            Loaded::Failed(e) if !self.details.is_gone() => crate::ui::error_panel(
                "volume-detail-error",
                s::DETAIL_LOAD_FAILED,
                e,
                Box::new(crate::actions::Refresh),
                cx,
            ),
            _ => div().into_any_element(),
        }
    }

    fn render_used_by(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.details.data() else {
            return self.render_state(cx);
        };
        let rows: Vec<(Route, Vec<AnyElement>)> = d
            .used_by
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let route = Route::ContainerDetail {
                    id: c.id.clone(),
                    tab: ContainerTab::Overview,
                };
                (
                    route.clone(),
                    vec![
                        route_link(("used-by", ix), c.name.clone(), route, cx),
                        mono_value(c.detail.clone().unwrap_or_default(), cx),
                        div()
                            .text_xs()
                            .child(match c.rw {
                                Some(true) => s::READ_WRITE,
                                Some(false) => s::READ_ONLY_SHORT,
                                None => s::DASH,
                            })
                            .into_any_element(),
                    ],
                )
            })
            .collect();
        self.used_by.render(
            "volume-used-by",
            &[s::COL_NAME, s::COL_DESTINATION, s::COL_MODE],
            rows,
            s::NOT_USED,
            window,
            cx,
            |p: &mut VolumeDetailPage| &mut p.used_by,
        )
    }
}

impl Focusable for VolumeDetailPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.header_focus.clone()
    }
}

impl PageView for VolumeDetailPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.tabs_focus.clone()
    }
}

impl RoutedPage for VolumeDetailPage {
    fn accept_route(&mut self, route: &Route, _: &mut Window, _: &mut Context<Self>) -> bool {
        match route {
            Route::VolumeDetail { name, tab } if *name == self.name => {
                self.tab = *tab;
                true
            }
            _ => false,
        }
    }
}

impl Render for VolumeDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gone = self.details.is_gone();
        let ro = self.read_only(cx) || gone;
        let tabs: Vec<TabSpec> = TABS.iter().map(|t| tab_spec(*t)).collect();
        let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let this = cx.entity().downgrade();
        let in_use = self.details.data().map(|d| d.used_by.len()).unwrap_or(0);
        let body = match self.tab {
            VolumeTab::Overview => self.render_overview(cx),
            VolumeTab::UsedBy => self.render_used_by(window, cx),
            VolumeTab::Inspect => self.inspect.clone().into_any_element(),
        };
        v_flex()
            .id("volume-detail")
            .size_full()
            .p_4()
            .gap_3()
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_delete))
            .child(breadcrumb(
                vec![
                    Crumb::link(s::PAGE_VOLUMES, Route::Volumes),
                    Crumb::here(self.name.clone()),
                ],
                cx,
            ))
            .child(header(
                &self.header_focus,
                self.name.clone(),
                if in_use > 0 {
                    vec![
                        crate::ui::status_chip::tone_tag(
                            crate::ui::status_chip::Tone::Success,
                            s::in_use_by(in_use),
                        )
                        .into_any_element(),
                    ]
                } else {
                    vec![]
                },
                vec![
                    header_action(
                        "vol-copy",
                        s::COPY_NAME,
                        Some(IconName::Copy),
                        Box::new(list::CopyId),
                        false,
                        &self.header_focus,
                    ),
                    header_action(
                        "vol-delete",
                        s::DELETE,
                        None,
                        Box::new(list::Delete),
                        ro,
                        &self.header_focus,
                    ),
                ],
                cx,
            ))
            .when(gone, |this| {
                this.child(gone_banner(s::VOLUME, Route::Volumes, cx))
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
                    .id("volume-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
