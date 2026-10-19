//! The hub-owned updater state (UPD-011, UPD-017): activity, the retained offer, the snapshot the
//! UI sees, and the seams that let tests replace the platform (install-kind probe, clock, trust
//! check, apply strategy).
//!
//! Locking: `core` is a short std mutex, never held across an `.await` or any I/O. `gate`
//! serialises the file work of *committing* an offer change with the start and end of an apply;
//! it is only taken on blocking workers. The check/download lock (`busy`) is separate, and an
//! apply never takes it (UPD-011).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dk_update::cleanup::KeepSet;
use dk_update::offer::{self, OfferStage as Stored, StoredOffer};
use dk_update::{InstallKind, PlatformAsset, UpdateError, UpdateSource};
use semver::Version;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use super::apply::ApplyStrategy;
use super::{DisabledReason, OfferMode, OfferStage, UpdateOffer, UpdateSnapshot, UpdateStatus};
use crate::hub::lock;

/// First automatic check after startup (UPD-004).
pub(crate) const FIRST_CHECK: Duration = Duration::from_secs(30);
/// Then daily, ± 1 h jitter.
pub(crate) const INTERVAL: Duration = Duration::from_secs(24 * 3600);
pub(crate) const JITTER_SECS: u64 = 3600;

/// An environment lookup (name → value); `State::production` passes the process
/// environment (only for the `update-e2e` seam), tests pass a map (REL-086).
pub(crate) type EnvFn<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Everything the updater asks of the machine it runs on. Production is [`SystemHost`]; tests
/// inject a fake, so a macOS-style *Download* offer, a lost install eligibility, a signature the
/// running exe rejects or a 14-day expiry can be tested on every OS without waiting.
///
/// `install_kind`, `appimage_target` and `check_installer` do blocking work: the hub calls them
/// on blocking workers (never `State::production`, which runs before the window).
pub(crate) trait Host: Send + Sync {
    /// The `<os>-<arch>` manifest key of this build (UPD-002).
    fn platform(&self) -> String;
    /// How this copy was installed, probed again at every check and at apply (UPD-006).
    fn install_kind(&self) -> InstallKind;
    /// The real path of the AppImage to replace, when this is a replaceable AppImage (UPD-013).
    fn appimage_target(&self) -> Option<PathBuf>;
    /// The wall clock (freshness of the offer, `confirmed_at`, `last_check`).
    fn now(&self) -> SystemTime;
    /// The Authenticode pin of a freshly downloaded `installer` against the running exe
    /// (UPD-003). At the click the pin runs again inside `spawn_installer_locked`, through the
    /// handle; the hub does not call this at apply.
    fn check_installer(&self, installer: &Path, running_exe: &Path) -> Result<(), UpdateError>;
}

/// The real machine.
pub(crate) struct SystemHost;

impl Host for SystemHost {
    fn platform(&self) -> String {
        dk_update::manifest::platform_key()
    }
    fn install_kind(&self) -> InstallKind {
        dk_update::install_kind::detect_current()
    }
    fn appimage_target(&self) -> Option<PathBuf> {
        dk_update::install_kind::current_appimage_path()
    }
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn check_installer(&self, installer: &Path, running_exe: &Path) -> Result<(), UpdateError> {
        dk_update::authenticode::check_installer(installer, running_exe).map(drop)
    }
}

/// How `HubInner` gets its updater.
// `Off` and `State` are what tests inject; production only ever asks for `Production`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum UpdaterInit {
    /// The GitHub source and embedded keys ([`State::production`]).
    Production,
    /// No updater (hub tests that aren't about updates: nothing may reach the network).
    Off,
    /// A ready-made state (updater tests: static source, fake host).
    State(Box<State>),
}

/// Whether an install of `kind` may install `asset` itself, and a failed install never retries
/// (UPD-006, UPD-017 (6)).
pub(crate) fn mode_for(
    kind: InstallKind,
    asset: &PlatformAsset,
    install_failed: bool,
) -> OfferMode {
    if kind.can_install()
        && dk_update::install_kind::asset_supports_install(kind, asset.kind)
        && !install_failed
    {
        OfferMode::Install
    } else {
        OfferMode::Download
    }
}

/// The retained offer in memory (UPD-017): the stored form that `offer.json` holds, what was
/// parsed from it, and what only the hub knows (mode, the verified payload, *Applying*).
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    /// The persisted part: signed manifest, `confirmed_at`, stage, failed flag. `stored.stage`
    /// is never *Applying*: that is [`Offer::applying`].
    pub(crate) stored: StoredOffer,
    pub(crate) version: Version,
    pub(crate) notes_url: String,
    /// The manifest entry for this platform.
    pub(crate) asset: PlatformAsset,
    pub(crate) mode: OfferMode,
    /// The install kind of the last probe (decides `needs_elevation`).
    pub(crate) kind: InstallKind,
    /// *Restart to update* is running for this offer (memory only).
    pub(crate) applying: bool,
    /// The verified payload; `Some` only for a *Ready* install offer.
    pub(crate) payload: Option<PathBuf>,
}

impl Offer {
    /// The offer `stored` describes for `platform`, with its mode decided for `kind`. `None` when
    /// the manifest doesn't parse or has no entry for `platform`.
    pub(crate) fn from_stored(
        stored: StoredOffer,
        platform: &str,
        kind: InstallKind,
    ) -> Option<Self> {
        let manifest = stored.parse_manifest()?;
        let asset = manifest.platforms.get(platform)?.clone();
        let mut offer = Self {
            version: manifest.version,
            notes_url: manifest.notes_url,
            asset,
            mode: OfferMode::Download,
            kind,
            applying: false,
            payload: None,
            stored,
        };
        offer.set_kind(kind);
        Some(offer)
    }

    /// Re-decides the mode after an install-kind probe (UPD-006). Losing eligibility downgrades an
    /// install offer to *Download*, never removes it; regaining it makes the offer *Downloading*
    /// again, because nothing was fetched for it.
    pub(crate) fn set_kind(&mut self, kind: InstallKind) {
        self.kind = kind;
        let mode = mode_for(kind, &self.asset, self.stored.install_failed);
        if mode == OfferMode::Install
            && self.mode == OfferMode::Download
            && self.stored.stage == Stored::Ready
        {
            self.stored.stage = Stored::Downloading;
            self.payload = None;
        }
        self.mode = mode;
        if mode == OfferMode::Download {
            // Nothing to fetch or to restart: the offer is as actionable as it will ever be.
            if matches!(
                self.stored.stage,
                Stored::Downloading | Stored::InstalledRestart
            ) {
                self.stored.stage = Stored::Ready;
            }
            self.payload = None;
        }
    }

    /// A failed install: *Download* for this version until a newer one arrives (UPD-017 (6)).
    pub(crate) fn degrade(&mut self) {
        self.stored.mark_install_failed();
        let kind = self.kind;
        self.set_kind(kind);
    }

    /// The UI's view of this offer.
    pub(crate) fn dto(&self) -> UpdateOffer {
        UpdateOffer {
            version: self.version.to_string(),
            notes_url: self.notes_url.clone(),
            mode: self.mode,
            stage: if self.applying {
                OfferStage::Applying
            } else {
                match self.stored.stage {
                    Stored::Downloading => OfferStage::Downloading,
                    Stored::Ready => OfferStage::Ready,
                    Stored::InstalledRestart => OfferStage::InstalledRestart,
                }
            },
            needs_elevation: self.mode == OfferMode::Install
                && self.kind == InstallKind::InnoMachine,
            install_failed: self.stored.install_failed,
        }
    }
}

/// A change to the retained offer, committed by `flow::commit` (or held while an apply runs).
#[derive(Clone, Debug)]
pub(crate) enum Change {
    /// A different entry became the offer.
    Replace(Box<Offer>),
    /// The same entry was confirmed again; the install kind may have changed.
    Refresh {
        confirmed_at: String,
        kind: InstallKind,
    },
    /// The release is gone: no offer, no payload.
    Withdraw,
    /// The payload of `version` is missing or failed its check: not actionable until fetched.
    Downloading { version: Version },
    /// The payload of `version` is verified: actionable.
    Ready { version: Version, payload: PathBuf },
    /// The installer of `version` can't be trusted (a signed running exe rejects it): *Download*.
    Degrade { version: Version },
}

impl Change {
    /// The version whose payload this change brings in, for the cleanup keep set.
    fn incoming_version(&self) -> Option<String> {
        match self {
            Self::Replace(offer) => Some(offer.version.to_string()),
            _ => None,
        }
    }
}

/// The offer after `change` (pure). A change aimed at another version than the current one is
/// ignored, so a late result can never touch a newer offer.
pub(crate) fn apply_change(current: Option<Offer>, change: &Change) -> Option<Offer> {
    match (current, change) {
        (_, Change::Replace(offer)) => Some((**offer).clone()),
        (_, Change::Withdraw) => None,
        (Some(mut offer), Change::Refresh { confirmed_at, kind }) => {
            offer.stored = offer.stored.refreshed(confirmed_at.clone());
            offer.set_kind(*kind);
            Some(offer)
        }
        (Some(mut offer), Change::Downloading { version }) if offer.version == *version => {
            offer.stored.stage = Stored::Downloading;
            offer.payload = None;
            Some(offer)
        }
        (Some(mut offer), Change::Ready { version, payload }) if offer.version == *version => {
            offer.stored.stage = Stored::Ready;
            offer.payload = Some(payload.clone());
            Some(offer)
        }
        (Some(mut offer), Change::Degrade { version }) if offer.version == *version => {
            offer.degrade();
            Some(offer)
        }
        (current, _) => current,
    }
}

/// What the lock protects.
pub(crate) struct Core {
    /// What the updater is doing now.
    pub(crate) activity: UpdateStatus,
    /// The retained offer, of any freshness (expiry is applied when a snapshot is built).
    pub(crate) offer: Option<Offer>,
    /// The version an apply runs for (UPD-011).
    pub(crate) applying: Option<String>,
    /// A check's result that finished during an apply; reconciled when the apply ends.
    pub(crate) held: Option<Change>,
    /// Why updates are off, if they are. A hard reason hides the offer (UPD-017 (8)).
    pub(crate) disabled: Option<DisabledReason>,
}

/// The snapshot for a state at `now` (pure): the activity as is, the offer unless it is hidden by
/// a hard disable or expired ([`offer::is_fresh`], evaluated on every build, UPD-017 (2)).
pub(crate) fn build_snapshot(
    activity: &UpdateStatus,
    offer: Option<&Offer>,
    now: SystemTime,
    disabled: Option<DisabledReason>,
    revision: u64,
) -> UpdateSnapshot {
    let hidden = disabled.is_some_and(DisabledReason::is_hard);
    UpdateSnapshot {
        status: activity.clone(),
        offer: offer
            .filter(|o| !hidden && offer::is_fresh(&o.stored, now))
            .map(Offer::dto),
        revision,
    }
}

fn unix_nanos(time: SystemTime) -> i128 {
    let nanos = |d: Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => nanos(after),
        Err(before) => -nanos(before.duration()),
    }
}

/// How long until `stored` expires, or `None` when it already has (or `confirmed_at` is
/// unparsable, which counts as expired).
pub(crate) fn expires_in(stored: &StoredOffer, now: SystemTime) -> Option<Duration> {
    let confirmed = OffsetDateTime::parse(&stored.confirmed_at, &Rfc3339).ok()?;
    let max_age = i128::try_from(offer::MAX_AGE.as_nanos()).ok()?;
    let remaining = confirmed
        .unix_timestamp_nanos()
        .checked_add(max_age)?
        .checked_sub(unix_nanos(now))?;
    (remaining > 0).then(|| Duration::from_nanos(u64::try_from(remaining).unwrap_or(u64::MAX)))
}

/// RFC 3339 text of `time` (for `confirmed_at` and `last_check`).
pub(crate) fn rfc3339(time: SystemTime) -> String {
    OffsetDateTime::from(time)
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// Hub-owned updater state.
pub(crate) struct State {
    /// The snapshot the UI streams; replaced only when the status or the offer changes.
    pub(crate) snapshot: watch::Sender<UpdateSnapshot>,
    pub(crate) source: Arc<dyn UpdateSource>,
    pub(crate) host: Arc<dyn Host>,
    /// What *Restart to update* runs (UPD-011).
    pub(crate) strategy: Arc<dyn ApplyStrategy>,
    pub(crate) current: Version,
    pub(crate) keys: Vec<String>,
    pub(crate) running_exe: Option<PathBuf>,
    pub(crate) core: Mutex<Core>,
    /// See the module documentation.
    pub(crate) gate: Mutex<()>,
    /// An apply is running; a second one fails fast (UPD-011). Never the check lock.
    pub(crate) applying: AtomicBool,
    /// Id of the latest *manual* check; a manual check superseded by a newer one returns
    /// `Cancelled` (UPD-011). Automatic checks don't touch it.
    pub(crate) request: AtomicU64,
    /// Serialises check + download + cleanup. An `Arc` so `start` can hold it from before the
    /// hub handle exists: the restore at start runs before any check (UPD-017 (1)).
    pub(crate) busy: Arc<tokio::sync::Mutex<()>>,
    /// Cancelled when updates get turned off at runtime.
    pub(crate) schedule: Mutex<Option<CancellationToken>>,
    /// Wakes the expiry timer when the offer changes.
    pub(crate) wake: Notify,
    /// Delay before the first automatic check (UPD-004): [`FIRST_CHECK`] unless a test or
    /// the `update-e2e` seam sets it.
    pub(crate) first_check: Duration,
    /// Time between automatic checks: [`INTERVAL`] by default.
    pub(crate) interval: Duration,
    /// The interval varies by ± this many seconds ([`JITTER_SECS`]).
    pub(crate) jitter_secs: u64,
}

impl State {
    pub(crate) fn new(
        source: Arc<dyn UpdateSource>,
        host: Arc<dyn Host>,
        current: &str,
        keys: Vec<String>,
    ) -> Self {
        let (snapshot, _) = watch::channel(UpdateSnapshot {
            status: UpdateStatus::Idle { last_check: None },
            offer: None,
            revision: 0,
        });
        Self {
            snapshot,
            source,
            host,
            strategy: Arc::new(super::apply::SystemStrategy::production()),
            current: Version::parse(current).unwrap_or_else(|_| Version::new(0, 0, 0)),
            keys,
            running_exe: std::env::current_exe().ok(),
            core: Mutex::new(Core {
                activity: UpdateStatus::Idle { last_check: None },
                offer: None,
                applying: None,
                held: None,
                disabled: None,
            }),
            gate: Mutex::new(()),
            applying: AtomicBool::new(false),
            request: AtomicU64::new(0),
            busy: Arc::new(tokio::sync::Mutex::new(())),
            schedule: Mutex::new(None),
            wake: Notify::new(),
            first_check: FIRST_CHECK,
            interval: INTERVAL,
            jitter_secs: JITTER_SECS,
        }
    }

    /// Production state: GitHub source, embedded keys, the real machine. Probes nothing here:
    /// the install kind is read on a blocking worker at every check and at apply. Reads the
    /// process environment only in a build with the `update-e2e` seam.
    pub(crate) fn production() -> Option<Self> {
        #[cfg(feature = "update-e2e")]
        let env = crate::e2e::seam::process_env;
        #[cfg(not(feature = "update-e2e"))]
        let env = |_: &str| -> Option<String> { None };
        Self::production_with(&env)
    }

    /// [`State::production`] with an injected environment: the `update-e2e` seam (REL-086)
    /// reads its `DOCKERING_UPDATE_E2E_*` variables from `env`; without the feature `env`
    /// is never consulted.
    pub(crate) fn production_with(env: EnvFn<'_>) -> Option<Self> {
        let version = crate::app_version();
        #[cfg(feature = "update-e2e")]
        let seam = crate::e2e::seam::Seam::from_env(env);
        #[cfg(not(feature = "update-e2e"))]
        let _ = env;

        #[cfg(feature = "update-e2e")]
        let source = match seam.base_url.as_deref() {
            // An explicit but unusable base fails closed (no updater), never falls back
            // to GitHub.
            Some(base) => dk_update::GithubSource::with_base(version, base),
            None => dk_update::GithubSource::new(version),
        };
        #[cfg(not(feature = "update-e2e"))]
        let source = dk_update::GithubSource::new(version);
        let source = match source {
            Ok(s) => Arc::new(s) as Arc<dyn UpdateSource>,
            Err(e) => {
                tracing::warn!(error = %e, "updater: HTTP client unavailable");
                return None;
            }
        };
        #[cfg_attr(not(feature = "update-e2e"), allow(unused_mut))]
        let mut state = Self::new(
            source,
            Arc::new(SystemHost),
            version,
            dk_update::keys::PUBLIC_KEYS
                .iter()
                .map(|k| (*k).to_owned())
                .collect(),
        );
        #[cfg(feature = "update-e2e")]
        {
            if let Some(key) = seam.key {
                state.keys = vec![key];
            }
            if let Some(first_check) = seam.first_check {
                state.first_check = first_check;
            }
            if let Some(interval) = seam.interval {
                state.interval = interval;
                // A seconds-scale interval can't carry an hour of jitter.
                state.jitter_secs = 0;
            }
        }
        Some(state)
    }

    /// Rebuilds the snapshot from `core` and streams it when the status or the offer changed;
    /// the revision then increases by one (UPD-011). Also the expiry re-evaluation: the clock
    /// is read here, on every build.
    pub(crate) fn publish_locked(&self, core: &Core) {
        let next = build_snapshot(
            &core.activity,
            core.offer.as_ref(),
            self.host.now(),
            core.disabled,
            0,
        );
        let mut offer_changed = false;
        self.snapshot.send_if_modified(|current| {
            offer_changed = current.offer != next.offer;
            if current.status == next.status && !offer_changed {
                return false;
            }
            current.status = next.status;
            current.offer = next.offer;
            current.revision += 1;
            true
        });
        if offer_changed {
            self.wake.notify_one();
        }
    }

    pub(crate) fn publish(&self) {
        let core = lock(&self.core);
        self.publish_locked(&core);
    }

    /// Sets what the updater is doing and streams the change.
    pub(crate) fn set_activity(&self, status: UpdateStatus) {
        let mut core = lock(&self.core);
        core.activity = status;
        self.publish_locked(&core);
    }

    /// Records why updates are off (or that they're on). When they are on, a `Disabled` or `Idle`
    /// activity becomes `idle`; an in-flight check's activity is left alone.
    pub(crate) fn set_disabled(&self, reason: Option<DisabledReason>, idle: UpdateStatus) {
        let mut core = lock(&self.core);
        core.disabled = reason;
        match reason {
            Some(reason) => core.activity = UpdateStatus::Disabled { reason },
            None if matches!(
                core.activity,
                UpdateStatus::Disabled { .. } | UpdateStatus::Idle { .. }
            ) =>
            {
                core.activity = idle;
            }
            None => {}
        }
        self.publish_locked(&core);
    }

    /// Time until the visible offer expires; `None` when there is none to expire.
    pub(crate) fn next_expiry(&self) -> Option<Duration> {
        let core = lock(&self.core);
        if core.disabled.is_some_and(DisabledReason::is_hard) {
            return None;
        }
        expires_in(&core.offer.as_ref()?.stored, self.host.now())
    }

    /// The version directories cleanup must keep (UPD-012).
    pub(crate) fn keep_set(&self) -> KeepSet {
        let core = lock(&self.core);
        KeepSet {
            offered: core.offer.as_ref().map(|o| o.version.to_string()),
            applying: core.applying.clone(),
            downloading: core
                .held
                .as_ref()
                .and_then(Change::incoming_version)
                .into_iter()
                .collect(),
        }
    }
}

/// Delay until the next automatic check: `interval` ± `jitter_secs`, picked by the random
/// number `r` (UPD-004). Never panics, whatever the inputs.
pub(crate) fn next_delay(interval: Duration, jitter_secs: u64, r: u64) -> Duration {
    let offset = r % jitter_secs.saturating_mul(2).saturating_add(1);
    interval
        .saturating_add(Duration::from_secs(offset))
        .saturating_sub(Duration::from_secs(jitter_secs))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PLATFORM: &str = "linux-x86_64";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// 2027-01-15, the clock these tests agree on.
    fn now() -> SystemTime {
        at(1_800_000_000)
    }

    fn manifest(version: &str, kind: &str) -> String {
        serde_json::to_string(&json!({
            "schema": 1,
            "version": version,
            "pub_date": "2026-10-03T00:00:00Z",
            "notes_url": format!("https://github.com/pavel-purma/dockering/releases/tag/v{version}"),
            "platforms": { PLATFORM: {
                "kind": kind,
                "url": format!(
                    "https://github.com/pavel-purma/dockering/releases/download/v{version}/Dockering-x86_64.AppImage"
                ),
                "sha256": "00".repeat(32),
                "size": 4,
            }},
        }))
        .expect("json")
    }

    fn stored(version: &str, kind: &str, stage: Stored, confirmed_secs_ago: u64) -> StoredOffer {
        StoredOffer {
            manifest: manifest(version, kind),
            signature: "sig".into(),
            confirmed_at: rfc3339(now() - Duration::from_secs(confirmed_secs_ago)),
            stage,
            install_failed: false,
        }
    }

    fn offer(kind: InstallKind, stage: Stored, confirmed_secs_ago: u64) -> Offer {
        Offer::from_stored(
            stored("0.4.0", "appimage", stage, confirmed_secs_ago),
            PLATFORM,
            kind,
        )
        .expect("offer")
    }

    const DAY: u64 = 24 * 3600;

    #[test]
    fn upd_011_snapshot_is_a_pure_function_of_its_inputs() {
        let activity = UpdateStatus::Checking;
        let mut o = offer(InstallKind::AppImage, Stored::Ready, 60);
        o.stored.stage = Stored::Ready;
        let a = build_snapshot(&activity, Some(&o), now(), None, 7);
        let b = build_snapshot(&activity, Some(&o), now(), None, 7);
        assert_eq!(a, b);
        assert_eq!(
            a.status,
            UpdateStatus::Checking,
            "activity is passed through"
        );
        assert_eq!(a.revision, 7);
        let dto = a.offer.expect("offer");
        assert_eq!(dto.version, "0.4.0");
        assert_eq!(dto.mode, OfferMode::Install);
        assert_eq!(dto.stage, OfferStage::Ready);
        assert!(!dto.needs_elevation && !dto.install_failed);
        assert_eq!(
            dto.notes_url,
            "https://github.com/pavel-purma/dockering/releases/tag/v0.4.0"
        );
        // No offer, no offer in the snapshot.
        assert_eq!(build_snapshot(&activity, None, now(), None, 0).offer, None);
    }

    #[test]
    fn upd_017_snapshot_hides_an_expired_offer_and_keeps_a_fresh_one() {
        let activity = UpdateStatus::Idle { last_check: None };
        let fresh = offer(InstallKind::AppImage, Stored::Ready, 14 * DAY);
        assert!(
            build_snapshot(&activity, Some(&fresh), now(), None, 0)
                .offer
                .is_some()
        );
        let expired = offer(InstallKind::AppImage, Stored::Ready, 14 * DAY + 1);
        assert!(
            build_snapshot(&activity, Some(&expired), now(), None, 0)
                .offer
                .is_none()
        );
        // The same offer, read a day later, expires: the clock is an input, not a cache.
        assert!(
            build_snapshot(
                &activity,
                Some(&fresh),
                now() + Duration::from_secs(DAY),
                None,
                0
            )
            .offer
            .is_none()
        );
        // A `confirmed_at` too far in the future (a skewed clock) counts as expired.
        let skewed = offer(InstallKind::AppImage, Stored::Ready, 0);
        assert!(
            build_snapshot(
                &activity,
                Some(&skewed),
                now() - Duration::from_secs(2 * DAY),
                None,
                0
            )
            .offer
            .is_none()
        );
    }

    #[test]
    fn upd_017_snapshot_hides_the_offer_on_a_hard_disable_only() {
        let o = offer(InstallKind::AppImage, Stored::Ready, 60);
        for (reason, visible) in [
            (None, true),
            (Some(DisabledReason::Setting), true),
            (Some(DisabledReason::Policy), false),
            (Some(DisabledReason::Unavailable), false),
        ] {
            let activity = reason.map_or(UpdateStatus::Checking, |reason| UpdateStatus::Disabled {
                reason,
            });
            let snap = build_snapshot(&activity, Some(&o), now(), reason, 0);
            assert_eq!(snap.offer.is_some(), visible, "{reason:?}");
            assert_eq!(snap.status, activity);
        }
    }

    #[test]
    fn upd_011_applying_is_memory_only_and_shown_as_a_stage() {
        let mut o = offer(InstallKind::AppImage, Stored::Ready, 60);
        o.stored.stage = Stored::Ready;
        o.applying = true;
        let snap = build_snapshot(&UpdateStatus::Checking, Some(&o), now(), None, 0);
        assert_eq!(snap.offer.expect("offer").stage, OfferStage::Applying);
        // The stored (persisted) stage is untouched, so `offer.json` never says "applying".
        assert_eq!(o.stored.stage, Stored::Ready);
        o.applying = false;
        o.stored.stage = Stored::InstalledRestart;
        let snap = build_snapshot(&UpdateStatus::Checking, Some(&o), now(), None, 0);
        assert_eq!(
            snap.offer.expect("offer").stage,
            OfferStage::InstalledRestart
        );
    }

    #[test]
    fn upd_006_mode_follows_kind_asset_and_failure() {
        let appimage = offer(InstallKind::AppImage, Stored::Ready, 1);
        assert_eq!(appimage.mode, OfferMode::Install);
        // A restored `Ready` install offer has no verified payload yet: not actionable.
        assert_eq!(appimage.stored.stage, Stored::Downloading);
        for kind in [InstallKind::Other, InstallKind::Portable] {
            let o = offer(kind, Stored::Downloading, 1);
            assert_eq!(o.mode, OfferMode::Download, "{kind:?}");
            assert_eq!(
                o.stored.stage,
                Stored::Ready,
                "a Download offer needs nothing fetched"
            );
        }
        // The asset kind must fit the install kind.
        let wrong = Offer::from_stored(
            stored("0.4.0", "deb", Stored::Downloading, 1),
            PLATFORM,
            InstallKind::AppImage,
        )
        .expect("offer");
        assert_eq!(wrong.mode, OfferMode::Download);
        // Windows: elevation only for an all-users install, and only when it can install.
        let inno = |kind| {
            Offer::from_stored(
                stored("0.4.0", "inno", Stored::Downloading, 1),
                PLATFORM,
                kind,
            )
            .expect("offer")
        };
        assert!(inno(InstallKind::InnoMachine).dto().needs_elevation);
        assert!(!inno(InstallKind::InnoUser).dto().needs_elevation);
        assert!(!inno(InstallKind::Portable).dto().needs_elevation);
        // A failed install degrades to Download and the flag survives a probe.
        let mut o = offer(InstallKind::AppImage, Stored::Ready, 1);
        o.degrade();
        assert_eq!(o.mode, OfferMode::Download);
        assert!(o.dto().install_failed);
        o.set_kind(InstallKind::AppImage);
        assert_eq!(o.mode, OfferMode::Download);
        // No entry for this platform: no offer.
        assert!(
            Offer::from_stored(
                stored("0.4.0", "appimage", Stored::Ready, 1),
                "macos-aarch64",
                InstallKind::Other
            )
            .is_none()
        );
    }

    #[test]
    fn upd_006_lost_eligibility_downgrades_and_regained_eligibility_redownloads() {
        let mut o = offer(InstallKind::AppImage, Stored::Ready, 1);
        o.stored.stage = Stored::Ready;
        o.payload = Some(PathBuf::from("/payload"));
        o.set_kind(InstallKind::Other);
        assert_eq!(
            (o.mode, o.stored.stage),
            (OfferMode::Download, Stored::Ready)
        );
        assert_eq!(o.payload, None);
        o.set_kind(InstallKind::AppImage);
        assert_eq!(o.mode, OfferMode::Install);
        assert_eq!(
            o.stored.stage,
            Stored::Downloading,
            "no payload was ever fetched for it"
        );
    }

    #[test]
    fn upd_017_changes_only_touch_the_offer_they_name() {
        let base = offer(InstallKind::AppImage, Stored::Downloading, 60);
        let v = base.version.clone();
        let other = Version::new(9, 9, 9);
        let ready = |version: &Version| Change::Ready {
            version: version.clone(),
            payload: PathBuf::from("/p"),
        };
        let done = apply_change(Some(base.clone()), &ready(&v)).expect("offer");
        assert_eq!(done.stored.stage, Stored::Ready);
        assert_eq!(done.payload.as_deref(), Some(Path::new("/p")));
        // A result for another version is ignored.
        let same = apply_change(Some(base.clone()), &ready(&other)).expect("offer");
        assert_eq!(same.stored.stage, Stored::Downloading);
        let same =
            apply_change(Some(base.clone()), &Change::Degrade { version: other }).expect("offer");
        assert!(!same.stored.install_failed);
        // Degrade, Downloading, Refresh and Withdraw.
        let degraded = apply_change(Some(done.clone()), &Change::Degrade { version: v.clone() })
            .expect("offer");
        assert_eq!(
            (degraded.mode, degraded.stored.install_failed),
            (OfferMode::Download, true)
        );
        let back =
            apply_change(Some(done.clone()), &Change::Downloading { version: v }).expect("offer");
        assert_eq!(
            (back.stored.stage, back.payload),
            (Stored::Downloading, None)
        );
        let refreshed = apply_change(
            Some(done.clone()),
            &Change::Refresh {
                confirmed_at: "2027-01-15T00:00:00Z".into(),
                kind: InstallKind::AppImage,
            },
        )
        .expect("offer");
        assert_eq!(refreshed.stored.confirmed_at, "2027-01-15T00:00:00Z");
        assert_eq!(
            refreshed.stored.stage,
            Stored::Ready,
            "refresh keeps the stage"
        );
        assert!(apply_change(Some(done), &Change::Withdraw).is_none());
        // A refresh with no offer stays no offer; a replacement needs no predecessor.
        let refresh = Change::Refresh {
            confirmed_at: String::new(),
            kind: InstallKind::Other,
        };
        assert!(apply_change(None, &refresh).is_none());
        assert!(apply_change(None, &Change::Replace(Box::new(base))).is_some());
    }

    #[test]
    fn upd_017_expires_in_counts_down_and_stops_at_expiry() {
        let o = stored("0.4.0", "appimage", Stored::Ready, 0);
        let max = offer::MAX_AGE;
        assert_eq!(expires_in(&o, now()), Some(max));
        assert_eq!(
            expires_in(&o, now() + Duration::from_secs(DAY)),
            Some(max - Duration::from_secs(DAY))
        );
        assert_eq!(
            expires_in(&o, now() + max),
            None,
            "expired exactly when the time is up"
        );
        assert_eq!(expires_in(&o, now() + max + Duration::from_secs(1)), None);
        let mut junk = o;
        junk.confirmed_at = "yesterday".into();
        assert_eq!(
            expires_in(&junk, now()),
            None,
            "unparsable counts as expired"
        );
    }
}
