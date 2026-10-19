//! Updater tests (UPD-004, 005, 011) on a paused clock with an in-memory source, plus the shared
//! fixture of `updates_offer_tests` (UPD-006, 007, 011, 012, 017).
//!
//! Nothing here touches the network, the real install kind, the real clock or the real
//! Authenticode check: the source is a [`StaticSource`] behind gates, the machine is a
//! [`TestHost`] whose clock follows the paused tokio clock, and the apply strategy is a fake.
//! Tests start with automatic checks **off** (`[updates] check = false`) unless they are about
//! the schedule: that keeps the paused clock from running a surprise check while a test waits.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dk_update::InstallKind;
use dk_update::UpdateError;
use dk_update::UpdateSource;
use dk_update::manifest::platform_key;
use dk_update::source::testing::StaticSource;
use futures::StreamExt;
use futures::future::BoxFuture;
use sha2::Digest;
use tempfile::TempDir;
use tokio::sync::watch;

use crate::config::{Config, UiState};
use crate::handle::{HubHandle, HubOptions};
use crate::hub::{HubInner, lock};
use crate::paths::Paths;
use crate::updates::fakes::FakeInstallers;
use crate::updates::{
    ApplyFailure, ApplyJob, ApplyResult, ApplyStrategy, FIRST_CHECK, Host, INTERVAL, JITTER_SECS,
    OfferMode, State, SystemStrategy, UpdateCheck, UpdateSnapshot, UpdateStatus, UpdaterInit,
    next_delay,
};

/// The installer every test release serves.
pub(crate) const FILE: &[u8] = b"installer bytes";
pub(crate) const DAY: Duration = Duration::from_secs(24 * 3600);
/// Long enough for anything event-driven; the paused clock only moves when nothing else can.
pub(crate) const SHORT: Duration = Duration::from_secs(60);

/// `std::env` is process-wide; the disable switch must not leak in from the environment.
pub(crate) fn updater_env_off() -> bool {
    std::env::var_os(crate::updates::DISABLE_ENV).is_some()
}

// ── a gate both sides can wait on ───────────────────────────────────────────────────────────

/// Holds a source call (or a strategy) until the test opens it, and counts the callers that
/// reached it, so a test can wait for "the download is in flight" without sleeping.
pub(crate) struct Gate {
    open: watch::Sender<bool>,
    hits: watch::Sender<usize>,
}

impl Gate {
    pub(crate) fn closed() -> Arc<Self> {
        Arc::new(Self {
            open: watch::channel(false).0,
            hits: watch::channel(0).0,
        })
    }
    async fn pass(&self) {
        self.hits.send_modify(|n| *n += 1);
        let mut open = self.open.subscribe();
        let _ = open.wait_for(|o| *o).await;
    }
    pub(crate) fn open(&self) {
        self.open.send_replace(true);
    }
    /// Resolves once `n` callers have reached the gate.
    pub(crate) async fn reached(&self, n: usize) {
        let mut hits = self.hits.subscribe();
        let _ = hits.wait_for(|h| *h >= n).await;
    }
}

// ── the source ──────────────────────────────────────────────────────────────────────────────

/// A [`StaticSource`] whose manifest fetch and download can be held and made to fail.
pub(crate) struct GatedSource {
    pub(crate) inner: StaticSource,
    manifest_gate: Mutex<Option<Arc<Gate>>>,
    download_gate: Mutex<Option<Arc<Gate>>>,
    download_error: Mutex<Option<UpdateError>>,
    downloads: AtomicUsize,
}

impl GatedSource {
    pub(crate) fn requests(&self) -> usize {
        self.inner.requests()
    }
    /// Downloads that actually started.
    pub(crate) fn downloads(&self) -> usize {
        self.downloads.load(Ordering::SeqCst)
    }
    pub(crate) fn hold_manifest(&self) -> Arc<Gate> {
        let gate = Gate::closed();
        *lock(&self.manifest_gate) = Some(gate.clone());
        gate
    }
    pub(crate) fn hold_downloads(&self) -> Arc<Gate> {
        let gate = Gate::closed();
        *lock(&self.download_gate) = Some(gate.clone());
        gate
    }
    pub(crate) fn fail_downloads(&self, error: Option<UpdateError>) {
        *lock(&self.download_error) = error;
    }
    pub(crate) fn fail_manifest(&self, error: UpdateError) {
        *lock(&self.inner.manifest) = Err(error);
    }
}

impl UpdateSource for GatedSource {
    fn fetch_manifest(&self) -> BoxFuture<'_, Result<(Vec<u8>, String), UpdateError>> {
        let gate = lock(&self.manifest_gate).clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.pass().await;
            }
            self.inner.fetch_manifest().await
        })
    }

    fn download<'a>(
        &'a self,
        asset: &'a dk_update::PlatformAsset,
        dir: &'a Path,
        file_name: &'a str,
        progress: Box<dyn FnMut(u64, u64) + Send + 'a>,
    ) -> BoxFuture<'a, Result<PathBuf, UpdateError>> {
        let gate = lock(&self.download_gate).clone();
        Box::pin(async move {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = gate {
                gate.pass().await;
            }
            if let Some(error) = lock(&self.download_error).clone() {
                return Err(error);
            }
            self.inner.download(asset, dir, file_name, progress).await
        })
    }
}

// ── the machine ─────────────────────────────────────────────────────────────────────────────

/// The machine the updater runs on, as tests want it: platform, install kind, AppImage target
/// and the Authenticode verdict are settable at any time; the clock is the paused tokio clock
/// added to a fixed date, so waiting 14 days is one `sleep`.
pub(crate) struct TestHost {
    pub(crate) platform: Mutex<String>,
    pub(crate) kind: Mutex<InstallKind>,
    pub(crate) appimage: Mutex<Option<PathBuf>>,
    /// `Some(error)`: the running exe's signature pin rejects every installer.
    pub(crate) trust: Mutex<Option<UpdateError>>,
    base: SystemTime,
    start: tokio::time::Instant,
}

impl TestHost {
    fn new(kind: InstallKind) -> Self {
        Self {
            platform: Mutex::new(platform_key()),
            kind: Mutex::new(kind),
            appimage: Mutex::new(Some(PathBuf::from("/opt/Dockering.AppImage"))),
            trust: Mutex::new(None),
            // 2027-01-15, the date the `offer.rs` tests use too.
            base: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            start: tokio::time::Instant::now(),
        }
    }
    pub(crate) fn set_kind(&self, kind: InstallKind) {
        *lock(&self.kind) = kind;
    }
}

impl Host for TestHost {
    fn platform(&self) -> String {
        lock(&self.platform).clone()
    }
    fn install_kind(&self) -> InstallKind {
        *lock(&self.kind)
    }
    fn appimage_target(&self) -> Option<PathBuf> {
        lock(&self.appimage).clone()
    }
    fn now(&self) -> SystemTime {
        self.base + self.start.elapsed()
    }
    fn check_installer(&self, _: &Path, _: &Path) -> Result<(), UpdateError> {
        lock(&self.trust).clone().map_or(Ok(()), Err)
    }
}

// ── the apply strategy ──────────────────────────────────────────────────────────────────────

/// A blocking-thread gate for a strategy that must stay inside `apply` while the test acts.
pub(crate) struct StdGate {
    open: Mutex<bool>,
    cv: Condvar,
}

impl StdGate {
    /// Blocks until opened; gives up after 10 real seconds so a test that failed before it
    /// opened the gate can't hang the runtime shutdown.
    fn wait(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut open = lock(&self.open);
        while !*open {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return;
            };
            open = self
                .cv
                .wait_timeout(open, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
    pub(crate) fn open(&self) {
        *lock(&self.open) = true;
        self.cv.notify_all();
    }
}

/// What the platform strategy does, as tests want it.
pub(crate) struct FakeStrategy {
    pub(crate) result: Mutex<Result<ApplyResult, ApplyFailure>>,
    pub(crate) jobs: Mutex<Vec<ApplyJob>>,
    gate: Mutex<Option<Arc<StdGate>>>,
    entered: watch::Sender<usize>,
    panics: Mutex<bool>,
}

impl FakeStrategy {
    fn new() -> Self {
        Self {
            result: Mutex::new(Ok(ApplyResult::Handoff)),
            jobs: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
            entered: watch::channel(0).0,
            panics: Mutex::new(false),
        }
    }
    pub(crate) fn set(&self, result: Result<ApplyResult, ApplyFailure>) {
        *lock(&self.result) = result;
    }
    pub(crate) fn fail_install(&self, message: &str) {
        self.set(Err(ApplyFailure {
            kind: crate::updates::FailureKind::InstallFailed,
            message: message.to_owned(),
        }));
    }
    pub(crate) fn fail_retryable(&self, message: &str) {
        self.set(Err(ApplyFailure::retryable(message)));
    }
    pub(crate) fn panic_next(&self) {
        *lock(&self.panics) = true;
    }
    /// The next `apply` blocks inside the strategy until the returned gate opens.
    pub(crate) fn hold(&self) -> Arc<StdGate> {
        let gate = Arc::new(StdGate {
            open: Mutex::new(false),
            cv: Condvar::new(),
        });
        *lock(&self.gate) = Some(gate.clone());
        gate
    }
    pub(crate) fn calls(&self) -> usize {
        lock(&self.jobs).len()
    }
    pub(crate) fn last_job(&self) -> ApplyJob {
        lock(&self.jobs).last().cloned().expect("a strategy call")
    }
    /// Resolves once the strategy has been entered `n` times.
    pub(crate) async fn entered(&self, n: usize) {
        let mut entered = self.entered.subscribe();
        let _ = entered.wait_for(|e| *e >= n).await;
    }
}

impl ApplyStrategy for FakeStrategy {
    fn apply(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure> {
        lock(&self.jobs).push(job.clone());
        self.entered.send_modify(|n| *n += 1);
        let gate = lock(&self.gate).take();
        if let Some(gate) = gate {
            gate.wait();
        }
        if std::mem::take(&mut *lock(&self.panics)) {
            panic!("the strategy exploded");
        }
        lock(&self.result).clone()
    }
}

// ── manifests ───────────────────────────────────────────────────────────────────────────────

/// The asset file name a release of `kind` publishes (REL-012).
fn asset_name(kind: &str) -> &'static str {
    match kind {
        "inno" => "Dockering-Setup-x64.exe",
        "appimage" => "Dockering-x86_64.AppImage",
        "dmg" => "Dockering-aarch64.dmg",
        "zip" => "Dockering-portable.zip",
        _ => "dockering_amd64.deb",
    }
}

/// The manifest entry kind a release for an install of `kind` carries.
fn entry_kind(kind: InstallKind) -> &'static str {
    match kind {
        InstallKind::InnoUser | InstallKind::InnoMachine => "inno",
        InstallKind::AppImage => "appimage",
        InstallKind::Portable => "zip",
        InstallKind::Other => "deb",
    }
}

fn sign(kp: &minisign::KeyPair, json: &[u8]) -> String {
    minisign::sign(None, &kp.sk, Cursor::new(json), None, None)
        .expect("sign")
        .into_string()
}

/// The lowercase hex SHA-256 of `bytes`, as a manifest carries it.
pub(crate) fn sha2_hex(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A signed manifest with one entry, `platform` → `kind`.
pub(crate) fn signed_manifest_for(
    kp: &minisign::KeyPair,
    version: &str,
    file: &[u8],
    platform: &str,
    kind: &str,
) -> (Vec<u8>, String) {
    let json = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "version": version,
        "pub_date": "2026-10-03T00:00:00Z",
        "notes_url": format!("https://github.com/pavel-purma/dockering/releases/tag/v{version}"),
        "platforms": { platform: {
            "kind": kind,
            "url": format!(
                "https://github.com/pavel-purma/dockering/releases/download/v{version}/{}",
                asset_name(kind)
            ),
            "sha256": hex::encode(sha2::Sha256::digest(file)),
            "size": file.len(),
        }},
    }))
    .expect("json");
    let sig = sign(kp, &json);
    (json, sig)
}

/// The installer manifest of the seam tests: this build's platform, kind `inno`.
#[cfg_attr(not(feature = "update-e2e"), allow(dead_code))]
pub(crate) fn signed_manifest(
    kp: &minisign::KeyPair,
    version: &str,
    file: &[u8],
) -> (Vec<u8>, String) {
    signed_manifest_for(kp, version, file, &platform_key(), "inno")
}

// ── the fixture ─────────────────────────────────────────────────────────────────────────────

/// Everything a hub on a temp dir needs, kept across restarts: a test can `launch` a second hub
/// on the same directory to see what the first one left behind.
pub(crate) struct Fixture {
    pub(crate) dir: TempDir,
    pub(crate) kp: minisign::KeyPair,
    pub(crate) source: Arc<GatedSource>,
    pub(crate) host: Arc<TestHost>,
    pub(crate) strategy: Arc<FakeStrategy>,
    /// The `dk-update` calls of the real strategy, faked: for tests of `SystemStrategy`.
    pub(crate) installers: Arc<FakeInstallers>,
    /// The bytes every release serves as its asset.
    pub(crate) file: Vec<u8>,
}

impl Fixture {
    /// An install of `kind`, serving nothing yet (call [`Fixture::publish`]).
    pub(crate) fn new(kind: InstallKind) -> Self {
        Self::with_payload(kind, |_| FILE.to_vec())
    }

    /// [`Fixture::new`] whose releases serve the bytes `payload` makes from the temp directory.
    pub(crate) fn with_payload(kind: InstallKind, payload: impl FnOnce(&Path) -> Vec<u8>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = payload(dir.path());
        let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
        let (json, sig) = signed_manifest_for(&kp, "0.0.1", &file, &platform_key(), "inno");
        Self {
            source: Arc::new(GatedSource {
                inner: StaticSource::new(json, sig, file.clone()),
                manifest_gate: Mutex::new(None),
                download_gate: Mutex::new(None),
                download_error: Mutex::new(None),
                downloads: AtomicUsize::new(0),
            }),
            host: Arc::new(TestHost::new(kind)),
            strategy: Arc::new(FakeStrategy::new()),
            installers: Arc::new(FakeInstallers::default()),
            dir,
            file,
            kp,
        }
    }

    pub(crate) fn updates_dir(&self) -> PathBuf {
        self.dir.path().join("updates")
    }

    /// The payload path of `version` for an install of `kind`.
    pub(crate) fn payload(&self, version: &str) -> PathBuf {
        let kind = entry_kind(*lock(&self.host.kind));
        self.updates_dir().join(version).join(asset_name(kind))
    }

    /// `latest` becomes `version`, with an entry for this host's platform and install kind.
    pub(crate) fn publish(&self, version: &str) {
        let platform = lock(&self.host.platform).clone();
        let kind = entry_kind(*lock(&self.host.kind));
        self.publish_entry(version, &platform, kind);
    }

    /// `latest` becomes `version` with exactly one entry, `platform` → `kind`.
    pub(crate) fn publish_entry(&self, version: &str, platform: &str, kind: &str) {
        let (json, sig) = signed_manifest_for(&self.kp, version, &self.file, platform, kind);
        *lock(&self.source.inner.manifest) = Ok((json, sig));
    }

    /// `latest` becomes a manifest for `version` with no entry for this platform.
    pub(crate) fn publish_without_entry(&self, version: &str) {
        self.publish_entry(version, "plan9-m68k", "deb");
    }

    pub(crate) fn launch(&self) -> Launch<'_> {
        Launch {
            fixture: self,
            config: quiet(),
            demo: false,
            running: "0.1.0",
            keys: None,
            exe: None,
            strategy: StrategyKind::Fake,
        }
    }

    /// A hub with automatic checks off, running version 0.1.0.
    pub(crate) fn start(&self) -> HubHandle {
        self.launch().go()
    }
}

/// `[updates] check = false`: no schedule, but *Check now* and the retained offer work.
pub(crate) fn quiet() -> Config {
    let mut config = Config::default();
    config.updates.check = false;
    config
}

/// Which strategy a launched hub applies with.
#[derive(Clone, Copy)]
enum StrategyKind {
    /// [`FakeStrategy`]: the platform is out of the picture.
    Fake,
    /// The real [`SystemStrategy`] on the fixture's fake `dk-update` calls.
    SystemOnFakes,
    /// The real [`SystemStrategy`] on the real `dk-update` calls (Linux AppImage tests).
    SystemOnReal,
}

pub(crate) struct Launch<'a> {
    fixture: &'a Fixture,
    config: Config,
    demo: bool,
    running: &'static str,
    keys: Option<Vec<String>>,
    exe: Option<PathBuf>,
    strategy: StrategyKind,
}

impl Launch<'_> {
    pub(crate) fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }
    pub(crate) fn demo(mut self) -> Self {
        self.demo = true;
        self
    }
    /// The version of the running app.
    pub(crate) fn running(mut self, version: &'static str) -> Self {
        self.running = version;
        self
    }
    /// A build with no trust keys: updates fail closed before any network access.
    pub(crate) fn no_keys(mut self) -> Self {
        self.keys = Some(Vec::new());
        self
    }
    /// The running exe is (Authenticode-)signed: the host's verdict applies to every installer.
    pub(crate) fn signed_exe(mut self) -> Self {
        self.exe = Some(PathBuf::from("dockering.exe"));
        self
    }
    /// Apply with the real `SystemStrategy` on the fixture's fake `dk-update` calls.
    pub(crate) fn system_on_fakes(mut self) -> Self {
        self.strategy = StrategyKind::SystemOnFakes;
        self
    }
    /// Apply with the real `SystemStrategy` on the real `dk-update` calls.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn system_on_real(mut self) -> Self {
        self.strategy = StrategyKind::SystemOnReal;
        self
    }
    pub(crate) fn go(self) -> HubHandle {
        let f = self.fixture;
        let keys = self.keys.unwrap_or_else(|| vec![f.kp.pk.to_base64()]);
        let mut state = State::new(f.source.clone(), f.host.clone(), self.running, keys);
        // The test binary and installer are unsigned: the Authenticode pin is skipped unless a
        // test gives the state a running exe (and the host a verdict).
        state.running_exe = self.exe;
        state.strategy = match self.strategy {
            StrategyKind::Fake => f.strategy.clone(),
            StrategyKind::SystemOnFakes => Arc::new(SystemStrategy::with(f.installers.clone())),
            StrategyKind::SystemOnReal => Arc::new(SystemStrategy::production()),
        };
        launch(self.config, self.demo, f.dir.path(), state)
    }
}

fn launch(config: Config, demo: bool, dir: &Path, state: State) -> HubHandle {
    let opts = HubOptions {
        paths: Paths::in_dir(dir),
        config,
        ui_state: UiState::default(),
        factories: Some(vec![]),
        discover_on_start: false,
        worker_threads: 2,
        demo,
    };
    HubInner::start_with(
        opts,
        tokio::runtime::Handle::current(),
        None,
        UpdaterInit::State(Box::new(state)),
    )
}

// ── waiting ─────────────────────────────────────────────────────────────────────────────────

/// The first snapshot (from now on) that satisfies `f`.
pub(crate) async fn until(
    hub: &HubHandle,
    max: Duration,
    f: impl Fn(&UpdateSnapshot) -> bool,
) -> UpdateSnapshot {
    let mut s = hub.update_status();
    tokio::time::timeout(max, async {
        while let Some(Ok(item)) = s.next().await {
            if f(&item) {
                return item;
            }
        }
        panic!("status stream ended");
    })
    .await
    .expect("snapshot not reached in time")
}

/// The current snapshot (the stream replays it first).
pub(crate) async fn current(hub: &HubHandle) -> UpdateSnapshot {
    hub.update_status()
        .next()
        .await
        .expect("a snapshot")
        .expect("no error")
}

// ── the tests that were here before the retained offer ──────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_004_first_check_after_30s_then_daily() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::Portable);
    f.publish("0.2.0");
    let hub = f.launch().config(Config::default()).go();
    tokio::time::sleep(FIRST_CHECK - Duration::from_secs(1)).await;
    assert_eq!(f.source.requests(), 0, "no request before 30 s");
    tokio::time::sleep(Duration::from_secs(2)).await;
    // A portable install gets a *Download* offer: the manifest only, never the asset (UPD-006).
    let snap = until(&hub, SHORT, |s| s.offer.is_some()).await;
    let offer = snap.offer.expect("offer");
    assert_eq!(
        (offer.version.as_str(), offer.mode),
        ("0.2.0", OfferMode::Download)
    );
    assert_eq!(f.source.requests(), 1);

    tokio::time::sleep(INTERVAL - Duration::from_secs(JITTER_SECS + 60)).await;
    assert_eq!(f.source.requests(), 1, "no second check before ~24 h");
    tokio::time::sleep(Duration::from_secs(2 * JITTER_SECS + 120)).await;
    assert_eq!(f.source.requests(), 2, "second check within 24 h ± 1 h");
}

#[tokio::test(start_paused = true)]
async fn upd_005_disabled_makes_no_requests() {
    let mut off = Config::default();
    off.updates.check = false;
    for (config, demo) in [(off, false), (Config::default(), true)] {
        let f = Fixture::new(InstallKind::InnoUser);
        f.publish("0.2.0");
        let hub = f.launch().config(config).demo_if(demo).go();
        tokio::time::sleep(INTERVAL * 2).await;
        assert_eq!(f.source.requests(), 0);
        if demo {
            // Demo mode has no updater at all.
            assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::Disabled));
            let snap = current(&hub).await;
            assert_eq!(snap, UpdateSnapshot::unavailable());
        }
        assert_eq!(f.source.requests(), 0);
    }
}

impl Launch<'_> {
    fn demo_if(self, demo: bool) -> Self {
        if demo { self.demo() } else { self }
    }
}

#[tokio::test(start_paused = true)]
async fn upd_005_no_keys_fails_closed_before_network() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    f.publish("0.2.0");
    // Even with automatic checks on, even for a manual check: no key, no request.
    let hub = f.launch().config(Config::default()).no_keys().go();
    tokio::time::sleep(INTERVAL * 2).await;
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::Disabled));
    assert_eq!(f.source.requests(), 0);
    let snap = current(&hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: crate::DisabledReason::Unavailable
        }
    );
    assert_eq!(snap.offer, None);
}

#[tokio::test(start_paused = true)]
async fn upd_007_manual_check_downloads_and_is_ready() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    f.publish("0.2.0");
    let hub = f.start();
    assert_eq!(
        hub.check_for_updates().await,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
    let snap = until(&hub, Duration::from_secs(1), |s| {
        s.offer
            .as_ref()
            .is_some_and(|o| o.stage == crate::OfferStage::Ready)
    })
    .await;
    let offer = snap.offer.expect("offer");
    assert_eq!(offer.version, "0.2.0");
    assert_eq!(offer.mode, OfferMode::Install);
    assert!(!offer.needs_elevation && !offer.install_failed);
    // A second check reuses the verified download.
    assert_eq!(
        hub.check_for_updates().await,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
    // The manifest is fetched again (a newer release could replace the offer), but the
    // verified download is reused.
    assert_eq!(f.source.requests(), 3, "2 manifests + one download");

    // Turning automatic checks off doesn't hide an update that's ready (UPD-007).
    hub.config().update(|c| c.updates.check = false);
    let snap = until(&hub, Duration::from_secs(1), |s| {
        matches!(s.status, UpdateStatus::Disabled { .. })
    })
    .await;
    assert_eq!(
        snap.offer.as_ref().map(|o| o.stage),
        Some(crate::OfferStage::Ready)
    );
    // The offer is kept in `offer.json`, not in `state.json` (UPD-012).
    let stored = dk_update::offer::read(&f.updates_dir())
        .expect("readable")
        .expect("offer.json");
    assert_eq!(
        stored.version().map(|v| v.to_string()).as_deref(),
        Some("0.2.0")
    );
}

#[tokio::test(start_paused = true)]
async fn upd_004_same_version_is_up_to_date() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    f.publish("0.1.0");
    let hub = f.start();
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    let snap = until(&hub, Duration::from_secs(1), |s| {
        matches!(
            s.status,
            UpdateStatus::Idle {
                last_check: Some(_)
            }
        ) || matches!(s.status, UpdateStatus::Disabled { .. })
    })
    .await;
    assert_eq!(snap.offer, None);
    // A default config shows `Idle` with the time of the check.
    let f = Fixture::new(InstallKind::InnoUser);
    f.publish("0.1.0");
    let hub = f.launch().config(Config::default()).go();
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    until(&hub, Duration::from_secs(1), |s| {
        matches!(
            s.status,
            UpdateStatus::Idle {
                last_check: Some(_)
            }
        )
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn upd_011_stale_manual_check_dropped() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::Portable);
    f.publish("0.2.0");
    let hub = f.start();
    // Hold the check lock so both requests are queued before either runs.
    let updates = hub.inner.updates.as_ref().expect("updater");
    let guard = updates.busy.lock().await;
    let first = hub.check_for_updates();
    let second = hub.check_for_updates();
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(guard);
    let (a, b) = futures::join!(first, second);
    assert_eq!(a, Err(dk_core::EngineError::Cancelled), "superseded check");
    assert_eq!(
        b,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
}

#[tokio::test(start_paused = true)]
async fn upd_008_previous_version_and_notified_once() {
    let f = Fixture::new(InstallKind::Portable);
    f.publish("0.2.0");
    let hub = f.launch().demo().go();
    hub.config()
        .update_ui_state(|s| s.updates.last_run_version = Some("0.0.1".into()));
    assert_eq!(hub.take_previous_version(), Some("0.0.1".into()));
    assert_eq!(hub.take_previous_version(), None, "only once");
    assert!(hub.mark_update_notified("0.3.0"));
    assert!(!hub.mark_update_notified("0.3.0"));
    assert!(hub.mark_update_notified("0.4.0"));
}

#[test]
fn upd_004_next_delay_stays_within_the_jitter_window() {
    let day = Duration::from_secs(24 * 3600);
    for r in [
        0,
        1,
        7,
        3599,
        3600,
        3601,
        7200,
        7201,
        u64::MAX / 2,
        u64::MAX - 1,
        u64::MAX,
    ] {
        let d = next_delay(INTERVAL, JITTER_SECS, r);
        assert!(
            d >= day - Duration::from_secs(JITTER_SECS)
                && d <= day + Duration::from_secs(JITTER_SECS),
            "r={r}: {d:?}"
        );
    }
    // The extremes are reachable and the middle is the plain interval.
    assert_eq!(
        next_delay(INTERVAL, JITTER_SECS, 0),
        day - Duration::from_secs(3600)
    );
    assert_eq!(next_delay(INTERVAL, JITTER_SECS, 3600), day);
    assert_eq!(
        next_delay(INTERVAL, JITTER_SECS, 7200),
        day + Duration::from_secs(3600)
    );
    // No jitter: exactly the interval, whatever the random number.
    for r in [0, 1, u64::MAX] {
        assert_eq!(
            next_delay(Duration::from_secs(10), 0, r),
            Duration::from_secs(10)
        );
    }
    // A short interval with a long jitter, or absurd inputs, must not panic or underflow.
    assert_eq!(next_delay(Duration::from_secs(10), 3600, 0), Duration::ZERO);
    let _ = next_delay(Duration::MAX, u64::MAX, u64::MAX);
    let _ = next_delay(Duration::ZERO, u64::MAX, 0);
}

#[tokio::test(start_paused = true)]
async fn upd_004_schedule_fields_drive_first_check_and_interval() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::Portable);
    f.publish("0.2.0");
    let mut state = State::new(
        f.source.clone(),
        f.host.clone(),
        "0.1.0",
        vec![f.kp.pk.to_base64()],
    );
    state.running_exe = None;
    state.first_check = Duration::from_secs(5);
    state.interval = Duration::from_secs(20);
    state.jitter_secs = 0;
    let hub = launch(Config::default(), false, f.dir.path(), state);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        f.source.requests(),
        0,
        "nothing before the first-check delay"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    until(&hub, Duration::from_secs(1), |s| s.offer.is_some()).await;
    assert_eq!(f.source.requests(), 1, "first check at 5 s");
    tokio::time::sleep(Duration::from_secs(18)).await;
    assert_eq!(f.source.requests(), 1, "next one 20 s after the first");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.source.requests(), 2);
}

fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| map.get(name).cloned()
}

fn embedded_keys() -> Vec<String> {
    dk_update::keys::PUBLIC_KEYS
        .iter()
        .map(|k| (*k).to_owned())
        .collect()
}

/// The state production code starts with, built from a lookup that knows nothing.
fn assert_production_defaults(state: &State) {
    assert_eq!(state.keys, embedded_keys());
    assert_eq!(state.first_check, FIRST_CHECK);
    assert_eq!(state.interval, INTERVAL);
    assert_eq!(state.jitter_secs, JITTER_SECS);
    assert_eq!(
        state.current,
        semver::Version::parse(crate::app_version()).expect("app version is semver")
    );
}

#[test]
fn upd_005_e2e_seam_inert_without_env() {
    let state = State::production_with(&env_map(&[])).expect("production state");
    assert_production_defaults(&state);
    // Empty and whitespace-only values are as good as unset.
    let blank = env_map(&[
        ("DOCKERING_UPDATE_E2E_URL", ""),
        ("DOCKERING_UPDATE_E2E_KEY", "  "),
        ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", ""),
        ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", " "),
        ("DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS", ""),
    ]);
    assert_production_defaults(&State::production_with(&blank).expect("production state"));
}

/// Without the Cargo feature the variables are not even looked at.
#[cfg(not(feature = "update-e2e"))]
#[test]
fn upd_005_e2e_seam_ignored_without_the_feature() {
    let all = env_map(&[
        ("DOCKERING_UPDATE_E2E_URL", "http://127.0.0.1:9"),
        ("DOCKERING_UPDATE_E2E_KEY", "RWQ0123456789abcdef"),
        ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", "1"),
        ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", "2"),
        ("DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS", "3"),
    ]);
    assert_production_defaults(&State::production_with(&all).expect("production state"));
}

#[cfg(feature = "update-e2e")]
mod seam {
    use dk_update::source::testing::{LoopbackServer, Reply};

    use super::*;

    fn server_path(url: &str) -> String {
        url.trim_start_matches("https://github.com").to_owned()
    }

    #[tokio::test]
    async fn upd_005_e2e_seam_applies_injected_env() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
        let (manifest, signature) = signed_manifest(&kp, "0.2.0", b"installer bytes");
        let server = LoopbackServer::start(vec![
            (
                server_path(dk_update::MANIFEST_URL),
                Reply::Body(manifest.clone()),
            ),
            (
                server_path(dk_update::SIGNATURE_URL),
                Reply::Body(signature.clone().into_bytes()),
            ),
        ])
        .expect("loopback server");
        let public_key = kp.pk.to_base64();
        let env = env_map(&[
            ("DOCKERING_UPDATE_E2E_URL", &server.base_url()),
            ("DOCKERING_UPDATE_E2E_KEY", &public_key),
            ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", "1"),
            ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", "2"),
        ]);
        let state = State::production_with(&env).expect("seam state");
        assert_eq!(state.keys, vec![public_key.clone()]);
        assert_eq!(state.first_check, Duration::from_secs(1));
        assert_eq!(state.interval, Duration::from_secs(2));
        assert_eq!(
            state.jitter_secs, 0,
            "a seconds-scale interval carries no jitter"
        );

        // The real client fetches the signed manifest from the loopback server ...
        let (json, sig) = state.source.fetch_manifest().await.expect("fetch");
        assert_eq!((&json, &sig), (&manifest, &signature));
        // ... the seam key verifies it, and the embedded keys don't.
        let keys: Vec<&str> = state.keys.iter().map(String::as_str).collect();
        dk_update::verify::verify_manifest(&json, &sig, &keys).expect("signed by the seam key");
        let embedded = embedded_keys();
        let embedded: Vec<&str> = embedded.iter().map(String::as_str).collect();
        assert!(dk_update::verify::verify_manifest(&json, &sig, &embedded).is_err());
        // The User-Agent carries app_version() (UPD-010).
        let expected_agent = format!(
            "Dockering/{} ({}; {})",
            crate::app_version(),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|r| r.header("user-agent") == Some(expected_agent.as_str()))
        );
    }

    #[test]
    fn upd_005_e2e_seam_unusable_base_fails_closed() {
        // An explicit but invalid base must not fall back to talking to GitHub.
        let env = env_map(&[("DOCKERING_UPDATE_E2E_URL", "not a url")]);
        assert!(State::production_with(&env).is_none());
    }
}
