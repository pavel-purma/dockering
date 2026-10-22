//! dk-engine-docker: `Engine` over the Docker Engine API via bollard (spec 20 §3, 21 §6).
//!
//! Public API fixed by the skeleton; no bollard types leak out of this crate.

mod connect;
mod containers;
mod desktop;
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

/// A fake Docker Desktop install and pipes for tests (`DockerFactory::for_desktop_tests`).
#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct DesktopTestConfig {
    /// Folder holding `Docker Desktop.exe` and `resources\bin\docker.exe`.
    pub install_location: std::path::PathBuf,
    pub engine_pipe: String,
    pub linux_pipe: String,
    pub start_timeout: std::time::Duration,
    pub poll_timeout: std::time::Duration,
    pub poll_interval: std::time::Duration,
    pub status_timeout: std::time::Duration,
}

#[cfg(feature = "test-support")]
impl DesktopTestConfig {
    /// Short timeouts and unique, absent pipe names.
    pub fn new(install_location: impl Into<std::path::PathBuf>) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let tag = format!(
            "{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            install_location: install_location.into(),
            engine_pipe: format!(r"\\.\pipe\dk-test-engine-{tag}"),
            linux_pipe: format!(r"\\.\pipe\dk-test-linux-{tag}"),
            start_timeout: std::time::Duration::from_secs(10),
            poll_timeout: std::time::Duration::from_secs(5),
            poll_interval: std::time::Duration::from_millis(50),
            status_timeout: std::time::Duration::from_secs(5),
        }
    }
}
