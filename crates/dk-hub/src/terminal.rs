//! Hub-side terminal actors (spec 10 §3.3, TRM-008). The engine's `TerminalSession` never
//! leaves the hub: an actor task owns it, pumps its output into a bounded `HubStream`
//! (backpressure, no data loss), and executes `TermCmd`s from the UI.
//!
//! Lifetime: dropping the output stream does NOT end the session (the UI may re-attach via
//! `TerminalRegistry`); `TermCmd::Close`, dropping every input sender, process exit, an
//! engine switch/removal, or hub shutdown does.

use std::collections::HashMap;
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

/// Executes commands until exit/close/cancel. `true` = the process exited on its own.
async fn run_actor(
    session: Box<dyn TerminalSession>,
    mut in_rx: mpsc::Receiver<TermCmd>,
    exit_tx: oneshot::Sender<EngineResult<Option<i64>>>,
    token: CancellationToken,
) -> bool {
    let mut wait = std::pin::pin!(guarded(session.wait()));
    loop {
        tokio::select! {
            biased;
            r = &mut wait => {
                let _ = exit_tx.send(r);
                return true;
            }
            _ = token.cancelled() => break,
            cmd = in_rx.next() => match cmd {
                Some(TermCmd::Data(d)) => {
                    let f = guarded(session.write(d));
                    if let Err(e) = f.await {
                        tracing::debug!(error = %e, "terminal write failed");
                    }
                }
                Some(TermCmd::Resize { cols, rows }) => {
                    let f = guarded(session.resize(cols, rows));
                    if let Err(e) = f.await {
                        tracing::debug!(error = %e, "terminal resize failed");
                    }
                }
                Some(TermCmd::Close) | None => break,
            },
        }
    }
    let close = guarded(session.close());
    if let Err(e) = close.await {
        tracing::debug!(error = %e, "terminal close failed");
    }
    let code = match tokio::time::timeout(EXIT_GRACE, &mut wait).await {
        Ok(r) => r,
        Err(_) => Ok(None),
    };
    let _ = exit_tx.send(code);
    false
}
