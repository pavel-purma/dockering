//! In-app updates (UPD-001…012, spec `features/distribution.md` §7): the hub-side
//! `UpdateService`. It checks the latest GitHub Release on a schedule, downloads and verifies the
//! installer, and starts it on request.
//!
//! The DTOs below are always compiled, so the UI builds the same either way. The network side
//! (`dk-update`) only exists with the `updater` feature; without it the status is
//! `Disabled { reason: Unavailable }` and nothing touches the network (UPD-005).

use serde::{Deserialize, Serialize};

/// What the UI shows (UPD-008). `update_status()` replays the current value first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Updates are off; `reason` says why (UPD-005, UPD-009).
    Disabled {
        reason: DisabledReason,
    },
    Idle {
        last_check: Option<String>,
    },
    Checking,
    /// A newer version exists. `notify_only`: the app can't install it itself (portable zip,
    /// macOS, Linux) and offers a link to the release page instead (UPD-006).
    Available {
        version: String,
        notes_url: String,
        notify_only: bool,
    },
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// Verified installer on disk; *Restart to update* (UPD-007). `needs_elevation` for
    /// all-users installs (the label says a UAC prompt follows).
    Ready {
        version: String,
        notes_url: String,
        needs_elevation: bool,
    },
    /// The last check or download failed. Only manual checks surface this (UPD-004).
    Error {
        message: String,
    },
}

/// Why updates are off (UPD-005). Settings shows a different note for each (UPD-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    /// An administrator policy (`DisableUpdates`): everything off, *Check now* hidden.
    Policy,
    /// The user turned automatic checks off; *Check now* still works.
    Setting,
    /// Not in this build (no `updater` feature), `DOCKERING_DISABLE_UPDATES`, or demo mode.
    Unavailable,
}

/// Result of a manual `check_for_updates()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available { version: String },
    Disabled,
}

/// Persisted updater state (`state.json` → `updates`, UPD-012).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// RFC 3339 time of the last completed check.
    pub last_check: Option<String>,
    /// Short result of the last check ("up to date", "0.3.0 available", or the error).
    pub last_result: Option<String>,
    /// Version that ran last; when it's older than this build, the UI says "Updated to X.Y.Z".
    pub last_run_version: Option<String>,
    /// Version whose "ready to install" notification was already shown (once per version).
    pub notified_version: Option<String>,
    /// Version whose installer is downloaded and verified, kept across restarts (UPD-007).
    pub pending_version: Option<String>,
}

/// Environment switch that turns updates off (UPD-005).
pub const DISABLE_ENV: &str = "DOCKERING_DISABLE_UPDATES";

#[cfg_attr(not(feature = "updater"), allow(dead_code))]
/// Why updates are off, if they are. `setting` is `[updates] check` in `config.toml`.
pub(crate) fn disabled_reason(compiled: bool, demo: bool, setting: bool) -> Option<DisabledReason> {
    if !compiled || demo {
        return Some(DisabledReason::Unavailable);
    }
    if policy_disabled() {
        return Some(DisabledReason::Policy);
    }
    let env_off = std::env::var(DISABLE_ENV).is_ok_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    });
    if env_off {
        return Some(DisabledReason::Unavailable);
    }
    (!setting).then_some(DisabledReason::Setting)
}

#[cfg(feature = "updater")]
fn policy_disabled() -> bool {
    static POLICY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *POLICY.get_or_init(dk_update::policy::updates_disabled_by_policy)
}

#[cfg(not(feature = "updater"))]
#[allow(dead_code)]
fn policy_disabled() -> bool {
    false
}

/// `true` when `a` is an older SemVer than `b` (unparsable → `false`).
#[cfg(feature = "updater")]
pub(crate) fn older(a: &str, b: &str) -> bool {
    matches!(
        (semver::Version::parse(a), semver::Version::parse(b)),
        (Ok(a), Ok(b)) if a < b
    )
}

/// `true` when `a` is an older SemVer than `b` (unparsable → `false`). Without the `semver`
/// dependency: numeric core, a pre-release is older than its release.
#[cfg(not(feature = "updater"))]
pub(crate) fn older(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> Option<(Vec<u64>, String)> {
        let (core, pre) = v.split_once('-').unwrap_or((v, ""));
        let nums = core
            .split('.')
            .map(|p| p.parse().ok())
            .collect::<Option<Vec<u64>>>()?;
        (nums.len() == 3).then(|| (nums, pre.to_owned()))
    }
    match (parse(a), parse(b)) {
        (Some((an, ap)), Some((bn, bp))) => match an.cmp(&bn) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            // 1.0.0-rc.1 < 1.0.0; between pre-releases a plain string order is close enough
            // for "was this an older build".
            std::cmp::Ordering::Equal => match (ap.is_empty(), bp.is_empty()) {
                (false, true) => true,
                (true, _) => false,
                (false, false) => ap < bp,
            },
        },
        _ => false,
    }
}

#[cfg(feature = "updater")]
pub(crate) use service::*;

#[cfg(feature = "updater")]
mod service {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use dk_core::{EngineError, EngineResult};
    use dk_update::source::release_asset_name;
    use dk_update::{InstallKind, PlatformAsset, UpdateError, UpdateSource};
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::hub::{HubInner, lock};

    /// First automatic check after startup (UPD-004).
    pub(crate) const FIRST_CHECK: Duration = Duration::from_secs(30);
    /// Then daily, ± 1 h jitter.
    pub(crate) const INTERVAL: Duration = Duration::from_secs(24 * 3600);
    pub(crate) const JITTER_SECS: u64 = 3600;

    /// A downloaded, verified installer (UPD-007). Kept with its manifest entry so `apply` can
    /// verify the file again right before running it (the updates dir is user-writable).
    #[derive(Clone)]
    pub(crate) struct Pending {
        pub(crate) path: PathBuf,
        pub(crate) version: String,
        pub(crate) notes_url: String,
        pub(crate) asset: PlatformAsset,
    }

    /// Hub-owned updater state.
    pub(crate) struct State {
        pub(crate) status: watch::Sender<UpdateStatus>,
        pub(crate) source: Arc<dyn UpdateSource>,
        pub(crate) kind: InstallKind,
        pub(crate) current: semver::Version,
        pub(crate) keys: Vec<String>,
        pub(crate) running_exe: Option<PathBuf>,
        /// Verified installer waiting for *Restart to update*.
        pub(crate) ready: Mutex<Option<Pending>>,
        /// Id of the latest *manual* check; a manual check superseded by a newer one returns
        /// `Cancelled` (UPD-011). Automatic checks don't touch it.
        pub(crate) request: std::sync::atomic::AtomicU64,
        /// Serialises check + download.
        pub(crate) busy: tokio::sync::Mutex<()>,
        /// Cancelled when updates get turned off at runtime.
        pub(crate) schedule: Mutex<Option<CancellationToken>>,
    }

    impl State {
        pub(crate) fn new(
            source: Arc<dyn UpdateSource>,
            kind: InstallKind,
            current: &str,
            keys: Vec<String>,
        ) -> Self {
            let (status, _) = watch::channel(UpdateStatus::Idle { last_check: None });
            Self {
                status,
                source,
                kind,
                current: semver::Version::parse(current)
                    .unwrap_or_else(|_| semver::Version::new(0, 0, 0)),
                keys,
                running_exe: std::env::current_exe().ok(),
                ready: Mutex::new(None),
                request: std::sync::atomic::AtomicU64::new(0),
                busy: tokio::sync::Mutex::new(()),
                schedule: Mutex::new(None),
            }
        }

        /// Production state: GitHub source, embedded keys, detected install kind.
        pub(crate) fn production() -> Option<Self> {
            let version = env!("CARGO_PKG_VERSION");
            let source = match dk_update::GithubSource::new(version) {
                Ok(s) => Arc::new(s) as Arc<dyn UpdateSource>,
                Err(e) => {
                    tracing::warn!(error = %e, "updater: HTTP client unavailable");
                    return None;
                }
            };
            Some(Self::new(
                source,
                dk_update::install_kind::detect_current(),
                version,
                dk_update::keys::PUBLIC_KEYS
                    .iter()
                    .map(|k| (*k).to_owned())
                    .collect(),
            ))
        }

        pub(crate) fn set(&self, status: UpdateStatus) {
            self.status.send_replace(status);
        }
    }

    fn engine_err(e: UpdateError) -> EngineError {
        EngineError::protocol(e.to_string())
    }

    /// `<data-local>/updates` (UPD-012): next to the logs, never in the roaming profile.
    fn updates_dir(inner: &HubInner) -> PathBuf {
        inner.paths.log_dir.parent().map_or_else(
            || inner.paths.data_dir.join("updates"),
            |p| p.join("updates"),
        )
    }

    fn ready_status(u: &State, p: &Pending) -> UpdateStatus {
        UpdateStatus::Ready {
            version: p.version.clone(),
            notes_url: p.notes_url.clone(),
            needs_elevation: u.kind == InstallKind::InnoMachine,
        }
    }

    fn now_rfc3339() -> Option<String> {
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
    }

    fn idle_status(inner: &HubInner) -> UpdateStatus {
        UpdateStatus::Idle {
            last_check: inner.ui_state().updates.last_check,
        }
    }

    /// Called from `HubInner::start_on`: cleanup, the "updated to" marker, and the schedule.
    pub(crate) fn start(inner: &Arc<HubInner>) {
        let Some(u) = inner.updates.as_ref() else {
            return;
        };
        let setting = lock(&inner.config).updates.check;
        if let Some(reason) = disabled_reason(true, inner.demo, setting) {
            if reason != DisabledReason::Setting || lock(&u.ready).is_none() {
                u.set(UpdateStatus::Disabled { reason });
            }
            // Policy, env, or demo: no network, and nothing pending is kept.
            if reason != DisabledReason::Setting {
                return;
            }
            // Only the setting is off: no schedule, but restore a pending update.
            inner.handle.spawn(restore_pending(inner.clone()));
            return;
        }
        if lock(&u.ready).is_none() {
            u.set(idle_status(inner));
        }
        let token = inner.shutdown.child_token();
        *lock(&u.schedule) = Some(token.clone());
        inner.handle.spawn(schedule(inner.clone(), token));
    }

    /// UPD-007: a downloaded update stays offered across restarts. The pending version is in
    /// `state.json`; its file is only trusted again after a fresh signed-manifest check, so here
    /// we only keep the file (cleanup) and let the next check pick it up without downloading.
    async fn restore_pending(inner: Arc<HubInner>) {
        let pending = inner.ui_state().updates.pending_version;
        dk_update::cleanup::cleanup(&updates_dir(&inner), pending.as_deref()).await;
    }

    /// Re-evaluates the gates after a Settings change (`[updates] check`).
    pub(crate) fn settings_changed(inner: &Arc<HubInner>) {
        let Some(u) = inner.updates.as_ref() else {
            return;
        };
        let setting = lock(&inner.config).updates.check;
        let running = lock(&u.schedule).is_some();
        match disabled_reason(true, inner.demo, setting) {
            Some(reason) => {
                if let Some(t) = lock(&u.schedule).take() {
                    t.cancel();
                }
                // An already verified update stays installable (it was the user's choice).
                if reason != DisabledReason::Setting || lock(&u.ready).is_none() {
                    u.set(UpdateStatus::Disabled { reason });
                }
            }
            None if !running => start(inner),
            None => {}
        }
    }

    async fn schedule(inner: Arc<HubInner>, token: CancellationToken) {
        restore_pending(inner.clone()).await;
        let mut delay = FIRST_CHECK;
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            if let Err(e) = check(&inner, false).await {
                tracing::info!(error = %e, "automatic update check failed");
            }
            let jitter = rand::random::<u64>() % (2 * JITTER_SECS + 1);
            delay = INTERVAL + Duration::from_secs(jitter) - Duration::from_secs(JITTER_SECS);
        }
    }

    /// One check (+ download when the install kind supports it). `manual` surfaces errors.
    pub(crate) async fn check(inner: &Arc<HubInner>, manual: bool) -> EngineResult<UpdateCheck> {
        let Some(u) = inner.updates.as_ref() else {
            return Ok(UpdateCheck::Disabled);
        };
        let setting = lock(&inner.config).updates.check;
        if let Some(reason) = disabled_reason(true, inner.demo, setting) {
            // A manual check while only the setting is off still runs (Settings → *Check
            // now*), but never against policy, env, or demo.
            if reason != DisabledReason::Setting || !manual {
                return Ok(UpdateCheck::Disabled);
            }
        }
        use std::sync::atomic::Ordering::SeqCst;
        let request = manual.then(|| u.request.fetch_add(1, SeqCst) + 1);
        let superseded = || request.is_some_and(|r| u.request.load(SeqCst) != r);
        let _busy = u.busy.lock().await;
        if superseded() {
            // A newer manual check superseded this one (UPD-011).
            return Err(EngineError::Cancelled);
        }
        if !manual && lock(&u.schedule).as_ref().is_none_or(|t| t.is_cancelled()) {
            // Turned off while this automatic check waited.
            return Ok(UpdateCheck::Disabled);
        }
        let had_ready = lock(&u.ready).clone();
        if had_ready.is_none() {
            u.set(UpdateStatus::Checking);
        }
        let result = check_inner(inner, u).await;
        let stale = superseded();
        let summary = match &result {
            Ok(UpdateCheck::UpToDate) => "up to date".to_owned(),
            Ok(UpdateCheck::Available { version }) => format!("{version} available"),
            Ok(UpdateCheck::Disabled) => "disabled".to_owned(),
            Err(e) => e.to_string(),
        };
        let checked_at = now_rfc3339();
        inner.update_ui_state(|s| {
            s.updates.last_check = checked_at.clone();
            s.updates.last_result = Some(summary);
        });
        // `Checking`/`Downloading` never outlive the check (a failed download must not leave
        // "Downloading… n%" behind). `Available`/`Ready` set by `check_inner` stay.
        let in_flight = matches!(
            *u.status.borrow(),
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );
        let ready_now = lock(&u.ready).clone();
        match (&result, ready_now) {
            // A failed re-check never hides an update that is already verified on disk.
            (Err(_), Some(p)) => u.set(ready_status(u, &p)),
            (Err(e), None) if manual && !stale && in_flight => u.set(UpdateStatus::Error {
                message: e.to_string(),
            }),
            (_, None) if in_flight => u.set(UpdateStatus::Idle {
                last_check: checked_at,
            }),
            _ => {}
        }
        let _ = had_ready;
        if stale {
            return Err(EngineError::Cancelled);
        }
        result.map_err(engine_err)
    }

    async fn check_inner(inner: &Arc<HubInner>, u: &State) -> Result<UpdateCheck, UpdateError> {
        let (json, sig) = u.source.fetch_manifest().await?;
        let keys: Vec<&str> = u.keys.iter().map(String::as_str).collect();
        dk_update::verify::verify_manifest(&json, &sig, &keys)?;
        let Some((manifest, asset)) =
            dk_update::manifest::evaluate(&json, &u.current, &dk_update::manifest::platform_key())?
        else {
            // E.g. the pending release was pulled: forget it.
            *lock(&u.ready) = None;
            inner.update_ui_state(|s| s.updates.pending_version = None);
            return Ok(UpdateCheck::UpToDate);
        };
        let version = manifest.version.to_string();
        if let Some(p) = lock(&u.ready).clone()
            && p.version == version
            && p.asset == asset
        {
            u.set(ready_status(u, &p));
            return Ok(UpdateCheck::Available { version });
        }
        if !u.kind.can_install() {
            u.set(UpdateStatus::Available {
                version: version.clone(),
                notes_url: manifest.notes_url.clone(),
                notify_only: true,
            });
            return Ok(UpdateCheck::Available { version });
        }
        let path = download(inner, u, &version, &asset).await?;
        if let Some(exe) = u.running_exe.clone() {
            // WinVerifyTrust blocks (and may build a certificate chain): keep it off the
            // runtime's worker threads.
            let installer = path.clone();
            let trust = tokio::task::spawn_blocking(move || {
                dk_update::authenticode::check_installer(&installer, &exe)
            })
            .await
            .map_err(|e| UpdateError::Io(e.to_string()))?;
            if let Err(e) = trust {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(e);
            }
        }
        let pending = Pending {
            path,
            version: version.clone(),
            notes_url: manifest.notes_url,
            asset,
        };
        u.set(ready_status(u, &pending));
        *lock(&u.ready) = Some(pending);
        inner.update_ui_state(|s| s.updates.pending_version = Some(version.clone()));
        Ok(UpdateCheck::Available { version })
    }

    async fn download(
        inner: &Arc<HubInner>,
        u: &State,
        version: &str,
        asset: &PlatformAsset,
    ) -> Result<PathBuf, UpdateError> {
        let root = updates_dir(inner);
        dk_update::cleanup::cleanup(&root, Some(version)).await;
        let dir = root.join(version);
        let name = release_asset_name(&asset.url, version)?;
        let existing = dir.join(&name);
        if dk_update::verify::verify_file(&existing, &asset.sha256, asset.size)
            .await
            .is_ok()
        {
            return Ok(existing);
        }
        let status = u.status.clone();
        let v = version.to_owned();
        status.send_replace(UpdateStatus::Downloading {
            version: v.clone(),
            done: 0,
            total: asset.size,
        });
        u.source
            .download(
                asset,
                &dir,
                &name,
                Box::new(move |done, total| {
                    status.send_replace(UpdateStatus::Downloading {
                        version: v.clone(),
                        done,
                        total,
                    });
                }),
            )
            .await
    }

    /// Starts the verified installer (UPD-007). The UI quits right after this resolves.
    pub(crate) fn apply(inner: &Arc<HubInner>) -> EngineResult<()> {
        let Some(u) = inner.updates.as_ref() else {
            return Err(EngineError::protocol("updates are disabled"));
        };
        let Some(p) = lock(&u.ready).clone() else {
            return Err(EngineError::protocol("no update is ready to install"));
        };
        // UPD-003: the file sits in a user-writable dir and may have been verified days ago.
        // Check it again right before running it (this runs on a blocking thread).
        let verified = futures::executor::block_on(dk_update::verify::verify_file(
            &p.path,
            &p.asset.sha256,
            p.asset.size,
        ));
        let trusted = verified.and_then(|()| match &u.running_exe {
            Some(exe) => dk_update::authenticode::check_installer(&p.path, exe).map(drop),
            None => Ok(()),
        });
        if let Err(e) = trusted {
            *lock(&u.ready) = None;
            u.set(idle_status(inner));
            return Err(engine_err(e));
        }
        dk_update::apply::spawn_installer(&p.path, u.kind).map_err(engine_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upd_004_older_versions() {
        assert!(older("0.1.0", "0.2.0"));
        assert!(older("0.2.0-rc.1", "0.2.0"));
        assert!(!older("0.2.0", "0.2.0"));
        assert!(!older("0.3.0", "0.2.0"));
        assert!(!older("garbage", "0.2.0"));
    }

    #[test]
    fn upd_005_not_compiled_or_demo_is_disabled() {
        assert_eq!(
            disabled_reason(false, false, true),
            Some(DisabledReason::Unavailable)
        );
        assert_eq!(
            disabled_reason(true, true, true),
            Some(DisabledReason::Unavailable)
        );
        if std::env::var_os(DISABLE_ENV).is_none() && !policy_disabled() {
            assert_eq!(
                disabled_reason(true, false, false),
                Some(DisabledReason::Setting)
            );
            assert_eq!(disabled_reason(true, false, true), None);
        }
    }
}
