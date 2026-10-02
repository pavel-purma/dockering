//! Shared engine event streams (spec 10 §3.3): one upstream `Engine::events()` per engine,
//! fanned out to N subscribers through a broadcast ring. Started with the first subscriber,
//! stopped when the last one leaves. Slow consumers get `Feed::Lagged` and must refetch;
//! so does everyone when the engine reports lost events (spec 20 §5.4).

use std::sync::Arc;

use dk_core::{EngineEvent, EngineId, EngineResult, EventFilter};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::bridge::{Feed, HubStream, STREAM_CAPACITY};
use crate::hub::{HubInner, disconnected, guarded_stream, lock, panic_error};

/// Ring size of the per-engine event broadcast.
pub(crate) const EVENTS_CAPACITY: usize = 256;

/// Item of the per-engine broadcast ring: already the subscriber-facing `Feed`.
type Item = EngineResult<Feed<EngineEvent>>;

pub(crate) struct Shared {
    tx: broadcast::Sender<Item>,
    subscribers: usize,
    /// Cancels the upstream task.
    token: CancellationToken,
    /// Connection generation this upstream belongs to.
    conn_generation: u64,
    /// Unique id of this upstream (guards against removing a successor).
    key: u64,
}

pub(crate) fn subscribe(inner: &Arc<HubInner>, engine: &EngineId) -> HubStream<Feed<EngineEvent>> {
    let conn = match inner.conn(engine) {
        Ok(c) => c,
        Err(e) => return HubStream::failed(e),
    };
    let (rx, key) = {
        let mut map = lock(&inner.engine_events);
        let reuse = map
            .get(engine)
            .is_some_and(|s| s.conn_generation == conn.generation && !s.token.is_cancelled());
        if !reuse {
            if let Some(old) = map.remove(engine) {
                old.token.cancel();
            }
            let (tx, _) = broadcast::channel(EVENTS_CAPACITY);
            let key = inner.next_generation();
            let token = conn.token.child_token();
            map.insert(
                engine.clone(),
                Shared {
                    tx: tx.clone(),
                    subscribers: 0,
                    token: token.clone(),
                    conn_generation: conn.generation,
                    key,
                },
            );
            inner.handle.spawn(upstream(
                inner.clone(),
                engine.clone(),
                conn.engine.clone(),
                tx,
                token,
                conn.token.clone(),
                key,
            ));
        }
        let Some(s) = map.get_mut(engine) else {
            return HubStream::failed(disconnected(engine));
        };
        s.subscribers += 1;
        (s.tx.subscribe(), s.key)
    };
    let (tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
    let guard = SubscriberGuard {
        inner: inner.clone(),
        engine: engine.clone(),
        key,
    };
    inner.handle.spawn(forward(rx, tx, token, guard));
    stream
}

/// Decrements the subscriber count when a forwarder ends; stops the upstream at zero.
struct SubscriberGuard {
    inner: Arc<HubInner>,
    engine: EngineId,
    key: u64,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        let mut map = lock(&self.inner.engine_events);
        if let Some(s) = map.get_mut(&self.engine)
            && s.key == self.key
        {
            s.subscribers = s.subscribers.saturating_sub(1);
            if s.subscribers == 0 {
                s.token.cancel();
                map.remove(&self.engine);
            }
        }
    }
}

fn remove_if(inner: &HubInner, engine: &EngineId, key: u64) {
    let mut map = lock(&inner.engine_events);
    if map.get(engine).is_some_and(|s| s.key == key) {
        map.remove(engine);
    }
}

async fn upstream(
    inner: Arc<HubInner>,
    id: EngineId,
    engine: Arc<dyn dk_core::Engine>,
    tx: broadcast::Sender<Item>,
    token: CancellationToken,
    conn_token: CancellationToken,
    key: u64,
) {
    let s = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.events(EventFilter::default())
    })) {
        Ok(s) => s,
        Err(p) => {
            let _ = tx.send(Err(panic_error(&*p)));
            remove_if(&inner, &id, key);
            return;
        }
    };
    let mut s = Box::pin(guarded_stream(s));
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                if conn_token.is_cancelled() {
                    let _ = tx.send(Err(disconnected(&id)));
                }
                break;
            }
            item = s.next() => match item {
                Some(Ok(ev)) => { let _ = tx.send(Ok(Feed::Item(ev))); }
                // Spec 20 §5.4: the engine dropped events but the stream continues. Each
                // subscriber gets `Feed::Lagged` (→ full refetch) and keeps reading.
                Some(Err(e)) if e.is_events_lost() => {
                    tracing::debug!(engine = %id, "engine reported lost events");
                    let _ = tx.send(Ok(Feed::Lagged { dropped: 0 }));
                }
                Some(Err(e)) => {
                    let _ = tx.send(Err(e));
                    break;
                }
                None => break,
            },
        }
    }
    // Dropping the map's sender closes the ring for every subscriber after they drain it.
    remove_if(&inner, &id, key);
}

async fn forward(
    mut rx: broadcast::Receiver<Item>,
    mut tx: mpsc::Sender<EngineResult<Feed<EngineEvent>>>,
    token: CancellationToken,
    _guard: SubscriberGuard,
) {
    loop {
        let (item, last) = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = rx.recv() => match r {
                Ok(Ok(feed)) => (Ok(feed), false),
                Ok(Err(e)) => (Err(e), true),
                Err(broadcast::error::RecvError::Lagged(n)) => (Ok(Feed::Lagged { dropped: n }), false),
                Err(broadcast::error::RecvError::Closed) => return,
            },
        };
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = tx.send(item) => if r.is_err() { return },
        }
        if last {
            return;
        }
    }
}
