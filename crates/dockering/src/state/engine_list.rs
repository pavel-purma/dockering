use dk_core::{EngineId, EngineState, EngineStatus};
use dk_hub::{Feed, HubEvent, HubHandle};
use futures::StreamExt;
use gpui_kit::{Context, EventEmitter, Task};

/// Every configured and discovered engine with its status (spec 10 §4.1). Fed by
/// `hub.hub_events()`; a `Lagged` feed triggers a full `hub.engines()` refetch.
pub struct EngineListStore {
    hub: HubHandle,
    engines: Vec<EngineStatus>,
    active: Option<EngineId>,
    /// True once the first snapshot arrived (before that the shell shows skeletons).
    loaded: bool,
    revision: u64,
    _events: Option<Task<()>>,
    refetch: Option<Task<()>>,
}

/// Notifications for views that care about specific changes.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineListEvent {
    /// The active engine changed (ENG-102): the shell swaps its `EngineStore`.
    ActiveChanged(Option<EngineId>),
    /// ENG-022: the active engine reconnected; stores resubscribe + refetch.
    Reconnected(EngineId),
    /// Capabilities of an engine changed (transport switch).
    CapabilitiesChanged(EngineId),
}

impl EventEmitter<EngineListEvent> for EngineListStore {}

impl EngineListStore {
    pub fn new(hub: HubHandle, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            active: hub.active_engine(),
            hub,
            engines: Vec::new(),
            loaded: false,
            revision: 0,
            _events: None,
            refetch: None,
        };
        this.subscribe(cx);
        this
    }

    fn subscribe(&mut self, cx: &mut Context<Self>) {
        let mut stream = self.hub.hub_events();
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(item) = stream.next().await {
                let ok = this
                    .update(cx, |this, cx| match item {
                        Ok(Feed::Item(ev)) => this.apply(ev, cx),
                        Ok(Feed::Lagged { dropped }) => {
                            tracing::debug!(dropped, "hub events lagged; refetching engines");
                            this.refetch(cx);
                        }
                        Err(err) => tracing::warn!(%err, "hub events stream error"),
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        }));
    }

    /// Full resync (`Feed::Lagged`).
    pub fn refetch(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let rev = self.revision;
        let call = self.hub.engines();
        let active = self.hub.active_engine();
        self.refetch = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                if this.revision != rev {
                    return;
                }
                if let Ok(list) = result {
                    this.engines = list;
                    this.loaded = true;
                    this.set_active(active, cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn apply(&mut self, ev: HubEvent, cx: &mut Context<Self>) {
        match ev {
            HubEvent::Snapshot(list) => {
                let active = list.iter().find(|s| s.active).map(|s| s.id().clone());
                self.engines = list;
                self.loaded = true;
                self.set_active(active.or_else(|| self.hub.active_engine()), cx);
            }
            HubEvent::Added(status) | HubEvent::StatusChanged(status) => {
                let id = status.id().clone();
                let active_now = status.active;
                match self.engines.iter_mut().find(|e| e.id() == &id) {
                    Some(slot) => *slot = status,
                    None => self.engines.push(status),
                }
                if active_now && self.active.as_ref() != Some(&id) {
                    self.set_active(Some(id), cx);
                }
            }
            HubEvent::Removed(id) => {
                self.engines.retain(|e| e.id() != &id);
                if self.active.as_ref() == Some(&id) {
                    self.set_active(None, cx);
                }
            }
            HubEvent::CapabilitiesChanged { id, capabilities } => {
                if let Some(info) = self
                    .engines
                    .iter_mut()
                    .find(|e| e.id() == &id)
                    .and_then(|e| e.info.as_mut())
                {
                    info.capabilities = capabilities;
                }
                cx.emit(EngineListEvent::CapabilitiesChanged(id));
            }
            HubEvent::ActiveChanged(id) => self.set_active(id, cx),
            HubEvent::Reconnected(id) => cx.emit(EngineListEvent::Reconnected(id)),
        }
        cx.notify();
    }

    fn set_active(&mut self, id: Option<EngineId>, cx: &mut Context<Self>) {
        for e in &mut self.engines {
            e.active = Some(e.id()) == id.as_ref();
        }
        if self.active != id {
            self.active = id.clone();
            cx.emit(EngineListEvent::ActiveChanged(id));
        }
    }

    pub fn engines(&self) -> &[EngineStatus] {
        &self.engines
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn active_id(&self) -> Option<&EngineId> {
        self.active.as_ref()
    }

    pub fn active(&self) -> Option<&EngineStatus> {
        let id = self.active.as_ref()?;
        self.engines.iter().find(|e| e.id() == id)
    }

    pub fn get(&self, id: &EngineId) -> Option<&EngineStatus> {
        self.engines.iter().find(|e| e.id() == id)
    }

    /// No engine exists at all (ENG-111 first-run screen).
    pub fn is_empty(&self) -> bool {
        self.loaded && self.engines.is_empty()
    }

    /// True when every known engine is unusable and none is active (first-run guidance).
    pub fn none_usable(&self) -> bool {
        self.loaded
            && self.active.is_none()
            && !self
                .engines
                .iter()
                .any(|e| matches!(e.state, EngineState::Connected | EngineState::Connecting))
    }

    /// Optimistically mark `id` active so the shell switches within one frame (ENG-102).
    pub fn mark_switching(&mut self, id: EngineId, cx: &mut Context<Self>) {
        self.set_active(Some(id), cx);
        cx.notify();
    }
}
