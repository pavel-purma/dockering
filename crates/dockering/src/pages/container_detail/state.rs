//! `ContainerDetailState` (spec 10 §4.1): the inspect data for one container, refreshed on
//! every engine event for its id (CDT-003), plus the "removed while open" flag (CDT-080).
//!
//! Ownership (spec 10 §3.3): this holds the inspect `Resource` and the events subscription
//! only. Logs, stats and terminals live in their tab entities / the hub.

use std::time::Duration;

use dk_core::{
    Capabilities, ContainerDetails, ContainerState, ContainerSummary, EngineError, EngineEvent,
    EngineId, ResourceKind,
};
use dk_hub::{Feed, HubHandle};
use futures::StreamExt;
use gpui_kit::{App, Context, Entity, EventEmitter, Subscription, Task};

use crate::state::{Collection, EngineStore, EngineStoreEvent, Resource};

/// Debounce between an event for this container and the inspect refetch (spec 10 §4.2).
pub const EVENT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Something tabs may care about beyond "data changed" (`cx.observe` covers that).
#[derive(Debug, Clone, PartialEq)]
pub enum DetailEvent {
    /// An engine event for this container arrived (`start`, `die`, …).
    Engine(String),
    /// The container disappeared (CDT-080).
    Removed,
}

pub struct ContainerDetailState {
    hub: HubHandle,
    engine: EngineId,
    id: String,
    store: Option<Entity<EngineStore>>,
    pub details: Resource<ContainerDetails>,
    removed: bool,
    /// Stale fetch results are dropped (NFR-005).
    revision: u64,
    fetch: Option<Task<()>>,
    events: Option<Task<()>>,
    debounce: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DetailEvent> for ContainerDetailState {}

impl ContainerDetailState {
    pub fn new(
        hub: HubHandle,
        engine: EngineId,
        id: String,
        store: Option<Entity<EngineStore>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = store
            .as_ref()
            .map(|s| vec![cx.subscribe(s, Self::on_store_event)])
            .unwrap_or_default();
        let mut this = Self {
            hub,
            engine,
            id,
            store,
            details: Resource::Idle,
            removed: false,
            revision: 0,
            fetch: None,
            events: None,
            debounce: None,
            _subs: subs,
        };
        this.refresh(cx);
        this.subscribe(cx);
        this
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn engine(&self) -> &EngineId {
        &self.engine
    }

    pub fn store(&self) -> Option<&Entity<EngineStore>> {
        self.store.as_ref()
    }

    pub fn is_removed(&self) -> bool {
        self.removed
    }

    pub fn capabilities(&self, cx: &App) -> Capabilities {
        self.store
            .as_ref()
            .map(|s| s.read(cx).capabilities())
            .unwrap_or_default()
    }

    /// The summary for the header: from inspect, or the list row while inspect loads.
    pub fn summary<'a>(&'a self, cx: &'a App) -> Option<&'a ContainerSummary> {
        self.details
            .data()
            .map(|d| &d.summary)
            .or_else(|| self.list_summary(cx))
    }

    fn list_summary<'a>(&self, cx: &'a App) -> Option<&'a ContainerSummary> {
        let store = self.store.as_ref()?.read(cx);
        store
            .containers
            .data()?
            .iter()
            .find(|c| matches_id(&c.id, &self.id) || c.name == self.id)
    }

    pub fn state(&self, cx: &App) -> Option<ContainerState> {
        self.summary(cx).map(|s| s.state)
    }

    pub fn is_running(&self, cx: &App) -> bool {
        !self.removed && self.state(cx).is_some_and(|s| s.is_running())
    }

    /// Refetch `inspect_container`; old data stays visible (SHL-004).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.removed {
            return;
        }
        self.revision += 1;
        let rev = self.revision;
        self.details.start_loading();
        cx.notify();
        let id = self.id.clone();
        let call = self.hub.call(&self.engine, move |e| async move {
            e.inspect_container(&id).await
        });
        self.fetch = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                if this.revision != rev {
                    return; // stale (NFR-005)
                }
                if matches!(result, Err(EngineError::NotFound { .. })) {
                    this.mark_removed(cx);
                    // Keep the last data visible read-only (CDT-080).
                    this.details.finish(result);
                } else {
                    this.details.finish(result);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Engine events for this id (CDT-003). The hub shares one engine stream per engine.
    fn subscribe(&mut self, cx: &mut Context<Self>) {
        let mut stream = self.hub.events(&self.engine);
        self.events = Some(cx.spawn(async move |this, cx| {
            while let Some(item) = stream.next().await {
                let keep = this
                    .update(cx, |this, cx| match item {
                        Ok(Feed::Item(ev)) => {
                            this.on_event(&ev, cx);
                            true
                        }
                        Ok(Feed::Lagged { .. }) => {
                            // A missed `destroy` must not leave us stale: refetch.
                            this.schedule_refresh(cx);
                            true
                        }
                        Err(err) => {
                            tracing::debug!(%err, "detail events failed; relying on the store");
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn on_event(&mut self, ev: &EngineEvent, cx: &mut Context<Self>) {
        if ev.kind != ResourceKind::Container || !matches_id(&ev.id, &self.id) {
            return;
        }
        cx.emit(DetailEvent::Engine(ev.action.clone()));
        if ev.action == "destroy" {
            self.mark_removed(cx);
            return;
        }
        self.schedule_refresh(cx);
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.debounce.is_some() || self.removed {
            return;
        }
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EVENT_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.debounce = None;
                this.refresh(cx);
            })
            .ok();
        }));
    }

    /// The list refetched (events or polling, spec 10 §4.2): detect removal and state
    /// changes we may have missed (engines without `EVENTS`).
    fn on_store_event(
        &mut self,
        store: Entity<EngineStore>,
        event: &EngineStoreEvent,
        cx: &mut Context<Self>,
    ) {
        if *event != EngineStoreEvent::Changed(Collection::Containers) || self.removed {
            return;
        }
        let s = store.read(cx);
        if s.containers.is_loading() || s.containers.error().is_some() {
            return;
        }
        let Some(list) = s.containers.data() else {
            return;
        };
        match list.iter().find(|c| matches_id(&c.id, &self.id)) {
            None => self.mark_removed(cx),
            Some(row) => {
                let stale = self.details.data().is_some_and(|d| {
                    d.summary.state != row.state
                        || d.summary.status_text != row.status_text
                        || d.summary.health != row.health
                });
                if stale && !self.details.is_loading() {
                    self.schedule_refresh(cx);
                }
            }
        }
        cx.notify();
    }

    fn mark_removed(&mut self, cx: &mut Context<Self>) {
        if self.removed {
            return;
        }
        self.removed = true;
        self.debounce = None;
        cx.emit(DetailEvent::Removed);
        cx.notify();
    }
}

/// Full ids, or one being a prefix of the other (short ids in deep links).
pub fn matches_id(a: &str, b: &str) -> bool {
    a == b || (b.len() >= 12 && a.starts_with(b)) || (a.len() >= 12 && b.starts_with(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_by_prefix() {
        let full = "a1b2c3d4e5f6a1b2c3d4e5f6";
        assert!(matches_id(full, full));
        assert!(matches_id(full, "a1b2c3d4e5f6"));
        assert!(!matches_id(full, "a1b2"));
        assert!(!matches_id("abc", "abd"));
    }
}
