//! The Containers `ListDelegate`: columns (CON-002), cells, group rows (CON-011/012), row
//! menus (CON-020), and the empty state (CON-032).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use dk_core::grouping::AggregateState;
use dk_core::{Capabilities, ContainerState, ContainerSummary};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, SharedString, Window, div, px};

use super::model::{GroupInfo, sort_keys};
use crate::actions::{OnRow, RowCommand};
use crate::assets::Lucide;
use crate::pages::resources::chrome::{name_cell, row_menu_button};
use crate::strings as s;
use crate::ui::list_table::{ColumnSpec, ListDelegate, ListRow, RowKind};
use crate::ui::status_chip::{Tone, container_chip, health_chip, tone_tag};
use crate::ui::widgets::{port_link, relative_time};

pub mod col {
    pub const SELECT: &str = "select";
    pub const NAME: &str = super::sort_keys::NAME;
    pub const IMAGE: &str = super::sort_keys::IMAGE;
    pub const STATUS: &str = super::sort_keys::STATUS;
    pub const CPU: &str = super::sort_keys::CPU;
    pub const MEM: &str = "mem";
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
        }
    }

    fn row_action_buttons(&self, row_ix: usize, c: &ContainerSummary) -> AnyElement {
        if self.pending.contains(&c.id) {
            return Spinner::new().small().into_any_element();
        }
        let key: SharedString = c.id.clone().into();
        let running = c.state.is_running() || c.state == ContainerState::Paused;
        let disabled = self.read_only;
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
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(
                        Box::new(OnRow {
                            row: key.clone(),
                            action: cmd,
                        }),
                        cx,
                    )
                })
        };
        h_flex()
            .gap_0p5()
            .map(|this| {
                if running {
                    this.child(btn(
                        "stop",
                        Lucide::Square.into(),
                        s::ACTION_STOP,
                        RowCommand::Stop,
                        key.clone(),
                    ))
                    .child(btn(
                        "restart",
                        IconName::RotateCw.into(),
                        s::ACTION_RESTART,
                        RowCommand::Restart,
                        key.clone(),
                    ))
                } else {
                    this.child(btn(
                        "start",
                        IconName::Play.into(),
                        s::ACTION_START,
                        RowCommand::Start,
                        key.clone(),
                    ))
                }
            })
            .child(btn(
                "delete",
                Lucide::Trash.into(),
                s::ACTION_DELETE,
                RowCommand::Delete,
                key.clone(),
            ))
            .child(row_menu_button(
                ("more", row_ix),
                Box::new(OnRow {
                    row: key,
                    action: RowCommand::ContextMenu,
                }),
            ))
            .into_any_element()
    }

    fn group_action_buttons(
        &self,
        row_ix: usize,
        key: &SharedString,
        info: &GroupInfo,
    ) -> AnyElement {
        if info.members.iter().any(|m| self.pending.contains(&m.id)) {
            return Spinner::new().small().into_any_element();
        }
        let disabled = self.read_only;
        let any_running = info.group.aggregate.running > 0;
        let all_running = info.group.aggregate.state == AggregateState::Running;
        let btn = |id: &'static str,
                   icon: gpui_kit::component::Icon,
                   tip: &'static str,
                   cmd: RowCommand| {
            let key = key.clone();
            Button::new((id, row_ix))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .disabled(disabled)
                .tab_stop(false)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(
                        Box::new(OnRow {
                            row: key.clone(),
                            action: cmd,
                        }),
                        cx,
                    )
                })
        };
        h_flex()
            .gap_0p5()
            .when(!all_running, |this| {
                this.child(btn(
                    "g-start",
                    IconName::Play.into(),
                    s::ACTION_START_ALL,
                    RowCommand::Start,
                ))
            })
            .when(any_running, |this| {
                this.child(btn(
                    "g-stop",
                    Lucide::Square.into(),
                    s::ACTION_STOP_ALL,
                    RowCommand::Stop,
                ))
            })
            .child(btn(
                "g-restart",
                IconName::RotateCw.into(),
                s::ACTION_RESTART_ALL,
                RowCommand::Restart,
            ))
            .child(btn(
                "g-delete",
                Lucide::Trash.into(),
                s::ACTION_DELETE_ALL,
                RowCommand::Delete,
            ))
            .into_any_element()
    }

    fn checkbox(&self, row_ix: usize, key: &SharedString, checked: bool) -> AnyElement {
        let key = key.clone();
        Checkbox::new(("row-check", row_ix))
            .checked(checked)
            .tab_stop(false)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(
                    Box::new(OnRow {
                        row: key.clone(),
                        action: RowCommand::ToggleSelected,
                    }),
                    cx,
                )
            })
            .into_any_element()
    }
}

/// Column specs (CON-002). CPU/Mem only when enabled and supported (STA-006).
pub fn columns(show_stats: bool) -> Vec<ColumnSpec> {
    let mut v = vec![
        ColumnSpec::new(col::SELECT, "", 36.).fixed(),
        ColumnSpec::new(col::NAME, s::COL_NAME, 260.).sortable(),
        ColumnSpec::new(col::IMAGE, s::COL_IMAGE, 180.).sortable(),
        ColumnSpec::new(col::STATUS, s::COL_STATUS, 200.).sortable(),
    ];
    if show_stats {
        v.push(
            ColumnSpec::new(col::CPU, s::COL_CPU, 70.)
                .sortable()
                .right(),
        );
        v.push(ColumnSpec::new(col::MEM, s::COL_MEMORY, 90.).right());
    }
    v.push(ColumnSpec::new(col::PORTS, s::COL_PORTS, 150.));
    v.push(ColumnSpec::new(col::CREATED, s::COL_CREATED, 120.).sortable());
    v.push(ColumnSpec::new(col::ACTIONS, s::COL_ACTIONS, 150.).pin_right());
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
        selected: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match &row.kind {
            RowKind::Group {
                group, expanded, ..
            } => match column.key {
                col::SELECT => self.checkbox(row_ix, &row.key, selected),
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
                            div()
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
                col::STATUS => {
                    let a = group.group.aggregate;
                    let tone = match a.state {
                        AggregateState::Running => Tone::Success,
                        AggregateState::Partial => Tone::Warning,
                        AggregateState::Exited => Tone::Neutral,
                    };
                    tone_tag(tone, s::group_running(a.running, a.total)).into_any_element()
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
                col::ACTIONS => self.group_action_buttons(row_ix, &row.key, group),
                _ => div().into_any_element(),
            },
            RowKind::Item(c) => match column.key {
                col::SELECT => self.checkbox(row_ix, &row.key, selected),
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
                col::IMAGE => div()
                    .text_xs()
                    .truncate()
                    .child(c.image.clone())
                    .into_any_element(),
                col::STATUS => h_flex()
                    .gap_1()
                    .items_center()
                    .child(container_chip(c.state, c.exit_code))
                    .when_some(c.health, |this, h| this.child(health_chip(h)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(c.status_text.clone()),
                    )
                    .into_any_element(),
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
                col::PORTS => h_flex()
                    .gap_1()
                    .overflow_hidden()
                    .children(
                        c.ports
                            .iter()
                            .enumerate()
                            .take(3)
                            .map(|(i, p)| port_link(("port", row_ix * 16 + i), p, cx)),
                    )
                    .into_any_element(),
                col::CREATED => div()
                    .text_xs()
                    .text_color(muted)
                    .child(relative_time(c.created, cx))
                    .into_any_element(),
                col::ACTIONS => self.row_action_buttons(row_ix, c),
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
        _window: &Window,
        _cx: &App,
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

    fn render_empty(&self, _window: &mut Window, cx: &mut App) -> AnyElement {
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
