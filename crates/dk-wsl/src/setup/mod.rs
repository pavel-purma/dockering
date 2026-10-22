//! Setting up a dedicated Docker-in-WSL distro (ENG-150…157, 160, 162, 163, 164).
//!
//! `offers` decides whether the offer exists; `start` runs one setup as a spawned task that
//! reports through a channel. Dropping the returned stream cancels the task, and the task
//! cleans up after itself (cache files, and the distro only if this run imported it).

mod download;
mod import;
mod messages;
mod pins;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use dk_core::{
    EngineConfig, EngineError, EngineResult, EngineStream, SetupOffer, SetupPhase, SetupProgress,
};
use futures::SinkExt;
use futures::Stream;
use futures::StreamExt;
use futures::channel::mpsc;
use tokio_util::sync::{CancellationToken, DropGuard};

use self::download::{FetchError, Source};
use self::import::{CleanupGuard, RunState, Step};
use self::pins::Pin as RootfsPin;
use crate::discovery::BridgeTool;
use crate::runner::{Wsl, WslVersion, ping_ok};

pub(crate) const OFFER_ID: &str = "docker-in-wsl";
/// The one distro name both images use (ENG-155).
pub(crate) const DISTRO: &str = "dockering-engine";

const OFFER_TITLE: &str = "Set up Docker in WSL";
/// Consent text (ENG-151); the dialog adds the sizes of the selected image.
const CONSENT: &str = "Dockering will create a Linux distro named \"dockering-engine\" that runs \
Docker Engine. It is stored in %LOCALAPPDATA%\\Dockering\\wsl\\dockering-engine and downloaded \
from github.com. No administrator rights are needed, and your other distros are not touched.";
/// The lifetime sentence shown in the consent and the start prompt (ENG-163).
pub(crate) const LIFETIME: &str =
    "The engine and its containers run only while Dockering, or another WSL session, is open.";

/// Conservative gate for `systemd=true` and the `[boot]` command (spike W-3.9).
const MIN_WSL: WslVersion = (2, 0, 0, 0);
const WORKING_MARGIN: u64 = 1 << 30;
const IMPORT_TIMEOUT: Duration = Duration::from_secs(300);
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(500);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(2);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// A re-check of the cached file slower than this is shown as *Verifying*.
const RECHECK_VISIBLE: Duration = Duration::from_millis(300);

type FreeSpace = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;
#[cfg(test)]
type AfterVerify = dyn Fn(&Path) + Send + Sync;

/// Everything outside the flow's logic: processes, disk roots, the network and the table.
pub(crate) struct SetupEnv {
    pub(crate) wsl: Arc<dyn Wsl>,
    pub(crate) distro: String,
    /// `%LOCALAPPDATA%\Dockering\cache`
    pub(crate) download_root: PathBuf,
    /// `%LOCALAPPDATA%\Dockering\wsl`; the distro folder is `<wsl_root>\<distro>`.
    pub(crate) wsl_root: PathBuf,
    /// `None` builds the production client when a setup runs.
    pub(crate) source: Option<Source>,
    pub(crate) pins: Vec<RootfsPin>,
    pub(crate) arch: &'static str,
    pub(crate) free_space: Box<FreeSpace>,
    /// Wait for the WSL service to settle after a cancelled import.
    pub(crate) settle: Duration,
    pub(crate) ready_poll: Duration,
    pub(crate) ready_timeout: Duration,
    pub(crate) import_timeout: Duration,
    /// Serialises runs, so a new one waits for the previous one's cleanup.
    gate: Arc<tokio::sync::Mutex<()>>,
    /// Test seam: runs on the cached file after the download was verified.
    #[cfg(test)]
    pub(crate) after_verify: Option<Box<AfterVerify>>,
}

impl std::fmt::Debug for SetupEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupEnv")
            .field("distro", &self.distro)
            .field("arch", &self.arch)
            .finish_non_exhaustive()
    }
}

impl SetupEnv {
    pub(crate) fn new(
        wsl: Arc<dyn Wsl>,
        download_root: PathBuf,
        wsl_root: PathBuf,
        free_space: Box<FreeSpace>,
    ) -> Self {
        Self {
            wsl,
            distro: DISTRO.to_owned(),
            download_root,
            wsl_root,
            source: None,
            pins: pins::PINS.to_vec(),
            arch: std::env::consts::ARCH,
            free_space,
            settle: SETTLE,
            ready_poll: READY_POLL,
            ready_timeout: READY_TIMEOUT,
            import_timeout: IMPORT_TIMEOUT,
            gate: Arc::default(),
            #[cfg(test)]
            after_verify: None,
        }
    }

    /// Roots under `%LOCALAPPDATA%\Dockering`; `None` off Windows or without that variable.
    pub(crate) fn production(wsl: Arc<dyn Wsl>) -> Option<Self> {
        let (cache, wsl_root) = production_roots()?;
        Some(Self::new(wsl, cache, wsl_root, Box::new(free_space_of)))
    }

    /// A published, usable pin for this CPU and a `choice_id` that is a key of the table.
    fn pin_for(&self, choice_id: &str) -> Option<&RootfsPin> {
        pins::find(&self.pins, choice_id, self.arch).filter(|pin| pin.usable())
    }

    fn distro_dir(&self) -> PathBuf {
        self.wsl_root.join(&self.distro)
    }
}

#[cfg(windows)]
fn production_roots() -> Option<(PathBuf, PathBuf)> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Dockering");
    Some((base.join("cache"), base.join("wsl")))
}

#[cfg(not(windows))]
fn production_roots() -> Option<(PathBuf, PathBuf)> {
    None
}

#[cfg(windows)]
fn free_space_of(path: &Path) -> io::Result<u64> {
    crate::win32::free_space(path)
}

#[cfg(not(windows))]
fn free_space_of(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "not Windows"))
}

/// Bytes needed on the target volume (ENG-153).
fn required_bytes(pin: &RootfsPin) -> u64 {
    pin.size_bytes + pin.disk_bytes + WORKING_MARGIN
}

/// The conditions an offer needs, checked again when a setup runs (ENG-150, ENG-153 step 1).
async fn preflight(env: &SetupEnv) -> EngineResult<()> {
    if !env.wsl.supported() {
        return Err(messages::not_windows());
    }
    let Some(version) = env.wsl.wsl_version().await else {
        return Err(messages::wsl_missing());
    };
    if version < MIN_WSL {
        return Err(messages::wsl_too_old(&format!(
            "{}.{}.{}",
            version.0, version.1, version.2
        )));
    }
    let names = env
        .wsl
        .registered_distros()
        .await
        .map_err(|error| import::map_io(&error))?;
    if names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&env.distro))
    {
        return Err(messages::already_exists(&env.distro));
    }
    Ok(())
}

/// ENG-150: the offer, or nothing when any condition fails (ENG-162: published pins only).
pub(crate) async fn offers(env: Option<&SetupEnv>) -> Vec<SetupOffer> {
    let Some(env) = env else {
        return Vec::new();
    };
    let choices = pins::choices(&env.pins, env.arch);
    if choices.is_empty() || preflight(env).await.is_err() {
        return Vec::new();
    }
    vec![SetupOffer {
        id: OFFER_ID.to_owned(),
        title: OFFER_TITLE.to_owned(),
        body: format!("{CONSENT} {LIFETIME}"),
        action_label: OFFER_TITLE.to_owned(),
        choices,
    }]
}

struct SetupStream {
    rx: mpsc::Receiver<EngineResult<SetupProgress>>,
    /// Dropping the stream cancels the task.
    _cancel: DropGuard,
}

impl Stream for SetupStream {
    type Item = EngineResult<SetupProgress>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}

fn error_stream(error: EngineError) -> EngineStream<SetupProgress> {
    dk_core::engine::error_stream(error)
}

/// ENG-154: validates the request without I/O, then runs the setup on a spawned task.
pub(crate) fn start(
    env: Option<Arc<SetupEnv>>,
    offer_id: &str,
    choice_id: &str,
) -> EngineStream<SetupProgress> {
    if offer_id != OFFER_ID {
        return error_stream(messages::rejected("that setup isn't available"));
    }
    let Some(env) = env else {
        return error_stream(messages::not_windows());
    };
    let Some(pin) = env.pin_for(choice_id).copied() else {
        return error_stream(messages::rejected("choose one of the offered options"));
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return error_stream(messages::stopped_unexpectedly());
    };
    let token = CancellationToken::new();
    let (tx, rx) = mpsc::channel(32);
    runtime.spawn(run(env, pin, token.clone(), tx));
    Box::pin(SetupStream {
        rx,
        _cancel: token.drop_guard(),
    })
}

async fn run(
    env: Arc<SetupEnv>,
    pin: RootfsPin,
    token: CancellationToken,
    mut tx: mpsc::Sender<EngineResult<SetupProgress>>,
) {
    let state = Arc::new(RunState::default());
    let part = env.download_root.join(format!("{}.part", pin.file_name()));
    let file = env.download_root.join(pin.file_name());
    let mut guard = CleanupGuard {
        env: env.clone(),
        state: state.clone(),
        files: vec![part.clone(), file.clone()],
        done: false,
        turn: None,
    };
    // One run at a time; the turn is held until this run's cleanup is finished.
    let turn = tokio::select! {
        biased;
        () = token.cancelled() => None,
        turn = env.gate.clone().lock_owned() => Some(turn),
    };
    let result = if turn.is_none() {
        Err(EngineError::Cancelled)
    } else {
        guard.turn = turn;
        tokio::select! {
            biased;
            () = token.cancelled() => Err(EngineError::Cancelled),
            r = flow(&env, &pin, &state, &part, &file, tx.clone()) => r,
        }
    };
    // A run cancelled at the very end is undone like any other cancel.
    guard.finish(result.is_ok() && !token.is_cancelled()).await;
    match result {
        Ok(config) => {
            let _ = tx.send(Ok(SetupProgress::Done(config))).await;
        }
        Err(EngineError::Cancelled) => {}
        Err(error) => {
            let _ = tx.send(Err(error)).await;
        }
    }
}

type Events = mpsc::Sender<EngineResult<SetupProgress>>;

async fn phase(tx: &mut Events, phase: SetupPhase, fraction: Option<f32>, detail: &str) {
    let _ = tx
        .send(Ok(SetupProgress::Phase {
            phase,
            fraction,
            detail: detail.to_owned(),
        }))
        .await;
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

async fn flow(
    env: &SetupEnv,
    pin: &RootfsPin,
    state: &RunState,
    part: &Path,
    file: &Path,
    mut tx: Events,
) -> EngineResult<EngineConfig> {
    let drive = messages::drive_label(&env.download_root);

    // 1. Checking
    phase(&mut tx, SetupPhase::Checking, None, "Checking your system").await;
    preflight(env).await?;
    for dir in [&env.wsl_root, &env.download_root] {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|_| messages::cannot_save(&drive))?;
    }
    check_leaf(&env.distro_dir()).await?;
    let needed = required_bytes(pin);
    match (env.free_space)(&env.wsl_root) {
        Ok(free) if free < needed => return Err(messages::low_disk(&drive, needed)),
        Ok(_) => {}
        Err(error) => tracing::debug!(%error, "free space unavailable; continuing"),
    }
    for stale in [part, file] {
        let _ = tokio::fs::remove_file(stale).await;
    }

    // 2. Downloading
    phase(
        &mut tx,
        SetupPhase::Downloading,
        Some(0.0),
        &format!("0 MB of {}", megabytes(pin.size_bytes)),
    )
    .await;
    let source = match &env.source {
        Some(source) => source.clone(),
        None => Source::production().map_err(|e| map_fetch(e, &drive))?,
    };
    let mut progress_tx = tx.clone();
    let mut last: Option<Instant> = None;
    let total = pin.size_bytes;
    let fetched = download::fetch(&source, pin, part, |received| {
        if received < total && last.is_some_and(|at| at.elapsed() < PROGRESS_INTERVAL) {
            return;
        }
        last = Some(Instant::now());
        let _ = progress_tx.try_send(Ok(SetupProgress::Phase {
            phase: SetupPhase::Downloading,
            fraction: Some((received as f64 / total as f64).min(1.0) as f32),
            detail: format!("{} of {}", megabytes(received), megabytes(total)),
        }));
    })
    .await
    .map_err(|e| map_fetch(e, &drive))?;

    // 3. Verifying
    phase(
        &mut tx,
        SetupPhase::Verifying,
        None,
        "Checking the download",
    )
    .await;
    if !fetched.sha256.eq_ignore_ascii_case(pin.sha256) {
        let _ = tokio::fs::remove_file(part).await;
        return Err(messages::verification_failed());
    }
    tokio::fs::rename(part, file)
        .await
        .map_err(|_| messages::cannot_save(&drive))?;

    // 4. Importing: never over an existing distro, and `imported` is set before the spawn.
    preflight(env).await?;
    #[cfg(test)]
    if let Some(hook) = &env.after_verify {
        hook(file);
    }
    recheck(&mut tx, file, pin).await?;
    phase(&mut tx, SetupPhase::Importing, None, "Creating the engine").await;
    let dir = env.distro_dir();
    let (Some(dir_arg), Some(file_arg)) = (dir.to_str(), file.to_str()) else {
        return Err(messages::fail(
            "The engine's folder has an unsupported name.",
            "Move your Windows profile to a folder with a plain name and try again.",
        ));
    };
    state.imported.store(true, Ordering::SeqCst);
    state.importing.store(true, Ordering::SeqCst);
    let output = env
        .wsl
        .run_timeout(
            &["--import", &env.distro, dir_arg, file_arg, "--version", "2"],
            env.import_timeout,
        )
        .await;
    state.importing.store(false, Ordering::SeqCst);
    let output = output.map_err(|e| import::map_io(&e))?;
    if !output.success() {
        // Without our own disk nothing of ours was created: never unregister that name.
        let ours = import::created_our_disk(env, &output).await;
        state.imported.store(ours, Ordering::SeqCst);
        return Err(import::map_failure(Step::Import, &output, env, &drive));
    }

    // 5. Starting
    phase(&mut tx, SetupPhase::Starting, None, "Starting Docker").await;
    let output = env
        .wsl
        .run_timeout(&["-d", &env.distro, "--exec", "true"], BOOT_TIMEOUT)
        .await
        .map_err(|e| import::map_io(&e))?;
    if !output.success() {
        return Err(import::map_failure(Step::Boot, &output, env, &drive));
    }

    // 6. Wait for Docker to answer `GET /_ping` (the socket file is not a readiness signal).
    let deadline = tokio::time::Instant::now() + env.ready_timeout;
    loop {
        let reply = env
            .wsl
            .ping(&env.distro, BridgeTool::Docker)
            .await
            .unwrap_or_default();
        if ping_ok(&reply) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(messages::docker_not_ready(env.ready_timeout.as_secs()));
        }
        tokio::time::sleep(env.ready_poll).await;
    }

    // 7. The config discovery would produce for this distro.
    Ok(crate::factory::discovered_config(&env.distro))
}

/// The bytes about to be imported must still match the pin (the cache is writable by others).
async fn recheck(tx: &mut Events, file: &Path, pin: &RootfsPin) -> EngineResult<()> {
    let hash = download::sha256_file(file);
    tokio::pin!(hash);
    let digest = tokio::select! {
        digest = &mut hash => digest,
        () = tokio::time::sleep(RECHECK_VISIBLE) => {
            phase(tx, SetupPhase::Verifying, None, "Checking the download").await;
            hash.await
        }
    };
    if digest.is_ok_and(|d| d.eq_ignore_ascii_case(pin.sha256)) {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(file).await;
    Err(messages::verification_failed())
}

/// The leaf may be absent or an empty directory (a stray file there fails the import).
async fn check_leaf(dir: &Path) -> EngineResult<()> {
    match tokio::fs::read_dir(dir).await {
        Ok(mut entries) => match entries.next_entry().await {
            Ok(None) => Ok(()),
            _ => Err(messages::folder_in_use(dir)),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(messages::folder_in_use(dir)),
    }
}

fn map_fetch(error: FetchError, drive: &str) -> EngineError {
    match error {
        FetchError::Network(message) => {
            tracing::debug!(%message, "setup download failed");
            messages::no_network()
        }
        FetchError::Http(status) => messages::http_status(status),
        FetchError::TooLarge => messages::verification_failed(),
        FetchError::BadUrl => messages::rejected("the download address isn't allowed"),
        FetchError::Io(error) => {
            tracing::debug!(%error, "setup download couldn't be written");
            messages::cannot_save(drive)
        }
    }
}
