//! Per-invocation evidence, never a shared "last error" slot (ENG-129/130).
use dk_core::{EngineError, EngineResult};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchPhase {
    NotDispatched,
    MayHaveDispatched,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportFailure {
    pub(crate) hresult: i32,
    pub(crate) phase: DispatchPhase,
}

#[derive(Default)]
struct State {
    failure: Option<TransportFailure>,
    phase: Option<DispatchPhase>,
    source_items: u64,
    producers: usize,
    connect_failure: Option<ConnectFailure>,
}

/// Connect fallback decisions must not depend on lossy public messages/hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectFailure {
    Policy,
    SelfCheckMismatch,
    Final,
}
#[derive(Clone, Default)]
pub(crate) struct Evidence(Arc<Mutex<State>>);
thread_local! { static CURRENT: RefCell<Option<Evidence>> = const { RefCell::new(None) }; }

impl Evidence {
    pub(crate) fn phase(&self) -> DispatchPhase {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase
            .unwrap_or(DispatchPhase::NotDispatched)
    }
    pub(crate) fn connect_failure(&self) -> Option<ConnectFailure> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .connect_failure
    }
    pub(crate) fn is_drained(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).producers == 0
    }
    /// Register before spawning, so a not-yet-started producer is also accounted for.
    pub(crate) fn producer(&self) -> Producer {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).producers += 1;
        Producer(self.clone())
    }
    pub(crate) async fn drain(&self) -> EngineResult<()> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if self.is_drained() { return; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.map_err(|_| EngineError::unreachable_with_hint("WSLC source cancellation has not completed", "No replacement stream was started. Retry the subscription after the source has stopped."))
    }
    #[allow(dead_code)] // T10 stream routing integration API.
    pub(crate) fn source_items(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .source_items
    }
    pub(crate) fn failure(&self) -> Option<TransportFailure> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).failure
    }
    pub(crate) fn current() -> Self {
        CURRENT.with(|c| c.borrow().clone().unwrap_or_default())
    }
    pub(crate) fn unknown_outcome<T>(&self, result: EngineResult<T>) -> EngineResult<T> {
        if let Some(f) = self.failure()
            && f.phase != DispatchPhase::NotDispatched
            && result.is_err()
        {
            Err(EngineError::unreachable_with_hint(
                format!(
                    "WSLC transport failed (0x{:08X}); operation outcome is unknown",
                    f.hresult as u32
                ),
                "The operation may have completed. Refresh before manually retrying; do not resubmit automatically.",
            ))
        } else {
            result
        }
    }
    pub(crate) fn scope<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<Evidence>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|c| *c.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some(self.clone()))));
        f()
    }
}

pub(crate) struct Producer(Evidence);
impl Drop for Producer {
    fn drop(&mut self) {
        self.0.0.lock().unwrap_or_else(|e| e.into_inner()).producers -= 1;
    }
}

pub(crate) fn phase(phase: DispatchPhase) {
    CURRENT.with(|c| {
        if let Some(e) = c.borrow().as_ref() {
            let mut s = e.0.lock().unwrap_or_else(|e| e.into_inner());
            if s.phase != Some(DispatchPhase::Completed) {
                s.phase = Some(phase);
            }
        }
    });
}

/// Construct stream under this scope so producer threads inherit independent evidence.
#[allow(dead_code)] // Router integration API; remove allowance once T10 consumes it.
pub(crate) fn capture_stream<S>(make: impl FnOnce() -> S) -> (S, Evidence) {
    let evidence = Evidence::default();
    let stream = evidence.scope(make);
    (stream, evidence)
}

pub(crate) fn record(hr: i32) {
    if hr == super::ffi::hr::WSLC_E_CONTAINER_DISABLED {
        connect_failure(ConnectFailure::Policy);
    } else if hr < 0 && !super::ffi::is_disconnect(hr) {
        connect_failure(ConnectFailure::Final);
    }
    if !super::ffi::is_disconnect(hr) {
        return;
    }
    CURRENT.with(|c| {
        if let Some(e) = c.borrow().as_ref() {
            let mut s = e.0.lock().unwrap_or_else(|e| e.into_inner());
            s.failure = Some(TransportFailure {
                hresult: hr,
                phase: s.phase.unwrap_or(DispatchPhase::NotDispatched),
            });
        }
    });
}

pub(crate) fn connect_failure(kind: ConnectFailure) {
    CURRENT.with(|c| {
        if let Some(e) = c.borrow().as_ref() {
            e.0.lock()
                .unwrap_or_else(|e| e.into_inner())
                .connect_failure = Some(kind);
        }
    });
}

pub(crate) fn source_item() {
    CURRENT.with(|c| {
        if let Some(e) = c.borrow().as_ref() {
            let mut state = e.0.lock().unwrap_or_else(|e| e.into_inner());
            state.source_items = state.source_items.saturating_add(1);
        }
    });
}

/// Wrap a delegate future; poll-local scope is propagated to each queued RPC. Concurrent calls
/// have independent evidence. Domain/protocol/cancel errors never acquire a transport tag.
pub(crate) async fn capture<T>(
    future: impl Future<Output = EngineResult<T>>,
    mutation: bool,
) -> (EngineResult<T>, Option<TransportFailure>) {
    let (result, failure, _) = capture_with_phase(future, mutation).await;
    (result, failure)
}

/// Phase is independent of the transport allowlist (e.g. E_ABORT after a completed create).
pub(crate) async fn capture_with_phase<T>(
    future: impl Future<Output = EngineResult<T>>,
    mutation: bool,
) -> (EngineResult<T>, Option<TransportFailure>, DispatchPhase) {
    let evidence = Evidence::current();
    let mut future = std::pin::pin!(future);
    let mut result =
        futures::future::poll_fn(|cx| evidence.scope(|| future.as_mut().poll(cx))).await;
    let failure = evidence.failure();
    if mutation
        && let Some(f) = failure
        && f.phase != DispatchPhase::NotDispatched
        && result.is_err()
    {
        result = Err(EngineError::unreachable_with_hint(
            format!(
                "WSLC transport failed (0x{:08X}); operation outcome is unknown",
                f.hresult as u32
            ),
            "The operation may have completed. Refresh before manually retrying; do not resubmit automatically.",
        ));
    }
    (result, failure, evidence.phase())
}

/// Typed metadata for factory activation/self-check. Policy/domain are final; only an
/// allowlisted transport fault or a recognized self-check mismatch permits Auto fallback.
#[allow(dead_code)] // factory connect owner consumes this API.
pub(crate) async fn capture_connect<T>(
    future: impl Future<Output = EngineResult<T>>,
) -> (
    EngineResult<T>,
    Option<TransportFailure>,
    Option<ConnectFailure>,
) {
    let evidence = Evidence::current();
    let mut future = std::pin::pin!(future);
    let result = futures::future::poll_fn(|cx| evidence.scope(|| future.as_mut().poll(cx))).await;
    let kind = evidence.connect_failure();
    (result, evidence.failure(), kind)
}
