//! The Images `ListDelegate` (IMG-001): columns, cells, row menu (KBD-036), empty states.

use std::collections::HashSet;

use dk_core::format::format_size;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, SharedString, Window, div};

use super::model::{ImageRow, sort_keys};
use crate::actions::res::RunRow;
use crate::actions::{OnRow, RowCommand, image, list};
use crate::assets::Lucide;
use crate::keymap::ctx;
use crate::pages::resources::chrome::{
    hinted, mono_cell, name_cell, row_button, row_menu_button, text_cell,
};
use crate::strings as s;
use crate::ui::list_table::{ColumnSpec, ListDelegate, ListRow, RowKind};
use crate::ui::status_chip::{Tone, tone_tag};
use crate::ui::widgets::{copy_id, relative_time};

pub mod col {
    pub const SELECT: &str = "select";
    pub const NAME: &str = super::sort_keys::NAME;
    pub const TAG: &str = super::sort_keys::TAG;
    pub const ID: &str = "id";
    pub const CREATED: &str = super::sort_keys::CREATED;
    pub const SIZE: &str = super::sort_keys::SIZE;
    pub const STATUS: &str = "status";
    pub const ACTIONS: &str = "actions";
}

pub fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new(col::SELECT, "", 36.).fixed(),
        ColumnSpec::new(col::NAME, s::COL_NAME, 240.).sortable(),
        ColumnSpec::new(col::TAG, s::COL_TAG, 120.).sortable(),
        ColumnSpec::new(col::ID, s::COL_IMAGE_ID, 140.),
        ColumnSpec::new(col::CREATED, s::COL_CREATED, 120.).sortable(),
        ColumnSpec::new(col::SIZE, s::COL_SIZE, 90.)
            .sortable()
            .right(),
        ColumnSpec::new(col::STATUS, s::COL_STATUS, 90.),
        ColumnSpec::new(col::ACTIONS, s::COL_ACTIONS, 110.).pin_right(),
    ]
}

#[derive(Default)]
pub struct ImagesDelegate {
    pub read_only: bool,
    /// Image ids with a delete in flight (spinner, SHL-001).
    pub pending: HashSet<String>,
    /// True when rows exist but the filter/search hides them all.
    pub filtered_out: bool,
}

fn on_row(key: &SharedString, cmd: RowCommand) -> Box<dyn gpui_kit::Action> {
    Box::new(OnRow {
        row: key.clone(),
        action: cmd,
    })
}

impl ListDelegate for ImagesDelegate {
    type Group = ();
    type Item = ImageRow;

    fn columns(&self) -> Vec<ColumnSpec> {
        columns()
    }

    fn render_cell(
        &self,
        row: &ListRow<(), ImageRow>,
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
                    Icon::new(Lucide::Layers)
                        .small()
                        .text_color(if r.in_use > 0 {
                            cx.theme().success
                        } else {
                            muted
                        }),
                )
                .child(name_cell(r.repo.clone()))
                .into_any_element(),
            col::TAG => {
                if r.tag == s::NONE_TAG {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(s::NONE_TAG)
                        .into_any_element()
                } else {
                    text_cell(r.tag.clone())
                }
            }
            col::ID => copy_id(("copy-id", row_ix), &r.image_id, cx).into_any_element(),
            col::CREATED => div()
                .text_xs()
                .text_color(muted)
                .child(relative_time(r.created, cx))
                .into_any_element(),
            col::SIZE => mono_cell(format_size(r.size), cx),
            col::STATUS => {
                if r.in_use > 0 {
                    tone_tag(Tone::Success, s::IN_USE).into_any_element()
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
                        ("run", row_ix),
                        IconName::Play,
                        s::RUN,
                        self.read_only,
                        Box::new(RunRow { row: key.clone() }),
                    ))
                    .child(row_button(
                        ("delete", row_ix),
                        Lucide::Trash,
                        s::ACTION_DELETE,
                        self.read_only,
                        on_row(&key, RowCommand::Delete),
                    ))
                    .child(row_menu_button(
                        ("more", row_ix),
                        on_row(&key, RowCommand::ContextMenu),
                    ))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn row_text(&self, row: &ListRow<(), ImageRow>) -> String {
        match &row.kind {
            RowKind::Item(r) => format!("{}:{}", r.repo, r.tag),
            RowKind::Group { .. } => String::new(),
        }
    }

    fn context_menu(
        &self,
        row: &ListRow<(), ImageRow>,
        menu: PopupMenu,
        _window: &Window,
        _cx: &App,
    ) -> PopupMenu {
        let key = row.key.clone();
        let ro = self.read_only;
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
            s::CMD_RUN_IMAGE,
            Box::new(RunRow { row: key.clone() }),
            &image::Run,
            lk,
            ro,
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
            ro,
        )
    }

    fn render_empty(&self, _window: &mut Window, cx: &mut App) -> AnyElement {
        if self.filtered_out {
            return crate::ui::empty_state(IconName::Search, s::NO_MATCHING_IMAGES, "", None, cx);
        }
        crate::ui::empty_state(
            Lucide::Layers,
            s::NO_IMAGES,
            s::NO_IMAGES_BODY,
            Some((s::PULL_IMAGE.into(), Box::new(image::Pull))),
            cx,
        )
    }

    fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }
}
