//! Container ops, logs, stats, events, and exec (spec 21 §5, §6).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::query_parameters::{
    EventsOptionsBuilder, KillContainerOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, PruneContainersOptions, RemoveContainerOptionsBuilder,
    ResizeExecOptionsBuilder, RestartContainerOptionsBuilder, StartContainerOptions,
    StatsOptionsBuilder, StopContainerOptionsBuilder,
};
use bytes::Bytes;
use dk_core::docker_json;
use dk_core::stats::{RawStats, StatsNormalizer};
use dk_core::validate::{validate_id_or_name, validate_signal};
use dk_core::{
    ContainerAction, ContainerDetails, ContainerQuery, ContainerSummary, EngineError, EngineEvent,
    EngineResult, EngineStream, EventFilter, ExecRequest, LogChunk, LogOpts, LogStream,
    ProcessList, PruneReport, RemoveContainerOpts, ResourceKind, StatsSample, TerminalSession,
    error_stream,
};
use futures::{StreamExt, stream};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, watch};

use crate::DockerEngine;
use crate::errors::{ErrCtx, is_not_modified, map_err};

/// Poll interval for the exec exit code once output ended (spec 21 §5).
const EXEC_POLL_FAST: Duration = Duration::from_millis(200);
/// Poll interval while output is still open (only matters if output is never consumed).
const EXEC_POLL_SLOW: Duration = Duration::from_secs(2);

pub(crate) fn report(deleted: Option<Vec<String>>, space: Option<i64>) -> PruneReport {
    PruneReport {
        deleted: deleted.unwrap_or_default(),
        space_reclaimed: space.and_then(|n| u64::try_from(n).ok()).unwrap_or(0),
    }
}

fn clamp_i32(n: i64) -> i32 {
    i32::try_from(n.max(0)).unwrap_or(i32::MAX)
}

/// Split one log frame into per-line chunks (a frame may hold several lines; `\n` kept).
/// With `timestamps`, each line's leading RFC 3339 token goes to `ts` and is stripped.
pub(crate) fn split_log_frame(stream: LogStream, msg: &Bytes, timestamps: bool) -> Vec<LogChunk> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = msg.as_ref();
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| start + i + 1);
        let line = msg.slice(start..end);
        out.push(log_line(stream, line, timestamps));
        start = end;
    }
    out
}

fn log_line(stream: LogStream, line: Bytes, timestamps: bool) -> LogChunk {
    if timestamps {
        let token_end = line.iter().position(|b| *b == b' ' || *b == b'\n');
        let token = token_end.map_or(&line[..], |i| &line[..i]);
        if let Some(ts) = std::str::from_utf8(token)
            .ok()
            .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        {
            let rest = match token_end {
                Some(i) if line[i] == b' ' => line.slice(i + 1..),
                Some(i) => line.slice(i..),
                None => Bytes::new(),
            };
            return LogChunk {
                stream,
                ts: Some(ts),
                bytes: rest,
            };
        }
    }
    LogChunk {
        stream,
        ts: None,
        bytes: line,
    }
}

fn log_output_parts(o: LogOutput) -> (Option<LogStream>, Bytes) {
    match o {
        LogOutput::StdOut { message } => (Some(LogStream::Stdout), message),
        LogOutput::StdErr { message } => (Some(LogStream::Stderr), message),
        LogOutput::Console { message } => (Some(LogStream::Console), message),
        // Never expected from logs; treat as stdout.
        LogOutput::StdIn { message } => (Some(LogStream::Stdout), message),
    }
}

fn event_type(kind: ResourceKind) -> Option<&'static str> {
    match kind {
        ResourceKind::Container => Some("container"),
        ResourceKind::Image => Some("image"),
        ResourceKind::Volume => Some("volume"),
        ResourceKind::Network => Some("network"),
        ResourceKind::Daemon => Some("daemon"),
        ResourceKind::Engine | ResourceKind::Session => None,
    }
}

impl DockerEngine {
    pub(crate) async fn list_containers_impl(
        &self,
        q: ContainerQuery,
    ) -> EngineResult<Vec<ContainerSummary>> {
        let mut b = ListContainersOptionsBuilder::new().all(q.all).size(q.size);
        if !q.label_filter.is_empty() {
            let labels: Vec<String> = q
                .label_filter
                .iter()
                .map(|(k, v)| match v {
                    Some(v) => format!("{k}={v}"),
                    None => k.clone(),
                })
                .collect();
            let mut filters = HashMap::new();
            filters.insert("label".to_owned(), labels);
            b = b.filters(&filters);
        }
        let list = self
            .docker
            .list_containers(Some(b.build()))
            .await
            .map_err(|e| self.err(e))?;
        list.iter()
            .map(|c| Self::to_value(c).map(|v| docker_json::container_summary(&v)))
            .collect()
    }

    pub(crate) async fn inspect_container_impl(&self, id: &str) -> EngineResult<ContainerDetails> {
        validate_id_or_name(id)?;
        let c = self
            .docker
            .inspect_container(id, None)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, id))?;
        docker_json::container_details(&Self::to_value(&c)?)
    }

    pub(crate) async fn container_action_impl(
        &self,
        id: &str,
        action: ContainerAction,
    ) -> EngineResult<()> {
        validate_id_or_name(id)?;
        let d = &self.docker;
        let res = match &action {
            ContainerAction::Start => d.start_container(id, None::<StartContainerOptions>).await,
            ContainerAction::Stop { timeout_s } => {
                let mut b = StopContainerOptionsBuilder::new();
                if let Some(t) = timeout_s {
                    b = b.t(clamp_i32(i64::from(*t)));
                }
                d.stop_container(id, Some(b.build())).await
            }
            ContainerAction::Restart { timeout_s } => {
                let mut b = RestartContainerOptionsBuilder::new();
                if let Some(t) = timeout_s {
                    b = b.t(clamp_i32(i64::from(*t)));
                }
                d.restart_container(id, Some(b.build())).await
            }
            ContainerAction::Kill { signal } => {
                let signal = signal.as_deref().unwrap_or("SIGKILL");
                validate_signal(signal)?;
                d.kill_container(
                    id,
                    Some(KillContainerOptionsBuilder::new().signal(signal).build()),
                )
                .await
            }
            ContainerAction::Pause => d.pause_container(id).await,
            ContainerAction::Unpause => d.unpause_container(id).await,
        };
        match res {
            Ok(()) => Ok(()),
            // 304: already started/stopped (spec 21 §6).
            Err(e) if is_not_modified(&e) && !matches!(action, ContainerAction::Kill { .. }) => {
                Ok(())
            }
            Err(e) => Err(self.err_nf(e, ResourceKind::Container, id)),
        }
    }

    pub(crate) async fn remove_container_impl(
        &self,
        id: &str,
        opts: RemoveContainerOpts,
    ) -> EngineResult<()> {
        validate_id_or_name(id)?;
        let o = RemoveContainerOptionsBuilder::new()
            .force(opts.force)
            .v(opts.volumes)
            .build();
        self.docker
            .remove_container(id, Some(o))
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, id))
    }

    pub(crate) async fn prune_containers_impl(&self) -> EngineResult<PruneReport> {
        let r = self
            .docker
            .prune_containers(None::<PruneContainersOptions>)
            .await
            .map_err(|e| self.err(e))?;
        Ok(report(r.containers_deleted, r.space_reclaimed))
    }

    pub(crate) async fn top_impl(&self, id: &str) -> EngineResult<ProcessList> {
        validate_id_or_name(id)?;
        let t = self
            .docker
            .top_processes(id, None)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, id))?;
        Ok(ProcessList {
            titles: t.titles.unwrap_or_default(),
            processes: t.processes.unwrap_or_default(),
        })
    }

    pub(crate) fn logs_stream(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        if let Err(e) = validate_id_or_name(id) {
            return error_stream(e);
        }
        let mut b = LogsOptionsBuilder::new()
            .follow(opts.follow)
            .stdout(true)
            .stderr(true)
            .timestamps(opts.timestamps)
            .tail(
                &opts
                    .tail
                    .map_or_else(|| "all".to_owned(), |n| n.to_string()),
            );
        if let Some(since) = opts.since {
            b = b.since(clamp_i32(since.unix_timestamp()));
        }
        let ctx = self.ctx.clone();
        let id = id.to_owned();
        let timestamps = opts.timestamps;
        let s = self.docker.logs(&id, Some(b.build()));
        Box::pin(s.flat_map(move |item| {
            let out: Vec<EngineResult<LogChunk>> = match item {
                Ok(o) => {
                    let (stream, bytes) = log_output_parts(o);
                    match stream {
                        Some(s) => split_log_frame(s, &bytes, timestamps)
                            .into_iter()
                            .map(Ok)
                            .collect(),
                        None => Vec::new(),
                    }
                }
                Err(e) => vec![Err(map_err(e, &ctx, Some((ResourceKind::Container, &id))))],
            };
            stream::iter(out)
        }))
    }

    pub(crate) fn stats_stream(&self, id: &str) -> EngineStream<StatsSample> {
        if let Err(e) = validate_id_or_name(id) {
            return error_stream(e);
        }
        let ctx = self.ctx.clone();
        let id = id.to_owned();
        let opts = StatsOptionsBuilder::new().stream(true).build();
        let s = self.docker.stats(&id, Some(opts));
        let mut norm = StatsNormalizer::new();
        Box::pin(s.map(move |item| {
            let raw = item.map_err(|e| map_err(e, &ctx, Some((ResourceKind::Container, &id))))?;
            let v = serde_json::to_value(&raw)?;
            let raw = RawStats::from_docker_json(&v)?;
            Ok(norm.push(raw, OffsetDateTime::now_utc()))
        }))
    }

    pub(crate) fn events_stream(&self, filter: EventFilter) -> EngineStream<EngineEvent> {
        let mut b = EventsOptionsBuilder::new();
        let types: Vec<String> = filter
            .kinds
            .iter()
            .filter_map(|k| event_type(*k))
            .map(ToOwned::to_owned)
            .collect();
        if !types.is_empty() {
            let mut filters = HashMap::new();
            filters.insert("type".to_owned(), types);
            b = b.filters(&filters);
        }
        if let Some(since) = filter.since {
            b = b.since(&since.unix_timestamp().max(0).to_string());
        }
        let ctx = self.ctx.clone();
        let s = self.docker.events(Some(b.build()));
        Box::pin(s.filter_map(move |item| {
            let out = match item {
                Ok(msg) => match serde_json::to_value(&msg) {
                    Ok(v) => docker_json::engine_event(&v).map(Ok),
                    Err(e) => Some(Err(EngineError::from(e))),
                },
                Err(e) => Some(Err(map_err(e, &ctx, None))),
            };
            futures::future::ready(out)
        }))
    }

    pub(crate) async fn exec_impl(
        &self,
        id: &str,
        req: ExecRequest,
    ) -> EngineResult<Box<dyn TerminalSession>> {
        validate_id_or_name(id)?;
        if req.cmd.is_empty() {
            return Err(EngineError::Api {
                status: 400,
                message: "exec: empty command".into(),
            });
        }
        let config = CreateExecOptions::<String> {
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(req.tty),
            cmd: Some(req.cmd.clone()),
            env: (!req.env.is_empty()).then(|| req.env.clone()),
            user: req.user.clone().filter(|u| !u.is_empty()),
            working_dir: req.working_dir.clone().filter(|w| !w.is_empty()),
            ..Default::default()
        };
        let created = self
            .docker
            .create_exec(id, config)
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, id))?;
        let exec_id = created.id;
        let started = self
            .docker
            .start_exec(
                &exec_id,
                Some(StartExecOptions {
                    detach: false,
                    tty: req.tty,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(|e| self.err_nf(e, ResourceKind::Container, id))?;
        let StartExecResults::Attached { output, input } = started else {
            return Err(EngineError::protocol("exec: engine did not attach"));
        };

        let (ended_tx, ended_rx) = watch::channel(false);
        let ctx = self.ctx.clone();
        let out_ctx = ctx.clone();
        let output: EngineStream<Bytes> = Box::pin(EndSignal {
            inner: Box::pin(output.map(move |item| {
                item.map(LogOutput::into_bytes)
                    .map_err(|e| map_err(e, &out_ctx, None))
            })),
            tx: Some(ended_tx),
        });

        let session = DockerExecSession {
            docker: self.docker.clone(),
            ctx,
            exec_id,
            output: StdMutex::new(Some(output)),
            input: Mutex::new(Some(input)),
            ended: ended_rx,
            tty: req.tty,
        };
        if req.tty && req.cols > 0 && req.rows > 0 {
            // The process may not have its PTY yet; a failed initial resize isn't fatal.
            if let Err(e) = session.resize(req.cols, req.rows).await {
                tracing::debug!(error = %e, "initial exec resize failed");
            }
        }
        Ok(Box::new(session))
    }
}

/// Wraps the exec output stream and flips `tx` to `true` when it ends or is dropped.
struct EndSignal {
    inner: EngineStream<Bytes>,
    tx: Option<watch::Sender<bool>>,
}

impl futures::Stream for EndSignal {
    type Item = EngineResult<Bytes>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let r = self.inner.as_mut().poll_next(cx);
        if let std::task::Poll::Ready(None) = r
            && let Some(tx) = self.tx.take()
        {
            let _ = tx.send(true);
        }
        r
    }
}

impl Drop for EndSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(true);
        }
    }
}

struct DockerExecSession {
    docker: Docker,
    ctx: ErrCtx,
    exec_id: String,
    output: StdMutex<Option<EngineStream<Bytes>>>,
    input: Mutex<Option<Pin<Box<dyn AsyncWrite + Send>>>>,
    ended: watch::Receiver<bool>,
    tty: bool,
}

impl DockerExecSession {
    fn closed() -> EngineError {
        EngineError::Api {
            status: 0,
            message: "terminal session closed".into(),
        }
    }

    /// `Some(exit code)` once the exec process is no longer running.
    async fn poll_exit(&self) -> EngineResult<Option<Option<i64>>> {
        let r = self
            .docker
            .inspect_exec(&self.exec_id)
            .await
            .map_err(|e| map_err(e, &self.ctx, Some((ResourceKind::Session, &self.exec_id))))?;
        Ok((!r.running.unwrap_or(false)).then_some(r.exit_code))
    }
}

#[async_trait]
impl TerminalSession for DockerExecSession {
    fn output(&mut self) -> EngineStream<Bytes> {
        let taken = match self.output.get_mut() {
            Ok(slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        taken.unwrap_or_else(|| Box::pin(stream::empty()))
    }

    async fn write(&self, data: Bytes) -> EngineResult<()> {
        let mut guard = self.input.lock().await;
        let input = guard.as_mut().ok_or_else(Self::closed)?;
        let io = |e: std::io::Error| EngineError::Api {
            status: 0,
            message: format!("terminal write failed: {e}"),
        };
        input.write_all(&data).await.map_err(io)?;
        input.flush().await.map_err(io)
    }

    async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
        if !self.tty || cols == 0 || rows == 0 {
            return Ok(());
        }
        let o = ResizeExecOptionsBuilder::new()
            .w(i32::from(cols))
            .h(i32::from(rows))
            .build();
        self.docker
            .resize_exec(&self.exec_id, o)
            .await
            .map_err(|e| map_err(e, &self.ctx, Some((ResourceKind::Session, &self.exec_id))))
    }

    async fn wait(&self) -> EngineResult<Option<i64>> {
        let mut ended = self.ended.clone();
        loop {
            let output_done = *ended.borrow();
            if let Some(code) = self.poll_exit().await? {
                return Ok(code);
            }
            if output_done {
                tokio::time::sleep(EXEC_POLL_FAST).await;
            } else {
                let _ = tokio::time::timeout(EXEC_POLL_SLOW, ended.changed()).await;
            }
        }
    }

    async fn close(&self) -> EngineResult<()> {
        let mut guard = self.input.lock().await;
        if let Some(mut input) = guard.take() {
            // EOF on the process's stdin; errors (already closed) are fine.
            let _ = input.shutdown().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn log_008_splits_frames_and_parses_timestamps() {
        let msg = Bytes::from_static(
            b"2026-10-02T12:00:00.123456789Z hello\n2026-10-02T12:00:01Z world\nno-ts tail",
        );
        let chunks = split_log_frame(LogStream::Stdout, &msg, true);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks[0].ts,
            Some(datetime!(2026-10-02 12:00:00.123456789 UTC))
        );
        assert_eq!(chunks[0].bytes, Bytes::from_static(b"hello\n"));
        assert_eq!(chunks[1].bytes, Bytes::from_static(b"world\n"));
        assert_eq!(chunks[2].ts, None);
        assert_eq!(chunks[2].bytes, Bytes::from_static(b"no-ts tail"));

        let plain = split_log_frame(LogStream::Console, &Bytes::from_static(b"a\nb\n"), false);
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0].stream, LogStream::Console);
        assert_eq!(plain[1].bytes, Bytes::from_static(b"b\n"));

        let empty_line = split_log_frame(
            LogStream::Stderr,
            &Bytes::from_static(b"2026-10-02T12:00:00Z\n"),
            true,
        );
        assert_eq!(empty_line[0].bytes, Bytes::from_static(b"\n"));
        assert!(split_log_frame(LogStream::Stdout, &Bytes::new(), true).is_empty());
    }

    #[test]
    fn eng_event_type_filters() {
        assert_eq!(event_type(ResourceKind::Container), Some("container"));
        assert_eq!(event_type(ResourceKind::Session), None);
    }

    #[test]
    fn prune_report_tolerates_missing_fields() {
        assert_eq!(report(None, Some(-1)), PruneReport::default());
        let r = report(Some(vec!["a".into()]), Some(10));
        assert_eq!(r.space_reclaimed, 10);
    }
}
