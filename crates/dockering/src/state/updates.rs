//! In-app updates, UI side (UPD-008, UPD-009): the latest `UpdateSnapshot` from the hub (the
//! activity plus the retained offer) and the manual check. The shell observes it for the status
//! bar and notifications, Settings → Updates for its section.

use dk_core::EngineError;
use dk_hub::{
    ApplyOutcome, HubHandle, OfferMode, OfferStage, UpdateCheck, UpdateOffer, UpdateSnapshot,
    UpdateStatus,
};
use futures::StreamExt;
use futures::future::{FutureExt, LocalBoxFuture};
use gpui_kit::{App, AppContext, Context, Entity, Global, Task};

/// The pending result of *Restart to update* (the hub call, or a test's stand-in).
pub type ApplyCall = LocalBoxFuture<'static, Result<ApplyOutcome, EngineError>>;

/// Result of the last manual *Check now* (shown inline in Settings, UPD-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCheck {
    Running,
    Done(UpdateCheck),
    Failed(String),
}

pub struct UpdateStore {
    hub: HubHandle,
    /// The latest snapshot; an older one (lower `revision`) is dropped (UPD-011, NFR-005).
    snapshot: UpdateSnapshot,
    manual: Option<ManualCheck>,
    /// Request id of the latest manual check; older results are dropped (NFR-005).
    request: u64,
    _status_task: Task<()>,
    check_task: Option<Task<()>>,
    /// What a view test runs instead of the hub's `apply_update()` (the test hub has no
    /// updater, so it can only fail).
    #[cfg(test)]
    apply_stub: Option<std::rc::Rc<dyn Fn() -> ApplyCall>>,
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
                        // A failed update means the store is gone: treat it as a cancel.
                        if this
                            .update(cx, |s, cx| {
                                s.apply_snapshot(snapshot, cx);
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }),
                check_task: None,
                #[cfg(test)]
                apply_stub: None,
            }
        });
        cx.set_global(GlobalUpdates(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<UpdateStore>> {
        cx.try_global::<GlobalUpdates>().map(|g| g.0.clone())
    }

    /// Takes `snapshot` unless it is older than the one held (a lower `revision`: a replay
    /// racing a newer value, UPD-011). Returns whether it was taken. Observers are notified
    /// only when something changed.
    fn apply_snapshot(&mut self, snapshot: UpdateSnapshot, cx: &mut Context<Self>) -> bool {
        if snapshot.revision < self.snapshot.revision {
            return false;
        }
        if snapshot != self.snapshot {
            self.snapshot = snapshot;
            cx.notify();
        }
        true
    }

    /// What the updater is doing (checking, downloading, off, failed); the offer is separate.
    pub fn status(&self) -> &UpdateStatus {
        &self.snapshot.status
    }

    /// Revision of the snapshot held.
    pub fn revision(&self) -> u64 {
        self.snapshot.revision
    }

    /// The retained offer, if there is one (UPD-017), as the hub reported it.
    pub fn offer(&self) -> Option<&UpdateOffer> {
        self.snapshot.offer.as_ref()
    }

    /// Policy, `DOCKERING_DISABLE_UPDATES`, demo mode, no `updater` feature or no trust keys
    /// (UPD-005): the retained offer is inert. The hub already leaves it out of the snapshot
    /// then; this keeps the UI from acting on one anyway (UPD-015, UPD-017 (8)). *Check for
    /// updates automatically* being off is not a hard disable.
    pub fn hard_disabled(&self) -> bool {
        matches!(&self.snapshot.status, UpdateStatus::Disabled { reason } if reason.is_hard())
    }

    /// The offer the title-bar button, the notification and *Restart to update* may act on.
    /// It comes from the retained offer alone, so checking, a failed check, an offline start
    /// and the automatic-check switch never hide it (UPD-015).
    pub fn actionable_offer(&self) -> Option<&UpdateOffer> {
        self.offer().filter(|_| !self.hard_disabled())
    }

    pub fn manual(&self) -> Option<&ManualCheck> {
        self.manual.as_ref()
    }

    /// The version that can be installed right now, if any: an install offer whose verified
    /// payload is *Ready*.
    pub fn ready_version(&self) -> Option<&str> {
        self.actionable_offer()
            .filter(|o| o.mode == OfferMode::Install && o.stage == OfferStage::Ready)
            .map(|o| o.version.as_str())
    }

    /// The version *Restart to update* can act on: *Ready*, or *Installed, restart* where the
    /// click only retries starting the new process (KBD-076).
    pub fn restartable_version(&self) -> Option<&str> {
        self.actionable_offer()
            .filter(|o| {
                o.mode == OfferMode::Install
                    && !o.install_failed
                    && matches!(o.stage, OfferStage::Ready | OfferStage::InstalledRestart)
            })
            .map(|o| o.version.as_str())
    }

    /// Release notes of the offered version, if there is an offer.
    pub fn notes_url(&self) -> Option<&str> {
        self.offer().map(|o| o.notes_url.as_str())
    }

    /// *Restart to update*: the hub's `apply_update()` (UPD-011).
    pub fn apply(&self) -> ApplyCall {
        #[cfg(test)]
        if let Some(stub) = &self.apply_stub {
            return stub();
        }
        self.hub.apply_update().boxed_local()
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

    /// Feeds `snapshot` through the same guard as the hub stream; `false` when it was dropped
    /// as stale.
    #[cfg(test)]
    pub fn set_snapshot_for_test(
        &mut self,
        snapshot: UpdateSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        self.apply_snapshot(snapshot, cx)
    }

    /// Makes `apply()` run `stub` instead of the hub call.
    #[cfg(test)]
    pub fn stub_apply_for_test(&mut self, stub: impl Fn() -> ApplyCall + 'static) {
        self.apply_stub = Some(std::rc::Rc::new(stub));
    }
}
