//! StatsService (STA-002/003/006): one engine stream per (engine, container) shared by N
//! subscribers; ring buffers of the history window live here. SKELETON — `rust-core`.

use dk_core::{EngineId, StatsSample};

use crate::bridge::{Feed, HubStream};
use crate::handle::HubHandle;

pub(crate) fn subscribe(h: &HubHandle, engine: &EngineId, id: &str) -> HubStream<Feed<StatsSample>> {
    let _ = (h, engine, id);
    unimplemented!("rust-core")
}
