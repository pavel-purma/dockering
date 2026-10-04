//! `wslc.exe` CLI fallback transport (spec 20 §5.5). Owner: `engine-integrator`.
//! PUBLIC (crate) API FIXED — the factory (owned by `windows-platform`) calls these.
//!
//! Every invocation is an argv vector (`wslc.exe [--session s] <cmd…>`) with
//! `CREATE_NO_WINDOW`, `NO_COLOR=1`, a timeout and at most 4 concurrent processes
//! ([`runner`]). wslc 3.0.1 treats `--` as a positional value, so ids, names and refs are
//! validated (NFR-022) and never start with `-`.

pub(crate) mod exec;
pub(crate) mod parse;
pub(crate) mod runner;
pub(crate) mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dk_core::grouping::{COMPOSE_PROJECT_LABEL, compose_info_from_labels};
use dk_core::validate::{
    validate_env_key, validate_id, validate_id_or_name, validate_image_ref, validate_name,
    validate_signal,
};
use dk_core::{
    Capabilities, ContainerAction, ContainerCounts, ContainerDetails, ContainerQuery,
    ContainerState, ContainerSummary, DiskUsage, Engine, EngineError, EngineEvent, EngineId,
    EngineInfo, EngineKind, EngineResult, EngineStream, EventFilter, ExecRequest, ImageDeleteItem,
    ImageDetails, ImageLayer, ImageSummary, LogChunk, LogOpts, MountKind, NetworkDetails,
    NetworkSummary, ProcessList, PruneReport, PullProgress, RegistryAuth, RemoveContainerOpts,
    ResourceKind, RunSpec, StatsSample, TerminalSession, VolumeDetails, VolumeSpec, VolumeSummary,
    error_stream,
};
use serde_json::Value;

use self::runner::{DEFAULT_TIMEOUT, ErrCtx, RUN_TIMEOUT, Runner, UPDATE_HINT};

/// Capabilities of the CLI transport (spec 20 §5.6).
pub const CLI_CAPABILITIES: Capabilities = Capabilities::EVENTS
    .union(Capabilities::LOGS_FOLLOW)
    .union(Capabilities::NETWORK_MGMT)
    .union(Capabilities::EXEC_TTY)
    .union(Capabilities::EXEC_RESIZE);

/// Env var overriding the `wslc.exe` path (tests use the fake, spec 21 §7).
pub const WSLC_EXE_ENV: &str = "DOCKERING_WSLC_EXE";

struct Inner {
    id: EngineId,
    runner: Runner,
    session: Option<String>,
    wsl_version: Option<String>,
    transport_note: Option<String>,
    online_cpus: u32,
}

/// `Engine` implementation over `wslc.exe`.
pub struct WslcCliEngine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WslcCliEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WslcCliEngine")
            .field("id", &self.inner.id)
            .field("session", &self.inner.session)
            .field("exe", &self.inner.runner.exe())
            .finish_non_exhaustive()
    }
}

impl WslcCliEngine {
    /// Keep the completed write separate from enrichment so callers never replay Create.
    pub(crate) async fn create_volume_raw(&self, spec: VolumeSpec) -> EngineResult<String> {
        let mut args: Vec<String> = vec!["volume".into(), "create".into()];
        if let Some(d) = &spec.driver
            && d != "local"
        {
            validate_name(d)?;
            args.extend(["--driver".into(), d.clone()]);
        }
        for (k, v) in &spec.driver_opts {
            validate_env_key(k)?;
            args.extend(["--opt".into(), format!("{k}={v}")]);
        }
        for (k, v) in &spec.labels {
            validate_env_key(k)?;
            args.extend(["--label".into(), format!("{k}={v}")]);
        }
        if let Some(n) = &spec.name {
            validate_name(n)?;
            args.push(n.clone());
        }
        let out = self.run(&args, None).await?;
        let name = out
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(str::to_owned)
            .or(spec.name)
            .ok_or_else(|| EngineError::protocol("wslc volume create printed no name"))?;
        validate_name(&name)?;
        Ok(name)
    }
    /// Verify `wslc.exe` works for `session` (None = the caller's default session) and build
    /// the engine. `transport_note` explains why the CLI is used (ENG-110 chip), e.g.
    /// "WSL 3.1.0 not yet verified — using CLI". `wsl_version` is shown in diagnostics.
    pub async fn connect(
        id: EngineId,
        session: Option<String>,
        wsl_version: Option<String>,
        transport_note: Option<String>,
    ) -> EngineResult<WslcCliEngine> {
        Self::connect_guarded(id, session, wsl_version, transport_note, None).await
    }

    pub(crate) async fn connect_guarded(
        id: EngineId,
        session: Option<String>,
        wsl_version: Option<String>,
        transport_note: Option<String>,
        guard: Option<runner::SpawnGuard>,
    ) -> EngineResult<WslcCliEngine> {
        let exe = wslc_exe()
            .ok_or_else(|| EngineError::unreachable_with_hint("wslc.exe not found", UPDATE_HINT))?;
        if let Some(s) = &session {
            validate_session(s)?;
        }
        let runner = Runner::new(exe, session.clone()).with_guard(guard);
        let engine = WslcCliEngine {
            inner: Arc::new(Inner {
                id,
                runner,
                session,
                wsl_version,
                transport_note,
                online_cpus: stream::default_online_cpus(),
            }),
        };
        // `version` doesn't touch the session; `container list -q` proves the session opens
        // (wslc boots the session VM on demand).
        let out = engine
            .run(&["version"], None)
            .await
            .map_err(|e| unreachable_with_update_hint(e, "wslc.exe version failed"))?;
        if parse::version(&out).is_none() {
            return Err(EngineError::unreachable_with_hint(
                "wslc.exe printed no version",
                UPDATE_HINT,
            ));
        }
        engine.run(&["container", "list", "--quiet"], None).await?;
        Ok(engine)
    }

    fn runner(&self) -> &Runner {
        &self.inner.runner
    }

    async fn run<S: AsRef<str>>(
        &self,
        args: &[S],
        ctx: Option<ErrCtx<'_>>,
    ) -> EngineResult<String> {
        Ok(self.runner().run(args, DEFAULT_TIMEOUT, ctx).await?.stdout)
    }

    async fn version_string(&self) -> EngineResult<String> {
        let out = self.run(&["version", "--format", "json"], None).await;
        let out = match out {
            Ok(o) => o,
            // Older builds may lack `--format` on `version`.
            Err(EngineError::Api { .. }) => self.run(&["version"], None).await?,
            Err(e) => return Err(e),
        };
        parse::version(&out).ok_or_else(|| EngineError::protocol("wslc version: no version"))
    }

    /// `container list` rows without compose metadata (internal: counts, volume "used by").
    async fn container_rows(&self, all: bool) -> EngineResult<Vec<ContainerSummary>> {
        let mut args = vec!["container", "list", "--no-trunc", "--format", "json"];
        if all {
            args.insert(2, "--all");
        }
        let out = self.run(&args, None).await?;
        Ok(parse::json_list(&out)?
            .iter()
            .map(parse::container_summary)
            .collect())
    }

    async fn inspect_json(&self, kind: ResourceKind, id: &str) -> EngineResult<Value> {
        let noun = noun(kind);
        let out = self
            .run(
                &[noun, "inspect", "--format", "json", id],
                Some(ErrCtx::new(kind, id)),
            )
            .await?;
        parse::inspect_object(&out)?.ok_or_else(|| EngineError::not_found(kind, id))
    }

    /// One `inspect` for many names; falls back to per-item calls when one is missing (wslc
    /// fails the whole batch with exit 1 but still prints the found ones).
    async fn inspect_many(&self, kind: ResourceKind, names: &[String]) -> EngineResult<Vec<Value>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let noun = noun(kind);
        let mut args: Vec<&str> = vec![noun, "inspect", "--format", "json"];
        args.extend(names.iter().map(String::as_str));
        match self.runner().run(&args, DEFAULT_TIMEOUT, None).await {
            Ok(o) => parse::inspect_batch(&o.stdout),
            Err(EngineError::NotFound { kind: missing, .. }) if missing == kind => {
                let mut out = Vec::new();
                for n in names {
                    match self.inspect_json(kind, n).await {
                        Ok(v) => out.push(v),
                        Err(EngineError::NotFound { kind: missing, .. }) if missing == kind => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(out)
            }
            Err(e) => Err(e),
        }
    }

    async fn prune(&self, noun: &str, extra: &[&str]) -> EngineResult<PruneReport> {
        let mut args = vec![noun, "prune", "--force"];
        args.extend_from_slice(extra);
        let out = self.run(&args, None).await?;
        Ok(parse::prune_report(&out))
    }
}

/// Compose metadata from labels (spec 21 §4); only consulted when the project label exists.
fn compose(labels: &std::collections::BTreeMap<String, String>) -> Option<dk_core::ComposeInfo> {
    if labels.contains_key(COMPOSE_PROJECT_LABEL) {
        compose_info_from_labels(labels)
    } else {
        None
    }
}

fn noun(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Image => "image",
        ResourceKind::Volume => "volume",
        ResourceKind::Network => "network",
        _ => "container",
    }
}

fn unreachable_with_update_hint(e: EngineError, reason: &str) -> EngineError {
    match e {
        EngineError::Unreachable { .. } => e,
        other => EngineError::unreachable_with_hint(format!("{reason}: {other}"), UPDATE_HINT),
    }
}

/// Session display names are free text (`wslc-cli-<user>`), but must not look like options or
/// contain control characters (NFR-022).
fn validate_session(s: &str) -> EngineResult<()> {
    if s.is_empty() || s.starts_with('-') || s.len() > 256 || s.chars().any(char::is_control) {
        return Err(EngineError::Api {
            status: 400,
            message: format!("invalid session name: \"{}\"", s.escape_default()),
        });
    }
    Ok(())
}

/// Container ids may be given as names, ids or id prefixes.
fn check_container(id: &str) -> EngineResult<()> {
    validate_id_or_name(id)
}

/// Volume/network host paths and container targets for `-v src:dst[:ro]`.
fn validate_mount_part(what: &str, s: &str) -> EngineResult<()> {
    if s.is_empty() || s.starts_with('-') || s.chars().any(char::is_control) {
        return Err(EngineError::Api {
            status: 400,
            message: format!("invalid mount {what}: \"{}\"", s.escape_default()),
        });
    }
    Ok(())
}

/// `container run -d …` argv (spec 20 §5.5); flags verified against `wslc container run
/// --help` 3.0.1. Validates every value (NFR-022).
pub(crate) fn run_args(spec: &RunSpec) -> EngineResult<Vec<String>> {
    validate_image_ref(&spec.image)?;
    let mut a: Vec<String> = vec!["container".into(), "run".into(), "--detach".into()];
    if let Some(n) = &spec.name {
        validate_name(n)?;
        a.extend(["--name".into(), n.clone()]);
    }
    for p in &spec.ports {
        let target = format!("{}/{}", p.private, p.proto);
        let v = match (p.ip, p.public) {
            (Some(ip @ std::net::IpAddr::V6(_)), Some(pub_)) => format!("[{ip}]:{pub_}:{target}"),
            (Some(ip), Some(pub_)) => format!("{ip}:{pub_}:{target}"),
            (Some(ip @ std::net::IpAddr::V6(_)), None) => format!("[{ip}]::{target}"),
            (Some(ip), None) => format!("{ip}::{target}"),
            (None, Some(pub_)) => format!("{pub_}:{target}"),
            (None, None) => target,
        };
        a.extend(["--publish".into(), v]);
    }
    for (k, v) in &spec.env {
        validate_env_key(k)?;
        if v.contains('\0') {
            return Err(EngineError::Api {
                status: 400,
                message: format!("invalid env value for {k}"),
            });
        }
        a.extend(["--env".into(), format!("{k}={v}")]);
    }
    for m in &spec.mounts {
        match m.kind {
            MountKind::Volume => validate_name(&m.source)?,
            MountKind::Bind => validate_mount_part("source", &m.source)?,
            MountKind::Tmpfs => {}
            MountKind::Npipe | MountKind::Unknown => {
                return Err(EngineError::Api {
                    status: 400,
                    message: format!("unsupported mount type {:?}", m.kind),
                });
            }
        }
        validate_mount_part("target", &m.target)?;
        if m.kind == MountKind::Tmpfs {
            a.extend(["--tmpfs".into(), m.target.clone()]);
            continue;
        }
        let mut v = format!("{}:{}", m.source, m.target);
        if m.read_only {
            v.push_str(":ro");
        }
        a.extend(["--volume".into(), v]);
    }
    for (k, v) in &spec.labels {
        validate_env_key(k)?;
        a.extend(["--label".into(), format!("{k}={v}")]);
    }
    if spec.auto_remove {
        a.push("--rm".into());
    }
    a.push(spec.image.clone());
    if let Some(cmd) = &spec.cmd {
        if cmd.iter().any(|c| c.contains('\0')) {
            return Err(EngineError::Api {
                status: 400,
                message: "invalid command".into(),
            });
        }
        a.extend(cmd.iter().cloned());
    }
    Ok(a)
}

/// `container stop|restart|kill|start` argv for an action.
pub(crate) fn action_args(id: &str, action: &ContainerAction) -> EngineResult<Vec<String>> {
    let mut a: Vec<String> = vec!["container".into(), action.verb().into()];
    match action {
        ContainerAction::Start => {}
        ContainerAction::Stop { timeout_s } => {
            if let Some(t) = timeout_s {
                a.extend(["--time".into(), t.to_string()]);
            }
        }
        ContainerAction::Restart { timeout_s } => {
            if let Some(t) = timeout_s {
                a.extend(["--timeout".into(), t.to_string()]);
            }
        }
        ContainerAction::Kill { signal } => {
            if let Some(s) = signal {
                validate_signal(s)?;
                a.extend(["--signal".into(), s.clone()]);
            }
        }
        ContainerAction::Pause | ContainerAction::Unpause => {
            return Err(EngineError::Unsupported(Capabilities::PAUSE));
        }
    }
    a.push(id.to_owned());
    Ok(a)
}

/// Docker-shaped inspect objects use `Id`; WSLC network lists use `ID`.
fn same_object(a: &Value, id: &str, name: &str) -> bool {
    let i = parse::text(a, "Id");
    let n = parse::text(a, "Name");
    (!name.is_empty() && n == name) || (!id.is_empty() && i == id)
}

fn counts(list: &[ContainerSummary]) -> ContainerCounts {
    let mut c = ContainerCounts::default();
    for s in list {
        match s.state {
            ContainerState::Running | ContainerState::Restarting => c.running += 1,
            ContainerState::Paused => c.paused += 1,
            _ => c.stopped += 1,
        }
    }
    c
}

/// wslc's `inspect` puts `Ports` at the top level; Docker puts it under
/// `NetworkSettings.Ports`. Copy it over (and `Config.Labels`) so the Docker mapper sees it.
#[cfg(test)]
fn dockerize_container_inspect(mut v: Value) -> Value {
    let ports = v.get("Ports").cloned();
    if let (Some(ports), Some(ns)) = (ports, v.get_mut("NetworkSettings"))
        && let Some(o) = ns.as_object_mut()
    {
        o.entry("Ports").or_insert(ports);
    }
    v
}

#[async_trait]
impl Engine for WslcCliEngine {
    fn id(&self) -> &EngineId {
        &self.inner.id
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }

    fn capabilities(&self) -> Capabilities {
        CLI_CAPABILITIES
    }

    async fn ping(&self) -> EngineResult<()> {
        self.run(&["container", "list", "--quiet"], None)
            .await
            .map(|_| ())
    }

    async fn info(&self) -> EngineResult<EngineInfo> {
        let version = self.version_string().await?;
        let sys = match self
            .run(&["system", "info", "--format", "json"], None)
            .await
        {
            Ok(o) => parse::system_info(&o).unwrap_or_default(),
            Err(e) if e.is_unreachable() => return Err(e),
            Err(_) => parse::SystemInfo::default(),
        };
        let containers = self.container_rows(true).await?;
        let images = self
            .run(&["image", "list", "--no-trunc", "--format", "json"], None)
            .await
            .and_then(|o| parse::json_list(&o))
            .map(|rows| parse::image_summaries(&rows).len() as u32)
            .unwrap_or(0);
        let session = self.inner.session.clone();
        let name = match session {
            Some(s) => format!("WSL containers ({s})"),
            None => "WSL containers".to_owned(),
        };
        Ok(EngineInfo {
            name,
            kind: EngineKind::Wslc,
            transport: Some("cli".into()),
            transport_note: self.inner.transport_note.clone(),
            server_version: sys
                .session_manager_version
                .or_else(|| self.inner.wsl_version.clone())
                .unwrap_or(version),
            api_version: None,
            os: "linux".into(),
            arch: std::env::consts::ARCH
                .replace("x86_64", "amd64")
                .replace("aarch64", "arm64"),
            kernel: sys.kernel,
            cpus: Some(self.inner.online_cpus),
            mem_total: None,
            containers: counts(&containers),
            images,
            storage_driver: None,
            root_dir: None,
            daemon_id: None,
            list_stats_limit: 0,
            capabilities: CLI_CAPABILITIES,
        })
    }

    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent> {
        stream::events(self.runner().clone(), filter)
    }

    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        let mut list = self.container_rows(q.all).await?;
        if !q.label_filter.is_empty() {
            list.retain(|c| {
                q.label_filter.iter().all(|(k, v)| match v {
                    Some(v) => c.labels.get(k) == Some(v),
                    None => c.labels.contains_key(k),
                })
            });
        }
        for c in &mut list {
            c.compose = compose(&c.labels);
            if !q.size {
                c.size_rw = None;
                c.size_root_fs = None;
            }
        }
        Ok(list)
    }

    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails> {
        check_container(id)?;
        let v = self.inspect_json(ResourceKind::Container, id).await?;
        let mut d = crate::inspect::container_details(&v)?;
        if d.summary.compose.is_none() {
            d.summary.compose = compose(&d.summary.labels);
        }
        d.summary.labels.remove(parse::WSL_METADATA_LABEL);
        Ok(d)
    }

    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        check_container(id)?;
        let args = action_args(id, &action)?;
        self.run(&args, Some(ErrCtx::new(ResourceKind::Container, id)))
            .await
            .map(|_| ())
    }

    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        check_container(id)?;
        let mut args = vec!["container", "remove"];
        if opts.force {
            args.push("--force");
        }
        if opts.volumes {
            args.push("--volumes");
        }
        args.push(id);
        self.run(&args, Some(ErrCtx::new(ResourceKind::Container, id)))
            .await
            .map(|_| ())
    }

    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        self.prune("container", &[]).await
    }

    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        if let Err(e) = check_container(id) {
            return error_stream(e);
        }
        stream::logs(self.runner().clone(), id.to_owned(), opts)
    }

    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        if let Err(e) = check_container(id) {
            return error_stream(e);
        }
        stream::stats(self.runner().clone(), id.to_owned(), self.inner.online_cpus)
    }

    async fn top(&self, _id: &str) -> EngineResult<ProcessList> {
        Err(EngineError::Unsupported(Capabilities::TOP))
    }

    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        check_container(id)?;
        exec::validate_exec(&req)?;
        #[cfg(windows)]
        {
            // Fail fast (NotFound/Conflict) before opening a pseudo console.
            let v = self.inspect_json(ResourceKind::Container, id).await?;
            let running = v
                .pointer("/State/Running")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !running {
                return Err(EngineError::Conflict(format!(
                    "container {id} is not running"
                )));
            }
            let argv = self.runner().argv(&exec::exec_args(id, &req));
            let exe = self.runner().exe().to_path_buf();
            let (cols, rows) = (req.cols, req.rows);
            let runner = self.runner().clone();
            exec::spawn_checked(exe, argv, cols, rows, async move {
                runner.validate_spawn().await
            })
            .await
        }
        #[cfg(not(windows))]
        {
            Err(EngineError::Unsupported(Capabilities::EXEC_TTY))
        }
    }

    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        let out = self
            .run(
                &[
                    "image",
                    "list",
                    "--no-trunc",
                    "--digests",
                    "--format",
                    "json",
                ],
                None,
            )
            .await?;
        Ok(parse::image_summaries(&parse::json_list(&out)?))
    }

    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails> {
        validate_image_ref(id)?;
        let v = self.inspect_json(ResourceKind::Image, id).await?;
        dk_core::docker_json::image_details(&v)
    }

    async fn image_history(&self, _id: &str) -> EngineResult<Vec<ImageLayer>> {
        Err(EngineError::Unsupported(Capabilities::IMAGE_HISTORY))
    }

    fn pull_image(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        if let Err(e) = validate_image_ref(reference) {
            return error_stream(e);
        }
        if auth.is_some() {
            // `wslc image pull` has no credential flags; it uses `wslc login` state. Never
            // pass secrets on a command line (NFR-020).
            return error_stream(EngineError::Api { status: 400, message: "WSLC CLI cannot honor supplied registry credentials; use native COM or configure CLI login explicitly".into() });
        }
        stream::pull(self.runner().clone(), reference.to_owned())
    }

    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        validate_image_ref(id)?;
        let mut args = vec!["image", "remove"];
        if force {
            args.push("--force");
        }
        args.push(id);
        let out = self
            .run(&args, Some(ErrCtx::new(ResourceKind::Image, id)))
            .await?;
        Ok(parse::image_delete_items(&out))
    }

    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        if dangling_only {
            self.prune("image", &[]).await
        } else {
            self.prune("image", &["--all"]).await
        }
    }

    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        validate_image_ref(id)?;
        let target = if tag.is_empty() {
            repo.to_owned()
        } else {
            format!("{repo}:{tag}")
        };
        validate_image_ref(&target)?;
        if validate_id(&target).is_ok() && !target.contains(':') {
            // A bare hex string would be read as an id, not a repository.
            return Err(EngineError::Api {
                status: 400,
                message: format!("invalid repository: \"{repo}\""),
            });
        }
        self.run(
            &["image", "tag", id, target.as_str()],
            Some(ErrCtx::new(ResourceKind::Image, id)),
        )
        .await
        .map(|_| ())
    }

    async fn run_image(&self, spec: RunSpec) -> EngineResult<String> {
        let args = run_args(&spec)?;
        let out = self
            .runner()
            .run(
                &args,
                RUN_TIMEOUT,
                Some(ErrCtx::new(ResourceKind::Image, &spec.image)),
            )
            .await?;
        // stdout is the new id; pull progress (if any) goes to stderr.
        out.stdout
            .lines()
            .map(str::trim)
            .rev()
            .find(|l| validate_id(l).is_ok())
            .map(str::to_owned)
            .ok_or_else(runner::unknown_outcome)
    }

    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        let out = self
            .run(&["volume", "list", "--format", "json"], None)
            .await?;
        let mut list: Vec<VolumeSummary> = parse::json_list(&out)?
            .iter()
            .map(parse::volume_summary)
            .collect();
        // `created` is only in `inspect`: one batch call.
        let names: Vec<String> = list
            .iter()
            .filter(|v| validate_name(&v.name).is_ok())
            .map(|v| v.name.clone())
            .collect();
        let details = self.inspect_many(ResourceKind::Volume, &names).await?;
        for v in &mut list {
            if let Some(d) = details.iter().find(|d| same_object(d, "", &v.name)) {
                parse::enrich_volume(v, d);
            }
            v.compose = compose(&v.labels);
        }
        Ok(list)
    }

    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails> {
        validate_name(name)?;
        let v = self.inspect_json(ResourceKind::Volume, name).await?;
        let mut summary = dk_core::docker_json::volume_summary(&v);
        summary.compose = compose(&summary.labels);
        let containers = self.container_rows(true).await.unwrap_or_default();
        let used_by = dk_core::docker_json::volume_used_by(name, &containers);
        let options = v
            .get("Options")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(VolumeDetails {
            summary,
            options,
            status: v.get("Status").cloned().filter(|s| !s.is_null()),
            used_by,
            raw: v,
        })
    }

    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        let name = self.create_volume_raw(spec).await?;
        let v = self
            .inspect_json(ResourceKind::Volume, &name)
            .await
            .map_err(|e| {
                EngineError::unreachable_with_hint(
                    format!("Volume {name} was created but inspect failed: {e}"),
                    "Refresh the volume list; do not create the volume again.",
                )
            })?;
        let mut s = dk_core::docker_json::volume_summary(&v);
        s.compose = compose(&s.labels);
        Ok(s)
    }

    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        validate_name(name)?;
        // wslc's `volume remove -f` only means "don't error if missing" (not Docker's
        // force-remove), so `force` is not forwarded: a missing volume stays NotFound.
        let _ = force;
        self.run(
            &["volume", "remove", name],
            Some(ErrCtx::new(ResourceKind::Volume, name)),
        )
        .await
        .map(|_| ())
    }

    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        // Docker API ≥ 1.42 prunes anonymous volumes only; wslc does the same without --all.
        self.prune("volume", &[]).await
    }

    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        Err(EngineError::Unsupported(Capabilities::DISK_USAGE))
    }

    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        let out = self
            .run(&["network", "list", "--no-trunc", "--format", "json"], None)
            .await?;
        let mut list: Vec<NetworkSummary> = parse::json_list(&out)?
            .iter()
            .map(parse::network_summary)
            .collect();
        let names: Vec<String> = list
            .iter()
            .filter(|n| validate_name(&n.name).is_ok())
            .map(|n| n.name.clone())
            .collect();
        let details = self.inspect_many(ResourceKind::Network, &names).await?;
        for n in &mut list {
            if let Some(d) = details.iter().find(|d| same_object(d, &n.id, &n.name)) {
                parse::enrich_network(n, d);
            }
            n.compose = compose(&n.labels);
        }
        Ok(list)
    }

    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails> {
        validate_id_or_name(id)?;
        let v = self.inspect_json(ResourceKind::Network, id).await?;
        let mut d = dk_core::docker_json::network_details(&v)?;
        if d.summary.compose.is_none() {
            d.summary.compose = compose(&d.summary.labels);
        }
        Ok(d)
    }

    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        validate_id_or_name(id)?;
        // wslc 3.0.1 `network remove` accepts names only (ids → "Network not found").
        let name = if validate_id(id).is_ok() {
            let v = self.inspect_json(ResourceKind::Network, id).await?;
            let n = parse::text(&v, "Name");
            validate_name(&n)?;
            n
        } else {
            id.to_owned()
        };
        self.run(
            &["network", "remove", name.as_str()],
            Some(ErrCtx::new(ResourceKind::Network, id)),
        )
        .await
        .map(|_| ())
    }

    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        self.prune("network", &[]).await
    }
}

/// `wslc system session list` → display names (table output, F-10).
pub async fn list_sessions() -> EngineResult<Vec<String>> {
    let exe = wslc_exe()
        .ok_or_else(|| EngineError::unreachable_with_hint("wslc.exe not found", UPDATE_HINT))?;
    let runner = Runner::new(exe, None);
    let out = runner
        .run(&["system", "session", "list"], DEFAULT_TIMEOUT, None)
        .await?;
    Ok(parse::session_table(&out.stdout)
        .into_iter()
        .map(|r| r.name)
        .collect())
}

/// Path of `wslc.exe` if present: `DOCKERING_WSLC_EXE` (tests, any OS), then
/// `%ProgramFiles%\WSL\wslc.exe`, then `PATH` (Windows only).
pub fn wslc_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(WSLC_EXE_ENV).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    #[cfg(windows)]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let p = PathBuf::from(pf).join("WSL").join("wslc.exe");
            if p.is_file() {
                return Some(p);
            }
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join("wslc.exe"))
            .find(|p| p.is_file())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dk_core::{MountRequest, PortMapping, Proto};

    use super::*;

    fn fixture_engine(runner: Runner) -> WslcCliEngine {
        WslcCliEngine {
            inner: Arc::new(Inner {
                id: EngineId::new("review-test"),
                runner,
                session: Some("repair-session".into()),
                wsl_version: None,
                transport_note: None,
                online_cpus: 1,
            }),
        }
    }

    #[tokio::test]
    async fn eng_129_inspect_many_errors_no_fanout_only_partial_notfound() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (names, expected) in [
            (["repair-a", "repair-b"], 1),
            (["repair-denied", "repair-b"], 1),
            (["repair-missing", "repair-b"], 3),
        ] {
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let e = fixture_engine(Runner::fixture().with_guard(Some(Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }))));
            let result = e
                .inspect_many(ResourceKind::Volume, &names.map(String::from))
                .await;
            if names[0] == "repair-a" {
                assert!(matches!(result, Err(EngineError::Protocol(_))));
            } else if names[0] == "repair-denied" {
                assert_eq!(
                    result.expect_err("policy").hint(),
                    Some(runner::POLICY_HINT)
                );
            } else {
                assert_eq!(result.expect("partial").len(), 1);
            }
            assert_eq!(count.load(Ordering::SeqCst), expected);
        }
        for error in [
            EngineError::Cancelled,
            EngineError::Api {
                status: 403,
                message: "denied".into(),
            },
            EngineError::unreachable_with_hint("policy", runner::POLICY_HINT),
        ] {
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let injected = error.clone();
            let e = fixture_engine(Runner::fixture().with_guard(Some(Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                let error = injected.clone();
                Box::pin(async move { Err(error) })
            }))));
            assert_eq!(
                e.inspect_many(
                    ResourceKind::Volume,
                    &["repair-a".into(), "repair-b".into()]
                )
                .await,
                Err(error)
            );
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn eng_127_cli_stats_revalidates_every_poll_and_stops_on_replacement() {
        use futures::StreamExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let e = fixture_engine(Runner::fixture().with_guard(Some(Arc::new(move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Ok(())
                } else {
                    Err(EngineError::unreachable(
                        "target replaced between stats polls",
                    ))
                }
            })
        }))));
        let mut stats = e.stats("repair-ports");
        assert!(stats.next().await.expect("sample").is_ok());
        assert_eq!(
            stats.next().await.expect("replacement error"),
            Err(EngineError::unreachable(
                "target replaced between stats polls"
            ))
        );
        assert!(stats.next().await.is_none());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn eng_127_exec_revalidates_after_inspect_inside_blocking_spawn() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let e = fixture_engine(Runner::fixture().with_guard(Some(Arc::new(move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Ok(())
                } else {
                    Err(EngineError::unreachable(
                        "target replaced after exec inspect",
                    ))
                }
            })
        }))));
        let error = match e.exec("repair-running", ExecRequest::default()).await {
            Ok(_) => panic!("must not spawn"),
            Err(e) => e,
        };
        assert_eq!(
            error,
            EngineError::unreachable("target replaced after exec inspect")
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn eng_131_cli_pull_rpc_unknown_policy_domain_final() {
        use futures::StreamExt;
        let e = fixture_engine(Runner::fixture());
        let rpc = e
            .pull_image("repair-rpc", None)
            .next()
            .await
            .expect("error")
            .expect_err("lost response");
        assert!(rpc.hint().expect("unknown").contains("Refresh"));
        let domain = e
            .pull_image("repair-domain", None)
            .next()
            .await
            .expect("error")
            .expect_err("domain");
        assert!(matches!(
            domain,
            EngineError::NotFound {
                kind: ResourceKind::Image,
                ..
            }
        ));
        let policy = e
            .pull_image("repair-policy", None)
            .next()
            .await
            .expect("error")
            .expect_err("policy");
        assert!(!policy.hint().is_some_and(|h| h.contains("Refresh")));
    }

    #[tokio::test]
    async fn eng_131_cli_auth_limitation_before_any_spawn_hook() {
        use futures::StreamExt;
        let e = fixture_engine(Runner::fixture().with_guard(Some(Arc::new(|| {
            panic!("auth rejection must not prepare or spawn a child")
        }))));
        let auth = RegistryAuth {
            server: "registry.invalid".into(),
            username: None,
            password: None,
            identity_token: None,
        };
        let error = e
            .pull_image("hello-world", Some(auth))
            .next()
            .await
            .expect("limitation")
            .expect_err("auth");
        assert!(matches!(error, EngineError::Api { status: 400, .. }));
    }

    #[test]
    fn cli_capabilities_match_spec_20_5_6() {
        let c = CLI_CAPABILITIES;
        for f in [
            Capabilities::EVENTS,
            Capabilities::LOGS_FOLLOW,
            Capabilities::NETWORK_MGMT,
            Capabilities::EXEC_TTY,
            Capabilities::EXEC_RESIZE,
        ] {
            assert!(c.contains(f));
        }
        for f in [
            Capabilities::PULL_PROGRESS,
            Capabilities::STATS_STREAM,
            Capabilities::PAUSE,
            Capabilities::TOP,
            Capabilities::IMAGE_HISTORY,
            Capabilities::DISK_USAGE,
        ] {
            assert!(!c.contains(f));
        }
    }

    #[test]
    fn actions() {
        let a = |act| action_args("web", &act);
        assert_eq!(
            a(ContainerAction::Start).ok(),
            Some(vec!["container".into(), "start".into(), "web".into()])
        );
        assert_eq!(
            a(ContainerAction::Stop { timeout_s: Some(5) }).ok(),
            Some(
                ["container", "stop", "--time", "5", "web"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            a(ContainerAction::Restart { timeout_s: Some(1) }).ok(),
            Some(
                ["container", "restart", "--timeout", "1", "web"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            a(ContainerAction::Kill {
                signal: Some("SIGHUP".into())
            })
            .ok(),
            Some(
                ["container", "kill", "--signal", "SIGHUP", "web"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert!(
            a(ContainerAction::Kill {
                signal: Some("-9".into())
            })
            .is_err()
        );
        assert_eq!(
            a(ContainerAction::Pause),
            Err(EngineError::Unsupported(Capabilities::PAUSE))
        );
        assert_eq!(
            a(ContainerAction::Unpause),
            Err(EngineError::Unsupported(Capabilities::PAUSE))
        );
    }

    #[test]
    fn nfr_022_run_args() {
        let spec = RunSpec {
            image: "nginx:alpine".into(),
            name: Some("web".into()),
            ports: vec![
                PortMapping {
                    ip: Some("127.0.0.1".parse().expect("ip")),
                    private: 80,
                    public: Some(8080),
                    proto: Proto::Tcp,
                },
                PortMapping {
                    ip: None,
                    private: 53,
                    public: Some(5353),
                    proto: Proto::Udp,
                },
                PortMapping {
                    ip: None,
                    private: 9000,
                    public: None,
                    proto: Proto::Tcp,
                },
                PortMapping {
                    ip: Some("::1".parse().expect("ip")),
                    private: 81,
                    public: Some(8081),
                    proto: Proto::Tcp,
                },
            ],
            env: vec![("FOO".into(), "bar baz".into())],
            mounts: vec![
                MountRequest {
                    kind: MountKind::Volume,
                    source: "data".into(),
                    target: "/data".into(),
                    read_only: true,
                },
                MountRequest {
                    kind: MountKind::Bind,
                    source: "C:\\work".into(),
                    target: "/w".into(),
                    read_only: false,
                },
                MountRequest {
                    kind: MountKind::Tmpfs,
                    source: String::new(),
                    target: "/tmp".into(),
                    read_only: false,
                },
            ],
            auto_remove: true,
            cmd: Some(vec!["echo".into(), "-n".into()]),
            labels: BTreeMap::from([("app".into(), "x".into())]),
        };
        let a = run_args(&spec).expect("args");
        assert_eq!(
            a,
            [
                "container",
                "run",
                "--detach",
                "--name",
                "web",
                "--publish",
                "127.0.0.1:8080:80/tcp",
                "--publish",
                "5353:53/udp",
                "--publish",
                "9000/tcp",
                "--publish",
                "[::1]:8081:81/tcp",
                "--env",
                "FOO=bar baz",
                "--volume",
                "data:/data:ro",
                "--volume",
                "C:\\work:/w",
                "--tmpfs",
                "/tmp",
                "--label",
                "app=x",
                "--rm",
                "nginx:alpine",
                "echo",
                "-n"
            ]
        );
        for bad in [
            RunSpec {
                image: "--privileged".into(),
                ..RunSpec::default()
            },
            RunSpec {
                image: "nginx".into(),
                name: Some("-x".into()),
                ..RunSpec::default()
            },
            RunSpec {
                image: "nginx".into(),
                env: vec![("-e".into(), "x".into())],
                ..RunSpec::default()
            },
            RunSpec {
                image: "nginx".into(),
                mounts: vec![MountRequest {
                    kind: MountKind::Volume,
                    source: "../x".into(),
                    target: "/d".into(),
                    read_only: false,
                }],
                ..RunSpec::default()
            },
            RunSpec {
                image: "nginx".into(),
                mounts: vec![MountRequest {
                    kind: MountKind::Bind,
                    source: "-v".into(),
                    target: "/d".into(),
                    read_only: false,
                }],
                ..RunSpec::default()
            },
        ] {
            assert!(run_args(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sessions_are_validated() {
        assert!(validate_session("wslc-cli-user").is_ok());
        assert!(validate_session("My Session").is_ok());
        for bad in ["", "--format", "a\nb"] {
            assert!(validate_session(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn inspect_ports_moved_under_network_settings() {
        let v: Value =
            serde_json::json!({"Ports": {"80/tcp": []}, "NetworkSettings": {"Networks": {}}});
        let v = dockerize_container_inspect(v);
        assert!(v.pointer("/NetworkSettings/Ports/80~1tcp").is_some());
    }

    #[test]
    fn counts_by_state() {
        let rows = parse::json_list(include_str!(
            "../../tests/fixtures/cli/container_list/list.ndjson"
        ))
        .expect("rows");
        let list: Vec<_> = rows.iter().map(parse::container_summary).collect();
        assert_eq!(
            counts(&list),
            ContainerCounts {
                running: 1,
                paused: 0,
                stopped: 1
            }
        );
    }
}
