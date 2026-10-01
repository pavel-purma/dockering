//! Hub-side terminal actors (spec 10 §3.3, TRM-008). SKELETON — `rust-core`.

use dk_core::{EngineId, ExecRequest};

use crate::bridge::{HubCall, TerminalHandle};
use crate::handle::HubHandle;

pub(crate) fn open(
    h: &HubHandle,
    engine: &EngineId,
    id: &str,
    req: ExecRequest,
) -> HubCall<TerminalHandle> {
    let _ = (h, engine, id, req);
    unimplemented!("rust-core")
}
