//! In-app updates (UPD-001…017, spec `features/distribution.md` §7): the hub-side coordinator.
//! It checks the latest GitHub Release on a schedule, keeps the retained offer (UPD-017),
//! downloads and verifies the payload of installs that can update themselves, and applies it on
//! request.
//!
//! The DTOs below are always compiled, so the UI builds the same either way. The network side
//! (`dk-update`) only exists with the `updater` feature; without it the snapshot is
//! `Disabled { reason: Unavailable }` with no offer and nothing touches the network (UPD-005).
//!
//! Layout of the feature-gated part:
//!
//! - `model`: the state, the retained offer, the pure snapshot builder and the host seam;
//! - `flow`: start (restore before any network), schedule, check, download, commit, cleanup;
//! - `apply`: the apply flag, the failure classification and the platform strategy seam.

use serde::{Deserialize, Serialize};

/// What the updater is *doing* (UPD-008). The retained offer is separate
/// ([`UpdateSnapshot::offer`]), so checking or a failed check never hides it (UPD-017).
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
    /// A payload is being downloaded.
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// The last check or download failed. Only manual checks surface this (UPD-004).
    Error {
        message: String,
    },
}

/// What *Restart to update* did (UPD-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The installer or the new process was started: the UI saves its state and quits.
    Restarting,
    /// An AppImage was replaced but the new process couldn't be started; the app keeps running
    /// and the offer is in its *Installed, restart* stage.
    InstalledRestartManually,
}

/// What the offer's button does (UPD-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferMode {
    /// *Restart to update*: this installation can install the update itself.
    Install,
    /// *Download*: open the release page. Nothing is downloaded and no install is ever reported.
    Download,
}

/// Where the offer is in its life (UPD-017). `Applying` exists in memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStage {
    /// The install payload is being fetched or verified: no button yet.
    Downloading,
    /// Actionable: the verified payload is ready (install) or the offer is a *Download* one.
    Ready,
    /// *Restart to update* is running (memory only; a second click is ignored).
    Applying,
    /// The new version is installed but the new process didn't start; a click retries the start.
    InstalledRestart,
}

/// The retained update offer as the UI sees it (UPD-015, UPD-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOffer {
    /// The offered version (SemVer text).
    pub version: String,
    /// The release notes page of that version.
    pub notes_url: String,
    pub mode: OfferMode,
    pub stage: OfferStage,
    /// An all-users Windows install: a UAC prompt follows *Restart to update*.
    pub needs_elevation: bool,
    /// Installing this version failed: it stays a *Download* offer until a newer one arrives.
    pub install_failed: bool,
}

/// Activity plus the retained offer, replayed first by `update_status()` (UPD-008, UPD-011).
/// `revision` increases on every change, so a UI can drop a stale snapshot (NFR-005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSnapshot {
    pub status: UpdateStatus,
    pub offer: Option<UpdateOffer>,
    pub revision: u64,
}

impl UpdateSnapshot {
    /// The snapshot of a build or a state with no updater: disabled, no offer.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            status: UpdateStatus::Disabled {
                reason: DisabledReason::Unavailable,
            },
            offer: None,
            revision: 0,
        }
    }
}

/// Why updates are off (UPD-005). Settings shows a different note for each (UPD-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    /// An administrator policy (`DisableUpdates`): everything off, *Check now* hidden.
    Policy,
    /// The user turned automatic checks off; *Check now* and the retained offer still work.
    Setting,
    /// Not in this build (no `updater` feature), `DOCKERING_DISABLE_UPDATES`, demo mode, or no
    /// trust keys.
    Unavailable,
}

impl DisabledReason {
    /// Policy, env, demo, no feature and no keys suppress every action and the button but keep
    /// the retained file (UPD-017 (8)); only the setting leaves explicit actions alone.
    #[must_use]
    pub fn is_hard(self) -> bool {
        !matches!(self, Self::Setting)
    }
}

/// The `Protocol` error text of a second `apply_update()` while one runs (UPD-011); the UI
/// ignores that error silently.
pub const ALREADY_APPLYING: &str = "already being applied";

/// Result of a manual `check_for_updates()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available { version: String },
    Disabled,
}

/// Persisted updater state (`state.json` → `updates`, UPD-012). The retained offer lives in
/// `<data-local>/updates/offer.json`, not here.
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
mod apply;
#[cfg(feature = "updater")]
mod flow;
#[cfg(feature = "updater")]
mod model;

#[cfg(all(feature = "updater", test))]
pub(crate) use apply::fakes;
#[cfg(feature = "updater")]
#[allow(unused_imports)]
pub(crate) use apply::{
    ApplyFailure, ApplyJob, ApplyResult, ApplyStrategy, FailureKind, Installers, SystemStrategy,
    apply as apply_update,
};
#[cfg(feature = "updater")]
pub(crate) use flow::{check, settings_changed, start};
#[cfg(feature = "updater")]
#[allow(unused_imports)]
pub(crate) use model::{
    EnvFn, FIRST_CHECK, Host, INTERVAL, JITTER_SECS, State, UpdaterInit, next_delay,
};

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

    #[test]
    fn upd_005_only_the_setting_is_a_soft_disable() {
        assert!(DisabledReason::Policy.is_hard());
        assert!(DisabledReason::Unavailable.is_hard());
        assert!(!DisabledReason::Setting.is_hard());
    }

    #[test]
    fn upd_011_unavailable_snapshot_has_no_offer() {
        let s = UpdateSnapshot::unavailable();
        assert_eq!(
            s.status,
            UpdateStatus::Disabled {
                reason: DisabledReason::Unavailable
            }
        );
        assert_eq!((s.offer, s.revision), (None, 0));
    }

    /// UPD-012: `state.json` keeps only four fields; a file written by an older build that
    /// still has `pending_version` loads (the field is ignored), and it isn't written back.
    #[test]
    fn upd_012_old_state_json_with_pending_version_loads() {
        let old = r#"{
            "last_check": "2026-10-03T08:15:42Z",
            "last_result": "up to date",
            "last_run_version": "0.3.0",
            "notified_version": "0.3.0",
            "pending_version": "0.4.0"
        }"#;
        let state: UpdateState = serde_json::from_str(old).expect("old file loads");
        assert_eq!(state.last_check.as_deref(), Some("2026-10-03T08:15:42Z"));
        assert_eq!(state.last_run_version.as_deref(), Some("0.3.0"));
        assert_eq!(state.notified_version.as_deref(), Some("0.3.0"));
        let written = serde_json::to_value(&state).expect("serialise");
        let keys: std::collections::BTreeSet<&str> = written
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "last_check",
                "last_result",
                "last_run_version",
                "notified_version"
            ])
        );
    }
}
