//! Updater error types.

/// An error produced while checking, downloading, verifying, or applying an update.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// A transport or request error.
    #[error("network error: {0}")]
    Network(String),
    /// An unsuccessful HTTP response.
    #[error("server returned HTTP {0}")]
    Http(u16),
    /// The manifest did not have a valid signature from any configured key.
    #[error("the update signature is invalid")]
    Signature,
    /// A downloaded file did not match its declared size or SHA-256 digest.
    #[error("the downloaded file failed its integrity check")]
    Integrity,
    /// The installer signature is missing, invalid, or from a different publisher.
    #[error("the installer is not signed by the Dockering publisher")]
    UntrustedInstaller,
    /// A filesystem operation failed.
    #[error("file error: {0}")]
    Io(String),
    /// The verified installer could not be started.
    #[error("couldn't start the installer: {0}")]
    Spawn(String),
    /// Windows (UPD-007): the verified installer could not be held `FILE_SHARE_READ`-locked, or
    /// it would not start while it is held (a sharing violation from the open, from the child
    /// process or from `ShellExecuteExW`). Nothing was started. The hub decides with
    /// [`crate::apply::locked_start_fallback`]: a per-user install retries once, an all-users
    /// install degrades the offer to *Download* rather than elevating a file that isn't locked.
    #[error("couldn't start the installer while it is locked: {0}")]
    LockedStartFailed(String),
    /// Updating this installation in place is not supported.
    #[error("updates aren't supported for this installation")]
    Unsupported,
    /// Linux AppImage apply (UPD-014): the staged copy could not be written, flushed or put in
    /// place (disk full, permission denied, `rename` failed). The old file is untouched and the
    /// offer stays ready, so the user can retry.
    #[error("couldn't write the update: {0}")]
    Stage(String),
    /// Linux AppImage apply (UPD-014): the new AppImage failed its `--version` self-test (wrong
    /// output, non-zero exit, timeout, crash, or it could not be run). It was not installed and
    /// the offer degrades to *Download* for that version.
    #[error("the new version failed its self-test: {0}")]
    SelfTest(String),
    /// The signed update manifest was malformed.
    #[error("invalid update manifest: {0}")]
    Manifest(String),
}

impl From<std::io::Error> for UpdateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
