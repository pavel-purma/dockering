//! Discovery, auto-select, and the connection supervisor (spec 20 §2, §6; ENG-020…025,
//! ENG-103). The active engine is supervised continuously; other enabled engines get a
//! lightweight `factory.probe()` (never a connect, never booting a stopped distro).

use std::sync::Arc;
use std::time::Duration;

use dk_core::{
    Engine, EngineConfig, EngineError, EngineFactory, EngineId, EngineInfo, EngineKind,
    EngineResult, EngineState, ProbeResult,
};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::bridge::HubEvent;
use crate::hub::{HubInner, guarded, lock, panic_error};
use crate::registry::Conn;

/// Per-factory discovery timeout (spec 20 §2).
pub(crate) const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
/// Connect timeout while auto-selecting the first engine (ENG-103).
pub(crate) const AUTOSELECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Connect timeout of the active engine.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// "Test connection" timeout (ENG-104).
pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// Background probe interval for non-active engines (ENG-020); WSLC every other tick (60 s).
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Spec 20 §4.4: stop auto-reconnecting a WSL distro after this many failures.
pub(crate) const WSL_MAX_FAILURES: u32 = 3;
pub(crate) const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// ENG-021: exponential backoff 1 s, 2 s, 4 s … capped at 30 s, plus up to 10 % jitter
/// derived from `jitter_seed` (0 = no jitter). `attempt` is 0-based.
pub fn backoff_delay(attempt: u32, jitter_seed: u64) -> Duration {
    let factor = 1u64 << attempt.min(16);
    let base_ms = (BACKOFF_BASE.as_millis() as u64)
        .saturating_mul(factor)
        .min(BACKOFF_MAX.as_millis() as u64);
    let jitter_ms = jitter_seed % (base_ms / 10 + 1);
    Duration::from_millis(base_ms + jitter_ms)
}

/// `factory.connect` + `info()`, panic-safe (NFR-030).
pub(crate) async fn connect_info(
    factory: &Arc<dyn EngineFactory>,
    cfg: &EngineConfig,
) -> EngineResult<(Arc<dyn Engine>, EngineInfo)> {
    let engine = guarded(factory.connect(cfg)).await?;
    let info = guarded(engine.info()).await?;
    Ok((engine, info))
}

async fn connect_with_timeout(
    factory: &Arc<dyn EngineFactory>,
    cfg: &EngineConfig,
    t: Duration,
) -> EngineResult<(Arc<dyn Engine>, EngineInfo)> {
    match tokio::time::timeout(t, connect_info(factory, cfg)).await {
        Ok(r) => r,
        Err(_) => Err(EngineError::Timeout(t)),
    }
}

/// Startup (spec 10 §7 step 5): discovery, then the default engine, the last engine, or
/// auto-select, then the background probe loop.
pub(crate) async fn bootstrap(inner: Arc<HubInner>) {
    if inner.discover_on_start {
        discover(&inner).await;
    }
    select_engine(&inner).await;
    probe_loop(inner).await;
}

/// ENG-103: activates the startup engine (default, else last-used), else auto-selects. Used at
/// startup and by *Rescan* when no engine is active (so a pinned default that only shows up
/// on a later scan still wins over preference order). No-op once an engine is active.
pub(crate) async fn select_engine(inner: &Arc<HubInner>) {
    if inner.active_id().is_some() {
        return;
    }
    match startup_engine(inner) {
        Some(id) => {
            let _ = inner.activate(&id, None, true);
        }
        None => autoselect(inner).await,
    }
}

/// ENG-103: the engine to open on at startup: the pinned default (ENG-116), else the last-used
/// one. Each must exist, be enabled, not hidden, and contactable; otherwise `None` and the
/// caller auto-selects. A candidate that is merely down is still returned (it shows *Failed*
/// with Retry rather than silently opening another engine).
pub(crate) fn startup_engine(inner: &HubInner) -> Option<EngineId> {
    let usable = |id: &EngineId| {
        lock(&inner.reg)
            .get(id)
            .is_some_and(|e| e.config.enabled && !e.config.hidden && e.contactable())
    };
    inner
        .default_engine()
        .filter(&usable)
        .or_else(|| inner.ui_state().last_engine.filter(&usable))
}

/// Runs every factory's discovery in parallel (3 s each) and merges the result (ENG-009).
pub(crate) async fn discover(inner: &Arc<HubInner>) {
    let futs = inner.factories.iter().enumerate().map(|(i, f)| {
        let f = f.clone();
        async move {
            let fut = AssertUnwindSafe(f.discover()).catch_unwind();
            match tokio::time::timeout(DISCOVERY_TIMEOUT, fut).await {
                Ok(Ok(v)) => v.into_iter().map(|d| (i, d)).collect::<Vec<_>>(),
                Ok(Err(p)) => {
                    let _ = panic_error(&*p);
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!(factory = ?f.kind(), "engine discovery timed out");
                    Vec::new()
                }
            }
        }
    });
    let found: Vec<_> = futures::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect();
    tracing::info!(found = found.len(), "engine discovery finished");
    let stored = lock(&inner.config).engines.entries.clone();
    let mut reg = lock(&inner.reg);
    reg.merge_discovered(found, &stored);
    reg.emit_snapshot();
}

/// ENG-103: try candidates by preference (sequentially, 3 s each); the first that connects
/// becomes active. No-op once an engine is active.
pub(crate) async fn autoselect(inner: &Arc<HubInner>) {
    let candidates: Vec<(EngineId, EngineConfig, Arc<dyn EngineFactory>)> = {
        let reg = lock(&inner.reg);
        let mut c: Vec<_> = reg
            .entries
            .iter()
            .filter(|e| {
                reg.visible(e)
                    && !e.config.hidden
                    && e.contactable()
                    && !matches!(e.state, EngineState::Stopped)
            })
            .filter_map(|e| {
                let f = inner.factories.get(e.factory?)?.clone();
                Some((e.preference, e.config.id.clone(), e.config.clone(), f))
            })
            .collect();
        c.sort_by_key(|(p, ..)| *p);
        c.into_iter().map(|(_, id, cfg, f)| (id, cfg, f)).collect()
    };
    for (id, cfg, factory) in candidates {
        if inner.active_id().is_some() || inner.shutdown.is_cancelled() {
            return;
        }
        inner.set_state(&id, EngineState::Connecting);
        match connect_with_timeout(&factory, &cfg, AUTOSELECT_TIMEOUT).await {
            Ok(pre) => {
                if inner.activate(&id, Some(pre), true).is_ok() && inner.is_active(&id) {
                    return;
                }
                inner.set_state(&id, EngineState::Disconnected);
            }
            Err(e) => {
                tracing::info!(engine = %id, error = %e, "auto-select candidate failed");
                // Don't clobber a supervisor that took over meanwhile.
                if !inner.is_active(&id) {
                    inner.set_state(
                        &id,
                        EngineState::Failed {
                            error: e,
                            retry_in_ms: None,
                        },
                    );
                }
            }
        }
    }
}

impl HubInner {
    pub(crate) fn is_active(&self, id: &EngineId) -> bool {
        self.active_id().as_ref() == Some(id)
    }
}

enum Lost {
    Cancelled,
    Failed(EngineError),
}

/// The active engine's state machine (spec 20 §6).
pub(crate) async fn supervise(
    inner: Arc<HubInner>,
    id: EngineId,
    token: CancellationToken,
    retry: Arc<Notify>,
    mut pre: Option<(Arc<dyn Engine>, EngineInfo)>,
) {
    let mut failures: u32 = 0;
    let mut had_failure = false;
    loop {
        if token.is_cancelled() {
            return;
        }
        let target = {
            let reg = lock(&inner.reg);
            reg.get(&id).map(|e| {
                (
                    e.config.clone(),
                    e.factory.and_then(|i| inner.factories.get(i).cloned()),
                    e.contactable(),
                    matches!(e.state, EngineState::Stopped),
                    e.kind(),
                )
            })
        };
        let Some((cfg, factory, contactable, stopped, kind)) = target else {
            return;
        };
        let factory = match factory {
            Some(f) if contactable && !(stopped && pre.is_none()) => f,
            // Disabled / unsupported / stopped distro (ENG-025/010/106): wait for the user.
            _ => {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = retry.notified() => continue,
                }
            }
        };

        inner.set_state(&id, EngineState::Connecting);
        let res = match pre.take() {
            Some(p) => Ok(p),
            None => tokio::select! {
                _ = token.cancelled() => return,
                r = connect_with_timeout(&factory, &cfg, CONNECT_TIMEOUT) => r,
            },
        };
        match res {
            Err(e) => {
                failures += 1;
                had_failure = true;
                tracing::info!(engine = %id, error = %e, failures, "engine connect failed");
                let give_up = kind == Some(EngineKind::WslDistro) && failures >= WSL_MAX_FAILURES;
                let delay = (!give_up).then(|| backoff_delay(failures - 1, rand::random()));
                if token.is_cancelled() {
                    return;
                }
                inner.set_state(
                    &id,
                    EngineState::Failed {
                        error: e,
                        retry_in_ms: delay.map(|d| d.as_millis() as u64),
                    },
                );
                let manual = match delay {
                    Some(d) => tokio::select! {
                        _ = token.cancelled() => return,
                        _ = tokio::time::sleep(d) => false,
                        _ = retry.notified() => true,
                    },
                    None => tokio::select! {
                        _ = token.cancelled() => return,
                        _ = retry.notified() => true,
                    },
                };
                if manual {
                    failures = 0;
                }
            }
            Ok((engine, info)) => {
                let conn_token = token.child_token();
                let gen_ = inner.next_generation();
                {
                    let mut reg = lock(&inner.reg);
                    if token.is_cancelled() {
                        return;
                    }
                    let daemon = info.daemon_id.clone();
                    if let Some(e) = reg.get_mut(&id) {
                        e.conn = Some(Conn {
                            engine: engine.clone(),
                            token: conn_token.clone(),
                            generation: gen_,
                        });
                        e.info = Some(info);
                        e.state = EngineState::Connected;
                    }
                    reg.emit_status(&id);
                    if let Some(d) = daemon {
                        reg.dedupe_daemon(&id, &d);
                    }
                    if had_failure {
                        reg.emit(HubEvent::Reconnected(id.clone()));
                    }
                }
                tracing::info!(engine = %id, "engine connected");
                let lost = ping_loop(&inner, &id, &engine, &token, &retry).await;
                {
                    let mut reg = lock(&inner.reg);
                    if let Some(e) = reg.get_mut(&id)
                        && e.conn.as_ref().is_some_and(|c| c.generation == gen_)
                    {
                        e.conn = None;
                    }
                }
                conn_token.cancel();
                match lost {
                    Lost::Cancelled => return,
                    Lost::Failed(e) => {
                        had_failure = true;
                        // The lost connection counts as the first failure (backoff grows,
                        // WSL distros give up after 3, spec 20 §4.4).
                        failures = 1;
                        tracing::warn!(engine = %id, error = %e, "engine connection lost");
                        let d = backoff_delay(0, rand::random());
                        inner.set_state(
                            &id,
                            EngineState::Failed {
                                error: e,
                                retry_in_ms: Some(d.as_millis() as u64),
                            },
                        );
                        tokio::select! {
                            _ = token.cancelled() => return,
                            _ = tokio::time::sleep(d) => {}
                            _ = retry.notified() => {}
                        }
                    }
                }
            }
        }
    }
}

async fn ping(engine: &Arc<dyn Engine>) -> EngineResult<()> {
    match tokio::time::timeout(PING_TIMEOUT, guarded(engine.ping())).await {
        Ok(r) => r,
        Err(_) => Err(EngineError::Timeout(PING_TIMEOUT)),
    }
}

/// Pings every 10 s; one failure → Degraded + immediate re-ping; two consecutive → lost.
/// Detects capability changes (transport switches) on every ping.
async fn ping_loop(
    inner: &Arc<HubInner>,
    id: &EngineId,
    engine: &Arc<dyn Engine>,
    token: &CancellationToken,
    retry: &Notify,
) -> Lost {
    let mut degraded = false;
    loop {
        if !degraded {
            tokio::select! {
                _ = token.cancelled() => return Lost::Cancelled,
                _ = tokio::time::sleep(PING_INTERVAL) => {}
                _ = retry.notified() => {}
            }
        }
        let r = tokio::select! {
            _ = token.cancelled() => return Lost::Cancelled,
            r = ping(engine) => r,
        };
        match r {
            Ok(()) => {
                let caps = std::panic::catch_unwind(AssertUnwindSafe(|| engine.capabilities()));
                let mut reg = lock(&inner.reg);
                if token.is_cancelled() {
                    return Lost::Cancelled;
                }
                let mut changed = None;
                if let (Ok(caps), Some(e)) = (caps, reg.get_mut(id))
                    && let Some(info) = e.info.as_mut()
                    && info.capabilities != caps
                {
                    info.capabilities = caps;
                    changed = Some(caps);
                }
                if let Some(capabilities) = changed {
                    reg.emit(HubEvent::CapabilitiesChanged {
                        id: id.clone(),
                        capabilities,
                    });
                    reg.emit_status(id);
                }
                if degraded {
                    degraded = false;
                    reg.set_state(id, EngineState::Connected);
                }
            }
            Err(e) if degraded => return Lost::Failed(e),
            Err(e) => {
                tracing::info!(engine = %id, error = %e, "ping failed; degraded");
                degraded = true;
                if token.is_cancelled() {
                    return Lost::Cancelled;
                }
                inner.set_state(id, EngineState::Degraded);
            }
        }
    }
}

/// Background status checks for non-active enabled engines (ENG-020/025).
async fn probe_loop(inner: Arc<HubInner>) {
    let mut tick: u64 = 0;
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(PROBE_INTERVAL) => {}
        }
        tick += 1;
        let ids: Vec<EngineId> = {
            let reg = lock(&inner.reg);
            reg.entries
                .iter()
                .filter(|e| !reg.is_active(&e.config.id) && e.contactable())
                .filter(|e| e.kind() != Some(EngineKind::Wslc) || tick.is_multiple_of(2))
                .map(|e| e.config.id.clone())
                .collect()
        };
        let futs = ids.iter().map(|id| probe_one(&inner, id));
        futures::future::join_all(futs).await;
    }
}

/// One `factory.probe()` of a non-active engine; applies the result unless it became active.
pub(crate) async fn probe_one(inner: &Arc<HubInner>, id: &EngineId) {
    let target = {
        let reg = lock(&inner.reg);
        reg.get(id)
            .filter(|e| !reg.is_active(id) && e.contactable())
            .and_then(|e| Some((e.config.clone(), inner.factories.get(e.factory?)?.clone())))
    };
    let Some((cfg, factory)) = target else { return };
    let fut = AssertUnwindSafe(factory.probe(&cfg)).catch_unwind();
    let res = match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(p)) => ProbeResult::Unreachable(panic_error(&*p)),
        Err(_) => ProbeResult::Unreachable(EngineError::Timeout(PROBE_TIMEOUT)),
    };
    let mut reg = lock(&inner.reg);
    if reg.is_active(id)
        || reg
            .get(id)
            .is_none_or(|e| e.conn.is_some() || !e.contactable())
    {
        return;
    }
    let state = match res {
        ProbeResult::Reachable => EngineState::Disconnected,
        ProbeResult::Unreachable(error) => EngineState::Failed {
            error,
            retry_in_ms: None,
        },
        ProbeResult::Stopped => EngineState::Stopped,
        ProbeResult::Unknown => return,
    };
    reg.set_state(id, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_021_backoff_doubles_and_caps_at_30s() {
        let secs: Vec<u64> = (0..8).map(|a| backoff_delay(a, 0).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff_delay(100, 0), Duration::from_secs(30));
        for seed in [1u64, 7, 99, 12345, u64::MAX] {
            for a in 0..10 {
                let base = backoff_delay(a, 0);
                let d = backoff_delay(a, seed);
                assert!(d >= base && d <= base + base / 10, "{a} {seed} {d:?}");
            }
        }
    }
}
