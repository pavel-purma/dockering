//! `DockerFactory`. SKELETON (stub impl so the crate graph type-checks; replaced by the owner agent).

use std::sync::Arc;

use async_trait::async_trait;
use dk_core::{
    DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema, EngineEndpoint, EngineError,
    EngineFactory, EngineKind, EngineResult,
};

pub struct DockerFactory {
    _private: (),
}

impl DockerFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for DockerFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EngineFactory for DockerFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }
    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        endpoint.kind() == Some(EngineKind::Docker)
    }
    async fn discover(&self) -> Vec<DiscoveredEngine> {
        Vec::new()
    }
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::unreachable(format!(
            "Docker backend not implemented yet ({})",
            cfg.id
        )))
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        Vec::new()
    }
}
