//! In-app updates, UI side (UPD-008, UPD-009): the latest `UpdateStatus` from the hub plus the
//! manual check. The shell observes it for the status bar and notifications, Settings → Updates
//! for its section.

use dk_core::EngineError;
use dk_hub::{HubHandle, UpdateCheck, UpdateStatus};
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
    status: UpdateStatus,
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
                status: UpdateStatus::Idle { last_check: None },
                manual: None,
                request: 0,
                _status_task: cx.spawn(async move |this, cx| {
                    while let Some(item) = stream.next().await {
                        let Ok(status) = item else { continue };
                        if this
                            .update(cx, |s, cx| {
                                s.status = status;
                                cx.notify();
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

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    pub fn manual(&self) -> Option<&ManualCheck> {
        self.manual.as_ref()
    }

    /// The version that can be installed right now, if any.
    pub fn ready_version(&self) -> Option<&str> {
        match &self.status {
            UpdateStatus::Ready { version, .. } => Some(version),
            _ => None,
        }
    }

    /// Release notes of the offered version, if there is one.
    pub fn notes_url(&self) -> Option<&str> {
        match &self.status {
            UpdateStatus::Ready { notes_url, .. } | UpdateStatus::Available { notes_url, .. } => {
                Some(notes_url)
            }
            _ => None,
        }
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
    pub fn set_status_for_test(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }
}
