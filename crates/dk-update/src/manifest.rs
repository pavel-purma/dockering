//! Signed update manifest DTOs and release eligibility policy (UPD-002, UPD-004).

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::UpdateError;
use crate::source::release_asset_name;

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
    // UPD-002: an unknown schema means "no update", whatever else is in the file: a newer manifest
    // shape must not make an old client fail, so `schema` is read before anything else is.
    #[derive(Deserialize)]
    struct SchemaProbe {
        schema: Option<serde_json::Value>,
    }
    let probe: SchemaProbe =
        serde_json::from_slice(json).map_err(|error| UpdateError::Manifest(error.to_string()))?;
    if probe.schema.as_ref().is_some_and(|s| s.as_u64() != Some(1)) {
        return Ok(None);
    }

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

    // UPD-001: the asset must be this project's release download for exactly this version, so a
    // signed manifest can't point anywhere else; checked here so no offer (and no Download
    // fallback) ever exists for a rejected manifest.
    release_asset_name(&asset.url, &manifest.version.to_string())?;

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
                    "url": format!("https://github.com/pavel-purma/dockering/releases/download/v{version}/Dockering-Setup-x64.exe"),
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
    fn upd_002_unknown_schema_with_a_new_shape_is_no_update() {
        // A future manifest may look nothing like schema 1: an old client still reads it as
        // "no update", never as an error (UPD-002).
        let current = Version::parse("1.0.0").expect("valid test version");
        for value in [
            json!({ "schema": 2, "release": { "version": "9.0.0" }, "assets": [ { "os": "linux" } ] }),
            json!({ "schema": 2, "version": 9, "platforms": ["linux-x86_64"] }),
            json!({ "schema": 2 }),
            json!({ "schema": "2" }),
            json!({ "schema": 0 }),
        ] {
            let bytes = serde_json::to_vec(&value).expect("serializable");
            assert_eq!(
                evaluate(&bytes, &current, "windows-x86_64")
                    .unwrap_or_else(|e| panic!("{value}: {e}")),
                None,
                "{value}"
            );
        }
        // No schema at all is not "unknown": it is a malformed manifest.
        let bytes = serde_json::to_vec(&json!({ "version": "9.0.0" })).expect("serializable");
        assert!(matches!(
            evaluate(&bytes, &current, "windows-x86_64"),
            Err(UpdateError::Manifest(_))
        ));
        assert!(matches!(
            evaluate(b"not json", &current, "windows-x86_64"),
            Err(UpdateError::Manifest(_))
        ));
    }

    #[test]
    fn upd_001_asset_url_outside_the_pinned_release_is_rejected() {
        let current = Version::parse("1.0.0").expect("valid test version");
        let good = "https://github.com/pavel-purma/dockering/releases/download/v1.2.3/Dockering-Setup-x64.exe";
        for url in [
            "https://evil.example/pavel-purma/dockering/releases/download/v1.2.3/Dockering-Setup-x64.exe",
            "https://github.com/someone-else/dockering/releases/download/v1.2.3/Dockering-Setup-x64.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v1.2.4/Dockering-Setup-x64.exe",
            "http://github.com/pavel-purma/dockering/releases/download/v1.2.3/Dockering-Setup-x64.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v1.2.3/../Dockering-Setup-x64.exe",
            "https://github.com/pavel-purma/dockering/releases/download/v1.2.3/",
        ] {
            let mut value: serde_json::Value =
                serde_json::from_slice(&manifest_json(1, "1.2.3")).expect("json");
            value["platforms"]["windows-x86_64"]["url"] = json!(url);
            let bytes = serde_json::to_vec(&value).expect("serializable");
            assert!(
                matches!(
                    evaluate(&bytes, &current, "windows-x86_64"),
                    Err(UpdateError::Manifest(_))
                ),
                "{url}"
            );
        }
        // The pinned URL is accepted, and an entry for another platform is never looked at.
        let mut value: serde_json::Value =
            serde_json::from_slice(&manifest_json(1, "1.2.3")).expect("json");
        assert_eq!(value["platforms"]["windows-x86_64"]["url"], json!(good));
        value["platforms"]["macos-aarch64"] = value["platforms"]["windows-x86_64"].clone();
        value["platforms"]["macos-aarch64"]["url"] = json!("https://evil.example/x");
        let bytes = serde_json::to_vec(&value).expect("serializable");
        assert!(
            evaluate(&bytes, &current, "windows-x86_64")
                .expect("valid")
                .is_some()
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
    "linux-x86_64": { "kind": "appimage", "sha256": "00", "size": 1, "url": "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/Dockering-x86_64.AppImage" }
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
