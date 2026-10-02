//! Image operations on the hub (IMG-003…006, IMG-011). Pure async wrappers; the views spawn
//! them and report through notifications.

use dk_core::{EngineId, PruneReport, RunSpec};
use dk_hub::{HubCall, HubHandle};
use futures::FutureExt;

use crate::pages::resources::ops::{self, Outcome, Target};

/// Delete images by reference (`repo:tag` untags, an id removes the image) (IMG-006).
pub fn remove(
    hub: &HubHandle,
    engine: &EngineId,
    targets: Vec<Target>,
    force: bool,
) -> HubCall<Outcome> {
    ops::remove_many(hub, engine, targets, move |e, arg| {
        async move { e.remove_image(&arg, force).await.map(|_| ()) }.boxed()
    })
}

/// IMG-003
pub fn prune(hub: &HubHandle, engine: &EngineId, dangling_only: bool) -> HubCall<PruneReport> {
    hub.call(engine, move |e| async move {
        e.prune_images(dangling_only).await
    })
}

/// IMG-005: create + start; resolves to the new container id.
pub fn run(hub: &HubHandle, engine: &EngineId, spec: RunSpec) -> HubCall<String> {
    hub.call(engine, move |e| async move { e.run_image(spec).await })
}

/// IMG-011
pub fn tag(
    hub: &HubHandle,
    engine: &EngineId,
    id: String,
    repo: String,
    tag: String,
) -> HubCall<()> {
    hub.call(engine, move |e| async move {
        e.tag_image(&id, &repo, &tag).await
    })
}

/// Unused-image bytes for the prune confirmation (IMG-003, `DISK_USAGE`).
pub fn reclaimable(hub: &HubHandle, engine: &EngineId) -> HubCall<Option<u64>> {
    hub.call(engine, |e| async move {
        e.disk_usage().await.map(|d| d.images_reclaimable)
    })
}
