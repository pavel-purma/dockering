//! *Restart to update* (UPD-011, UPD-014, UPD-017 (6)): the apply flag, the failure
//! classification and the platform strategy seam.
//!
//! `apply` runs on a hub blocking worker under its own [`AtomicBool`]; it never takes the check
//! lock. At the click it snapshots the payload (an [`ApplyJob`]) and marks the offer *Applying*
//! (memory only). While it runs, a check may finish but its result is **held** (see
//! `flow::commit`) and reconciled when the apply ends, so nothing replaces or deletes the payload
//! being applied. A failed apply is recorded against the applied version only.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dk_core::{EngineError, EngineResult};
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
/// strategy needs no hub state. The hub has already verified the payload (size, SHA-256 and the
/// Authenticode pin) unless `retry_start` is set.
#[derive(Clone, Debug)]
pub(crate) struct ApplyJob {
    /// The offered version (SemVer text), the one a self-test must report.
    pub(crate) version: String,
    /// The install kind probed at this click.
    pub(crate) kind: InstallKind,
    /// The verified payload; `None` only for `retry_start`.
    pub(crate) payload: Option<PathBuf>,
    /// The manifest entry the payload was verified against.
    pub(crate) asset: PlatformAsset,
    /// The real AppImage file to replace (UPD-013), probed at this click. Read by the AppImage
    /// strategy, which is wired in the AppImage apply task.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) target: Option<PathBuf>,
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
    /// Produced by the AppImage strategy (wired in the AppImage apply task) and by test fakes.
    #[cfg_attr(not(test), allow(dead_code))]
    InstalledRestartManually,
}

/// What a failure means for the offer (UPD-017 (6), UPD-014).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// The payload changed, failed its self-test, or an installer can't be trusted: the offer
    /// degrades to *Download* for that version and is never retried.
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
    /// A strategy's error: an untrusted installer, a changed payload or a failed self-test mean
    /// the install is broken for this version; everything else may work next time.
    pub(crate) fn from_update(error: &UpdateError) -> Self {
        let kind = match error {
            UpdateError::UntrustedInstaller | UpdateError::Integrity | UpdateError::SelfTest(_) => {
                FailureKind::InstallFailed
            }
            _ => FailureKind::Retryable,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }

    /// An error from verifying the payload at the click: besides a changed payload, a missing or
    /// unreadable file (the installer was removed or blocked) also ends this version's install.
    fn from_verify(error: &UpdateError) -> Self {
        let mut failure = Self::from_update(error);
        if matches!(error, UpdateError::Io(_)) {
            failure.kind = FailureKind::InstallFailed;
        }
        failure
    }

    pub(crate) fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Retryable,
            message: message.into(),
        }
    }
}

/// The platform's way of applying a verified payload. Production is [`SystemStrategy`]; tests
/// inject a fake (a blocked apply, a cancelled installer, `InstalledRestartManually`).
/// `apply` runs on a blocking worker and may block.
pub(crate) trait ApplyStrategy: Send + Sync {
    fn apply(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure>;
}

/// The real strategies by install kind.
pub(crate) struct SystemStrategy;

impl ApplyStrategy for SystemStrategy {
    fn apply(&self, job: &ApplyJob) -> Result<ApplyResult, ApplyFailure> {
        match job.kind {
            InstallKind::InnoUser | InstallKind::InnoMachine => {
                let Some(payload) = job.payload.as_deref() else {
                    return Err(ApplyFailure::from_update(&UpdateError::Unsupported));
                };
                dk_update::apply::spawn_installer(payload, job.kind)
                    .map(|()| ApplyResult::Handoff)
                    .map_err(|e| ApplyFailure::from_update(&e))
            }
            // The Linux AppImage strategy is wired in the AppImage apply task: it calls
            // dk_update's AppImage apply (and its *Installed, restart* retry, `job.retry_start`)
            // for `job.target` from this arm and maps the outcome to an `ApplyResult`.
            InstallKind::AppImage | InstallKind::Portable | InstallKind::Other => {
                Err(ApplyFailure::from_update(&UpdateError::Unsupported))
            }
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
/// offer; re-verifies the payload (size, SHA-256, the Authenticode pin); then runs the strategy.
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
    // Eligibility is probed again here, on this blocking worker (UPD-006).
    let kind = u.host.install_kind();
    let target = (kind == InstallKind::AppImage)
        .then(|| u.host.appimage_target())
        .flatten();
    let job = begin(u, kind, target)?;

    let verified = if job.retry_start {
        Ok(())
    } else {
        verify(u, &job)
    };
    let result = match verified {
        Err(failure) => Err(failure),
        Ok(()) => match std::panic::catch_unwind(AssertUnwindSafe(|| u.strategy.apply(&job))) {
            Ok(result) => result,
            Err(panic) => Err(ApplyFailure::retryable(format!(
                "the update crashed: {}",
                crate::hub::panic_message(&*panic)
            ))),
        },
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
    let job = job_for(offer, usable, target);
    core.applying = Some(job.version.clone());
    u.publish_locked(&core);
    Ok(job)
}

fn job_for(offer: &Offer, kind: InstallKind, target: Option<PathBuf>) -> ApplyJob {
    ApplyJob {
        version: offer.version.to_string(),
        kind,
        payload: offer.payload.clone(),
        asset: offer.asset.clone(),
        target,
        retry_start: offer.stored.stage == Stored::InstalledRestart,
    }
}

/// Size and SHA-256 of the payload, then the Authenticode pin when the running exe is checked
/// (the file sits in a user-writable directory and may have been verified days ago, UPD-003).
fn verify(u: &State, job: &ApplyJob) -> Result<(), ApplyFailure> {
    let Some(payload) = job.payload.as_deref() else {
        return Err(ApplyFailure::retryable(MSG_NOTHING));
    };
    futures::executor::block_on(dk_update::verify::verify_file(
        payload,
        &job.asset.sha256,
        job.asset.size,
    ))
    .map_err(|e| ApplyFailure::from_verify(&e))?;
    if let Some(exe) = &u.running_exe {
        u.host
            .check_installer(payload, exe)
            .map_err(|e| ApplyFailure::from_verify(&e))?;
    }
    Ok(())
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
