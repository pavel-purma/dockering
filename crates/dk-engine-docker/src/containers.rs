//! Container ops (spec 21 §6). Filled in by item 3.

use dk_core::{
    ContainerAction, ContainerDetails, ContainerQuery, ContainerSummary, EngineError, EngineEvent,
    EngineResult, EngineStream, EventFilter, ExecRequest, LogChunk, LogOpts, ProcessList,
    PruneReport, RemoveContainerOpts, StatsSample, TerminalSession, error_stream,
};

use crate::DockerEngine;

fn todo_err() -> EngineError {
    EngineError::protocol("not implemented yet")
}

impl DockerEngine {
    pub(crate) async fn list_containers_impl(
        &self,
        _q: ContainerQuery,
    ) -> EngineResult<Vec<ContainerSummary>> {
        Err(todo_err())
    }
    pub(crate) async fn inspect_container_impl(&self, _id: &str) -> EngineResult<ContainerDetails> {
        Err(todo_err())
    }
    pub(crate) async fn container_action_impl(
        &self,
        _id: &str,
        _action: ContainerAction,
    ) -> EngineResult<()> {
        Err(todo_err())
    }
    pub(crate) async fn remove_container_impl(
        &self,
        _id: &str,
        _opts: RemoveContainerOpts,
    ) -> EngineResult<()> {
        Err(todo_err())
    }
    pub(crate) async fn prune_containers_impl(&self) -> EngineResult<PruneReport> {
        Err(todo_err())
    }
    pub(crate) async fn top_impl(&self, _id: &str) -> EngineResult<ProcessList> {
        Err(todo_err())
    }
    pub(crate) async fn exec_impl(
        &self,
        _id: &str,
        _req: ExecRequest,
    ) -> EngineResult<Box<dyn TerminalSession>> {
        Err(todo_err())
    }
    pub(crate) fn logs_stream(&self, _id: &str, _opts: LogOpts) -> EngineStream<LogChunk> {
        error_stream(todo_err())
    }
    pub(crate) fn stats_stream(&self, _id: &str) -> EngineStream<StatsSample> {
        error_stream(todo_err())
    }
    pub(crate) fn events_stream(&self, _f: EventFilter) -> EngineStream<EngineEvent> {
        error_stream(todo_err())
    }
}
