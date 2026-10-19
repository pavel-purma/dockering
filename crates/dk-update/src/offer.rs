//! The retained update offer (UPD-012, UPD-017): pure rules and the `offer.json` file store.
//!
//! The offer mirrors the newest *authenticated* manifest that is newer than the running version.
//! It is kept in `<data-local>/updates/offer.json` so the title-bar button and Settings survive
//! restarts, failed checks and long offline sessions, and it is dropped when it is withdrawn,
//! superseded, obsolete or too old. Nothing here does network I/O or owns a runtime; the hub calls
//! these functions from blocking workers and keeps the in-memory *Applying* stage to itself (it is
//! never saved).
//!
//! - [`write`], [`read`], [`read_or_discard`], [`remove`], [`discard`]: the file store.
//! - [`restore`]: the start-up path (signature, version, platform entry, freshness).
//! - [`is_fresh`], [`is_obsolete`]: the clock and version rules, evaluated on every read.
//! - [`after_check`]: what a finished check does to the stored offer (UPD-017 (3), (4)).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::UpdateError;
use crate::manifest::{PlatformAsset, UpdateManifest, evaluate};
use crate::verify::verify_manifest;

/// The retained offer's file name inside the updates directory (UPD-012).
pub const OFFER_FILE: &str = "offer.json";
/// The temp file [`write`] renames over [`OFFER_FILE`]; cleanup removes a stale one.
pub const OFFER_TMP_FILE: &str = "offer.json.tmp";
/// The only file format version this build reads and writes. Anything else is unreadable.
pub const FILE_VERSION: u32 = 1;
/// An offer whose `confirmed_at` is older than this is expired (UPD-017 (2)).
pub const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// A `confirmed_at` further in the future than this is clock skew and counts as expired.
pub const MAX_FUTURE_SKEW: Duration = Duration::from_secs(24 * 60 * 60);
/// A real offer is a few KB; refuse to parse anything absurd.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// The stage of an offer. Only these three are persisted: *Applying* exists in memory only, so a
/// file that says `"applying"` is unreadable (and is deleted by [`read_or_discard`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferStage {
    /// The install payload is being downloaded or verified.
    Downloading,
    /// The payload is verified (or the offer is a *Download* one) and can be acted on.
    Ready,
    /// The new version is installed but the new process was not started (Linux).
    InstalledRestart,
}

/// The retained offer as stored in `offer.json` (UPD-017).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOffer {
    /// The signed manifest JSON text, byte-exact (the signature covers these bytes).
    pub manifest: String,
    /// The minisign signature text (`.minisig`) for `manifest`.
    pub signature: String,
    /// RFC 3339 time of the last successful check that confirmed this offer.
    pub confirmed_at: String,
    /// Where the offer is in its life.
    pub stage: OfferStage,
    /// Installing this version failed; it stays a *Download* offer until a newer one arrives.
    pub install_failed: bool,
}

impl StoredOffer {
    /// A new offer from the manifest bytes. A manifest that isn't UTF-8 is rejected: the offer
    /// file stores the text, and a lossy conversion would break the signature.
    pub fn new(
        manifest: Vec<u8>,
        signature: String,
        confirmed_at: String,
        stage: OfferStage,
    ) -> Result<Self, UpdateError> {
        let manifest = String::from_utf8(manifest)
            .map_err(|_| UpdateError::Manifest("the manifest is not valid UTF-8".to_owned()))?;
        Ok(Self {
            manifest,
            signature,
            confirmed_at,
            stage,
            install_failed: false,
        })
    }

    /// The stored manifest, parsed. `None` when it doesn't parse (a damaged offer).
    #[must_use]
    pub fn parse_manifest(&self) -> Option<UpdateManifest> {
        serde_json::from_str(&self.manifest).ok()
    }

    /// The offered version.
    #[must_use]
    pub fn version(&self) -> Option<Version> {
        self.parse_manifest().map(|m| m.version)
    }

    /// The release notes URL of the offered version.
    #[must_use]
    pub fn notes_url(&self) -> Option<String> {
        self.parse_manifest().map(|m| m.notes_url)
    }

    /// The manifest entry for `platform` (`<os>-<arch>`).
    #[must_use]
    pub fn asset(&self, platform: &str) -> Option<PlatformAsset> {
        self.parse_manifest()?.platforms.remove(platform)
    }

    /// Records a failed install. The flag survives [`OfferChange::Refresh`] (the same version
    /// stays failed) and is reset only by [`OfferChange::Replace`], i.e. by a different entry.
    pub fn mark_install_failed(&mut self) {
        self.install_failed = true;
    }

    /// The same offer confirmed at `confirmed_at`: stage and failed flag are kept.
    #[must_use]
    pub fn refreshed(&self, confirmed_at: String) -> Self {
        Self {
            confirmed_at,
            ..self.clone()
        }
    }
}

// ---- file store ---------------------------------------------------------------------------

/// `<dir>/offer.json`.
#[must_use]
pub fn offer_path(dir: &Path) -> PathBuf {
    dir.join(OFFER_FILE)
}

fn tmp_path(dir: &Path) -> PathBuf {
    dir.join(OFFER_TMP_FILE)
}

/// The on-disk shape: the offer plus the format version.
#[derive(Serialize)]
struct FileRef<'a> {
    version: u32,
    #[serde(flatten)]
    offer: &'a StoredOffer,
}

#[derive(Deserialize)]
struct FileOwned {
    version: u32,
    #[serde(flatten)]
    offer: StoredOffer,
}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Saves `offer` atomically (UPD-012): the bytes go to `offer.json.tmp` in the same directory,
/// are flushed to disk, and the file is renamed over `offer.json`; on Unix the directory is
/// flushed too (a directory can't be opened on Windows, so that step is skipped there). A crash
/// leaves the old or the new file, never a `.bak`, and a failure removes the temp file. The
/// directory is created when missing. An error from the final directory flush comes after the
/// rename: the new file is in place but its durability isn't guaranteed.
pub fn write(dir: &Path, offer: &StoredOffer) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&FileRef {
        version: FILE_VERSION,
        offer,
    })
    .map_err(io::Error::other)?;
    fs::create_dir_all(dir)?;
    let tmp = tmp_path(dir);
    let renamed = write_synced(&tmp, &bytes).and_then(|()| fs::rename(&tmp, offer_path(dir)));
    if let Err(error) = renamed {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    sync_dir(dir)
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Reads `offer.json`: `Ok(None)` when there is none; an [`io::ErrorKind::InvalidData`] error
/// when it is unreadable as an offer (not JSON, missing fields, a stage that isn't saved such as
/// `"applying"`, or a `version` other than [`FILE_VERSION`]); any other error is an I/O error.
pub fn read(dir: &Path) -> io::Result<Option<StoredOffer>> {
    let file = match File::open(offer_path(dir)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(invalid("offer.json is too large"));
    }
    let parsed: FileOwned = serde_json::from_slice(&bytes).map_err(invalid)?;
    if parsed.version != FILE_VERSION {
        return Err(invalid(format!(
            "unsupported offer.json version {}",
            parsed.version
        )));
    }
    Ok(Some(parsed.offer))
}

/// [`read`], except that an unreadable or corrupt `offer.json` is deleted (never renamed to
/// `.bak`; the next check rediscovers the offer, UPD-012) and reported as no offer.
#[must_use]
pub fn read_or_discard(dir: &Path) -> Option<StoredOffer> {
    match read(dir) {
        Ok(offer) => offer,
        Err(error) => {
            tracing::warn!(%error, "updater: deleting an unreadable offer.json");
            let _ = fs::remove_file(offer_path(dir));
            None
        }
    }
}

/// Deletes `offer.json` and a stale `offer.json.tmp`. A missing file is not an error.
pub fn remove(dir: &Path) -> io::Result<()> {
    let mut result = Ok(());
    for path in [offer_path(dir), tmp_path(dir)] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => result = result.and(Err(error)),
        }
    }
    result
}

/// [`remove`], logging instead of failing: for an offer the caller has decided to drop
/// (expired, obsolete, withdrawn).
pub fn discard(dir: &Path) {
    if let Err(error) = remove(dir) {
        tracing::warn!(%error, "updater: couldn't delete offer.json");
    }
}

// ---- rules --------------------------------------------------------------------------------

/// Nanoseconds since the Unix epoch, negative before it. Total: no `SystemTime` arithmetic, so no
/// platform range panics.
fn unix_nanos(time: SystemTime) -> i128 {
    let nanos = |d: Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => nanos(after),
        Err(before) => -nanos(before.duration()),
    }
}

/// Whether the offer may still be shown and acted on at `now` (UPD-017 (2)). A pure function of
/// the clock, so it holds with automatic checks off and in a long offline session. False when
/// `confirmed_at` isn't RFC 3339, is more than [`MAX_FUTURE_SKEW`] in the future (a skewed
/// clock), or is more than [`MAX_AGE`] in the past.
#[must_use]
pub fn is_fresh(offer: &StoredOffer, now: SystemTime) -> bool {
    let Ok(confirmed) = OffsetDateTime::parse(&offer.confirmed_at, &Rfc3339) else {
        return false;
    };
    let elapsed = unix_nanos(now) - confirmed.unix_timestamp_nanos();
    if elapsed >= 0 {
        elapsed.unsigned_abs() <= MAX_AGE.as_nanos()
    } else {
        elapsed.unsigned_abs() <= MAX_FUTURE_SKEW.as_nanos()
    }
}

/// Whether the running version has caught up with the offer (an update, or an install by hand),
/// so the offer and its payload are deleted (UPD-017 (5)). A manifest that doesn't parse is
/// treated as obsolete: a damaged offer is never worth keeping.
#[must_use]
pub fn is_obsolete(offer: &StoredOffer, running: &Version) -> bool {
    offer.version().is_none_or(|version| version <= *running)
}

/// The start-up path (UPD-017 (1)): the offer saved in `dir`, only when it is trustworthy, newer
/// than `running`, has an entry for `platform` and is fresh at `now`; `None` otherwise.
///
/// - An unreadable file is deleted ([`read_or_discard`]).
/// - No `keys` fails closed (`None`) and leaves the file alone: a build without trust keys keeps
///   the offer but never shows it (UPD-017 (8)).
/// - The signature is checked against `keys` with [`verify_manifest`]; the version and platform
///   rules are those of [`evaluate`] (strictly newer, a stable build never gets a pre-release).
/// - A valid but expired or obsolete file is **not** deleted here; the caller decides (see
///   [`discard`]).
#[must_use]
pub fn restore(
    dir: &Path,
    keys: &[&str],
    running: &Version,
    platform: &str,
    now: SystemTime,
) -> Option<StoredOffer> {
    if keys.is_empty() {
        return None;
    }
    let offer = read_or_discard(dir)?;
    verify_manifest(offer.manifest.as_bytes(), &offer.signature, keys).ok()?;
    evaluate(offer.manifest.as_bytes(), running, platform).ok()??;
    is_fresh(&offer, now).then_some(offer)
}

// ---- what a check does to the offer -------------------------------------------------------

/// An *authenticated* manifest (its signature was verified by the caller) that has an entry
/// newer than the running version for this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewerManifest {
    /// The signed manifest text, byte-exact.
    pub json: String,
    /// The minisign signature text for `json`.
    pub signature: String,
    /// The offered version.
    pub version: Version,
    /// The manifest entry for this platform.
    pub asset: PlatformAsset,
    /// The release notes URL.
    pub notes_url: String,
    /// RFC 3339 time of the check that fetched it (becomes `confirmed_at`).
    pub checked_at: String,
}

impl NewerManifest {
    /// Evaluates an **already verified** manifest for `platform`: `Ok(None)` when it has no entry
    /// newer than `running` (the unknown-schema, not-newer, pre-release and missing-platform
    /// rules of [`evaluate`]), an error when it isn't UTF-8 or doesn't parse. Never verifies the
    /// signature itself.
    pub fn from_authenticated(
        json: &[u8],
        signature: &str,
        running: &Version,
        platform: &str,
        checked_at: &str,
    ) -> Result<Option<Self>, UpdateError> {
        let text = std::str::from_utf8(json)
            .map_err(|_| UpdateError::Manifest("the manifest is not valid UTF-8".to_owned()))?;
        let Some((manifest, asset)) = evaluate(json, running, platform)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            json: text.to_owned(),
            signature: signature.to_owned(),
            version: manifest.version,
            asset,
            notes_url: manifest.notes_url,
            checked_at: checked_at.to_owned(),
        }))
    }

    /// The offer this manifest starts: *Downloading*, install not failed.
    #[must_use]
    pub fn to_offer(&self) -> StoredOffer {
        StoredOffer {
            manifest: self.json.clone(),
            signature: self.signature.clone(),
            confirmed_at: self.checked_at.clone(),
            stage: OfferStage::Downloading,
            install_failed: false,
        }
    }
}

/// How a check ended, as far as the offer is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Network, HTTP, signature or manifest failure: nothing was learned.
    Failed,
    /// An authenticated manifest with no entry newer than the running version for this platform.
    NoNewer,
    /// An authenticated manifest with a newer entry for this platform.
    Newer(NewerManifest),
}

/// What to do with the stored offer after a check (UPD-017 (3), (4)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferChange {
    /// Change nothing.
    Keep,
    /// Same entry: set `confirmed_at` ([`StoredOffer::refreshed`]); stage and failed flag stay.
    Refresh {
        /// The new `confirmed_at`.
        confirmed_at: String,
    },
    /// A different entry: `new` (a *Downloading* offer) replaces the stored one.
    ///
    /// With `keep_old_until_verified` (a higher release arrived) the old offer stays actionable
    /// until the new payload is verified and its `offer.json` written (UPD-012). Without it (no
    /// offer yet, or the manifest is *lower* than the offer, which is what GitHub's `latest`
    /// serves after the offered release was pulled) the old offer is non-actionable at once and
    /// its payload is never applied.
    Replace {
        /// The replacement.
        new: StoredOffer,
        /// Keep the old offer until the new one's payload is verified.
        keep_old_until_verified: bool,
    },
    /// Delete the offer and its payload: the release is gone.
    Withdraw,
}

/// Whether the entry in `new` is the one the stored manifest `prev` already offers: the same
/// version and a manifest entry with the same URL, SHA-256 and size. The entry is matched by
/// content because a URL names exactly one platform's file.
fn same_entry(prev: &UpdateManifest, new: &NewerManifest) -> bool {
    prev.version == new.version
        && prev.platforms.values().any(|a| {
            a.url == new.asset.url
                && a.sha256.eq_ignore_ascii_case(&new.asset.sha256)
                && a.size == new.asset.size
        })
}

/// The effect of a finished check on the stored offer `prev` (UPD-017 (3), (4)):
///
/// - `Failed` keeps the offer ([`OfferChange::Keep`]).
/// - `NoNewer` withdraws it ([`OfferChange::Withdraw`]) when there is one, else `Keep`.
/// - `Newer` with the same entry as `prev` (same version, URL, SHA-256, size) refreshes it
///   ([`OfferChange::Refresh`]); a different entry replaces it ([`OfferChange::Replace`]), with
///   `keep_old_until_verified` only when the new version is higher than the old one.
///
/// A `Newer` that isn't newer than `running` breaks its own contract and is treated as `NoNewer`.
#[must_use]
pub fn after_check(
    prev: Option<&StoredOffer>,
    outcome: &CheckOutcome,
    running: &Version,
) -> OfferChange {
    let newer = match outcome {
        CheckOutcome::Failed => return OfferChange::Keep,
        CheckOutcome::Newer(newer)
            if newer.version > *running
                && (!running.pre.is_empty() || newer.version.pre.is_empty()) =>
        {
            newer
        }
        CheckOutcome::NoNewer | CheckOutcome::Newer(_) => {
            return if prev.is_some() {
                OfferChange::Withdraw
            } else {
                OfferChange::Keep
            };
        }
    };
    let replace = |keep_old_until_verified| OfferChange::Replace {
        new: newer.to_offer(),
        keep_old_until_verified,
    };
    let Some(prev_manifest) = prev.and_then(StoredOffer::parse_manifest) else {
        // No offer, or a damaged one: nothing to protect.
        return replace(false);
    };
    if same_entry(&prev_manifest, newer) {
        OfferChange::Refresh {
            confirmed_at: newer.checked_at.clone(),
        }
    } else {
        replace(newer.version > prev_manifest.version)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use proptest::prelude::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;

    const PLATFORM: &str = "linux-x86_64";

    fn running() -> Version {
        Version::new(0, 3, 0)
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// The clock all the tests agree on (2027-01-15).
    fn now() -> SystemTime {
        at(1_800_000_000)
    }

    fn rfc3339(time: SystemTime) -> String {
        OffsetDateTime::from(time).format(&Rfc3339).expect("format")
    }

    /// `now()` shifted by `secs` (negative = in the past), as an RFC 3339 string.
    fn confirmed(secs: i64) -> String {
        let shifted = if secs >= 0 {
            now() + Duration::from_secs(secs.unsigned_abs())
        } else {
            now() - Duration::from_secs(secs.unsigned_abs())
        };
        rfc3339(shifted)
    }

    fn manifest_with(schema: u32, version: &str, platform: &str, tag: &str) -> String {
        serde_json::to_string_pretty(&json!({
            "schema": schema,
            "version": version,
            "pub_date": "2026-10-03T00:00:00Z",
            "notes_url": format!("https://github.com/pavel-purma/dockering/releases/tag/v{version}"),
            "platforms": { platform: {
                "kind": "appimage",
                "url": format!(
                    "https://github.com/pavel-purma/dockering/releases/download/v{version}/Dockering-{tag}.AppImage"
                ),
                "sha256": hex::encode(Sha256::digest(tag.as_bytes())),
                "size": 1234,
            }},
        }))
        .expect("json")
    }

    fn manifest_text(version: &str, tag: &str) -> String {
        manifest_with(1, version, PLATFORM, tag)
    }

    fn keypair() -> minisign::KeyPair {
        minisign::KeyPair::generate_unencrypted_keypair().expect("keypair")
    }

    /// Signs like `crates/dk-hub/src/updates_tests.rs::signed_manifest`.
    fn sign(kp: &minisign::KeyPair, text: &str) -> String {
        minisign::sign(None, &kp.sk, Cursor::new(text.as_bytes()), None, None)
            .expect("sign")
            .into_string()
    }

    fn stored(version: &str, tag: &str, stage: OfferStage) -> StoredOffer {
        StoredOffer {
            manifest: manifest_text(version, tag),
            signature: "untrusted signature".to_owned(),
            confirmed_at: confirmed(-60),
            stage,
            install_failed: false,
        }
    }

    fn newer(version: &str, tag: &str) -> NewerManifest {
        NewerManifest::from_authenticated(
            manifest_text(version, tag).as_bytes(),
            "sig",
            &running(),
            PLATFORM,
            &confirmed(0),
        )
        .expect("valid")
        .expect("newer")
    }

    // ---- file store ------------------------------------------------------------------------

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn upd_012_offer_written_atomically_and_never_bak() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The directory is created on demand.
        let dir = tmp.path().join("updates");
        let mut offer = stored("0.4.0", "a", OfferStage::Downloading);
        write(&dir, &offer).expect("write");
        assert_eq!(names(&dir), [OFFER_FILE]);
        assert_eq!(read(&dir).expect("read"), Some(offer.clone()));

        // The file is versioned.
        let on_disk: serde_json::Value =
            serde_json::from_slice(&fs::read(offer_path(&dir)).expect("read")).expect("json");
        assert_eq!(on_disk["version"], json!(1));
        assert_eq!(on_disk["stage"], json!("downloading"));
        assert_eq!(on_disk["install_failed"], json!(false));

        // A stale temp file from a crashed write is overwritten and gone afterwards, and a
        // replacement leaves neither a `.tmp` nor a `.bak` behind.
        fs::write(tmp_path(&dir), b"half a wri").expect("stale tmp");
        offer.stage = OfferStage::Ready;
        offer.mark_install_failed();
        write(&dir, &offer).expect("write");
        assert_eq!(names(&dir), [OFFER_FILE]);
        assert_eq!(read(&dir).expect("read"), Some(offer));

        // A failing write reports the error, keeps the old file and leaves no temp file: here
        // `offer.json` is a directory, which the rename can't replace.
        let blocked = tmp.path().join("blocked");
        fs::create_dir_all(offer_path(&blocked)).expect("mkdir");
        assert!(write(&blocked, &stored("0.4.0", "a", OfferStage::Ready)).is_err());
        assert_eq!(names(&blocked), [OFFER_FILE]);

        // Remove deletes the file and a stale temp file, and is idempotent.
        fs::write(tmp_path(&dir), b"stale").expect("stale tmp");
        remove(&dir).expect("remove");
        assert!(names(&dir).is_empty());
        remove(&dir).expect("remove again");
        assert_eq!(read(&dir).expect("read"), None);
    }

    #[test]
    fn upd_012_manifest_text_survives_roundtrip_byte_exact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Whitespace, escapes and non-ASCII characters: the signature covers every byte.
        let text = "{ \"schema\":1 ,\n\t\"version\" : \"0.4.0\",\r\n \"note\": \"caf\u{e9} \\u00e9 \\\" \u{1f433}\" }  \n";
        let offer = StoredOffer::new(
            text.as_bytes().to_vec(),
            "sig\nwith lines\n".to_owned(),
            confirmed(0),
            OfferStage::Ready,
        )
        .expect("utf-8");
        write(tmp.path(), &offer).expect("write");
        let back = read(tmp.path()).expect("read").expect("offer");
        assert_eq!(back.manifest, text);
        assert_eq!(back.signature, "sig\nwith lines\n");
    }

    #[test]
    fn upd_012_non_utf8_manifest_rejected_at_creation() {
        let error = StoredOffer::new(
            vec![b'{', 0xFF, 0xFE, b'}'],
            "sig".to_owned(),
            confirmed(0),
            OfferStage::Ready,
        )
        .expect_err("not UTF-8");
        assert!(matches!(error, UpdateError::Manifest(_)));
        assert!(
            NewerManifest::from_authenticated(
                &[b'{', 0xFF, b'}'],
                "sig",
                &running(),
                PLATFORM,
                &confirmed(0)
            )
            .is_err()
        );
    }

    #[test]
    fn upd_012_unreadable_offer_is_deleted() {
        let good = serde_json::to_value(stored("0.4.0", "a", OfferStage::Ready)).expect("value");
        let with = |version: u64, stage: Option<&str>| {
            let mut v = good.clone();
            v["version"] = json!(version);
            if let Some(stage) = stage {
                v["stage"] = json!(stage);
            }
            serde_json::to_vec(&v).expect("json")
        };
        let mut truncated = with(1, None);
        truncated.truncate(truncated.len() - 20);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("garbage", b"\x00\x01 not json".to_vec()),
            ("non-UTF-8", vec![0xFF, 0xFE, 0xFD]),
            ("truncated", truncated),
            ("no version", {
                let mut v = good.clone();
                v.as_object_mut().expect("object").remove("version");
                serde_json::to_vec(&v).expect("json")
            }),
            ("newer version", with(2, None)),
            ("version 0", with(0, None)),
            ("applying stage", with(1, Some("applying"))),
            ("unknown stage", with(1, Some("paused"))),
            ("json array", b"[]".to_vec()),
            ("too large", vec![b' '; (MAX_FILE_BYTES as usize) + 1]),
        ];
        for (name, bytes) in cases {
            let tmp = tempfile::tempdir().expect("tempdir");
            fs::write(offer_path(tmp.path()), &bytes).expect("write");
            assert_eq!(
                read(tmp.path()).expect_err(name).kind(),
                io::ErrorKind::InvalidData,
                "{name}"
            );
            assert_eq!(read_or_discard(tmp.path()), None, "{name}");
            // Deleted, not moved aside.
            assert!(
                names(tmp.path()).is_empty(),
                "{name}: {:?}",
                names(tmp.path())
            );
        }

        // The control: the same helper makes a readable file when nothing is wrong.
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(offer_path(tmp.path()), with(1, None)).expect("write");
        assert_eq!(
            read_or_discard(tmp.path()),
            Some(stored_like(&good)),
            "a good file is returned and kept"
        );
        assert_eq!(names(tmp.path()), [OFFER_FILE]);

        // No file at all is not an error.
        let empty = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(empty.path()).expect("read"), None);
        assert_eq!(read_or_discard(empty.path()), None);
    }

    fn stored_like(value: &serde_json::Value) -> StoredOffer {
        serde_json::from_value(value.clone()).expect("offer")
    }

    #[test]
    fn upd_017_applying_stage_not_persisted() {
        // Only three stages exist, so *Applying* can't be written...
        for (stage, name) in [
            (OfferStage::Downloading, "downloading"),
            (OfferStage::Ready, "ready"),
            (OfferStage::InstalledRestart, "installed_restart"),
        ] {
            assert_eq!(serde_json::to_value(stage).expect("value"), json!(name));
            assert_eq!(
                serde_json::from_value::<OfferStage>(json!(name)).expect("stage"),
                stage
            );
        }
        assert!(serde_json::from_value::<OfferStage>(json!("applying")).is_err());

        // ...and a file that says so is unreadable and deleted.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut value = serde_json::to_value(stored("0.4.0", "a", OfferStage::Ready)).expect("v");
        value["version"] = json!(1);
        value["stage"] = json!("applying");
        fs::write(
            offer_path(tmp.path()),
            serde_json::to_vec(&value).expect("json"),
        )
        .expect("write");
        assert_eq!(
            read(tmp.path()).expect_err("unreadable").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(read_or_discard(tmp.path()), None);
        assert!(!offer_path(tmp.path()).exists());
    }

    // ---- freshness ---------------------------------------------------------------------------

    fn offer_confirmed_at(confirmed_at: String) -> StoredOffer {
        StoredOffer {
            confirmed_at,
            ..stored("0.4.0", "a", OfferStage::Ready)
        }
    }

    #[test]
    fn upd_017_restore_expires_after_14_days() {
        let kp = keypair();
        let key = kp.pk.to_base64();
        let text = manifest_text("0.4.0", "a");
        let signature = sign(&kp, &text);
        let tmp = tempfile::tempdir().expect("tempdir");
        let at_age = |secs: u64| {
            let offer = StoredOffer::new(
                text.clone().into_bytes(),
                signature.clone(),
                rfc3339(now() - Duration::from_secs(secs)),
                OfferStage::Ready,
            )
            .expect("utf-8");
            write(tmp.path(), &offer).expect("write");
            restore(tmp.path(), &[&key], &running(), PLATFORM, now())
        };
        let day = 24 * 60 * 60;
        assert!(at_age(0).is_some());
        assert!(at_age(13 * day + 23 * 3600).is_some());
        assert!(at_age(14 * day).is_some(), "exactly 14 days is still fresh");
        assert!(at_age(14 * day + 1).is_none());
        assert!(at_age(400 * day).is_none());
        // `restore` doesn't delete a valid but expired file: the caller decides.
        assert!(offer_path(tmp.path()).exists());
        discard(tmp.path());
        assert!(names(tmp.path()).is_empty());
    }

    #[test]
    fn upd_017_expiry_applies_when_checks_are_off() {
        // Freshness is a pure function of the offer and the clock: no check, no setting and no
        // stage takes part, so a long offline session or `check = false` still expires it.
        let confirmed_at = confirmed(0);
        for stage in [
            OfferStage::Downloading,
            OfferStage::Ready,
            OfferStage::InstalledRestart,
        ] {
            for failed in [false, true] {
                let mut offer = offer_confirmed_at(confirmed_at.clone());
                offer.stage = stage;
                offer.install_failed = failed;
                assert!(is_fresh(&offer, now()));
                assert!(is_fresh(&offer, now() + MAX_AGE - Duration::from_secs(1)));
                assert!(is_fresh(&offer, now() + MAX_AGE));
                assert!(!is_fresh(&offer, now() + MAX_AGE + Duration::from_secs(1)));
                assert!(!is_fresh(&offer, now() + MAX_AGE * 100));
            }
        }
        // Sub-second precision, and a UTC offset other than `Z`.
        let offer = offer_confirmed_at("2027-01-15T10:30:00.123456789+02:00".to_owned());
        let confirmed = OffsetDateTime::parse(&offer.confirmed_at, &Rfc3339).expect("parse");
        let confirmed_secs = u64::try_from(confirmed.unix_timestamp()).expect("positive");
        assert!(is_fresh(&offer, at(confirmed_secs + 14 * 24 * 3600)));
        assert!(!is_fresh(&offer, at(confirmed_secs + 14 * 24 * 3600 + 1)));
    }

    #[test]
    fn upd_017_future_confirmed_at_expires() {
        let fresh = |secs: i64| is_fresh(&offer_confirmed_at(confirmed(secs)), now());
        assert!(fresh(60), "a minute of skew is tolerated");
        assert!(fresh(23 * 3600));
        assert!(fresh(24 * 3600), "exactly 24 h ahead is still tolerated");
        assert!(!fresh(24 * 3600 + 1));
        assert!(!fresh(30 * 24 * 3600));

        for unparsable in [
            "",
            "yesterday",
            "2027-01-15",
            "2027-01-15T10:30:00",
            "2027-13-45T00:00:00Z",
            "1800000000",
            " 2027-01-15T10:30:00Z",
        ] {
            assert!(
                !is_fresh(&offer_confirmed_at(unparsable.to_owned()), now()),
                "{unparsable:?}"
            );
        }
    }

    #[test]
    fn upd_017_catching_up_deletes_offer() {
        let offer = stored("0.4.0", "a", OfferStage::Ready);
        assert!(!is_obsolete(&offer, &Version::new(0, 3, 9)));
        assert!(
            is_obsolete(&offer, &Version::new(0, 4, 0)),
            "installed by hand"
        );
        assert!(is_obsolete(&offer, &Version::new(0, 5, 0)));
        // A pre-release of the offered version is older than it.
        assert!(!is_obsolete(
            &offer,
            &Version::parse("0.4.0-rc.1").expect("version")
        ));

        let mut damaged = offer.clone();
        damaged.manifest = "{".to_owned();
        assert!(is_obsolete(&damaged, &Version::new(0, 0, 1)));

        // The caller deletes an obsolete offer from disk.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), &offer).expect("write");
        let stored = read_or_discard(tmp.path()).expect("offer");
        if is_obsolete(&stored, &Version::new(0, 4, 0)) {
            discard(tmp.path());
        }
        assert!(names(tmp.path()).is_empty());
    }

    // ---- restore -----------------------------------------------------------------------------

    #[test]
    fn upd_017_restore_verifies_signature_and_version() {
        let kp = keypair();
        let key = kp.pk.to_base64();
        let tmp = tempfile::tempdir().expect("tempdir");
        let put = |text: &str, signature: &str| {
            let offer = StoredOffer::new(
                text.as_bytes().to_vec(),
                signature.to_owned(),
                confirmed(-3600),
                OfferStage::Ready,
            )
            .expect("utf-8");
            write(tmp.path(), &offer).expect("write");
            offer
        };
        let restore_with = |keys: &[&str], running: &Version, platform: &str| {
            restore(tmp.path(), keys, running, platform, now())
        };

        // A good offer comes back unchanged, with the current or the next key.
        let text = manifest_text("0.4.0", "a");
        let signature = sign(&kp, &text);
        let offer = put(&text, &signature);
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            Some(offer.clone())
        );
        let other = keypair().pk.to_base64();
        assert_eq!(
            restore_with(&[&other, &key], &running(), PLATFORM),
            Some(offer.clone())
        );
        assert_eq!(offer.version(), Some(Version::new(0, 4, 0)));
        assert_eq!(
            offer.notes_url().as_deref(),
            Some("https://github.com/pavel-purma/dockering/releases/tag/v0.4.0")
        );
        assert_eq!(offer.asset(PLATFORM).map(|a| a.size), Some(1234));
        assert_eq!(offer.asset("windows-x86_64"), None);

        // Signature problems: tampered text, a signature for another manifest, garbage.
        let tampered = text.replace("1234", "9999");
        put(&tampered, &signature);
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "tampered"
        );
        put(&text, &sign(&kp, &manifest_text("0.4.0", "b")));
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "other manifest"
        );
        put(&text, "not a minisign signature");
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "garbage signature"
        );
        put(&text, "");
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "no signature"
        );

        // Key problems: the wrong key, an unparsable key, and no keys (fail closed, and the
        // file stays: a build without keys keeps the offer, UPD-017 (8)).
        put(&text, &signature);
        assert_eq!(
            restore_with(&[&other], &running(), PLATFORM),
            None,
            "wrong key"
        );
        assert_eq!(
            restore_with(&["not a key"], &running(), PLATFORM),
            None,
            "bad key"
        );
        assert_eq!(restore_with(&[], &running(), PLATFORM), None, "no keys");
        assert!(offer_path(tmp.path()).exists());
        assert!(restore_with(&[&key], &running(), PLATFORM).is_some());

        // Version problems: not newer than the running version (an update by hand), and a
        // pre-release is never restored on a stable build.
        assert_eq!(
            restore_with(&[&key], &Version::new(0, 4, 0), PLATFORM),
            None
        );
        assert_eq!(
            restore_with(&[&key], &Version::new(1, 0, 0), PLATFORM),
            None
        );
        assert!(restore_with(&[&key], &Version::new(0, 3, 99), PLATFORM).is_some());
        let rc = manifest_text("0.4.0-rc.1", "a");
        put(&rc, &sign(&kp, &rc));
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "pre-release"
        );
        let preview = Version::parse("0.3.0-beta.1").expect("version");
        assert!(restore_with(&[&key], &preview, PLATFORM).is_some());

        // Entry problems: no entry for this platform, an unknown schema.
        put(&text, &signature);
        assert_eq!(
            restore_with(&[&key], &running(), "linux-aarch64"),
            None,
            "platform"
        );
        assert_eq!(
            restore_with(&[&key], &running(), "windows-x86_64"),
            None,
            "platform"
        );
        let future = manifest_with(2, "0.4.0", PLATFORM, "a");
        put(&future, &sign(&kp, &future));
        assert_eq!(
            restore_with(&[&key], &running(), PLATFORM),
            None,
            "schema 2"
        );

        // No file, and a damaged file (which is deleted).
        let empty = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            restore(empty.path(), &[&key], &running(), PLATFORM, now()),
            None
        );
        fs::write(offer_path(empty.path()), b"{").expect("write");
        assert_eq!(
            restore(empty.path(), &[&key], &running(), PLATFORM, now()),
            None
        );
        assert!(names(empty.path()).is_empty());
    }

    // ---- after_check -------------------------------------------------------------------------

    #[test]
    fn upd_017_failed_check_keeps_offer() {
        let offer = stored("0.4.0", "a", OfferStage::Ready);
        assert_eq!(
            after_check(Some(&offer), &CheckOutcome::Failed, &running()),
            OfferChange::Keep
        );
        assert_eq!(
            after_check(None, &CheckOutcome::Failed, &running()),
            OfferChange::Keep
        );
    }

    #[test]
    fn upd_017_withdrawn_release_clears_offer_and_payload() {
        for stage in [
            OfferStage::Downloading,
            OfferStage::Ready,
            OfferStage::InstalledRestart,
        ] {
            let offer = stored("0.4.0", "a", stage);
            assert_eq!(
                after_check(Some(&offer), &CheckOutcome::NoNewer, &running()),
                OfferChange::Withdraw,
                "{stage:?}"
            );
        }
        // Nothing to withdraw.
        assert_eq!(
            after_check(None, &CheckOutcome::NoNewer, &running()),
            OfferChange::Keep
        );
        // A `Newer` that isn't newer than the running version is the same as no newer entry.
        let stale = CheckOutcome::Newer(NewerManifest {
            version: Version::new(0, 3, 0),
            ..newer("0.4.0", "a")
        });
        let offer = stored("0.4.0", "a", OfferStage::Ready);
        assert_eq!(
            after_check(Some(&offer), &stale, &running()),
            OfferChange::Withdraw
        );
        // From the manifest side: a manifest that offers nothing newer is `Ok(None)`.
        assert_eq!(
            NewerManifest::from_authenticated(
                manifest_text("0.3.0", "a").as_bytes(),
                "sig",
                &running(),
                PLATFORM,
                &confirmed(0)
            ),
            Ok(None)
        );
        assert_eq!(
            NewerManifest::from_authenticated(
                manifest_text("0.4.0", "a").as_bytes(),
                "sig",
                &running(),
                "linux-aarch64",
                &confirmed(0)
            ),
            Ok(None)
        );
    }

    #[test]
    fn upd_017_older_manifest_after_pull_replaces_offer() {
        // 0.5.0 was offered, then pulled: GitHub's `latest` is 0.4.0 again.
        let mut offer = stored("0.5.0", "a", OfferStage::Ready);
        offer.install_failed = true;
        let outcome = CheckOutcome::Newer(newer("0.4.0", "a"));
        let change = after_check(Some(&offer), &outcome, &running());
        let OfferChange::Replace {
            new,
            keep_old_until_verified,
        } = change
        else {
            panic!("expected a replacement, got {change:?}");
        };
        assert!(
            !keep_old_until_verified,
            "the pulled release is never applied"
        );
        assert_eq!(new.version(), Some(Version::new(0, 4, 0)));
        assert_eq!(new.stage, OfferStage::Downloading);
        assert!(!new.install_failed);
    }

    #[test]
    fn upd_017_newer_manifest_replaces_after_payload_verified() {
        let mut offer = stored("0.4.0", "a", OfferStage::Ready);
        offer.install_failed = true;
        let n = newer("0.5.0", "a");
        let change = after_check(Some(&offer), &CheckOutcome::Newer(n.clone()), &running());
        assert_eq!(
            change,
            OfferChange::Replace {
                new: StoredOffer {
                    manifest: n.json.clone(),
                    signature: "sig".to_owned(),
                    confirmed_at: n.checked_at.clone(),
                    stage: OfferStage::Downloading,
                    install_failed: false,
                },
                keep_old_until_verified: true,
            },
            "a higher release replaces the old one only after its payload verified"
        );

        // No offer yet: nothing to keep.
        let change = after_check(None, &CheckOutcome::Newer(n.clone()), &running());
        assert_eq!(
            change,
            OfferChange::Replace {
                new: n.to_offer(),
                keep_old_until_verified: false
            }
        );
        // A damaged stored offer is nothing to protect either.
        let mut damaged = offer.clone();
        damaged.manifest = "{".to_owned();
        let change = after_check(Some(&damaged), &CheckOutcome::Newer(n.clone()), &running());
        assert_eq!(
            change,
            OfferChange::Replace {
                new: n.to_offer(),
                keep_old_until_verified: false
            }
        );
    }

    #[test]
    fn upd_017_identical_entry_keeps_stage_and_failed_flag() {
        let n = newer("0.4.0", "a");
        for stage in [
            OfferStage::Downloading,
            OfferStage::Ready,
            OfferStage::InstalledRestart,
        ] {
            for failed in [false, true] {
                let mut offer = stored("0.4.0", "a", stage);
                offer.confirmed_at = confirmed(-7 * 24 * 3600);
                offer.install_failed = failed;
                let change = after_check(Some(&offer), &CheckOutcome::Newer(n.clone()), &running());
                let OfferChange::Refresh { confirmed_at } = change else {
                    panic!("expected a refresh, got {change:?}");
                };
                assert_eq!(confirmed_at, n.checked_at);
                let refreshed = offer.refreshed(confirmed_at);
                assert_eq!(refreshed.confirmed_at, n.checked_at);
                assert_eq!(refreshed.stage, stage);
                assert_eq!(refreshed.install_failed, failed);
                assert_eq!(refreshed.manifest, offer.manifest);
                assert_eq!(refreshed.signature, offer.signature);
                assert!(is_fresh(&refreshed, now()));
            }
        }

        // Only version, URL, SHA-256 and size count: another publication date, a re-signed
        // manifest or an upper-case digest is still the same entry.
        let offer = stored("0.4.0", "a", OfferStage::Ready);
        let mut shifted = n.clone();
        shifted.json = shifted.json.replace("2026-10-03", "2026-10-04");
        shifted.signature = "another signature".to_owned();
        shifted.asset.sha256 = shifted.asset.sha256.to_uppercase();
        assert!(matches!(
            after_check(Some(&offer), &CheckOutcome::Newer(shifted), &running()),
            OfferChange::Refresh { .. }
        ));

        // A different digest, URL or size, or another version, is a different entry.
        for different in [
            NewerManifest {
                asset: PlatformAsset {
                    sha256: "00".repeat(32),
                    ..n.asset.clone()
                },
                ..n.clone()
            },
            NewerManifest {
                asset: PlatformAsset {
                    url: format!("{}?x", n.asset.url),
                    ..n.asset.clone()
                },
                ..n.clone()
            },
            NewerManifest {
                asset: PlatformAsset {
                    size: n.asset.size + 1,
                    ..n.asset.clone()
                },
                ..n.clone()
            },
            newer("0.4.1", "a"),
        ] {
            assert!(
                matches!(
                    after_check(Some(&offer), &CheckOutcome::Newer(different), &running()),
                    OfferChange::Replace { .. }
                ),
                "a different entry replaces the offer"
            );
        }
    }

    #[test]
    fn upd_017_failed_install_flag_resets_only_with_a_different_entry() {
        let mut offer = stored("0.4.0", "a", OfferStage::Ready);
        offer.mark_install_failed();
        assert!(offer.install_failed);

        // The same release confirmed again stays failed...
        let same = CheckOutcome::Newer(newer("0.4.0", "a"));
        let OfferChange::Refresh { confirmed_at } = after_check(Some(&offer), &same, &running())
        else {
            panic!("expected a refresh");
        };
        assert!(offer.refreshed(confirmed_at).install_failed);

        // ...and a newer release starts from a clean offer.
        let newer_release = CheckOutcome::Newer(newer("0.4.1", "a"));
        let OfferChange::Replace { new, .. } =
            after_check(Some(&offer), &newer_release, &running())
        else {
            panic!("expected a replacement");
        };
        assert!(!new.install_failed);
        assert_eq!(new.stage, OfferStage::Downloading);
    }

    // ---- properties --------------------------------------------------------------------------

    type Triple = (u64, u64, u64);

    fn version_of((major, minor, patch): Triple) -> Version {
        Version::new(major, minor, patch)
    }

    fn arb_triple() -> impl Strategy<Value = Triple> {
        (0u64..4, 0u64..4, 0u64..4)
    }

    fn arb_stage() -> impl Strategy<Value = OfferStage> {
        prop_oneof![
            Just(OfferStage::Downloading),
            Just(OfferStage::Ready),
            Just(OfferStage::InstalledRestart),
        ]
    }

    fn offer_of(version: Triple, tag: u8, stage: OfferStage, failed: bool) -> StoredOffer {
        let mut offer = stored(&version_of(version).to_string(), &tag.to_string(), stage);
        offer.install_failed = failed;
        offer
    }

    /// A `Newer` built directly (no signature work), whatever its version.
    fn newer_of(version: Triple, tag: u8) -> NewerManifest {
        let version = version_of(version).to_string();
        let tag = tag.to_string();
        let text = manifest_text(&version, &tag);
        let (manifest, asset) = evaluate(text.as_bytes(), &Version::new(0, 0, 0), PLATFORM)
            .expect("valid")
            .unwrap_or_else(|| panic!("0.0.0 is older than {version}"));
        NewerManifest {
            json: text,
            signature: "sig".to_owned(),
            version: manifest.version,
            asset,
            notes_url: manifest.notes_url,
            checked_at: confirmed(0),
        }
    }

    prop_compose! {
        fn arb_prev()(
            version in arb_triple(),
            tag in 0u8..3,
            stage in arb_stage(),
            failed in any::<bool>(),
        ) -> StoredOffer {
            offer_of(version, tag, stage, failed)
        }
    }

    proptest! {
        #[test]
        fn upd_017_after_check_never_panics(
            prev in proptest::option::of(arb_prev()),
            kind in 0u8..3,
            new_version in (0u64..4, 0u64..4, 1u64..4),
            tag in 0u8..3,
            run in arb_triple(),
        ) {
            let outcome = match kind {
                0 => CheckOutcome::Failed,
                1 => CheckOutcome::NoNewer,
                _ => CheckOutcome::Newer(newer_of(new_version, tag)),
            };
            let _ = after_check(prev.as_ref(), &outcome, &version_of(run));
        }

        #[test]
        fn upd_017_failed_check_always_keeps(
            prev in proptest::option::of(arb_prev()),
            run in arb_triple(),
        ) {
            prop_assert_eq!(
                after_check(prev.as_ref(), &CheckOutcome::Failed, &version_of(run)),
                OfferChange::Keep
            );
        }

        #[test]
        fn upd_017_no_newer_entry_always_withdraws_an_offer(
            prev in arb_prev(),
            run in arb_triple(),
        ) {
            prop_assert_eq!(
                after_check(Some(&prev), &CheckOutcome::NoNewer, &version_of(run)),
                OfferChange::Withdraw
            );
            prop_assert_eq!(
                after_check(None, &CheckOutcome::NoNewer, &version_of(run)),
                OfferChange::Keep
            );
        }

        #[test]
        fn upd_017_refresh_preserves_stage_and_failed_flag(
            version in (0u64..4, 0u64..4, 1u64..4),
            tag in 0u8..3,
            stage in arb_stage(),
            failed: bool,
        ) {
            // The same entry, whatever the stage: a refresh that keeps everything but the time.
            let prev = offer_of(version, tag, stage, failed);
            let n = newer_of(version, tag);
            let change = after_check(Some(&prev), &CheckOutcome::Newer(n.clone()), &Version::new(0, 0, 0));
            let OfferChange::Refresh { confirmed_at } = change else {
                return Err(TestCaseError::fail(format!("expected a refresh, got {change:?}")));
            };
            let refreshed = prev.refreshed(confirmed_at);
            prop_assert_eq!(refreshed.stage, stage);
            prop_assert_eq!(refreshed.install_failed, failed);
            prop_assert_eq!(&refreshed.manifest, &prev.manifest);
            prop_assert_eq!(&refreshed.confirmed_at, &n.checked_at);
        }

        #[test]
        fn upd_017_replacement_starts_clean_and_keeps_old_only_when_higher(
            prev in arb_prev(),
            new_version in (0u64..4, 0u64..4, 1u64..4),
            tag in 0u8..3,
        ) {
            let n = newer_of(new_version, tag);
            let prev_version = prev.version().expect("prev parses");
            let change = after_check(Some(&prev), &CheckOutcome::Newer(n.clone()), &Version::new(0, 0, 0));
            match change {
                OfferChange::Refresh { .. } => {
                    prop_assert_eq!(&n.version, &prev_version);
                }
                OfferChange::Replace { new, keep_old_until_verified } => {
                    prop_assert_eq!(keep_old_until_verified, n.version > prev_version);
                    prop_assert_eq!(new.stage, OfferStage::Downloading);
                    prop_assert!(!new.install_failed);
                    prop_assert_eq!(new.version(), Some(n.version.clone()));
                }
                other => {
                    return Err(TestCaseError::fail(format!("unexpected {other:?}")));
                }
            }
        }

        #[test]
        fn upd_017_is_fresh_matches_the_window(offset in -(40i64 * 86_400)..(40i64 * 86_400)) {
            // `offset` seconds after `now`: fresh from 24 h ahead to 14 days back.
            let offer = offer_confirmed_at(confirmed(offset));
            let expected = (-14 * 86_400..=86_400).contains(&offset);
            prop_assert_eq!(is_fresh(&offer, now()), expected, "offset {}", offset);
        }

        #[test]
        fn upd_017_is_fresh_never_panics(
            confirmed_at in ".{0,40}",
            secs in any::<u64>(),
            nanos in 0u32..1_000_000_000,
        ) {
            let offer = offer_confirmed_at(confirmed_at);
            let clock = UNIX_EPOCH.checked_add(Duration::new(secs / 2, nanos)).unwrap_or(UNIX_EPOCH);
            let _ = is_fresh(&offer, clock);
        }
    }
}
