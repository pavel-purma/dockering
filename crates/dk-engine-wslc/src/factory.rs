//! `WslcFactory`. SKELETON (stub impl so the crate graph type-checks; replaced by the owner agent).

use std::sync::Arc;

use async_trait::async_trait;
use dk_core::{
    DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema, EngineEndpoint, EngineError,
    EngineFactory, EngineKind, EngineResult,
};

pub struct WslcFactory {
    _private: (),
}

impl WslcFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WslcFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EngineFactory for WslcFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }
    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        endpoint.kind() == Some(EngineKind::Wslc)
    }
    async fn discover(&self) -> Vec<DiscoveredEngine> {
        Vec::new()
    }
    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::unreachable(format!("WSLC backend not implemented yet ({})", cfg.id)))
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        Vec::new()
    }
}
