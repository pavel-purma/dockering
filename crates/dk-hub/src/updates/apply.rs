//! *Restart to update* (UPD-011, UPD-014, UPD-007, UPD-017 (6)): the apply flag, the failure
//! classification and the platform strategy seam.
//!
//! `apply` runs on a hub blocking worker under its own [`AtomicBool`]; it never takes the check
//! lock. At the click it snapshots the payload (an [`ApplyJob`]) and marks the offer *Applying*
//! (memory only). While it runs, a check may finish but its result is **held** (see
//! `flow::commit`) and reconciled when the apply ends, so nothing replaces or deletes the payload
//! being applied. A failed apply is recorded against the applied version only.
//!
//! **One verification path per kind.** The hub does not hash the payload before the strategy
//! runs: the strategy does it, through the one call that owns the file. A Windows installer is
//! verified through the share-locked handle of `spawn_installer_locked`; an AppImage by
//! `apply_appimage` (size and SHA-256 before staging, hashed again while staged). A payload that
//! changed since the check is [`UpdateError::Integrity`] either way, and the *Installed, restart*
//! retry never hashes at all.

use std::ffi::OsString;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dk_core::{EngineError, EngineResult};
use dk_update::apply::{AppImageOutcome, LockedFallback};
use dk_update::offer::{self, OfferStage as Stored};
use dk_update::{InstallKind, PlatformAsset, UpdateError};

use super::model::{Offer, State};
use super::{ApplyOutcome, DisabledReason, OfferMode};
use crate::hub::{HubInner, lock};

/// Updates are off (policy, env, demo, no keys, no feature): no actions (UPD-017 (8)).
pub(crate) const MSG_DISABLED: &str = "updates are disabled";
/// A second *Restart to update* while one runs (the UI ignores this error silently).
pub(crate) const MSG_BUSY: &str = super::ALREADY_APPLYING;
/// No offer, or one that can't be applied (a *Download* offer, or the payload isn't ready).
pub(crate) const MSG_NOTHING: &str = "no update is ready to install";
/// The offer expired (UPD-017 (2)).
pub(crate) const MSG_EXPIRED: &str = "Check for updates first";
/// The install lost its eligibility since the check; the offer is a *Download* one now.
pub(crate) const MSG_INELIGIBLE: &str = "this installation can't update itself any more; download the new version from the release page";

/// What the strategy is asked to do: everything about the offer at the click, owned, so the
/// strategy needs no hub state. The hub has **not** verified the payload: the strategy does it
/// (see the module documentation), except for `retry_start`, which never hashes.
#[derive(Clone, Debug)]
pub(crate) struct ApplyJob {
    /// The offered version (SemVer text), the one a self-test must report.
    pub(crate) version: String,
    /// The install kind probed at this click.
    pub(crate) kind: InstallKind,
    /// The downloaded payload; `None` only for `retry_start`.
    pub(crate) payload: Option<PathBuf>,
    /// The manifest entry the payload must match (size and SHA-256).
    pub(crate) asset: PlatformAsset,
    /// The real AppImage file to replace (UPD-013), probed at this click through the host seam.
    pub(crate) target: Option<PathBuf>,
    /// The running executable, for the Windows Authenticode pin (`None`: no pin, UPD-003).
    pub(crate) running_exe: Option<PathBuf>,
    /// The *Installed, restart* stage: only start the installed file again, never stage or hash.
    pub(crate) retry_start: bool,
}

/// How a strategy succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyResult {
    /// The installer or the new process was started: [`ApplyOutcome::Restarting`].
    Handoff,
    /// The new version is installed but its process couldn't be started:
    /// [`ApplyOutcome::InstalledRestartManually`], the offer moves to *Installed, restart*.
    InstalledRestartManually,
}

/// What a failure means for the offer (UPD-017 (6), UPD-014).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// The payload changed, failed its self-test, or an installer can't be trusted or started
    /// the safe way: the offer degrades to *Download* for that version and is never retried.
    InstallFailed,
    /// Staging, permission, disk or start failed (a declined UAC prompt included): nothing
    /// changed, the offer keeps its stage and the user can retry.
    Retryable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApplyFailure {
    pub(crate) kind: FailureKind,
    pub(crate) message: String,
}

impl ApplyFailure {
    /// What `error`, from the apply of an install of `kind`, means for the offer. Every
    /// [`UpdateError`] is named: a new variant must be decided here, not fall into a default.
    ///
    /// | error | Inno (Windows) | AppImage |
    /// |---|---|---|
    /// | `Integrity` (payload changed) | degrade | degrade |
    /// | `UntrustedInstaller` | degrade | degrade |
    /// | `SelfTest` | degrade | degrade |
    /// | `Unsupported` (not replaceable / not Windows) | degrade | degrade |
    /// | `LockedStartFailed` (after the fallback was tried or refused) | degrade | degrade |
    /// | `Io` | degrade (installer missing or blocked) | keep *Ready* (payload unreadable) |
    /// | `Spawn` (UAC declined, start failed) | keep *Ready* | keep *Ready* |
    /// | `Stage` (disk full, permission, `rename`) | keep *Ready* | keep *Ready* |
    /// | `Manifest`, `Network`, `Http`, `Signature` | keep *Ready* | keep *Ready* |
    ///
    /// `LockedStartFailed` of a per-user install is retried *before* it gets here (see
    /// `SystemStrategy`); reaching this function means no retry is left.
    pub(crate) fn from_update(error: &UpdateError, kind: InstallKind) -> Self {
        let failure = match error {
            UpdateError::Integrity
            | UpdateError::UntrustedInstaller
            | UpdateError::SelfTest(_)
            | UpdateError::Unsupported
            | UpdateError::LockedStartFailed(_) => FailureKind::InstallFailed,
            UpdateError::Io(_) if kind == InstallKind::AppImage => FailureKind::Retryable,
            UpdateError::Io(_) => FailureKind::InstallFailed,
            UpdateError::Spawn(_)
            | UpdateError::Stage(_)
            | UpdateError::Manifest(_)
            | UpdateError::Network(_)
            | UpdateError::Http(_)
            | UpdateError::Signature => FailureKind::Retryable,
        };
        Self {
            kind: failure,
            message: error.to_string(),
        }
    }

    pub(crate) fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Retryable,
            message: message.into(),
        }
    }
}

/// The platform's way of applying a downloaded payload. Production is [`SystemStrategy`]; tests
/// inject a fake (a blocked apply, a cancelled installer, `InstalledRestartManually`).
/// `apply` runs on a blocking worker and may block.
pub(crate) trait ApplyStrategy: Send + Sync {
    fn apply(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure>;
}

/// The calls [`SystemStrategy`] makes into `dk-update`, as a seam: the Windows retry rule and
/// every error mapping run on every OS against a fake, while production is
/// [`DkUpdateInstallers`]. All of them block.
pub(crate) trait Installers: Send + Sync {
    /// UPD-007: verify through a share-locked handle, then start the installer.
    fn spawn_locked(
        &self,
        installer: &Path,
        kind: InstallKind,
        sha256: &str,
        size: u64,
        running_exe: Option<&Path>,
    ) -> Result<(), UpdateError>;

    /// UPD-007: the per-user retry after [`UpdateError::LockedStartFailed`].
    fn spawn_unlocked_after_verify(
        &self,
        installer: &Path,
        kind: InstallKind,
        sha256: &str,
        size: u64,
        running_exe: Option<&Path>,
    ) -> Result<(), UpdateError>;

    /// UPD-014: stage, self-test, `rename`, relaunch.
    fn apply_appimage(
        &self,
        payload: &Path,
        target: &Path,
        version: &str,
        sha256: &str,
        size: u64,
        env: &[(OsString, OsString)],
    ) -> Result<AppImageOutcome, UpdateError>;

    /// UPD-014, the *Installed, restart* stage: self-test the installed file and start it.
    fn restart_installed(
        &self,
        target: &Path,
        version: &str,
        env: &[(OsString, OsString)],
    ) -> Result<AppImageOutcome, UpdateError>;

    /// The environment the AppImage children start from: **the full environment of this
    /// process** (the scrub is `dk-update`'s job; a partial list would give the new app no
    /// `HOME`, no `DISPLAY`).
    fn env(&self) -> Vec<(OsString, OsString)>;
}

/// The real `dk-update` calls.
pub(crate) struct DkUpdateInstallers;

impl Installers for DkUpdateInstallers {
    fn spawn_locked(
        &self,
        installer: &Path,
        kind: InstallKind,
        sha256: &str,
        size: u64,
        running_exe: Option<&Path>,
    ) -> Result<(), UpdateError> {
        dk_update::apply::spawn_installer_locked(installer, kind, sha256, size, running_exe)
    }

    fn spawn_unlocked_after_verify(
        &self,
        installer: &Path,
        kind: InstallKind,
        sha256: &str,
        size: u64,
        running_exe: Option<&Path>,
    ) -> Result<(), UpdateError> {
        dk_update::apply::spawn_installer_unlocked_after_verify(
            installer,
            kind,
            sha256,
            size,
            running_exe,
        )
    }

    fn apply_appimage(
        &self,
        payload: &Path,
        target: &Path,
        version: &str,
        sha256: &str,
        size: u64,
        env: &[(OsString, OsString)],
    ) -> Result<AppImageOutcome, UpdateError> {
        dk_update::apply::apply_appimage(dk_update::apply::AppImageJob::new(
            payload, target, version, sha256, size, env,
        ))
    }

    fn restart_installed(
        &self,
        target: &Path,
        version: &str,
        env: &[(OsString, OsString)],
    ) -> Result<AppImageOutcome, UpdateError> {
        dk_update::apply::restart_installed(target, version, env)
    }

    fn env(&self) -> Vec<(OsString, OsString)> {
        dk_update::apply::current_env()
    }
}

/// The real strategies by install kind: the Windows installer (UPD-007) and the Linux AppImage
/// (UPD-014). Nothing else is ever applied in place; every other kind gets a *Download* offer.
pub(crate) struct SystemStrategy {
    installers: Arc<dyn Installers>,
}

impl SystemStrategy {
    pub(crate) fn production() -> Self {
        Self {
            installers: Arc::new(DkUpdateInstallers),
        }
    }

    #[cfg(test)]
    pub(crate) fn with(installers: Arc<dyn Installers>) -> Self {
        Self { installers }
    }

    /// One locked call that verifies through the handle and starts the installer. A start that
    /// can't be locked is retried once, unlocked but verified again, for a per-user install only;
    /// an all-users install, and a second failure, degrade the offer to *Download*.
    fn apply_inno(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure> {
        let Some(payload) = job.payload.as_deref() else {
            return Err(ApplyFailure::retryable(MSG_NOTHING));
        };
        let (sha256, size) = (job.asset.sha256.as_str(), job.asset.size);
        let running_exe = job.running_exe.as_deref();
        let started = self
            .installers
            .spawn_locked(payload, job.kind, sha256, size, running_exe);
        let started = match started {
            Err(UpdateError::LockedStartFailed(reason))
                if dk_update::apply::locked_start_fallback(job.kind)
                    == LockedFallback::RetryOnce =>
            {
                tracing::info!(%reason, "updater: the installer couldn't be started locked; retrying once");
                self.installers.spawn_unlocked_after_verify(
                    payload,
                    job.kind,
                    sha256,
                    size,
                    running_exe,
                )
            }
            other => other,
        };
        started
            .map(|()| ApplyResult::Handoff)
            .map_err(|e| ApplyFailure::from_update(&e, job.kind))
    }

    /// UPD-014: replace the AppImage in place, or in the *Installed, restart* stage only start
    /// the installed file again. `dk-update` verifies the payload itself.
    fn apply_appimage(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure> {
        // `begin` refuses an AppImage it can't locate; this is the same rule, defensively.
        let Some(target) = job.target.as_deref() else {
            return Err(ApplyFailure::from_update(
                &UpdateError::Unsupported,
                job.kind,
            ));
        };
        let env = self.installers.env();
        let outcome = if job.retry_start {
            self.installers
                .restart_installed(target, &job.version, &env)
        } else {
            let Some(payload) = job.payload.as_deref() else {
                return Err(ApplyFailure::retryable(MSG_NOTHING));
            };
            self.installers.apply_appimage(
                payload,
                target,
                &job.version,
                &job.asset.sha256,
                job.asset.size,
                &env,
            )
        };
        match outcome {
            Ok(AppImageOutcome::Restarting) => Ok(ApplyResult::Handoff),
            // The file on disk is the new one: the offer moves to *Installed, restart* and the
            // app keeps running. The reason is for the log; the UI shows a plain notice.
            Ok(AppImageOutcome::InstalledRestartManually(reason)) => {
                tracing::warn!(%reason, "updater: installed, but the new version didn't start");
                Ok(ApplyResult::InstalledRestartManually)
            }
            Err(e) => Err(ApplyFailure::from_update(&e, job.kind)),
        }
    }
}

impl ApplyStrategy for SystemStrategy {
    fn apply(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure> {
        match job.kind {
            InstallKind::InnoUser | InstallKind::InnoMachine => self.apply_inno(job),
            InstallKind::AppImage => self.apply_appimage(job),
            // A *Download* offer never reaches the strategy; refuse rather than guess.
            InstallKind::Portable | InstallKind::Other => Err(ApplyFailure::from_update(
                &UpdateError::Unsupported,
                job.kind,
            )),
        }
    }
}

/// The apply flag, released on every exit including a panic. It also undoes a half-done
/// *Applying* mark, so the offer can never stay stuck in that stage.
struct ApplyScope<'a>(&'a State);

impl<'a> ApplyScope<'a> {
    fn enter(u: &'a State) -> Option<Self> {
        u.applying
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(u))
    }
}

impl Drop for ApplyScope<'_> {
    fn drop(&mut self) {
        let u = self.0;
        {
            let mut core = lock(&u.core);
            if core.applying.take().is_some() {
                if let Some(offer) = core.offer.as_mut() {
                    offer.applying = false;
                }
                u.publish_locked(&core);
            }
        }
        u.applying.store(false, Ordering::SeqCst);
    }
}

/// Starts *Restart to update* (UPD-011). Runs on a blocking worker. Fails fast with
/// [`MSG_BUSY`] when another apply runs; refuses a missing, expired, non-install or not-ready
/// offer; then runs the strategy, which verifies the payload itself (see the module
/// documentation).
pub(crate) fn apply(inner: &Arc<HubInner>) -> EngineResult<ApplyOutcome> {
    let Some(u) = inner.updates.as_ref() else {
        return Err(EngineError::protocol(MSG_DISABLED));
    };
    if super::flow::current_disabled(inner, u).is_some_and(DisabledReason::is_hard) {
        return Err(EngineError::protocol(MSG_DISABLED));
    }
    let Some(_scope) = ApplyScope::enter(u) else {
        return Err(EngineError::protocol(MSG_BUSY));
    };
    // Eligibility is probed again here, on this blocking worker (UPD-006): the install kind, and
    // for an AppImage the real file to replace, right before the strategy runs (UPD-013).
    let kind = u.host.install_kind();
    let target = (kind == InstallKind::AppImage)
        .then(|| u.host.appimage_target())
        .flatten();
    let job = begin(u, kind, target)?;

    let result = match std::panic::catch_unwind(AssertUnwindSafe(|| u.strategy.apply(&job))) {
        Ok(result) => result,
        Err(panic) => Err(ApplyFailure::retryable(format!(
            "the update crashed: {}",
            crate::hub::panic_message(&*panic)
        ))),
    };
    finish(inner, u, &job, result)
}

/// Checks the offer, marks it *Applying* and snapshots what the strategy needs (under the gate,
/// so no commit is half-done while the click is evaluated).
fn begin(u: &State, kind: InstallKind, target: Option<PathBuf>) -> EngineResult<ApplyJob> {
    let _gate = lock(&u.gate);
    let mut core = lock(&u.core);
    let now = u.host.now();
    let usable = if kind == InstallKind::AppImage && target.is_none() {
        // An AppImage we can't locate (or can't write next to) is not replaceable.
        InstallKind::Other
    } else {
        kind
    };

    let Some(offer) = core.offer.as_mut() else {
        return Err(EngineError::protocol(MSG_NOTHING));
    };
    if !offer::is_fresh(&offer.stored, now) {
        return Err(EngineError::protocol(MSG_EXPIRED));
    }
    let was_install = offer.mode == OfferMode::Install;
    offer.set_kind(usable);
    let refused = if offer.mode != OfferMode::Install {
        // A lost eligibility downgrades the offer instead of removing it (UPD-006).
        Some(if was_install {
            MSG_INELIGIBLE
        } else {
            MSG_NOTHING
        })
    } else if !matches!(offer.stored.stage, Stored::Ready | Stored::InstalledRestart)
        || (offer.stored.stage == Stored::Ready && offer.payload.is_none())
    {
        Some(MSG_NOTHING)
    } else {
        None
    };
    if let Some(message) = refused {
        u.publish_locked(&core);
        return Err(EngineError::protocol(message));
    }

    let Some(offer) = core.offer.as_mut() else {
        return Err(EngineError::protocol(MSG_NOTHING));
    };
    offer.applying = true;
    let job = job_for(offer, usable, target, u.running_exe.clone());
    core.applying = Some(job.version.clone());
    u.publish_locked(&core);
    Ok(job)
}

fn job_for(
    offer: &Offer,
    kind: InstallKind,
    target: Option<PathBuf>,
    running_exe: Option<PathBuf>,
) -> ApplyJob {
    ApplyJob {
        version: offer.version.to_string(),
        kind,
        payload: offer.payload.clone(),
        asset: offer.asset.clone(),
        target,
        running_exe,
        retry_start: offer.stored.stage == Stored::InstalledRestart,
    }
}

/// Ends the apply: records the outcome against the applied version only (UPD-011), persists
/// what changed, and schedules the reconciliation of a check result held meanwhile.
fn finish(
    inner: &Arc<HubInner>,
    u: &State,
    job: &ApplyJob,
    result: Result<ApplyResult, ApplyFailure>,
) -> EngineResult<ApplyOutcome> {
    let _gate = lock(&u.gate);
    let (persist, held) = {
        let mut core = lock(&u.core);
        core.applying = None;
        let mut persist = false;
        if let Some(offer) = core.offer.as_mut() {
            offer.applying = false;
            // A check can't have replaced the offer meanwhile (its result is held), but the
            // version is compared anyway: a failure never lands on another offer.
            if offer.version.to_string() == job.version {
                match &result {
                    Ok(ApplyResult::Handoff) => {}
                    Ok(ApplyResult::InstalledRestartManually) => {
                        offer.stored.stage = Stored::InstalledRestart;
                        persist = true;
                    }
                    Err(failure) if failure.kind == FailureKind::InstallFailed => {
                        offer.degrade();
                        persist = true;
                        if let Some(payload) = &job.payload {
                            let _ = std::fs::remove_file(payload);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        u.publish_locked(&core);
        (persist, core.held.is_some())
    };
    if persist {
        let stored = lock(&u.core).offer.as_ref().map(|o| o.stored.clone());
        if let Some(stored) = stored
            && let Err(error) = offer::write(inner.paths.updates_dir(), &stored)
        {
            tracing::warn!(%error, "updater: couldn't save offer.json after an apply");
        }
    }
    if held {
        inner.handle.spawn(super::flow::reconcile(inner.clone()));
    }
    match result {
        Ok(ApplyResult::Handoff) => Ok(ApplyOutcome::Restarting),
        Ok(ApplyResult::InstalledRestartManually) => Ok(ApplyOutcome::InstalledRestartManually),
        Err(failure) => Err(EngineError::protocol(failure.message)),
    }
}

/// A fake [`Installers`] for tests: scripted results, every call recorded.
#[cfg(test)]
pub(crate) mod fakes {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// One call the strategy made.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Call {
        Locked {
            installer: PathBuf,
            kind: InstallKind,
            sha256: String,
            size: u64,
            running_exe: Option<PathBuf>,
        },
        Unlocked {
            installer: PathBuf,
            kind: InstallKind,
            sha256: String,
            size: u64,
            running_exe: Option<PathBuf>,
        },
        AppImage {
            payload: PathBuf,
            target: PathBuf,
            version: String,
            sha256: String,
            size: u64,
            env: Vec<(OsString, OsString)>,
        },
        Restart {
            target: PathBuf,
            version: String,
            env: Vec<(OsString, OsString)>,
        },
    }

    /// The environment the fake reports as the process environment.
    pub(crate) fn fake_env() -> Vec<(OsString, OsString)> {
        vec![
            (OsString::from("HOME"), OsString::from("/home/fake")),
            (OsString::from("DISPLAY"), OsString::from(":0")),
        ]
    }

    #[derive(Default)]
    pub(crate) struct FakeInstallers {
        locked: Mutex<VecDeque<Result<(), UpdateError>>>,
        unlocked: Mutex<VecDeque<Result<(), UpdateError>>>,
        appimage: Mutex<VecDeque<Result<AppImageOutcome, UpdateError>>>,
        restart: Mutex<VecDeque<Result<AppImageOutcome, UpdateError>>>,
        calls: Mutex<Vec<Call>>,
    }

    impl FakeInstallers {
        /// The next `spawn_locked` fails (or succeeds) like this; an empty script succeeds.
        pub(crate) fn script_locked(&self, result: Result<(), UpdateError>) {
            lock(&self.locked).push_back(result);
        }
        pub(crate) fn script_unlocked(&self, result: Result<(), UpdateError>) {
            lock(&self.unlocked).push_back(result);
        }
        pub(crate) fn script_appimage(&self, result: Result<AppImageOutcome, UpdateError>) {
            lock(&self.appimage).push_back(result);
        }
        pub(crate) fn script_restart(&self, result: Result<AppImageOutcome, UpdateError>) {
            lock(&self.restart).push_back(result);
        }
        pub(crate) fn calls(&self) -> Vec<Call> {
            lock(&self.calls).clone()
        }
    }

    impl Installers for FakeInstallers {
        fn spawn_locked(
            &self,
            installer: &Path,
            kind: InstallKind,
            sha256: &str,
            size: u64,
            running_exe: Option<&Path>,
        ) -> Result<(), UpdateError> {
            lock(&self.calls).push(Call::Locked {
                installer: installer.to_path_buf(),
                kind,
                sha256: sha256.to_owned(),
                size,
                running_exe: running_exe.map(Path::to_path_buf),
            });
            lock(&self.locked).pop_front().unwrap_or(Ok(()))
        }

        fn spawn_unlocked_after_verify(
            &self,
            installer: &Path,
            kind: InstallKind,
            sha256: &str,
            size: u64,
            running_exe: Option<&Path>,
        ) -> Result<(), UpdateError> {
            lock(&self.calls).push(Call::Unlocked {
                installer: installer.to_path_buf(),
                kind,
                sha256: sha256.to_owned(),
                size,
                running_exe: running_exe.map(Path::to_path_buf),
            });
            lock(&self.unlocked).pop_front().unwrap_or(Ok(()))
        }

        fn apply_appimage(
            &self,
            payload: &Path,
            target: &Path,
            version: &str,
            sha256: &str,
            size: u64,
            env: &[(OsString, OsString)],
        ) -> Result<AppImageOutcome, UpdateError> {
            lock(&self.calls).push(Call::AppImage {
                payload: payload.to_path_buf(),
                target: target.to_path_buf(),
                version: version.to_owned(),
                sha256: sha256.to_owned(),
                size,
                env: env.to_vec(),
            });
            lock(&self.appimage)
                .pop_front()
                .unwrap_or(Ok(AppImageOutcome::Restarting))
        }

        fn restart_installed(
            &self,
            target: &Path,
            version: &str,
            env: &[(OsString, OsString)],
        ) -> Result<AppImageOutcome, UpdateError> {
            lock(&self.calls).push(Call::Restart {
                target: target.to_path_buf(),
                version: version.to_owned(),
                env: env.to_vec(),
            });
            lock(&self.restart)
                .pop_front()
                .unwrap_or(Ok(AppImageOutcome::Restarting))
        }

        fn env(&self) -> Vec<(OsString, OsString)> {
            fake_env()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::{Call, FakeInstallers, fake_env};
    use super::*;

    const SHA: &str = "ab12";
    const ALL_KINDS: [InstallKind; 5] = [
        InstallKind::InnoUser,
        InstallKind::InnoMachine,
        InstallKind::AppImage,
        InstallKind::Portable,
        InstallKind::Other,
    ];

    fn asset() -> PlatformAsset {
        PlatformAsset {
            kind: dk_update::AssetKind::Inno,
            url: "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-x64.exe"
                .to_owned(),
            sha256: SHA.to_owned(),
            size: 4242,
        }
    }

    fn job(kind: InstallKind) -> ApplyJob {
        ApplyJob {
            version: "0.2.0".to_owned(),
            kind,
            payload: Some(PathBuf::from("/updates/0.2.0/payload")),
            asset: asset(),
            target: (kind == InstallKind::AppImage)
                .then(|| PathBuf::from("/opt/Dockering.AppImage")),
            running_exe: Some(PathBuf::from("/app/dockering")),
            retry_start: false,
        }
    }

    fn strategy() -> (SystemStrategy, Arc<FakeInstallers>) {
        let fake = Arc::new(FakeInstallers::default());
        (SystemStrategy::with(fake.clone()), fake)
    }

    fn failure(kind: FailureKind, message: &str) -> Result<ApplyResult, ApplyFailure> {
        Err(ApplyFailure {
            kind,
            message: message.to_owned(),
        })
    }

    /// Every `UpdateError` variant, once.
    fn every_error() -> Vec<UpdateError> {
        vec![
            UpdateError::Network("n".into()),
            UpdateError::Http(500),
            UpdateError::Signature,
            UpdateError::Integrity,
            UpdateError::UntrustedInstaller,
            UpdateError::Io("i".into()),
            UpdateError::Spawn("s".into()),
            UpdateError::LockedStartFailed("l".into()),
            UpdateError::Unsupported,
            UpdateError::Stage("st".into()),
            UpdateError::SelfTest("t".into()),
            UpdateError::Manifest("m".into()),
        ]
    }

    /// What the table in `from_update` says, written out once more, independently.
    fn expected(error: &UpdateError, kind: InstallKind) -> FailureKind {
        use FailureKind::{InstallFailed, Retryable};
        let appimage = kind == InstallKind::AppImage;
        match error {
            UpdateError::Integrity
            | UpdateError::UntrustedInstaller
            | UpdateError::SelfTest(_)
            | UpdateError::Unsupported
            | UpdateError::LockedStartFailed(_) => InstallFailed,
            UpdateError::Io(_) if appimage => Retryable,
            UpdateError::Io(_) => InstallFailed,
            UpdateError::Spawn(_)
            | UpdateError::Stage(_)
            | UpdateError::Manifest(_)
            | UpdateError::Network(_)
            | UpdateError::Http(_)
            | UpdateError::Signature => Retryable,
        }
    }

    #[test]
    fn upd_007_locked_start_failed_mapping() {
        // Every variant, for every install kind: the table, and the message is the error's text.
        for kind in ALL_KINDS {
            for error in every_error() {
                let mapped = ApplyFailure::from_update(&error, kind);
                assert_eq!(mapped.kind, expected(&error, kind), "{error:?} on {kind:?}");
                assert_eq!(mapped.message, error.to_string());
            }
        }
        // The cases the plan spells out.
        let locked = UpdateError::LockedStartFailed("sharing violation".into());
        for kind in [InstallKind::InnoUser, InstallKind::InnoMachine] {
            assert_eq!(
                ApplyFailure::from_update(&locked, kind).kind,
                FailureKind::InstallFailed,
                "a start that can't be locked is never a plain retry ({kind:?})"
            );
        }
        for degrade in [
            UpdateError::UntrustedInstaller,
            UpdateError::Integrity,
            UpdateError::Io("gone".into()),
        ] {
            assert_eq!(
                ApplyFailure::from_update(&degrade, InstallKind::InnoUser).kind,
                FailureKind::InstallFailed
            );
        }
        // UAC declined or a failed start: nothing changed, the user can retry.
        assert_eq!(
            ApplyFailure::from_update(
                &UpdateError::Spawn("declined".into()),
                InstallKind::InnoMachine
            )
            .kind,
            FailureKind::Retryable
        );
        // AppImage: a staging error keeps the offer, a failed self-test degrades it.
        assert_eq!(
            ApplyFailure::from_update(&UpdateError::Stage("full".into()), InstallKind::AppImage)
                .kind,
            FailureKind::Retryable
        );
        assert_eq!(
            ApplyFailure::from_update(&UpdateError::SelfTest("x".into()), InstallKind::AppImage)
                .kind,
            FailureKind::InstallFailed
        );
    }

    #[test]
    fn upd_007_inno_apply_is_one_locked_call_that_verifies_and_pins() {
        for kind in [InstallKind::InnoUser, InstallKind::InnoMachine] {
            let (strategy, fake) = strategy();
            assert_eq!(
                strategy.apply(&job(kind)),
                Ok(ApplyResult::Handoff),
                "{kind:?}"
            );
            // ONE call, with the manifest's size and hash and the running exe for the pin: the
            // verification happens through the handle there, the hub does none of its own.
            assert_eq!(
                fake.calls(),
                [Call::Locked {
                    installer: PathBuf::from("/updates/0.2.0/payload"),
                    kind,
                    sha256: SHA.to_owned(),
                    size: 4242,
                    running_exe: Some(PathBuf::from("/app/dockering")),
                }]
            );
        }
        // An unsigned running exe passes no pin.
        let (strategy, fake) = strategy();
        let mut unsigned = job(InstallKind::InnoUser);
        unsigned.running_exe = None;
        assert_eq!(strategy.apply(&unsigned), Ok(ApplyResult::Handoff));
        assert!(matches!(
            &fake.calls()[0],
            Call::Locked {
                running_exe: None,
                ..
            }
        ));
    }

    #[test]
    fn upd_007_per_user_locked_start_failure_retries_once_then_degrades() {
        let locked = || UpdateError::LockedStartFailed("sharing violation".into());
        // The retry works: a handoff, one locked and one unlocked call, same arguments.
        let (strategy, fake) = strategy();
        fake.script_locked(Err(locked()));
        assert_eq!(
            strategy.apply(&job(InstallKind::InnoUser)),
            Ok(ApplyResult::Handoff)
        );
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        let (
            Call::Locked {
                installer: a,
                sha256: sa,
                size: za,
                running_exe: ra,
                ..
            },
            Call::Unlocked {
                installer: b,
                sha256: sb,
                size: zb,
                running_exe: rb,
                kind,
                ..
            },
        ) = (&calls[0], &calls[1])
        else {
            panic!("a locked call, then an unlocked one: {calls:?}");
        };
        assert_eq!((a, sa, za, ra), (b, sb, zb, rb));
        assert_eq!(*kind, InstallKind::InnoUser);

        // The retry fails to lock too: *Download*, and the retry isn't repeated.
        let (strategy, fake) = self::strategy();
        fake.script_locked(Err(locked()));
        fake.script_unlocked(Err(locked()));
        let result = strategy.apply(&job(InstallKind::InnoUser));
        assert_eq!(
            result,
            failure(FailureKind::InstallFailed, &locked().to_string())
        );
        assert_eq!(fake.calls().len(), 2);

        // The retry fails for another reason (the file changed meanwhile): that reason decides.
        let (strategy, fake) = self::strategy();
        fake.script_locked(Err(locked()));
        fake.script_unlocked(Err(UpdateError::Integrity));
        assert_eq!(
            strategy.apply(&job(InstallKind::InnoUser)),
            failure(
                FailureKind::InstallFailed,
                &UpdateError::Integrity.to_string()
            )
        );
        let (strategy, fake) = self::strategy();
        fake.script_locked(Err(locked()));
        fake.script_unlocked(Err(UpdateError::Spawn("UAC declined".into())));
        assert_eq!(
            strategy.apply(&job(InstallKind::InnoUser)),
            failure(
                FailureKind::Retryable,
                &UpdateError::Spawn("UAC declined".into()).to_string()
            ),
            "a declined start keeps the offer"
        );

        // All-users: never elevate a file that isn't held. No retry at all.
        let (strategy, fake) = self::strategy();
        fake.script_locked(Err(locked()));
        assert_eq!(
            strategy.apply(&job(InstallKind::InnoMachine)),
            failure(FailureKind::InstallFailed, &locked().to_string())
        );
        assert_eq!(
            fake.calls().len(),
            1,
            "no unlocked start for an all-users install"
        );
        assert!(matches!(fake.calls()[0], Call::Locked { .. }));
    }

    #[test]
    fn upd_007_inno_errors_map_per_the_plan() {
        for (error, kind) in [
            (UpdateError::UntrustedInstaller, FailureKind::InstallFailed),
            (UpdateError::Integrity, FailureKind::InstallFailed),
            (
                UpdateError::Io("missing".into()),
                FailureKind::InstallFailed,
            ),
            (
                UpdateError::Spawn("declined".into()),
                FailureKind::Retryable,
            ),
        ] {
            for install in [InstallKind::InnoUser, InstallKind::InnoMachine] {
                let (strategy, fake) = strategy();
                fake.script_locked(Err(error.clone()));
                assert_eq!(
                    strategy.apply(&job(install)),
                    failure(kind, &error.to_string()),
                    "{error:?} on {install:?}"
                );
                assert_eq!(fake.calls().len(), 1, "only a locked-start failure retries");
            }
        }
    }

    #[test]
    fn upd_014_appimage_apply_passes_the_full_process_env_and_maps_the_outcome() {
        // Restarting is a handoff.
        let (strategy, fake) = strategy();
        assert_eq!(
            strategy.apply(&job(InstallKind::AppImage)),
            Ok(ApplyResult::Handoff)
        );
        assert_eq!(
            fake.calls(),
            [Call::AppImage {
                payload: PathBuf::from("/updates/0.2.0/payload"),
                target: PathBuf::from("/opt/Dockering.AppImage"),
                version: "0.2.0".to_owned(),
                sha256: SHA.to_owned(),
                size: 4242,
                env: fake_env(),
            }]
        );
        // Installed, but the new process didn't start: the app keeps running.
        let (strategy, fake) = self::strategy();
        fake.script_appimage(Ok(AppImageOutcome::InstalledRestartManually(
            "the new process died".into(),
        )));
        assert_eq!(
            strategy.apply(&job(InstallKind::AppImage)),
            Ok(ApplyResult::InstalledRestartManually)
        );
    }

    #[test]
    fn upd_014_appimage_errors_map_per_the_plan() {
        for (error, kind) in [
            (
                UpdateError::SelfTest("wrong version".into()),
                FailureKind::InstallFailed,
            ),
            (UpdateError::Integrity, FailureKind::InstallFailed),
            (UpdateError::Unsupported, FailureKind::InstallFailed),
            (
                UpdateError::Stage("disk full".into()),
                FailureKind::Retryable,
            ),
            (
                UpdateError::Io("unreadable payload".into()),
                FailureKind::Retryable,
            ),
            (
                UpdateError::Manifest("not semver".into()),
                FailureKind::Retryable,
            ),
        ] {
            let (strategy, fake) = strategy();
            fake.script_appimage(Err(error.clone()));
            assert_eq!(
                strategy.apply(&job(InstallKind::AppImage)),
                failure(kind, &error.to_string()),
                "{error:?}"
            );
        }
    }

    #[test]
    fn upd_014_installed_restart_retry_only_restarts() {
        let (strategy, fake) = strategy();
        let mut retry = job(InstallKind::AppImage);
        retry.retry_start = true;
        retry.payload = None;
        assert_eq!(strategy.apply(&retry), Ok(ApplyResult::Handoff));
        // `restart_installed` and nothing else: no staging, no hashing, no payload needed.
        assert_eq!(
            fake.calls(),
            [Call::Restart {
                target: PathBuf::from("/opt/Dockering.AppImage"),
                version: "0.2.0".to_owned(),
                env: fake_env(),
            }]
        );
        // It can fail again the same way.
        let (strategy, fake) = self::strategy();
        fake.script_restart(Ok(AppImageOutcome::InstalledRestartManually(
            "still no".into(),
        )));
        assert_eq!(
            strategy.apply(&retry),
            Ok(ApplyResult::InstalledRestartManually)
        );
        // And an installed file that no longer answers `--version` is a failed install.
        let (strategy, fake) = self::strategy();
        fake.script_restart(Err(UpdateError::SelfTest("no output".into())));
        assert_eq!(
            strategy.apply(&retry),
            failure(
                FailureKind::InstallFailed,
                &UpdateError::SelfTest("no output".into()).to_string()
            )
        );
    }

    #[test]
    fn upd_014_appimage_without_a_target_or_other_kinds_are_not_applied() {
        let (strategy, fake) = strategy();
        let mut lost = job(InstallKind::AppImage);
        lost.target = None;
        assert_eq!(
            strategy.apply(&lost).map_err(|f| f.kind),
            Err(FailureKind::InstallFailed)
        );
        for kind in [InstallKind::Portable, InstallKind::Other] {
            assert_eq!(
                strategy.apply(&job(kind)).map_err(|f| f.kind),
                Err(FailureKind::InstallFailed),
                "{kind:?}"
            );
        }
        assert!(fake.calls().is_empty(), "nothing was started");
    }

    #[test]
    fn upd_014_production_installers_report_the_full_process_environment() {
        assert_eq!(DkUpdateInstallers.env(), dk_update::apply::current_env());
        assert_eq!(
            DkUpdateInstallers.env().len(),
            std::env::vars_os().count(),
            "not a hand-picked list"
        );
    }
}
