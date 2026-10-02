//! StatsService (STA-002/003/006): one engine stream per (engine, container) shared by N
//! subscribers; ring buffers of the history window live here. A new subscriber first receives
//! the buffered history, then live samples. The upstream stops 5 s after the last subscriber
//! leaves; the buffer is evicted after `history_minutes` without subscribers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dk_core::stats::StatsRing;
use dk_core::{Engine, EngineId, EngineResult, StatsSample};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::bridge::{Feed, HubStream, STREAM_CAPACITY};
use crate::handle::HubHandle;
use crate::hub::{HubInner, disconnected, guarded_stream, lock, panic_error};

/// The upstream keeps running this long after the last subscriber leaves (tab flips).
pub(crate) const LINGER: Duration = Duration::from_secs(5);
/// Live ring per key; slow consumers see `Feed::Lagged`.
pub(crate) const LIVE_CAPACITY: usize = 64;
/// STA-003: at most 3,600 samples.
pub(crate) const MAX_SAMPLES: usize = 3600;

type Key = (EngineId, String);

struct Upstream {
    tx: broadcast::Sender<EngineResult<StatsSample>>,
    token: CancellationToken,
    conn_generation: u64,
}

struct Entry {
    ring: StatsRing,
    upstream: Option<Upstream>,
    subscribers: usize,
    /// Bumped on every subscribe/unsubscribe; linger/evict timers check it.
    epoch: u64,
}

#[derive(Default)]
pub(crate) struct State {
    map: std::sync::Mutex<HashMap<Key, Entry>>,
}

/// Ring capacity for the configured history window (STA-003).
pub(crate) fn ring_capacity(history_minutes: u32) -> usize {
    (history_minutes as usize)
        .saturating_mul(60)
        .clamp(1, MAX_SAMPLES)
}

pub(crate) fn subscribe(
    h: &HubHandle,
    engine: &EngineId,
    id: &str,
) -> HubStream<Feed<StatsSample>> {
    subscribe_inner(&h.inner, engine, id)
}

pub(crate) fn subscribe_inner(
    inner: &Arc<HubInner>,
    engine: &EngineId,
    id: &str,
) -> HubStream<Feed<StatsSample>> {
    let conn = match inner.conn(engine) {
        Ok(c) => c,
        Err(e) => return HubStream::failed(e),
    };
    let history_minutes = lock(&inner.config).stats.history_minutes;
    let key: Key = (engine.clone(), id.to_owned());
    let (history, rx) = {
        let mut map = lock(&inner.stats.map);
        let entry = map.entry(key.clone()).or_insert_with(|| Entry {
            ring: StatsRing::new(ring_capacity(history_minutes)),
            upstream: None,
            subscribers: 0,
            epoch: 0,
        });
        let live = entry
            .upstream
            .as_ref()
            .is_some_and(|u| u.conn_generation == conn.generation && !u.token.is_cancelled());
        if !live {
            if let Some(u) = entry.upstream.take() {
                u.token.cancel();
            }
            let (tx, _) = broadcast::channel(LIVE_CAPACITY);
            let token = conn.token.child_token();
            entry.upstream = Some(Upstream {
                tx: tx.clone(),
                token: token.clone(),
                conn_generation: conn.generation,
            });
            inner.handle.spawn(upstream(
                inner.clone(),
                key.clone(),
                conn.engine.clone(),
                tx,
                token,
                conn.token.clone(),
            ));
        }
        entry.subscribers += 1;
        entry.epoch += 1;
        let rx = entry.upstream.as_ref().map(|u| u.tx.subscribe());
        (entry.ring.to_vec(), rx)
    };
    let Some(rx) = rx else {
        return HubStream::failed(disconnected(engine));
    };
    let (tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
    let guard = Guard {
        inner: inner.clone(),
        key,
    };
    inner.handle.spawn(forward(history, rx, tx, token, guard));
    stream
}

async fn upstream(
    inner: Arc<HubInner>,
    key: Key,
    engine: Arc<dyn Engine>,
    tx: broadcast::Sender<EngineResult<StatsSample>>,
    token: CancellationToken,
    conn_token: CancellationToken,
) {
    let s = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.stats(&key.1)));
    let mut s = match s {
        Ok(s) => Box::pin(guarded_stream(s)),
        Err(p) => {
            finish(&inner, &key, &tx, Some(Err(panic_error(&*p))));
            return;
        }
    };
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                let err = conn_token.is_cancelled().then(|| Err(disconnected(&key.0)));
                finish(&inner, &key, &tx, err);
                return;
            }
            item = s.next() => match item {
                Some(Ok(sample)) => {
                    // Push + broadcast under the map lock: a concurrent subscriber sees each
                    // sample exactly once (either in its history or live).
                    let mut map = lock(&inner.stats.map);
                    if let Some(e) = map.get_mut(&key) {
                        e.ring.push(sample.clone());
                    }
                    let _ = tx.send(Ok(sample));
                }
                Some(Err(e)) => {
                    finish(&inner, &key, &tx, Some(Err(e)));
                    return;
                }
                None => {
                    finish(&inner, &key, &tx, None);
                    return;
                }
            },
        }
    }
}

/// Ends an upstream: optionally broadcasts a final error and detaches it from the entry so
/// the next subscriber starts a fresh one. The buffer is kept.
fn finish(
    inner: &HubInner,
    key: &Key,
    tx: &broadcast::Sender<EngineResult<StatsSample>>,
    last: Option<EngineResult<StatsSample>>,
) {
    let mut map = lock(&inner.stats.map);
    if let Some(item) = last {
        let _ = tx.send(item);
    }
    if let Some(e) = map.get_mut(key)
        && e.upstream.as_ref().is_some_and(|u| u.tx.same_channel(tx))
    {
        e.upstream = None;
    }
}

struct Guard {
    inner: Arc<HubInner>,
    key: Key,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let epoch = {
            let mut map = lock(&self.inner.stats.map);
            let Some(e) = map.get_mut(&self.key) else {
                return;
            };
            e.subscribers = e.subscribers.saturating_sub(1);
            e.epoch += 1;
            if e.subscribers > 0 {
                return;
            }
            e.epoch
        };
        let history = Duration::from_secs(
            u64::from(lock(&self.inner.config).stats.history_minutes).max(1) * 60,
        );
        let inner = self.inner.clone();
        let key = self.key.clone();
        let shutdown = inner.shutdown.clone();
        self.inner.handle.spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(LINGER) => {}
            }
            {
                let mut map = lock(&inner.stats.map);
                let Some(e) = map.get_mut(&key) else { return };
                if e.epoch != epoch {
                    return;
                }
                if let Some(u) = e.upstream.take() {
                    u.token.cancel();
                }
            }
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(history.saturating_sub(LINGER)) => {}
            }
            let mut map = lock(&inner.stats.map);
            if map.get(&key).is_some_and(|e| e.epoch == epoch) {
                map.remove(&key);
            }
        });
    }
}

async fn forward(
    history: Vec<StatsSample>,
    mut rx: broadcast::Receiver<EngineResult<StatsSample>>,
    mut tx: mpsc::Sender<EngineResult<Feed<StatsSample>>>,
    token: CancellationToken,
    _guard: Guard,
) {
    for s in history {
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = tx.send(Ok(Feed::Item(s))) => if r.is_err() { return },
        }
    }
    loop {
        let (item, last): (EngineResult<Feed<StatsSample>>, bool) = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = rx.recv() => match r {
                Ok(Ok(s)) => (Ok(Feed::Item(s)), false),
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

/// For tests: (subscribers, buffered samples, upstream running).
#[cfg(test)]
pub(crate) fn debug_entry(
    inner: &HubInner,
    engine: &EngineId,
    id: &str,
) -> Option<(usize, usize, bool)> {
    let map = lock(&inner.stats.map);
    map.get(&(engine.clone(), id.to_owned()))
        .map(|e| (e.subscribers, e.ring.len(), e.upstream.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sta_003_ring_capacity_is_window_capped_at_3600() {
        assert_eq!(ring_capacity(15), 900);
        assert_eq!(ring_capacity(60), 3600);
        assert_eq!(ring_capacity(600), 3600);
        assert_eq!(ring_capacity(0), 1);
    }
}
