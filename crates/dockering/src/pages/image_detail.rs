//! Image detail (IMG-010, IMG-011): tabs Overview / Layers (`IMAGE_HISTORY`) / Used by /
//! Inspect; actions Run, Tag…, Delete, Copy id, Copy digest; a banner when the image is gone.
//! Single letters work on the header (KBD-041): `U` run, `C` copy id, `Del` delete.

use dk_core::format::{format_size, format_timestamp, short_id};
use dk_core::{Capabilities, EngineId, ImageDetails, ImageLayer};
use gpui_kit::component::table::{
    Table as KitTable, TableBody, TableCell, TableHead, TableHeader, TableRow,
};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Subscription, Task, Window, div,
};

use crate::actions::res::{CopyDigest, ReplaceRoute, TagImage};
use crate::actions::{detail, image, list};
use crate::assets::Lucide;
use crate::nav::{ContainerTab, ImageTab, Route};
use crate::pages::images::dialogs::{RunDialog, TagDialog};
use crate::pages::images::model::{ImageRow, rows as image_rows};
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

pub const TABS: [ImageTab; 4] = [
    ImageTab::Overview,
    ImageTab::Layers,
    ImageTab::UsedBy,
    ImageTab::Inspect,
];

fn tab_spec(t: ImageTab) -> TabSpec {
    match t {
        ImageTab::Overview => TabSpec::new(s::TAB_OVERVIEW, IconName::LayoutDashboard),
        ImageTab::Layers => TabSpec::new(s::TAB_LAYERS, Lucide::Layers),
        ImageTab::UsedBy => TabSpec::new(s::TAB_USED_BY, Lucide::Boxes),
        ImageTab::Inspect => TabSpec::new(s::TAB_INSPECT, Lucide::Braces),
    }
}

pub struct ImageDetailPage {
    id: String,
    tab: ImageTab,
    store: Entity<EngineStore>,
    engine: EngineId,
    details: Loaded<ImageDetails>,
    history: Loaded<Vec<ImageLayer>>,
    revision: u64,
    fetch: Option<Task<()>>,
    history_fetch: Option<Task<()>>,
    action_task: Option<Task<()>>,
    inspect: Entity<InspectView>,
    used_by: LinkList,
    header_focus: FocusHandle,
    tabs_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl ImageDetailPage {
    pub fn new(
        id: String,
        tab: ImageTab,
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
                    // The image list changed (tag/untag/delete): refetch the details.
                    if *e == EngineStoreEvent::Changed(Collection::Images)
                        && !this.store.read(cx).images.is_loading()
                    {
                        this.refresh(window, cx);
                    }
                    cx.notify();
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
            history: Loaded::Loading,
            revision: 0,
            fetch: None,
            history_fetch: None,
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

    pub fn tab(&self) -> ImageTab {
        self.tab
    }
    pub fn details(&self) -> &Loaded<ImageDetails> {
        &self.details
    }
    pub fn history(&self) -> &Loaded<Vec<ImageLayer>> {
        &self.history
    }
    pub fn inspect(&self) -> &Entity<InspectView> {
        &self.inspect
    }
    pub fn header_focus(&self) -> &FocusHandle {
        &self.header_focus
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.store.read(cx).capabilities()
    }

    fn read_only(&self, cx: &App) -> bool {
        crate::shell::engine_read_only(cx)
    }

    /// Fetches inspect + history (revision-guarded, NFR-005). Old data stays (SHL-004).
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        let rev = self.revision;
        let hub = AppState::hub(cx);
        let id = self.id.clone();
        let call = hub.call(
            &self.engine,
            move |e| async move { e.inspect_image(&id).await },
        );
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
        if self.caps(cx).contains(Capabilities::IMAGE_HISTORY) {
            let id = self.id.clone();
            let call = hub.call(
                &self.engine,
                move |e| async move { e.image_history(&id).await },
            );
            self.history_fetch = Some(cx.spawn(async move |this, cx| {
                let r = call.await;
                this.update(cx, |this, cx| {
                    if this.revision != rev {
                        return;
                    }
                    this.history = match r {
                        Ok(h) => Loaded::Ready(h),
                        Err(e) => Loaded::Failed(e),
                    };
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Tags of this image as list rows (for Run / Delete / labels).
    fn rows(&self, cx: &App) -> Vec<ImageRow> {
        let s = self.store.read(cx);
        let images = s.images.data().map(Vec::as_slice).unwrap_or(&[]);
        let containers = s.containers.data().map(Vec::as_slice).unwrap_or(&[]);
        image_rows(images, containers)
            .into_iter()
            .filter(|r| r.image_id == self.id)
            .collect()
    }

    fn title(&self, cx: &App) -> String {
        if let Some(d) = self.details.data()
            && let Some(t) = d.summary.repo_tags.first()
        {
            return t.clone();
        }
        self.rows(cx)
            .first()
            .map(|r| r.label())
            .unwrap_or_else(|| short_id(&self.id).to_owned())
    }

    fn enabled_tabs(&self, cx: &App) -> Vec<bool> {
        let hist = self.caps(cx).contains(Capabilities::IMAGE_HISTORY);
        TABS.iter()
            .map(|t| *t != ImageTab::Layers || hist)
            .collect()
    }

    pub fn set_tab(&mut self, tab: ImageTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        window.dispatch_action(
            Box::new(ReplaceRoute {
                route: Route::ImageDetail {
                    id: self.id.clone(),
                    tab,
                },
            }),
            cx,
        );
        cx.notify();
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self.enabled_tabs(cx);
        let cur = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = step_tab(&enabled, cur, forward);
        self.set_tab(TABS[next], window, cx);
    }

    // ── actions ────────────────────────────────────────────────────────────────────────

    fn on_next_tab(&mut self, _: &detail::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }
    fn on_prev_tab(&mut self, _: &detail::PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    fn on_run(&mut self, _: &image::Run, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || self.details.is_gone() {
            return;
        }
        let reference = self
            .rows(cx)
            .first()
            .map(|r| r.reference())
            .unwrap_or_else(|| self.id.clone());
        RunDialog::open(
            self.engine.clone(),
            reference,
            self.store.downgrade(),
            window,
            cx,
        );
    }

    fn on_tag(&mut self, _: &TagImage, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || self.details.is_gone() {
            return;
        }
        let repo = self
            .rows(cx)
            .into_iter()
            .find(|r| !r.dangling)
            .map(|r| r.repo);
        TagDialog::open(
            self.engine.clone(),
            self.id.clone(),
            repo,
            self.store.downgrade(),
            window,
            cx,
        );
    }

    fn on_copy_id(&mut self, _: &list::CopyId, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.id.clone()));
        notify::info(window, cx, s::COPIED);
    }

    fn on_copy_digest(&mut self, _: &CopyDigest, window: &mut Window, cx: &mut Context<Self>) {
        let digest = self
            .details
            .data()
            .and_then(|d| d.summary.repo_digests.first().cloned());
        match digest {
            Some(d) => {
                cx.write_to_clipboard(ClipboardItem::new_string(d));
                notify::info(window, cx, s::COPIED);
            }
            None => notify::info(window, cx, s::NO_DIGEST),
        }
    }

    /// IMG-006 on the detail page: delete the image by id (all tags); in use → Force.
    fn on_delete(&mut self, _: &list::Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || self.details.is_gone() {
            return;
        }
        let labels: Vec<String> = {
            let r: Vec<String> = self.rows(cx).iter().map(|r| r.label()).collect();
            if r.is_empty() {
                vec![self.title(cx)]
            } else {
                r
            }
        };
        let page = cx.entity().downgrade();
        confirm_destructive(
            ConfirmSpec::new(s::confirm_delete_images(1)).items(labels),
            window,
            cx,
            move |_, window, cx| {
                page.update(cx, |p, cx| p.delete(false, window, cx)).ok();
            },
        );
    }

    fn delete(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let hub = AppState::hub(cx);
        let id = self.id.clone();
        let call = hub.call(&self.engine, move |e| async move {
            e.remove_image(&id, force).await
        });
        let title = self.title(cx);
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| match r {
                Ok(_) => {
                    notify::success(window, cx, s::deleted_n(s::IMAGE, 1));
                    this.store
                        .update(cx, |s, cx| s.refetch(Collection::Images, cx));
                    window.dispatch_action(
                        Box::new(crate::actions::Navigate {
                            route: Route::Images,
                        }),
                        cx,
                    );
                }
                Err(e) if crate::pages::resources::ops::is_in_use(&e) && !force => {
                    let mut users: Vec<String> = this
                        .rows(cx)
                        .first()
                        .map(|r| r.used_by.clone())
                        .unwrap_or_default();
                    if users.is_empty() {
                        users.push(e.to_string());
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
                            page.update(cx, |p, cx| p.delete(true, window, cx)).ok();
                        },
                    );
                }
                Err(e) => notify::engine_error(window, cx, s::delete_failed(s::IMAGE, &title), &e),
            })
            .ok();
        }));
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_overview(&self, cx: &App) -> AnyElement {
        let Some(d) = self.details.data() else {
            return self.render_state(cx);
        };
        let sm = &d.summary;
        let c = &d.config;
        let in_use = self.rows(cx).first().map(|r| r.in_use).unwrap_or(0);
        v_flex()
            .gap_4()
            .child(kv_section(
                None,
                vec![
                    (s::ID, mono_value(sm.id.clone(), cx)),
                    (s::TAGS, lines_value(&sm.repo_tags, cx)),
                    (s::DIGESTS, lines_value(&sm.repo_digests, cx)),
                    (
                        s::CREATED,
                        text_value(Some(format_timestamp(sm.created)), cx),
                    ),
                    (s::SIZE, text_value(Some(format_size(sm.size)), cx)),
                    (
                        s::STATUS,
                        if in_use > 0 {
                            crate::pages::resources::detail::chip_value(
                                crate::ui::status_chip::tone_tag(
                                    crate::ui::status_chip::Tone::Success,
                                    s::in_use_by(in_use),
                                ),
                            )
                        } else {
                            text_value(Some(s::NOT_USED), cx)
                        },
                    ),
                    (
                        s::ARCHITECTURE,
                        text_value(Some(d.architecture.clone()), cx),
                    ),
                    (s::OS, text_value(Some(d.os.clone()), cx)),
                    (s::VARIANT, text_value(d.variant.clone(), cx)),
                    (s::AUTHOR, text_value(d.author.clone(), cx)),
                ],
                cx,
            ))
            .child(kv_section(
                Some("Config"),
                vec![
                    (s::ENTRYPOINT, text_or_dash_mono(c.entrypoint.join(" "), cx)),
                    (s::CMD, text_or_dash_mono(c.cmd.join(" "), cx)),
                    (s::ENV, lines_value(&c.env, cx)),
                    (s::EXPOSED_PORTS, lines_value(&c.exposed_ports, cx)),
                    (s::WORKDIR, text_value(c.working_dir.clone(), cx)),
                    (s::USER, text_value(c.user.clone(), cx)),
                    (s::VOLUMES_LABEL, lines_value(&c.volumes, cx)),
                    (s::LABELS, map_value(&c.labels, cx)),
                ],
                cx,
            ))
            .into_any_element()
    }

    fn render_state(&self, cx: &App) -> AnyElement {
        match &self.details {
            Loaded::Loading => crate::ui::skeleton_rows(6, 2),
            Loaded::Failed(e) if !self.details.is_gone() => crate::ui::error_panel(
                "image-detail-error",
                s::DETAIL_LOAD_FAILED,
                e,
                Box::new(crate::actions::Refresh),
                cx,
            ),
            _ => div().into_any_element(),
        }
    }

    fn render_layers(&self, cx: &App) -> AnyElement {
        if !self.caps(cx).contains(Capabilities::IMAGE_HISTORY) {
            return div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(s::NO_HISTORY)
                .into_any_element();
        }
        match &self.history {
            Loaded::Loading => crate::ui::skeleton_rows(6, 4),
            Loaded::Failed(e) => crate::ui::error_panel(
                "image-history-error",
                s::DETAIL_LOAD_FAILED,
                e,
                Box::new(crate::actions::Refresh),
                cx,
            ),
            Loaded::Ready(layers) => KitTable::new()
                .small()
                .child(
                    TableHeader::new().child(
                        TableRow::new()
                            .child(TableHead::new().child(s::COL_CREATED_BY))
                            .child(TableHead::new().child(s::COL_SIZE))
                            .child(TableHead::new().child(s::COL_CREATED))
                            .child(TableHead::new().child(s::COL_COMMENT)),
                    ),
                )
                .child(TableBody::new().children(layers.iter().map(|l| {
                    TableRow::new()
                        .child(
                            TableCell::new().child(
                                div()
                                    .text_xs()
                                    .w_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(l.created_by.clone()),
                            ),
                        )
                        .child(TableCell::new().child(format_size(l.size)))
                        .child(TableCell::new().child(crate::ui::relative_time(l.created, cx)))
                        .child(TableCell::new().child(l.comment.clone()))
                })))
                .into_any_element(),
        }
    }

    fn render_used_by(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let users: Vec<(Route, Vec<AnyElement>)> = {
            let s = self.store.read(cx);
            s.containers
                .data()
                .map(|cs| {
                    cs.iter()
                        .filter(|c| c.image_id == self.id)
                        .enumerate()
                        .map(|(ix, c)| {
                            let route = Route::ContainerDetail {
                                id: c.id.clone(),
                                tab: ContainerTab::Overview,
                            };
                            (
                                route.clone(),
                                vec![
                                    route_link(("used-by", ix), c.name.clone(), route),
                                    crate::ui::status_chip::container_chip(c.state, c.exit_code)
                                        .into_any_element(),
                                    div().text_xs().child(c.image.clone()).into_any_element(),
                                ],
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        self.used_by.render(
            "image-used-by",
            &[s::COL_NAME, s::COL_STATUS, s::COL_IMAGE],
            users,
            s::NOT_USED,
            window,
            cx,
            |p: &mut ImageDetailPage| &mut p.used_by,
        )
    }
}

impl Focusable for ImageDetailPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.header_focus.clone()
    }
}

impl PageView for ImageDetailPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.tabs_focus.clone()
    }
}

impl RoutedPage for ImageDetailPage {
    fn accept_route(&mut self, route: &Route, _: &mut Window, _: &mut Context<Self>) -> bool {
        match route {
            Route::ImageDetail { id, tab } if *id == self.id => {
                self.tab = *tab;
                true
            }
            _ => false,
        }
    }
}

impl Render for ImageDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title(cx);
        let gone = self.details.is_gone();
        let ro = self.read_only(cx) || gone;
        let enabled = self.enabled_tabs(cx);
        let tabs: Vec<TabSpec> = TABS
            .iter()
            .zip(&enabled)
            .map(|(t, e)| tab_spec(*t).disabled(!*e))
            .collect();
        let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let this = cx.entity().downgrade();
        let in_use = self.rows(cx).first().map(|r| r.in_use).unwrap_or(0);
        let body = match self.tab {
            ImageTab::Overview => self.render_overview(cx),
            ImageTab::Layers => self.render_layers(cx),
            ImageTab::UsedBy => self.render_used_by(window, cx),
            ImageTab::Inspect => self.inspect.clone().into_any_element(),
        };
        v_flex()
            .id("image-detail")
            .size_full()
            .p_4()
            .gap_3()
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_run))
            .on_action(cx.listener(Self::on_tag))
            .on_action(cx.listener(Self::on_copy_id))
            .on_action(cx.listener(Self::on_copy_digest))
            .on_action(cx.listener(Self::on_delete))
            .child(breadcrumb(vec![
                Crumb::link(s::PAGE_IMAGES, Route::Images),
                Crumb::here(title.clone()),
            ]))
            .child(header(
                &self.header_focus,
                title,
                if in_use > 0 {
                    vec![
                        crate::ui::status_chip::tone_tag(
                            crate::ui::status_chip::Tone::Success,
                            s::IN_USE,
                        )
                        .into_any_element(),
                    ]
                } else {
                    vec![]
                },
                vec![
                    header_action(
                        "img-run",
                        s::RUN,
                        Some(IconName::Play),
                        Box::new(image::Run),
                        ro,
                    ),
                    header_action("img-tag", s::TAG_IMAGE, None, Box::new(TagImage), ro),
                    header_action(
                        "img-copy-id",
                        s::COPY_ID,
                        Some(IconName::Copy),
                        Box::new(list::CopyId),
                        false,
                    ),
                    header_action(
                        "img-copy-digest",
                        s::COPY_DIGEST,
                        None,
                        Box::new(CopyDigest),
                        gone,
                    ),
                    header_action("img-delete", s::DELETE, None, Box::new(list::Delete), ro),
                ],
                cx,
            ))
            .when(gone, |this| {
                this.child(gone_banner(s::IMAGE, Route::Images, cx))
            })
            .child(tab_bar(
                &self.tabs_focus,
                tabs,
                selected,
                move |ix, window, cx| {
                    this.update(cx, |p, cx| {
                        if p.enabled_tabs(cx).get(ix).copied().unwrap_or(false) {
                            p.set_tab(TABS[ix], window, cx)
                        }
                    })
                    .ok();
                },
                cx,
            ))
            .child(
                div()
                    .id("image-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

/// Short helper for tests: the tab's label.
pub fn label_of(t: ImageTab) -> &'static str {
    tab_spec(t).label
}

#[allow(dead_code)]
fn _unused(_: SharedString, _: bool) -> AnyElement {
    let _ = h_flex();
    bool_value(false)
}

/// Monospace value, or "—" when empty.
fn text_or_dash_mono(v: String, cx: &App) -> AnyElement {
    if v.is_empty() {
        crate::pages::resources::chrome::dash(cx)
    } else {
        mono_value(v, cx)
    }
}
