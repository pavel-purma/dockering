//! Replacing a Linux AppImage in place (UPD-014).
//!
//! The hub calls [`apply_appimage`] (or, in the *Installed, restart* stage, [`restart_installed`])
//! on a blocking worker, under its apply flag. Steps of [`apply_appimage`]:
//!
//! 1. the payload's size and SHA-256 and the target's eligibility are checked again;
//! 2. a copy is staged **in the target's directory** as `.dockering-update-<n>` (created
//!    exclusively, the target's permission bits plus execute, hashed while written, flushed with
//!    `fsync`, closed);
//! 3. the staged file runs `--version` in the [scrubbed environment](scrubbed_env) and must print
//!    `dockering <version>` and exit with status 0 (stdin closed, 20 s limit, `ETXTBSY` retried
//!    five times 100 ms apart);
//! 4. it is renamed over the real target and the directory is flushed;
//! 5. the new file is started detached with [`RELAUNCH_FLAG`] and a piped stdin whose write end
//!    lives in a process-lifetime `static`, so end-of-file reaches the child only when the kernel
//!    closes the old process (`dk_hub::single_instance::wait_for_predecessor`, UPD-017).
//!
//! Everything before the `rename` removes the staged file and leaves the target untouched. After
//! it, every failure is `Ok(`[`AppImageOutcome::InstalledRestartManually`]`)`: the file on disk
//! is the new one. Only Linux implements this; other systems answer
//! [`UpdateError::Unsupported`].

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::UpdateError;

#[cfg(target_os = "linux")]
mod child;
#[cfg(target_os = "linux")]
mod flow;
#[cfg(target_os = "linux")]
mod relaunch;
#[cfg(target_os = "linux")]
mod seams;
#[cfg(target_os = "linux")]
mod selftest;
#[cfg(target_os = "linux")]
mod stage;
#[cfg(all(test, target_os = "linux"))]
mod tests;

/// The argument the new AppImage is started with (UPD-014 step 5, UPD-017). **Frozen** from the
/// first updater release: an old binary starts a new one with it. The same literal is
/// `dk_hub::single_instance::RELAUNCH_FLAG`; `dk-update` doesn't depend on `dk-hub`, and a test
/// pins the literal here.
pub const RELAUNCH_FLAG: &str = "--update-relaunch=1";

/// The `--version` self-test limit (UPD-014 step 3).
pub const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a started new process must stay alive to count as started (UPD-014 step 5).
pub const RELAUNCH_GRACE: Duration = Duration::from_secs(1);

/// Everything [`apply_appimage`] needs. Build it with [`AppImageJob::new`]; the optional limits
/// are public fields with the production defaults.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AppImageJob<'a> {
    /// The downloaded, verified AppImage (`<data-local>/updates/<version>/<asset>`).
    pub payload: &'a Path,
    /// The real AppImage file to replace: the result of `install_kind::current_appimage_path()`
    /// (absolute, symlinks resolved), derived right before the call.
    pub target: &'a Path,
    /// The manifest version; the self-test requires the output `dockering <version>`.
    pub expected_version: &'a str,
    /// The manifest SHA-256 of the payload, in hex.
    pub expected_sha256: &'a str,
    /// The manifest size of the payload in bytes.
    pub expected_size: u64,
    /// The environment of this process, normally [`current_env`](super::current_env). The child
    /// processes get exactly this environment minus the [scrub](scrubbed_env): they do not
    /// inherit from the running process, so a test (or a caller) controls what they see. An
    /// incomplete list means an incomplete child environment (no `HOME`, no `DISPLAY`).
    pub env: &'a [(OsString, OsString)],
    /// The `--version` limit; [`SELF_TEST_TIMEOUT`] by default.
    pub self_test_timeout: Duration,
    /// How long the started process must survive; [`RELAUNCH_GRACE`] by default.
    pub relaunch_grace: Duration,
}

impl<'a> AppImageJob<'a> {
    /// A job with the production limits.
    #[must_use]
    pub fn new(
        payload: &'a Path,
        target: &'a Path,
        expected_version: &'a str,
        expected_sha256: &'a str,
        expected_size: u64,
        env: &'a [(OsString, OsString)],
    ) -> Self {
        Self {
            payload,
            target,
            expected_version,
            expected_sha256,
            expected_size,
            env,
            self_test_timeout: SELF_TEST_TIMEOUT,
            relaunch_grace: RELAUNCH_GRACE,
        }
    }
}

/// How an AppImage apply ended without an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppImageOutcome {
    /// The new file is in place and its process runs; the caller saves its state and quits
    /// (`ApplyOutcome::Restarting`).
    Restarting,
    /// The new file is in place, but a later step failed (the directory flush, the start of the
    /// new process, or its immediate exit). The old process keeps running and the offer moves
    /// to *Installed, restart*; the reason is for the log and the notification
    /// (`ApplyOutcome::InstalledRestartManually`).
    InstalledRestartManually(String),
}

/// UPD-014, steps 1 to 5 (see the [module documentation](self)): checks, stages, self-tests,
/// replaces `job.target` and starts the new file.
///
/// Blocking: call it on a blocking worker, never on the UI thread. `Err` always means the target
/// is byte-identical to before and no staging file is left behind:
///
/// * [`UpdateError::Unsupported`]: the target is no longer a replaceable AppImage (not absolute,
///   not a regular file, no AppImage header, directory not writable): degrade to *Download*;
/// * [`UpdateError::Integrity`]: the payload no longer matches its size or SHA-256 (also when it
///   changed while being staged): degrade to *Download* for that version;
/// * [`UpdateError::SelfTest`]: the staged file did not pass `--version`: degrade to *Download*;
/// * [`UpdateError::Stage`]: disk full, permission denied or `rename` failed: stay *Ready*;
/// * [`UpdateError::Io`]: the payload could not be read; [`UpdateError::Manifest`]: the version
///   isn't a semantic version.
///
/// `Ok` means the file was replaced; see [`AppImageOutcome`].
pub fn apply_appimage(job: AppImageJob<'_>) -> Result<AppImageOutcome, UpdateError> {
    #[cfg(target_os = "linux")]
    {
        flow::apply(&job, &seams::PRODUCTION)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = job;
        Err(UpdateError::Unsupported)
    }
}

/// UPD-014, the *Installed, restart* stage: the file at `target` is already the new version, so
/// this skips steps 1 to 4. It re-runs the `--version` self-test on the installed file and starts
/// it (step 5); it never stages and never hashes. Same blocking rules and the same environment
/// handling as [`apply_appimage`].
///
/// * `Ok(Restarting)`: the new process runs, the caller saves its state and quits;
/// * `Ok(InstalledRestartManually)`: it could not be started (or died at once) again;
/// * `Err(SelfTest)`: the installed file doesn't answer `--version` as `dockering <version>`;
/// * `Err(Unsupported)`: `target` is no longer a regular AppImage file.
pub fn restart_installed(
    target: &Path,
    expected_version: &str,
    env: &[(OsString, OsString)],
) -> Result<AppImageOutcome, UpdateError> {
    #[cfg(target_os = "linux")]
    {
        flow::restart(
            target,
            expected_version,
            env,
            flow::Limits {
                self_test_timeout: SELF_TEST_TIMEOUT,
                relaunch_grace: RELAUNCH_GRACE,
            },
            &seams::PRODUCTION,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (target, expected_version, env);
        Err(UpdateError::Unsupported)
    }
}

// Referenced by the module docs.
#[cfg(doc)]
use super::scrubbed_env;
