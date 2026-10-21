//! `wsl.exe` failures mapped by their stable `Wsl/…` token, never by the localised sentence
//! (ENG-156, spike W-3.6), and the cleanup that undoes only what this run created (ENG-154/155).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dk_core::EngineError;

use super::SetupEnv;
use super::messages as m;
use crate::runner::{WslOutput, decode_message};

pub(crate) const UNREGISTER_TIMEOUT: Duration = Duration::from_secs(60);

/// What this run did, shared between the flow and its cleanup.
#[derive(Debug, Default)]
pub(crate) struct RunState {
    /// Set before `wsl.exe --import` is spawned; the only licence to `--unregister`.
    pub(crate) imported: AtomicBool,
    /// The import call is in flight (cancelled mid-way, the service may still finish it).
    pub(crate) importing: AtomicBool,
}

/// The last `/`-separated segment of the first `Wsl/…` token in `text`, for example
/// `Wsl/Service/RegisterDistro/ERROR_ALREADY_EXISTS` gives `ERROR_ALREADY_EXISTS`. The sentence
/// around it is localised, the token is not.
pub(crate) fn error_token(text: &str) -> Option<&str> {
    let start = text.find("Wsl/")?;
    let token = text[start..].split_whitespace().next()?;
    let token = token.trim_end_matches(['.', ',', ';', ')']);
    token.rsplit('/').next().filter(|t| !t.is_empty())
}

fn token_of(output: &WslOutput) -> Option<String> {
    let text = format!(
        "{}\n{}",
        decode_message(&output.stdout),
        decode_message(&output.stderr)
    );
    error_token(&text).map(str::to_owned)
}

pub(crate) fn is_distro_not_found(output: &WslOutput) -> bool {
    token_of(output).as_deref() == Some("WSL_E_DISTRO_NOT_FOUND")
}

/// The import left nothing behind and the name belongs to someone else.
pub(crate) fn import_hit_existing(output: &WslOutput) -> bool {
    matches!(
        token_of(output).as_deref(),
        Some("ERROR_ALREADY_EXISTS" | "ERROR_FILE_EXISTS")
    )
}

/// Which `wsl.exe` call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Import,
    Boot,
}

/// A failed `wsl.exe` call as a message with a hint.
pub(crate) fn map_failure(
    step: Step,
    output: &WslOutput,
    env: &SetupEnv,
    drive: &str,
) -> EngineError {
    let token = token_of(output);
    let token = token.as_deref().unwrap_or("");
    match token {
        "ERROR_ALREADY_EXISTS" => m::already_exists(&env.distro),
        "ERROR_FILE_EXISTS" => m::folder_in_use(&env.wsl_root.join(&env.distro)),
        "WSL_E_IMPORT_FAILED" => m::fail(
            "The engine couldn't be created from the download.",
            format!("Run `wsl --update`, make sure {drive} has free space, and try again."),
        ),
        "ERROR_FILE_NOT_FOUND" => m::fail("The downloaded file went missing.", "Try again."),
        "ERROR_PATH_NOT_FOUND" => m::fail(
            "The engine's folder couldn't be created.",
            format!("Free up space on {drive} and try again."),
        ),
        "E_INVALIDARG" => m::fail(
            "WSL didn't accept the engine's name.",
            "Run `wsl --update` and try again.",
        ),
        "WSL_E_DISTRO_NOT_FOUND" => m::fail(
            "The engine's distro disappeared during setup.",
            "Try again.",
        ),
        t if t.contains("DISK_FULL") => m::low_disk(drive, 0),
        t if t.contains("HYPERV_NOT_INSTALLED")
            || t.contains("VIRTUAL_MACHINE_PLATFORM")
            || t.contains("VIRTUALIZATION") =>
        {
            m::virtualization_off()
        }
        t if t.contains("OPTIONAL_COMPONENT") => m::wsl_missing(),
        _ => m::fail(
            match step {
                Step::Import => "WSL couldn't create the engine.",
                Step::Boot => "WSL couldn't start the engine.",
            },
            "Run `wsl --update` and try again.",
        ),
    }
}

/// An I/O error from running `wsl.exe` itself.
pub(crate) fn map_io(error: &std::io::Error) -> EngineError {
    match error.kind() {
        std::io::ErrorKind::NotFound => m::wsl_missing(),
        std::io::ErrorKind::TimedOut => m::fail("WSL took too long to answer.", "Try again."),
        _ => m::fail("WSL couldn't be run.", "Run `wsl --update` and try again."),
    }
}

/// Deletes the cache files and, only if this run called `--import`, unregisters its own distro.
pub(crate) async fn cleanup(
    env: &SetupEnv,
    state: &RunState,
    files: &[PathBuf],
    keep_distro: bool,
) {
    for file in files {
        // A just-dropped download may still hold the file for a moment.
        for attempt in 0..5 {
            match tokio::fs::remove_file(file).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if attempt == 4 => {
                    tracing::warn!(%error, "couldn't delete a setup cache file");
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }
    if keep_distro || !state.imported.load(Ordering::SeqCst) {
        return;
    }
    if state.importing.load(Ordering::SeqCst) {
        tokio::time::sleep(env.settle).await;
    }
    for attempt in 0..3 {
        match env
            .wsl
            .run_timeout(&["--unregister", &env.distro], UNREGISTER_TIMEOUT)
            .await
        {
            Ok(output) if output.success() || is_distro_not_found(&output) => {}
            Ok(output) => {
                tracing::warn!(message = %output.error_message(), "wsl --unregister failed");
            }
            Err(error) => tracing::warn!(%error, "wsl --unregister couldn't run"),
        }
        let gone = env
            .wsl
            .registered_distros()
            .await
            .is_ok_and(|names| !names.iter().any(|n| n.eq_ignore_ascii_case(&env.distro)));
        if gone {
            return;
        }
        if attempt < 2 {
            tokio::time::sleep(env.settle).await;
        }
    }
    tracing::warn!("the setup's distro is still registered after cleanup");
}

/// Runs [`cleanup`] on drop if the flow never reached its own cleanup (a panic).
pub(crate) struct CleanupGuard {
    pub(crate) env: Arc<SetupEnv>,
    pub(crate) state: Arc<RunState>,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) done: bool,
}

impl CleanupGuard {
    pub(crate) async fn finish(&mut self, keep_distro: bool) {
        self.done = true;
        cleanup(&self.env, &self.state, &self.files, keep_distro).await;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let (env, state, files) = (self.env.clone(), self.state.clone(), self.files.clone());
            handle.spawn(async move { cleanup(&env, &state, &files, false).await });
        }
    }
}
