//! The Containers `ListDelegate`: columns (CON-002), cells, group rows (CON-011/012), row
//! menus (CON-020), and the empty state (CON-032).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use dk_core::{Capabilities, ContainerState, ContainerSummary};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Anchor, AnyElement, App, IntoElement, SharedString, div, px};

use super::model::{GroupInfo, sort_keys};
use crate::actions::{OnRow, OpenUrl, RowCommand};
use crate::assets::Lucide;
use crate::pages::resources::chrome::{name_cell, row_menu_button};
use crate::strings as s;
use crate::ui::action_icons::{StateIcon, state_button};
use crate::ui::dispatch::{self, DispatchAnchor};
use crate::ui::list_table::{ColumnSpec, ListDelegate, ListRow, RowKind};
use crate::ui::status_chip::{container_chip, health_chip, status_without_health};
use crate::ui::widgets::{OverflowSet, port_link, port_url, relative_time};

pub mod col {
    pub const SELECT: &str = crate::ui::list_table::SELECT_COLUMN;
    pub const NAME: &str = super::sort_keys::NAME;
    pub const IMAGE: &str = super::sort_keys::IMAGE;
    pub const STATUS: &str = super::sort_keys::STATUS;
    pub const CPU: &str = super::sort_keys::CPU;
    pub const MEM: &str = super::sort_keys::MEM;
    pub const PORTS: &str = "ports";
    pub const CREATED: &str = super::sort_keys::CREATED;
    pub const ACTIONS: &str = "actions";
}

/// Latest list stats per container (CON-002 optional columns, STA-006).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowStats {
    pub cpu: f64,
    pub mem: u64,
}

pub struct ContainersDelegate {
    pub caps: Capabilities,
    pub show_stats: bool,
    pub read_only: bool,
    /// Rows with an action in flight (spinner, CON-031).
    pub pending: HashSet<String>,
    pub stats: HashMap<String, RowStats>,
    pub visible: Range<usize>,
    pub has_images: bool,
    pub filtered_out: bool,
    /// Image and port cells that don't fit their column (tooltip / ports dropdown).
    pub overflow: OverflowSet,
}

impl ContainersDelegate {
    pub fn new() -> Self {
        Self {
            caps: Capabilities::empty(),
            show_stats: false,
            read_only: false,
            pending: HashSet::new(),
            stats: HashMap::new(),
            visible: 0..0,
            has_images: false,
            filtered_out: false,
            overflow: OverflowSet::default(),
        }
    }

    /// The image name, truncated; the full value shows on hover when it doesn't fit.
    fn image_cell(&self, row_ix: usize, c: &ContainerSummary) -> AnyElement {
        let key: SharedString = format!("image:{}", c.id).into();
        let image: SharedString = c.image.clone().into();
        let truncated = self.overflow.contains(&key);
        div()
            .id(("image", row_ix))
            .relative()
            .flex_1()
            .min_w_0()
            .text_xs()
            .truncate()
            .child(image.clone())
            .child(self.overflow.probe_text(key, image.clone()))
            .when(truncated, |this| {
                this.tooltip(move |window, cx| Tooltip::new(image.clone()).build(window, cx))
            })
            .into_any_element()
    }

    /// Port links on one line. When they don't all fit, a small chevron at the end opens a
    /// menu listing every port, one per line (published ones open in the browser).
    fn ports_cell(&self, row_ix: usize, c: &ContainerSummary, cx: &App) -> AnyElement {
        let key: SharedString = format!("ports:{}", c.id).into();
        let clipped = self.overflow.contains(&key);
        let links = h_flex().gap_1().children(
            c.ports
                .iter()
                .enumerate()
                .map(|(i, p)| port_link(("port", row_ix * 64 + i), p, cx)),
        );
        let ports = c.ports.clone();
        h_flex()
            .size_full()
            .gap_0p5()
            .items_center()
            .child(self.overflow.clip(key, links))
            .when(clipped, |this| {
                this.child(
                    Button::new(("ports-more", row_ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronDown)
                        .tooltip(s::ALL_PORTS)
                        .tab_stop(false)
                        // The menu opens on mouse down; don't let the click open the row.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                            for p in &ports {
                                let label = dk_core::format::format_port(p);
                                let item = match port_url(p) {
                                    Some(url) => {
                                        let url: SharedString = url.into();
                                        PopupMenuItem::new(format!("{label} ↗")).on_click(
                                            move |_, window, cx| {
                                                window.dispatch_action(
                                                    Box::new(OpenUrl { url: url.clone() }),
                                                    cx,
                                                )
                                            },
                                        )
                                    }
                                    None => PopupMenuItem::new(label).disabled(true),
                                };
                                menu = menu.item(item);
                            }
                            menu.scrollable(true).max_h(px(320.))
                        }),
                )
            })
            .into_any_element()
    }

    fn row_action_buttons(&self, row_ix: usize, c: &ContainerSummary, cx: &App) -> AnyElement {
        if self.pending.contains(&c.id) {
            return Spinner::new().small().into_any_element();
        }
        let key: SharedString = c.id.clone().into();
        let running = c.state.is_running() || c.state == ContainerState::Paused;
        let disabled = self.read_only;
        let anchor = DispatchAnchor::new(cx);
        let origin = anchor.handle().clone();
        let btn = |id: &'static str,
                   icon: gpui_kit::component::Icon,
                   tip: &'static str,
                   cmd: RowCommand,
                   key: SharedString| {
            Button::new((id, row_ix))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .disabled(disabled)
                .tab_stop(false)
                .on_click(dispatch::on_click_stop(
                    &origin,
                    Box::new(OnRow {
                        row: key,
                        action: cmd,
                    }),
                ))
        };
        // Fixed slots so the icons line up across rows: start/stop, restart, ⋮. Delete
        // lives in the ⋮ menu and the selection actions.
        h_flex()
            .relative()
            .gap_0p5()
            .child(anchor.element())
            .child({
                let (id, icon, tip, cmd) = if running {
                    ("stop", StateIcon::Stop, s::ACTION_STOP, RowCommand::Stop)
                } else {
                    (
                        "start",
                        StateIcon::Start,
                        s::ACTION_START,
                        RowCommand::Start,
                    )
                };
                state_button(
                    (id, row_ix),
                    format!("row-{id}-{row_ix}"),
                    icon,
                    tip,
                    disabled,
                    (
                        &origin,
                        Box::new(OnRow {
                            row: key.clone(),
                            action: cmd,
                        }),
                    ),
                    cx,
                )
            })
            .child(
                btn(
                    "restart",
                    IconName::RotateCw.into(),
                    s::ACTION_RESTART,
                    RowCommand::Restart,
                    key.clone(),
                )
                .when(!running, |b| b.disabled(true)),
            )
            .child(row_menu_button(
                ("more", row_ix),
                Box::new(OnRow {
                    row: key,
                    action: RowCommand::ContextMenu,
                }),
                cx,
            ))
            .into_any_element()
    }

    fn group_action_buttons(
        &self,
        row_ix: usize,
        key: &SharedString,
        info: &GroupInfo,
        cx: &App,
    ) -> AnyElement {
        if info.members.iter().any(|m| self.pending.contains(&m.id)) {
            return Spinner::new().small().into_any_element();
        }
        let disabled = self.read_only;
        let any_running = info.group.aggregate.running > 0;
        let anchor = DispatchAnchor::new(cx);
        let origin = anchor.handle().clone();
        let btn = |id: &'static str,
                   icon: gpui_kit::component::Icon,
                   tip: &'static str,
                   cmd: RowCommand| {
            Button::new((id, row_ix))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .disabled(disabled)
                .tab_stop(false)
                .on_click(dispatch::on_click_stop(
                    &origin,
                    Box::new(OnRow {
                        row: key.clone(),
                        action: cmd,
                    }),
                ))
        };
        // Same slots as member rows: start/stop all, restart all, ⋮ (start all for a
        // partly running group and delete all are in the ⋮ menu).
        h_flex()
            .relative()
            .gap_0p5()
            .child(anchor.element())
            .child({
                let (id, icon, tip, cmd) = if any_running {
                    (
                        "g-stop",
                        StateIcon::Stop,
                        s::ACTION_STOP_ALL,
                        RowCommand::Stop,
                    )
                } else {
                    (
                        "g-start",
                        StateIcon::Start,
                        s::ACTION_START_ALL,
                        RowCommand::Start,
                    )
                };
                state_button(
                    (id, row_ix),
                    format!("row-{id}-{row_ix}"),
                    icon,
                    tip,
                    disabled,
                    (
                        &origin,
                        Box::new(OnRow {
                            row: key.clone(),
                            action: cmd,
                        }),
                    ),
                    cx,
                )
            })
            .child(btn(
                "g-restart",
                IconName::RotateCw.into(),
                s::ACTION_RESTART_ALL,
                RowCommand::Restart,
            ))
            .child(row_menu_button(
                ("g-more", row_ix),
                Box::new(OnRow {
                    row: key.clone(),
                    action: RowCommand::ContextMenu,
                }),
                cx,
            ))
            .into_any_element()
    }
}

/// Column specs (CON-002). CPU/Mem only when enabled and supported (STA-006). *Created* is
/// the last data column, as on every list.
pub fn columns(show_stats: bool) -> Vec<ColumnSpec> {
    let mut v = vec![
        ColumnSpec::new(col::SELECT, "", 42.).fixed().pin_left(),
        ColumnSpec::new(col::NAME, s::COL_NAME, 260.)
            .sortable()
            .pin_left(),
        ColumnSpec::new(col::IMAGE, s::COL_IMAGE, 180.).sortable(),
        ColumnSpec::new(col::STATUS, s::COL_STATUS, 200.).sortable(),
        ColumnSpec::new(col::PORTS, s::COL_PORTS, 150.),
    ];
    if show_stats {
        v.push(
            ColumnSpec::new(col::CPU, s::COL_CPU, 70.)
                .sortable()
                .right(),
        );
        v.push(
            ColumnSpec::new(col::MEM, s::COL_MEMORY, 90.)
                .sortable()
                .right(),
        );
    }
    v.push(ColumnSpec::new(col::CREATED, s::COL_CREATED, 120.).sortable());
    v.push(ColumnSpec::new(col::ACTIONS, s::COL_ACTIONS, 110.).pin_right());
    v
}

impl ListDelegate for ContainersDelegate {
    type Group = GroupInfo;
    type Item = ContainerSummary;

    fn columns(&self) -> Vec<ColumnSpec> {
        columns(self.show_stats)
    }

    fn render_cell(
        &self,
        row: &ListRow<GroupInfo, ContainerSummary>,
        row_ix: usize,
        column: &ColumnSpec,
        cx: &mut App,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match &row.kind {
            RowKind::Group {
                group, expanded, ..
            } => match column.key {
                col::NAME => {
                    let tooltip = group.group.working_dir.clone();
                    h_flex()
                        .id(("group-name", row_ix))
                        .gap_1p5()
                        .items_center()
                        // The whole row toggles the group (CON-011); the chevron is a cue.
                        .child(
                            Icon::new(if *expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .small()
                            .text_color(muted),
                        )
                        .child(
                            Icon::new(Lucide::Layers)
                                .small()
                                .text_color(cx.theme().foreground.opacity(0.7)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(group.group.label.clone()),
                        )
                        .when(group.group.is_compose, |this| {
                            this.child(Tag::secondary().small().child(s::COMPOSE_TAG))
                        })
                        .when_some(tooltip, |this, dir| {
                            this.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(dir.clone())
                                    .build(window, cx)
                            })
                        })
                        .into_any_element()
                }
                col::CPU if self.show_stats => {
                    let sum: f64 = group
                        .members
                        .iter()
                        .filter_map(|m| self.stats.get(&m.id).map(|s| s.cpu))
                        .sum();
                    div()
                        .text_xs()
                        .child(dk_core::format::format_percent(sum))
                        .into_any_element()
                }
                col::MEM if self.show_stats => {
                    let sum: u64 = group
                        .members
                        .iter()
                        .filter_map(|m| self.stats.get(&m.id).map(|s| s.mem))
                        .sum();
                    div()
                        .text_xs()
                        .child(dk_core::format::format_size(sum))
                        .into_any_element()
                }
                col::ACTIONS => self.group_action_buttons(row_ix, &row.key, group, cx),
                _ => div().into_any_element(),
            },
            RowKind::Item(c) => match column.key {
                col::NAME => {
                    let oneoff = c.compose.as_ref().is_some_and(|ci| ci.oneoff);
                    h_flex()
                        .gap_1()
                        .items_center()
                        .when(row.depth > 0, |this| this.pl(px(22.)))
                        .child(Icon::new(Lucide::Container).small().text_color(
                            if c.state.is_running() {
                                cx.theme().success
                            } else {
                                muted
                            },
                        ))
                        .child(name_cell(c.name.clone()))
                        .when(oneoff, |this| {
                            this.child(Tag::secondary().outline().small().child(s::ONEOFF_TAG))
                        })
                        .into_any_element()
                }
                col::IMAGE => self.image_cell(row_ix, c),
                col::STATUS => {
                    // The health chip already says "healthy"; don't repeat it in the text.
                    let text = if c.health.is_some() {
                        status_without_health(&c.status_text).to_owned()
                    } else {
                        c.status_text.clone()
                    };
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(container_chip(c.state, c.exit_code))
                        .when_some(c.health, |this, h| this.child(health_chip(h)))
                        .child(div().text_xs().text_color(muted).truncate().child(text))
                        .into_any_element()
                }
                col::CPU => div()
                    .text_xs()
                    .child(
                        self.stats
                            .get(&c.id)
                            .map(|s| dk_core::format::format_percent(s.cpu))
                            .unwrap_or_else(|| "—".into()),
                    )
                    .into_any_element(),
                col::MEM => div()
                    .text_xs()
                    .child(
                        self.stats
                            .get(&c.id)
                            .map(|s| dk_core::format::format_size(s.mem))
                            .unwrap_or_else(|| "—".into()),
                    )
                    .into_any_element(),
                col::PORTS => self.ports_cell(row_ix, c, cx),
                col::CREATED => div()
                    .text_xs()
                    .text_color(muted)
                    .child(relative_time(c.created, cx))
                    .into_any_element(),
                col::ACTIONS => self.row_action_buttons(row_ix, c, cx),
                _ => div().into_any_element(),
            },
        }
    }

    fn row_text(&self, row: &ListRow<GroupInfo, ContainerSummary>) -> String {
        match &row.kind {
            RowKind::Group { group, .. } => group.group.label.clone(),
            RowKind::Item(c) => c.name.clone(),
        }
    }

    fn context_menu(
        &self,
        row: &ListRow<GroupInfo, ContainerSummary>,
        menu: PopupMenu,
    ) -> PopupMenu {
        let key = row.key.clone();
        let on = |cmd: RowCommand| -> Box<dyn gpui_kit::Action> {
            Box::new(OnRow {
                row: key.clone(),
                action: cmd,
            })
        };
        let ro = self.read_only;
        match &row.kind {
            RowKind::Group { group, .. } => {
                let a = group.group.aggregate;
                menu.menu_with_disabled(
                    s::ACTION_START_ALL,
                    on(RowCommand::Start),
                    ro || a.running == a.total,
                )
                .menu_with_disabled(
                    s::ACTION_STOP_ALL,
                    on(RowCommand::Stop),
                    ro || a.running == 0,
                )
                .menu_with_disabled(s::ACTION_RESTART_ALL, on(RowCommand::Restart), ro)
                .separator()
                .menu_with_disabled(
                    s::ACTION_DELETE_ALL,
                    on(RowCommand::Delete),
                    ro,
                )
            }
            RowKind::Item(c) => {
                let running = c.state.is_running();
                let paused = c.state == ContainerState::Paused;
                let pause_cap = self.caps.contains(Capabilities::PAUSE);
                let tty = self.caps.contains(Capabilities::EXEC_TTY);
                let has_port = c.ports.iter().any(|p| p.public.is_some());
                use crate::actions::{container as k, list as l};
                let mut m = if running || paused {
                    hinted(
                        menu,
                        s::ACTION_STOP,
                        on(RowCommand::Stop),
                        &k::StartStop,
                        ro,
                    )
                } else {
                    hinted(
                        menu,
                        s::ACTION_START,
                        on(RowCommand::Start),
                        &k::StartStop,
                        ro,
                    )
                };
                m = hinted(
                    m,
                    s::ACTION_RESTART,
                    on(RowCommand::Restart),
                    &k::Restart,
                    ro,
                );
                if pause_cap {
                    m = if paused {
                        hinted(
                            m,
                            s::ACTION_UNPAUSE,
                            on(RowCommand::Unpause),
                            &k::PauseToggle,
                            ro,
                        )
                    } else {
                        hinted(
                            m,
                            s::ACTION_PAUSE,
                            on(RowCommand::Pause),
                            &k::PauseToggle,
                            ro || !running,
                        )
                    };
                }
                m = m
                    .menu_with_disabled(
                        s::ACTION_KILL,
                        on(RowCommand::Kill),
                        ro || !(running || paused),
                    )
                    .separator();
                m = hinted(m, s::ACTION_LOGS, on(RowCommand::Logs), &k::Logs, false);
                if tty {
                    m = hinted(
                        m,
                        s::ACTION_TERMINAL,
                        on(RowCommand::Terminal),
                        &k::Terminal,
                        !running,
                    );
                }
                m = hinted(
                    m,
                    s::ACTION_INSPECT,
                    on(RowCommand::Inspect),
                    &k::Inspect,
                    false,
                );
                m = hinted(
                    m,
                    s::ACTION_OPEN_PORT,
                    on(RowCommand::OpenPort),
                    &k::OpenPort,
                    !has_port,
                );
                m = hinted(m, s::COPY_ID, on(RowCommand::CopyId), &l::CopyId, false);
                hinted(
                    m.separator(),
                    s::ACTION_DELETE,
                    on(RowCommand::Delete),
                    &l::Delete,
                    ro,
                )
            }
        }
    }

    fn render_empty(&self, cx: &mut App) -> AnyElement {
        if self.filtered_out {
            return crate::ui::empty_state(
                IconName::Search,
                s::NO_MATCHING_CONTAINERS,
                "",
                None,
                cx,
            );
        }
        let cta = self.has_images.then(|| {
            (
                SharedString::from(s::RUN_AN_IMAGE),
                Box::new(crate::actions::GoImages) as Box<dyn gpui_kit::Action>,
            )
        });
        crate::ui::empty_state(
            IconName::Inbox,
            s::NO_CONTAINERS,
            s::NO_CONTAINERS_BODY,
            cta,
            cx,
        )
    }

    fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }

    fn visible_rows_changed(&mut self, range: Range<usize>) {
        self.visible = range;
    }
}

/// A menu item that dispatches `action` (an `OnRow`) and shows the binding of the
/// equivalent list command `hint` (KBD-036: shortcuts shown in the row menu).
fn hinted(
    menu: PopupMenu,
    label: &'static str,
    action: Box<dyn gpui_kit::Action>,
    hint: &dyn gpui_kit::Action,
    disabled: bool,
) -> PopupMenu {
    let keys = crate::keymap::hint_for(hint.name(), crate::keymap::ctx::LIST_KEYS);
    menu.menu_element_with_disabled(action, disabled, move |_, cx| {
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
}
