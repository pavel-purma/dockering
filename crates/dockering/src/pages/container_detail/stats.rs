//! Stats tab (STA-001…011, KBD-070).
//!
//! - Subscribes `hub.stats(engine, id)` only while the tab is visible (STA-006); the hub's
//!   StatsService replays its history first, so reopening shows the buffer (STA-003).
//! - Keeps a local window of samples (≤ 3,600) and a 1m / 5m / 15m selector (default 5m) as a
//!   focusable segmented control (`←`/`→`, `1`/`5`/`F`, KBD-070).
//! - Four chart cards (GPUI Kit `AreaChart` / `LineChart`, stable ids, built from cloned
//!   `Vec<Point>`s per render and downsampled to ≤ 300 points with LTTB, STA-003/004), with
//!   human units (STA-005) and the current values as text in each header (STA-011).
//! - Gaps (`Feed::Lagged`) are shown; a stopped container shows the last buffer greyed with
//!   "Container not running" (STA-007); processes from `top()` every 5 s while visible with
//!   `TOP` (STA-008); disk usage on demand (`size = true`, STA-010).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dk_core::format::{format_rate, format_size};
use dk_core::stats::downsample_lttb;
use dk_core::{Capabilities, ContainerQuery, EngineError, ProcessList, StatsSample};
use dk_hub::Feed;
use futures::StreamExt;
use gpui_kit::component::button::Button;
use gpui_kit::component::chart::{AreaChart, LineChart};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Hsla, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, linear_color_stop, linear_gradient, px,
};
use time::OffsetDateTime;

use super::state::ContainerDetailState;
use crate::actions::stats as act;
use crate::keymap::ctx;
use crate::state::AppState;
use crate::strings as s;
use crate::ui::dispatch;

/// Max points per chart series (STA-003).
pub const MAX_POINTS: usize = 300;
/// Local sample cap (STA-003).
pub const MAX_SAMPLES: usize = 3600;
/// `top()` refresh while visible (STA-008).
pub const TOP_INTERVAL: Duration = Duration::from_secs(5);
/// Max stream items applied per `update` / `cx.notify()`: the hub's history replay (up to
/// `MAX_SAMPLES` at once) lands in a handful of frames instead of one notify per sample.
pub const APPLY_CHUNK: usize = 256;

/// Time window (KBD-070).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsWindow {
    M1,
    M5,
    M15,
}

impl StatsWindow {
    pub const ALL: [StatsWindow; 3] = [StatsWindow::M1, StatsWindow::M5, StatsWindow::M15];

    pub fn label(self) -> &'static str {
        match self {
            StatsWindow::M1 => "1m",
            StatsWindow::M5 => "5m",
            StatsWindow::M15 => "15m",
        }
    }

    pub fn duration(self) -> time::Duration {
        match self {
            StatsWindow::M1 => time::Duration::minutes(1),
            StatsWindow::M5 => time::Duration::minutes(5),
            StatsWindow::M15 => time::Duration::minutes(15),
        }
    }

    fn ix(self) -> usize {
        Self::ALL.iter().position(|w| *w == self).unwrap_or(1)
    }
}

/// One chart point: label (time) + y values (one per series).
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub label: SharedString,
    pub a: f64,
    pub b: f64,
}

/// A sample or a gap marker (Feed::Lagged).
#[derive(Debug, Clone)]
enum Entry {
    Sample(StatsSample),
    Gap(u64),
}

/// Series for the four charts, already downsampled (pure; benchmarked, STA-009).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChartData {
    pub cpu: Vec<Point>,
    pub mem: Vec<Point>,
    pub net: Vec<Point>,
    pub disk: Vec<Point>,
    pub mem_limit: Option<u64>,
}

fn time_label(t: OffsetDateTime) -> SharedString {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()).into()
}

/// Downsamples `(x, a)` / `(x, b)` pairs to ≤ `MAX_POINTS` (LTTB on series `a`, `b` follows
/// the same x positions so the two series stay aligned).
fn series(samples: &[&StatsSample], f: impl Fn(&StatsSample) -> (f64, f64)) -> Vec<Point> {
    if samples.is_empty() {
        return Vec::new();
    }
    let t0 = samples[0].at;
    let xs: Vec<(f64, f64)> = samples
        .iter()
        .map(|s| ((s.at - t0).as_seconds_f64(), f(s).0))
        .collect();
    let picked = downsample_lttb(&xs, MAX_POINTS);
    // Map picked x back to sample indices (x is monotonic).
    let mut out = Vec::with_capacity(picked.len());
    let mut j = 0;
    for (x, _) in picked {
        while j + 1 < xs.len() && xs[j].0 < x {
            j += 1;
        }
        let smp = samples[j];
        let (a, b) = f(smp);
        out.push(Point {
            label: time_label(smp.at),
            a,
            b,
        });
    }
    out
}

/// Builds the chart series for `window` (STA-003/005). `relative` = CPU % of all cores.
pub fn chart_data(
    samples: &[StatsSample],
    window: StatsWindow,
    relative: bool,
    now: OffsetDateTime,
) -> ChartData {
    let since = now - window.duration();
    let in_window: Vec<&StatsSample> = samples.iter().filter(|s| s.at >= since).collect();
    let cpu = |s: &StatsSample| {
        let v = if relative {
            s.cpu_percent / s.online_cpus.max(1) as f64
        } else {
            s.cpu_percent
        };
        (v, 0.0)
    };
    ChartData {
        cpu: series(&in_window, cpu),
        mem: series(&in_window, |s| (s.mem_used as f64, s.mem_limit as f64)),
        net: series(&in_window, |s| (s.net_rx_bps, s.net_tx_bps)),
        disk: series(&in_window, |s| (s.blk_read_bps, s.blk_write_bps)),
        mem_limit: in_window.last().map(|s| s.mem_limit).filter(|l| *l > 0),
    }
}

pub struct StatsTab {
    state: Entity<ContainerDetailState>,
    entries: VecDeque<Entry>,
    window: StatsWindow,
    visible: bool,
    window_focus: FocusHandle,
    card_focus: [FocusHandle; 4],
    stream: Option<Task<()>>,
    stream_error: Option<EngineError>,
    /// Revision of the stats subscription (stale stream items dropped, NFR-005).
    generation: u64,
    top: Option<ProcessList>,
    top_error: Option<EngineError>,
    top_task: Option<Task<()>>,
    disk: Option<(Option<u64>, Option<u64>)>,
    disk_loading: bool,
    disk_task: Option<Task<()>>,
    /// Last rebuild cost (STA-009, diagnostics).
    pub last_build: Duration,
    /// `cx.notify()` calls caused by stream items (tests: replay batching).
    stream_notifies: usize,
    _subs: Vec<Subscription>,
}

impl StatsTab {
    pub fn new(state: Entity<ContainerDetailState>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&state, |this, _, cx| {
            this.sync_running(cx);
            cx.notify();
        })];
        let mut this = Self {
            state,
            entries: VecDeque::new(),
            window: StatsWindow::M5,
            visible: false,
            window_focus: cx.focus_handle().tab_stop(true),
            card_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            stream: None,
            stream_error: None,
            generation: 0,
            top: None,
            top_error: None,
            top_task: None,
            disk: None,
            disk_loading: false,
            disk_task: None,
            last_build: Duration::ZERO,
            stream_notifies: 0,
            _subs: subs,
        };
        this.set_visible(true, cx);
        this
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn window(&self) -> StatsWindow {
        self.window
    }
    pub fn samples(&self) -> Vec<StatsSample> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Sample(s) => Some(s.clone()),
                Entry::Gap(_) => None,
            })
            .collect()
    }
    pub fn gaps(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| match e {
                Entry::Gap(n) => *n,
                _ => 0,
            })
            .sum()
    }
    pub fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }
    /// Notifies caused by stream items (one per applied chunk).
    pub fn stream_notifies(&self) -> usize {
        self.stream_notifies
    }
    pub fn top(&self) -> Option<&ProcessList> {
        self.top.as_ref()
    }
    pub fn disk(&self) -> Option<(Option<u64>, Option<u64>)> {
        self.disk
    }
    pub fn window_focus(&self) -> &FocusHandle {
        &self.window_focus
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.state.read(cx).capabilities(cx)
    }

    fn running(&self, cx: &App) -> bool {
        self.state.read(cx).is_running(cx)
    }

    // ── visibility & streams (STA-006/008) ─────────────────────────────────────────────

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible {
            self.subscribe(cx);
            self.start_top(cx);
        } else {
            // Dropping the stream unsubscribes; the hub keeps the history (STA-006).
            self.generation += 1;
            self.stream = None;
            self.top_task = None;
        }
    }

    fn sync_running(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if self.running(cx) && self.stream.is_none() {
            self.subscribe(cx);
        }
        if self.running(cx) && self.top_task.is_none() {
            self.start_top(cx);
        }
    }

    fn subscribe(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).is_removed() {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        // History is replayed on every subscribe: start from a clean local window.
        self.entries.clear();
        self.stream_error = None;
        let (engine, id) = {
            let st = self.state.read(cx);
            (st.engine().clone(), st.id().to_owned())
        };
        // Whatever is already buffered (the history replay) arrives as chunks of up to
        // `APPLY_CHUNK` items; each chunk is applied in one `update` with one notify.
        let mut stream = AppState::hub(cx)
            .stats(&engine, &id)
            .ready_chunks(APPLY_CHUNK);
        self.stream = Some(cx.spawn(async move |this, cx| {
            while let Some(chunk) = stream.next().await {
                let keep = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        let keep = this.apply_chunk(chunk);
                        this.stream_notifies += 1;
                        cx.notify();
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.stream = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Applies stream items in order; `false` once an error ends the stream (later items in
    /// the chunk are ignored, like the per-item loop did).
    fn apply_chunk(&mut self, chunk: Vec<Result<Feed<StatsSample>, EngineError>>) -> bool {
        for item in chunk {
            match item {
                Ok(Feed::Item(s)) => self.push(Entry::Sample(s)),
                Ok(Feed::Lagged { dropped }) => self.push(Entry::Gap(dropped)),
                Err(e) => {
                    self.stream_error = Some(e);
                    return false;
                }
            }
        }
        true
    }

    fn push(&mut self, e: Entry) {
        if self.entries.len() >= MAX_SAMPLES {
            self.entries.pop_front();
        }
        self.entries.push_back(e);
    }

    /// `top()` every 5 s while visible and running, when the engine has `TOP` (STA-008).
    fn start_top(&mut self, cx: &mut Context<Self>) {
        if !self.caps(cx).contains(Capabilities::TOP) || !self.running(cx) {
            self.top_task = None;
            return;
        }
        let (engine, id) = {
            let st = self.state.read(cx);
            (st.engine().clone(), st.id().to_owned())
        };
        let hub = AppState::hub(cx);
        self.top_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let id2 = id.clone();
                let result = hub
                    .call(&engine, move |e| async move { e.top(&id2).await })
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        match result {
                            Ok(p) => {
                                this.top = Some(p);
                                this.top_error = None;
                            }
                            Err(e) => this.top_error = Some(e),
                        }
                        cx.notify();
                        this.visible && this.running(cx)
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                cx.background_executor().timer(TOP_INTERVAL).await;
            }
            this.update(cx, |this, _| this.top_task = None).ok();
        }));
    }

    /// STA-010: `list_containers(size = true)` filtered to this id, on demand.
    fn load_disk(&mut self, _: &act::LoadDiskUsage, _: &mut Window, cx: &mut Context<Self>) {
        let (engine, id) = {
            let st = self.state.read(cx);
            (st.engine().clone(), st.id().to_owned())
        };
        self.disk_loading = true;
        cx.notify();
        let call = AppState::hub(cx).call(&engine, |e| async move {
            e.list_containers(ContainerQuery {
                all: true,
                size: true,
                label_filter: Vec::new(),
            })
            .await
        });
        self.disk_task = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.disk_loading = false;
                if let Ok(list) = result
                    && let Some(c) = list
                        .into_iter()
                        .find(|c| super::state::matches_id(&c.id, &id))
                {
                    this.disk = Some((c.size_rw, c.size_root_fs));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_window(&mut self, w: StatsWindow, cx: &mut Context<Self>) {
        self.window = w;
        cx.notify();
    }

    fn step_window(&mut self, d: isize, cx: &mut Context<Self>) {
        let ix = (self.window.ix() as isize + d).clamp(0, 2) as usize;
        self.set_window(StatsWindow::ALL[ix], cx);
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    /// A chart card. `value` is the current value as text (STA-011); `legend` colours its
    /// parts like the chart lines (colour is never the only signal: the text names them).
    #[allow(clippy::too_many_arguments)]
    fn card(
        &self,
        ix: usize,
        title: &'static str,
        value: Vec<(String, Option<Hsla>)>,
        chart: AnyElement,
        cx: &App,
    ) -> AnyElement {
        v_flex()
            .id(("stats-card", ix))
            .track_focus(&self.card_focus[ix])
            .flex_1()
            .min_w(px(280.))
            .h(px(220.))
            .p_3()
            .gap_2()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(h_flex().gap_3().text_sm().children(value.into_iter().map(
                        |(text, color)| {
                            div().when_some(color, |el, c| el.text_color(c)).child(text)
                        },
                    ))),
            )
            .child(div().flex_1().min_h_0().child(chart))
            .into_any_element()
    }
}

fn gradient(color: Hsla) -> gpui_kit::Background {
    linear_gradient(
        0.,
        linear_color_stop(color.opacity(0.4), 1.),
        linear_color_stop(color.opacity(0.05), 0.),
    )
}

/// The four chart elements (STA-004): built per render from cloned point vectors with
/// stable ids. Public for the STA-009 benchmark.
pub fn build_charts(data: &ChartData, colors: [Hsla; 4]) -> [AnyElement; 4] {
    let [c1, c2, c3, c4] = colors;
    let mem_max = data.mem_limit.map(|l| l as f64);
    // CPU: at least 0–1 % so an idle container still gets readable ticks; one decimal
    // while the scale is small (STA-005).
    let cpu_max = nice_max(data.cpu.iter().map(|p| p.a), 1.0);
    let cpu_fmt = if cpu_max <= 5.0 {
        |v: f64| format!("{v:.1}%")
    } else {
        |v: f64| format!("{v:.0}%")
    };
    let cpu = AreaChart::new(data.cpu.clone())
        .id("stats-cpu")
        .x(|p: &Point| p.label.clone())
        .y(|p: &Point| p.a)
        .stroke(c1)
        .fill(gradient(c1))
        .linear()
        .y_domain(0.0, cpu_max)
        .x_tick_count(4)
        .y_axis(true)
        .y_tick_count(3)
        .y_tick_format(cpu_fmt)
        .tooltip_value(|_, _, v| format!("{v:.2}%").into())
        .into_any_element();
    let mut mem = AreaChart::new(data.mem.clone())
        .id("stats-mem")
        .x(|p: &Point| p.label.clone())
        .y(|p: &Point| p.a)
        .stroke(c2)
        .fill(gradient(c2))
        .linear()
        .x_tick_count(4)
        .y_axis(true)
        .y_tick_count(3)
        .y_tick_format(|v| format_size(v.max(0.0) as u64))
        .tooltip_value(|_, _, v| format_size(v.max(0.0) as u64).into());
    // Pinned to the limit only when one is set explicitly (otherwise the "limit" is the
    // host's RAM and the line would hug the axis); auto-scaled from 0 otherwise.
    mem = match mem_max {
        Some(max) if data.mem.iter().all(|p| p.a <= max) => mem.y_domain(0.0, max),
        _ => mem.y_domain(0.0, nice_max(data.mem.iter().map(|p| p.a), 1024.0 * 1024.0)),
    };
    let rate = |v: f64| format_rate(v.max(0.0));
    let net = two_lines(
        "stats-net",
        &data.net,
        c3,
        c4,
        rate,
        s::STATS_RX,
        s::STATS_TX,
    );
    let disk = two_lines(
        "stats-disk",
        &data.disk,
        c3,
        c4,
        rate,
        s::STATS_READ,
        s::STATS_WRITE,
    );
    [cpu, mem.into_any_element(), net, disk]
}

/// Two series (rx/tx, read/write) as overlaid GPUI Kit `LineChart`s (one series each).
fn two_lines(
    id: &'static str,
    points: &[Point],
    ca: Hsla,
    cb: Hsla,
    fmt: fn(f64) -> String,
    name_a: &'static str,
    name_b: &'static str,
) -> AnyElement {
    // Both lines share one y scale: pin the domain to the max of both (≥ 1 kB/s so idle
    // charts get distinct tick labels).
    let max = nice_max(points.iter().flat_map(|p| [p.a, p.b]), 1024.0);
    let a = LineChart::new(points.to_vec())
        .id(SharedString::from(format!("{id}-a")))
        .x(|p: &Point| p.label.clone())
        .y(|p: &Point| p.a)
        .stroke(ca)
        .linear()
        .name(name_a)
        .y_domain(0.0, max)
        .x_tick_count(4)
        .y_axis(true)
        .y_tick_count(3)
        .y_tick_format(fmt)
        .tooltip_value(move |_, v| fmt(v).into());
    // Same axes, labels and gutter as `a` so the two lines align exactly; its labels paint
    // over `a`'s identically.
    let b = LineChart::new(points.to_vec())
        .id(SharedString::from(format!("{id}-b")))
        .x(|p: &Point| p.label.clone())
        .y(|p: &Point| p.b)
        .stroke(cb)
        .linear()
        .name(name_b)
        .y_domain(0.0, max)
        .x_tick_count(4)
        .grid(false)
        .y_axis(true)
        .y_tick_count(3)
        .y_tick_format(fmt)
        .tooltip_value(move |_, v| fmt(v).into());
    div()
        .size_full()
        .relative()
        .child(div().absolute().inset_0().child(a))
        .child(div().absolute().inset_0().child(b))
        .into_any_element()
}

/// A rounded-up axis maximum: the data max with 10 % headroom, at least `floor`.
fn nice_max(values: impl Iterator<Item = f64>, floor: f64) -> f64 {
    let max = values.fold(0.0f64, f64::max);
    let target = (max * 1.1).max(floor);
    // Round up to 1 / 2 / 5 × 10^n.
    let mag = 10f64.powf(target.log10().floor());
    let n = target / mag;
    let step = if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * mag
}

impl Focusable for StatsTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.window_focus.clone()
    }
}

impl Render for StatsTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running(cx);
        let relative = AppState::config(cx).stats.cpu_relative_to_all_cores;
        let samples = self.samples();
        let last = samples.last().cloned();
        // The window ends at the newest sample (keeps a stopped container's buffer visible).
        let now = last
            .as_ref()
            .map(|s| s.at)
            .unwrap_or_else(OffsetDateTime::now_utc);
        let started = Instant::now();
        let mut data = chart_data(&samples, self.window, relative, now);
        let explicit_limit = self
            .state
            .read(cx)
            .details
            .data()
            .and_then(|d| d.resources.memory)
            .is_some_and(|m| m > 0);
        if !explicit_limit {
            data.mem_limit = None;
        }
        let theme = cx.theme();
        let colors = [theme.chart_1, theme.chart_2, theme.blue, theme.yellow];
        let charts = build_charts(&data, colors);
        self.last_build = started.elapsed();

        let cpu_v = last.as_ref().map_or_else(
            || s::NONE_VALUE.to_owned(),
            |l| {
                let v = if relative {
                    l.cpu_percent / l.online_cpus.max(1) as f64
                } else {
                    l.cpu_percent
                };
                dk_core::format::format_percent(v)
            },
        );
        let mem_v = last.as_ref().map_or_else(
            || s::NONE_VALUE.to_owned(),
            |l| {
                let limit = (l.mem_limit > 0).then(|| format_size(l.mem_limit));
                s::stats_mem(&format_size(l.mem_used), limit.as_deref())
            },
        );
        let none = || vec![(s::NONE_VALUE.to_owned(), None)];
        let (ca, cb) = (Some(colors[2]), Some(colors[3]));
        let net_v = last.as_ref().map_or_else(none, |l| {
            vec![
                (format!("↓ {}", format_rate(l.net_rx_bps)), ca),
                (format!("↑ {}", format_rate(l.net_tx_bps)), cb),
            ]
        });
        let disk_v = last.as_ref().map_or_else(none, |l| {
            vec![
                (format!("R {}", format_rate(l.blk_read_bps)), ca),
                (format!("W {}", format_rate(l.blk_write_bps)), cb),
            ]
        });
        let cpu_v = vec![(cpu_v, None)];
        let mem_v = vec![(mem_v, None)];
        let [cpu, mem, net, disk] = charts;
        let cards = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(self.card(0, s::STATS_CPU, cpu_v, cpu, cx))
                    .child(self.card(1, s::STATS_MEMORY, mem_v, mem, cx)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(self.card(2, s::STATS_NETWORK, net_v, net, cx))
                    .child(self.card(3, s::STATS_DISK, disk_v, disk, cx)),
            );

        let selector = div()
            .id("stats-window")
            .key_context(format!("{} {}", ctx::DETAIL_HEADER, ctx::STATS_WINDOW).as_str())
            .track_focus(&self.window_focus)
            .rounded(theme.radius)
            .on_action(cx.listener(|this, _: &act::PrevWindow, _, cx| this.step_window(-1, cx)))
            .on_action(cx.listener(|this, _: &act::NextWindow, _, cx| this.step_window(1, cx)))
            .child(
                TabBar::new("stats-window-tabs")
                    .segmented()
                    .small()
                    .selected_index(self.window.ix())
                    .on_click(cx.listener(|this, ix: &usize, w, cx| {
                        this.set_window(StatsWindow::ALL[*ix], cx);
                        w.focus(&this.window_focus, cx);
                    }))
                    .children(StatsWindow::ALL.iter().map(|w| Tab::new().label(w.label()))),
            );
        let totals = last.as_ref().map(|l| {
            s::stats_totals(
                &format_size(l.net_rx_total),
                &format_size(l.net_tx_total),
                &format_size(l.blk_read_total),
                &format_size(l.blk_write_total),
            )
        });
        let pids = last.as_ref().and_then(|l| l.pids).map(s::stats_pids);
        let gaps = self.gaps();
        let info = h_flex()
            .gap_3()
            .items_center()
            .flex_wrap()
            .text_sm()
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(s::STATS_WINDOW),
            )
            .child(selector)
            .children(pids.map(|p| div().child(p)))
            .children(totals.map(|t| div().text_color(theme.muted_foreground).child(t)))
            .when(gaps > 0, |this| {
                this.child(div().text_color(theme.warning).child(s::stats_gap(gaps)))
            });

        let disk_row = h_flex()
            .gap_2()
            .items_center()
            .text_sm()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(s::STATS_DISK_USAGE),
            )
            .map(|this| match self.disk {
                Some((rw, root)) => this.child(s::stats_disk(
                    &rw.map(format_size).unwrap_or_else(|| s::NONE_VALUE.into()),
                    &root
                        .map(format_size)
                        .unwrap_or_else(|| s::NONE_VALUE.into()),
                )),
                None => this.child(
                    Button::new("stats-load-disk")
                        .small()
                        .outline()
                        .label(s::STATS_LOAD_DISK)
                        .loading(self.disk_loading)
                        .tooltip(s::CMD_STATS_DISK)
                        .on_click(dispatch::on_click(
                            &self.window_focus,
                            Box::new(act::LoadDiskUsage),
                        )),
                ),
            });

        let processes =
            (self.caps(cx).contains(Capabilities::TOP)).then(|| {
                let body: AnyElement =
                    match &self.top {
                        Some(p) if !p.titles.is_empty() => Table::new()
                            .small()
                            .child(TableHeader::new().child(TableRow::new().children(
                                p.titles.iter().map(|t| TableHead::new().child(t.clone())),
                            )))
                            .child(TableBody::new().children(p.processes.iter().map(|row| {
                                TableRow::new().children(row.iter().map(|c| {
                                    TableCell::new().child(
                                        div()
                                            .font_family(theme.mono_font_family.clone())
                                            .text_xs()
                                            .child(c.clone()),
                                    )
                                }))
                            })))
                            .into_any_element(),
                        _ if running => Spinner::new().small().into_any_element(),
                        _ => div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(s::NOT_RUNNING)
                            .into_any_element(),
                    };
                v_flex()
                    .id("stats-processes")
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(s::STATS_PROCESSES),
                    )
                    .child(body)
            });

        let empty = samples.is_empty();
        let banner: Option<AnyElement> = if let Some(e) = &self.stream_error {
            Some(
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(format!("{}: {e}", s::STATS_FAILED))
                    .into_any_element(),
            )
        } else if !running {
            Some(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(s::NOT_RUNNING)
                    .into_any_element(),
            )
        } else if empty {
            Some(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().small())
                    .child(s::STATS_WAITING)
                    .into_any_element(),
            )
        } else {
            None
        };

        v_flex()
            .id("stats-tab")
            .size_full()
            .overflow_y_scroll()
            .gap_3()
            .on_action(
                cx.listener(|this, _: &act::Window1m, _, cx| this.set_window(StatsWindow::M1, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::Window5m, _, cx| this.set_window(StatsWindow::M5, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::Window15m, _, cx| {
                    this.set_window(StatsWindow::M15, cx)
                }),
            )
            .on_action(cx.listener(Self::load_disk))
            .child(info)
            .children(banner)
            // STA-007: a stopped container shows the last buffer greyed out.
            .child(div().when(!running, |el| el.opacity(0.5)).child(cards))
            .child(disk_row)
            .children(processes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::fake::fixtures::stats_sample;

    fn samples(n: usize) -> Vec<StatsSample> {
        let t0 = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        (0..n)
            .map(|i| {
                let mut s = stats_sample(
                    t0 + time::Duration::seconds(i as i64),
                    (i % 37) as f64,
                    1_000_000 + (i as u64 % 13) * 10_000,
                );
                s.net_rx_bps = (i % 11) as f64 * 100.0;
                s.net_tx_bps = (i % 7) as f64 * 50.0;
                s
            })
            .collect()
    }

    #[test]
    fn sta_003_window_and_downsampling() {
        let all = samples(3600);
        let now = all.last().unwrap().at;
        let d = chart_data(&all, StatsWindow::M1, false, now);
        assert_eq!(d.cpu.len(), 61, "1 minute of 1 s samples, inclusive");
        let d = chart_data(&all, StatsWindow::M15, false, now);
        assert!(
            d.cpu.len() <= MAX_POINTS && d.cpu.len() > 200,
            "{}",
            d.cpu.len()
        );
        assert_eq!(d.net.len(), d.cpu.len());
        assert_eq!(d.mem_limit, Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn sta_005_nice_axis_max() {
        assert_eq!(nice_max([0.05, 0.1].into_iter(), 1.0), 1.0);
        assert_eq!(nice_max([3.4].into_iter(), 1.0), 5.0);
        assert_eq!(nice_max([47.0].into_iter(), 1.0), 100.0);
        assert_eq!(nice_max(std::iter::empty(), 1024.0), 2000.0);
    }

    #[test]
    fn sta_005_cpu_relative_to_all_cores() {
        let all = samples(10);
        let now = all.last().unwrap().at;
        let abs = chart_data(&all, StatsWindow::M1, false, now);
        let rel = chart_data(&all, StatsWindow::M1, true, now);
        assert!((rel.cpu[9].a - abs.cpu[9].a / 8.0).abs() < 1e-9);
    }

    /// STA-009: building the chart elements for 300 points × 2 series stays cheap. Measured
    /// and printed; the assert is generous so CI never flakes (target ≤ 2 ms per frame).
    #[test]
    fn sta_009_chart_build_benchmark() {
        let all = samples(3600);
        let now = all.last().unwrap().at;
        let colors = [
            gpui_kit::red(),
            gpui_kit::green(),
            gpui_kit::blue(),
            gpui_kit::yellow(),
        ];
        let runs = 50;
        let started = Instant::now();
        for _ in 0..runs {
            let d = chart_data(&all, StatsWindow::M15, false, now);
            assert!(d.net.len() <= MAX_POINTS);
            let els = build_charts(&d, colors);
            std::hint::black_box(els);
        }
        let per = started.elapsed() / runs;
        println!("STA-009: chart data + element build for 4 charts ≤300 pts: {per:?} per frame");
        assert!(per < Duration::from_millis(50), "{per:?}");
    }
}
