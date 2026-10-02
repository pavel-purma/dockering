//! Mounts tab (CDT-020): Type, Source (a volume name links to volume detail), Destination,
//! Mode, RW, as one focusable rows panel (KBD-044: `Enter` follows a volume link).

use dk_core::{ContainerDetails, MountKind};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Window,
};

use super::rows::{self, Cell, Row, RowsState, Section};
use super::state::ContainerDetailState;
use crate::actions::Navigate;
use crate::strings as s;
use crate::ui::links::volume_route;

pub struct MountsTab {
    state: Entity<ContainerDetailState>,
    rows: RowsState,
    _sub: Subscription,
}

impl MountsTab {
    pub fn new(state: Entity<ContainerDetailState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            rows: RowsState::new(cx),
            _sub: sub,
        }
    }

    pub fn rows(&self) -> &RowsState {
        &self.rows
    }

    pub fn rows_mut(&mut self) -> &mut RowsState {
        &mut self.rows
    }
}

pub fn kind_label(k: MountKind) -> &'static str {
    match k {
        MountKind::Volume => "volume",
        MountKind::Bind => "bind",
        MountKind::Tmpfs => "tmpfs",
        MountKind::Npipe => "npipe",
        MountKind::Unknown => "unknown",
    }
}

pub fn sections(d: &ContainerDetails) -> Vec<Section> {
    let rows = d
        .mounts
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let volume = (m.kind == MountKind::Volume)
                .then(|| m.volume_name.clone().unwrap_or_else(|| m.source.clone()))
                .filter(|v| !v.is_empty());
            let source = match &volume {
                Some(v) => Cell::Link(v.clone().into()),
                None => Cell::mono(m.source.clone()),
            };
            let mode = if m.mode.is_empty() {
                s::NONE_VALUE.to_owned()
            } else {
                m.mode.clone()
            };
            let row = Row::new(
                format!("mount:{i}"),
                vec![
                    Cell::text(kind_label(m.kind)),
                    source,
                    Cell::mono(m.destination.clone()),
                    Cell::text(mode),
                    Cell::text(if m.rw { s::YES } else { s::NO }),
                ],
                volume.clone().unwrap_or_else(|| m.source.clone()),
            );
            match volume {
                Some(v) => row.action(Box::new(Navigate {
                    route: volume_route(&v),
                })),
                None => row,
            }
        })
        .collect();
    vec![Section::table(
        "mounts",
        crate::nav::ContainerTab::Mounts.label(),
        &[
            s::COL_TYPE,
            s::COL_SOURCE,
            s::COL_DESTINATION,
            s::COL_MODE,
            s::COL_RW,
        ],
        rows,
        s::NO_MOUNTS,
    )]
}

impl Focusable for MountsTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.rows.focus.clone()
    }
}

impl Render for MountsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = self
            .state
            .read(cx)
            .details
            .data()
            .map(sections)
            .unwrap_or_default();
        rows::render(
            "mounts-rows",
            &mut self.rows,
            sections,
            window,
            cx,
            |v: &mut Self| &mut v.rows,
        )
    }
}
