//! EngineHub internals. SKELETON — implemented by `rust-core`.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use dk_core::{
    Engine, EngineConfig, EngineConfigSchema, EngineEvent, EngineId, EngineInfo, EngineResult,
    EngineStatus,
};
use futures::Stream;

use crate::bridge::{Feed, HubCall, HubEvent, HubStream};
use crate::config::{Config, UiState};
use crate::handle::{ConfigHandle, HubHandle, HubOptions};
use crate::paths::Paths;

pub struct HubInner {
    pub(crate) paths: Paths,
}

pub struct EngineHub;

impl EngineHub {
    /// Creates the dedicated multi-thread tokio runtime (`dk-hub-N` threads) and starts the
    /// registry, supervisor, and discovery. Never blocks for engine I/O.
    pub fn start(opts: HubOptions) -> std::io::Result<HubHandle> {
        let _ = opts;
        unimplemented!("rust-core")
    }
}

pub(crate) fn call<T, F, C>(h: &HubHandle, engine: &EngineId, f: C) -> HubCall<T>
where
    C: FnOnce(Arc<dyn Engine>) -> F + Send + 'static,
    F: Future<Output = EngineResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let _ = (h, engine, f);
    unimplemented!("rust-core")
}

pub(crate) fn subscribe<T, S, C>(h: &HubHandle, engine: &EngineId, f: C) -> HubStream<T>
where
    C: FnOnce(Arc<dyn Engine>) -> S + Send + 'static,
    S: Stream<Item = EngineResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let _ = (h, engine, f);
    unimplemented!("rust-core")
}

pub(crate) fn hub_events(h: &HubHandle) -> HubStream<Feed<HubEvent>> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn engines(h: &HubHandle) -> HubCall<Vec<EngineStatus>> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn active_engine(h: &HubHandle) -> Option<EngineId> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn set_active(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let _ = (h, id);
    unimplemented!("rust-core")
}
pub(crate) fn add_engine(h: &HubHandle, cfg: EngineConfig) -> HubCall<EngineId> {
    let _ = (h, cfg);
    unimplemented!("rust-core")
}
pub(crate) fn update_engine(h: &HubHandle, cfg: EngineConfig) -> HubCall<()> {
    let _ = (h, cfg);
    unimplemented!("rust-core")
}
pub(crate) fn remove_engine(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let _ = (h, id);
    unimplemented!("rust-core")
}
pub(crate) fn test_engine(h: &HubHandle, cfg: EngineConfig) -> HubCall<EngineInfo> {
    let _ = (h, cfg);
    unimplemented!("rust-core")
}
pub(crate) fn rescan(h: &HubHandle) -> HubCall<()> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn retry(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let _ = (h, id);
    unimplemented!("rust-core")
}
pub(crate) fn start_engine(h: &HubHandle, id: &EngineId) -> HubCall<()> {
    let _ = (h, id);
    unimplemented!("rust-core")
}
pub(crate) fn engine_schemas(h: &HubHandle) -> Vec<EngineConfigSchema> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn events(h: &HubHandle, engine: &EngineId) -> HubStream<Feed<EngineEvent>> {
    let _ = (h, engine);
    unimplemented!("rust-core")
}
pub(crate) fn save_file(h: &HubHandle, path: PathBuf, bytes: Bytes) -> HubCall<()> {
    let _ = (h, path, bytes);
    unimplemented!("rust-core")
}
pub(crate) fn launch(h: &HubHandle, argv: Vec<String>) -> HubCall<()> {
    let _ = (h, argv);
    unimplemented!("rust-core")
}
pub(crate) fn diagnostics(h: &HubHandle) -> HubCall<String> {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn config_handle(h: &HubHandle) -> ConfigHandle {
    ConfigHandle {
        inner: h.inner.clone(),
    }
}
pub(crate) fn shutdown(h: &HubHandle) {
    let _ = h;
    unimplemented!("rust-core")
}
pub(crate) fn config_get(inner: &Arc<HubInner>) -> Config {
    let _ = inner;
    unimplemented!("rust-core")
}
pub(crate) fn config_update(inner: &Arc<HubInner>, f: impl FnOnce(&mut Config)) {
    let _ = (inner, f);
    unimplemented!("rust-core")
}
pub(crate) fn ui_state_get(inner: &Arc<HubInner>) -> UiState {
    let _ = inner;
    unimplemented!("rust-core")
}
pub(crate) fn ui_state_update(inner: &Arc<HubInner>, f: impl FnOnce(&mut UiState)) {
    let _ = (inner, f);
    unimplemented!("rust-core")
}
