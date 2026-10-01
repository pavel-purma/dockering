//! UI ⇄ hub bridge types (spec 10 §3.3). Only executor-agnostic `futures` types cross the
//! boundary; the UI never names a tokio type.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use dk_core::{EngineError, EngineResult, EngineStatus, EngineId, Capabilities};
use futures::Stream;
use futures::channel::{mpsc, oneshot};
use tokio_util::sync::{CancellationToken, DropGuard};

/// Default bounded channel size for streams.
pub const STREAM_CAPACITY: usize = 256;

/// A request/response; the future resolves on any executor.
#[must_use = "a HubCall does nothing unless awaited"]
pub struct HubCall<T> {
    pub(crate) rx: oneshot::Receiver<EngineResult<T>>,
}

impl<T> HubCall<T> {
    /// A call that has already completed.
    pub fn ready(value: EngineResult<T>) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(value);
        Self { rx }
    }

    #[allow(dead_code)] // used by hub.rs once implemented
    pub(crate) fn channel() -> (oneshot::Sender<EngineResult<T>>, Self) {
        let (tx, rx) = oneshot::channel();
        (tx, Self { rx })
    }
}

impl<T> Future for HubCall<T> {
    type Output = EngineResult<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|r| r.unwrap_or(Err(EngineError::Cancelled)))
    }
}

/// A live subscription. Dropping it cancels the producer on the hub runtime.
pub struct HubStream<T> {
    pub(crate) rx: mpsc::Receiver<EngineResult<T>>,
    pub(crate) _guard: Option<DropGuard>,
}

impl<T> HubStream<T> {
    #[allow(dead_code)] // used by hub.rs once implemented
    pub(crate) fn channel(
        capacity: usize,
    ) -> (mpsc::Sender<EngineResult<T>>, CancellationToken, Self) {
        let (tx, rx) = mpsc::channel(capacity);
        let token = CancellationToken::new();
        let stream = Self {
            rx,
            _guard: Some(token.clone().drop_guard()),
        };
        (tx, token, stream)
    }

    /// A stream that yields one error and ends.
    pub fn failed(err: EngineError) -> Self {
        let (mut tx, rx) = mpsc::channel(1);
        let _ = tx.try_send(Err(err));
        Self { rx, _guard: None }
    }
}

impl<T> Stream for HubStream<T> {
    type Item = EngineResult<T>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

/// Item wrapper for "latest-state" streams (events, stats). On `Lagged` the consumer must
/// resynchronise (events → full refetch; stats → show a gap).
#[derive(Debug, Clone, PartialEq)]
pub enum Feed<T> {
    Item(T),
    Lagged { dropped: u64 },
}

/// Engine registry changes (ENG-023).
#[derive(Debug, Clone, PartialEq)]
pub enum HubEvent {
    /// Full snapshot; sent first on subscribe and after rescans.
    Snapshot(Vec<EngineStatus>),
    Added(EngineStatus),
    Removed(EngineId),
    StatusChanged(EngineStatus),
    CapabilitiesChanged { id: EngineId, capabilities: Capabilities },
    ActiveChanged(Option<EngineId>),
    /// ENG-022: the engine reconnected; stores resubscribe and refetch everything.
    Reconnected(EngineId),
}

/// Commands to a hub-side terminal actor.
#[derive(Debug, Clone, PartialEq)]
pub enum TermCmd {
    Data(Bytes),
    Resize { cols: u16, rows: u16 },
    Close,
}

/// UI side of a hub terminal actor (spec 10 §3.3). The engine's `TerminalSession` never
/// leaves the hub.
pub struct TerminalHandle {
    /// Hub-wide id, for `TerminalRegistry` (TRM-008).
    pub session_id: u64,
    /// PTY bytes; ends on process exit.
    pub output: HubStream<Bytes>,
    pub input: mpsc::Sender<TermCmd>,
    /// Resolves to the exit code when the process exits.
    pub exit: HubCall<Option<i64>>,
}
