//! The sequence of the AppImage apply (UPD-014) and of the *Installed, restart* retry.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use super::child::ChildEnv;
use super::seams::Seams;
use super::stage::{StagedFile, open_verified_payload, stage, staged_mode};
use super::{AppImageJob, AppImageOutcome, relaunch, selftest};
use crate::UpdateError;

/// The time limits of one run.
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    pub self_test_timeout: Duration,
    pub relaunch_grace: Duration,
}

/// UPD-014 steps 1 to 5. See [`super::apply_appimage`] for the result mapping.
pub(super) fn apply(
    job: &AppImageJob<'_>,
    seams: &Seams<'_>,
) -> Result<AppImageOutcome, UpdateError> {
    let limits = Limits {
        self_test_timeout: job.self_test_timeout,
        relaunch_grace: job.relaunch_grace,
    };
    check_version(job.expected_version)?;

    // 1. The eligibility (UPD-013) and the payload, again.
    let target_mode = check_file(job.target, seams)?;
    let dir = job
        .target
        .parent()
        .filter(|dir| seams.probe.dir_writable(dir))
        .ok_or(UpdateError::Unsupported)?;
    let mut payload = open_verified_payload(job)?;

    // 2. Stage in the target's directory (removed again on any early return).
    let staged = stage(&mut payload, job, dir, staged_mode(target_mode), seams.sink)?;
    drop(payload);

    // 3. The self-test, in the scrubbed environment.
    let env = ChildEnv::new(job.env);
    selftest::self_test(
        staged.path(),
        job.expected_version,
        &env,
        limits.self_test_timeout,
        seams,
    )?;

    // 4. Replace the target. A failure here still removes the staged file and keeps the offer.
    swap(staged, job.target)?;
    // From here on the file on disk is the new one: nothing below may return `Err`.
    if let Err(error) = seams.fsync.sync_dir(dir) {
        return Ok(AppImageOutcome::InstalledRestartManually(format!(
            "the update was installed, but the folder couldn't be flushed: {error}"
        )));
    }

    // 5. Start it.
    Ok(started(relaunch::start(
        job.target,
        &env,
        limits.relaunch_grace,
        seams,
    )))
}

/// The *Installed, restart* retry: self-test the installed file and start it; no staging, no
/// hashing, no `rename`.
pub(super) fn restart(
    target: &Path,
    expected_version: &str,
    env: &[(OsString, OsString)],
    limits: Limits,
    seams: &Seams<'_>,
) -> Result<AppImageOutcome, UpdateError> {
    check_version(expected_version)?;
    check_file(target, seams)?;
    let env = ChildEnv::new(env);
    selftest::self_test(
        target,
        expected_version,
        &env,
        limits.self_test_timeout,
        seams,
    )?;
    Ok(started(relaunch::start(
        target,
        &env,
        limits.relaunch_grace,
        seams,
    )))
}

fn started(result: Result<(), String>) -> AppImageOutcome {
    match result {
        Ok(()) => AppImageOutcome::Restarting,
        Err(reason) => AppImageOutcome::InstalledRestartManually(reason),
    }
}

/// The self-test compares text, so the manifest version must be a semantic version: the format
/// `dockering <semver>` is what the new binary prints.
fn check_version(version: &str) -> Result<(), UpdateError> {
    semver::Version::parse(version).map(drop).map_err(|error| {
        UpdateError::Manifest(format!("`{version}` is not a semantic version: {error}"))
    })
}

/// The target must still be an absolute path to a regular file (not a link) with an AppImage
/// header. Returns its permission bits. Anything else means the eligibility is lost
/// ([`UpdateError::Unsupported`]).
fn check_file(target: &Path, seams: &Seams<'_>) -> Result<u32, UpdateError> {
    if !target.is_absolute() {
        return Err(UpdateError::Unsupported);
    }
    // `symlink_metadata`: the real file is what gets replaced, never a link to it.
    let meta = fs::symlink_metadata(target).map_err(|_| UpdateError::Unsupported)?;
    if !meta.is_file() || !seams.probe.is_appimage_file(target) {
        return Err(UpdateError::Unsupported);
    }
    Ok(meta.permissions().mode())
}

/// `rename` over the target; the staged file is only forgotten when it worked.
fn swap(staged: StagedFile, target: &Path) -> Result<(), UpdateError> {
    match fs::rename(staged.path(), target) {
        Ok(()) => {
            staged.commit();
            Ok(())
        }
        Err(error) => Err(UpdateError::Stage(format!(
            "couldn't replace the app with the new version: {error}"
        ))),
    }
}
