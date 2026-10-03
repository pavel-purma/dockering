//! UpdateService tests (UPD-004, 005, 011) on a paused clock with an in-memory source.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use dk_update::InstallKind;
use dk_update::source::testing::StaticSource;
use futures::StreamExt;
use sha2::Digest;
use tempfile::TempDir;

use crate::config::{Config, UiState};
use crate::handle::{HubHandle, HubOptions};
use crate::hub::HubInner;
use crate::paths::Paths;
use crate::updates::{FIRST_CHECK, INTERVAL, JITTER_SECS, State, UpdateCheck, UpdateStatus};

struct Fixture {
    hub: HubHandle,
    source: Arc<StaticSource>,
    _dir: TempDir,
}

fn signed_manifest(kp: &minisign::KeyPair, version: &str, file: &[u8]) -> (Vec<u8>, String) {
    let key = dk_update::manifest::platform_key();
    let json = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "version": version,
        "pub_date": "2026-10-03T00:00:00Z",
        "notes_url": format!("https://github.com/pavel-purma/dockering/releases/tag/v{version}"),
        "platforms": { key: {
            "kind": "inno",
            "url": format!("https://github.com/pavel-purma/dockering/releases/download/v{version}/Dockering-Setup-x64.exe"),
            "sha256": hex::encode(sha2::Sha256::digest(file)),
            "size": file.len(),
        }},
    }))
    .expect("json");
    let sig = minisign::sign(None, &kp.sk, Cursor::new(&json), None, None)
        .expect("sign")
        .into_string();
    (json, sig)
}

/// Hub on the paused test runtime with a static update source. `kind` decides install vs notify.
fn start(config: Config, kind: InstallKind, demo: bool, version: &str) -> Fixture {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let file = b"installer bytes".to_vec();
    let (json, sig) = signed_manifest(&kp, version, &file);
    let source = Arc::new(StaticSource::new(json, sig, file));
    let mut state = State::new(source.clone(), kind, "0.1.0", vec![kp.pk.to_base64()]);
    // Unsigned test installer + unsigned test binary: the Authenticode pin is skipped.
    state.running_exe = None;
    let dir = tempfile::tempdir().expect("tempdir");
    let opts = HubOptions {
        paths: Paths::in_dir(dir.path()),
        config,
        ui_state: UiState::default(),
        factories: Some(vec![]),
        discover_on_start: false,
        worker_threads: 2,
        demo,
    };
    let hub = HubInner::start_with(opts, tokio::runtime::Handle::current(), None, Some(state));
    Fixture {
        hub,
        source,
        _dir: dir,
    }
}

async fn status_until(hub: &HubHandle, max: Duration, f: impl Fn(&UpdateStatus) -> bool) {
    let mut s = hub.update_status();
    tokio::time::timeout(max, async {
        while let Some(Ok(item)) = s.next().await {
            if f(&item) {
                return;
            }
        }
        panic!("status stream ended");
    })
    .await
    .expect("status not reached in time");
}

#[tokio::test(start_paused = true)]
async fn upd_004_first_check_after_30s_then_daily() {
    // `std::env` is process-wide; the disable switch must not leak in from the environment.
    if std::env::var_os(crate::updates::DISABLE_ENV).is_some() {
        return;
    }
    let f = start(Config::default(), InstallKind::Portable, false, "0.2.0");
    tokio::time::sleep(FIRST_CHECK - Duration::from_secs(1)).await;
    assert_eq!(f.source.requests(), 0, "no request before 30 s");
    tokio::time::sleep(Duration::from_secs(2)).await;
    status_until(&f.hub, Duration::from_secs(5), |s| {
        matches!(
            s,
            UpdateStatus::Available {
                notify_only: true,
                ..
            }
        )
    })
    .await;
    assert_eq!(f.source.requests(), 1);

    tokio::time::sleep(INTERVAL - Duration::from_secs(JITTER_SECS + 60)).await;
    assert_eq!(f.source.requests(), 1, "no second check before ~24 h");
    tokio::time::sleep(Duration::from_secs(2 * JITTER_SECS + 120)).await;
    assert_eq!(f.source.requests(), 2, "second check within 24 h ± 1 h");
}

#[tokio::test(start_paused = true)]
async fn upd_005_disabled_makes_no_requests() {
    let mut off = Config::default();
    off.updates.check = false;
    for (config, demo) in [(off, false), (Config::default(), true)] {
        let f = start(config, InstallKind::InnoUser, demo, "0.2.0");
        tokio::time::sleep(INTERVAL * 2).await;
        assert_eq!(f.source.requests(), 0);
        if demo {
            // Demo mode has no updater at all.
            assert_eq!(f.hub.check_for_updates().await, Ok(UpdateCheck::Disabled));
        }
        assert_eq!(f.source.requests(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn upd_007_manual_check_downloads_and_is_ready() {
    if std::env::var_os(crate::updates::DISABLE_ENV).is_some() {
        return;
    }
    let f = start(Config::default(), InstallKind::InnoUser, false, "0.2.0");
    assert_eq!(
        f.hub.check_for_updates().await,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
    status_until(&f.hub, Duration::from_secs(1), |s| {
        matches!(s, UpdateStatus::Ready { version, needs_elevation: false, .. } if version == "0.2.0")
    })
    .await;
    // A second check reuses the verified download.
    assert_eq!(
        f.hub.check_for_updates().await,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
    assert_eq!(f.source.requests(), 2, "manifest + one download");
}

#[tokio::test(start_paused = true)]
async fn upd_004_same_version_is_up_to_date() {
    if std::env::var_os(crate::updates::DISABLE_ENV).is_some() {
        return;
    }
    let f = start(Config::default(), InstallKind::InnoUser, false, "0.1.0");
    assert_eq!(f.hub.check_for_updates().await, Ok(UpdateCheck::UpToDate));
    status_until(&f.hub, Duration::from_secs(1), |s| {
        matches!(
            s,
            UpdateStatus::Idle {
                last_check: Some(_)
            }
        )
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn upd_011_stale_manual_check_dropped() {
    if std::env::var_os(crate::updates::DISABLE_ENV).is_some() {
        return;
    }
    let f = start(Config::default(), InstallKind::Portable, false, "0.2.0");
    // Hold the check lock so both requests are queued before either runs.
    let updates = f.hub.inner.updates.as_ref().expect("updater");
    let guard = updates.busy.lock().await;
    let first = f.hub.check_for_updates();
    let second = f.hub.check_for_updates();
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(guard);
    let (a, b) = futures::join!(first, second);
    assert_eq!(a, Err(dk_core::EngineError::Cancelled), "superseded check");
    assert_eq!(
        b,
        Ok(UpdateCheck::Available {
            version: "0.2.0".into()
        })
    );
}

#[tokio::test(start_paused = true)]
async fn upd_008_previous_version_and_notified_once() {
    let f = start(Config::default(), InstallKind::Portable, true, "0.2.0");
    f.hub
        .config()
        .update_ui_state(|s| s.updates.last_run_version = Some("0.0.1".into()));
    assert_eq!(f.hub.take_previous_version(), Some("0.0.1".into()));
    assert_eq!(f.hub.take_previous_version(), None, "only once");
    assert!(f.hub.mark_update_notified("0.3.0"));
    assert!(!f.hub.mark_update_notified("0.3.0"));
    assert!(f.hub.mark_update_notified("0.4.0"));
}
