//! Hub-side terminal actors (spec 10 §3.3, TRM-008). The engine's `TerminalSession` never
//! leaves the hub: an actor task owns it, pumps its output into a bounded `HubStream`
//! (backpressure, no data loss), and executes `TermCmd`s from the UI.
//!
//! Lifetime: dropping the output stream does NOT end the session (the UI may re-attach via
//! `TerminalRegistry`); `TermCmd::Close`, dropping every input sender, process exit, an
//! engine switch/removal, or hub shutdown does.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use dk_core::{EngineId, EngineResult, ExecRequest, TerminalSession};
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::bridge::{HubCall, HubStream, STREAM_CAPACITY, TermCmd, TerminalHandle};
use crate::handle::HubHandle;
use crate::hub::{HubInner, guarded, guarded_stream, lock};

/// Input commands buffered per terminal.
const INPUT_CAPACITY: usize = 64;

/// Per-engine registry of live actors' cancel tokens.
#[derive(Default)]
pub(crate) struct Registry {
    next_id: AtomicU64,
    actors: std::sync::Mutex<HashMap<u64, (EngineId, CancellationToken)>>,
}

impl Registry {
    fn register(&self, engine: &EngineId, token: CancellationToken) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&self.actors).insert(id, (engine.clone(), token));
        id
    }

    fn unregister(&self, id: u64) {
        lock(&self.actors).remove(&id);
    }

    /// TRM-008: close every terminal of `engine` (engine switch / removal).
    pub(crate) fn close_engine(&self, engine: &EngineId) {
        let mut actors = lock(&self.actors);
        actors.retain(|_, (e, token)| {
            if e == engine {
                token.cancel();
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn close_all(&self) {
        for (_, (_, token)) in lock(&self.actors).drain() {
            token.cancel();
        }
    }

    #[cfg(test)]
    pub(crate) fn count(&self, engine: &EngineId) -> usize {
        lock(&self.actors)
            .values()
            .filter(|(e, _)| e == engine)
            .count()
    }
}

pub(crate) fn open(
    h: &HubHandle,
    engine: &EngineId,
    id: &str,
    req: ExecRequest,
) -> HubCall<TerminalHandle> {
    let inner = h.inner.clone();
    let conn = match inner.conn(engine) {
        Ok(c) => c,
        Err(e) => return HubCall::ready(Err(e)),
    };
    let engine_id = engine.clone();
    let container = id.to_owned();
    let (mut tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        let exec = guarded(conn.engine.exec(&container, req));
        let session = tokio::select! {
            r = exec => r,
            _ = tx.cancellation() => return,
            _ = conn.token.cancelled() => Err(crate::hub::disconnected(&engine_id)),
        };
        let session = match session {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let handle = start_actor(&inner, &engine_id, session, conn.token.clone());
        if let Err(Ok(handle)) = tx.send(Ok(handle)) {
            // The caller is gone: close the session right away.
            let mut input = handle.input;
            let _ = input.try_send(TermCmd::Close);
        }
    });
    call
}

/// How long to wait for the exit code after closing a session.
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawns the actor owning `session`; returns the UI-side handle.
pub(crate) fn start_actor(
    inner: &Arc<HubInner>,
    engine: &EngineId,
    mut session: Box<dyn TerminalSession>,
    conn_token: CancellationToken,
) -> TerminalHandle {
    let token = conn_token.child_token();
    let session_id = inner.terminals.register(engine, token.clone());
    let (out_tx, out_token, output) = HubStream::channel(STREAM_CAPACITY);
    let (in_tx, in_rx) = mpsc::channel(INPUT_CAPACITY);
    let (exit_tx, exit) = HubCall::channel();
    let out = session.output();
    let mut pump = inner
        .handle
        .spawn(pump_output(out, out_tx, out_token, token.clone()));
    let inner2 = inner.clone();
    inner.handle.spawn(async move {
        let exited = run_actor(session, in_rx, exit_tx, token.clone()).await;
        if exited {
            // Natural exit: let the pump drain trailing output (it ends with the stream).
            let _ = tokio::time::timeout(EXIT_GRACE, &mut pump).await;
        }
        token.cancel();
        inner2.terminals.unregister(session_id);
    });
    TerminalHandle {
        session_id,
        output,
        input: in_tx,
        exit,
    }
}

/// Forwards PTY output with backpressure. If the UI dropped the output stream, output is
/// drained and discarded so the session never stalls (it stays alive, TRM-008).
async fn pump_output(
    out: dk_core::EngineStream<Bytes>,
    mut tx: mpsc::Sender<EngineResult<Bytes>>,
    consumer_gone: CancellationToken,
    session: CancellationToken,
) {
    let mut out = Box::pin(guarded_stream(out));
    loop {
        let item = tokio::select! {
            biased;
            _ = session.cancelled() => return,
            item = out.next() => item,
        };
        let Some(item) = item else { return };
        if consumer_gone.is_cancelled() {
            continue;
        }
        tokio::select! {
            biased;
            _ = session.cancelled() => return,
            _ = consumer_gone.cancelled() => {}
            _ = tx.send(item) => {}
        }
    }
}

/// Bytes of input buffered while a write/resize is still in flight. Past this the session is
/// considered wedged and further input is dropped (logged); `Close` and dropping the senders
/// are still observed, so the session can always be closed.
const PENDING_BYTES_MAX: usize = 1 << 20;

/// Input received while an op is in flight, coalesced so the queue stays short: consecutive
/// `Data` chunks are concatenated and only the latest `Resize` is kept.
#[derive(Default)]
struct Pending {
    cmds: VecDeque<TermCmd>,
    bytes: usize,
}

impl Pending {
    fn pop(&mut self) -> Option<TermCmd> {
        let cmd = self.cmds.pop_front()?;
        if let TermCmd::Data(d) = &cmd {
            self.bytes -= d.len();
        }
        Some(cmd)
    }

    /// Queues a `Data`/`Resize` command. Returns `false` if it was dropped (over budget).
    fn push(&mut self, cmd: TermCmd) -> bool {
        match cmd {
            TermCmd::Data(d) => {
                if self.bytes + d.len() > PENDING_BYTES_MAX {
                    return false;
                }
                self.bytes += d.len();
                match self.cmds.back_mut() {
                    Some(TermCmd::Data(tail)) => {
                        let mut joined = Vec::with_capacity(tail.len() + d.len());
                        joined.extend_from_slice(tail);
                        joined.extend_from_slice(&d);
                        *tail = Bytes::from(joined);
                    }
                    _ => self.cmds.push_back(TermCmd::Data(d)),
                }
            }
            resize @ TermCmd::Resize { .. } => match self.cmds.back_mut() {
                Some(tail @ TermCmd::Resize { .. }) => *tail = resize,
                _ => self.cmds.push_back(resize),
            },
            // Handled by the caller (ends the actor).
            TermCmd::Close => {}
        }
        true
    }
}

/// Executes commands until exit/close/cancel. `true` = the process exited on its own.
///
/// A `write`/`resize` that never resolves must not wedge the actor: every op is raced against
/// process exit, the cancel token (engine switch TRM-008, hub shutdown) and the input channel,
/// where `TermCmd::Close` or dropping every sender ends the session. Data/resize commands that
/// arrive meanwhile are buffered (coalesced, see [`Pending`]) and executed afterwards.
async fn run_actor(
    session: Box<dyn TerminalSession>,
    mut in_rx: mpsc::Receiver<TermCmd>,
    exit_tx: oneshot::Sender<EngineResult<Option<i64>>>,
    token: CancellationToken,
) -> bool {
    let mut wait = std::pin::pin!(guarded(session.wait()));
    let mut pending = Pending::default();
    'actor: loop {
        let cmd = match pending.pop() {
            Some(cmd) => cmd,
            None => tokio::select! {
                biased;
                r = &mut wait => {
                    let _ = exit_tx.send(r);
                    return true;
                }
                _ = token.cancelled() => break 'actor,
                cmd = in_rx.next() => match cmd {
                    Some(cmd) => cmd,
                    None => break 'actor,
                },
            },
        };
        let (what, op) = match cmd {
            TermCmd::Data(d) => ("write", session.write(d)),
            TermCmd::Resize { cols, rows } => ("resize", session.resize(cols, rows)),
            TermCmd::Close => break 'actor,
        };
        let mut op = std::pin::pin!(guarded(op));
        loop {
            tokio::select! {
                biased;
                r = &mut wait => {
                    let _ = exit_tx.send(r);
                    return true;
                }
                _ = token.cancelled() => break 'actor,
                r = &mut op => {
                    if let Err(e) = r {
                        tracing::debug!(error = %e, "terminal {what} failed");
                    }
                    break;
                }
                cmd = in_rx.next() => match cmd {
                    Some(TermCmd::Close) | None => break 'actor,
                    Some(cmd) => {
                        if !pending.push(cmd) {
                            tracing::warn!("terminal {what} stuck; input dropped");
                        }
                    }
                },
            }
        }
    }
    // `close` and the exit code are bounded too: a wedged session can't hold up shutdown.
    match tokio::time::timeout(EXIT_GRACE, guarded(session.close())).await {
        Ok(Err(e)) => tracing::debug!(error = %e, "terminal close failed"),
        Err(_) => tracing::debug!("terminal close timed out"),
        Ok(Ok(())) => {}
    }
    let code = match tokio::time::timeout(EXIT_GRACE, &mut wait).await {
        Ok(r) => r,
        Err(_) => Ok(None),
    };
    let _ = exit_tx.send(code);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trm_008_pending_input_coalesces_and_is_bounded() {
        let mut p = Pending::default();
        assert!(p.push(TermCmd::Data(Bytes::from_static(b"ab"))));
        assert!(p.push(TermCmd::Data(Bytes::from_static(b"c"))));
        assert!(p.push(TermCmd::Resize { cols: 80, rows: 24 }));
        assert!(p.push(TermCmd::Resize {
            cols: 100,
            rows: 30
        }));
        assert!(p.push(TermCmd::Data(Bytes::from_static(b"d"))));
        assert_eq!(p.cmds.len(), 3);
        assert_eq!(p.pop(), Some(TermCmd::Data(Bytes::from_static(b"abc"))));
        assert_eq!(
            p.pop(),
            Some(TermCmd::Resize {
                cols: 100,
                rows: 30
            })
        );
        assert_eq!(p.bytes, 1);
        assert!(!p.push(TermCmd::Data(Bytes::from(vec![0u8; PENDING_BYTES_MAX]))));
        assert_eq!(p.pop(), Some(TermCmd::Data(Bytes::from_static(b"d"))));
        assert_eq!(p.pop(), None);
        assert_eq!(p.bytes, 0);
    }
}
