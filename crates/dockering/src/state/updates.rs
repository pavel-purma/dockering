//! In-app updates, UI side (UPD-008, UPD-009): the latest `UpdateSnapshot` from the hub (the
//! activity plus the retained offer) and the manual check. The shell observes it for the status
//! bar and notifications, Settings → Updates for its section.

use dk_core::EngineError;
use dk_hub::{
    HubHandle, OfferMode, OfferStage, UpdateCheck, UpdateOffer, UpdateSnapshot, UpdateStatus,
};
use futures::StreamExt;
use gpui_kit::{App, AppContext, Context, Entity, Global, Task};

/// Result of the last manual *Check now* (shown inline in Settings, UPD-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCheck {
    Running,
    Done(UpdateCheck),
    Failed(String),
}

pub struct UpdateStore {
    hub: HubHandle,
    /// The latest snapshot; an older one (lower `revision`) is dropped (NFR-005).
    snapshot: UpdateSnapshot,
    manual: Option<ManualCheck>,
    /// Request id of the latest manual check; older results are dropped (NFR-005).
    request: u64,
    _status_task: Task<()>,
    check_task: Option<Task<()>>,
}

struct GlobalUpdates(Entity<UpdateStore>);
impl Global for GlobalUpdates {}

impl UpdateStore {
    pub fn install(hub: HubHandle, cx: &mut App) -> Entity<UpdateStore> {
        let entity = cx.new(|cx: &mut Context<UpdateStore>| {
            let mut stream = hub.update_status();
            UpdateStore {
                hub,
                snapshot: UpdateSnapshot {
                    status: UpdateStatus::Idle { last_check: None },
                    offer: None,
                    revision: 0,
                },
                manual: None,
                request: 0,
                _status_task: cx.spawn(async move |this, cx| {
                    while let Some(item) = stream.next().await {
                        let Ok(snapshot) = item else { continue };
                        if this
                            .update(cx, |s, cx| {
                                // The stream is ordered; the guard is for a replay racing a
                                // newer value.
                                if snapshot.revision >= s.snapshot.revision {
                                    s.snapshot = snapshot;
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }),
                check_task: None,
            }
        });
        cx.set_global(GlobalUpdates(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<UpdateStore>> {
        cx.try_global::<GlobalUpdates>().map(|g| g.0.clone())
    }

    /// What the updater is doing (checking, downloading, off, failed); the offer is separate.
    pub fn status(&self) -> &UpdateStatus {
        &self.snapshot.status
    }

    /// The retained offer, if there is one (UPD-017).
    pub fn offer(&self) -> Option<&UpdateOffer> {
        self.snapshot.offer.as_ref()
    }

    pub fn manual(&self) -> Option<&ManualCheck> {
        self.manual.as_ref()
    }

    /// The version that can be installed right now, if any: an install offer whose verified
    /// payload is ready.
    pub fn ready_version(&self) -> Option<&str> {
        self.offer()
            .filter(|o| o.mode == OfferMode::Install && o.stage == OfferStage::Ready)
            .map(|o| o.version.as_str())
    }

    /// Release notes of the offered version, if there is one.
    pub fn notes_url(&self) -> Option<&str> {
        self.offer().map(|o| o.notes_url.as_str())
    }

    /// *Check now* (UPD-004: a manual check surfaces its errors).
    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        self.request += 1;
        let request = self.request;
        self.manual = Some(ManualCheck::Running);
        cx.notify();
        let call = self.hub.check_for_updates();
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |s, cx| {
                if s.request != request {
                    return;
                }
                s.manual = match result {
                    Ok(check) => Some(ManualCheck::Done(check)),
                    // Superseded (a newer check owns `manual`) or hub shutdown.
                    Err(EngineError::Cancelled) => None,
                    Err(e) => Some(ManualCheck::Failed(e.to_string())),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn set_snapshot_for_test(&mut self, snapshot: UpdateSnapshot, cx: &mut Context<Self>) {
        self.snapshot = snapshot;
        cx.notify();
    }
}
