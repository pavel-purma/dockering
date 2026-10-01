//! `DockerEngine` (bollard). SKELETON — `engine-integrator`.

use dk_core::{EngineId, EngineResult};

use crate::{DockerEngineOptions, DockerTarget};

pub struct DockerEngine {
    _private: (),
}

impl DockerEngine {
    /// Connect, negotiate the API version (`min(server, client)`, require ≥ 1.41), and
    /// compute capabilities. Fails with `Unreachable{hint}` when the daemon can't be reached,
    /// `Api{status: 0, ..}`-style "unsupported version" error for API < 1.41.
    pub async fn connect(
        id: EngineId,
        target: DockerTarget,
        opts: DockerEngineOptions,
    ) -> EngineResult<DockerEngine> {
        let _ = (id, target, opts);
        unimplemented!("engine-integrator")
    }
}
