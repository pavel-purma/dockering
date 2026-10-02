//! Logs tab (LOG-001…009, KBD-050…053).
//!
//! Pipeline (spec 10 §3.3, LOG-007):
//! 1. a stored foreground task reads `hub.subscribe(engine, logs(id, …))` and appends chunks
//!    to a pending batch;
//! 2. the batch is flushed every 50 ms (timer task stored on the entity) or at 500 lines;
//! 3. each flush parses ANSI on `background_spawn` (`dk_core::ansi::AnsiParser`, one parser
//!    per stream so split escapes survive chunk boundaries) and appends the styled lines to
//!    a ring buffer capped at `config.logs.max_lines` (LOG-005), with one `cx.notify()`.
//!
//! The list is GPUI's virtualised `list` (variable heights so wrapping works) in tail-follow
//! mode (LOG-003); scrolling up pauses following and shows the "Jump to bottom (N new)"
//! pill. When the stream ends with the container stopped, a footer shows the exit code
//! (LOG-006); the next `start` event resumes with `since = last ts`. The buffer is UI-owned
//! and dropped with the detail page (spec 10 ownership).

use std::collections::VecDeque;
use std::ops::Range;
use std::time::Duration;

use bytes::Bytes;
use dk_core::ansi::{AnsiColor, AnsiParser, SgrStyle, StyledLine};
use dk_core::format::format_timestamp;
use dk_core::{EngineError, LogChunk, LogOpts, LogStream};
use futures::StreamExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Selectable, Sizable, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontStyle,
    FontWeight, HighlightStyle, Hsla, IntoElement, ListAlignment, ListOffset, ListState, Render,
    SharedString, StyledText, Subscription, Task, UnderlineStyle, Window, div, list, px, rgb,
};
use time::OffsetDateTime;

use super::state::{ContainerDetailState, DetailEvent};
use crate::actions::{logs, logs_nav};
use crate::keymap::ctx;
use crate::state::AppState;
use crate::strings as s;
use crate::ui::notify;

/// Flush cadence (LOG-007).
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
/// Flush early at this many pending lines (LOG-007).
pub const FLUSH_LINES: usize = 500;

/// One rendered log line.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub stream: LogStream,
    pub ts: Option<OffsetDateTime>,
    pub text: SharedString,
    /// SGR spans (byte ranges into `text`), non-plain only.
    pub spans: Vec<(Range<usize>, SgrStyle)>,
}

/// Stream lifecycle. The "Container exited (code N)" footer is derived at render time from
/// `Ended` + the container's current state (LOG-006), so a late inspect refresh still shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamState {
    Connecting,
    Live,
    /// The stream ended (the container stopped, or it wasn't running).
    Ended,
    Failed(EngineError),
}

/// Pending raw chunks of one batch.
#[derive(Default)]
struct Batch {
    chunks: Vec<LogChunk>,
    lines: usize,
}

/// ANSI parsers per stream (stdout/stderr/console are independent byte streams).
#[derive(Default)]
struct Parsers {
    stdout: AnsiParser,
    stderr: AnsiParser,
    console: AnsiParser,
}

impl Parsers {
    fn get(&mut self, s: LogStream) -> &mut AnsiParser {
        match s {
            LogStream::Stdout => &mut self.stdout,
            LogStream::Stderr => &mut self.stderr,
            LogStream::Console => &mut self.console,
        }
    }
}

/// Parses a batch (runs on `background_spawn`, LOG-007). Partial lines (and escapes split
/// across chunks, common on TTY streams) stay in the parsers until a newline arrives or the
/// stream ends (`last`), when they are flushed as their own lines.
fn parse_batch(mut parsers: Parsers, chunks: Vec<LogChunk>, last: bool) -> (Parsers, Vec<LogLine>) {
    let mut out = Vec::new();
    let mut last_ts = None;
    for c in chunks {
        last_ts = c.ts.or(last_ts);
        let lines = parsers.get(c.stream).push(&c.bytes);
        out.extend(lines.into_iter().map(|l| to_line(c.stream, c.ts, l)));
    }
    if last {
        for stream in [LogStream::Stdout, LogStream::Stderr, LogStream::Console] {
            if let Some(rest) = parsers.get(stream).flush() {
                out.push(to_line(stream, last_ts, rest));
            }
        }
    }
    (parsers, out)
}

fn to_line(stream: LogStream, ts: Option<OffsetDateTime>, l: StyledLine) -> LogLine {
    LogLine {
        stream,
        ts,
        spans: l.spans.into_iter().map(|s| (s.range, s.style)).collect(),
        text: l.text.into(),
    }
}

/// Approximate line count of a chunk (flush threshold only).
fn count_lines(b: &Bytes) -> usize {
    b.iter().filter(|c| **c == b'\n').count().max(1)
}

pub struct LogsView {
    state: Entity<ContainerDetailState>,
    focus: FocusHandle,
    search: Entity<InputState>,
    list: ListState,
    /// Ring buffer (LOG-005).
    lines: VecDeque<LogLine>,
    max_lines: usize,
    dropped: u64,
    /// Lines appended while not following (the pill count, LOG-003).
    unseen: usize,
    /// Last timestamp seen (resume with `since`, LOG-006).
    last_ts: Option<OffsetDateTime>,
    /// After a resume, chunks at or before this are duplicates (`since` has 1 s granularity).
    resume_after: Option<OffsetDateTime>,
    timestamps: bool,
    wrap: bool,
    stream_state: StreamState,
    /// Bumped on every (re)subscribe; stale stream items are dropped (NFR-005).
    generation: u64,
    stream_task: Option<Task<()>>,
    flush_task: Option<Task<()>>,
    parse_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    batch: Batch,
    parsers: Option<Parsers>,
    /// A parse is in flight (parses run one at a time, so lines stay in order).
    parsing: bool,
    /// Flush the parsers' partial lines with the next parse (the stream ended).
    final_flush: bool,
    query: String,
    /// Indices into `lines` of matching lines and the current one.
    matches: Vec<usize>,
    current_match: Option<usize>,
    /// Number of `cx.notify()` calls caused by flushes (tests, LOG-007).
    flushes: usize,
    visible: bool,
    _subs: Vec<Subscription>,
}

impl LogsView {
    pub fn new(
        state: Entity<ContainerDetailState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cfg = AppState::config(cx).logs.clone();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::LOGS_SEARCH)
                .clean_on_escape()
        });
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(gpui_kit::FollowMode::Tail);
        let subs = vec![
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe(&state, Self::on_detail_event),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            state,
            focus: cx.focus_handle().tab_stop(true),
            search,
            list,
            lines: VecDeque::new(),
            max_lines: (cfg.max_lines as usize).max(100),
            dropped: 0,
            unseen: 0,
            last_ts: None,
            resume_after: None,
            timestamps: cfg.timestamps,
            wrap: cfg.wrap,
            stream_state: StreamState::Connecting,
            generation: 0,
            stream_task: None,
            flush_task: None,
            parse_task: None,
            save_task: None,
            batch: Batch::default(),
            parsers: Some(Parsers::default()),
            parsing: false,
            final_flush: false,
            query: String::new(),
            matches: Vec::new(),
            current_match: None,
            flushes: 0,
            visible: true,
            _subs: subs,
        };
        let tail = cfg.initial_tail;
        this.subscribe(Some(tail), None, None, cx);
        this
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn lines(&self) -> &VecDeque<LogLine> {
        &self.lines
    }
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    pub fn unseen(&self) -> usize {
        self.unseen
    }
    pub fn stream_state(&self) -> &StreamState {
        &self.stream_state
    }
    pub fn flushes(&self) -> usize {
        self.flushes
    }
    pub fn timestamps(&self) -> bool {
        self.timestamps
    }
    pub fn wrap(&self) -> bool {
        self.wrap
    }
    pub fn matches(&self) -> &[usize] {
        &self.matches
    }
    pub fn current_match(&self) -> Option<usize> {
        self.current_match
    }
    pub fn is_following(&self) -> bool {
        self.list.is_following_tail()
    }
    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search
    }
    pub fn list_focus(&self) -> &FocusHandle {
        &self.focus
    }
    /// For tests: limit the ring buffer.
    pub fn set_max_lines(&mut self, n: usize) {
        self.max_lines = n.max(1);
    }

    pub fn set_visible(&mut self, visible: bool, _cx: &mut Context<Self>) {
        self.visible = visible;
    }

    // ── streaming ──────────────────────────────────────────────────────────────────────

    /// (Re)subscribes. `tail` = initial lines (LOG-001), `since` = resume point (LOG-006),
    /// `follow` overrides the running check (a `start` event arrives before inspect says so).
    fn subscribe(
        &mut self,
        tail: Option<u32>,
        since: Option<OffsetDateTime>,
        follow: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        let generation = self.generation;
        let (engine, id, running, hub) = {
            let st = self.state.read(cx);
            (
                st.engine().clone(),
                st.id().to_owned(),
                st.is_running(cx),
                AppState::hub(cx),
            )
        };
        let opts = LogOpts {
            follow: follow.unwrap_or(running),
            tail,
            since,
            timestamps: true,
        };
        self.stream_state = StreamState::Connecting;
        let mut stream = hub.subscribe(&engine, move |e| e.logs(&id, opts));
        self.stream_task = Some(cx.spawn(async move |this, cx| {
            let mut error = None;
            while let Some(item) = stream.next().await {
                let ok = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        match item {
                            Ok(chunk) => {
                                this.push_chunk(chunk, cx);
                                true
                            }
                            Err(e) => {
                                error = Some(e);
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !ok {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.stream_ended(error, cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn push_chunk(&mut self, chunk: LogChunk, cx: &mut Context<Self>) {
        if self.stream_state == StreamState::Connecting {
            self.stream_state = StreamState::Live;
        }
        if let (Some(after), Some(ts)) = (self.resume_after, chunk.ts)
            && ts <= after
        {
            return; // already shown before the restart
        }
        if let Some(ts) = chunk.ts {
            self.last_ts = Some(self.last_ts.map_or(ts, |t| t.max(ts)));
        }
        self.batch.lines += count_lines(&chunk.bytes);
        self.batch.chunks.push(chunk);
        if self.batch.lines >= FLUSH_LINES {
            self.flush_task = None;
            self.flush(cx);
        } else if self.flush_task.is_none() {
            self.flush_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FLUSH_INTERVAL).await;
                this.update(cx, |this, cx| {
                    this.flush_task = None;
                    this.flush(cx);
                })
                .ok();
            }));
        }
    }

    /// Hands the pending batch to a background parse (LOG-007). Parses run one at a time so
    /// lines stay in order and the parser state carries across batches.
    fn flush(&mut self, cx: &mut Context<Self>) {
        if (self.batch.chunks.is_empty() && !self.final_flush) || self.parsing {
            return;
        }
        let Some(parsers) = self.parsers.take() else {
            return;
        };
        let chunks = std::mem::take(&mut self.batch.chunks);
        let last = std::mem::take(&mut self.final_flush);
        self.batch.lines = 0;
        self.parsing = true;
        let generation = self.generation;
        let job = cx.background_spawn(async move { parse_batch(parsers, chunks, last) });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let (parsers, lines) = job.await;
            this.update(cx, |this, cx| {
                this.parsing = false;
                this.parsers = Some(parsers);
                if this.generation == generation {
                    this.append(lines, cx);
                }
                // More arrived (or the stream ended) while parsing.
                if (!this.batch.chunks.is_empty() || this.final_flush) && this.flush_task.is_none()
                {
                    this.flush(cx);
                }
            })
            .ok();
        }));
    }

    /// Appends parsed lines to the ring buffer and the list (one notify per batch).
    fn append(&mut self, new: Vec<LogLine>, cx: &mut Context<Self>) {
        if new.is_empty() {
            return;
        }
        let following = self.list.is_following_tail();
        let n = new.len();
        let start = self.lines.len();
        self.lines.extend(new);
        self.list.splice(start..start, n);
        // Ring cap (LOG-005): drop the head.
        let over = self.lines.len().saturating_sub(self.max_lines);
        if over > 0 {
            let current_line = self
                .current_match
                .and_then(|m| self.matches.get(m))
                .copied();
            self.lines.drain(..over);
            self.list.splice(0..over, 0);
            self.dropped += over as u64;
            self.matches = self
                .matches
                .iter()
                .filter_map(|ix| ix.checked_sub(over))
                .collect();
            self.current_match = current_line
                .and_then(|l| l.checked_sub(over))
                .and_then(|l| self.matches.iter().position(|m| *m == l));
        }
        if !following {
            self.unseen += n;
        }
        if !self.query.is_empty() {
            let from = self.lines.len() - n.min(self.lines.len());
            self.find_in(from);
        }
        self.flushes += 1;
        cx.notify();
    }

    fn stream_ended(&mut self, error: Option<EngineError>, cx: &mut Context<Self>) {
        // Drain what's pending, including a last line without a newline.
        self.flush_task = None;
        self.final_flush = true;
        self.flush(cx);
        self.stream_task = None;
        self.stream_state = match error {
            Some(e) => StreamState::Failed(e),
            None => StreamState::Ended,
        };
        cx.notify();
    }

    /// A `start`/`restart` event resumes following from the last timestamp (LOG-006).
    fn on_detail_event(
        &mut self,
        _: Entity<ContainerDetailState>,
        event: &DetailEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            DetailEvent::Engine(action)
                if matches!(action.as_str(), "start" | "restart" | "unpause") =>
            {
                if self.stream_task.is_none() {
                    self.resume(cx);
                }
            }
            DetailEvent::Engine(action) if matches!(action.as_str(), "die" | "stop" | "kill") => {
                // The stream ends on its own; refresh the exit code once inspect reloads.
                cx.notify();
            }
            DetailEvent::Removed => {
                self.generation += 1;
                self.stream_task = None;
                if matches!(
                    self.stream_state,
                    StreamState::Live | StreamState::Connecting
                ) {
                    self.stream_state = StreamState::Ended;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Follow again from the last timestamp (LOG-006). `since` has second granularity on
    /// Docker, so lines at or before the last one shown are dropped as duplicates. Without a
    /// timestamp, a fresh tail is taken.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let since = self.last_ts;
        self.resume_after = since;
        let tail = if since.is_some() {
            None
        } else {
            Some(AppState::config(cx).logs.initial_tail)
        };
        self.subscribe(tail, since, Some(true), cx);
    }

    /// The footer text (LOG-006): the stream ended and the container is stopped.
    pub fn exit_footer(&self, cx: &App) -> Option<String> {
        if self.stream_state != StreamState::Ended {
            return None;
        }
        let st = self.state.read(cx);
        let c = st.summary(cx)?;
        matches!(
            c.state,
            dk_core::ContainerState::Exited | dk_core::ContainerState::Dead
        )
        .then(|| s::container_exited(c.exit_code))
    }

    // ── search (LOG-004, KBD-050) ──────────────────────────────────────────────────────

    fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Enter / Shift+Enter are bound in `Logs > Input` (KBD-050).
        if let InputEvent::Change = event {
            self.query = self.search.read(cx).value().to_lowercase();
            self.matches.clear();
            self.current_match = None;
            if !self.query.is_empty() {
                self.find_in(0);
                self.current_match = self.matches.len().checked_sub(1);
                self.reveal_current();
            }
            cx.notify();
        }
    }

    fn find_in(&mut self, from: usize) {
        let q = self.query.clone();
        for (ix, l) in self.lines.iter().enumerate().skip(from) {
            if l.text.to_lowercase().contains(&q) {
                self.matches.push(ix);
            }
        }
    }

    fn reveal_current(&mut self) {
        if let Some(&ix) = self.current_match.and_then(|m| self.matches.get(m)) {
            self.list.scroll_to_reveal_item(ix);
            self.list.pause_following_tail();
        }
    }

    pub fn find_next(&mut self, _: &logs::FindNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = Some(match self.current_match {
            Some(m) => (m + 1) % self.matches.len(),
            None => 0,
        });
        self.reveal_current();
        cx.notify();
    }

    pub fn find_prev(&mut self, _: &logs::FindPrev, _: &mut Window, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len();
        self.current_match = Some(match self.current_match {
            Some(m) => (m + n - 1) % n,
            None => n - 1,
        });
        self.reveal_current();
        cx.notify();
    }

    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |i, cx| i.focus(window, cx));
    }

    pub fn clear_search_if_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focused = self.search.focus_handle(cx).is_focused(window);
        if focused && !self.search.read(cx).value().is_empty() {
            self.search.update(cx, |i, cx| i.set_value("", window, cx));
            return true;
        }
        false
    }

    // ── navigation (KBD-051) ───────────────────────────────────────────────────────────

    pub fn jump_bottom(&mut self, _: &logs::Bottom, _: &mut Window, cx: &mut Context<Self>) {
        self.list.set_follow_mode(gpui_kit::FollowMode::Tail);
        self.unseen = 0;
        cx.notify();
    }

    fn jump_top(&mut self, _: &logs::Top, _: &mut Window, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset::default());
        cx.notify();
    }

    fn scroll_px(&mut self, d: f32, cx: &mut Context<Self>) {
        // Scrolling up pauses following (LOG-003); reaching the end resumes it on layout.
        self.list.scroll_by(px(d));
        cx.notify();
    }

    fn line_height(&self) -> f32 {
        18.0
    }

    // ── toolbar (LOG-004, KBD-052/053) ─────────────────────────────────────────────────

    fn toggle_timestamps(
        &mut self,
        _: &logs::ToggleTimestamps,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.timestamps = !self.timestamps;
        let v = self.timestamps;
        AppState::update_config(cx, |c| c.logs.timestamps = v);
        self.list.remeasure();
        cx.notify();
    }

    fn toggle_wrap(&mut self, _: &logs::ToggleWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        let v = self.wrap;
        AppState::update_config(cx, |c| c.logs.wrap = v);
        self.list.remeasure();
        cx.notify();
    }

    /// Client-side only (LOG-004).
    pub fn clear_view(&mut self, _: &logs::ClearView, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.lines.len();
        self.lines.clear();
        self.list.splice(0..n, 0);
        self.list.set_follow_mode(gpui_kit::FollowMode::Tail);
        self.matches.clear();
        self.current_match = None;
        self.unseen = 0;
        self.dropped = 0;
        cx.notify();
    }

    fn export_text(&self) -> String {
        let mut out = String::with_capacity(self.lines.len() * 64);
        for l in &self.lines {
            if self.timestamps
                && let Some(ts) = l.ts
            {
                out.push_str(&rfc3339(ts));
                out.push(' ');
            }
            out.push_str(&l.text);
            out.push('\n');
        }
        out
    }

    fn copy_all(&mut self, _: &logs::CopyAll, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.export_text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        notify::info(window, cx, s::COPIED);
    }

    /// Native save dialog, then the write runs on the hub runtime (LOG-004).
    pub fn save(&mut self, _: &logs::Save, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .state
            .read(cx)
            .summary(cx)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "container".to_owned());
        let dir = AppState::hub(cx).paths().data_dir.clone();
        let dir = dirs_home().unwrap_or(dir);
        let prompt = cx.prompt_for_new_path(&dir, Some(&s::logs_file_name(&name)));
        let text = self.export_text();
        let hub = AppState::hub(cx);
        self.save_task = Some(cx.spawn_in(window, async move |_, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(p))) => p,
                _ => return,
            };
            let result = hub.save_file(path, Bytes::from(text)).await;
            cx.update(|window, cx| match result {
                Ok(()) => notify::success(window, cx, s::LOGS_SAVED),
                Err(e) => notify::engine_error(window, cx, s::LOGS_SAVE_FAILED, &e),
            })
            .ok();
        }));
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let has_query = !self.query.is_empty();
        let counter = has_query
            .then(|| s::logs_matches(self.current_match.map_or(0, |m| m + 1), self.matches.len()));
        let btn = |id: &'static str,
                   icon: IconName,
                   tip: &'static str,
                   action: Box<dyn gpui_kit::Action>| {
            let a = action.boxed_clone();
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip_with_action(tip, action.as_ref(), Some(ctx::LOGS))
                .on_click(move |_, w, cx| w.dispatch_action(a.boxed_clone(), cx))
        };
        let toggle =
            |id: &'static str, label: &'static str, on: bool, action: Box<dyn gpui_kit::Action>| {
                let a = action.boxed_clone();
                Button::new(id)
                    .small()
                    .outline()
                    .label(label)
                    .selected(on)
                    .tooltip_with_action(label, action.as_ref(), Some(ctx::LOGS))
                    .on_click(move |_, w, cx| w.dispatch_action(a.boxed_clone(), cx))
            };
        h_flex()
            .id("logs-toolbar")
            .gap_2()
            .items_center()
            .child(
                div().w(px(260.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(gpui_kit::component::Icon::new(IconName::Search).small()),
                ),
            )
            .when_some(counter, |this, c| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(c),
                )
            })
            .child(
                btn(
                    "logs-prev",
                    IconName::ChevronUp,
                    s::LOGS_PREV,
                    Box::new(logs::FindPrev),
                )
                .disabled(self.matches.is_empty()),
            )
            .child(
                btn(
                    "logs-next",
                    IconName::ChevronDown,
                    s::LOGS_NEXT,
                    Box::new(logs::FindNext),
                )
                .disabled(self.matches.is_empty()),
            )
            .child(div().flex_1())
            .child(toggle(
                "logs-ts",
                s::LOGS_TIMESTAMPS,
                self.timestamps,
                Box::new(logs::ToggleTimestamps),
            ))
            .child(toggle(
                "logs-wrap",
                s::LOGS_WRAP,
                self.wrap,
                Box::new(logs::ToggleWrap),
            ))
            .child(btn(
                "logs-clear",
                IconName::Delete,
                s::LOGS_CLEAR,
                Box::new(logs::ClearView),
            ))
            .child(btn(
                "logs-copy",
                IconName::Copy,
                s::LOGS_COPY_ALL,
                Box::new(logs::CopyAll),
            ))
            .child(btn(
                "logs-save",
                IconName::File,
                s::LOGS_SAVE,
                Box::new(logs::Save),
            ))
            .into_any_element()
    }

    fn render_lines(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.focus.is_focused(window);
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let stderr_bg = theme.danger.opacity(0.08);
        let stderr_fg = theme.danger;
        let match_bg = theme.warning.opacity(0.35);
        let current_bg = theme.warning.opacity(0.7);
        let dark = theme.is_dark();
        let view = cx.entity().downgrade();
        let wrap = self.wrap;
        let timestamps = self.timestamps;
        let query = self.query.clone();
        let current_line = self
            .current_match
            .and_then(|m| self.matches.get(m))
            .copied();
        let lines = list(self.list.clone(), move |ix, _window, cx| {
            let Some(view) = view.upgrade() else {
                return div().into_any_element();
            };
            let v = view.read(cx);
            let Some(line) = v.lines.get(ix) else {
                return div().into_any_element();
            };
            let mut highlights = sgr_highlights(&line.spans, dark);
            if !query.is_empty() {
                let bg = if current_line == Some(ix) {
                    current_bg
                } else {
                    match_bg
                };
                highlights = overlay_matches(&line.text, &query, highlights, bg);
            }
            let text = StyledText::new(line.text.clone()).with_highlights(highlights);
            let ts = (timestamps)
                .then(|| line.ts.map(short_ts))
                .flatten()
                .map(|t| div().flex_shrink_0().text_color(muted).child(t));
            let stderr = line.stream == LogStream::Stderr;
            h_flex()
                .id(ix)
                .w_full()
                .px_2()
                .gap_2()
                .items_start()
                .when(stderr, |el| el.bg(stderr_bg))
                .when(stderr, |el| {
                    el.child(div().w(px(2.)).h_full().flex_shrink_0().bg(stderr_fg))
                })
                .children(ts)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(fg)
                        .map(|el| {
                            if wrap {
                                el
                            } else {
                                el.whitespace_nowrap().overflow_hidden()
                            }
                        })
                        .child(text),
                )
                .into_any_element()
        })
        .size_full();
        div()
            .id("logs-lines")
            .key_context(format!("{} {}", ctx::DETAIL_HEADER, ctx::LOG_LINES).as_str())
            .track_focus(&self.focus)
            .size_full()
            .rounded(theme.radius)
            .border_1()
            .border_color(if focused { theme.ring } else { theme.border })
            .bg(theme.background)
            .font_family(mono)
            .text_xs()
            // `scroll_by`: positive = towards the end.
            .on_action(cx.listener(|this, _: &logs_nav::LineUp, _, cx| {
                let d = -this.line_height();
                this.scroll_px(d, cx)
            }))
            .on_action(cx.listener(|this, _: &logs_nav::LineDown, _, cx| {
                let d = this.line_height();
                this.scroll_px(d, cx)
            }))
            .on_action(cx.listener(|this, _: &logs_nav::PageUp, _, cx| this.scroll_px(-400.0, cx)))
            .on_action(cx.listener(|this, _: &logs_nav::PageDown, _, cx| this.scroll_px(400.0, cx)))
            .child(lines)
            .into_any_element()
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = match &self.stream_state {
            StreamState::Failed(e) => Some(format!("{}: {e}", s::LOGS_FAILED)),
            _ => self.exit_footer(cx),
        }?;
        let theme = cx.theme();
        Some(
            div()
                .id("logs-footer")
                .px_2()
                .py_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element(),
        )
    }
}

/// `2026-10-02T14:03:11.123Z`
fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| format_timestamp(t))
}

/// `14:03:11.123`
fn short_ts(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

/// SGR colour → RGB. Black on a dark theme and white on a light one would vanish, so those
/// get a readable grey.
fn ansi_color(c: AnsiColor, dark: bool) -> Hsla {
    let (r, g, b) = match c {
        AnsiColor::Indexed(0) if dark => (128, 128, 128),
        AnsiColor::Indexed(7 | 15) if !dark => (110, 110, 110),
        AnsiColor::Indexed(i) => dk_core::ansi::indexed_to_rgb(i),
        AnsiColor::Rgb(r, g, b) => (r, g, b),
    };
    rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
}

/// SGR spans as GPUI highlights (LOG-002).
pub fn sgr_highlights(
    spans: &[(Range<usize>, SgrStyle)],
    dark: bool,
) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|(r, st)| {
            let color = |c| ansi_color(c, dark);
            let (mut fg, mut bg) = (st.fg.map(color), st.bg.map(color));
            if st.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            let fade = st.dim.then_some(0.4);
            (
                r.clone(),
                HighlightStyle {
                    color: fg,
                    background_color: bg,
                    font_weight: st.bold.then_some(FontWeight::BOLD),
                    font_style: st.italic.then_some(FontStyle::Italic),
                    underline: st.underline.then(|| UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    fade_out: fade,
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// Adds search-match backgrounds on top of the SGR highlights (non-overlapping output).
fn overlay_matches(
    text: &str,
    query: &str,
    base: Vec<(Range<usize>, HighlightStyle)>,
    bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; only highlight when it doesn't.
    if lower.len() != text.len() {
        return base;
    }
    let mut hits: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(query) {
        let start = from + i;
        let end = start + query.len();
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            break;
        }
        hits.push(start..end);
        from = end.max(start + 1);
        if from >= lower.len() {
            break;
        }
    }
    if hits.is_empty() {
        return base;
    }
    // Split points from both lists, then style each segment.
    let mut cuts: Vec<usize> = Vec::new();
    for r in base.iter().map(|(r, _)| r).chain(hits.iter()) {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let seg = w[0]..w[1];
        if seg.is_empty() {
            continue;
        }
        let sgr = base
            .iter()
            .find(|(r, _)| r.start <= seg.start && seg.end <= r.end)
            .map(|(_, h)| *h);
        let hit = hits
            .iter()
            .any(|r| r.start <= seg.start && seg.end <= r.end);
        let style = match (sgr, hit) {
            (Some(h), true) => HighlightStyle {
                background_color: Some(bg),
                ..h
            },
            (Some(h), false) => h,
            (None, true) => HighlightStyle {
                background_color: Some(bg),
                ..Default::default()
            },
            (None, false) => continue,
        };
        out.push((seg, style));
    }
    out
}

impl Focusable for LogsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LogsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(cx);
        let body = self.render_lines(window, cx);
        let footer = self.render_footer(cx);
        let theme = cx.theme();
        let following = self.list.is_following_tail();
        if following {
            self.unseen = 0;
        }
        let pill = (!following && self.unseen > 0).then(|| {
            Button::new("logs-jump")
                .small()
                .primary()
                .icon(IconName::ArrowDown)
                .label(s::logs_jump(self.unseen))
                .tooltip_with_action(s::LOGS_FOLLOW, &logs::Bottom, Some(ctx::LOGS))
                .on_click(|_, w, cx| w.dispatch_action(Box::new(logs::Bottom), cx))
        });
        let empty = self.lines.is_empty();
        let connecting = self.stream_state == StreamState::Connecting;
        v_flex()
            .id("logs-view")
            .key_context(ctx::LOGS)
            .size_full()
            .gap_2()
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::jump_bottom))
            .on_action(cx.listener(Self::jump_top))
            .on_action(cx.listener(Self::toggle_timestamps))
            .on_action(cx.listener(Self::toggle_wrap))
            .on_action(cx.listener(Self::clear_view))
            .on_action(cx.listener(Self::copy_all))
            .on_action(cx.listener(Self::save))
            .child(toolbar)
            .when(self.dropped > 0, |this| {
                this.child(
                    h_flex().child(Tag::warning().small().child(s::logs_dropped(self.dropped))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(body)
                    .when(empty, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .p_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(if connecting {
                                    s::LOADING
                                } else {
                                    s::LOGS_EMPTY
                                }),
                        )
                    })
                    .when_some(pill, |this, pill| {
                        this.child(div().absolute().bottom(px(12.)).right(px(16.)).child(pill))
                    }),
            )
            .children(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(stream: LogStream, s: &str) -> LogChunk {
        LogChunk {
            stream,
            ts: None,
            bytes: Bytes::from(s.to_owned()),
        }
    }

    #[test]
    fn log_002_parse_batch_keeps_colours_and_splits_lines() {
        let (_, lines) = parse_batch(
            Parsers::default(),
            vec![
                chunk(LogStream::Stdout, "\x1b[31mred\x1b[0m plain\n"),
                chunk(LogStream::Stderr, "oops\nno newline"),
            ],
            true,
        );
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_ref()).collect();
        assert_eq!(texts, vec!["red plain", "oops", "no newline"]);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(lines[0].spans[0].0, 0..3);
        assert_eq!(lines[0].spans[0].1.fg, Some(AnsiColor::Indexed(1)));
        assert_eq!(lines[1].stream, LogStream::Stderr);
    }

    #[test]
    fn log_002_split_escape_across_chunks() {
        let (p, l1) = parse_batch(
            Parsers::default(),
            vec![chunk(LogStream::Console, "\x1b[3")],
            false,
        );
        assert!(l1.is_empty());
        let (_, l2) = parse_batch(p, vec![chunk(LogStream::Console, "2mgreen\n")], false);
        assert_eq!(l2.last().map(|l| l.text.as_ref()), Some("green"));
        assert_eq!(
            l2.last().unwrap().spans[0].1.fg,
            Some(AnsiColor::Indexed(2))
        );
    }

    #[test]
    fn log_004_match_overlay_splits_sgr_spans() {
        let base = vec![(
            0..5,
            HighlightStyle {
                color: Some(gpui_kit::red()),
                ..Default::default()
            },
        )];
        let out = overlay_matches("hello world", "lo w", base, gpui_kit::yellow());
        let ranges: Vec<Range<usize>> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, vec![0..3, 3..5, 5..7]);
        assert!(out[1].1.color.is_some() && out[1].1.background_color.is_some());
        assert!(out[2].1.color.is_none() && out[2].1.background_color.is_some());
    }
}
