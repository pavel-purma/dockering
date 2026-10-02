//! Overview tab (CDT-010): General / Command / Compose / Environment / Labels / Resources /
//! Health, as one focusable rows panel (KBD-044). Environment values whose key matches
//! `pass|secret|token|key` are masked with a per-row reveal toggle (`EnvVar::is_sensitive`).

use dk_core::format::{format_size, format_timestamp, short_id};
use dk_core::{ContainerDetails, HealthCheckResult};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Window,
};

use super::rows::{self, Cell, Row, RowsState, Section};
use super::state::ContainerDetailState;
use crate::actions::Navigate;
use crate::strings as s;
use crate::ui::links::image_route;

pub struct OverviewTab {
    state: Entity<ContainerDetailState>,
    rows: RowsState,
    _sub: Subscription,
}

impl OverviewTab {
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

    /// Rendered value of an env row (masked unless revealed), for tests.
    pub fn env_display(&self, key: &str, cx: &App) -> Option<String> {
        let d = self.state.read(cx).details.data()?;
        let var = d.env.iter().find(|e| e.key == key)?;
        let row_key = format!("env:{key}");
        Some(if var.is_sensitive() && !self.rows.is_revealed(&row_key) {
            s::MASKED_VALUE.to_owned()
        } else {
            var.value.clone()
        })
    }
}

fn opt(v: Option<String>) -> String {
    v.filter(|s| !s.is_empty())
        .unwrap_or_else(|| s::NONE_VALUE.to_owned())
}

fn joined(v: &[String]) -> String {
    if v.is_empty() {
        s::NONE_VALUE.to_owned()
    } else {
        v.iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn text_row(key: &str, label: &str, value: String) -> Row {
    Row::kv(key, label, Cell::text(value.clone()), value)
}

fn mono_row(key: &str, label: &str, value: String) -> Row {
    Row::kv(key, label, Cell::mono(value.clone()), value)
}

/// The sections for `d` (pure; unit-tested).
pub fn sections(d: &ContainerDetails, cx: &App) -> Vec<Section> {
    let c = &d.summary;
    let ts = |t: Option<time::OffsetDateTime>| t.map(format_timestamp);
    let mut general = vec![
        mono_row("id", s::F_ID, c.id.clone()),
        text_row("name", s::F_NAME, c.name.clone()),
        Row::kv(
            "image",
            s::F_IMAGE,
            Cell::Link(c.image.clone().into()),
            c.image.clone(),
        )
        .action(Box::new(Navigate {
            route: image_route(&c.image_id),
        })),
        mono_row("image_id", s::F_IMAGE_ID, short_id(&c.image_id).to_owned()),
        text_row(
            "created",
            s::F_CREATED,
            format!(
                "{} ({})",
                format_timestamp(c.created),
                crate::ui::relative_time(c.created, cx)
            ),
        ),
        text_row("started", s::F_STARTED, opt(ts(d.started_at))),
        text_row("finished", s::F_FINISHED, opt(ts(d.finished_at))),
        text_row(
            "restart_count",
            s::F_RESTART_COUNT,
            d.restart_count.to_string(),
        ),
        text_row(
            "restart_policy",
            s::F_RESTART_POLICY,
            opt(d.restart_policy.clone()),
        ),
        text_row("platform", s::F_PLATFORM, opt(d.platform.clone())),
        text_row(
            "pid",
            s::F_PID,
            opt(d.pid.filter(|p| *p > 0).map(|p| p.to_string())),
        ),
    ];
    if let Some(code) = c.exit_code.filter(|_| !c.state.is_running()) {
        general.push(text_row("exit_code", s::F_EXIT_CODE, code.to_string()));
    }
    let command = vec![
        mono_row("entrypoint", s::F_ENTRYPOINT, joined(&d.entrypoint)),
        mono_row("cmd", s::F_CMD, joined(&d.cmd)),
        mono_row("workdir", s::F_WORKDIR, opt(d.working_dir.clone())),
        text_row("user", s::F_USER, opt(d.user.clone())),
        text_row(
            "tty",
            s::F_TTY,
            if d.tty { s::YES } else { s::NO }.to_owned(),
        ),
    ];
    let mut out = vec![
        Section::list("ov-general", s::SEC_GENERAL, general),
        Section::list("ov-command", s::SEC_COMMAND, command),
    ];
    if let Some(ci) = &c.compose {
        out.push(Section::list(
            "ov-compose",
            s::SEC_COMPOSE,
            vec![
                text_row("compose:project", s::F_PROJECT, ci.project.clone()),
                text_row("compose:service", s::F_SERVICE, ci.service.clone()),
                text_row(
                    "compose:number",
                    s::F_NUMBER,
                    opt(ci.number.map(|n| n.to_string())),
                ),
                mono_row("compose:workdir", s::F_WORKDIR, opt(ci.working_dir.clone())),
                mono_row("compose:files", s::F_CONFIG_FILES, joined(&ci.config_files)),
            ],
        ));
    }
    let env: Vec<Row> = d
        .env
        .iter()
        .map(|e| {
            let value = if e.is_sensitive() {
                Cell::Secret(e.value.clone().into())
            } else {
                Cell::mono(e.value.clone())
            };
            Row::new(
                format!("env:{}", e.key),
                vec![Cell::mono(e.key.clone()), value],
                e.value.clone(),
            )
        })
        .collect();
    out.push(Section::table(
        "ov-env",
        s::SEC_ENVIRONMENT,
        &[s::F_KEY, s::F_VALUE],
        env,
        s::NO_ENV,
    ));
    let labels: Vec<Row> = c
        .labels
        .iter()
        .map(|(k, v)| {
            Row::new(
                format!("label:{k}"),
                vec![Cell::mono(k.clone()), Cell::mono(v.clone())],
                v.clone(),
            )
        })
        .collect();
    out.push(Section::table(
        "ov-labels",
        s::SEC_LABELS,
        &[s::F_KEY, s::F_VALUE],
        labels,
        s::NO_LABELS,
    ));
    let r = &d.resources;
    let cpu = r
        .nano_cpus
        .filter(|n| *n > 0)
        .map(|n| s::cpus(n as f64 / 1e9))
        .unwrap_or_else(|| s::UNLIMITED.to_owned());
    let mem = r
        .memory
        .filter(|m| *m > 0)
        .map(|m| format_size(m as u64))
        .unwrap_or_else(|| s::UNLIMITED.to_owned());
    let pids = r
        .pids_limit
        .filter(|p| *p > 0)
        .map(|p| p.to_string())
        .unwrap_or_else(|| s::UNLIMITED.to_owned());
    out.push(Section::list(
        "ov-resources",
        s::SEC_RESOURCES,
        vec![
            text_row("res:cpu", s::F_CPU_LIMIT, cpu),
            text_row("res:mem", s::F_MEMORY_LIMIT, mem),
            text_row("res:pids", s::F_PIDS_LIMIT, pids),
        ],
    ));
    let health: Vec<Row> = d
        .health_log
        .iter()
        .rev()
        .take(5)
        .enumerate()
        .map(|(i, h)| health_row(i, h))
        .collect();
    if c.health.is_some() || !health.is_empty() {
        out.push(Section::table(
            "ov-health",
            s::SEC_HEALTH,
            &[s::F_STARTED, s::F_EXIT_CODE, s::F_CMD],
            health,
            s::NO_HEALTH,
        ));
    }
    out
}

fn health_row(i: usize, h: &HealthCheckResult) -> Row {
    let when = h
        .start
        .map(format_timestamp)
        .unwrap_or_else(|| s::NONE_VALUE.to_owned());
    let output = h.output.trim().to_owned();
    Row::new(
        format!("health:{i}"),
        vec![
            Cell::text(when),
            Cell::text(h.exit_code.to_string()),
            Cell::mono(output.clone()),
        ],
        output,
    )
}

impl Focusable for OverviewTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.rows.focus.clone()
    }
}

impl Render for OverviewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = self
            .state
            .read(cx)
            .details
            .data()
            .map(|d| sections(d, cx))
            .unwrap_or_default();
        rows::render(
            "overview-rows",
            &mut self.rows,
            sections,
            window,
            cx,
            |v: &mut Self| &mut v.rows,
        )
    }
}
