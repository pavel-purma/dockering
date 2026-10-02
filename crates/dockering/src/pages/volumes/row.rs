//! The Volumes `ListDelegate` (VOL-001/002): columns, lazy size cells, row menu, empty state.

use std::collections::HashSet;

use dk_core::format::format_size;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, SharedString, Window, div};

use super::model::{VolumeRow, sort_keys};
use crate::actions::{OnRow, RowCommand, list, volume};
use crate::assets::Lucide;
use crate::keymap::ctx;
use crate::pages::resources::chrome::{
    dash, hinted, mono_cell, name_cell, row_button, skeleton_cell, text_cell,
};
use crate::strings as s;
use crate::ui::list_table::{ColumnSpec, ListDelegate, ListRow, RowKind};
use crate::ui::status_chip::{Tone, tone_tag};
use crate::ui::widgets::relative_time;

pub mod col {
    pub const SELECT: &str = "select";
    pub const NAME: &str = super::sort_keys::NAME;
    pub const DRIVER: &str = "driver";
    pub const COMPOSE: &str = "compose";
    pub const CREATED: &str = super::sort_keys::CREATED;
    pub const SIZE: &str = super::sort_keys::SIZE;
    pub const STATUS: &str = "status";
    pub const ACTIONS: &str = "actions";
}

pub fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new(col::SELECT, "", 36.).fixed(),
        ColumnSpec::new(col::NAME, s::COL_NAME, 280.).sortable(),
        ColumnSpec::new(col::DRIVER, s::COL_DRIVER, 80.),
        ColumnSpec::new(col::COMPOSE, s::COL_COMPOSE, 140.),
        ColumnSpec::new(col::CREATED, s::COL_CREATED, 120.).sortable(),
        ColumnSpec::new(col::SIZE, s::COL_SIZE, 90.)
            .sortable()
            .right(),
        ColumnSpec::new(col::STATUS, s::COL_STATUS, 170.),
        ColumnSpec::new(col::ACTIONS, s::COL_ACTIONS, 80.).pin_right(),
    ]
}

#[derive(Default)]
pub struct VolumesDelegate {
    pub read_only: bool,
    pub pending: HashSet<String>,
    pub filtered_out: bool,
    /// `disk_usage` is loading: size/status cells show skeletons (VOL-002).
    pub sizes_loading: bool,
}

fn on_row(key: &SharedString, cmd: RowCommand) -> Box<dyn gpui_kit::Action> {
    Box::new(OnRow {
        row: key.clone(),
        action: cmd,
    })
}

impl ListDelegate for VolumesDelegate {
    type Group = ();
    type Item = VolumeRow;

    fn columns(&self) -> Vec<ColumnSpec> {
        columns()
    }

    fn render_cell(
        &self,
        row: &ListRow<(), VolumeRow>,
        row_ix: usize,
        column: &ColumnSpec,
        selected: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let RowKind::Item(r) = &row.kind else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        match column.key {
            col::SELECT => {
                let key = row.key.clone();
                Checkbox::new(("row-check", row_ix))
                    .checked(selected)
                    .tab_stop(false)
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        window.dispatch_action(on_row(&key, RowCommand::ToggleSelected), cx)
                    })
                    .into_any_element()
            }
            col::NAME => h_flex()
                .gap_1()
                .items_center()
                .overflow_hidden()
                .child(
                    Icon::new(IconName::HardDrive)
                        .small()
                        .text_color(if r.in_use() > 0 {
                            cx.theme().success
                        } else {
                            muted
                        }),
                )
                .child(name_cell(r.name.clone()))
                .into_any_element(),
            col::DRIVER => text_cell(r.driver.clone()),
            col::COMPOSE => match &r.compose {
                Some(p) => Tag::secondary().small().child(p.clone()).into_any_element(),
                None => div().into_any_element(),
            },
            col::CREATED => match r.created {
                Some(t) => div()
                    .text_xs()
                    .text_color(muted)
                    .child(relative_time(t, cx))
                    .into_any_element(),
                None => dash(cx),
            },
            col::SIZE => match r.size {
                Some(sz) => mono_cell(format_size(sz), cx),
                None if self.sizes_loading => skeleton_cell(48.),
                None => dash(cx),
            },
            col::STATUS => {
                if self.sizes_loading && r.ref_count.is_none() && r.used_by.is_empty() {
                    return skeleton_cell(90.);
                }
                let n = r.in_use();
                if n > 0 {
                    tone_tag(Tone::Success, s::in_use_by(n)).into_any_element()
                } else {
                    div().into_any_element()
                }
            }
            col::ACTIONS => {
                if self.pending.contains(row.key.as_ref()) {
                    return Spinner::new().small().into_any_element();
                }
                let key = row.key.clone();
                h_flex()
                    .gap_0p5()
                    .child(row_button(
                        ("delete", row_ix),
                        Lucide::Trash,
                        s::ACTION_DELETE,
                        self.read_only,
                        on_row(&key, RowCommand::Delete),
                    ))
                    .child(row_button(
                        ("more", row_ix),
                        IconName::EllipsisVertical,
                        s::MORE_ACTIONS,
                        false,
                        on_row(&key, RowCommand::ContextMenu),
                    ))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn row_text(&self, row: &ListRow<(), VolumeRow>) -> String {
        match &row.kind {
            RowKind::Item(r) => r.name.clone(),
            RowKind::Group { .. } => String::new(),
        }
    }

    fn context_menu(
        &self,
        row: &ListRow<(), VolumeRow>,
        menu: PopupMenu,
        _window: &Window,
        _cx: &App,
    ) -> PopupMenu {
        let key = row.key.clone();
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
            s::COPY_NAME,
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
            self.read_only,
        )
    }

    fn render_empty(&self, _window: &mut Window, cx: &mut App) -> AnyElement {
        if self.filtered_out {
            return crate::ui::empty_state(IconName::Search, s::NO_MATCHING_VOLUMES, "", None, cx);
        }
        crate::ui::empty_state(
            IconName::HardDrive,
            s::NO_VOLUMES,
            s::NO_VOLUMES_BODY,
            Some((s::CREATE_VOLUME.into(), Box::new(volume::Create))),
            cx,
        )
    }

    fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }
}
