//! In-app updates (UPD-001…012, spec `features/distribution.md` §7): the hub-side
//! `UpdateService`. It checks the latest GitHub Release on a schedule, downloads and verifies the
//! installer, and starts it on request.
//!
//! The DTOs below are always compiled, so the UI builds the same either way. The network side
//! (`dk-update`) only exists with the `updater` feature; without it the status is
//! `Disabled { by_policy: false }` and nothing touches the network (UPD-005).

use serde::{Deserialize, Serialize};

/// What the UI shows (UPD-008). `update_status()` replays the current value first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Updates are off: not compiled in, turned off in Settings, `DOCKERING_DISABLE_UPDATES`,
    /// demo mode, or (`by_policy`) an administrator policy, which also hides *Check now*.
    Disabled {
        by_policy: bool,
    },
    /// Nothing to do. `last_check` is an RFC 3339 time, `None` if never checked.
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
}

/// Environment switch that turns updates off (UPD-005).
pub const DISABLE_ENV: &str = "DOCKERING_DISABLE_UPDATES";

#[cfg_attr(not(feature = "updater"), allow(dead_code))]
/// Why updates are off, if they are. `setting` is `[updates] check` in `config.toml`.
pub(crate) fn disabled_reason(compiled: bool, demo: bool, setting: bool) -> Option<bool> {
    if !compiled || demo {
        return Some(false);
    }
    if policy_disabled() {
        return Some(true);
    }
    let env_off = std::env::var(DISABLE_ENV).is_ok_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    });
    if env_off || !setting {
        return Some(false);
    }
    None
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
    use dk_update::source::asset_file_name;
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

    /// Hub-owned updater state.
    pub(crate) struct State {
        pub(crate) status: watch::Sender<UpdateStatus>,
        pub(crate) source: Arc<dyn UpdateSource>,
        pub(crate) kind: InstallKind,
        pub(crate) current: semver::Version,
        pub(crate) keys: Vec<String>,
        pub(crate) running_exe: Option<PathBuf>,
        /// Verified installer ready to run: (path, version).
        pub(crate) ready: Mutex<Option<(PathBuf, String)>>,
        /// Manual check request id; results of superseded checks are dropped (UPD-011).
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

    fn updates_dir(inner: &HubInner) -> PathBuf {
        inner.paths.data_dir.join("updates")
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
        if let Some(by_policy) = disabled_reason(true, inner.demo, setting) {
            u.set(UpdateStatus::Disabled { by_policy });
            return;
        }
        u.set(idle_status(inner));
        let token = inner.shutdown.child_token();
        *lock(&u.schedule) = Some(token.clone());
        inner.handle.spawn(schedule(inner.clone(), token));
    }

    /// Re-evaluates the gates after a Settings change (`[updates] check`).
    pub(crate) fn settings_changed(inner: &Arc<HubInner>) {
        let Some(u) = inner.updates.as_ref() else {
            return;
        };
        let setting = lock(&inner.config).updates.check;
        let running = lock(&u.schedule).is_some();
        match disabled_reason(true, inner.demo, setting) {
            Some(by_policy) => {
                if let Some(t) = lock(&u.schedule).take() {
                    t.cancel();
                }
                u.set(UpdateStatus::Disabled { by_policy });
            }
            None if !running => start(inner),
            None => {}
        }
    }

    async fn schedule(inner: Arc<HubInner>, token: CancellationToken) {
        let cleanup_dir = updates_dir(&inner);
        dk_update::cleanup::cleanup(&cleanup_dir, None).await;
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
        if let Some(by_policy) = disabled_reason(true, inner.demo, setting) {
            // A manual check while the setting is off still runs (Settings → *Check now*), but
            // never against policy, env, or demo.
            if by_policy || !manual || disabled_reason(true, inner.demo, true).is_some() {
                return Ok(UpdateCheck::Disabled);
            }
        }
        let request = u.request.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _busy = u.busy.lock().await;
        if manual && u.request.load(std::sync::atomic::Ordering::SeqCst) != request {
            // A newer manual check superseded this one (UPD-011).
            return Err(EngineError::Cancelled);
        }
        if let Some((_, version)) = lock(&u.ready).clone() {
            return Ok(UpdateCheck::Available { version });
        }
        u.set(UpdateStatus::Checking);
        let result = check_inner(inner, u).await;
        let stale = manual && u.request.load(std::sync::atomic::Ordering::SeqCst) != request;
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
        match &result {
            Err(e) if manual && !stale => u.set(UpdateStatus::Error {
                message: e.to_string(),
            }),
            Err(_) | Ok(UpdateCheck::UpToDate) | Ok(UpdateCheck::Disabled) => {
                if !matches!(
                    *u.status.borrow(),
                    UpdateStatus::Available { .. } | UpdateStatus::Ready { .. }
                ) {
                    u.set(UpdateStatus::Idle {
                        last_check: checked_at,
                    });
                }
            }
            Ok(UpdateCheck::Available { .. }) => {}
        }
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
            return Ok(UpdateCheck::UpToDate);
        };
        let version = manifest.version.to_string();
        if !u.kind.can_install() {
            u.set(UpdateStatus::Available {
                version: version.clone(),
                notes_url: manifest.notes_url.clone(),
                notify_only: true,
            });
            return Ok(UpdateCheck::Available { version });
        }
        let path = download(inner, u, &version, &asset).await?;
        if let Some(exe) = &u.running_exe {
            dk_update::authenticode::check_installer(&path, exe).inspect_err(|_| {
                let _ = std::fs::remove_file(&path);
            })?;
        }
        *lock(&u.ready) = Some((path, version.clone()));
        u.set(UpdateStatus::Ready {
            version: version.clone(),
            notes_url: manifest.notes_url,
            needs_elevation: u.kind == InstallKind::InnoMachine,
        });
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
        let name = asset_file_name(asset)?;
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
        let Some((path, _)) = lock(&u.ready).clone() else {
            return Err(EngineError::protocol("no update is ready to install"));
        };
        dk_update::apply::spawn_installer(&path, u.kind).map_err(engine_err)
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
        assert_eq!(disabled_reason(false, false, true), Some(false));
        assert_eq!(disabled_reason(true, true, true), Some(false));
        assert_eq!(disabled_reason(true, false, false), Some(false));
    }
}
