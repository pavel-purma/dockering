//! dk-engine-docker: `Engine` over the Docker Engine API via bollard (spec 20 §3, 21 §6).
//!
//! Public API fixed by the skeleton; no bollard types leak out of this crate.

mod connect;
mod containers;
mod discovery;
mod engine;
mod errors;
pub mod registry_auth;
mod resources;

use std::sync::Arc;

pub use discovery::DockerFactory;
pub use engine::DockerEngine;

/// Minimum supported Docker Engine API version (ENG-010).
pub const MIN_API_VERSION: &str = "1.41";

/// How to reach a Docker daemon. `NamedPipe` is also used by `dk-wsl` for its bridge pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerTarget {
    Unix(std::path::PathBuf),
    NamedPipe(String),
    Tcp {
        host: String,
        port: u16,
        tls: Option<dk_core::TlsFiles>,
    },
}

/// Overrides for engines built on top of `DockerEngine` by other backends (WSL distro).
#[derive(Clone, Default)]
pub struct DockerEngineOptions {
    /// Reported `EngineKind` (default `Docker`).
    pub kind: Option<dk_core::EngineKind>,
    /// Reported `EngineInfo.transport`, e.g. `"bridge"`.
    pub transport: Option<String>,
    /// Kept alive as long as the engine lives (e.g. the WSL pipe bridge).
    pub keepalive: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for DockerEngineOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerEngineOptions")
            .field("kind", &self.kind)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}
