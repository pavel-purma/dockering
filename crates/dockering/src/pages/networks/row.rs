//! The Networks `ListDelegate` (NET-002): columns, cells, row menu, empty state. Delete is
//! disabled for the built-in `bridge`/`host`/`none` and without `NETWORK_MGMT`.

use std::collections::HashSet;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, SharedString, Window, div};

use super::model::{NetworkRow, sort_keys};
use crate::actions::{OnRow, RowCommand, list};
use crate::assets::Lucide;
use crate::keymap::ctx;
use crate::pages::resources::chrome::{dash, hinted, mono_cell, name_cell, text_cell};
use crate::strings as s;
use crate::ui::list_table::{ColumnSpec, ListDelegate, ListRow, RowKind};
use crate::ui::widgets::relative_time;

pub mod col {
    pub const NAME: &str = super::sort_keys::NAME;
    pub const DRIVER: &str = super::sort_keys::DRIVER;
    pub const SCOPE: &str = "scope";
    pub const SUBNETS: &str = "subnets";
    pub const GATEWAY: &str = "gateway";
    pub const CONTAINERS: &str = super::sort_keys::CONTAINERS;
    pub const COMPOSE: &str = "compose";
    pub const CREATED: &str = super::sort_keys::CREATED;
    pub const ACTIONS: &str = "actions";
}

pub fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new(col::NAME, s::COL_NAME, 220.).sortable(),
        ColumnSpec::new(col::DRIVER, s::COL_DRIVER, 80.).sortable(),
        ColumnSpec::new(col::SCOPE, s::COL_SCOPE, 70.),
        ColumnSpec::new(col::SUBNETS, s::COL_SUBNETS, 140.),
        ColumnSpec::new(col::GATEWAY, s::COL_GATEWAY, 110.),
        ColumnSpec::new(col::CONTAINERS, s::COL_CONTAINERS, 90.)
            .sortable()
            .right(),
        ColumnSpec::new(col::COMPOSE, s::COL_COMPOSE, 130.),
        ColumnSpec::new(col::CREATED, s::COL_CREATED, 110.).sortable(),
        ColumnSpec::new(col::ACTIONS, s::COL_ACTIONS, 80.).pin_right(),
    ]
}

#[derive(Default)]
pub struct NetworksDelegate {
    pub read_only: bool,
    /// `NETWORK_MGMT` capability (NET-002 delete).
    pub can_manage: bool,
    pub pending: HashSet<String>,
    pub filtered_out: bool,
}

fn on_row(key: &SharedString, cmd: RowCommand) -> Box<dyn gpui_kit::Action> {
    Box::new(OnRow {
        row: key.clone(),
        action: cmd,
    })
}

impl NetworksDelegate {
    pub fn can_delete(&self, r: &NetworkRow) -> bool {
        !self.read_only && self.can_manage && !r.builtin
    }
}

impl ListDelegate for NetworksDelegate {
    type Group = ();
    type Item = NetworkRow;

    fn columns(&self) -> Vec<ColumnSpec> {
        columns()
    }

    fn render_cell(
        &self,
        row: &ListRow<(), NetworkRow>,
        row_ix: usize,
        column: &ColumnSpec,
        _selected: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let RowKind::Item(r) = &row.kind else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        match column.key {
            col::NAME => h_flex()
                .gap_1()
                .items_center()
                .overflow_hidden()
                .child(
                    Icon::new(IconName::Network)
                        .small()
                        .text_color(if r.containers > 0 {
                            cx.theme().success
                        } else {
                            muted
                        }),
                )
                .child(name_cell(r.name.clone()))
                .when(r.builtin, |this| {
                    this.child(Tag::secondary().outline().small().child("built-in"))
                })
                .into_any_element(),
            col::DRIVER => text_cell(r.driver.clone()),
            col::SCOPE => text_cell(r.scope.clone()),
            col::SUBNETS => {
                if r.subnets.is_empty() {
                    dash(cx)
                } else {
                    mono_cell(r.subnets.join(", "), cx)
                }
            }
            col::GATEWAY => {
                if r.gateways.is_empty() {
                    dash(cx)
                } else {
                    mono_cell(r.gateways.join(", "), cx)
                }
            }
            col::CONTAINERS => mono_cell(r.containers.to_string(), cx),
            col::COMPOSE => match &r.compose {
                Some(p) => Tag::secondary().small().child(p.clone()).into_any_element(),
                None => div().into_any_element(),
            },
            col::CREATED => div()
                .text_xs()
                .text_color(muted)
                .child(relative_time(r.created, cx))
                .into_any_element(),
            col::ACTIONS => {
                if self.pending.contains(row.key.as_ref()) {
                    return Spinner::new().small().into_any_element();
                }
                let key = row.key.clone();
                let tip = if r.builtin {
                    s::BUILTIN_NETWORK
                } else if !self.can_manage {
                    s::NETWORK_MGMT_UNSUPPORTED
                } else {
                    s::ACTION_DELETE
                };
                h_flex()
                    .gap_0p5()
                    .child(crate::pages::resources::chrome::row_button(
                        ("delete", row_ix),
                        Lucide::Trash,
                        tip,
                        !self.can_delete(r),
                        on_row(&key, RowCommand::Delete),
                    ))
                    .child(crate::pages::resources::chrome::row_menu_button(
                        ("more", row_ix),
                        on_row(&key, RowCommand::ContextMenu),
                    ))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn row_text(&self, row: &ListRow<(), NetworkRow>) -> String {
        match &row.kind {
            RowKind::Item(r) => r.name.clone(),
            RowKind::Group { .. } => String::new(),
        }
    }

    fn context_menu(
        &self,
        row: &ListRow<(), NetworkRow>,
        menu: PopupMenu,
        _window: &Window,
        _cx: &App,
    ) -> PopupMenu {
        let key = row.key.clone();
        let can_delete = row.item().is_some_and(|r| self.can_delete(r));
        let lk = ctx::LIST_KEYS;
        let m = hinted(
            menu,
            s::CMD_OPEN_DETAIL,
            on_row(&key, RowCommand::Open),
            &list::OpenDetail,
            lk,
            false,
        );
        let m = hinted(
            m,
            s::COPY_ID,
            on_row(&key, RowCommand::CopyId),
            &list::CopyId,
            lk,
            false,
        );
        hinted(
            m.separator(),
            s::ACTION_DELETE,
            on_row(&key, RowCommand::Delete),
            &list::Delete,
            lk,
            !can_delete,
        )
    }

    fn render_empty(&self, _window: &mut Window, cx: &mut App) -> AnyElement {
        if self.filtered_out {
            return crate::ui::empty_state(IconName::Search, s::NO_MATCHING_NETWORKS, "", None, cx);
        }
        crate::ui::empty_state(
            IconName::Network,
            s::NO_NETWORKS,
            s::NO_NETWORKS_BODY,
            None,
            cx,
        )
    }

    fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }
}
