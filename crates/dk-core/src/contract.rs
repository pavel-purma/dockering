//! Reusable engine contract suite (spec 21 §7). Every backend runs it in CI against a real
//! engine or a fixture server; `FakeEngine` runs it in this crate's tests.
//!
//! Executor-agnostic: no tokio. Timeouts race the check against a helper-thread timer, and
//! every check is panic-guarded, so a misbehaving engine shows up as a failure in the report
//! instead of aborting or hanging the suite.

use std::any::Any;
use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::future::Future;
use std::hash::BuildHasher;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::StreamExt;
use futures::channel::oneshot;
use futures::future::{Either, select};

use crate::capabilities::Capabilities;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::grouping::{COMPOSE_PROJECT_LABEL, GroupBy, GroupNode, group};
use crate::model::*;

/// Upper bound for any single request-style check.
const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// Stats and logs must deliver within this window.
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);
/// The events stream is polled once for this long; no item is required.
const EVENTS_POLL: Duration = Duration::from_millis(300);
const STATS_SAMPLES: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct ContractOptions {
    /// Also run checks that create and delete resources (a throwaway volume).
    pub mutating: bool,
    /// Container used for inspect/logs/top checks; defaults to the first listed container.
    pub container_id_for_reads: Option<String>,
}

#[derive(Debug, Default)]
pub struct ContractReport {
    pub passed: Vec<&'static str>,
    pub skipped: Vec<(&'static str, String)>,
    pub failed: Vec<(&'static str, String)>,
}

impl ContractReport {
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }

    /// Panics, listing every failed check, if any check failed.
    pub fn assert_ok(&self) {
        if self.failed.is_empty() {
            return;
        }
        let failures: Vec<String> = self
            .failed
            .iter()
            .map(|(name, why)| format!("  - {name}: {why}"))
            .collect();
        panic!(
            "engine contract suite: {} failed, {} passed, {} skipped\n{}",
            self.failed.len(),
            self.passed.len(),
            self.skipped.len(),
            failures.join("\n")
        );
    }

    fn record(&mut self, name: &'static str, outcome: Result<Outcome, String>) {
        match outcome {
            Ok(Outcome::Pass) => self.passed.push(name),
            Ok(Outcome::Skip(why)) => self.skipped.push((name, why)),
            Err(why) => self.failed.push((name, why)),
        }
    }
}

enum Outcome {
    Pass,
    Skip(String),
}

type CheckResult = Result<Outcome, String>;

fn skip(why: impl Into<String>) -> CheckResult {
    Ok(Outcome::Skip(why.into()))
}

/// Runs the contract suite against `engine`. Never panics on engine misbehaviour; call
/// [`ContractReport::assert_ok`] on the result.
pub async fn run_suite(engine: Arc<dyn Engine>, opts: ContractOptions) -> ContractReport {
    let mut report = ContractReport::default();
    let e = &engine;

    report.record("ping", check(OP_TIMEOUT, check_ping(e)).await);
    report.record("info", check(OP_TIMEOUT, check_info(e)).await);

    let containers = guarded(OP_TIMEOUT, e.list_containers(ContainerQuery::default())).await;
    let containers: Vec<ContainerSummary> = match containers {
        Ok(Ok(list)) => {
            report.record("list_containers", validate_containers(&list));
            list
        }
        Ok(Err(err)) => {
            report.record("list_containers", Err(format!("error: {err}")));
            Vec::new()
        }
        Err(why) => {
            report.record("list_containers", Err(why.into()));
            Vec::new()
        }
    };
    let read_id = opts
        .container_id_for_reads
        .clone()
        .or_else(|| containers.first().map(|c| c.id.clone()));
    let running_id = pick_running(&containers, opts.container_id_for_reads.as_deref());

    let images = match guarded(OP_TIMEOUT, e.list_images()).await {
        Ok(Ok(list)) => {
            report.record("list_images", validate_images(&list));
            list
        }
        Ok(Err(err)) => {
            report.record("list_images", Err(format!("error: {err}")));
            Vec::new()
        }
        Err(why) => {
            report.record("list_images", Err(why.into()));
            Vec::new()
        }
    };
    report.record("list_volumes", check(OP_TIMEOUT, check_volumes(e)).await);
    report.record("list_networks", check(OP_TIMEOUT, check_networks(e)).await);

    report.record(
        "inspect_container",
        check(OP_TIMEOUT, check_inspect(e, read_id.as_deref())).await,
    );

    let caps = guarded(OP_TIMEOUT, async { e.capabilities() }).await;
    let caps = match caps {
        Ok(caps) => caps,
        Err(why) => {
            report.record("capabilities", Err(why.into()));
            Capabilities::empty()
        }
    };
    report.record(
        "gating_pause",
        check(
            OP_TIMEOUT,
            gated(
                caps,
                Capabilities::PAUSE,
                read_id.as_deref(),
                |id| async move {
                    let result = e.container_action(id, ContainerAction::Pause).await;
                    if result.is_ok() {
                        // The engine paused despite not advertising PAUSE: undo it.
                        let _ = e.container_action(id, ContainerAction::Unpause).await;
                    }
                    result
                },
            ),
        )
        .await,
    );
    report.record(
        "gating_top",
        check(
            OP_TIMEOUT,
            gated(caps, Capabilities::TOP, read_id.as_deref(), |id| e.top(id)),
        )
        .await,
    );
    let first_image = images.first().map(|i| i.id.clone());
    report.record(
        "gating_image_history",
        check(
            OP_TIMEOUT,
            gated(
                caps,
                Capabilities::IMAGE_HISTORY,
                first_image.as_deref(),
                |id| e.image_history(id),
            ),
        )
        .await,
    );
    report.record(
        "gating_disk_usage",
        check(
            OP_TIMEOUT,
            gated(caps, Capabilities::DISK_USAGE, Some(""), |_| e.disk_usage()),
        )
        .await,
    );

    report.record(
        "stats",
        check_or_fail(check_stats(e, running_id.as_deref())).await,
    );
    report.record(
        "logs",
        check(STREAM_TIMEOUT, check_logs(e, read_id.as_deref())).await,
    );
    report.record("events", check_or_fail(check_events(e, caps)).await);
    report.record(
        "inspect_invalid_id",
        check(OP_TIMEOUT, check_inspect_invalid(e)).await,
    );

    if opts.mutating {
        report.record(
            "volume_lifecycle",
            check(OP_TIMEOUT * 4, check_volume_lifecycle(e)).await,
        );
    } else {
        report
            .skipped
            .push(("volume_lifecycle", "mutating checks disabled".to_owned()));
    }

    report
}

// ───────────────────────────── checks ─────────────────────────────

async fn check_ping(e: &Arc<dyn Engine>) -> CheckResult {
    e.ping().await.map_err(|err| format!("error: {err}"))?;
    Ok(Outcome::Pass)
}

async fn check_info(e: &Arc<dyn Engine>) -> CheckResult {
    let info = e.info().await.map_err(|err| format!("error: {err}"))?;
    if info.server_version.trim().is_empty() {
        return Err("server_version is empty".into());
    }
    let caps = e.capabilities();
    if info.capabilities != caps {
        return Err(format!(
            "info.capabilities {:?} != engine.capabilities() {:?}",
            info.capabilities.names(),
            caps.names()
        ));
    }
    Ok(Outcome::Pass)
}

fn validate_containers(list: &[ContainerSummary]) -> CheckResult {
    let mut seen = HashSet::new();
    for c in list {
        if c.id.is_empty() {
            return Err(format!("container '{}' has an empty id", c.name));
        }
        if !seen.insert(c.id.as_str()) {
            return Err(format!("duplicate container id {}", c.id));
        }
        if c.name.starts_with('/') {
            return Err(format!("container name '{}' has a leading '/'", c.name));
        }
        if let Some(project) = c.labels.get(COMPOSE_PROJECT_LABEL) {
            match &c.compose {
                None => {
                    return Err(format!(
                        "container '{}' has a compose project label but no compose info",
                        c.name
                    ));
                }
                Some(info) if &info.project != project => {
                    return Err(format!(
                        "container '{}': compose.project '{}' != label '{project}'",
                        c.name, info.project
                    ));
                }
                Some(_) => {}
            }
        }
    }
    // Grouping invariant: every container appears exactly once in the grouped view.
    let mut members: Vec<usize> = group(list, &GroupBy::Compose, false)
        .into_iter()
        .flat_map(|node| match node {
            GroupNode::Group(g) => g.members,
            GroupNode::Container(i) => vec![i],
        })
        .collect();
    members.sort_unstable();
    if members != (0..list.len()).collect::<Vec<_>>() {
        return Err("compose grouping doesn't cover every container exactly once".into());
    }
    Ok(Outcome::Pass)
}

fn validate_images(list: &[ImageSummary]) -> CheckResult {
    match list.iter().find(|i| i.id.is_empty()) {
        Some(i) => Err(format!("image {:?} has an empty id", i.repo_tags)),
        None => Ok(Outcome::Pass),
    }
}

async fn check_volumes(e: &Arc<dyn Engine>) -> CheckResult {
    let list = e
        .list_volumes()
        .await
        .map_err(|err| format!("error: {err}"))?;
    if list.iter().any(|v| v.name.is_empty()) {
        return Err("a volume has an empty name".into());
    }
    Ok(Outcome::Pass)
}

async fn check_networks(e: &Arc<dyn Engine>) -> CheckResult {
    let list = e
        .list_networks()
        .await
        .map_err(|err| format!("error: {err}"))?;
    if let Some(n) = list.iter().find(|n| n.id.is_empty() || n.name.is_empty()) {
        return Err(format!(
            "network has an empty id or name (id '{}', name '{}')",
            n.id, n.name
        ));
    }
    Ok(Outcome::Pass)
}

async fn check_inspect(e: &Arc<dyn Engine>, id: Option<&str>) -> CheckResult {
    let Some(id) = id else {
        return skip("no containers");
    };
    let d = e
        .inspect_container(id)
        .await
        .map_err(|err| format!("error: {err}"))?;
    let got = &d.summary.id;
    // `container_id_for_reads` may be a name or short id; listed ids must match exactly.
    let same = got == id || (!got.is_empty() && got.starts_with(id)) || d.summary.name == id;
    if !same {
        return Err(format!("inspect_container({id}) returned id '{got}'"));
    }
    if !d.raw.is_object() {
        return Err("raw inspect JSON is not an object".into());
    }
    Ok(Outcome::Pass)
}

/// For a capability the engine does NOT advertise, `op` must return `Unsupported(cap)`.
async fn gated<'a, T, F, Fut>(
    caps: Capabilities,
    cap: Capabilities,
    target: Option<&'a str>,
    op: F,
) -> CheckResult
where
    F: FnOnce(&'a str) -> Fut,
    Fut: Future<Output = Result<T, EngineError>>,
{
    if caps.contains(cap) {
        return skip(format!("{:?} is advertised", cap.names()));
    }
    let Some(target) = target else {
        return skip("no resource to run the op on");
    };
    match op(target).await {
        Err(EngineError::Unsupported(c)) if c == cap => Ok(Outcome::Pass),
        Err(EngineError::Unsupported(c)) => Err(format!(
            "returned Unsupported({:?}), expected Unsupported({:?})",
            c.names(),
            cap.names()
        )),
        Err(err) => Err(format!(
            "{:?} not advertised but the op returned '{err}' instead of Unsupported",
            cap.names()
        )),
        Ok(_) => Err(format!(
            "{:?} not advertised but the op succeeded",
            cap.names()
        )),
    }
}

async fn check_stats(e: &Arc<dyn Engine>, id: Option<&str>) -> CheckResult {
    let Some(id) = id else {
        return skip("no running container");
    };
    let mut stream = e.stats(id);
    let deadline = Instant::now() + STREAM_TIMEOUT;
    let mut samples: Vec<StatsSample> = Vec::new();
    while samples.len() < STATS_SAMPLES {
        let left = deadline.saturating_duration_since(Instant::now());
        match guarded(left, stream.next()).await {
            Ok(Some(Ok(s))) => samples.push(s),
            Ok(Some(Err(err))) => return Err(format!("stream error: {err}")),
            // Stream ended (container stopped) or the window is over.
            Ok(None) | Err(GuardError::Timeout(_)) => break,
            Err(why) => return Err(why.into()),
        }
    }
    if samples.is_empty() {
        return Err(format!("no stats sample within {STREAM_TIMEOUT:?}"));
    }
    for (i, s) in samples.iter().enumerate() {
        if s.cpu_percent.is_nan() || s.cpu_percent < 0.0 {
            return Err(format!("sample {i}: cpu_percent {} < 0", s.cpu_percent));
        }
        if i > 0 && s.at < samples[i - 1].at {
            return Err(format!("sample {i}: `at` went backwards"));
        }
    }
    Ok(Outcome::Pass)
}

async fn check_logs(e: &Arc<dyn Engine>, id: Option<&str>) -> CheckResult {
    let Some(id) = id else {
        return skip("no containers");
    };
    let opts = LogOpts {
        follow: false,
        tail: Some(5),
        since: None,
        timestamps: false,
    };
    let mut stream = e.logs(id, opts);
    while let Some(item) = stream.next().await {
        item.map_err(|err| format!("stream error: {err}"))?;
    }
    Ok(Outcome::Pass)
}

async fn check_events(e: &Arc<dyn Engine>, caps: Capabilities) -> CheckResult {
    let mut stream = e.events(EventFilter::default());
    let advertised = caps.contains(Capabilities::EVENTS);
    match guarded(EVENTS_POLL, stream.next()).await {
        Ok(Some(Ok(_))) if advertised => Ok(Outcome::Pass),
        Ok(Some(Ok(_))) => Err("EVENTS not advertised but an event arrived".into()),
        Ok(Some(Err(EngineError::Unsupported(c)))) if !advertised => {
            if c == Capabilities::EVENTS {
                Ok(Outcome::Pass)
            } else {
                Err(format!("returned Unsupported({:?})", c.names()))
            }
        }
        Ok(Some(Err(err))) => Err(format!("stream error: {err}")),
        Ok(None) => Err("events stream ended immediately".into()),
        Err(GuardError::Timeout(_)) if advertised => Ok(Outcome::Pass),
        Err(GuardError::Timeout(_)) => {
            Err("EVENTS not advertised but the stream didn't return Unsupported".into())
        }
        Err(why) => Err(why.into()),
    }
}

async fn check_inspect_invalid(e: &Arc<dyn Engine>) -> CheckResult {
    match e.inspect_container("-rm").await {
        Err(_) => Ok(Outcome::Pass),
        Ok(d) => Err(format!(
            "inspect_container(\"-rm\") succeeded (id '{}')",
            d.summary.id
        )),
    }
}

async fn check_volume_lifecycle(e: &Arc<dyn Engine>) -> CheckResult {
    let name = format!(
        "dk-contract-{:012x}",
        RandomState::new().hash_one(Instant::now()) & 0xffff_ffff_ffff
    );
    let spec = VolumeSpec {
        name: Some(name.clone()),
        ..Default::default()
    };
    let created = e
        .create_volume(spec)
        .await
        .map_err(|err| format!("create_volume: {err}"))?;
    let result = volume_lifecycle_after_create(e, &name, &created).await;
    if result.is_err() {
        // Best effort cleanup; the original failure is what matters.
        let _ = e.remove_volume(&name, true).await;
    }
    result
}

async fn volume_lifecycle_after_create(
    e: &Arc<dyn Engine>,
    name: &str,
    created: &VolumeSummary,
) -> CheckResult {
    if created.name != name {
        return Err(format!(
            "create_volume returned name '{}', expected '{name}'",
            created.name
        ));
    }
    let listed = e
        .list_volumes()
        .await
        .map_err(|err| format!("list_volumes: {err}"))?;
    if !listed.iter().any(|v| v.name == name) {
        return Err(format!("created volume '{name}' is not listed"));
    }
    let details = e
        .inspect_volume(name)
        .await
        .map_err(|err| format!("inspect_volume: {err}"))?;
    if details.summary.name != name {
        return Err(format!(
            "inspect_volume returned name '{}'",
            details.summary.name
        ));
    }
    e.remove_volume(name, false)
        .await
        .map_err(|err| format!("remove_volume: {err}"))?;
    let listed = e
        .list_volumes()
        .await
        .map_err(|err| format!("list_volumes after remove: {err}"))?;
    if listed.iter().any(|v| v.name == name) {
        return Err(format!("volume '{name}' is still listed after remove"));
    }
    Ok(Outcome::Pass)
}

fn pick_running(containers: &[ContainerSummary], preferred: Option<&str>) -> Option<String> {
    let running = |c: &&ContainerSummary| c.state == ContainerState::Running;
    preferred
        .and_then(|p| {
            containers
                .iter()
                .filter(running)
                .find(|c| c.id == p || c.name == p || c.id.starts_with(p))
        })
        .or_else(|| containers.iter().find(running))
        .map(|c| c.id.clone())
}

// ───────────────────────────── harness ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum GuardError {
    Timeout(Duration),
    Panic(String),
}

impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(d) => write!(f, "timed out after {d:?}"),
            Self::Panic(msg) => write!(f, "panicked: {msg}"),
        }
    }
}

impl From<GuardError> for String {
    fn from(e: GuardError) -> Self {
        e.to_string()
    }
}

/// Runs a check with a timeout and panic guard.
async fn check(limit: Duration, fut: impl Future<Output = CheckResult>) -> CheckResult {
    guarded(limit, fut).await?
}

/// Runs a check that has its own deadlines; panic-guarded with a generous backstop.
async fn check_or_fail(fut: impl Future<Output = CheckResult>) -> CheckResult {
    guarded(OP_TIMEOUT, fut).await?
}

/// `Err` when `fut` panics or doesn't finish within `limit`.
async fn guarded<T>(limit: Duration, fut: impl Future<Output = T>) -> Result<T, GuardError> {
    match with_timeout(limit, AssertUnwindSafe(fut).catch_unwind()).await {
        None => Err(GuardError::Timeout(limit)),
        Some(Err(payload)) => Err(GuardError::Panic(panic_message(&*payload))),
        Some(Ok(v)) => Ok(v),
    }
}

/// Executor-agnostic timeout: races `fut` against a helper-thread timer. The helper exits as
/// soon as `fut` finishes (its cancel channel disconnects), so no thread outlives the call.
async fn with_timeout<F: Future>(limit: Duration, fut: F) -> Option<F::Output> {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    let (fire_tx, fire_rx) = oneshot::channel::<()>();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = cancel_rx.recv_timeout(limit) {
            let _ = fire_tx.send(());
        }
    });
    let fut = std::pin::pin!(fut);
    let out = match select(fut, fire_rx).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    };
    drop(cancel_tx);
    out
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use time::OffsetDateTime;

    use super::*;
    use crate::fake::{FakeEngine, fixtures};

    fn populated() -> Arc<FakeEngine> {
        let fake = FakeEngine::new("contract");
        let mut loose = fixtures::container("loose", ContainerState::Exited);
        loose.labels.insert("app".into(), "x".into());
        fake.set_containers(vec![
            fixtures::compose_container("shop", "web", ContainerState::Running),
            fixtures::compose_container("shop", "db", ContainerState::Running),
            loose,
        ]);
        fake.set_images(vec![
            fixtures::image("nginx:1.27", "nginx:1.27"),
            fixtures::image("", "dangling"),
        ]);
        fake.set_volumes(vec![fixtures::volume("data")]);
        fake.set_networks(vec![
            fixtures::network("bridge", "bridge"),
            fixtures::network("shop_default", "bridge"),
        ]);
        let web = fixtures::compose_container("shop", "web", ContainerState::Running).id;
        fake.set_logs(
            &web,
            (0..20)
                .map(|i| fixtures::log_line(LogStream::Stdout, &format!("line {i}")))
                .collect(),
        );
        fake
    }

    /// Feeds stats samples to every open stats stream until dropped.
    struct StatsFeeder {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl StatsFeeder {
        fn start(fake: Arc<FakeEngine>, ids: Vec<String>) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::spawn(move || {
                let mut at = OffsetDateTime::now_utc();
                while !flag.load(Ordering::Relaxed) {
                    for id in &ids {
                        fake.push_stats(id, fixtures::stats_sample(at, 3.5, 1 << 20));
                    }
                    at += time::Duration::seconds(1);
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            Self {
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for StatsFeeder {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn run(fake: &Arc<FakeEngine>, opts: ContractOptions) -> ContractReport {
        let engine: Arc<dyn Engine> = fake.clone();
        let running = futures::executor::block_on(engine.list_containers(ContainerQuery {
            all: false,
            ..Default::default()
        }))
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.id)
        .collect();
        let _feeder = StatsFeeder::start(fake.clone(), running);
        futures::executor::block_on(run_suite(engine, opts))
    }

    fn failed_names(r: &ContractReport) -> Vec<&'static str> {
        r.failed.iter().map(|(n, _)| *n).collect()
    }

    #[test]
    fn contract_suite_passes_on_fake_engine() {
        // Full Docker capabilities: gating checks are skipped as advertised.
        let fake = populated();
        let opts = ContractOptions {
            mutating: true,
            container_id_for_reads: None,
        };
        let report = run(&fake, opts.clone());
        report.assert_ok();
        for name in [
            "ping",
            "info",
            "list_containers",
            "list_images",
            "list_volumes",
            "list_networks",
            "inspect_container",
            "stats",
            "logs",
            "events",
            "inspect_invalid_id",
            "volume_lifecycle",
        ] {
            assert!(
                report.passed.contains(&name),
                "{name} didn't pass: {report:?}"
            );
        }
        assert_eq!(report.skipped.len(), 4, "{report:?}");
        assert_eq!(fake.calls_to("create_volume").len(), 1);
        assert_eq!(fake.calls_to("remove_volume").len(), 1);

        // WSLC-like reduced capabilities: every gated op must return Unsupported.
        let fake = populated();
        fake.set_capabilities(
            Capabilities::EVENTS
                | Capabilities::EXEC_TTY
                | Capabilities::EXEC_RESIZE
                | Capabilities::NETWORK_MGMT
                | Capabilities::LOGS_FOLLOW,
        );
        let report = run(&fake, opts);
        report.assert_ok();
        for name in [
            "gating_pause",
            "gating_top",
            "gating_image_history",
            "gating_disk_usage",
        ] {
            assert!(
                report.passed.contains(&name),
                "{name} didn't pass: {report:?}"
            );
        }
        assert!(report.skipped.is_empty(), "{report:?}");

        // No capabilities at all (events unsupported), with an explicit read container.
        let fake = populated();
        fake.set_capabilities(Capabilities::empty());
        let report = run(
            &fake,
            ContractOptions {
                mutating: false,
                container_id_for_reads: Some("shop-db-1".into()),
            },
        );
        report.assert_ok();
        assert!(report.passed.contains(&"events"));
        assert_eq!(
            report.skipped,
            [("volume_lifecycle", "mutating checks disabled".into())]
        );
    }

    #[test]
    fn contract_suite_passes_on_empty_engine() {
        let fake = FakeEngine::new("empty");
        let report = run(&fake, ContractOptions::default());
        report.assert_ok();
        let skipped: Vec<_> = report.skipped.iter().map(|(n, _)| *n).collect();
        assert!(skipped.contains(&"inspect_container"));
        assert!(skipped.contains(&"stats"));
        assert!(skipped.contains(&"logs"));
    }

    #[test]
    fn contract_suite_reports_violations() {
        let fake = populated();
        let mut bad = fixtures::compose_container("shop", "web", ContainerState::Running);
        bad.name = "/shop-web-1".into();
        bad.compose = None;
        fake.set_containers(vec![bad]);
        fake.set_error("ping", Some(EngineError::unreachable("down")));
        fake.set_error("list_volumes", Some(EngineError::protocol("bad json")));
        fake.panic_on("inspect_container");
        let report = run(&fake, ContractOptions::default());
        let failed = failed_names(&report);
        for name in [
            "ping",
            "list_containers",
            "list_volumes",
            "inspect_container",
            "inspect_invalid_id",
        ] {
            assert!(failed.contains(&name), "{name} should fail: {report:?}");
        }
        let (_, why) = report
            .failed
            .iter()
            .find(|(n, _)| *n == "inspect_container")
            .unwrap();
        assert!(why.contains("panicked"), "{why}");
        let caught = std::panic::catch_unwind(AssertUnwindSafe(|| report.assert_ok()));
        assert!(caught.is_err());
    }

    #[test]
    fn contract_silent_stats_stream_is_bounded() {
        // A stats stream that never yields can't hang the suite: the guard cuts it off.
        let fake = FakeEngine::new("silent");
        fake.set_containers(vec![fixtures::container("c", ContainerState::Running)]);
        let engine: Arc<dyn Engine> = fake.clone();
        let started = Instant::now();
        let result = futures::executor::block_on(guarded(
            Duration::from_secs(1),
            check_stats(&engine, Some("c")),
        ));
        assert!(result.is_err(), "outer guard should time out first");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn contract_with_timeout_is_executor_agnostic() {
        let slow = futures::future::pending::<()>();
        let out = futures::executor::block_on(with_timeout(Duration::from_millis(20), slow));
        assert_eq!(out, None);
        let fast = async { 7 };
        assert_eq!(
            futures::executor::block_on(with_timeout(Duration::from_secs(60), fast)),
            Some(7)
        );
    }

    #[test]
    fn contract_suite_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let engine: Arc<dyn Engine> = FakeEngine::new("send");
        let fut = run_suite(engine, ContractOptions::default());
        assert_send(&fut);
    }
}
