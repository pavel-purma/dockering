//! Tier 1 of the update end-to-end plan (REL-085; `docs/plan/features/linux-appimage-updates.md`
//! §6.5 and §8; UPD-001…004, 006, 011…014, 017): the whole mechanism **below the UI** over
//! **real HTTP**. A real `reqwest` client (`GithubSource::with_base`) talks to an in-process
//! [`LoopbackServer`] that serves a signed `dockering-update.json`, its `.minisig` and the
//! asset at the paths GitHub uses; the hub (`State`) checks, downloads, verifies, retains the
//! offer in `offer.json` and applies it. Every scenario asserts the hub's snapshot **and** the
//! server's request log (counts per kind and the exact `User-Agent`).
//!
//! # Techniques
//!
//! * **Real clock, short schedules.** These tests run on the *real* tokio clock
//!   (`#[tokio::test]`, never `start_paused`): a paused clock advances whenever the runtime is
//!   idle, and a task waiting for a loopback reply is idle for a moment, so the client's own
//!   connect (15 s) and read (60 s) timeouts would fire at random. The automatic schedule is
//!   driven through the `State` fields `first_check`, `interval` and `jitter_secs` set to
//!   milliseconds. Phases that make **no** request may pause the clock after the I/O is done
//!   (`tokio::time::pause()`), which is how the 14-day expiry timer is crossed in one step.
//! * **Injectable wall clock.** [`HttpHost`] is the `Host` seam: `now()` is the real time plus
//!   a settable skew, so an offer can be aged by 14 days (or dated into the future) without
//!   waiting; the offer's freshness is evaluated on every read, so a request-free "poke" (a
//!   failed manual check) makes the snapshot notice.
//! * **No process environment.** Edition 2024 forbids `set_var`. The AppImage environment
//!   (`APPIMAGE`, `APPDIR`, the exe) is injected as an [`AppImageEnv`] into the real
//!   `detect_appimage` rules. `DOCKERING_DISABLE_UPDATES` is read from the process environment
//!   by the product, so its test re-runs a child test in a **child process** started with the
//!   variable (see [`run_child`]).
//! * **Held requests.** [`LoopbackServer::hold`] keeps one request in flight deterministically
//!   (a slow check) instead of racing a delay.
//! * **Linux only where the OS matters.** The AppImage swap and relaunch are Linux code: those
//!   scenarios are `cfg(target_os = "linux")` and use the compiled stand-in AppImage of
//!   `updates_apply_tests`. Everything else (checks, *Download* offers, faults, opt-outs, the
//!   retained offer) runs on every OS through a host that *says* it is an AppImage install.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use dk_core::EngineError;
use dk_update::GithubSource;
use dk_update::InstallKind;
use dk_update::install_kind::{AppImageEnv, FsProbe, detect_appimage};
use dk_update::manifest::platform_key;
use dk_update::offer::{self, OFFER_FILE, OfferStage as Stored, StoredOffer};
use dk_update::source::testing::{Fault, LoopbackServer, Reply};
use serde_json::json;
use tempfile::TempDir;

use crate::config::{Config, UiState};
use crate::handle::{HubHandle, HubOptions};
use crate::hub::{HubInner, lock};
use crate::paths::Paths;
use crate::updates::model::rfc3339;
use crate::updates::{
    ApplyStrategy, Host, State, SystemStrategy, UpdateCheck, UpdateStatus, UpdaterInit,
};
use crate::updates_offer_tests::{available, is_ready, protocol, settled, the_offer};
use crate::updates_tests::{
    DAY, FakeStrategy, SHORT, current, sha2_hex, sign, signed_manifest_for, until, updater_env_off,
};
use crate::{ApplyOutcome, DisabledReason, OfferMode, OfferStage, UpdateSnapshot};

/// The version of the running app in every scenario (and so in the `User-Agent`).
const RUNNING: &str = "0.1.0";
/// The Linux asset every release publishes (REL-012).
const ASSET_NAME: &str = "Dockering-x86_64.AppImage";
const HOUR: Duration = Duration::from_secs(3600);
/// Bytes that stand in for an AppImage where nothing will run it.
const PAYLOAD: &[u8] = b"appimage payload bytes, never executed";

// ── the host seam ───────────────────────────────────────────────────────────────────────────

/// The machine the updater runs on: the platform key, the install kind (fixed, or computed by the
/// **real** `detect_appimage` rules from an injected environment) and a wall clock that is the
/// real time plus a settable skew.
struct HttpHost {
    platform: Mutex<String>,
    kind: Mutex<InstallKind>,
    /// The AppImage a fixed `AppImage` kind names (nothing is applied through it).
    target: Mutex<Option<PathBuf>>,
    /// When set, kind and target come from `detect_appimage` on the real file system.
    detect: Mutex<Option<AppImageEnv>>,
    base: SystemTime,
    start: tokio::time::Instant,
    skew_secs: AtomicI64,
}

impl HttpHost {
    fn new() -> Self {
        Self {
            platform: Mutex::new(platform_key()),
            kind: Mutex::new(InstallKind::Other),
            target: Mutex::new(None),
            detect: Mutex::new(None),
            base: SystemTime::now(),
            start: tokio::time::Instant::now(),
            skew_secs: AtomicI64::new(0),
        }
    }

    /// Moves the wall clock `secs` forward (negative: back), on top of the time that passes.
    fn set_skew(&self, secs: i64) {
        self.skew_secs.store(secs, Ordering::SeqCst);
    }

    fn set_platform(&self, platform: &str) {
        *lock(&self.platform) = platform.to_owned();
    }

    fn set_kind(&self, kind: InstallKind) {
        *lock(&self.kind) = kind;
    }
}

impl Host for HttpHost {
    fn platform(&self) -> String {
        lock(&self.platform).clone()
    }
    fn install_kind(&self) -> InstallKind {
        match &*lock(&self.detect) {
            Some(env) if detect_appimage(env, &FsProbe).is_some() => InstallKind::AppImage,
            Some(_) => InstallKind::Other,
            None => *lock(&self.kind),
        }
    }
    fn appimage_target(&self) -> Option<PathBuf> {
        match &*lock(&self.detect) {
            Some(env) => detect_appimage(env, &FsProbe),
            None => lock(&self.target).clone(),
        }
    }
    fn now(&self) -> SystemTime {
        let now = self.base + self.start.elapsed();
        let skew = self.skew_secs.load(Ordering::SeqCst);
        if skew >= 0 {
            now + Duration::from_secs(skew.unsigned_abs())
        } else {
            now - Duration::from_secs(skew.unsigned_abs())
        }
    }
    fn check_installer(&self, _: &Path, _: &Path) -> Result<(), dk_update::UpdateError> {
        Ok(())
    }
}

// ── the rig ─────────────────────────────────────────────────────────────────────────────────

/// `https://github.com<path>` → `<path>`: where the loopback server serves what GitHub would.
fn github_path(url: &str) -> String {
    url.strip_prefix("https://github.com")
        .unwrap_or_else(|| panic!("not a github.com URL: {url}"))
        .to_owned()
}

fn manifest_path() -> String {
    github_path(dk_update::MANIFEST_URL)
}

fn signature_path() -> String {
    github_path(dk_update::SIGNATURE_URL)
}

/// The pinned asset URL of `version` (UPD-001).
fn asset_url(version: &str) -> String {
    format!("https://github.com/pavel-purma/dockering/releases/download/v{version}/{ASSET_NAME}")
}

/// A manifest of schema 1 with one entry, for hand-made (often malicious) variants.
fn manifest_value(
    version: &str,
    platform: &str,
    kind: &str,
    url: &str,
    file: &[u8],
) -> serde_json::Value {
    json!({
        "schema": 1,
        "version": version,
        "pub_date": "2026-10-03T00:00:00Z",
        "notes_url": format!("https://github.com/pavel-purma/dockering/releases/tag/v{version}"),
        "platforms": { platform: {
            "kind": kind,
            "url": url,
            "sha256": sha2_hex(file),
            "size": file.len(),
        }},
    })
}

/// One published release: the signed manifest, its signature and its asset route.
struct Release {
    json: Vec<u8>,
    sig: String,
    asset_path: String,
}

/// How many requests of each kind the server saw.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    manifest: usize,
    signature: usize,
    asset: usize,
}

const fn counts(manifest: usize, signature: usize, asset: usize) -> Counts {
    Counts {
        manifest,
        signature,
        asset,
    }
}

/// Everything one scenario needs: a temp directory (the hub's config, state and `updates/`, plus
/// the install layout), a throwaway signing key, the loopback server and the host seam.
struct Rig {
    dir: TempDir,
    kp: minisign::KeyPair,
    server: LoopbackServer,
    host: Arc<HttpHost>,
    running: &'static str,
}

/// How a hub is started.
struct Opts {
    config: Config,
    ui_state: UiState,
    demo: bool,
    keys: Option<Vec<String>>,
    first_check: Duration,
    interval: Duration,
    strategy: Option<Arc<dyn ApplyStrategy>>,
}

impl Opts {
    /// Automatic checks on, but the first one an hour away: only manual checks happen.
    fn manual() -> Self {
        Self {
            config: Config::default(),
            ui_state: UiState::default(),
            demo: false,
            keys: None,
            first_check: HOUR,
            interval: HOUR,
            strategy: None,
        }
    }

    /// Automatic checks on a millisecond schedule (no jitter).
    fn scheduled(first: Duration, interval: Duration) -> Self {
        Self {
            first_check: first,
            interval,
            ..Self::manual()
        }
    }

    fn checks_off(mut self) -> Self {
        self.config.updates.check = false;
        self
    }

    fn strategy(mut self, strategy: Arc<dyn ApplyStrategy>) -> Self {
        self.strategy = Some(strategy);
        self
    }
}

impl Rig {
    fn new() -> Self {
        Self::with_running(RUNNING)
    }

    fn with_running(running: &'static str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("out")).expect("mkdir");
        Self {
            dir,
            kp: minisign::KeyPair::generate_unencrypted_keypair().expect("keypair"),
            server: LoopbackServer::start(Vec::new()).expect("loopback server"),
            host: Arc::new(HttpHost::new()),
            running,
        }
    }

    fn paths(&self) -> Paths {
        Paths::in_dir(self.dir.path())
    }

    fn updates_dir(&self) -> PathBuf {
        self.dir.path().join("updates")
    }

    /// `<updates>/<version>/Dockering-x86_64.AppImage`.
    fn payload(&self, version: &str) -> PathBuf {
        self.updates_dir().join(version).join(ASSET_NAME)
    }

    /// An install that says it is an AppImage in a writable place (UPD-013) without being one:
    /// enough for everything but the real swap.
    fn fake_appimage(&self) {
        self.host.set_kind(InstallKind::AppImage);
        *lock(&self.host.detect) = None;
        *lock(&self.host.target) = Some(
            self.dir
                .path()
                .join("home")
                .join(".local")
                .join("bin")
                .join("dockering"),
        );
    }

    /// Serves `version` as `latest`, with an `appimage` entry for this host's platform.
    fn publish(&self, version: &str, file: &[u8]) -> Release {
        self.publish_entry(version, &self.host.platform(), "appimage", file)
    }

    /// Serves `version` as `latest` with exactly one entry, `platform` → `kind`.
    fn publish_entry(&self, version: &str, platform: &str, kind: &str, file: &[u8]) -> Release {
        let (json, sig) = signed_manifest_for(&self.kp, version, file, platform, kind);
        let value: serde_json::Value = serde_json::from_slice(&json).expect("manifest json");
        let url = value["platforms"][platform]["url"].as_str().expect("url");
        let asset_path = github_path(url);
        self.server
            .set_route(&asset_path, Reply::Body(file.to_vec()));
        let release = Release {
            json,
            sig,
            asset_path,
        };
        self.make_latest(&release);
        release
    }

    /// `latest` points at `release` (it keeps being served at its own asset path).
    fn make_latest(&self, release: &Release) {
        self.serve_manifest(release.json.clone(), release.sig.clone());
    }

    fn serve_manifest(&self, json: Vec<u8>, sig: String) {
        self.server.set_route(&manifest_path(), Reply::Body(json));
        self.server
            .set_route(&signature_path(), Reply::Body(sig.into_bytes()));
    }

    /// Signs `value` with the rig's key and serves it as `latest`.
    fn serve_value(&self, value: &serde_json::Value) -> Release {
        let json = serde_json::to_vec(value).expect("json");
        let sig = sign(&self.kp, &json);
        let release = Release {
            json,
            sig,
            asset_path: String::new(),
        };
        self.make_latest(&release);
        release
    }

    /// What the server saw, by kind. Every request is checked on the way: a `GET` with exactly
    /// the `User-Agent` the spec promises (UPD-010), no cookie, credential or referer, and one of
    /// the three kinds (any other path fails the test).
    fn counts(&self) -> Counts {
        let agent = format!(
            "Dockering/{} ({}; {})",
            self.running,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let mut seen = Counts::default();
        for request in self.server.requests() {
            assert_eq!(request.method, "GET", "{}", request.path);
            assert_eq!(
                request.header("user-agent"),
                Some(agent.as_str()),
                "{}",
                request.path
            );
            for name in ["cookie", "authorization", "referer", "x-request-id"] {
                assert_eq!(request.header(name), None, "{name} on {}", request.path);
            }
            if request.path == manifest_path() {
                seen.manifest += 1;
            } else if request.path == signature_path() {
                seen.signature += 1;
            } else if request
                .path
                .starts_with("/pavel-purma/dockering/releases/download/")
            {
                seen.asset += 1;
            } else {
                panic!("unexpected request {}", request.path);
            }
        }
        seen
    }

    fn start(&self) -> HubHandle {
        self.start_with(Opts::manual())
    }

    fn start_with(&self, o: Opts) -> HubHandle {
        let source = GithubSource::with_base(self.running, &self.server.base_url())
            .expect("a loopback source");
        let keys = o.keys.unwrap_or_else(|| vec![self.kp.pk.to_base64()]);
        let mut state = State::new(Arc::new(source), self.host.clone(), self.running, keys);
        // The test binary is unsigned: the Authenticode pin is out of the picture.
        state.running_exe = None;
        state.strategy = o
            .strategy
            .unwrap_or_else(|| Arc::new(SystemStrategy::production()));
        state.first_check = o.first_check;
        state.interval = o.interval;
        state.jitter_secs = 0;
        let opts = HubOptions {
            paths: self.paths(),
            config: o.config,
            ui_state: o.ui_state,
            factories: Some(vec![]),
            discover_on_start: false,
            worker_threads: 2,
            demo: o.demo,
        };
        HubInner::start_with(
            opts,
            tokio::runtime::Handle::current(),
            None,
            UpdaterInit::State(Box::new(state)),
        )
    }

    /// What `offer.json` says now.
    fn stored(&self) -> Option<StoredOffer> {
        offer::read(&self.updates_dir()).expect("offer.json is readable")
    }

    fn offer_json(&self) -> Vec<u8> {
        std::fs::read(self.updates_dir().join(OFFER_FILE)).expect("offer.json")
    }

    /// `offer.json` without `confirmed_at`, which every successful check refreshes (and whose
    /// RFC 3339 text changes length with the sub-second digits).
    fn offer_json_sans_time(&self) -> serde_json::Value {
        let mut value: serde_json::Value =
            serde_json::from_slice(&self.offer_json()).expect("offer.json is JSON");
        value
            .as_object_mut()
            .expect("an object")
            .remove("confirmed_at")
            .expect("confirmed_at is stored");
        value
    }

    /// Everything under the updates directory (files, relative, sorted), and its top-level names.
    fn update_files(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.updates_dir(), &self.updates_dir(), &mut out);
        out.sort();
        out
    }

    fn update_entries(&self) -> Vec<String> {
        names(&self.updates_dir())
    }
}

/// The names in `dir`, sorted (empty when it doesn't exist).
fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Polls on the real clock until `f` holds.
async fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..2000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn spawn_check(hub: &HubHandle) -> tokio::task::JoinHandle<Result<UpdateCheck, EngineError>> {
    let hub = hub.clone();
    tokio::spawn(async move { hub.check_for_updates().await })
}

/// A manual check that must fail; returns its message.
async fn failed_check(hub: &HubHandle) -> String {
    match hub.check_for_updates().await {
        Err(EngineError::Protocol(message)) => message,
        other => panic!("expected a failed check, got {other:?}"),
    }
}

/// The status of the snapshot after a failed manual check: the error, shown inline (UPD-004).
fn is_error(snap: &UpdateSnapshot, message: &str) -> bool {
    snap.status
        == UpdateStatus::Error {
            message: message.to_owned(),
        }
}

/// After a download that failed before its payload verified (UPD-017, UPD-012): the manifest was
/// authenticated, so the offer may exist, but only as the hidden *Downloading* one that waits for
/// the next check ("payload missing"); it is never actionable, and nothing partial is left behind:
/// the only file is `offer.json`, and only when there is an offer.
fn assert_download_failed_cleanly(rig: &Rig, snap: &UpdateSnapshot, what: &str) {
    match &snap.offer {
        None => assert_eq!(rig.update_entries(), Vec::<String>::new(), "{what}"),
        Some(offer) => {
            assert_eq!(offer.stage, OfferStage::Downloading, "{what}: {offer:?}");
            assert_eq!(offer.mode, OfferMode::Install, "{what}: {offer:?}");
            // The only file is `offer.json`: no payload and no `*.part`. An empty version
            // directory may remain (checked by files, not by directory entries).
            assert_eq!(
                rig.update_files(),
                vec![OFFER_FILE.to_owned()],
                "{what}: no payload and no `*.part`"
            );
        }
    }
}

fn is_idle(snap: &UpdateSnapshot) -> bool {
    matches!(
        snap.status,
        UpdateStatus::Idle {
            last_check: Some(_)
        }
    )
}

// ── log capture ─────────────────────────────────────────────────────────────────────────────

/// A `tracing` writer that keeps what the updater logs on this thread (the runtime of a
/// `#[tokio::test]` is current-thread, so the hub's tasks run here).
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    /// The updater's lines at `info` and above.
    fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&lock(&self.0))
            .lines()
            .filter(|l| l.contains("dk_hub::updates"))
            .map(str::to_owned)
            .collect()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;
    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

// ── the Linux install layout (real swap) ────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
use crate::updates_apply_tests::stub;

/// An AppImage install laid out like a user's: `~/.local/bin/dockering` is the AppImage file (a
/// compiled stand-in of version 0.1.0), the AppDir lies beside it, and the environment the
/// runtime would set (`APPIMAGE`, `APPDIR`) is injected into the real detection rules.
#[cfg(target_os = "linux")]
struct Install {
    /// The real AppImage file.
    bin: PathBuf,
    /// The symlink `APPIMAGE` names instead, when the install is symlinked.
    link: Option<PathBuf>,
    /// The bytes of the installed file before any update.
    old: Vec<u8>,
}

#[cfg(target_os = "linux")]
fn write_executable(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    // Through a temp name and a `rename`, like the real swap: no `ETXTBSY` on a file that ran.
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, bytes).expect("write");
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::rename(&tmp, path).expect("rename");
}

#[cfg(target_os = "linux")]
impl Rig {
    /// Where a relaunched stand-in records itself.
    fn marker(&self) -> PathBuf {
        self.dir.path().join("out").join("relaunch.marker")
    }

    /// A stand-in AppImage of `version` whose relaunch writes [`Rig::marker`].
    fn stub(&self, version: &str, relaunch_dies: bool) -> Vec<u8> {
        stub::appimage(version, Some(&self.marker()), relaunch_dies)
    }

    fn bin_dir(&self) -> PathBuf {
        self.dir.path().join("home").join(".local").join("bin")
    }

    fn install_appimage(&self, symlinked: bool) -> Install {
        let home = self.dir.path().join("home");
        std::fs::create_dir_all(self.bin_dir()).expect("mkdir");
        let bin = self.bin_dir().join("dockering");
        let old = stub::appimage("0.1.0", None, false);
        write_executable(&bin, &old);
        // Where the runtime mounts the image; the running executable lies under it.
        let appdir = self.dir.path().join("mnt").join("appdir");
        let exe = appdir.join("usr").join("bin").join("dockering");
        std::fs::create_dir_all(exe.parent().expect("dir")).expect("mkdir");
        std::fs::write(&exe, b"the running binary").expect("exe");
        let link = symlinked.then(|| {
            let dir = home.join("bin");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let link = dir.join("dockering");
            std::os::unix::fs::symlink(&bin, &link).expect("symlink");
            link
        });
        *lock(&self.host.detect) = Some(AppImageEnv {
            appimage: Some(link.clone().unwrap_or_else(|| bin.clone()).into_os_string()),
            appdir: Some(appdir.into_os_string()),
            exe,
        });
        Install { bin, link, old }
    }

    /// Waits (real time) for the relaunched stand-in to record itself; returns what it wrote.
    async fn relaunch_marker(&self) -> String {
        let marker = self.marker();
        eventually("the relaunched process to write its marker", || {
            marker.exists()
        })
        .await;
        // The file is written in one call; give a slow disk a moment to show the content.
        eventually("the marker to have content", || {
            std::fs::read_to_string(&marker).is_ok_and(|s| !s.is_empty())
        })
        .await;
        std::fs::read_to_string(&marker).expect("marker")
    }
}

// ═══ UPD-001…004: protocol level, over HTTP ═════════════════════════════════════════════════

#[tokio::test]
async fn upd_001_wrong_host_and_url_rejected_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let hub = rig.start();
    let pinned = "/pavel-purma/dockering/releases/download";
    // A signed manifest whose asset URL isn't *the* release URL of *that* version: every one of
    // these must be rejected before a single asset request (a lax client would fetch the same
    // path from the loopback server, which is what the request log would show).
    let bad = [
        // another host, even one that looks like GitHub
        format!("https://evil.example{pinned}/v0.2.0/{ASSET_NAME}"),
        format!("https://github.com.evil.example{pinned}/v0.2.0/{ASSET_NAME}"),
        format!("https://raw.githubusercontent.com{pinned}/v0.2.0/{ASSET_NAME}"),
        // an allowed CDN host, but not the pinned URL
        format!("https://objects.githubusercontent.com{pinned}/v0.2.0/{ASSET_NAME}"),
        // plain http to the right host
        format!("http://github.com{pinned}/v0.2.0/{ASSET_NAME}"),
        // another repository
        format!("https://github.com/attacker/dockering/releases/download/v0.2.0/{ASSET_NAME}"),
        format!("https://github.com/pavel-purma/other/releases/download/v0.2.0/{ASSET_NAME}"),
        // a different version path, and `latest` instead of a version
        format!("https://github.com{pinned}/v0.9.0/{ASSET_NAME}"),
        format!("https://github.com/pavel-purma/dockering/releases/latest/download/{ASSET_NAME}"),
        // not a plain file name
        format!("https://github.com{pinned}/v0.2.0/../{ASSET_NAME}"),
        format!("https://github.com{pinned}/v0.2.0/sub/{ASSET_NAME}"),
        format!("https://github.com{pinned}/v0.2.0/"),
    ];
    for url in &bad {
        let release = rig.serve_value(&manifest_value(
            "0.2.0",
            &platform_key(),
            "appimage",
            url,
            PAYLOAD,
        ));
        assert!(release.asset_path.is_empty());
        // The pinned asset exists on the server: nothing but the pin keeps it from being fetched.
        rig.server.set_route(
            &github_path(&asset_url("0.2.0")),
            Reply::Body(PAYLOAD.to_vec()),
        );
        let message = failed_check(&hub).await;
        assert!(message.contains("unexpected asset URL"), "{url}: {message}");
        let snap = current(&hub).await;
        assert_eq!(snap.offer, None, "{url}");
        assert!(is_error(&snap, &message), "{url}");
        assert_eq!(rig.update_entries(), Vec::<String>::new(), "{url}");
    }
    let n = bad.len();
    assert_eq!(
        rig.counts(),
        counts(n, n, 0),
        "manifest and signature only: no asset was ever requested"
    );

    // The same manifest with the pinned URL is accepted and downloaded: the pins are what stopped it.
    rig.serve_value(&manifest_value(
        "0.2.0",
        &platform_key(),
        "appimage",
        &asset_url("0.2.0"),
        PAYLOAD,
    ));
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(rig.counts(), counts(n + 1, n + 1, 1));
}

#[tokio::test]
async fn upd_001_download_only_install_never_requests_the_asset_over_http() {
    if updater_env_off() {
        return;
    }
    // A macOS-style install gets a *Download* offer and fetches the manifest only, whatever the
    // URL in its entry says: not even a hostile URL turns into a request.
    let rig = Rig::new();
    rig.host.set_platform("macos-aarch64");
    rig.host.set_kind(InstallKind::Other);
    let hub = rig.start();
    for url in [
        "https://evil.example/pavel-purma/dockering/releases/download/v0.2.0/Dockering-aarch64.dmg",
        "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-aarch64.dmg",
    ] {
        rig.serve_value(&manifest_value(
            "0.2.0",
            "macos-aarch64",
            "dmg",
            url,
            PAYLOAD,
        ));
        let _ = hub.check_for_updates().await;
    }
    let seen = rig.counts();
    assert_eq!(seen.asset, 0, "a Download offer starts no asset download");
    assert_eq!((seen.manifest, seen.signature), (2, 2));
    // Whatever the offer is made of, no payload is on disk.
    assert!(
        !rig.updates_dir().join("0.2.0").exists(),
        "no payload directory"
    );
}

#[tokio::test]
async fn upd_001_redirect_to_another_host_fails_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let release = rig.publish("0.2.0", PAYLOAD);
    // The asset URL passes the pins, but the CDN answers with a redirect to somewhere else:
    // redirects anywhere but the allowed hosts fail (UPD-010), and nothing is left behind.
    rig.server.set_route(
        &release.asset_path,
        Reply::Redirect("https://evil.example/payload".into()),
    );
    let hub = rig.start();
    let message = failed_check(&hub).await;
    assert!(message.starts_with("network error"), "{message}");
    assert_eq!(rig.counts(), counts(1, 1, 1));
    assert_download_failed_cleanly(&rig, &current(&hub).await, "redirect to another host");
}

#[tokio::test]
async fn upd_002_unknown_schema_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let hub = rig.start();
    // A future schema, even one that offers a newer version for this platform: no update, no
    // error, nothing downloaded (UPD-002).
    for schema in [0u32, 2, 7] {
        let mut value = manifest_value(
            "9.0.0",
            &platform_key(),
            "appimage",
            &asset_url("9.0.0"),
            PAYLOAD,
        );
        value["schema"] = json!(schema);
        rig.serve_value(&value);
        assert_eq!(
            hub.check_for_updates().await,
            Ok(UpdateCheck::UpToDate),
            "schema {schema}"
        );
        let snap = current(&hub).await;
        assert_eq!(snap.offer, None, "schema {schema}");
        assert!(is_idle(&snap), "no error for schema {schema}: {snap:?}");
    }
    assert_eq!(rig.counts(), counts(3, 3, 0));
    assert_eq!(rig.update_entries(), Vec::<String>::new());
    assert_eq!(
        hub.config().ui_state().updates.last_result.as_deref(),
        Some("up to date")
    );
}

/// A manifest of an unknown schema is allowed to have *another shape* (that is what a schema
/// number is for): an old client must still read it as "no update", not as an error.
#[tokio::test]
async fn upd_002_unknown_schema_with_a_new_shape_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let hub = rig.start();
    for value in [
        json!({ "schema": 2, "release": { "version": "9.0.0" }, "assets": [ { "os": "linux" } ] }),
        json!({ "schema": 2, "version": 9, "platforms": ["linux-x86_64"] }),
        json!({ "schema": 2 }),
    ] {
        rig.serve_value(&value);
        assert_eq!(
            hub.check_for_updates().await,
            Ok(UpdateCheck::UpToDate),
            "{value}"
        );
        let snap = current(&hub).await;
        assert_eq!(snap.offer, None, "{value}");
        assert!(is_idle(&snap), "{value}: {snap:?}");
    }
    assert_eq!(rig.counts().asset, 0);
}

#[tokio::test]
async fn upd_002_unknown_fields_are_ignored_and_malformed_is_an_error_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let hub = rig.start();
    // Unknown fields everywhere, an unknown kind and a platform we don't know: still offered.
    let mut value = manifest_value(
        "0.2.0",
        &platform_key(),
        "appimage",
        &asset_url("0.2.0"),
        PAYLOAD,
    );
    value["future_top_level"] = json!({ "anything": [1, 2, 3] });
    value["platforms"][platform_key()]["future_asset_field"] = json!(42);
    value["platforms"]["haiku-riscv64"] = json!({
        "kind": "a-format-from-the-future", "url": "https://example.invalid/x", "sha256": "00", "size": 1
    });
    rig.serve_value(&value);
    rig.server.set_route(
        &github_path(&asset_url("0.2.0")),
        Reply::Body(PAYLOAD.to_vec()),
    );
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);

    // Malformed manifests (signed, so authentic) are a failed check that changes nothing.
    let json_before = rig.offer_json();
    for bytes in [
        b"not json at all".to_vec(),
        b"{\"schema\":1}".to_vec(),
        b"[]".to_vec(),
        Vec::new(),
        vec![0xff, 0xfe, 0x00],
    ] {
        let sig = sign(&rig.kp, &bytes);
        rig.serve_manifest(bytes.clone(), sig);
        let message = failed_check(&hub).await;
        assert!(
            message.starts_with("invalid update manifest") || message.contains("not valid UTF-8"),
            "{bytes:?}: {message}"
        );
        let snap = current(&hub).await;
        assert_eq!(the_offer(&snap).version, "0.2.0", "{bytes:?}");
        assert!(is_error(&snap, &message));
        assert_eq!(rig.offer_json(), json_before, "{bytes:?}");
    }
}

#[tokio::test]
async fn upd_003_bad_signature_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let good = rig.publish("0.2.0", PAYLOAD);
    let hub = rig.start();
    let other = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let (by_other, sig_other) =
        signed_manifest_for(&other, "0.2.0", PAYLOAD, &platform_key(), "appimage");
    // The manifest is bumped to a version it was never signed for.
    let tampered = String::from_utf8(good.json.clone())
        .expect("utf-8")
        .replace("0.2.0", "9.9.9")
        .into_bytes();
    let cases: [(&str, Vec<u8>, String); 4] = [
        ("another key", by_other, sig_other),
        (
            "garbage signature",
            good.json.clone(),
            "not a signature".into(),
        ),
        ("empty signature", good.json.clone(), String::new()),
        ("tampered manifest", tampered, good.sig.clone()),
    ];
    for (what, json, sig) in cases {
        rig.serve_manifest(json, sig);
        let message = failed_check(&hub).await;
        assert_eq!(message, "the update signature is invalid", "{what}");
        let snap = current(&hub).await;
        assert_eq!(snap.offer, None, "{what}");
        assert!(is_error(&snap, &message), "{what}");
        assert_eq!(rig.update_entries(), Vec::<String>::new(), "{what}");
    }
    // The signature file is missing altogether: also a failed check.
    rig.make_latest(&good);
    rig.server.remove_route(&signature_path());
    assert_eq!(failed_check(&hub).await, "server returned HTTP 404");
    assert_eq!(
        rig.counts(),
        counts(5, 5, 0),
        "five failed checks, and not one asset request"
    );

    // The genuine pair works: it was the signature, nothing else.
    rig.make_latest(&good);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(rig.counts(), counts(6, 6, 1));
}

#[tokio::test]
async fn upd_004_no_downgrade_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    // An install that *would* download anything it is offered.
    rig.fake_appimage();
    let hub = rig.start();
    // The running version, an older one and a pre-release for a stable build: nothing offered.
    for version in ["0.1.0", "0.0.9", "0.0.1", "0.2.0-rc.1", "1.0.0-beta.2"] {
        rig.publish(version, PAYLOAD);
        assert_eq!(
            hub.check_for_updates().await,
            Ok(UpdateCheck::UpToDate),
            "{version}"
        );
        let snap = current(&hub).await;
        assert_eq!(snap.offer, None, "{version}");
        assert!(is_idle(&snap), "{version}: {snap:?}");
    }
    assert_eq!(rig.counts(), counts(5, 5, 0), "no asset for any of them");
    assert_eq!(rig.update_entries(), Vec::<String>::new());

    // A strictly newer stable release is offered (the rule is not "never offer anything").
    rig.publish("0.1.1", PAYLOAD);
    assert_eq!(hub.check_for_updates().await, available("0.1.1"));
    assert_eq!(rig.counts().asset, 1);
}

#[tokio::test]
async fn upd_004_a_prerelease_build_is_offered_a_prerelease_over_http() {
    if updater_env_off() {
        return;
    }
    // "A pre-release is never offered to a *stable* build": a beta may get the next one.
    let rig = Rig::with_running("0.2.0-beta.1");
    rig.fake_appimage();
    rig.publish("0.2.0-rc.1", PAYLOAD);
    let hub = rig.start();
    assert_eq!(hub.check_for_updates().await, available("0.2.0-rc.1"));
    until(&hub, SHORT, is_ready("0.2.0-rc.1")).await;
    // The User-Agent carries the running version, whatever it is.
    assert_eq!(rig.counts(), counts(1, 1, 1));
}

/// UPD-004 (schedule), UPD-005: the automatic schedule over HTTP on a millisecond clock. The
/// first check comes after `first_check`, then one per `interval`; setting the automatic-check
/// switch off stops it at once and on again restarts it; *Check now* works either way.
#[tokio::test]
async fn upd_004_opt_outs_make_no_requests() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    rig.publish("0.2.0", PAYLOAD);
    let period = Duration::from_millis(150);
    let hub = rig.start_with(Opts::scheduled(Duration::from_millis(30), period).checks_off());

    // Off by the setting: two periods and more, not one request.
    tokio::time::sleep(period * 3).await;
    assert_eq!(rig.counts(), Counts::default());
    let snap = current(&hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting
        }
    );

    // Switched on at runtime: the schedule starts and finds the update by itself.
    hub.config().update(|c| c.updates.check = true);
    until(&hub, SHORT, is_ready("0.2.0")).await;
    eventually("a second automatic check", || rig.counts().manifest >= 2).await;

    // Switched off again: once the check in flight is done, the schedule stays silent.
    hub.config().update(|c| c.updates.check = false);
    until(&hub, SHORT, |s| {
        matches!(s.status, UpdateStatus::Disabled { .. })
    })
    .await;
    settled(&hub).await;
    let before = rig.counts();
    assert_eq!(before.asset, 1, "the payload was fetched once");
    tokio::time::sleep(period * 3).await;
    assert_eq!(rig.counts(), before, "no automatic request while off");
    assert!(the_offer(&current(&hub).await).stage == OfferStage::Ready);

    // The explicit action still works (only the setting is off).
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(rig.counts().manifest, before.manifest + 1);
    assert_eq!(rig.counts().asset, 1);
}

// ═══ S1, S2: the happy path and the symlinked install (Linux: the real swap) ════════════════

#[cfg(target_os = "linux")]
#[tokio::test]
async fn upd_e2e_s1_happy_path_check_download_apply() {
    use std::os::unix::fs::PermissionsExt;
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    let install = rig.install_appimage(false);
    let payload = rig.stub("0.2.0", false);
    let release = rig.publish("0.2.0", &payload);
    let hub = rig.start();

    // Before the check: no offer, no button, no request.
    let before = current(&hub).await;
    assert_eq!(before.offer, None);
    assert_eq!(before.status, UpdateStatus::Idle { last_check: None });
    assert_eq!(rig.counts(), Counts::default());

    // The manual check finds, downloads and verifies it: the offer is *Install / Ready*.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let snap = until(&hub, SHORT, is_ready("0.2.0")).await;
    assert!(is_idle(&snap), "{snap:?}");
    let offer = the_offer(&snap);
    assert_eq!(
        (
            offer.version.as_str(),
            offer.mode,
            offer.stage,
            offer.needs_elevation,
            offer.install_failed
        ),
        ("0.2.0", OfferMode::Install, OfferStage::Ready, false, false)
    );
    assert_eq!(
        offer.notes_url,
        "https://github.com/pavel-purma/dockering/releases/tag/v0.2.0"
    );
    // Exactly one manifest, one `.minisig` and one asset request, with the exact User-Agent.
    assert_eq!(rig.counts(), counts(1, 1, 1));
    let ui = hub.config().ui_state().updates;
    assert_eq!(ui.last_result.as_deref(), Some("0.2.0 available"));
    assert!(ui.last_check.is_some());

    // The offer persisted: the signed manifest byte for byte, its signature, the stage; the
    // payload is the only other file (no `.part`, no temp file).
    let stored = rig.stored().expect("offer.json");
    assert_eq!(stored.stage, Stored::Ready);
    assert!(!stored.install_failed);
    assert_eq!(stored.manifest.as_bytes(), release.json.as_slice());
    assert_eq!(stored.signature, release.sig);
    assert_eq!(
        rig.update_files(),
        [format!("0.2.0/{ASSET_NAME}"), "offer.json".to_owned()]
    );
    assert_eq!(
        std::fs::read(rig.payload("0.2.0")).expect("payload"),
        payload
    );
    assert_eq!(
        std::fs::read(&install.bin).expect("target"),
        install.old,
        "nothing is installed by a check"
    );
    let json = rig.offer_json();

    // *Restart to update*: the REAL file is replaced, the mode is kept, the relaunch started.
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(std::fs::read(&install.bin).expect("target"), payload);
    assert_eq!(
        std::fs::metadata(&install.bin)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(names(&rig.bin_dir()), ["dockering"], "no staging file left");
    let marker = rig.relaunch_marker().await;
    assert!(marker.starts_with("version=0.2.0\n"), "{marker}");
    assert!(
        !marker
            .lines()
            .any(|l| l.starts_with("APPIMAGE=") || l.starts_with("APPDIR=")),
        "the relaunch environment is scrubbed"
    );
    // A handoff leaves the offer as it was (memory and disk), and makes no request.
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Install, OfferStage::Ready, false)
    );
    assert_eq!(rig.offer_json(), json);
    assert_eq!(rig.counts(), counts(1, 1, 1));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn upd_e2e_s2_symlinked_install_resolves_to_real_file() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    // `APPIMAGE` names a symlink (`~/bin/dockering -> ~/.local/bin/dockering`).
    let install = rig.install_appimage(true);
    let link = install.link.clone().expect("a symlinked install");
    let payload = rig.stub("0.2.0", false);
    rig.publish("0.2.0", &payload);
    let hub = rig.start();

    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(
        offer.mode,
        OfferMode::Install,
        "a symlinked AppImage is still an AppImage install"
    );

    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    // The REAL file was replaced; the symlink is still a symlink to it and shows the new bytes.
    assert_eq!(std::fs::read(&install.bin).expect("real file"), payload);
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat the link")
            .file_type()
            .is_symlink(),
        "the symlink wasn't replaced by a regular file"
    );
    assert_eq!(std::fs::read_link(&link).expect("read_link"), install.bin);
    assert_eq!(std::fs::read(&link).expect("through the link"), payload);
    assert_eq!(names(link.parent().expect("dir")), ["dockering"]);
    assert_eq!(names(&rig.bin_dir()), ["dockering"], "no staging file left");
    rig.relaunch_marker().await;
    assert_eq!(rig.counts(), counts(1, 1, 1));
}

// ═══ S3, S4: a bad signature, a bad hash ════════════════════════════════════════════════════

#[tokio::test]
async fn upd_e2e_s3_bad_signature_over_http() {
    if updater_env_off() {
        return;
    }
    let (logs, _guard) = capture_logs();
    let rig = Rig::new();
    rig.fake_appimage();
    // A well-formed release signed by someone else, with its asset on offer.
    let other = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let (json, sig) = signed_manifest_for(&other, "0.2.0", PAYLOAD, &platform_key(), "appimage");
    rig.serve_manifest(json, sig);
    rig.server.set_route(
        &github_path(&asset_url("0.2.0")),
        Reply::Body(PAYLOAD.to_vec()),
    );
    // An automatic check on a millisecond schedule (the next one is an hour away).
    let hub = rig.start_with(Opts::scheduled(Duration::from_millis(30), HOUR));
    eventually("the automatic check to be logged", || {
        !logs.lines().is_empty()
    })
    .await;

    // Nothing was downloaded, there is no offer, no error is shown, and one line is logged.
    assert_eq!(rig.counts(), counts(1, 1, 0));
    let snap = current(&hub).await;
    assert_eq!(snap.offer, None);
    assert!(is_idle(&snap), "an automatic check stays quiet: {snap:?}");
    assert_eq!(rig.update_entries(), Vec::<String>::new());
    let lines = logs.lines();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("automatic update check failed")
            && lines[0].contains("the update signature is invalid"),
        "{}",
        lines[0]
    );
    assert_eq!(
        hub.config().ui_state().updates.last_result.as_deref(),
        Some("the update signature is invalid")
    );
}

#[tokio::test]
async fn upd_e2e_s4_bad_hash_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let release = rig.publish("0.2.0", PAYLOAD);
    let hub = rig.start();
    let integrity = "the downloaded file failed its integrity check";
    let flipped = {
        let mut bytes = PAYLOAD.to_vec();
        bytes[0] ^= 0xff;
        bytes
    };
    let mut longer = PAYLOAD.to_vec();
    longer.extend_from_slice(b" and some more");
    let half = PAYLOAD[..PAYLOAD.len() / 2].to_vec();
    let cases: [(&str, Reply, Option<&str>); 4] = [
        // the same size, other bytes: hashed while written, the partial file is removed
        ("different bytes", Reply::Body(flipped), Some(integrity)),
        // the wrong length is refused before a byte is written
        ("shorter", Reply::Body(half.clone()), Some(integrity)),
        ("longer", Reply::Body(longer), Some(integrity)),
        // cut off mid-stream
        (
            "truncated stream",
            Reply::Truncated {
                body: half,
                claimed_len: PAYLOAD.len(),
            },
            None,
        ),
    ];
    for (i, (what, reply, expected)) in cases.into_iter().enumerate() {
        rig.server.set_route(&release.asset_path, reply);
        let message = failed_check(&hub).await;
        match expected {
            Some(expected) => assert_eq!(message, expected, "{what}"),
            None => assert!(message.starts_with("network error"), "{what}: {message}"),
        }
        // Never installable, no payload, no `*.part`.
        let snap = current(&hub).await;
        assert_download_failed_cleanly(&rig, &snap, what);
        assert!(is_error(&snap, &message), "{what}");
        assert_eq!(rig.counts(), counts(i + 1, i + 1, i + 1), "{what}");
    }

    // The right bytes are accepted: the hash, nothing else, was the problem.
    rig.server
        .set_route(&release.asset_path, Reply::Body(PAYLOAD.to_vec()));
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert_eq!(rig.counts(), counts(5, 5, 5));
    assert_eq!(
        rig.update_files(),
        [format!("0.2.0/{ASSET_NAME}"), "offer.json".to_owned()]
    );
}

// ═══ S5: a payload that fails its self-test ═════════════════════════════════════════════════

#[cfg(target_os = "linux")]
#[tokio::test]
async fn upd_e2e_s5_self_test_failure_degrades_offer() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    let install = rig.install_appimage(false);
    // The manifest says 0.2.0, but the payload answers `--version` as 9.9.9.
    let wrong = rig.stub("9.9.9", false);
    rig.publish("0.2.0", &wrong);
    let hub = rig.start();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;

    let Err(EngineError::Protocol(message)) = hub.apply_update().await else {
        panic!("a failed self-test must fail the apply");
    };
    assert!(message.contains("self-test"), "{message}");
    // The target is untouched, nothing was staged or started.
    assert_eq!(std::fs::read(&install.bin).expect("target"), install.old);
    assert_eq!(names(&rig.bin_dir()), ["dockering"]);
    assert!(!rig.marker().exists());
    // The offer is a *Download* one for that version, on disk too.
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Download, OfferStage::Ready, true)
    );
    assert!(rig.stored().expect("offer.json").install_failed);
    assert_eq!(rig.counts(), counts(1, 1, 1));

    // The next check does not retry it: same entry, no second download, still failed.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    assert_eq!(rig.counts(), counts(2, 2, 1), "no asset request");
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install"))
    );
    assert_eq!(std::fs::read(&install.bin).expect("target"), install.old);

    // A newer version with a good payload replaces it, installable again.
    let good = rig.stub("0.3.0", false);
    rig.publish("0.3.0", &good);
    assert_eq!(hub.check_for_updates().await, available("0.3.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.3.0")).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Install, false)
    );
    assert_eq!(rig.counts(), counts(3, 3, 2));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(std::fs::read(&install.bin).expect("target"), good);
    rig.relaunch_marker().await;
}

// ═══ S6: a read-only install directory ══════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
#[tokio::test]
async fn upd_e2e_s6_read_only_install_dir_is_download() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if updater_env_off() {
        return;
    }
    // root ignores permission bits: nothing to prove there.
    if std::fs::metadata("/proc/self").expect("stat").uid() == 0 {
        return;
    }
    let rig = Rig::new();
    let install = rig.install_appimage(false);
    let payload = rig.stub("0.2.0", false);
    rig.publish("0.2.0", &payload);
    let dir = rig.bin_dir();
    // Put the permissions back whatever happens, so the temp dir can be removed.
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let _restore = Restore(dir.clone());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");

    let hub = rig.start();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    // *Download* offer, found = actionable; the manifest only: ZERO asset requests.
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Download, OfferStage::Ready, false)
    );
    assert_eq!(rig.counts(), counts(1, 1, 0));
    assert_eq!(rig.update_files(), ["offer.json"]);
    assert_eq!(rig.stored().expect("offer.json").stage, Stored::Ready);
    // A *Download* offer never installs: refused, nothing touched, nothing staged.
    assert!(hub.apply_update().await.is_err());
    assert_eq!(std::fs::read(&install.bin).expect("target"), install.old);
    assert_eq!(names(&dir), ["dockering"]);

    // Writable again: the next check probes it again (UPD-006), fetches the payload and the
    // offer becomes an install one.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert_eq!(rig.counts(), counts(2, 2, 1));
}

// ═══ S7: opt-outs make no requests ══════════════════════════════════════════════════════════

/// The automatic schedule of the opt-out scenarios: a check 20 ms after the start, then every
/// 40 ms, so a few hundred milliseconds are several periods.
fn fast() -> Opts {
    Opts::scheduled(Duration::from_millis(20), Duration::from_millis(40))
}

#[tokio::test]
async fn upd_e2e_s7_opt_outs_make_no_requests() {
    if updater_env_off() {
        return;
    }
    // One server per case, so each request log is its own. The first case is the control:
    // everything on, so the schedule demonstrably runs in the time the others stay silent.
    struct Case {
        what: &'static str,
        rig: Rig,
        hub: HubHandle,
    }
    let make = |what, opts: Opts| {
        let rig = Rig::new();
        rig.fake_appimage();
        rig.publish("0.2.0", PAYLOAD);
        let hub = rig.start_with(opts);
        Case { what, rig, hub }
    };
    let mut keys = fast();
    keys.keys = Some(Vec::new());
    let mut demo = fast();
    demo.demo = true;
    let control = make("control", fast());
    let cases = [
        make("the setting is off", fast().checks_off()),
        make("demo mode", demo),
        make("no trust keys", keys),
    ];

    // The control checks at least three times; by then the others had the same three periods.
    eventually("three automatic checks of the control", || {
        control.rig.counts().manifest >= 3
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ran = control.rig.counts();
    // The control keeps checking while it is sampled: a check may be between its manifest and its
    // signature request.
    assert!(
        ran.manifest == ran.signature || ran.manifest == ran.signature + 1,
        "{}: {ran:?}",
        control.what
    );
    assert_eq!(ran.asset, 1, "the control downloaded the payload once");
    for case in &cases {
        assert_eq!(
            case.rig.counts(),
            Counts::default(),
            "{}: not one request",
            case.what
        );
    }

    // What the others look like, and that explicit actions don't sneak a request in either.
    let demo = &cases[1];
    assert_eq!(current(&demo.hub).await, UpdateSnapshot::unavailable());
    assert_eq!(
        demo.hub.check_for_updates().await,
        Ok(UpdateCheck::Disabled)
    );
    let no_keys = &cases[2];
    let snap = current(&no_keys.hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Unavailable
        }
    );
    assert_eq!(snap.offer, None);
    assert_eq!(
        no_keys.hub.check_for_updates().await,
        Ok(UpdateCheck::Disabled)
    );
    for case in &cases[1..] {
        assert_eq!(
            case.hub.apply_update().await,
            Err(protocol("updates are disabled")),
            "{}",
            case.what
        );
    }
    let setting = &cases[0];
    let snap = current(&setting.hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting
        }
    );
    for case in &cases {
        assert_eq!(
            case.rig.counts(),
            Counts::default(),
            "{}: still not one request",
            case.what
        );
        assert_eq!(
            case.rig.update_entries(),
            Vec::<String>::new(),
            "{}",
            case.what
        );
    }
}

/// Set (by [`run_child`]) in the child process of the environment opt-out test: `off` when
/// `DOCKERING_DISABLE_UPDATES` disables updates, `on` when its value doesn't.
const CHILD_VAR: &str = "DOCKERING_TEST_UPDATES_CHILD";

/// Re-runs the test `test` of this module in a child process started with
/// `DOCKERING_DISABLE_UPDATES=disable` (the product reads that variable from the process
/// environment; edition 2024 forbids `set_var`, and tests inject instead of mutating). Returns
/// the child's output after checking that it passed and really ran.
fn run_child(test: &str, disable: &str, expect: &str) -> std::process::Child {
    let exe = std::env::current_exe().expect("the test binary");
    let name = format!("{}::{test}", module_path!().trim_start_matches("dk_hub::"));
    std::process::Command::new(exe)
        .args(["--exact", &name, "--nocapture", "--test-threads=1"])
        .env(crate::updates::DISABLE_ENV, disable)
        .env(CHILD_VAR, expect)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child test")
}

/// S7, the environment switch: `DOCKERING_DISABLE_UPDATES` set to anything but empty, `0` or
/// `false` turns every updater request off (and hides a retained offer without deleting it); the
/// values that don't disable leave the updater on.
#[test]
fn upd_e2e_s7_env_opt_out_makes_no_requests() {
    if updater_env_off() || std::env::var_os(CHILD_VAR).is_some() {
        return;
    }
    let children: Vec<(&str, &str, std::process::Child)> = [
        ("1", "off"),
        ("true", "off"),
        ("yes", "off"),
        ("0", "on"),
        ("false", "on"),
        ("", "on"),
    ]
    .into_iter()
    .map(|(value, expect)| {
        (
            value,
            expect,
            run_child("upd_e2e_s7_env_child", value, expect),
        )
    })
    .collect();
    for (value, expect, child) in children {
        let output = child.wait_with_output().expect("the child finishes");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "DOCKERING_DISABLE_UPDATES={value:?} ({expect}):\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains(&format!("S7-CHILD-RAN {expect}")),
            "the child test didn't run ({value:?}):\n{stdout}"
        );
    }
}

/// The body of the S7 environment test; only runs in the child process of
/// [`upd_e2e_s7_env_opt_out_makes_no_requests`].
#[tokio::test]
async fn upd_e2e_s7_env_child() {
    let Ok(expect) = std::env::var(CHILD_VAR) else {
        return;
    };
    let rig = Rig::new();
    rig.fake_appimage();
    let release = rig.publish("0.2.0", PAYLOAD);
    // A retained offer from an earlier session, with its payload, on disk.
    let payload = rig.payload("0.2.0");
    std::fs::create_dir_all(payload.parent().expect("dir")).expect("mkdir");
    std::fs::write(&payload, PAYLOAD).expect("payload");
    let stored = StoredOffer::new(
        release.json.clone(),
        release.sig.clone(),
        rfc3339(rig.host.now()),
        Stored::Ready,
    )
    .expect("offer");
    offer::write(&rig.updates_dir(), &stored).expect("offer.json");
    let json = rig.offer_json();

    let hub = rig.start_with(fast());
    if expect == "off" {
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(rig.counts(), Counts::default(), "not one request");
        let snap = current(&hub).await;
        assert_eq!(
            snap.status,
            UpdateStatus::Disabled {
                reason: DisabledReason::Unavailable
            }
        );
        assert_eq!(snap.offer, None, "the offer is hidden");
        assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::Disabled));
        assert_eq!(
            hub.apply_update().await,
            Err(protocol("updates are disabled"))
        );
        assert_eq!(rig.counts(), Counts::default());
        // Hidden, not deleted (UPD-017 (8)).
        assert_eq!(rig.offer_json(), json);
        assert!(payload.exists());
    } else {
        eventually("two automatic checks", || rig.counts().manifest >= 2).await;
        assert!(current(&hub).await.offer.is_some());
    }
    println!("S7-CHILD-RAN {expect}");
}

// ═══ S8: a restart with an announced offer ══════════════════════════════════════════════════

#[tokio::test]
async fn upd_e2e_s8_restart_with_announced_offer() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    rig.publish("0.2.0", PAYLOAD);

    // Session 1: found, downloaded, announced once.
    let hub = rig.start();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert!(hub.mark_update_notified("0.2.0"), "the toast is shown once");
    assert!(!hub.mark_update_notified("0.2.0"));
    hub.shutdown();
    assert_eq!(rig.counts(), counts(1, 1, 1));
    let json = rig.offer_json_sans_time();
    let saved = || crate::load_config(&rig.paths()).1;
    assert_eq!(saved().updates.notified_version.as_deref(), Some("0.2.0"));

    // Session 2, online, checks on: the offer is back from disk before any request, and the
    // announcement is remembered.
    rig.server.clear_requests();
    let mut opts = Opts::manual();
    opts.ui_state = saved();
    let hub = rig.start_with(opts);
    settled(&hub).await;
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.version.as_str(), offer.mode, offer.stage),
        ("0.2.0", OfferMode::Install, OfferStage::Ready)
    );
    assert_eq!(rig.counts(), Counts::default(), "restore makes no request");
    assert!(!hub.mark_update_notified("0.2.0"), "no second notification");
    // The first check only confirms it: a manifest and a signature, never the payload again.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(rig.counts(), counts(1, 1, 0));
    assert!(the_offer(&current(&hub).await).stage == OfferStage::Ready);
    hub.shutdown();

    // Session 3, offline (the server drops every connection): the offer is shown from disk, and
    // a failed check changes nothing.
    rig.server.clear_requests();
    rig.server.set_fault(Some(Fault::Drop));
    let mut opts = Opts::manual();
    opts.ui_state = saved();
    let hub = rig.start_with(opts);
    settled(&hub).await;
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
    assert_eq!(rig.counts(), Counts::default());
    assert!(!hub.mark_update_notified("0.2.0"));
    assert!(failed_check(&hub).await.starts_with("network error"));
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
    hub.shutdown();
    rig.server.set_fault(None);

    // Session 4, automatic checks off: the offer is there, no schedule runs, and *Restart to
    // update* works from the restored payload.
    rig.server.clear_requests();
    let strategy = Arc::new(FakeStrategy::new());
    let mut opts = Opts::scheduled(Duration::from_millis(20), Duration::from_millis(40))
        .checks_off()
        .strategy(strategy.clone());
    opts.ui_state = saved();
    let hub = rig.start_with(opts);
    settled(&hub).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
    assert_eq!(rig.counts(), Counts::default());
    assert!(!hub.mark_update_notified("0.2.0"));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(
        strategy.last_job().payload.as_deref(),
        Some(rig.payload("0.2.0").as_path())
    );
    assert_eq!(rig.offer_json_sans_time(), json, "the same offer file");
    assert_eq!(rig.counts(), Counts::default());
}

// ═══ S9: the server is down, then up ════════════════════════════════════════════════════════

#[tokio::test]
async fn upd_e2e_s9_server_down_then_up() {
    if updater_env_off() {
        return;
    }
    let (logs, _guard) = capture_logs();
    let rig = Rig::new();
    rig.fake_appimage();
    rig.publish("0.2.0", PAYLOAD);
    rig.server.set_fault(Some(Fault::Down));
    let hub = rig.start_with(Opts::scheduled(
        Duration::from_millis(20),
        Duration::from_millis(60),
    ));

    // Down (every request 503): the automatic checks fail silently, cycle after cycle.
    eventually("two failed cycles", || rig.counts().manifest >= 2).await;
    let snap = current(&hub).await;
    assert_eq!(snap.offer, None);
    assert!(
        !matches!(snap.status, UpdateStatus::Error { .. }),
        "an automatic failure shows nothing: {snap:?}"
    );
    assert_eq!(rig.counts().asset, 0);
    assert_eq!(rig.update_entries(), Vec::<String>::new());
    assert!(
        logs.lines()
            .iter()
            .all(|l| l.contains("server returned HTTP 503")),
        "{:#?}",
        logs.lines()
    );

    // A dead network path (connection dropped) is the same.
    rig.server.set_fault(Some(Fault::Drop));
    let seen = rig.counts().manifest;
    eventually("two cycles with a dropped connection", || {
        rig.counts().manifest >= seen + 2
    })
    .await;
    assert_eq!(current(&hub).await.offer, None);

    // Up: the next cycle finds it, downloads it once, and keeps it.
    rig.server.set_fault(None);
    let snap = until(&hub, SHORT, is_ready("0.2.0")).await;
    assert!(!matches!(snap.status, UpdateStatus::Error { .. }));
    assert_eq!(rig.counts().asset, 1);
    assert_eq!(
        rig.update_files(),
        [format!("0.2.0/{ASSET_NAME}"), "offer.json".to_owned()]
    );
    let json = rig.offer_json();

    // Down again with an offer: the snapshot stays as it is and the file isn't touched.
    rig.server.set_fault(Some(Fault::Down));
    let seen = rig.counts().manifest;
    eventually("two more failed cycles", || {
        rig.counts().manifest >= seen + 2
    })
    .await;
    let snap = current(&hub).await;
    assert_eq!(the_offer(&snap).stage, OfferStage::Ready);
    assert!(!matches!(snap.status, UpdateStatus::Error { .. }));
    assert_eq!(rig.offer_json(), json);
    assert_eq!(rig.counts().asset, 1);
}

// ═══ S11: a re-check and failed checks keep the offer ═══════════════════════════════════════

#[tokio::test]
async fn upd_e2e_s11_recheck_and_failed_check_keep_offer() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let release = rig.publish("0.2.0", PAYLOAD);
    let hub = rig.start();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    let first = rig.stored().expect("offer.json");

    // A re-check of the same release only confirms it: `confirmed_at` moves, the stage stays,
    // no second download.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let again = rig.stored().expect("offer.json");
    assert_ne!(again.confirmed_at, first.confirmed_at);
    assert_eq!(
        StoredOffer {
            confirmed_at: first.confirmed_at.clone(),
            ..again
        },
        first
    );
    assert_eq!(rig.counts(), counts(2, 2, 1));
    let json = rig.offer_json();
    let payload = std::fs::read(rig.payload("0.2.0")).expect("payload");

    // Every kind of failed check leaves the offer, its file and its payload exactly as they were,
    // and says so inline.
    let other = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let (forged, forged_sig) =
        signed_manifest_for(&other, "0.3.0", PAYLOAD, &platform_key(), "appimage");
    let garbage = b"this is not a manifest".to_vec();
    let garbage_sig = sign(&rig.kp, &garbage);
    // (what fails, how to make it fail, the error the check reports)
    type Failure<'a> = (&'a str, Box<dyn Fn() + 'a>, &'a str);
    let failures: Vec<Failure<'_>> = vec![
        (
            "server down",
            Box::new(|| rig.server.set_fault(Some(Fault::Down))),
            "server returned HTTP 503",
        ),
        (
            "dropped connection",
            Box::new(|| rig.server.set_fault(Some(Fault::Drop))),
            "network error",
        ),
        (
            "a manifest signed by another key",
            Box::new(|| rig.serve_manifest(forged.clone(), forged_sig.clone())),
            "the update signature is invalid",
        ),
        (
            "a signed manifest that isn't one",
            Box::new(|| rig.serve_manifest(garbage.clone(), garbage_sig.clone())),
            "invalid update manifest",
        ),
        (
            "no latest release at all",
            Box::new(|| rig.server.remove_route(&manifest_path())),
            "server returned HTTP 404",
        ),
    ];
    let mut requests = 2;
    for (what, make_it_fail, expected) in failures {
        rig.server.set_fault(None);
        rig.make_latest(&release);
        make_it_fail();
        let message = failed_check(&hub).await;
        assert!(message.starts_with(expected), "{what}: {message}");
        let snap = current(&hub).await;
        assert!(is_error(&snap, &message), "{what}: {snap:?}");
        let offer = the_offer(&snap);
        assert_eq!(
            (offer.version.as_str(), offer.mode, offer.stage),
            ("0.2.0", OfferMode::Install, OfferStage::Ready),
            "{what}"
        );
        assert_eq!(rig.offer_json(), json, "{what}");
        assert_eq!(
            std::fs::read(rig.payload("0.2.0")).expect("payload"),
            payload
        );
        requests += 1;
        assert_eq!(rig.counts().manifest, requests, "{what}");
        assert_eq!(rig.counts().asset, 1, "{what}");
    }

    // The next good check clears the error and confirms the offer again.
    rig.server.set_fault(None);
    rig.make_latest(&release);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let snap = current(&hub).await;
    assert!(is_idle(&snap), "{snap:?}");
    assert_eq!(the_offer(&snap).stage, OfferStage::Ready);
    assert_ne!(
        rig.stored().expect("offer.json").confirmed_at,
        again_at(&json)
    );
    assert_eq!(rig.counts().asset, 1);
}

/// `confirmed_at` of the `offer.json` bytes `json`.
fn again_at(json: &[u8]) -> String {
    serde_json::from_slice::<StoredOffer>(json)
        .expect("offer.json parses")
        .confirmed_at
}

// ═══ withdrawal, expiry, apply during a re-check ════════════════════════════════════════════

#[tokio::test]
async fn upd_e2e_withdrawal_after_release_pulled() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    let old = rig.publish("0.2.0", PAYLOAD);
    let strategy = Arc::new(FakeStrategy::new());
    let hub = rig.start_with(Opts::manual().strategy(strategy.clone()));

    // 0.3.0 is the latest and Ready.
    let new = rig.publish("0.3.0", b"the payload of 0.3.0");
    assert_eq!(hub.check_for_updates().await, available("0.3.0"));
    until(&hub, SHORT, is_ready("0.3.0")).await;
    assert!(rig.payload("0.3.0").exists());

    // 0.3.0 is pulled: its assets go and `latest` falls back to 0.2.0. The old offer is not
    // actionable at once (while 0.2.0's payload is still in flight), and is never applied.
    rig.server.remove_route(&new.asset_path);
    rig.make_latest(&old);
    let hold = rig.server.hold(&old.asset_path);
    let check = spawn_check(&hub);
    eventually("the asset request to reach the server", || {
        hold.arrived() == 1
    })
    .await;
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.version.as_str(), offer.stage),
        ("0.2.0", OfferStage::Downloading)
    );
    assert_eq!(
        rig.stored()
            .expect("offer.json")
            .version()
            .map(|v| v.to_string())
            .as_deref(),
        Some("0.2.0")
    );
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install")),
        "the pulled release is never applied"
    );
    assert_eq!(strategy.calls(), 0);
    hold.release();
    assert_eq!(check.await.expect("join"), available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert!(!rig.payload("0.3.0").exists(), "its payload is deleted");
    assert!(rig.payload("0.2.0").exists());
    assert_eq!(
        rig.update_files(),
        [format!("0.2.0/{ASSET_NAME}"), "offer.json".to_owned()]
    );

    // `latest` now has no entry for this platform: withdrawn, with its payload.
    rig.publish_entry("0.2.0", "plan9-m68k", "deb", PAYLOAD);
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    let snap = current(&hub).await;
    assert_eq!(snap.offer, None);
    assert!(is_idle(&snap), "{snap:?}");
    assert_eq!(rig.update_entries(), Vec::<String>::new());

    // ... or only the running version: the same.
    rig.make_latest(&old);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    rig.publish("0.1.0", PAYLOAD);
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    assert_eq!(current(&hub).await.offer, None);
    assert_eq!(rig.update_entries(), Vec::<String>::new());
    assert_eq!(strategy.calls(), 0);
}

#[tokio::test]
async fn upd_e2e_offer_expires_after_14_days_unconfirmed() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    rig.publish("0.2.0", PAYLOAD);
    let strategy = Arc::new(FakeStrategy::new());
    let hub = rig.start_with(Opts::manual().strategy(strategy.clone()));
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    let day = DAY.as_secs() as i64;

    // The server goes away for good, and the wall clock moves on. A failed check is the poke
    // that makes the snapshot read the clock; the offer is evaluated on every read.
    rig.server.set_fault(Some(Fault::Down));
    rig.host.set_skew(14 * day - 3600);
    let _ = failed_check(&hub).await;
    assert!(
        the_offer(&current(&hub).await).version == "0.2.0",
        "13 days 23 hours: still offered"
    );
    rig.host.set_skew(14 * day + 60);
    let _ = failed_check(&hub).await;
    let snap = current(&hub).await;
    assert_eq!(snap.offer, None, "unconfirmed for 14 days: expired");
    // Apply decides on its own and refuses: 'Check for updates first'.
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("Check for updates first"))
    );
    assert_eq!(strategy.calls(), 0);
    assert!(
        rig.stored().is_some(),
        "the file stays for the next successful check"
    );

    // A clock that is more than a day *behind* `confirmed_at` is as good as expired.
    rig.host.set_skew(-(day + 3600));
    let _ = failed_check(&hub).await;
    assert_eq!(
        current(&hub).await.offer,
        None,
        "a future confirmed_at expires"
    );

    // *Check now* against a healthy server confirms the offer again, without a second download.
    rig.host.set_skew(0);
    rig.server.set_fault(None);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert_eq!(rig.counts().asset, 1);
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));

    // Checks off and the network gone: it still expires, by the clock alone, without any other
    // event. No request is made from here on, so the clock can be paused and crossed in a step.
    hub.config().update(|c| c.updates.check = false);
    until(&hub, SHORT, |s| {
        matches!(s.status, UpdateStatus::Disabled { .. })
    })
    .await;
    rig.server.set_fault(Some(Fault::Down));
    let requests = rig.counts();
    tokio::time::pause();
    let first = current(&hub).await;
    assert!(first.offer.is_some());
    let gone = until(&hub, DAY * 15, |s| s.offer.is_none()).await;
    assert!(gone.revision > first.revision);
    assert_eq!(
        gone.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting
        }
    );
    assert_eq!(rig.counts(), requests, "expiry makes no request");
    assert!(rig.stored().is_some());

    // The next start finds it expired and deletes the file (and the payload with it).
    hub.shutdown();
    let hub = rig.start_with(Opts::manual().checks_off());
    settled(&hub).await;
    assert_eq!(current(&hub).await.offer, None);
    assert!(
        rig.stored().is_none(),
        "an expired file is deleted at start"
    );
    assert_eq!(rig.update_files(), Vec::<String>::new());
}

#[tokio::test]
async fn upd_e2e_apply_during_recheck_over_http() {
    if updater_env_off() {
        return;
    }
    let rig = Rig::new();
    rig.fake_appimage();
    rig.publish("0.2.0", b"the payload of 0.2.0");
    let strategy = Arc::new(FakeStrategy::new());
    let hub = rig.start_with(Opts::manual().strategy(strategy.clone()));
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    let ready = rig.payload("0.2.0");

    // A slow re-check: its manifest request is in flight (held at the server) and the check lock
    // is taken. 0.3.0 is the latest by then.
    let new = rig.publish("0.3.0", b"the payload of 0.3.0");
    let hold = rig.server.hold(&manifest_path());
    let check = spawn_check(&hub);
    eventually("the manifest request to reach the server", || {
        hold.arrived() == 1
    })
    .await;
    let snap = current(&hub).await;
    assert_eq!(snap.status, UpdateStatus::Checking);
    assert_eq!(
        the_offer(&snap).stage,
        OfferStage::Ready,
        "checking never hides the offer"
    );

    // The click neither waits for the check nor fails because of it.
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert!(
        !check.is_finished(),
        "the check is still waiting for its reply"
    );
    assert_eq!(strategy.calls(), 1);
    assert_eq!(
        strategy.last_job().payload.as_deref(),
        Some(ready.as_path())
    );

    // The check completes afterwards: 0.3.0 is found, fetched and reconciled as the offer.
    hold.release();
    assert_eq!(check.await.expect("join"), available("0.3.0"));
    until(&hub, SHORT, is_ready("0.3.0")).await;
    assert_eq!(rig.counts(), counts(2, 2, 2));

    // The other order: the check finishes *while* an apply runs. Its result is held, the payload
    // being applied is not touched, and the result lands when the apply ends.
    let release = strategy.hold();
    let apply = hub.apply_update();
    strategy.entered(2).await;
    let ready_new = rig.payload("0.3.0");
    let newest = rig.publish("0.4.0", b"the payload of 0.4.0");
    assert_eq!(hub.check_for_updates().await, available("0.4.0"));
    let snap = current(&hub).await;
    let offer = the_offer(&snap);
    assert_eq!(
        (offer.version.as_str(), offer.stage),
        ("0.3.0", OfferStage::Applying),
        "the apply keeps its offer"
    );
    assert!(
        ready_new.exists(),
        "the payload being applied is still there"
    );
    assert_eq!(
        rig.stored()
            .expect("offer.json")
            .version()
            .map(|v| v.to_string())
            .as_deref(),
        Some("0.3.0")
    );
    assert_eq!(rig.counts().asset, 3, "0.4.0 was fetched meanwhile");
    release.open();
    assert_eq!(apply.await, Ok(ApplyOutcome::Restarting));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.4.0")).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Install, false)
    );
    assert_eq!(
        rig.stored()
            .expect("offer.json")
            .version()
            .map(|v| v.to_string())
            .as_deref(),
        Some("0.4.0")
    );
    // Once it ended, the old payloads are cleaned up; the new one stays.
    eventually("the old payloads to be cleaned", || {
        rig.update_files() == [format!("0.4.0/{ASSET_NAME}"), "offer.json".to_owned()]
    })
    .await;
    let _ = (new, newest);
}

// ═══ UPD-006: a macOS-style or package install gets a *Download* offer ══════════════════════

#[tokio::test]
async fn upd_006_deb_and_macos_get_download_offer_over_http() {
    if updater_env_off() {
        return;
    }
    // (platform key, entry kind, install kind): the platform's entry kind decides, and an install
    // that can't install itself never fetches the asset, whatever the entry says.
    let cases = [
        ("macos-aarch64", "dmg", InstallKind::Other),
        ("macos-x86_64", "dmg", InstallKind::Other),
        ("linux-x86_64", "deb", InstallKind::Other),
        ("windows-x86_64", "zip", InstallKind::Portable),
        // An AppImage install whose manifest entry is not an `appimage` (UPD-013).
        ("linux-x86_64", "deb", InstallKind::AppImage),
        ("linux-x86_64", "tar.gz", InstallKind::AppImage),
    ];
    for (platform, entry, kind) in cases {
        let what = format!("{platform} {entry} {kind:?}");
        let rig = Rig::new();
        rig.host.set_platform(platform);
        rig.host.set_kind(kind);
        if kind == InstallKind::AppImage {
            rig.fake_appimage();
            rig.host.set_platform(platform);
        }
        let release = rig.publish_entry("0.2.0", platform, entry, PAYLOAD);
        let strategy = Arc::new(FakeStrategy::new());
        let hub = rig.start_with(Opts::manual().strategy(strategy.clone()));

        assert_eq!(hub.check_for_updates().await, available("0.2.0"), "{what}");
        let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
        assert_eq!(offer.mode, OfferMode::Download, "{what}");
        assert_eq!(offer.stage, OfferStage::Ready, "found = actionable: {what}");
        assert!(!offer.needs_elevation && !offer.install_failed, "{what}");
        assert_eq!(
            offer.notes_url, "https://github.com/pavel-purma/dockering/releases/tag/v0.2.0",
            "{what}"
        );
        // The manifest and its signature, and not one asset request; no payload on disk.
        assert_eq!(rig.counts(), counts(1, 1, 0), "{what}");
        assert_eq!(rig.server.count(&release.asset_path), 0, "{what}");
        assert_eq!(rig.update_files(), ["offer.json"], "{what}");
        assert_eq!(rig.stored().expect("offer.json").stage, Stored::Ready);
        // It never installs, and the next check keeps it, still without a download.
        assert!(hub.apply_update().await.is_err(), "{what}");
        assert_eq!(strategy.calls(), 0, "{what}");
        assert_eq!(hub.check_for_updates().await, available("0.2.0"), "{what}");
        assert_eq!(rig.counts(), counts(2, 2, 0), "{what}");
        assert_eq!(rig.update_files(), ["offer.json"], "{what}");
    }
}
