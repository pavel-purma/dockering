//! Engine setup from nothing (ENG-150…154, ENG-162): offers from the factories and the
//! `run_setup` stream that registers and activates the engine it creates.

use std::panic::AssertUnwindSafe;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use dk_core::{
    EngineConfig, EngineError, EngineFactory, EngineResult, EngineState, SetupOffer, SetupProgress,
};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::bridge::{Feed, HubCall, HubEvent, HubStream, STREAM_CAPACITY};
use crate::handle::HubHandle;
use crate::hub::{HubInner, guarded_stream, lock, panic_error};
use crate::registry;

/// Per-factory bound on `setup_offers` (it may check the WSL version).
const OFFERS_TIMEOUT: Duration = Duration::from_secs(5);

fn reject(message: &str) -> EngineError {
    EngineError::Api {
        status: 0,
        message: message.into(),
    }
}

/// Offers of every factory, each with the index of the factory that made it.
async fn collect_offers(inner: &HubInner) -> Vec<(usize, SetupOffer)> {
    let futs = inner.factories.iter().enumerate().map(|(i, f)| {
        let f = f.clone();
        async move {
            let fut = AssertUnwindSafe(f.setup_offers()).catch_unwind();
            match tokio::time::timeout(OFFERS_TIMEOUT, fut).await {
                Ok(Ok(v)) => v.into_iter().map(|o| (i, o)).collect::<Vec<_>>(),
                Ok(Err(p)) => {
                    let _ = panic_error(&*p);
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!(factory = ?f.kind(), "setup offers timed out");
                    Vec::new()
                }
            }
        }
    });
    futures::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect()
}

pub(crate) fn setup_offers(h: &HubHandle) -> HubCall<Vec<SetupOffer>> {
    let inner = h.inner.clone();
    let (mut tx, call) = HubCall::channel();
    h.inner.handle.spawn(async move {
        let offers = async {
            let all = collect_offers(&inner).await;
            Ok(all.into_iter().map(|(_, o)| o).collect())
        };
        tokio::select! {
            r = offers => { let _ = tx.send(r); }
            _ = tx.cancellation() => {}
        }
    });
    call
}

/// Releases the one-setup-at-a-time flag when the producer ends (finished or cancelled).
struct RunGuard(Arc<HubInner>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.setup_running.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn run_setup(
    h: &HubHandle,
    offer_id: &str,
    choice_id: &str,
) -> HubStream<Feed<SetupProgress>> {
    let inner = h.inner.clone();
    if inner
        .setup_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return HubStream::failed(EngineError::Conflict("a setup is already running".into()));
    }
    let guard = RunGuard(inner.clone());
    let (offer_id, choice_id) = (offer_id.to_owned(), choice_id.to_owned());
    let (tx, token, stream) = HubStream::channel(STREAM_CAPACITY);
    h.inner.handle.spawn(async move {
        let _guard = guard;
        produce(inner, offer_id, choice_id, tx, token).await;
    });
    stream
}

/// Finds the factory offering `offer_id` and checks `choice_id` against that offer's choices.
async fn resolve(
    inner: &HubInner,
    offer_id: &str,
    choice_id: &str,
) -> EngineResult<Arc<dyn EngineFactory>> {
    let (fi, offer) = collect_offers(inner)
        .await
        .into_iter()
        .find(|(_, o)| o.id == offer_id)
        .ok_or_else(|| reject("that setup isn't available"))?;
    let valid = if offer.choices.is_empty() {
        choice_id.is_empty()
    } else {
        offer.choices.iter().any(|c| c.id == choice_id)
    };
    if !valid {
        return Err(reject("choose one of the offered options"));
    }
    Ok(inner.factories[fi].clone())
}

async fn produce(
    inner: Arc<HubInner>,
    offer_id: String,
    choice_id: String,
    mut tx: mpsc::Sender<EngineResult<Feed<SetupProgress>>>,
    token: CancellationToken,
) {
    let factory = tokio::select! {
        biased;
        _ = token.cancelled() => return,
        _ = inner.shutdown.cancelled() => return,
        r = resolve(&inner, &offer_id, &choice_id) => match r {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.try_send(Err(e));
                return;
            }
        },
    };
    let started =
        std::panic::catch_unwind(AssertUnwindSafe(|| factory.setup(&offer_id, &choice_id)));
    let stream = match started {
        Ok(s) => s,
        Err(p) => {
            let _ = tx.try_send(Err(panic_error(&*p)));
            return;
        }
    };
    let mut stream = pin!(guarded_stream(stream));
    loop {
        let item = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = inner.shutdown.cancelled() => return,
            item = stream.next() => item,
        };
        let out = match item {
            None => Err(EngineError::protocol("setup ended without finishing")),
            Some(Ok(SetupProgress::Done(cfg))) => register(&inner, cfg).map(SetupProgress::Done),
            Some(r) => r,
        };
        let last = !matches!(out, Ok(SetupProgress::Phase { .. }));
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            r = tx.send(out.map(Feed::Item)) => if r.is_err() { return },
        }
        if last {
            return;
        }
    }
}

/// ENG-153 step 7: registers the created engine (stored, so it survives restarts, ENG-115) and
/// makes it the active one. An engine with the same id is reused, not duplicated.
fn register(inner: &Arc<HubInner>, mut cfg: EngineConfig) -> EngineResult<EngineConfig> {
    cfg.enabled = true;
    cfg.hidden = false;
    let id = cfg.id.clone();
    let stored = {
        let mut reg = lock(&inner.reg);
        match reg.get_mut(&id) {
            Some(e) => {
                e.config.enabled = true;
                e.config.hidden = false;
                let stored = e.config.clone();
                if matches!(e.state, EngineState::Stopped | EngineState::Failed { .. }) {
                    e.state = EngineState::Disconnected;
                }
                reg.emit_status(&id);
                stored
            }
            None => {
                let f = registry::factory_for(&inner.factories, &cfg.endpoint);
                reg.insert_config(cfg.clone(), f);
                let status = reg.status(&reg.entries[reg.entries.len() - 1]);
                reg.emit(HubEvent::Added(status));
                cfg
            }
        }
    };
    inner.store_entry(stored.clone());
    inner.activate(&id, None, false)?;
    Ok(stored)
}
