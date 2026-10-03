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
    /// Updating this installation in place is not supported.
    #[error("updates aren't supported for this installation")]
    Unsupported,
    /// The signed update manifest was malformed.
    #[error("invalid update manifest: {0}")]
    Manifest(String),
}

impl From<std::io::Error> for UpdateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
