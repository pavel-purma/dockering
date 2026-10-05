//! Connection-local COM-first routing. No error-string classification or mutation replay.
use crate::{
    cli::WslcCliEngine,
    com::{
        WslcComEngine,
        dispatch::{self, DispatchPhase},
    },
};
use async_trait::async_trait;
use dk_core::*;
use futures::{StreamExt, future::BoxFuture};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct WslcEngine {
    inner: Arc<Inner>,
}
struct Inner {
    id: EngineId,
    com: Option<Arc<WslcComEngine>>,
    strict: bool,
    cli: tokio::sync::OnceCell<Arc<dyn Engine>>,
    version: Option<String>,
    routes: Mutex<BTreeSet<&'static str>>,
    drains: Mutex<BTreeMap<&'static str, Vec<dispatch::Evidence>>>,
}
impl WslcEngine {
    pub(crate) fn native(com: WslcComEngine, strict: bool, version: Option<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                id: com.id().clone(),
                com: Some(Arc::new(com)),
                strict,
                cli: tokio::sync::OnceCell::new(),
                version,
                routes: Mutex::new(BTreeSet::new()),
                drains: Mutex::new(BTreeMap::new()),
            }),
        }
    }
    pub(crate) fn cli(cli: WslcCliEngine) -> Self {
        Self {
            inner: Arc::new(Inner {
                id: cli.id().clone(),
                com: None,
                strict: false,
                cli: tokio::sync::OnceCell::new_with(Some(Arc::new(cli))),
                version: None,
                routes: Mutex::new(BTreeSet::new()),
                drains: Mutex::new(BTreeMap::new()),
            }),
        }
    }
    fn routes(&self) -> BTreeSet<&'static str> {
        self.inner
            .routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn sticky(&self, op: &'static str) {
        self.inner
            .routes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(op);
    }
    fn use_cli(&self, op: &str) -> bool {
        self.inner.com.is_none()
            || (!self.inner.strict && (op == "run_image" || self.routes().contains(op)))
    }
    async fn cli_delegate(&self) -> EngineResult<Arc<dyn Engine>> {
        if self.inner.strict {
            return Err(EngineError::protocol("strict COM prohibits CLI"));
        }
        let session = if let Some(com) = &self.inner.com {
            let expected = com.resolved_session().ok_or_else(|| {
                EngineError::unreachable_with_hint(
                    "WSLC exact session binding unavailable",
                    "Use COM-only or reconnect explicitly; no CLI operation was dispatched.",
                )
            })?;
            let actual = com.validate_target().await?;
            if actual != expected {
                return Err(EngineError::unreachable("WSLC target changed"));
            }
            Some(actual.name)
        } else {
            None
        };
        let cli = self
            .inner
            .cli
            .get_or_try_init(|| async {
                let guard = self.inner.com.as_ref().map(|com| {
                    let com = com.clone();
                    Arc::new(move || {
                        let com = com.clone();
                        Box::pin(async move { com.validate_target().await.map(|_| ()) })
                            as BoxFuture<'static, EngineResult<()>>
                    }) as crate::cli::runner::SpawnGuard
                });
                WslcCliEngine::connect_guarded(
                    self.inner.id.clone(),
                    session,
                    self.inner.version.clone(),
                    None,
                    guard,
                )
                .await
                .map(|e| Arc::new(e) as Arc<dyn Engine>)
            })
            .await
            .cloned()?;
        // Lazy preparation may itself take time (version/session health probes). Validate
        // again at the actual crossing, including cached-delegate calls.
        if let Some(com) = &self.inner.com {
            com.validate_target().await?;
        }
        Ok(cli)
    }
    async fn stream_cli(&self, op: &'static str) -> EngineResult<Arc<dyn Engine>> {
        let old = self
            .inner
            .drains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(op)
            .cloned();
        if let Some(old) = old {
            for source in old {
                source.drain().await?;
            }
            // Retaining completed evidence is inexpensive and avoids removing a newer
            // concurrent subscription's guard. This connection has finitely many ops.
        }
        self.cli_delegate().await
    }
    async fn call<T: Send + 'static>(
        &self,
        op: &'static str,
        mutation: bool,
        parity: bool,
        call: impl Fn(Arc<dyn Engine>) -> BoxFuture<'static, EngineResult<T>> + Send + Sync,
    ) -> EngineResult<T> {
        self.call_evidenced(op, mutation, parity, call).await.0
    }
    async fn call_evidenced<T: Send + 'static>(
        &self,
        op: &'static str,
        mutation: bool,
        parity: bool,
        call: impl Fn(Arc<dyn Engine>) -> BoxFuture<'static, EngineResult<T>> + Send + Sync,
    ) -> (EngineResult<T>, Option<dispatch::TransportFailure>) {
        self.call_inner(op, mutation, parity, &call).await
    }
    async fn call_inner<T: Send + 'static>(
        &self,
        op: &'static str,
        mutation: bool,
        parity: bool,
        call: &(impl Fn(Arc<dyn Engine>) -> BoxFuture<'static, EngineResult<T>> + Send + Sync),
    ) -> (EngineResult<T>, Option<dispatch::TransportFailure>) {
        if self.use_cli(op) {
            if !parity && self.inner.com.is_some() {
                return (Err(parity_error(op)), None);
            }
            let cli = match self.cli_delegate().await {
                Ok(cli) => cli,
                Err(e) => return (Err(e), None),
            };
            return (call(cli).await, None);
        }
        let com = self.inner.com.as_ref().expect("native route");
        let (mut result, mut fault, phase) =
            dispatch::capture_with_phase(call(com.clone()), mutation).await;
        if result.is_ok() || fault.is_none() || (mutation && self.inner.strict) {
            return (phase_outcome(result, mutation, phase), fault);
        }
        let initial = result.as_ref().err().expect("native failure").clone();
        if mutation {
            if fault.is_some_and(|f| f.phase != DispatchPhase::NotDispatched) {
                return (result, fault);
            }
        } else {
            let (reopened, reopen_fault) = dispatch::capture(com.reopen(), false).await;
            if reopened.is_ok() {
                (result, fault) = dispatch::capture(call(com.clone()), false).await;
                result = result.map_err(|e| native_retry_error(&initial, e));
                if result.is_ok() || fault.is_none() {
                    return (result, fault);
                }
            } else if reopen_fault.is_none() {
                return (
                    Err(native_retry_error(
                        &initial,
                        reopened.expect_err("failed reopen"),
                    )),
                    None,
                );
            }
        }
        if self.inner.strict {
            return (result, fault);
        }
        if !parity {
            return (Err(parity_error(op)), fault);
        }
        self.sticky(op);
        let native = result.err().expect("native fallback failure");
        let cli = match self.cli_delegate().await {
            Ok(cli) => cli,
            Err(e) => return (Err(fallback_error(&native, e)), None),
        };
        (
            call(cli).await.map_err(|e| fallback_error(&native, e)),
            None,
        )
    }
    fn stream<T: Send + 'static>(
        &self,
        op: &'static str,
        mutation: bool,
        parity: bool,
        make: impl Fn(Arc<dyn Engine>) -> EngineStream<T> + Send + Sync + 'static,
    ) -> EngineStream<T> {
        let router = self.clone();
        let make = Arc::new(make);
        Box::pin(futures::stream::unfold(
            (
                router,
                make,
                None::<EngineStream<T>>,
                None::<dispatch::Evidence>,
                false,
                false,
                None::<EngineError>,
            ),
            move |(
                router,
                make,
                mut source,
                mut evidence,
                mut emitted,
                mut ended,
                mut native_error,
            )| async move {
                if ended {
                    return None;
                }
                if source.is_none() {
                    if router.use_cli(op) {
                        let delegate = if !parity && router.inner.com.is_some() {
                            Err(parity_error(op))
                        } else {
                            router.stream_cli(op).await
                        };
                        match delegate {
                            Ok(e) => source = Some(make(e)),
                            Err(e) => {
                                return Some((
                                    Err(e),
                                    (router, make, source, evidence, emitted, true, native_error),
                                ));
                            }
                        }
                    } else {
                        let (s, ev) = dispatch::capture_stream(|| {
                            make(router.inner.com.as_ref().expect("native route").clone())
                        });
                        source = Some(s);
                        evidence = Some(ev);
                    }
                }
                loop {
                    let mut item = source.as_mut().expect("source").next().await?;
                    if let Err(ref error) = item {
                        let fault = evidence.as_ref().and_then(|e| e.failure());
                        if fault.is_some() {
                            let before = !emitted
                                && evidence.as_ref().is_some_and(|e| e.source_items() == 0);
                            let safe = !mutation
                                || fault.is_some_and(|f| f.phase == DispatchPhase::NotDispatched);
                            if !router.inner.strict && parity {
                                router.sticky(op);
                                if let Some(old) = &evidence {
                                    let mut drains = router
                                        .inner
                                        .drains
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    let pending = drains.entry(op).or_default();
                                    pending.retain(|e| !e.is_drained());
                                    pending.push(old.clone());
                                }
                            }
                            if before && safe && !router.inner.strict && parity {
                                native_error = Some(error.clone());
                                // Drop cancels the old producer before creating the replacement.
                                source.take();
                                if let Some(old) = evidence.take()
                                    && let Err(e) = old.drain().await
                                {
                                    return Some((
                                        Err(fallback_error(
                                            native_error.as_ref().expect("native error"),
                                            e,
                                        )),
                                        (
                                            router,
                                            make,
                                            source,
                                            evidence,
                                            emitted,
                                            true,
                                            native_error,
                                        ),
                                    ));
                                }
                                match router.stream_cli(op).await {
                                    Ok(e) => {
                                        source = Some(make(e));
                                        continue;
                                    }
                                    Err(e) => {
                                        return Some((
                                            Err(fallback_error(
                                                native_error.as_ref().expect("native error"),
                                                e,
                                            )),
                                            (
                                                router,
                                                make,
                                                source,
                                                evidence,
                                                emitted,
                                                true,
                                                native_error,
                                            ),
                                        ));
                                    }
                                }
                            }
                            ended = true;
                            if mutation && !safe {
                                item = Err(unknown());
                            }
                        } else if !matches!(error, EngineError::Protocol(s) if s == "events lost") {
                            ended = true;
                            if mutation
                                && matches!(error, EngineError::Timeout(_) | EngineError::Cancelled)
                                && evidence
                                    .as_ref()
                                    .is_some_and(|e| e.phase() != DispatchPhase::NotDispatched)
                            {
                                item = Err(unknown());
                            }
                        }
                    } else {
                        emitted = true;
                    }
                    if ended {
                        source.take();
                    }
                    if evidence.is_none()
                        && let (Some(native), Err(cli)) = (&native_error, &item)
                    {
                        item = Err(fallback_error(native, cli.clone()));
                    }
                    return Some((
                        item,
                        (router, make, source, evidence, emitted, ended, native_error),
                    ));
                }
            },
        ))
    }
    async fn reconcile<T>(&self, read: impl Future<Output = EngineResult<T>>) -> EngineResult<T> {
        tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .map_err(|_| unknown())?
    }
}
fn parity_error(op: &str) -> EngineError {
    EngineError::unreachable_with_hint(
        format!("WSLC CLI option parity is not verified for {op}"),
        "Use COM for this request; no CLI mutation was dispatched.",
    )
}
fn unknown() -> EngineError {
    EngineError::unreachable_with_hint(
        "WSLC operation outcome is unknown",
        "The operation may have completed. Refresh before manually retrying; do not resubmit automatically.",
    )
}
fn cli_outcome<T>(r: EngineResult<T>, mutation: bool) -> EngineResult<T> {
    match r {
        Err(EngineError::Timeout(_) | EngineError::Cancelled | EngineError::Protocol(_))
            if mutation =>
        {
            Err(unknown())
        }
        other => other,
    }
}
fn phase_outcome<T>(r: EngineResult<T>, mutation: bool, phase: DispatchPhase) -> EngineResult<T> {
    if mutation && phase != DispatchPhase::NotDispatched {
        cli_outcome(r, true)
    } else {
        r
    }
}

pub(crate) fn fallback_error(native: &EngineError, cli: EngineError) -> EngineError {
    // No request payload, stdout, environment or credentials is included here.
    let context = format!("Native COM: {native}; CLI: {cli}");
    match cli {
        EngineError::Unreachable { hint, .. } => EngineError::Unreachable {
            reason: context,
            hint,
        },
        EngineError::Api { status, .. } => EngineError::Api {
            status,
            message: context,
        },
        EngineError::Protocol(_) => EngineError::Protocol(context),
        EngineError::Conflict(_) => EngineError::Conflict(context),
        other => other,
    }
}
fn native_retry_error(first: &EngineError, retry: EngineError) -> EngineError {
    let context = format!("COM first attempt: {first}; same-target reopen/read: {retry}");
    match retry {
        EngineError::Unreachable { hint, .. } => EngineError::Unreachable {
            reason: context,
            hint,
        },
        EngineError::Api { status, .. } => EngineError::Api {
            status,
            message: context,
        },
        EngineError::Protocol(_) => EngineError::Protocol(context),
        EngineError::Conflict(_) => EngineError::Conflict(context),
        other => other,
    }
}

macro_rules! routed {
    ($name:ident($($arg:ident: $ty:ty),*) -> $out:ty, $mutation:expr, $parity:expr) => {
        fn $name<'life0, 'async_trait>(&'life0 self, $($arg: $ty),*) -> BoxFuture<'async_trait, EngineResult<$out>> where 'life0: 'async_trait, Self: 'async_trait {
            Box::pin(async move {
            $(let $arg = $arg.to_owned();)*
            self.call(stringify!($name), $mutation, $parity, move |e| {
                $(let $arg = $arg.clone();)*
                Box::pin(async move { e.$name($($arg),*).await })
            }).await })
        }
    };
}
// Borrowed arguments need their owned storage held inside each future.
macro_rules! by_id {
    ($name:ident -> $out:ty, $mutation:expr, $($arg:ident: $ty:ty),*) => {
        fn $name<'life0, 'life1, 'async_trait>(&'life0 self, id: &'life1 str, $($arg: $ty),*) -> BoxFuture<'async_trait, EngineResult<$out>> where 'life0: 'async_trait, 'life1: 'async_trait, Self: 'async_trait {
            Box::pin(async move {
            let id = id.to_owned();
            self.call(stringify!($name), $mutation, true, move |e| {
                let id = id.clone(); $(let $arg = $arg.clone();)*
                Box::pin(async move { e.$name(&id, $($arg),*).await })
            }).await })
        }
    };
}
#[async_trait]
impl Engine for WslcEngine {
    fn id(&self) -> &EngineId {
        &self.inner.id
    }
    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }
    fn capabilities(&self) -> Capabilities {
        let mut c = self
            .inner
            .com
            .as_ref()
            .map_or(crate::cli::CLI_CAPABILITIES, |e| e.capabilities());
        if self.routes().contains("pull_image") {
            c.remove(Capabilities::PULL_PROGRESS);
        }
        c
    }
    routed!(ping() -> (), false, true);
    async fn info(&self) -> EngineResult<EngineInfo> {
        let mut info = self
            .call("info", false, true, |e| {
                Box::pin(async move { e.info().await })
            })
            .await?;
        let routes = self.routes();
        if let Some(com) = &self.inner.com {
            info.transport = Some("com".into());
            let mut caps = com.capabilities();
            if routes.contains("pull_image") {
                caps.remove(Capabilities::PULL_PROGRESS);
            }
            info.capabilities = caps;
            info.list_stats_limit = if routes.contains("stats") { 0 } else { 20 };
            info.transport_note = if self.inner.strict {
                Some("COM only — Run unavailable".to_owned())
            } else if routes.is_empty() {
                None
            } else {
                const SHOWN: usize = 3;
                let mut ops: Vec<String> = routes
                    .iter()
                    .take(SHOWN)
                    .map(|op| (*op).to_owned())
                    .collect();
                if routes.len() > SHOWN {
                    ops.push(format!("+{} more", routes.len() - SHOWN));
                }
                Some(format!("CLI fallback: {}", ops.join(", ")))
            };
        }
        Ok(info)
    }
    fn events(&self, f: EventFilter) -> EngineStream<EngineEvent> {
        self.stream("events", false, true, move |e| e.events(f.clone()))
    }
    routed!(list_containers(q: ContainerQuery) -> Vec<ContainerSummary>, false, true);
    by_id!(inspect_container -> ContainerDetails, false,);
    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        // Resolve names/prefixes before mutation; reconciliation only accepts the same full ID.
        let full = match &action {
            ContainerAction::Start | ContainerAction::Stop { .. } => {
                Some(self.inspect_container(id).await?.summary.id)
            }
            _ => None,
        };
        let target = full.as_deref().unwrap_or(id).to_owned();
        let a = action.clone();
        let (result, failure) = self
            .call_evidenced("container_action", true, true, move |e| {
                let id = target.clone();
                let a = a.clone();
                Box::pin(async move { e.container_action(&id, a).await })
            })
            .await;
        if result.is_err()
            && failure.is_some_and(|f| f.phase == DispatchPhase::MayHaveDispatched)
            && let Some(full) = full
        {
            let read = self.reconcile(self.inspect_container(&full)).await;
            if let Ok(d) = read {
                let matches = d.summary.id == full
                    && match action {
                        ContainerAction::Start => d.summary.state.is_running(),
                        ContainerAction::Stop { .. } => d.summary.state == ContainerState::Exited,
                        _ => false,
                    };
                if matches {
                    return Ok(());
                }
            }
        }
        result
    }
    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        let full = self.inspect_container(id).await?.summary.id;
        let target = full.clone();
        let (result, failure) = self
            .call_evidenced("remove_container", true, true, move |e| {
                let id = target.clone();
                Box::pin(async move { e.remove_container(&id, opts).await })
            })
            .await;
        if !opts.volumes
            && result.is_err()
            && failure.is_some_and(|f| f.phase == DispatchPhase::MayHaveDispatched)
            && matches!(
                self.reconcile(self.inspect_container(&full)).await,
                Err(EngineError::NotFound {
                    kind: ResourceKind::Container,
                    ..
                })
            )
        {
            return Ok(());
        }
        result
    }
    routed!(prune_containers() -> PruneReport, true, true);
    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        let id = id.to_owned();
        self.stream("logs", false, true, move |e| e.logs(&id, opts.clone()))
    }
    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        let id = id.to_owned();
        self.stream("stats", false, true, move |e| e.stats(&id))
    }
    async fn top(&self, _: &str) -> EngineResult<ProcessList> {
        Err(EngineError::Unsupported(Capabilities::TOP))
    }
    by_id!(exec -> Box<dyn TerminalSession>, true, req: ExecRequest);
    routed!(list_images() -> Vec<ImageSummary>, false, true);
    by_id!(inspect_image -> ImageDetails, false,);
    async fn image_history(&self, _: &str) -> EngineResult<Vec<ImageLayer>> {
        Err(EngineError::Unsupported(Capabilities::IMAGE_HISTORY))
    }
    fn pull_image(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        let reference = reference.to_owned();
        let parity = auth.is_none();
        self.stream("pull_image", true, parity, move |e| {
            e.pull_image(&reference, auth.clone())
        })
    }
    by_id!(remove_image -> Vec<ImageDeleteItem>, true, force: bool);
    routed!(prune_images(dangling_only: bool) -> PruneReport, true, true);
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        let (id, repo, tag) = (id.to_owned(), repo.to_owned(), tag.to_owned());
        self.call("tag_image", true, true, move |e| {
            let (id, repo, tag) = (id.clone(), repo.clone(), tag.clone());
            Box::pin(async move { e.tag_image(&id, &repo, &tag).await })
        })
        .await
    }
    routed!(run_image(spec: RunSpec) -> String, true, true);
    routed!(list_volumes() -> Vec<VolumeSummary>, false, true);
    by_id!(inspect_volume -> VolumeDetails, false,);
    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        if self.use_cli("create_volume") {
            if self.inner.com.is_some() && spec.driver.as_deref() == Some("local") {
                return Err(parity_error("create_volume driver"));
            }
            return self.cli_delegate().await?.create_volume(spec).await;
        }
        let com = self.inner.com.as_ref().expect("native route");
        let (name, fault, phase) =
            dispatch::capture_with_phase(com.create_volume_raw(spec.clone()), true).await;
        match name {
            Ok(name) => self
                .reconcile(self.inspect_volume(&name))
                .await
                .map(|d| d.summary)
                .map_err(|e| {
                    EngineError::unreachable_with_hint(
                        format!(
                            "Volume {name} was created, but its details could not be read: {e}"
                        ),
                        "Refresh the volume list; do not create the volume again.",
                    )
                }),
            Err(_e)
                if !self.inner.strict
                    && fault.is_some_and(|f| f.phase == DispatchPhase::NotDispatched) =>
            {
                if spec.driver.as_deref().is_some_and(|d| d == "local") {
                    return Err(parity_error("create_volume driver"));
                }
                self.sticky("create_volume");
                self.cli_delegate().await?.create_volume(spec).await
            }
            Err(e) => phase_outcome(Err(e), true, phase),
        }
    }
    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        let target = name.to_owned();
        let (result, failure) = self
            .call_evidenced("remove_volume", true, true, move |e| {
                let name = target.clone();
                Box::pin(async move { e.remove_volume(&name, force).await })
            })
            .await;
        if result.is_err()
            && failure.is_some_and(|f| f.phase == DispatchPhase::MayHaveDispatched)
            && matches!(
                self.reconcile(self.inspect_volume(name)).await,
                Err(EngineError::NotFound {
                    kind: ResourceKind::Volume,
                    ..
                })
            )
        {
            return Ok(());
        }
        result
    }
    routed!(prune_volumes() -> PruneReport, true, false);
    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
    }
    routed!(list_networks() -> Vec<NetworkSummary>, false, true);
    by_id!(inspect_network -> NetworkDetails, false,);
    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        let full = self.inspect_network(id).await?.summary.id;
        let target = full.clone();
        let (result, failure) = self
            .call_evidenced("remove_network", true, true, move |e| {
                let id = target.clone();
                Box::pin(async move { e.remove_network(&id).await })
            })
            .await;
        if result.is_err()
            && failure.is_some_and(|f| f.phase == DispatchPhase::MayHaveDispatched)
            && matches!(
                self.reconcile(self.inspect_network(&full)).await,
                Err(EngineError::NotFound {
                    kind: ResourceKind::Network,
                    ..
                })
            )
        {
            return Ok(());
        }
        result
    }
    routed!(prune_networks() -> PruneReport, true, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::{
        fake::{FakeManager, FakeState, Shared},
        ffi::hr,
    };
    use dk_core::fake::FakeEngine;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn native(strict: bool) -> (WslcEngine, Shared, Arc<FakeEngine>) {
        let state = Arc::new(Mutex::new(FakeState {
            sessions: vec![(7, "exact session".into())],
            default_session: "exact session".into(),
            version: (3, 0, 1),
            volumes_json: "[]".into(),
            networks_json: "[]".into(),
            ..Default::default()
        }));
        let com = WslcComEngine::from_manager_for_tests(
            EngineId::new("router-test"),
            FakeManager::new_interface(state.clone()),
            None,
        );
        com.self_check(None).await.expect("binding");
        let router = WslcEngine::native(com, strict, None);
        let cli = FakeEngine::new("router-test");
        assert!(router.inner.cli.set(cli.clone()).is_ok());
        state.lock().expect("state").calls.clear();
        (router, state, cli)
    }

    #[tokio::test]
    async fn eng_126_strict_preferences_and_eng_128_run_exception() {
        for strict in [false, true] {
            let (r, state, cli) = native(strict).await;
            let result = r
                .run_image(RunSpec {
                    image: "hello-world".into(),
                    ..Default::default()
                })
                .await;
            assert_eq!(cli.calls_to("run_image").len(), usize::from(!strict));
            assert!(
                !state
                    .lock()
                    .expect("state")
                    .calls
                    .iter()
                    .any(|s| s == "CreateContainer")
            );
            if strict {
                assert!(matches!(result, Err(EngineError::Api { status: 501, .. })));
            } else {
                assert!(result.is_ok());
            }
            r.ping().await.expect("COM ping");
            assert_eq!(cli.calls_to("ping").len(), 0);
        }
    }

    #[tokio::test]
    async fn eng_127_validate_before_each_crossing_replacement_refuses() {
        let (r, state, cli) = native(false).await;
        state.lock().expect("state").sessions[0].0 = 8;
        assert!(
            r.run_image(RunSpec {
                image: "hello-world".into(),
                ..Default::default()
            })
            .await
            .is_err()
        );
        assert!(cli.calls().is_empty());
    }

    #[tokio::test]
    async fn eng_129_allowlist_budget_and_sticky_routes() {
        for hr in [
            hr::RPC_E_DISCONNECTED,
            hr::RPC_S_SERVER_UNAVAILABLE,
            hr::RPC_E_SERVER_DIED,
            hr::RPC_E_SERVER_DIED_DNE,
            hr::RPC_S_CALL_FAILED,
        ] {
            let (r, state, _) = native(false).await;
            let native_count = Arc::new(AtomicUsize::new(0));
            let cli_count = Arc::new(AtomicUsize::new(0));
            for _ in 0..2 {
                let (n, c) = (native_count.clone(), cli_count.clone());
                r.call("list_images", false, true, move |e| {
                    let (n, c) = (n.clone(), c.clone());
                    Box::pin(async move {
                        if e.kind() == EngineKind::Wslc {
                            n.fetch_add(1, Ordering::SeqCst);
                            dispatch::record(hr);
                            Err(EngineError::unreachable("injected"))
                        } else {
                            c.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                })
                .await
                .expect("CLI read");
            }
            assert_eq!(native_count.load(Ordering::SeqCst), 2);
            assert_eq!(cli_count.load(Ordering::SeqCst), 2);
            assert_eq!(
                state
                    .lock()
                    .expect("state")
                    .calls
                    .iter()
                    .filter(|s| *s == "OpenSessionByName")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn eng_129_domain_policy_protocol_cancel_never_fallback() {
        for hr in [
            hr::E_ACCESSDENIED,
            hr::E_INVALIDARG,
            hr::E_ABORT,
            hr::E_FAIL,
            hr::WSLC_E_CONTAINER_DISABLED,
            hr::WSLC_E_SESSION_NOT_FOUND,
            hr::WSLC_E_REGISTRY_BLOCKED_BY_POLICY,
        ] {
            let (r, state, cli) = native(false).await;
            state
                .lock()
                .expect("state")
                .fail_next
                .insert("GetState".into(), hr);
            assert!(r.ping().await.is_err());
            assert!(cli.calls().is_empty());
            assert!(!r.use_cli("ping"));
            assert_eq!(
                state
                    .lock()
                    .expect("state")
                    .calls
                    .iter()
                    .filter(|s| *s == "GetState")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn eng_130_mutation_phase_matrix_one_dispatch_and_parity_gate() {
        for phase in [
            DispatchPhase::NotDispatched,
            DispatchPhase::MayHaveDispatched,
            DispatchPhase::Completed,
        ] {
            for parity in [true, false] {
                let (r, _, _) = native(false).await;
                let count = Arc::new(AtomicUsize::new(0));
                let cli_count = Arc::new(AtomicUsize::new(0));
                let (n, c) = (count.clone(), cli_count.clone());
                let result = r
                    .call("prune_volumes", true, parity, move |e| {
                        let (n, c) = (n.clone(), c.clone());
                        Box::pin(async move {
                            if e.kind() == EngineKind::Wslc {
                                n.fetch_add(1, Ordering::SeqCst);
                                dispatch::phase(phase);
                                dispatch::record(hr::RPC_S_CALL_FAILED);
                                Err(EngineError::unreachable("injected"))
                            } else {
                                c.fetch_add(1, Ordering::SeqCst);
                                Ok(())
                            }
                        })
                    })
                    .await;
                assert_eq!(count.load(Ordering::SeqCst), 1);
                let fallback = parity && phase == DispatchPhase::NotDispatched;
                assert_eq!(cli_count.load(Ordering::SeqCst), usize::from(fallback));
                assert_eq!(result.is_ok(), fallback);
                if phase != DispatchPhase::NotDispatched {
                    assert!(
                        result
                            .expect_err("ambiguous")
                            .hint()
                            .expect("hint")
                            .contains("Refresh")
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn eng_131_source_boundary_clean_eof_and_pull_dispatch() {
        for (items, mutation, phase, fault) in [
            (0, false, DispatchPhase::NotDispatched, true),
            (1, false, DispatchPhase::NotDispatched, true),
            (0, true, DispatchPhase::MayHaveDispatched, true),
            (0, true, DispatchPhase::NotDispatched, true),
            (0, false, DispatchPhase::NotDispatched, false),
        ] {
            let (r, _, _) = native(false).await;
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let stream = r.stream("events", mutation, true, move |e| {
                c.fetch_add(1, Ordering::SeqCst);
                if e.kind() != EngineKind::Wslc {
                    return Box::pin(futures::stream::iter(vec![Ok(42)]));
                }
                for _ in 0..items {
                    dispatch::source_item();
                }
                dispatch::phase(phase);
                if fault {
                    dispatch::record(hr::RPC_E_DISCONNECTED);
                    Box::pin(futures::stream::iter(vec![Err(EngineError::unreachable(
                        "fault",
                    ))]))
                } else {
                    Box::pin(futures::stream::empty())
                }
            });
            let results: Vec<_> = stream.collect().await;
            let fallback =
                fault && items == 0 && (!mutation || phase == DispatchPhase::NotDispatched);
            assert_eq!(count.load(Ordering::SeqCst), 1 + usize::from(fallback));
            if fallback {
                assert_eq!(results, vec![Ok(42)]);
            } else if fault {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_err());
            } else {
                assert!(results.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn eng_136_coherent_metadata_no_last_call_demotion() {
        let (r, _, _) = native(false).await;
        r.run_image(RunSpec {
            image: "hello-world".into(),
            ..Default::default()
        })
        .await
        .expect("Run");
        let info = r.info().await.expect("info");
        assert_eq!(info.transport.as_deref(), Some("com"));
        assert_eq!(info.list_stats_limit, 20);
        assert!(info.capabilities.contains(Capabilities::PULL_PROGRESS));
        r.sticky("stats");
        r.sticky("pull_image");
        let info = r.info().await.expect("info");
        assert_eq!(info.list_stats_limit, 0);
        assert!(!info.capabilities.contains(Capabilities::PULL_PROGRESS));
        assert_eq!(info.capabilities, r.capabilities());
    }

    #[tokio::test]
    async fn eng_136_note_only_when_notable() {
        let (r, _, _) = native(false).await;
        assert_eq!(r.info().await.expect("info").transport_note, None);
        r.run_image(RunSpec {
            image: "hello-world".into(),
            ..Default::default()
        })
        .await
        .expect("Run");
        assert_eq!(r.info().await.expect("info").transport_note, None);
        r.sticky("stats");
        r.sticky("pull_image");
        assert_eq!(
            r.info().await.expect("info").transport_note.as_deref(),
            Some("CLI fallback: pull_image, stats")
        );
        r.sticky("list_containers");
        r.sticky("logs");
        assert_eq!(
            r.info().await.expect("info").transport_note.as_deref(),
            Some("CLI fallback: list_containers, logs, pull_image, +1 more")
        );
        let (strict, _, _) = native(true).await;
        assert_eq!(
            strict.info().await.expect("info").transport_note.as_deref(),
            Some("COM only — Run unavailable")
        );
    }

    #[tokio::test]
    async fn eng_126_router_contract_suite() {
        let (r, _, _) = native(false).await;
        dk_core::contract::run_suite(Arc::new(r), Default::default())
            .await
            .assert_ok();
    }
    #[tokio::test]
    async fn eng_130_commit_then_disconnect_stop_one_mutation_full_id_read() {
        let (r, state, cli) = native(false).await;
        let id = "a".repeat(64);
        state.lock().expect("state").containers.push(crate::com::fake::FakeContainerData {
            id: id.clone(), name: "web".into(), state: 1,
            inspect_json: serde_json::json!({"Id":id,"Name":"/web","Config":{},"State":{"Status":"running","Running":true}}).to_string(),
            ..Default::default()
        });
        state
            .lock()
            .expect("state")
            .commit_fault
            .insert("Stop".into(), hr::RPC_S_CALL_FAILED);
        let error = r
            .container_action(&id, ContainerAction::Stop { timeout_s: None })
            .await
            .expect_err("static fake inspect cannot prove postcondition");
        assert!(error.hint().expect("hint").contains("Refresh"));
        let state = state.lock().expect("state");
        assert_eq!(state.stopped.len(), 1);
        assert_eq!(state.calls.iter().filter(|s| *s == "Stop").count(), 1);
        assert_eq!(
            state.calls.iter().filter(|s| *s == "OpenContainer").count(),
            3
        );
        assert!(cli.calls_to("container_action").is_empty());
    }

    #[tokio::test]
    async fn eng_126_missing_cli_preserves_com_and_strict_cli_no_com() {
        let (mut r, _, _) = native(false).await;
        Arc::get_mut(&mut r.inner).expect("sole owner").cli = tokio::sync::OnceCell::new();
        r.ping().await.expect("no CLI preparation for COM ping");
        assert!(r.inner.cli.get().is_none());
        let cli = FakeEngine::new("cli-only");
        let r = WslcEngine {
            inner: Arc::new(Inner {
                id: cli.id().clone(),
                com: None,
                strict: false,
                cli: tokio::sync::OnceCell::new_with(Some(cli.clone())),
                version: None,
                routes: Mutex::new(BTreeSet::new()),
                drains: Mutex::new(BTreeMap::new()),
            }),
        };
        r.ping().await.expect("CLI ping");
        assert_eq!(cli.calls_to("ping").len(), 1);
        assert!(r.inner.com.is_none());
    }

    #[tokio::test]
    async fn eng_130_completed_create_enrichment_failure_never_recreates() {
        let (r, state, cli) = native(false).await;
        state
            .lock()
            .expect("state")
            .fail_next
            .insert("InspectVolume".into(), hr::RPC_E_DISCONNECTED);
        let result = r
            .create_volume(VolumeSpec {
                name: Some("owned-test-volume".into()),
                ..Default::default()
            })
            .await;
        // Fake InspectVolume has no object; a final NotFound may not switch to CLI.
        assert!(result.is_err());
        assert_eq!(
            state.lock().expect("state").created_volumes,
            vec!["owned-test-volume"]
        );
        assert!(cli.calls_to("create_volume").is_empty());
    }

    #[tokio::test]
    async fn eng_129_full_trait_read_and_mutation_routing_groups() {
        // Each routing group is checked with a fault on both native attempts. Unsupported
        // operations are tested separately below; streams have their own boundary matrix.
        let reads = [
            "ping",
            "info",
            "list_containers",
            "inspect_container",
            "list_images",
            "inspect_image",
            "list_volumes",
            "inspect_volume",
            "list_networks",
            "inspect_network",
        ];
        let writes = [
            "container_action",
            "remove_container",
            "prune_containers",
            "exec",
            "remove_image",
            "prune_images",
            "tag_image",
            "create_volume",
            "remove_volume",
            "remove_network",
            "prune_networks",
        ];
        for (ops, mutation) in [(reads.as_slice(), false), (writes.as_slice(), true)] {
            for &op in ops {
                let (r, _, _) = native(false).await;
                let counts = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
                let c = counts.clone();
                r.call(op, mutation, true, move |e| {
                    let c = c.clone();
                    Box::pin(async move {
                        if e.kind() == EngineKind::Wslc {
                            c.0.fetch_add(1, Ordering::SeqCst);
                            dispatch::phase(DispatchPhase::NotDispatched);
                            dispatch::record(hr::RPC_E_DISCONNECTED);
                            Err(EngineError::unreachable("injected"))
                        } else {
                            c.1.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                })
                .await
                .expect("bounded fallback");
                assert_eq!(
                    counts.0.load(Ordering::SeqCst),
                    if mutation { 1 } else { 2 },
                    "{op}"
                );
                assert_eq!(counts.1.load(Ordering::SeqCst), 1, "{op}");
            }
        }
        let (r, state, cli) = native(false).await;
        assert_eq!(
            r.top("x").await,
            Err(EngineError::Unsupported(Capabilities::TOP))
        );
        assert_eq!(
            r.image_history("x").await,
            Err(EngineError::Unsupported(Capabilities::IMAGE_HISTORY))
        );
        assert_eq!(
            r.disk_usage().await,
            Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
        );
        assert_eq!(
            r.container_action("x", ContainerAction::Pause).await,
            Err(EngineError::Unsupported(Capabilities::PAUSE))
        );
        assert!(cli.calls().is_empty());
        assert!(state.lock().expect("state").calls.is_empty());
    }

    #[tokio::test]
    async fn eng_131_events_lost_continues_and_auth_never_discarded() {
        let (r, _, cli) = native(false).await;
        let s = r.stream("events", false, true, |_| {
            Box::pin(futures::stream::iter(vec![
                Err(EngineError::events_lost()),
                Ok(42),
            ]))
        });
        let items: Vec<_> = s.collect().await;
        assert_eq!(items.len(), 2);
        assert!(items[0].as_ref().expect_err("gap").is_events_lost());
        assert_eq!(items[1], Ok(42));
        r.sticky("pull_image");
        let auth = RegistryAuth {
            server: "registry.invalid".into(),
            username: None,
            password: None,
            identity_token: None,
        };
        assert!(
            r.pull_image("hello-world", Some(auth))
                .next()
                .await
                .expect("limitation")
                .is_err()
        );
        assert!(cli.calls_to("pull_image").is_empty());
    }

    async fn populated() -> (WslcEngine, Shared, Arc<FakeEngine>, String) {
        let (r, state, cli) = native(false).await;
        let samples = crate::com::fake::sample_state();
        state.lock().expect("state").containers =
            samples.lock().expect("samples").containers.clone();
        let id = state.lock().expect("state").containers[0].id.clone();
        (r, state, cli, id)
    }

    #[tokio::test]
    async fn eng_130_timeout_then_late_commit_is_one_mutation() {
        let (r, state, cli, id) = populated().await;
        state
            .lock()
            .expect("state")
            .delay_next
            .insert("Stop".into(), Duration::from_millis(120));
        // Wait until the protected RPC entered, then drop its waiter. A timed-out
        // client does not cancel server-side commit or authorize a replacement write.
        let task = tokio::spawn({
            let r = r.clone();
            let id = id.clone();
            async move {
                r.container_action(&id, ContainerAction::Stop { timeout_s: None })
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !state
                .lock()
                .expect("state")
                .calls
                .iter()
                .any(|s| s == "Stop")
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("RPC admission");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), async {
                while !task.is_finished() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .is_err()
        );
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.lock().expect("state").stopped.is_empty() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("late commit");
        assert_eq!(
            state
                .lock()
                .expect("state")
                .calls
                .iter()
                .filter(|s| *s == "Stop")
                .count(),
            1
        );
        assert_eq!(state.lock().expect("state").stopped.len(), 1);
        assert!(cli.calls_to("container_action").is_empty());
        assert!(!r.use_cli("container_action"));
    }

    #[tokio::test]
    async fn eng_131_exec_committed_getstdhandle_failure_never_reexecs() {
        for partial in [false, true] {
            let (r, state, cli, id) = populated().await;
            {
                let mut s = state.lock().expect("state");
                let faults = if partial {
                    &mut s.commit_fault
                } else {
                    &mut s.fail_next
                };
                faults.insert("GetStdHandle".into(), hr::RPC_E_DISCONNECTED);
            }
            let error = match r.exec(&id, ExecRequest::default()).await {
                Ok(_) => panic!("expected lost process handle"),
                Err(e) => e,
            };
            assert!(error.hint().expect("unknown").contains("Refresh"));
            let s = state.lock().expect("state");
            assert_eq!(s.calls.iter().filter(|c| *c == "Exec").count(), 1);
            assert_eq!(s.calls.iter().filter(|c| *c == "GetStdHandle").count(), 1);
            assert!(cli.calls_to("exec").is_empty());
        }
    }

    #[tokio::test]
    async fn eng_131_terminal_remains_pinned_after_future_exec_route_changes() {
        let (r, state, cli, id) = populated().await;
        let mut terminal = r.exec(&id, ExecRequest::default()).await.expect("terminal");
        r.sticky("exec");
        terminal.resize(120, 40).await.expect("native resize");
        let mut output = terminal.output();
        let write = terminal.write(bytes::Bytes::from(vec![b'x'; 64 * 1024]));
        let close = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            terminal.close().await
        };
        let (write, close) =
            tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(write, close) })
                .await
                .expect("bounded cancellation");
        assert!(write.is_err());
        close.expect("close");
        tokio::time::timeout(Duration::from_secs(2), async {
            while output.next().await.is_some() {}
        })
        .await
        .expect("output ends");
        tokio::time::timeout(Duration::from_secs(2), terminal.wait())
            .await
            .expect("wait deadline")
            .expect("native wait");
        assert!(cli.calls_to("exec").is_empty());
        let s = state.lock().expect("state");
        assert_eq!(s.calls.iter().filter(|c| *c == "Exec").count(), 1);
        assert!(s.calls.iter().any(|c| c == "ResizeTty"));
        assert!(!s.signalled.is_empty());
    }

    #[tokio::test]
    async fn eng_131_waits_for_source_drain_before_cli_subscription() {
        struct Guarded {
            dropped: Arc<std::sync::atomic::AtomicBool>,
            stream: EngineStream<i32>,
        }
        impl futures::Stream for Guarded {
            type Item = EngineResult<i32>;
            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Self::Item>> {
                self.stream.as_mut().poll_next(cx)
            }
        }
        impl Drop for Guarded {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }
        let (r, _, _) = native(false).await;
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drained = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (d, done) = (dropped.clone(), drained.clone());
        let stream = r.stream("logs", false, true, move |e| {
            if e.kind() == EngineKind::Wslc {
                let producer = dispatch::Evidence::current().producer();
                dispatch::record(hr::RPC_E_DISCONNECTED);
                let (d, done) = (d.clone(), done.clone());
                tokio::spawn(async move {
                    while !d.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    done.store(true, Ordering::SeqCst);
                    drop(producer);
                });
                Box::pin(Guarded {
                    dropped: dropped.clone(),
                    stream: error_stream(EngineError::unreachable("injected")),
                })
            } else {
                assert!(
                    done.load(Ordering::SeqCst),
                    "CLI opened before native source drained"
                );
                Box::pin(futures::stream::iter(vec![Ok(42)]))
            }
        });
        assert_eq!(stream.collect::<Vec<_>>().await, vec![Ok(42)]);
        assert!(drained.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn eng_129_reopen_domain_and_second_read_protocol_final() {
        for reopen_fails in [true, false] {
            let (r, state, cli) = native(false).await;
            if reopen_fails {
                state
                    .lock()
                    .expect("state")
                    .fail_next
                    .insert("ListSessions".into(), hr::WSLC_E_SESSION_NOT_FOUND);
            }
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let result = r
                .call("inspect_container", false, true, move |_| {
                    let c = c.clone();
                    Box::pin(async move {
                        if c.fetch_add(1, Ordering::SeqCst) == 0 {
                            dispatch::record(hr::RPC_E_DISCONNECTED);
                            Err::<(), _>(EngineError::unreachable("injected"))
                        } else {
                            Err(EngineError::protocol("malformed second read"))
                        }
                    })
                })
                .await;
            assert!(result.is_err());
            assert_eq!(
                count.load(Ordering::SeqCst),
                if reopen_fails { 1 } else { 2 }
            );
            assert!(cli.calls().is_empty());
            assert!(!r.use_cli("inspect_container"));
        }
    }

    #[tokio::test]
    async fn eng_131_native_producer_drained_before_cli_logs_open() {
        let (r, state, _, id) = populated().await;
        state
            .lock()
            .expect("state")
            .fail_next
            .insert("Logs".into(), hr::RPC_E_DISCONNECTED);
        let original = Arc::new(Mutex::new(None::<dispatch::Evidence>));
        let evidence = original.clone();
        let result = r
            .stream("logs", false, true, move |e| {
                if e.kind() == EngineKind::Wslc {
                    *evidence.lock().expect("evidence") = Some(dispatch::Evidence::current());
                    e.logs(&id, LogOpts::default())
                } else {
                    let old = evidence
                        .lock()
                        .expect("evidence")
                        .clone()
                        .expect("native source");
                    // drain() must already be ready, not just have signalled cancellation.
                    assert!(
                        futures::FutureExt::now_or_never(old.drain())
                            .expect("producer finished")
                            .is_ok()
                    );
                    Box::pin(futures::stream::empty())
                }
            })
            .collect::<Vec<_>>()
            .await;
        assert!(result.is_empty());
        assert!(r.use_cli("logs"));
        assert_eq!(
            state
                .lock()
                .expect("state")
                .calls
                .iter()
                .filter(|s| *s == "Logs")
                .count(),
            1
        );
        let old = original.lock().expect("evidence").clone().expect("source");
        assert_eq!(old.source_items(), 0);
    }

    #[tokio::test]
    async fn eng_131_after_item_failure_next_subscription_waits_for_old_source() {
        let (r, _, _) = native(false).await;
        let drained = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = drained.clone();
        let first = r
            .stream("events", false, true, move |_| {
                let producer = dispatch::Evidence::current().producer();
                dispatch::source_item();
                dispatch::record(hr::RPC_E_DISCONNECTED);
                let done = done.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    done.store(true, Ordering::SeqCst);
                    drop(producer);
                });
                Box::pin(futures::stream::iter(vec![
                    Ok(1),
                    Err(EngineError::unreachable("after item")),
                ]))
            })
            .collect::<Vec<_>>()
            .await;
        assert_eq!(first.len(), 2);
        assert_eq!(first[0], Ok(1));
        assert!(first[1].is_err());
        let next = r
            .stream("events", false, true, move |e| {
                assert_eq!(e.kind(), EngineKind::Docker, "sticky CLI fake");
                assert!(
                    drained.load(Ordering::SeqCst),
                    "resubscription overlaps native producer"
                );
                Box::pin(futures::stream::iter(vec![Ok(2)]))
            })
            .collect::<Vec<_>>()
            .await;
        assert_eq!(next, vec![Ok(2)]);
    }

    #[tokio::test]
    async fn eng_130_cancel_phase_notdispatch_final_afterdispatch_unknown() {
        for phase in [
            DispatchPhase::NotDispatched,
            DispatchPhase::MayHaveDispatched,
            DispatchPhase::Completed,
        ] {
            let (r, _, cli) = native(false).await;
            let error = r
                .call("create_volume", true, true, move |_| {
                    Box::pin(async move {
                        dispatch::phase(phase);
                        Err::<(), _>(EngineError::Cancelled)
                    })
                })
                .await
                .expect_err("cancel");
            if phase == DispatchPhase::NotDispatched {
                assert_eq!(error, EngineError::Cancelled);
            } else {
                assert!(error.hint().expect("unknown").contains("Refresh"));
            }
            assert!(cli.calls().is_empty());
        }
    }

    #[tokio::test]
    async fn eng_131_cli_only_auth_uses_delegate_error_no_com_hook() {
        let cli = FakeEngine::new("strict-cli");
        cli.set_error(
            "pull_image",
            Some(EngineError::Api {
                status: 400,
                message: "CLI supplied auth unsupported".into(),
            }),
        );
        let r = WslcEngine {
            inner: Arc::new(Inner {
                id: cli.id().clone(),
                com: None,
                strict: false,
                cli: tokio::sync::OnceCell::new_with(Some(cli.clone())),
                version: None,
                routes: Mutex::new(BTreeSet::new()),
                drains: Mutex::new(BTreeMap::new()),
            }),
        };
        let auth = RegistryAuth {
            server: "registry.invalid".into(),
            username: None,
            password: None,
            identity_token: None,
        };
        let e = r
            .pull_image("hello-world", Some(auth))
            .next()
            .await
            .expect("error")
            .expect_err("auth unsupported");
        assert_eq!(
            e,
            EngineError::Api {
                status: 400,
                message: "CLI supplied auth unsupported".into()
            }
        );
        assert_eq!(cli.calls_to("pull_image").len(), 1);
    }

    #[tokio::test]
    async fn eng_129_fallback_preserves_both_transport_diagnostics() {
        let (r, _, cli) = native(false).await;
        cli.set_error(
            "ping",
            Some(EngineError::unreachable_with_hint(
                "CLI spawn unavailable",
                "Install CLI",
            )),
        );
        let error = r
            .call("ping", false, true, move |e| {
                Box::pin(async move {
                    if e.kind() == EngineKind::Wslc {
                        dispatch::record(hr::RPC_E_DISCONNECTED);
                        Err(EngineError::unreachable("COM RPC disconnected"))
                    } else {
                        e.ping().await
                    }
                })
            })
            .await
            .expect_err("both failed");
        let text = error.to_string();
        assert!(text.contains("COM RPC disconnected"));
        assert!(text.contains("CLI spawn unavailable"));
        assert_eq!(error.hint(), Some("Install CLI"));
    }
}
