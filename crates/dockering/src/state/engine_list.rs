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

/// Whether `e` can be pinned as the startup engine (ENG-116): enabled, listed, and not
/// unsupported. Mirrors the hub's `startup_engine`, so the UI never offers a pin the hub
/// would skip.
pub fn can_be_default(e: &EngineStatus) -> bool {
    e.config.enabled
        && !e.config.hidden
        && !matches!(
            e.state,
            EngineState::Unsupported { .. } | EngineState::Disabled
        )
}

/// How an engine row presents the pinned default (ENG-116).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultMark {
    /// Not the default.
    None,
    /// The default and usable at startup.
    Default,
    /// The default, but the hub would skip it (disabled, hidden, unsupported).
    Unavailable,
}

/// The mark for `e` given the pinned default id. Settings and the switcher share it.
pub fn default_mark(e: &EngineStatus, default: Option<&EngineId>) -> DefaultMark {
    match default {
        Some(d) if d == e.id() => {
            if can_be_default(e) {
                DefaultMark::Default
            } else {
                DefaultMark::Unavailable
            }
        }
        _ => DefaultMark::None,
    }
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
    /// Full metadata changed, including routing notes/limits with unchanged flags (ENG-136).
    InfoChanged(EngineId),
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
                    this.replace_snapshot(list, active, cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn apply(&mut self, ev: HubEvent, cx: &mut Context<Self>) {
        // A newer streamed update must not be overwritten by an in-flight resync.
        self.revision += 1;
        match ev {
            HubEvent::Snapshot(list) => {
                let active = list.iter().find(|s| s.active).map(|s| s.id().clone());
                self.replace_snapshot(list, active.or_else(|| self.hub.active_engine()), cx);
            }
            HubEvent::Added(status) | HubEvent::StatusChanged(status) => {
                let id = status.id().clone();
                let active_now = status.active;
                let info_changed = self.get(&id).map(|s| &s.info) != Some(&status.info);
                match self.engines.iter_mut().find(|e| e.id() == &id) {
                    Some(slot) => *slot = status,
                    None => self.engines.push(status),
                }
                if active_now && self.active.as_ref() != Some(&id) {
                    self.set_active(Some(id.clone()), cx);
                }
                if info_changed {
                    cx.emit(EngineListEvent::InfoChanged(id));
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

    fn replace_snapshot(
        &mut self,
        list: Vec<EngineStatus>,
        active: Option<EngineId>,
        cx: &mut Context<Self>,
    ) {
        let changed: Vec<_> = list
            .iter()
            .filter(|s| self.get(s.id()).map(|old| &old.info) != Some(&s.info))
            .map(|s| s.id().clone())
            .collect();
        self.engines = list;
        self.loaded = true;
        self.set_active(active, cx);
        for id in changed {
            cx.emit(EngineListEvent::InfoChanged(id));
        }
    }

    #[cfg(test)]
    pub(crate) fn apply_test_event(&mut self, ev: HubEvent, cx: &mut Context<Self>) {
        self.apply(ev, cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::{EngineConfig, EngineEndpoint, EngineOrigin};

    fn status(id: &str, state: EngineState, enabled: bool, hidden: bool) -> EngineStatus {
        EngineStatus {
            config: EngineConfig {
                id: EngineId::new(id),
                name: id.into(),
                endpoint: EngineEndpoint::UnixSocket {
                    path: format!("/fake/{id}.sock").into(),
                },
                origin: EngineOrigin::Discovered,
                enabled,
                hidden,
            },
            state,
            info: None,
            active: false,
            also_reachable_via: Vec::new(),
        }
    }

    #[test]
    fn eng_116_default_mark_follows_what_the_hub_would_open() {
        let a = EngineId::new("a");
        let ok = status("a", EngineState::Connected, true, false);
        assert_eq!(default_mark(&ok, Some(&a)), DefaultMark::Default);
        assert_eq!(default_mark(&ok, None), DefaultMark::None);
        assert_eq!(
            default_mark(&ok, Some(&EngineId::new("b"))),
            DefaultMark::None
        );
        // A down engine is still the default: it opens and shows Failed with Retry.
        let down = status("a", EngineState::Disconnected, true, false);
        assert_eq!(default_mark(&down, Some(&a)), DefaultMark::Default);
        for bad in [
            status("a", EngineState::Disabled, false, false),
            status("a", EngineState::Connected, true, true),
            status(
                "a",
                EngineState::Unsupported { reason: "x".into() },
                true,
                false,
            ),
        ] {
            assert_eq!(default_mark(&bad, Some(&a)), DefaultMark::Unavailable);
            assert!(!can_be_default(&bad));
        }
    }
}
