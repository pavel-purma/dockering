//! Long-running `wslc.exe` children (logs, events, pull) and the polled stats loop.
//!
//! Every stream owns its `tokio::process::Child` (spawned with `kill_on_drop(true)`), so
//! dropping the stream kills the process. Children are spawned lazily on first poll, inside the
//! hub runtime. stdout and stderr are read concurrently (no pipe-buffer deadlock).

use std::collections::VecDeque;
use std::io;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use dk_core::stats::{RawStats, StatsNormalizer};
use dk_core::{
    EngineError, EngineEvent, EngineResult, EngineStream, EventFilter, LogChunk, LogOpts,
    LogStream, PullProgress, ResourceKind, StatsSample, error_stream,
};
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;

use super::parse::{self, CliStats};
use super::runner::{DEFAULT_TIMEOUT, ErrCtx, PULL_TIMEOUT, Runner, spawn_error};

/// Lines longer than this are emitted in pieces (a progress bar without `\n` must not stall).
const MAX_LINE: usize = 64 * 1024;
/// stderr lines kept for the final error message.
const STDERR_TAIL: usize = 8;
/// `container stats` takes ~1 s for running containers (F-10) but returns at once for
/// stopped ones; never poll faster than this.
pub(crate) const STATS_MIN_INTERVAL: Duration = Duration::from_millis(1000);

/// Split a reader into `\n`-terminated chunks (the terminator is kept). The last chunk may
/// lack it.
pub(crate) fn line_stream<R>(reader: R) -> BoxStream<'static, io::Result<Bytes>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    stream::unfold(
        (reader, BytesMut::with_capacity(8192), false),
        |(mut r, mut buf, mut eof)| async move {
            loop {
                if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line = buf.split_to(pos + 1).freeze();
                    return Some((Ok(line), (r, buf, eof)));
                }
                if eof || buf.len() >= MAX_LINE {
                    if buf.is_empty() {
                        return None;
                    }
                    let line = buf.split().freeze();
                    return Some((Ok(line), (r, buf, eof)));
                }
                buf.reserve(8192);
                match r.read_buf(&mut buf).await {
                    Ok(0) => eof = true,
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Some((Err(e), (r, buf, true))),
                }
            }
        },
    )
    .boxed()
}

pub(crate) enum Line {
    Out(Bytes),
    Err(Bytes),
}

/// A spawned child with merged, tagged stdout/stderr lines.
pub(crate) struct ChildLines {
    child: Child,
    lines: BoxStream<'static, io::Result<Line>>,
    stderr_tail: VecDeque<String>,
    mutation: bool,
    phase: super::runner::ChildPhase,
}

impl ChildLines {
    pub async fn spawn(runner: &Runner, args: &[String]) -> EngineResult<Self> {
        runner.validate_spawn().await?;
        let mut child = runner
            .command(args)
            .spawn()
            .map_err(|e| spawn_error(runner.exe(), &e))?;
        let out = child.stdout.take().ok_or_else(|| {
            if args.get(1).is_some_and(|a| a == "pull") {
                super::runner::unknown_outcome()
            } else {
                EngineError::protocol("wslc: no stdout pipe")
            }
        })?;
        let err = child.stderr.take().ok_or_else(|| {
            if args.get(1).is_some_and(|a| a == "pull") {
                super::runner::unknown_outcome()
            } else {
                EngineError::protocol("wslc: no stderr pipe")
            }
        })?;
        let lines = stream::select(
            line_stream(out).map(|r| r.map(Line::Out)),
            line_stream(err).map(|r| r.map(Line::Err)),
        )
        .boxed();
        Ok(Self {
            child,
            lines,
            stderr_tail: VecDeque::new(),
            mutation: args.get(1).is_some_and(|a| a == "pull"),
            phase: super::runner::ChildPhase::MayHaveDispatched,
        })
    }

    pub async fn next(&mut self) -> Option<io::Result<Line>> {
        let item = self.lines.next().await;
        if let Some(Ok(Line::Err(b))) = &item {
            if self.stderr_tail.len() == STDERR_TAIL {
                self.stderr_tail.pop_front();
            }
            self.stderr_tail
                .push_back(String::from_utf8_lossy(b).trim_end().to_owned());
        }
        item
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// After all output was read: `Ok(())` on exit 0, else the mapped error.
    pub async fn finish(&mut self, ctx: Option<ErrCtx<'_>>) -> EngineResult<()> {
        match self.wait().await {
            Ok(s) if s.success() => {
                self.phase = super::runner::ChildPhase::Completed;
                Ok(())
            }
            Ok(s) => Err(super::runner::child_error(
                s.code(),
                &self.stderr_tail(),
                ctx,
                self.mutation,
                self.phase,
            )),
            Err(_) if self.mutation => Err(super::runner::unknown_outcome()),
            Err(e) => Err(EngineError::protocol(format!("wslc wait failed: {e}"))),
        }
    }
}

fn io_err(e: io::Error) -> EngineError {
    EngineError::protocol(format!("wslc output read failed: {e}"))
}

fn trim_eol(b: &[u8]) -> &[u8] {
    let b = b.strip_suffix(b"\n").unwrap_or(b);
    b.strip_suffix(b"\r").unwrap_or(b)
}

// ───────────────────────────── logs ─────────────────────────────

/// `container logs` argv (spec 20 §5.5). `tail = Some(0)` is rejected by wslc 3.0.1
/// ("Invalid tail option value: 0"), so the caller handles it via `--since now`.
pub(crate) fn logs_args(id: &str, opts: &LogOpts, now: OffsetDateTime) -> Vec<String> {
    let mut a: Vec<String> = vec!["container".into(), "logs".into()];
    if opts.follow {
        a.push("--follow".into());
    }
    if opts.timestamps {
        a.push("--timestamps".into());
    }
    match opts.tail {
        Some(0) => {
            // Only new lines: start from now.
            let since = opts.since.map_or(now, |s| s.max(now));
            a.extend(["--since".into(), since.unix_timestamp().to_string()]);
        }
        Some(n) => {
            a.extend(["--tail".into(), n.to_string()]);
            if let Some(s) = opts.since {
                a.extend(["--since".into(), s.unix_timestamp().to_string()]);
            }
        }
        None => {
            if let Some(s) = opts.since {
                a.extend(["--since".into(), s.unix_timestamp().to_string()]);
            }
        }
    }
    a.push(id.to_owned());
    a
}

fn log_chunk(line: Bytes, stream: LogStream, timestamps: bool) -> LogChunk {
    if !timestamps {
        return LogChunk {
            stream,
            ts: None,
            bytes: line,
        };
    }
    let (ts, rest) = parse::split_log_timestamp(&line);
    let bytes = if ts.is_some() {
        let off = line.len() - rest.len();
        line.slice(off..)
    } else {
        line
    };
    LogChunk { stream, ts, bytes }
}

/// Stream logs. wslc separates stdout/stderr; its `inspect` has no `Config.Tty`, so TTY
/// containers are reported as `Stdout` too (Docker also sends TTY output on stdout).
pub(crate) fn logs(runner: Runner, id: String, opts: LogOpts) -> EngineStream<LogChunk> {
    if opts.tail == Some(0) && !opts.follow {
        return Box::pin(stream::empty());
    }
    let args = logs_args(&id, &opts, OffsetDateTime::now_utc());
    let out_stream = LogStream::Stdout;
    let ts = opts.timestamps;
    struct St {
        lines: Option<ChildLines>,
        runner: Runner,
        args: Vec<String>,
        id: String,
        done: bool,
    }
    Box::pin(stream::unfold(
        St {
            lines: None,
            runner,
            args,
            id,
            done: false,
        },
        move |mut st| async move {
            if st.done {
                return None;
            }
            if st.lines.is_none() {
                match ChildLines::spawn(&st.runner, &st.args).await {
                    Ok(l) => st.lines = Some(l),
                    Err(e) => {
                        st.done = true;
                        return Some((Err(e), st));
                    }
                }
            }
            let lines = st.lines.as_mut()?;
            match lines.next().await {
                Some(Ok(Line::Out(b))) => Some((Ok(log_chunk(b, out_stream, ts)), st)),
                Some(Ok(Line::Err(b))) => Some((Ok(log_chunk(b, LogStream::Stderr, ts)), st)),
                Some(Err(e)) => {
                    st.done = true;
                    Some((Err(io_err(e)), st))
                }
                None => {
                    st.done = true;
                    let r = lines
                        .finish(Some(ErrCtx::new(ResourceKind::Container, &st.id)))
                        .await;
                    match r {
                        Ok(()) => None,
                        Err(e) => Some((Err(e), st)),
                    }
                }
            }
        },
    ))
}

// ───────────────────────────── events ─────────────────────────────

pub(crate) fn events_args(filter: &EventFilter) -> Vec<String> {
    let mut a: Vec<String> = vec!["system".into(), "events".into()];
    if let Some(s) = filter.since {
        a.extend(["--since".into(), s.unix_timestamp().to_string()]);
    }
    a
}

/// `system events` (text lines, F-10), filtered by kind client-side.
pub(crate) fn events(runner: Runner, filter: EventFilter) -> EngineStream<EngineEvent> {
    let args = events_args(&filter);
    let kinds = filter.kinds;
    let fut = async move {
        let lines = match ChildLines::spawn(&runner, &args).await {
            Ok(l) => l,
            Err(e) => return error_stream(e),
        };
        Box::pin(stream::unfold((lines, false), move |(mut lines, done)| {
            let kinds = kinds.clone();
            async move {
                if done {
                    return None;
                }
                loop {
                    match lines.next().await {
                        Some(Ok(Line::Out(b))) => {
                            let text = String::from_utf8_lossy(&b);
                            match parse::event_line(&text) {
                                Some(e) if kinds.is_empty() || kinds.contains(&e.kind) => {
                                    return Some((Ok(e), (lines, false)));
                                }
                                Some(_) => {}
                                None => {
                                    if !text.trim().is_empty() {
                                        tracing::debug!(
                                            target: "dk_engine_wslc::cli",
                                            "unparsed wslc event line"
                                        );
                                    }
                                }
                            }
                        }
                        Some(Ok(Line::Err(_))) => {}
                        Some(Err(e)) => return Some((Err(io_err(e)), (lines, true))),
                        None => {
                            return match lines.finish(None).await {
                                Ok(()) => None,
                                Err(e) => Some((Err(e), (lines, true))),
                            };
                        }
                    }
                }
            }
        })) as EngineStream<EngineEvent>
    };
    Box::pin(stream::once(fut).flatten())
}

// ───────────────────────────── pull ─────────────────────────────

/// `image pull <ref>`: text lines → `Status`, then `Done { digest }` (no PULL_PROGRESS).
pub(crate) fn pull(runner: Runner, reference: String) -> EngineStream<PullProgress> {
    let fut = async move {
        let args: Vec<String> = vec!["image".into(), "pull".into(), reference.clone()];
        let lines = match ChildLines::spawn(&runner, &args).await {
            Ok(l) => l,
            Err(e) => return error_stream(e),
        };
        struct St {
            lines: ChildLines,
            reference: String,
            digest: Option<String>,
            deadline: tokio::time::Instant,
            done: bool,
        }
        Box::pin(stream::unfold(
            St {
                lines,
                reference,
                digest: None,
                deadline: tokio::time::Instant::now() + PULL_TIMEOUT,
                done: false,
            },
            |mut st| async move {
                if st.done {
                    return None;
                }
                loop {
                    let next = tokio::time::timeout_at(st.deadline, st.lines.next()).await;
                    let Ok(next) = next else {
                        // The child is killed when the stream state is dropped (kill_on_drop).
                        st.done = true;
                        return Some((Err(super::runner::unknown_outcome()), st));
                    };
                    match next {
                        Some(Ok(Line::Out(b))) => {
                            let text = String::from_utf8_lossy(trim_eol(&b)).into_owned();
                            if let Some((p, d)) = parse::pull_line(&text) {
                                if d.is_some() {
                                    st.digest = d;
                                }
                                return Some((Ok(p), st));
                            }
                        }
                        Some(Ok(Line::Err(_))) => {}
                        Some(Err(_e)) => {
                            st.done = true;
                            return Some((Err(super::runner::unknown_outcome()), st));
                        }
                        None => {
                            st.done = true;
                            let reference = st.reference.clone();
                            let r = st
                                .lines
                                .finish(Some(ErrCtx::new(ResourceKind::Image, &reference)))
                                .await;
                            return match r {
                                Ok(()) => {
                                    let digest = st.digest.take();
                                    Some((Ok(PullProgress::Done { digest }), st))
                                }
                                Err(e) => Some((Err(e), st)),
                            };
                        }
                    }
                }
            },
        )) as EngineStream<PullProgress>
    };
    Box::pin(stream::once(fut).flatten())
}

// ───────────────────────────── stats ─────────────────────────────

/// Rate from consecutive cumulative totals; 0 for the first sample or a counter reset.
fn rate(cur: u64, prev: Option<u64>, dt: f64) -> f64 {
    match prev {
        Some(p) if dt > 0.0 && cur >= p => (cur - p) as f64 / dt,
        _ => 0.0,
    }
}

/// Synthesize a sample from the CLI's pre-formatted strings. CPU % is already in Docker CLI
/// semantics; rates come from consecutive totals (coarse: totals are human-rounded).
pub(crate) fn sample_from_cli(
    s: &CliStats,
    prev: Option<&(CliStats, Instant)>,
    now: Instant,
    at: OffsetDateTime,
    online_cpus: u32,
) -> StatsSample {
    let dt = prev.map_or(0.0, |(_, t)| {
        now.saturating_duration_since(*t).as_secs_f64()
    });
    let p = prev.map(|(p, _)| p);
    StatsSample {
        at,
        cpu_percent: s.cpu_percent,
        online_cpus,
        mem_used: s.mem_used,
        mem_limit: s.mem_limit,
        net_rx_bps: rate(s.net_rx, p.map(|p| p.net_rx), dt),
        net_tx_bps: rate(s.net_tx, p.map(|p| p.net_tx), dt),
        net_rx_total: s.net_rx,
        net_tx_total: s.net_tx,
        blk_read_bps: rate(s.blk_read, p.map(|p| p.blk_read), dt),
        blk_write_bps: rate(s.blk_write, p.map(|p| p.blk_write), dt),
        blk_read_total: s.blk_read,
        blk_write_total: s.blk_write,
        pids: s.pids,
    }
}

/// CPUs visible to the WSLC VM. wslc 3.0.1 doesn't report it; the utility VM gets all host
/// processors by default.
pub(crate) fn default_online_cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// Poll `container stats <id> --format json` back-to-back (spec 20 §5.5, F-10). Ends on the
/// first error.
pub(crate) fn stats(runner: Runner, id: String, online_cpus: u32) -> EngineStream<StatsSample> {
    struct St {
        runner: Runner,
        id: String,
        online_cpus: u32,
        last_poll: Option<Instant>,
        prev: Option<(CliStats, Instant)>,
        normalizer: StatsNormalizer,
        done: bool,
    }
    let st = St {
        runner,
        id,
        online_cpus,
        last_poll: None,
        prev: None,
        normalizer: StatsNormalizer::new(),
        done: false,
    };
    Box::pin(stream::unfold(st, |mut st| async move {
        if st.done {
            return None;
        }
        if let Some(last) = st.last_poll {
            let el = last.elapsed();
            if el < STATS_MIN_INTERVAL {
                tokio::time::sleep(STATS_MIN_INTERVAL - el).await;
            }
        }
        st.last_poll = Some(Instant::now());
        let args = ["container", "stats", st.id.as_str(), "--format", "json"];
        let out = st
            .runner
            .run(
                &args,
                DEFAULT_TIMEOUT,
                Some(ErrCtx::new(ResourceKind::Container, &st.id)),
            )
            .await
            .and_then(|o| parse::json_list(&o.stdout));
        let rows = match out {
            Ok(r) => r,
            Err(e) => {
                st.done = true;
                return Some((Err(e), st));
            }
        };
        let Some(v) = rows.first() else {
            st.done = true;
            let e = EngineError::not_found(ResourceKind::Container, st.id.clone());
            return Some((Err(e), st));
        };
        let now = Instant::now();
        let at = OffsetDateTime::now_utc();
        let sample = if parse::is_docker_stats(v) {
            match RawStats::from_docker_json(v) {
                Ok(raw) => st.normalizer.push(raw, at),
                Err(e) => {
                    st.done = true;
                    return Some((Err(e), st));
                }
            }
        } else {
            let cur = parse::cli_stats(v);
            let s = sample_from_cli(&cur, st.prev.as_ref(), now, at, st.online_cpus);
            st.prev = Some((cur, now));
            s
        };
        Some((Ok(sample), st))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[tokio::test]
    async fn lines_split_and_keep_partial_tail() {
        let data: &[u8] = b"a\r\nbb\n\nlast";
        let v: Vec<_> = line_stream(data)
            .map(|r| r.map(|b| b.to_vec()))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()
            .expect("lines");
        assert_eq!(
            v,
            [
                b"a\r\n".to_vec(),
                b"bb\n".to_vec(),
                b"\n".to_vec(),
                b"last".to_vec()
            ]
        );
        let long = vec![b'x'; MAX_LINE + 10];
        let n = line_stream(std::io::Cursor::new(long)).count().await;
        assert_eq!(n, 2);
    }

    #[test]
    fn log_args() {
        let now = datetime!(2026-10-02 00:00 UTC);
        let o = LogOpts::default();
        assert_eq!(
            logs_args("web", &o, now),
            [
                "container",
                "logs",
                "--follow",
                "--timestamps",
                "--tail",
                "1000",
                "web"
            ]
        );
        let o = LogOpts {
            follow: false,
            tail: None,
            since: Some(datetime!(2026-10-01 00:00 UTC)),
            timestamps: false,
        };
        assert_eq!(
            logs_args("web", &o, now),
            ["container", "logs", "--since", "1790812800", "web"]
        );
        let o = LogOpts {
            tail: Some(0),
            ..LogOpts::default()
        };
        assert_eq!(
            logs_args("web", &o, now),
            [
                "container",
                "logs",
                "--follow",
                "--timestamps",
                "--since",
                "1790899200",
                "web"
            ]
        );
    }

    #[test]
    fn log_chunks_strip_timestamps() {
        let c = log_chunk(
            Bytes::from_static(b"2026-10-01T23:16:32.926967408Z out-line\n"),
            LogStream::Stdout,
            true,
        );
        assert_eq!(c.ts, Some(datetime!(2026-10-01 23:16:32.926967408 UTC)));
        assert_eq!(&c.bytes[..], b"out-line\n");
        let c = log_chunk(Bytes::from_static(b"plain\n"), LogStream::Stderr, true);
        assert_eq!((c.ts, &c.bytes[..]), (None, &b"plain\n"[..]));
        let c = log_chunk(
            Bytes::from_static(b"2026-10-01T23:16:32Z x\n"),
            LogStream::Console,
            false,
        );
        assert_eq!(
            (c.ts, &c.bytes[..]),
            (None, &b"2026-10-01T23:16:32Z x\n"[..])
        );
    }

    #[test]
    fn cli_stats_rates() {
        let t0 = Instant::now();
        let at = datetime!(2026-10-02 00:00 UTC);
        let a = CliStats {
            cpu_percent: 12.5,
            mem_used: 100,
            mem_limit: 1000,
            net_rx: 1000,
            net_tx: 0,
            blk_read: 0,
            blk_write: 4100,
            pids: Some(3),
        };
        let s1 = sample_from_cli(&a, None, t0, at, 4);
        assert_eq!(
            (s1.net_rx_bps, s1.cpu_percent, s1.online_cpus),
            (0.0, 12.5, 4)
        );
        let b = CliStats {
            net_rx: 3000,
            blk_write: 4000,
            ..a.clone()
        };
        let s2 = sample_from_cli(&b, Some(&(a, t0)), t0 + Duration::from_secs(2), at, 4);
        assert_eq!(s2.net_rx_bps, 1000.0);
        assert_eq!(
            s2.blk_write_bps, 0.0,
            "counter went backwards (rounding) → 0"
        );
        assert_eq!((s2.net_rx_total, s2.pids), (3000, Some(3)));
    }

    #[test]
    fn event_args() {
        assert_eq!(events_args(&EventFilter::default()), ["system", "events"]);
        let f = EventFilter {
            kinds: vec![],
            since: Some(datetime!(2026-10-01 00:00 UTC)),
        };
        assert_eq!(
            events_args(&f),
            ["system", "events", "--since", "1790812800"]
        );
    }
}
