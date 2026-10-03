//! Dockering updater primitives (UPD-001…003, UPD-006, UPD-007, UPD-010, UPD-012).
//!
//! No UI and no runtime of its own: async operations run on the caller's Tokio runtime (the
//! hub's, via `dk-hub`'s `UpdateService`). See `docs/spec/features/distribution.md` §7.

#![deny(unsafe_code)]

pub mod apply;
pub mod authenticode;
pub mod cleanup;
pub mod error;
pub mod install_kind;
pub mod keys;
pub mod manifest;
pub mod policy;
pub mod source;
pub mod verify;

pub use error::UpdateError;
pub use install_kind::InstallKind;
pub use manifest::{AssetKind, PlatformAsset, UpdateManifest};
pub use source::{GithubSource, UpdateSource};

/// Latest stable release update manifest (UPD-001).
pub const MANIFEST_URL: &str =
    "https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json";

/// Minisign signature for [`MANIFEST_URL`] (UPD-001, UPD-003).
pub const SIGNATURE_URL: &str = "https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json.minisig";
