//! The retained offer, the apply flag and the install strategy seam (UPD-006, 007, 011, 012,
//! 017) on the paused clock. The fixture lives in `updates_tests`.
//!
//! Sessions: a test can `shutdown` a hub and `launch` another one on the same fixture, which is
//! what the next start of the app sees on disk.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dk_core::EngineError;
use dk_update::InstallKind;
use dk_update::UpdateError;
use dk_update::manifest::platform_key;
use dk_update::offer::{self, OFFER_FILE, OfferStage as Stored, StoredOffer};

use crate::UpdateStatus;
use crate::config::Config;
use crate::handle::HubHandle;
use crate::hub::lock;
use crate::updates::fakes::Call;
use crate::updates::{ApplyResult, UpdateCheck};
use crate::updates_tests::{
    DAY, FILE, Fixture, SHORT, current, quiet, sha2_hex, signed_manifest_for, until,
    updater_env_off,
};
use crate::{ApplyOutcome, DisabledReason, OfferMode, OfferStage, UpdateOffer, UpdateSnapshot};

pub(crate) fn available(version: &str) -> Result<UpdateCheck, EngineError> {
    Ok(UpdateCheck::Available {
        version: version.to_owned(),
    })
}

pub(crate) fn protocol(message: &str) -> EngineError {
    EngineError::protocol(message)
}

pub(crate) fn the_offer(snap: &UpdateSnapshot) -> UpdateOffer {
    snap.offer.clone().expect("the snapshot has an offer")
}

pub(crate) fn is_ready(version: &'static str) -> impl Fn(&UpdateSnapshot) -> bool {
    move |s| {
        s.offer
            .as_ref()
            .is_some_and(|o| o.version == version && o.stage == OfferStage::Ready)
    }
}

/// What `offer.json` says now.
pub(crate) fn stored(f: &Fixture) -> Option<StoredOffer> {
    offer::read(&f.updates_dir()).expect("offer.json is readable")
}

fn stored_version(f: &Fixture) -> Option<String> {
    stored(f).and_then(|o| o.version()).map(|v| v.to_string())
}

pub(crate) fn offer_json(f: &Fixture) -> Vec<u8> {
    std::fs::read(f.updates_dir().join(OFFER_FILE)).expect("offer.json")
}

/// `latest` serves `version`; one manual check; waits until that version is *Ready*.
pub(crate) async fn offered(f: &Fixture, hub: &HubHandle, version: &'static str) -> UpdateSnapshot {
    f.publish(version);
    assert_eq!(hub.check_for_updates().await, available(version));
    until(hub, SHORT, is_ready(version)).await
}

/// Waits for the work the start of a hub (restore, cleanup) holds the check lock for.
pub(crate) async fn settled(hub: &HubHandle) {
    let updates = hub.inner.updates.as_ref().expect("updater");
    drop(updates.busy.lock().await);
}

/// Polls (virtual time) until `f` holds.
async fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what}");
}

fn spawn_check(hub: &HubHandle) -> tokio::task::JoinHandle<Result<UpdateCheck, EngineError>> {
    let hub = hub.clone();
    tokio::spawn(async move { hub.check_for_updates().await })
}

// ── UPD-011: apply outcomes and the apply flag ──────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_011_apply_outcome_restarting() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    let before = offer_json(&f);

    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    let job = f.strategy.last_job();
    assert_eq!(job.version, "0.2.0");
    assert_eq!(job.kind, InstallKind::InnoUser);
    assert!(!job.retry_start);
    assert_eq!(job.payload.as_deref(), Some(f.payload("0.2.0").as_path()));
    // A handoff leaves the offer unchanged, in memory and on disk (UPD-011, UPD-017).
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.stage, offer.mode),
        (OfferStage::Ready, OfferMode::Install)
    );
    assert_eq!(offer_json(&f), before);
}

#[tokio::test(start_paused = true)]
async fn upd_011_apply_outcome_installed_restart_manually() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let target = PathBuf::from("/home/me/Dockering.AppImage");
    *lock(&f.host.appimage) = Some(target.clone());
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    f.strategy.set(Ok(ApplyResult::InstalledRestartManually));
    assert_eq!(
        hub.apply_update().await,
        Ok(ApplyOutcome::InstalledRestartManually)
    );
    assert_eq!(
        f.strategy.last_job().target.as_deref(),
        Some(target.as_path())
    );
    // The app keeps running and the offer is in its *Installed, restart* stage, on disk too.
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.stage, offer.mode),
        (OfferStage::InstalledRestart, OfferMode::Install)
    );
    assert_eq!(
        stored(&f).expect("offer.json").stage,
        Stored::InstalledRestart
    );

    // The same call again only retries the start: no staging, no hashing.
    f.strategy.set(Ok(ApplyResult::Handoff));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert!(f.strategy.last_job().retry_start);

    // A restart without the user restarting the app still shows the stage.
    hub.shutdown();
    let hub = f.start();
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(offer.stage, OfferStage::InstalledRestart);
}

#[tokio::test(start_paused = true)]
async fn upd_011_second_apply_fails_fast() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    let release = f.strategy.hold();
    let first = hub.apply_update();
    f.strategy.entered(1).await;
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::Applying,
        "the stage is memory only but visible"
    );
    // The click that arrives while one runs fails at once and changes nothing.
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("already being applied"))
    );
    assert_eq!(f.strategy.calls(), 1);
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Applying);

    release.open();
    assert_eq!(first.await, Ok(ApplyOutcome::Restarting));
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
    // The persisted stage was never *Applying*.
    assert_eq!(stored(&f).expect("offer.json").stage, Stored::Ready);
}

#[tokio::test(start_paused = true)]
async fn upd_011_apply_during_recheck() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    // A re-check is in flight and holds the check lock ...
    let held = f.source.hold_manifest();
    let check = spawn_check(&hub);
    held.reached(1).await;
    let snap = current(&hub).await;
    assert_eq!(snap.status, UpdateStatus::Checking);
    assert_eq!(
        the_offer(&snap).stage,
        OfferStage::Ready,
        "checking never hides the offer"
    );

    // ... and the click neither waits for it nor fails because of it (UPD-011).
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert!(!check.is_finished());
    assert_eq!(f.strategy.calls(), 1);

    held.open();
    assert_eq!(check.await.expect("join"), available("0.2.0"));
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
}

#[tokio::test(start_paused = true)]
async fn upd_011_check_result_during_apply_is_reconciled_after() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    let release = f.strategy.hold();
    let apply = hub.apply_update();
    f.strategy.entered(1).await;

    // A check finishes while the apply runs: it downloads 0.3.0, but its result is held.
    f.publish("0.3.0");
    assert_eq!(hub.check_for_updates().await, available("0.3.0"));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.version.as_str(), offer.stage),
        ("0.2.0", OfferStage::Applying)
    );
    assert_eq!(
        stored_version(&f).as_deref(),
        Some("0.2.0"),
        "offer.json is untouched"
    );
    assert!(
        f.payload("0.2.0").exists(),
        "the payload being applied is never deleted"
    );
    assert!(
        f.payload("0.3.0").exists(),
        "the held download is kept (keep set)"
    );

    // When the apply ends the held result is reconciled.
    release.open();
    assert_eq!(apply.await, Ok(ApplyOutcome::Restarting));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.3.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert_eq!(stored_version(&f).as_deref(), Some("0.3.0"));
    eventually("the old payload is deleted after the swap", || {
        !f.payload("0.2.0").exists()
    })
    .await;
    assert!(f.payload("0.3.0").exists());
}

#[tokio::test(start_paused = true)]
async fn upd_011_apply_failure_does_not_degrade_newer_offer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    let release = f.strategy.hold();
    f.strategy
        .fail_install("the new version failed its self-test");
    let apply = hub.apply_update();
    f.strategy.entered(1).await;
    f.publish("0.3.0");
    assert_eq!(hub.check_for_updates().await, available("0.3.0"));

    release.open();
    assert_eq!(
        apply.await,
        Err(protocol("the new version failed its self-test"))
    );
    // The failure is recorded against 0.2.0 only: 0.3.0 is a fresh, installable offer.
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.3.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert!(!offer.install_failed);
    let on_disk = stored(&f).expect("offer.json");
    assert_eq!(
        on_disk.version().map(|v| v.to_string()).as_deref(),
        Some("0.3.0")
    );
    assert!(!on_disk.install_failed);
}

#[tokio::test(start_paused = true)]
async fn upd_011_stale_snapshot_dropped() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    let mut stream = hub.update_status();
    let mut seen = vec![
        futures::StreamExt::next(&mut stream)
            .await
            .expect("replay")
            .expect("no error"),
    ];
    offered(&f, &hub, "0.2.0").await;
    f.strategy.fail_retryable("disk full");
    assert!(hub.apply_update().await.is_err());
    hub.config().update(|c| c.updates.check = true);

    // Drain what the stream delivered: whatever it coalesced, revisions only go up.
    while let Ok(Some(Ok(item))) = tokio::time::timeout(
        Duration::from_millis(50),
        futures::StreamExt::next(&mut stream),
    )
    .await
    {
        seen.push(item);
    }
    assert!(seen.len() > 2, "the changes arrived: {seen:?}");
    let revisions: Vec<u64> = seen.iter().map(|s| s.revision).collect();
    assert!(
        revisions.windows(2).all(|w| w[0] < w[1]),
        "revisions increase strictly: {revisions:?}"
    );
    // A UI that drops anything not newer than what it has shows exactly the latest state.
    let mut shown = 0;
    for s in &seen {
        if s.revision > shown {
            shown = s.revision;
        }
    }
    // A new subscriber is replayed the current snapshot first, with the current revision.
    let replay = current(&hub).await;
    assert_eq!(replay.revision, shown);
    assert_eq!(&replay, seen.last().expect("last"));
}

#[tokio::test(start_paused = true)]
async fn upd_011_republished_entry_during_apply_does_not_touch_the_payload() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    let release = f.strategy.hold();
    let apply = hub.apply_update();
    f.strategy.entered(1).await;

    // The same version is served again with another hash: a different entry, but its payload
    // would land on the file that is being applied, so nothing is fetched until the apply ends.
    let (json, sig) = signed_manifest_for(&f.kp, "0.2.0", b"rebuilt", &platform_key(), "appimage");
    *lock(&f.source.inner.manifest) = Ok((json, sig));
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(
        f.source.downloads(),
        1,
        "no download over the payload being applied"
    );
    assert_eq!(std::fs::read(f.payload("0.2.0")).expect("payload"), FILE);
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Applying);

    release.open();
    assert_eq!(apply.await, Ok(ApplyOutcome::Restarting));
    // Reconciled: the new entry is the offer, and its payload is fetched (and rejected here:
    // this source still serves the old bytes), so it isn't actionable.
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(offer.version, "0.2.0");
    eventually(
        "the payload of the new entry is fetched after the apply",
        || f.source.downloads() == 2,
    )
    .await;
    assert_ne!(
        the_offer(&current(&hub).await).stage,
        OfferStage::Ready,
        "bytes that don't match the manifest are never offered"
    );
}

#[tokio::test(start_paused = true)]
async fn upd_011_apply_without_offer_is_refused() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install"))
    );
    assert_eq!(f.strategy.calls(), 0);
    // The flag was released: a later apply isn't "already being applied".
    offered(&f, &hub, "0.2.0").await;
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_011_panicking_strategy_is_an_error_and_leaves_the_offer_ready() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    f.strategy.panic_next();
    let Err(EngineError::Protocol(message)) = hub.apply_update().await else {
        panic!("a panic must become a protocol error");
    };
    assert!(message.contains("crashed"), "{message}");
    // Not stuck in *Applying*, and the flag is free again.
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

// ── UPD-006: the install kind decides the mode ──────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_006_deb_and_macos_get_download_offer() {
    if updater_env_off() {
        return;
    }
    // The platform override makes the macOS entry testable on every OS.
    for (platform, entry) in [("macos-aarch64", "dmg"), ("linux-x86_64", "deb")] {
        let f = Fixture::new(InstallKind::Other);
        *lock(&f.host.platform) = platform.to_owned();
        f.publish_entry("0.2.0", platform, entry);
        let hub = f.start();
        assert_eq!(
            hub.check_for_updates().await,
            available("0.2.0"),
            "{platform}"
        );
        let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
        assert_eq!(offer.mode, OfferMode::Download, "{platform}");
        assert_eq!(offer.stage, OfferStage::Ready, "found = actionable");
        assert!(!offer.needs_elevation && !offer.install_failed);
        assert_eq!(
            offer.notes_url,
            "https://github.com/pavel-purma/dockering/releases/tag/v0.2.0"
        );
        // A *Download* offer fetches the manifest and nothing else, and never installs.
        assert_eq!(
            (f.source.requests(), f.source.downloads()),
            (1, 0),
            "{platform}"
        );
        assert!(!f.updates_dir().join("0.2.0").exists(), "{platform}");
        assert!(hub.apply_update().await.is_err(), "{platform}");
        assert_eq!(f.strategy.calls(), 0, "{platform}");
        // The next check keeps it, still without a download.
        assert_eq!(hub.check_for_updates().await, available("0.2.0"));
        assert_eq!(
            (f.source.requests(), f.source.downloads()),
            (2, 0),
            "{platform}"
        );
        assert_eq!(stored(&f).expect("offer.json").stage, Stored::Ready);
    }
}

#[tokio::test(start_paused = true)]
async fn upd_006_lost_eligibility_downgrades_to_download() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    let offer = the_offer(&offered(&f, &hub, "0.2.0").await);
    assert_eq!(offer.mode, OfferMode::Install);

    // The AppImage was moved to a read-only place: the next check downgrades, never removes.
    f.host.set_kind(InstallKind::Other);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.version.as_str(), offer.mode),
        ("0.2.0", OfferMode::Download)
    );
    assert_eq!(offer.stage, OfferStage::Ready);
    assert_eq!(stored_version(&f).as_deref(), Some("0.2.0"));

    // Eligible again: the payload that is still on disk verifies, nothing is fetched.
    f.host.set_kind(InstallKind::AppImage);
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert_eq!(f.source.downloads(), 1);

    // Lost between the check and the click: the click is refused and the offer downgraded.
    f.host.set_kind(InstallKind::Other);
    let Err(EngineError::Protocol(message)) = hub.apply_update().await else {
        panic!("a lost eligibility must refuse the apply");
    };
    assert!(message.contains("release page"), "{message}");
    assert_eq!(f.strategy.calls(), 0);
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage),
        (OfferMode::Download, OfferStage::Ready)
    );
    // An AppImage whose real path can't be found is as good as lost.
    f.host.set_kind(InstallKind::AppImage);
    *lock(&f.host.appimage) = None;
    assert!(hub.apply_update().await.is_err());
    assert_eq!(f.strategy.calls(), 0);
}

// ── UPD-012: replacement order and cleanup ──────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_012_old_payload_kept_until_new_verified() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    // A newer release whose download fails: nothing about 0.2.0 changes.
    f.publish("0.3.0");
    f.source
        .fail_downloads(Some(UpdateError::Network("offline".into())));
    assert!(hub.check_for_updates().await.is_err());
    let snap = current(&hub).await;
    assert!(
        matches!(snap.status, UpdateStatus::Error { .. }),
        "{:?}",
        snap.status
    );
    assert_eq!(the_offer(&snap).version, "0.2.0");
    assert_eq!(the_offer(&snap).stage, OfferStage::Ready);
    assert_eq!(stored_version(&f).as_deref(), Some("0.2.0"));
    assert!(f.payload("0.2.0").exists());
    assert!(
        !f.payload("0.3.0").exists(),
        "no partial or unverified payload is left"
    );
    f.source.fail_downloads(None);

    // The download is slow: during it the old offer is still the offer, still actionable.
    let gate = f.source.hold_downloads();
    let check = spawn_check(&hub);
    gate.reached(1).await;
    let snap = current(&hub).await;
    assert!(
        matches!(&snap.status, UpdateStatus::Downloading { version, .. } if version == "0.3.0"),
        "{:?}",
        snap.status
    );
    assert_eq!(the_offer(&snap).version, "0.2.0");
    assert_eq!(the_offer(&snap).stage, OfferStage::Ready);
    assert_eq!(
        stored_version(&f).as_deref(),
        Some("0.2.0"),
        "offer.json is written after"
    );
    assert!(
        f.payload("0.2.0").exists(),
        "the old payload is kept until the new one verifies"
    );

    // Verified: offer.json is written, the offer swaps, then the old directory goes.
    gate.open();
    assert_eq!(check.await.expect("join"), available("0.3.0"));
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.3.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert_eq!(stored_version(&f).as_deref(), Some("0.3.0"));
    assert!(f.payload("0.3.0").exists());
    assert!(!f.payload("0.2.0").exists());
}

#[tokio::test(start_paused = true)]
async fn upd_012_cleanup_keeps_offer_json_and_keep_set() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();

    // What a crashed or older session can leave behind.
    let dir = f.updates_dir();
    let put = |path: &str, body: &str| {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, body).expect("write");
    };
    put("0.2.0/Dockering-x86_64.AppImage.part", "partial");
    put("0.1.5/Dockering-x86_64.AppImage", "old release");
    put("0.9.9/Dockering-x86_64.AppImage", "never offered");
    put("stray.part", "x");
    put("offer.json.tmp", "half a write");
    put("notes.txt", "not ours");
    let json = offer_json(&f);

    let hub = f.start();
    settled(&hub).await;
    assert_eq!(offer_json(&f), json, "cleanup never touches offer.json");
    assert!(
        f.payload("0.2.0").exists(),
        "the offered payload is in the keep set"
    );
    for gone in [
        "0.2.0/Dockering-x86_64.AppImage.part",
        "0.1.5",
        "0.9.9",
        "stray.part",
        "offer.json.tmp",
    ] {
        assert!(!dir.join(gone).exists(), "{gone} must be removed");
    }
    assert!(
        dir.join("notes.txt").exists(),
        "files that aren't ours stay"
    );
    assert_eq!(
        the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await).version,
        "0.2.0"
    );

    // After a check too, and `download()` no longer cleans up on its own: the same payload
    // survives a check that fetches a newer one.
    put("0.1.6/old.bin", "x");
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert!(!dir.join("0.1.6").exists());
    assert!(
        stored(&f).is_some(),
        "offer.json survives a check's cleanup"
    );
    assert!(f.payload("0.2.0").exists());
}

// ── UPD-017: the retained offer ─────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_017_restored_offer_visible_before_any_network() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();
    let before = (f.source.requests(), f.source.downloads());
    assert_eq!(before, (2, 1), "one manifest and one download");

    // The next start: the offer is shown from disk, with automatic checks on, before the first
    // check (30 s) could have run, and without a second download.
    let hub = f.launch().config(Config::default()).go();
    let offer = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(offer.mode, OfferMode::Install);
    assert_eq!(
        f.source.requests(),
        before.0,
        "no request before the restore"
    );

    // The first check only confirms it: a manifest, no asset.
    tokio::time::sleep(Duration::from_secs(31)).await;
    until(&hub, SHORT, |s| {
        matches!(
            s.status,
            UpdateStatus::Idle {
                last_check: Some(_)
            }
        )
    })
    .await;
    assert_eq!(f.source.requests(), before.0 + 1);
    assert_eq!(f.source.downloads(), 1);
    assert_eq!(the_offer(&current(&hub).await).stage, OfferStage::Ready);
}

#[tokio::test(start_paused = true)]
async fn upd_017_restored_offer_without_payload_waits_for_a_check() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();

    // The payload is corrupt: the offer comes back, but no button until it is fetched again.
    std::fs::write(f.payload("0.2.0"), b"damaged").expect("corrupt");
    let requests = f.source.requests();
    let hub = f.start();
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(
        (offer.version.as_str(), offer.stage),
        ("0.2.0", OfferStage::Downloading)
    );
    assert_eq!(f.source.requests(), requests, "restoring makes no request");
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install"))
    );

    // *Check now* (allowed with checks off) fetches it; it is *Ready* again.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert_eq!(f.source.downloads(), 2);
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_017_hard_disable_hides_button_keeps_file() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();
    let json = offer_json(&f);
    let dir = f.updates_dir();
    // Something cleanup would remove, to prove cleanup doesn't run either.
    std::fs::create_dir_all(dir.join("0.1.5")).expect("mkdir");
    let requests = f.source.requests();

    // No trust keys (like a policy, the env switch, demo or a build without the feature) fail
    // closed: no offer in the snapshot, no actions, no network ...
    let hub = f.launch().config(Config::default()).no_keys().go();
    tokio::time::sleep(DAY * 2).await;
    let snap = current(&hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Unavailable
        }
    );
    assert_eq!(snap.offer, None);
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::Disabled));
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("updates are disabled"))
    );
    assert_eq!(f.strategy.calls(), 0);
    assert_eq!(f.source.requests(), requests);
    // ... and the file, its payload and everything beside them stay untouched.
    assert_eq!(offer_json(&f), json);
    assert!(f.payload("0.2.0").exists());
    assert!(dir.join("0.1.5").exists());
    hub.shutdown();

    // Demo mode has no updater at all, and leaves the files alone too.
    let demo = f.launch().demo().go();
    assert_eq!(current(&demo).await, UpdateSnapshot::unavailable());
    assert_eq!(
        demo.apply_update().await,
        Err(protocol("updates are disabled"))
    );
    demo.shutdown();
    assert_eq!(offer_json(&f), json);

    // Lifting the disable brings the offer back, as it was.
    let hub = f.start();
    until(&hub, SHORT, is_ready("0.2.0")).await;
}

#[tokio::test(start_paused = true)]
async fn upd_017_setting_off_keeps_offer_check_now_and_apply() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.launch().config(Config::default()).go();
    offered(&f, &hub, "0.2.0").await;
    // Turning automatic checks off hides nothing and stops the schedule ...
    hub.config().update(|c| c.updates.check = false);
    let snap = until(&hub, SHORT, |s| {
        matches!(s.status, UpdateStatus::Disabled { .. })
    })
    .await;
    assert_eq!(
        snap.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting
        }
    );
    assert_eq!(the_offer(&snap).stage, OfferStage::Ready);
    let requests = f.source.requests();
    tokio::time::sleep(DAY * 3).await;
    assert_eq!(f.source.requests(), requests, "no automatic check");
    // ... while the explicit actions keep working.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_017_failed_check_keeps_offer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    let json = offer_json(&f);

    f.source
        .fail_manifest(UpdateError::Network("offline".into()));
    assert!(hub.check_for_updates().await.is_err());
    let snap = current(&hub).await;
    assert_eq!(
        snap.status,
        UpdateStatus::Error {
            message: "network error: offline".into()
        }
    );
    assert_eq!(
        the_offer(&snap).stage,
        OfferStage::Ready,
        "the error never replaces the offer"
    );
    assert_eq!(offer_json(&f), json);
    assert!(f.payload("0.2.0").exists());

    // A manifest signed by another key doesn't verify: a failed check too.
    let other = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let forged = signed_manifest_for(&other, "0.3.0", FILE, &platform_key(), "inno");
    *lock(&f.source.inner.manifest) = Ok(forged);
    assert!(hub.check_for_updates().await.is_err());
    assert_eq!(the_offer(&current(&hub).await).version, "0.2.0");
    assert_eq!(offer_json(&f), json);
}

#[tokio::test(start_paused = true)]
async fn upd_017_failed_install_not_retried_until_newer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    f.strategy
        .fail_install("the new version failed its self-test");
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("the new version failed its self-test"))
    );
    // The offer degrades to *Download* for that version.
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Download, OfferStage::Ready, true)
    );
    assert!(stored(&f).expect("offer.json").install_failed);
    // Never retried: another click is refused without running the strategy again.
    assert!(hub.apply_update().await.is_err());
    assert_eq!(f.strategy.calls(), 1);

    // The same entry confirmed again keeps the flag and fetches nothing.
    let downloads = f.source.downloads();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    assert_eq!(f.source.downloads(), downloads);
    // A restart remembers it.
    hub.shutdown();
    let hub = f.start();
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );

    // A newer version is a new chance.
    f.strategy.set(Ok(ApplyResult::Handoff));
    let offer = the_offer(&offered(&f, &hub, "0.3.0").await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Install, false)
    );
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_017_staging_errors_keep_the_offer_ready() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    // Disk or permission errors (and a declined UAC prompt) leave everything as it was.
    f.strategy
        .fail_retryable("couldn't write the update: no space left on device");
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(
            "couldn't write the update: no space left on device"
        ))
    );
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Install, OfferStage::Ready, false)
    );
    assert_eq!(stored(&f).expect("offer.json").stage, Stored::Ready);
    f.strategy.set(Ok(ApplyResult::Handoff));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_017_signed_exe_rejecting_installer_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    // At the check: the signed running exe rejects the unsigned installer.
    let f = Fixture::new(InstallKind::InnoUser);
    *lock(&f.host.trust) = Some(UpdateError::UntrustedInstaller);
    f.publish("0.2.0");
    let hub = f.launch().signed_exe().go();
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Download, OfferStage::Ready, true),
        "a *Download* offer with the reason, not a failed check"
    );
    assert!(
        !f.payload("0.2.0").exists(),
        "the untrusted installer is deleted"
    );
    assert!(hub.apply_update().await.is_err());
    assert_eq!(f.strategy.calls(), 0);
    // Confirming it again doesn't retry the download.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(f.source.downloads(), 1);

    // At the click: the verdict changed since the check. The pin runs inside the one locked
    // call (through the handle), which gets the running exe; the hub checks nothing itself.
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.launch().signed_exe().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    f.installers
        .script_locked(Err(UpdateError::UntrustedInstaller));
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&UpdateError::UntrustedInstaller.to_string()))
    );
    let calls = f.installers.calls();
    assert_eq!(
        calls.len(),
        1,
        "no retry for an untrusted installer: {calls:?}"
    );
    assert!(matches!(
        &calls[0],
        Call::Locked { running_exe: Some(exe), .. } if exe == Path::new("dockering.exe")
    ));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    assert!(stored(&f).expect("offer.json").install_failed);
}

#[tokio::test(start_paused = true)]
async fn upd_017_changed_payload_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    // The file sits in a user-writable directory: it is verified again at the click, through the
    // handle of the one locked call. A payload that changed is an `Integrity` error there.
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    f.installers.script_locked(Err(UpdateError::Integrity));
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&UpdateError::Integrity.to_string()))
    );
    let calls = f.installers.calls();
    assert_eq!(calls.len(), 1, "the hub doesn't hash on its own: {calls:?}");
    assert!(matches!(
        &calls[0],
        Call::Locked { sha256, size, .. }
            if sha256 == &sha2_hex(&f.file) && *size == f.file.len() as u64
    ));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    // Not retried: the next click is refused without a call.
    assert!(hub.apply_update().await.is_err());
    assert_eq!(f.installers.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn upd_017_apply_refuses_expired_offer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;

    // Just past 14 days, before the snapshot has had a chance to say so: apply decides on its own.
    tokio::time::sleep(DAY * 14 + Duration::from_millis(100)).await;
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("Check for updates first"))
    );
    assert_eq!(f.strategy.calls(), 0);
    // *Check now* confirms the offer again; then it applies.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

#[tokio::test(start_paused = true)]
async fn upd_017_expiry_hides_offer_in_snapshot_with_checks_off() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    let first = offered(&f, &hub, "0.2.0").await;

    // Almost 14 days offline with checks off: still offered.
    tokio::time::sleep(DAY * 14 - Duration::from_secs(3600)).await;
    assert!(current(&hub).await.offer.is_some());
    // It expires without any other event, and the stream says so.
    let gone = until(&hub, DAY, |s| s.offer.is_none()).await;
    assert!(gone.revision > first.revision);
    assert_eq!(
        gone.status,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting
        }
    );
    // The file stays for the next successful check; a restart doesn't resurrect it.
    assert!(stored(&f).is_some());
    hub.shutdown();
    let hub = f.start();
    settled(&hub).await;
    assert_eq!(current(&hub).await.offer, None);
    assert!(
        stored(&f).is_none(),
        "an expired file is deleted at the next start"
    );
}

#[tokio::test(start_paused = true)]
async fn upd_017_withdrawn_release_clears_offer_and_payload() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::AppImage);
    let hub = f.start();
    offered(&f, &hub, "0.3.0").await;

    // GitHub's `latest` now serves an older release (0.3.0 was pulled): the offer is replaced,
    // and the old one is not actionable at once.
    f.publish("0.2.0");
    let gate = f.source.hold_downloads();
    let check = spawn_check(&hub);
    gate.reached(1).await;
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.version.as_str(), offer.stage),
        ("0.2.0", OfferStage::Downloading)
    );
    assert_eq!(stored_version(&f).as_deref(), Some("0.2.0"));
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install")),
        "the pulled release is never applied"
    );
    assert_eq!(f.strategy.calls(), 0);
    gate.open();
    assert_eq!(check.await.expect("join"), available("0.2.0"));
    until(&hub, SHORT, is_ready("0.2.0")).await;
    assert!(
        !f.payload("0.3.0").exists(),
        "the pulled release's payload is gone"
    );
    assert!(f.payload("0.2.0").exists());

    // `latest` falls back to a manifest with no entry for this platform: nothing is offered.
    f.publish_without_entry("0.2.0");
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    assert_eq!(current(&hub).await.offer, None);
    assert!(stored(&f).is_none(), "offer.json is removed");
    assert!(
        !f.updates_dir().join("0.2.0").exists(),
        "and so is the payload"
    );

    // ... or to the version that is running: the same.
    offered(&f, &hub, "0.2.0").await;
    f.publish("0.1.0");
    assert_eq!(hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    assert_eq!(current(&hub).await.offer, None);
    assert!(stored(&f).is_none());
    assert!(!f.updates_dir().join("0.2.0").exists());
}

#[tokio::test(start_paused = true)]
async fn upd_017_catching_up_deletes_offer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();
    assert!(f.payload("0.2.0").exists());
    let requests = f.source.requests();

    // The user installed 0.2.0 by hand: the next start runs it.
    let hub = f.launch().running("0.2.0").go();
    settled(&hub).await;
    assert_eq!(current(&hub).await.offer, None);
    assert!(stored(&f).is_none(), "offer.json is deleted");
    assert!(
        !f.updates_dir().join("0.2.0").exists(),
        "and so is its payload"
    );
    assert_eq!(f.source.requests(), requests);
}

#[tokio::test(start_paused = true)]
async fn upd_017_restore_rejects_a_forged_offer_file_and_deletes_it() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.start();
    offered(&f, &hub, "0.2.0").await;
    hub.shutdown();
    // An offer.json whose signature isn't from a trusted key is not restored (UPD-017 (1)),
    // and isn't worth keeping.
    let mut offer = stored(&f).expect("offer.json");
    let other = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    offer.signature = signed_manifest_for(&other, "0.2.0", FILE, &platform_key(), "inno").1;
    offer::write(&f.updates_dir(), &offer).expect("write");
    let hub = f.start();
    settled(&hub).await;
    assert_eq!(current(&hub).await.offer, None);
    assert!(stored(&f).is_none());
}

// ── UPD-007: a cancelled installer ──────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_007_cancelled_installer_leaves_offer() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoMachine);
    let hub = f.start();
    let offer = the_offer(&offered(&f, &hub, "0.2.0").await);
    assert!(offer.needs_elevation, "an all-users install asks for UAC");
    let before = offer_json(&f);

    // The installer started (a handoff) and the user cancelled it: the old app is still here
    // with the same offer, on disk and in memory ...
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(the_offer(&current(&hub).await), offer);
    assert_eq!(offer_json(&f), before);
    // ... and so is the next launch of the old version, which offers it again.
    hub.shutdown();
    let hub = f.start();
    let again = the_offer(&until(&hub, SHORT, is_ready("0.2.0")).await);
    assert_eq!(again, offer);
    assert_eq!(offer_json(&f), before);
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(f.strategy.calls(), 2);
}

#[test]
fn quiet_config_turns_automatic_checks_off() {
    assert!(!quiet().updates.check);
    assert!(Config::default().updates.check);
}
