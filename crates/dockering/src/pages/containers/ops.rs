//! Container operations on the hub (CON-013, CON-020…023): single actions, deletes,
//! group/bulk fan-out (stages from `grouping::start_order`, parallel limit 4, one summary).
//! Pure async functions: the page spawns them and reports through notifications.

use dk_core::grouping::start_order;
use dk_core::{
    ContainerAction, ContainerSummary, EngineError, EngineId, PruneReport, RemoveContainerOpts,
};
use dk_hub::{HubCall, HubHandle};
use futures::StreamExt;

/// Parallelism for group/bulk operations (spec 21 §4 rule 5).
pub const PARALLEL: usize = 4;

/// What to do to a set of containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Kill,
    Remove { force: bool },
}

impl Op {
    pub fn verb(&self) -> &'static str {
        match self {
            Op::Start => "start",
            Op::Stop => "stop",
            Op::Restart => "restart",
            Op::Pause => "pause",
            Op::Unpause => "unpause",
            Op::Kill => "kill",
            Op::Remove { .. } => "delete",
        }
    }

    fn action(&self) -> Option<ContainerAction> {
        Some(match self {
            Op::Start => ContainerAction::Start,
            Op::Stop => ContainerAction::Stop { timeout_s: None },
            Op::Restart => ContainerAction::Restart { timeout_s: None },
            Op::Pause => ContainerAction::Pause,
            Op::Unpause => ContainerAction::Unpause,
            Op::Kill => ContainerAction::Kill { signal: None },
            Op::Remove { .. } => return None,
        })
    }
}

/// Per-container outcome: `(id, name, result)`.
pub type Outcome = Vec<(String, String, Result<(), EngineError>)>;

/// One hub call for the whole fan-out; the engine runs on the hub runtime.
///
/// Ordering (CON-013): *Start* (and the start half of *Restart*) runs in compose dependency
/// stages, *Stop* in reverse stages, each stage in parallel (limit 4). Others: one stage.
pub fn run(
    hub: &HubHandle,
    engine: &EngineId,
    op: Op,
    targets: Vec<ContainerSummary>,
) -> HubCall<Outcome> {
    let stages: Vec<Vec<ContainerSummary>> = {
        let refs: Vec<&ContainerSummary> = targets.iter().collect();
        let order = start_order(&refs);
        let staged: Vec<Vec<ContainerSummary>> = order
            .into_iter()
            .map(|st| {
                st.into_iter()
                    .filter_map(|i| targets.get(i).cloned())
                    .collect()
            })
            .filter(|st: &Vec<ContainerSummary>| !st.is_empty())
            .collect();
        if staged.is_empty() {
            vec![targets.clone()]
        } else {
            staged
        }
    };
    hub.call(engine, move |e| async move {
        let mut out: Outcome = Vec::new();
        let apply = |op: Op, stage: Vec<ContainerSummary>| {
            let e = e.clone();
            async move {
                futures::stream::iter(stage)
                    .map(|c| {
                        let e = e.clone();
                        let op = op.clone();
                        async move {
                            let r = match (&op, op.action()) {
                                (Op::Remove { force }, _) => {
                                    e.remove_container(
                                        &c.id,
                                        RemoveContainerOpts {
                                            force: *force,
                                            volumes: false,
                                        },
                                    )
                                    .await
                                }
                                (_, Some(action)) => e.container_action(&c.id, action).await,
                                _ => Ok(()),
                            };
                            (c.id, c.name, r)
                        }
                    })
                    .buffer_unordered(PARALLEL)
                    .collect::<Vec<_>>()
                    .await
            }
        };
        match op {
            Op::Start => {
                for stage in stages {
                    out.extend(apply(Op::Start, stage).await);
                }
            }
            Op::Stop => {
                for stage in stages.into_iter().rev() {
                    out.extend(apply(Op::Stop, stage).await);
                }
            }
            Op::Restart => {
                // Restart all = stop all (reverse) + start all (CON-013).
                let mut failed = Vec::new();
                for stage in stages.iter().rev() {
                    for r in apply(Op::Stop, stage.clone()).await {
                        if r.2.is_err() {
                            failed.push(r);
                        }
                    }
                }
                // The start result decides; a stop failure only matters if start also failed.
                for stage in stages {
                    out.extend(apply(Op::Start, stage).await);
                }
                for f in failed {
                    if let Some(o) = out.iter_mut().find(|o| o.0 == f.0)
                        && o.2.is_err()
                    {
                        o.2 = f.2;
                    }
                }
            }
            other => {
                let all: Vec<ContainerSummary> = stages.into_iter().flatten().collect();
                out.extend(apply(other, all).await);
            }
        }
        Ok(out)
    })
}

/// CON-023
pub fn prune(hub: &HubHandle, engine: &EngineId) -> HubCall<PruneReport> {
    hub.call(engine, |e| async move { e.prune_containers().await })
}

/// Summarises an outcome into `(ok, failed)` counts and the first error text.
pub fn summarize(outcome: &Outcome) -> (usize, Vec<(String, String)>) {
    let ok = outcome.iter().filter(|o| o.2.is_ok()).count();
    let failed = outcome
        .iter()
        .filter_map(|o| o.2.as_ref().err().map(|e| (o.1.clone(), e.to_string())))
        .collect();
    (ok, failed)
}
