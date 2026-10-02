use std::collections::BTreeSet;
use std::time::Duration;

use dk_core::{
    Capabilities, ContainerQuery, ContainerSummary, EngineError, EngineEvent, EngineId, EngineInfo,
    ImageSummary, NetworkSummary, ResourceKind, VolumeSummary,
};
use dk_hub::{Feed, HubHandle};
use futures::StreamExt;
use gpui_kit::{Context, EventEmitter, Task};

use super::Resource;

/// Debounce between an engine event and the refetch (spec 10 §4.2, CON-030).
pub const EVENT_DEBOUNCE: Duration = Duration::from_millis(150);

/// The four list collections an engine store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Collection {
    Containers,
    Images,
    Volumes,
    Networks,
}

impl Collection {
    pub const ALL: [Collection; 4] = [
        Collection::Containers,
        Collection::Images,
        Collection::Volumes,
        Collection::Networks,
    ];

    fn ix(self) -> usize {
        self as usize
    }

    /// Which collections an engine event invalidates. Container events also refetch images
    /// (the "in use" flag, spec 10 §4.2).
    pub fn affected_by(event: &EngineEvent) -> Vec<Collection> {
        match event.kind {
            ResourceKind::Container => vec![Collection::Containers, Collection::Images],
            ResourceKind::Image => vec![Collection::Images],
            ResourceKind::Volume => vec![Collection::Volumes],
            ResourceKind::Network => vec![Collection::Networks],
            _ => Collection::ALL.to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineStoreEvent {
    /// A collection's resource changed (load started, data arrived, or failed).
    Changed(Collection),
    InfoChanged,
}

/// How the store learns about changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveMode {
    /// Not subscribed yet (engine not connected).
    Idle,
    Events,
    /// Events unavailable (no capability, stream error/end): poll (spec 10 §4.2 step 3).
    Polling,
}

/// Per-active-engine data (spec 10 §4.1). Dropped on engine switch, which cancels every
/// subscription and in-flight fetch (spec 10 §4.3).
pub struct EngineStore {
    hub: HubHandle,
    engine: EngineId,
    pub containers: Resource<Vec<ContainerSummary>>,
    pub images: Resource<Vec<ImageSummary>>,
    pub volumes: Resource<Vec<VolumeSummary>>,
    pub networks: Resource<Vec<NetworkSummary>>,
    pub info: Resource<EngineInfo>,
    /// Revision per collection (+ info at index 4); stale fetch results are dropped.
    revisions: [u64; 5],
    fetches: [Option<Task<()>>; 5],
    pending: BTreeSet<Collection>,
    debounce: Option<Task<()>>,
    live: Option<Task<()>>,
    live_mode: LiveMode,
    polling_interval: Duration,
    /// True once a fetch succeeded since the last (re)connect.
    connected_once: bool,
}

impl EventEmitter<EngineStoreEvent> for EngineStore {}

impl EngineStore {
    /// Creates the store and immediately loads everything (if the engine is usable).
    pub fn new(
        hub: HubHandle,
        engine: EngineId,
        polling_interval_s: u32,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            hub,
            engine,
            containers: Resource::Idle,
            images: Resource::Idle,
            volumes: Resource::Idle,
            networks: Resource::Idle,
            info: Resource::Idle,
            revisions: [0; 5],
            fetches: Default::default(),
            pending: BTreeSet::new(),
            debounce: None,
            live: None,
            live_mode: LiveMode::Idle,
            polling_interval: Duration::from_secs(polling_interval_s.max(1) as u64),
            connected_once: false,
        };
        this.activate(cx);
        this
    }

    pub fn engine_id(&self) -> &EngineId {
        &self.engine
    }

    pub fn live_mode(&self) -> LiveMode {
        self.live_mode
    }

    /// Capabilities of the engine (empty until `info` arrived).
    pub fn capabilities(&self) -> Capabilities {
        self.info.data().map(|i| i.capabilities).unwrap_or_default()
    }

    pub fn info(&self) -> Option<&EngineInfo> {
        self.info.data()
    }

    /// Loads all four collections in parallel plus info, then subscribes to events
    /// (spec 10 §4.2 step 1). Also used after `HubEvent::Reconnected`.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        self.refresh_all(cx);
        self.fetch_info(cx);
        self.subscribe(cx);
    }

    /// `F5` / `Mod+R` (spec 10 §4.2 step 4).
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        for c in Collection::ALL {
            self.refetch(c, cx);
        }
    }

    /// Refetch one collection. Old data stays visible (SHL-004).
    pub fn refetch(&mut self, collection: Collection, cx: &mut Context<Self>) {
        let ix = collection.ix();
        self.revisions[ix] += 1;
        let rev = self.revisions[ix];
        let hub = self.hub.clone();
        let engine = self.engine.clone();
        match collection {
            Collection::Containers => self.containers.start_loading(),
            Collection::Images => self.images.start_loading(),
            Collection::Volumes => self.volumes.start_loading(),
            Collection::Networks => self.networks.start_loading(),
        }
        cx.emit(EngineStoreEvent::Changed(collection));
        cx.notify();
        self.fetches[ix] = Some(cx.spawn(async move |this, cx| {
            macro_rules! fetch {
                ($field:ident, $call:expr) => {{
                    let result = $call.await;
                    this.update(cx, |this, cx| {
                        if this.revisions[ix] != rev {
                            return; // stale (NFR-005)
                        }
                        this.note_result(result.as_ref().err());
                        this.$field.finish(result);
                        cx.emit(EngineStoreEvent::Changed(collection));
                        cx.notify();
                    })
                    .ok();
                }};
            }
            match collection {
                Collection::Containers => fetch!(
                    containers,
                    hub.call(&engine, |e| async move {
                        e.list_containers(ContainerQuery::default()).await
                    })
                ),
                Collection::Images => fetch!(
                    images,
                    hub.call(&engine, |e| async move { e.list_images().await })
                ),
                Collection::Volumes => fetch!(
                    volumes,
                    hub.call(&engine, |e| async move { e.list_volumes().await })
                ),
                Collection::Networks => fetch!(
                    networks,
                    hub.call(&engine, |e| async move { e.list_networks().await })
                ),
            }
        }));
    }

    fn note_result(&mut self, err: Option<&EngineError>) {
        if err.is_none() {
            self.connected_once = true;
        }
    }

    pub fn fetch_info(&mut self, cx: &mut Context<Self>) {
        self.revisions[4] += 1;
        let rev = self.revisions[4];
        let hub = self.hub.clone();
        let engine = self.engine.clone();
        self.info.start_loading();
        self.fetches[4] = Some(cx.spawn(async move |this, cx| {
            let result = hub.call(&engine, |e| async move { e.info().await }).await;
            this.update(cx, |this, cx| {
                if this.revisions[4] != rev {
                    return;
                }
                let caps_before = this.capabilities();
                this.info.finish(result);
                cx.emit(EngineStoreEvent::InfoChanged);
                // The events capability may have appeared/disappeared.
                if this.capabilities() != caps_before && this.info.data().is_some() {
                    this.subscribe(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// (Re)subscribes to engine events, or polls when events aren't available.
    pub fn subscribe(&mut self, cx: &mut Context<Self>) {
        let has_info = self.info.data().is_some();
        if has_info && !self.capabilities().contains(Capabilities::EVENTS) {
            self.start_polling(cx);
            return;
        }
        let mut stream = self.hub.events(&self.engine);
        self.live_mode = LiveMode::Events;
        self.live = Some(cx.spawn(async move |this, cx| {
            let mut unreachable = false;
            while let Some(item) = stream.next().await {
                let keep = this
                    .update(cx, |this, cx| match item {
                        Ok(Feed::Item(ev)) => {
                            this.on_event(&ev, cx);
                            true
                        }
                        Ok(Feed::Lagged { dropped }) => {
                            // A missed `destroy` must not leave ghost rows: full refetch.
                            tracing::debug!(dropped, "engine events lagged; full refetch");
                            this.refresh_all(cx);
                            true
                        }
                        Err(err) => {
                            tracing::debug!(%err, "engine events failed");
                            unreachable = err.is_unreachable();
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                if unreachable {
                    // Not connected: wait for `set_connected(true)` / `Reconnected`.
                    this.live_mode = LiveMode::Idle;
                    cx.notify();
                } else {
                    // Stream ended or errored: fall back to polling (spec 10 §4.2 step 3).
                    this.start_polling(cx);
                }
            })
            .ok();
        }));
    }

    /// SET-020: the polling fallback interval changed in Settings; restarts the poll loop
    /// with it when polling is active.
    pub fn set_polling_interval(&mut self, seconds: u32, cx: &mut Context<Self>) {
        let interval = Duration::from_secs(seconds.max(1) as u64);
        if interval == self.polling_interval {
            return;
        }
        self.polling_interval = interval;
        if self.live_mode == LiveMode::Polling {
            self.start_polling(cx);
        }
    }

    pub fn polling_interval(&self) -> Duration {
        self.polling_interval
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.live_mode = LiveMode::Polling;
        let interval = self.polling_interval;
        self.live = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                if this.update(cx, |this, cx| this.refresh_all(cx)).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_event(&mut self, event: &EngineEvent, cx: &mut Context<Self>) {
        self.pending.extend(Collection::affected_by(event));
        if self.debounce.is_some() {
            return;
        }
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EVENT_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.debounce = None;
                let pending = std::mem::take(&mut this.pending);
                for c in pending {
                    this.refetch(c, cx);
                }
            })
            .ok();
        }));
    }

    /// The engine's connection state changed. On (re)connect after a failure: reload and
    /// resubscribe. On disconnect: stop live updates (calls would fail fast anyway).
    pub fn set_connected(&mut self, connected: bool, cx: &mut Context<Self>) {
        if connected {
            let failed =
                self.containers.error().is_some_and(|e| e.is_unreachable()) || !self.connected_once;
            if failed || self.live_mode == LiveMode::Idle {
                self.activate(cx);
            }
        } else {
            self.live = None;
            self.debounce = None;
            self.live_mode = LiveMode::Idle;
            self.connected_once = false;
        }
    }

    /// `HubEvent::Reconnected` (ENG-022): resubscribe + refetch everything.
    pub fn reconnected(&mut self, cx: &mut Context<Self>) {
        self.activate(cx);
    }

    pub fn resource(&self, c: Collection) -> ResourceState {
        let (loading, first, err) = match c {
            Collection::Containers => (
                self.containers.is_loading(),
                self.containers.is_first_load(),
                self.containers.error().cloned(),
            ),
            Collection::Images => (
                self.images.is_loading(),
                self.images.is_first_load(),
                self.images.error().cloned(),
            ),
            Collection::Volumes => (
                self.volumes.is_loading(),
                self.volumes.is_first_load(),
                self.volumes.error().cloned(),
            ),
            Collection::Networks => (
                self.networks.is_loading(),
                self.networks.is_first_load(),
                self.networks.error().cloned(),
            ),
        };
        ResourceState {
            loading,
            first_load: first,
            error: err,
        }
    }

    /// Count for the sidebar badge, when known.
    pub fn count(&self, c: Collection) -> Option<usize> {
        match c {
            Collection::Containers => self.containers.data().map(Vec::len),
            Collection::Images => self.images.data().map(Vec::len),
            Collection::Volumes => self.volumes.data().map(Vec::len),
            Collection::Networks => self.networks.data().map(Vec::len),
        }
    }
}

/// A view-friendly summary of a collection's load state.
#[derive(Debug, Clone)]
pub struct ResourceState {
    pub loading: bool,
    pub first_load: bool,
    pub error: Option<EngineError>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::fake::fixtures;

    #[test]
    fn container_events_also_refetch_images() {
        let ev = fixtures::event(ResourceKind::Container, "start", "x");
        assert_eq!(
            Collection::affected_by(&ev),
            vec![Collection::Containers, Collection::Images]
        );
        let ev = fixtures::event(ResourceKind::Daemon, "reload", "");
        assert_eq!(Collection::affected_by(&ev).len(), 4);
    }
}
