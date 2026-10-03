//! Mounts tab (CDT-020): Type, Source (a volume name links to volume detail), Destination,
//! Mode, as one focusable rows panel (KBD-044: `Enter` follows a volume link).

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

/// `RW`/`RO` from the access flag, then any other mode options (`z`, `Z`, …); Docker's
/// `Mode` string repeats `rw`/`ro` for binds and is often empty for volumes.
pub fn mode_label(rw: bool, mode: &str) -> String {
    let access = if rw {
        s::READ_WRITE
    } else {
        s::READ_ONLY_SHORT
    };
    std::iter::once(access)
        .chain(
            mode.split(',')
                .map(str::trim)
                .filter(|o| !o.is_empty() && *o != "rw" && *o != "ro"),
        )
        .collect::<Vec<_>>()
        .join(", ")
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
            let row = Row::new(
                format!("mount:{i}"),
                vec![
                    Cell::text(kind_label(m.kind)),
                    source,
                    Cell::mono(m.destination.clone()),
                    Cell::text(mode_label(m.rw, &m.mode)),
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
    vec![
        Section::table(
            "mounts",
            crate::nav::ContainerTab::Mounts.label(),
            &[s::COL_TYPE, s::COL_SOURCE, s::COL_DESTINATION, s::COL_MODE],
            rows,
            s::NO_MOUNTS,
        )
        // Paths get the room; type and mode are short.
        .weights(&[1., 4., 4., 1.]),
    ]
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

#[cfg(test)]
mod tests {
    use super::mode_label;

    #[test]
    fn cdt_020_mode_label_merges_access_and_options() {
        assert_eq!(mode_label(true, "rw"), "RW");
        assert_eq!(mode_label(false, "ro"), "RO");
        assert_eq!(mode_label(true, ""), "RW");
        assert_eq!(mode_label(true, "z"), "RW, z");
        assert_eq!(mode_label(false, "ro,Z"), "RO, Z");
    }
}
