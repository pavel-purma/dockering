//! Signed update manifest DTOs and release eligibility policy (UPD-002, UPD-004).

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::UpdateError;

/// The signed update manifest published with the latest stable GitHub Release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// Manifest schema version. Only schema 1 is currently understood.
    pub schema: u32,
    /// Version offered by this release.
    pub version: Version,
    /// Publication date supplied by the release workflow.
    pub pub_date: String,
    /// Browser-facing release notes URL.
    pub notes_url: String,
    /// Assets keyed by `<os>-<arch>`.
    pub platforms: BTreeMap<String, PlatformAsset>,
}

/// A downloadable asset for one operating system and architecture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAsset {
    /// Packaging format.
    pub kind: AssetKind,
    /// HTTPS release-asset URL.
    pub url: String,
    /// Lower- or uppercase hexadecimal SHA-256 digest.
    pub sha256: String,
    /// Exact expected size in bytes.
    pub size: u64,
}

/// Packaging format declared by an update manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// Inno Setup installer.
    Inno,
    /// Portable ZIP archive.
    Zip,
    /// macOS disk image.
    Dmg,
    /// Linux AppImage.
    Appimage,
    /// Debian package.
    Deb,
    /// Gzipped tar archive.
    #[serde(rename = "tar.gz")]
    TarGz,
    /// A packaging format introduced by a newer manifest producer.
    #[serde(other)]
    Unknown,
}

/// Return the manifest platform key (`<os>-<arch>`, e.g. `windows-x86_64`) for the current
/// build target. Rust's `OS`/`ARCH` names are exactly the manifest's.
#[must_use]
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Parse a manifest and select an eligible asset for `key`.
///
/// Unknown schemas and absent/ineligible releases intentionally return `Ok(None)`. A release is
/// eligible only when it is strictly newer. Stable builds never receive pre-release versions.
pub fn evaluate(
    json: &[u8],
    current: &Version,
    key: &str,
) -> Result<Option<(UpdateManifest, PlatformAsset)>, UpdateError> {
    let manifest: UpdateManifest =
        serde_json::from_slice(json).map_err(|error| UpdateError::Manifest(error.to_string()))?;

    if manifest.schema != 1
        || manifest.version <= *current
        || (current.pre.is_empty() && !manifest.version.pre.is_empty())
    {
        return Ok(None);
    }

    let Some(asset) = manifest.platforms.get(key).cloned() else {
        return Ok(None);
    };

    Ok(Some((manifest, asset)))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn manifest_json(schema: u32, version: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": schema,
            "version": version,
            "pub_date": "2026-10-03T00:00:00Z",
            "notes_url": "https://github.com/pavel-purma/dockering/releases/tag/v1.2.3",
            "future_top_level": { "ignored": true },
            "platforms": {
                "windows-x86_64": {
                    "kind": "inno",
                    "url": "https://github.com/pavel-purma/dockering/releases/download/v1.2.3/Dockering-Setup-x64.exe",
                    "sha256": "00",
                    "size": 1,
                    "future_asset_field": 42
                }
            }
        }))
        .expect("test JSON is serializable")
    }

    #[test]
    fn upd_002_manifest_roundtrip_ignores_unknown() {
        let current = Version::parse("1.0.0").expect("valid test version");
        let selected = evaluate(&manifest_json(1, "1.2.3"), &current, "windows-x86_64")
            .expect("manifest is valid")
            .expect("new platform update exists");

        assert_eq!(selected.0.version, Version::parse("1.2.3").unwrap());
        assert_eq!(selected.1.kind, AssetKind::Inno);
        let roundtrip = serde_json::to_vec(&selected.0).expect("manifest serializes");
        let decoded: UpdateManifest =
            serde_json::from_slice(&roundtrip).expect("manifest round-trips");
        assert_eq!(decoded, selected.0);
    }

    #[test]
    fn upd_002_unknown_schema_is_no_update() {
        let current = Version::parse("1.0.0").expect("valid test version");
        assert_eq!(
            evaluate(&manifest_json(2, "2.0.0"), &current, "windows-x86_64")
                .expect("unknown schemas are not errors"),
            None
        );
    }

    #[test]
    fn upd_004_no_downgrade_or_prerelease() {
        let stable = Version::parse("1.2.3").expect("valid test version");
        for version in ["1.2.3", "1.2.2", "2.0.0-rc.1"] {
            assert_eq!(
                evaluate(&manifest_json(1, version), &stable, "windows-x86_64")
                    .expect("manifest evaluates"),
                None,
                "{version} must not be offered to {stable}"
            );
        }

        let preview = Version::parse("2.0.0-beta.1").expect("valid test version");
        assert!(
            evaluate(&manifest_json(1, "2.0.0-rc.1"), &preview, "windows-x86_64")
                .expect("manifest evaluates")
                .is_some()
        );
    }

    /// The exact shape `cargo xtask update-manifest` writes (xtask/src/release.rs).
    #[test]
    fn upd_002_accepts_xtask_manifest() {
        let json = br#"{
  "notes_url": "https://github.com/pavel-purma/dockering/releases/tag/v0.2.0",
  "platforms": {
    "windows-x86_64": {
      "kind": "inno",
      "sha256": "0d2bd57a7c398f0ee3267315074c69f79a897aacb1ca7755ba7b00ad8f6acf61",
      "size": 11703661,
      "url": "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-Setup-x64.exe"
    },
    "linux-x86_64": { "kind": "appimage", "sha256": "00", "size": 1, "url": "https://github.com/x" }
  },
  "pub_date": "2026-10-03T00:12:41.1367851Z",
  "schema": 1,
  "version": "0.2.0"
}"#;
        let current = Version::new(0, 1, 0);
        let (manifest, asset) = evaluate(json, &current, "windows-x86_64")
            .expect("valid")
            .expect("update");
        assert_eq!(manifest.version, Version::new(0, 2, 0));
        assert_eq!(asset.size, 11_703_661);
        assert_eq!(
            evaluate(json, &current, "linux-x86_64")
                .expect("valid")
                .map(|(_, a)| a.kind),
            Some(AssetKind::Appimage)
        );
    }

    proptest! {
        #[test]
        fn upd_002_parse_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let current = Version::new(1, 0, 0);
            let _ = evaluate(&bytes, &current, "windows-x86_64");
        }
    }
}
