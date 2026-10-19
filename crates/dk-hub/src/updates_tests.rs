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
use crate::updates::{
    FIRST_CHECK, INTERVAL, JITTER_SECS, State, UpdateCheck, UpdateStatus, next_delay,
};

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

/// A test state on a static source; `kind` decides install vs notify.
fn test_state(kind: InstallKind, version: &str) -> (State, Arc<StaticSource>) {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
    let file = b"installer bytes".to_vec();
    let (json, sig) = signed_manifest(&kp, version, &file);
    let source = Arc::new(StaticSource::new(json, sig, file));
    let mut state = State::new(source.clone(), kind, "0.1.0", vec![kp.pk.to_base64()]);
    // Unsigned test installer + unsigned test binary: the Authenticode pin is skipped.
    state.running_exe = None;
    (state, source)
}

/// Hub on the paused test runtime with a static update source. `kind` decides install vs notify.
fn start(config: Config, kind: InstallKind, demo: bool, version: &str) -> Fixture {
    let (state, source) = test_state(kind, version);
    launch(config, demo, state, source)
}

fn launch(config: Config, demo: bool, state: State, source: Arc<StaticSource>) -> Fixture {
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
    // The manifest is fetched again (a newer release could replace the pending one), but the
    // verified download is reused.
    assert_eq!(f.source.requests(), 3, "2 manifests + one download");

    // Turning automatic checks off doesn't hide an update that's ready (UPD-007).
    f.hub.config().update(|c| c.updates.check = false);
    status_until(&f.hub, Duration::from_secs(1), |s| {
        matches!(s, UpdateStatus::Ready { .. })
    })
    .await;
    assert_eq!(
        f.hub.config().ui_state().updates.pending_version.as_deref(),
        Some("0.2.0")
    );
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

#[test]
fn upd_004_next_delay_stays_within_the_jitter_window() {
    let day = Duration::from_secs(24 * 3600);
    for r in [
        0,
        1,
        7,
        3599,
        3600,
        3601,
        7200,
        7201,
        u64::MAX / 2,
        u64::MAX - 1,
        u64::MAX,
    ] {
        let d = next_delay(INTERVAL, JITTER_SECS, r);
        assert!(
            d >= day - Duration::from_secs(JITTER_SECS)
                && d <= day + Duration::from_secs(JITTER_SECS),
            "r={r}: {d:?}"
        );
    }
    // The extremes are reachable and the middle is the plain interval.
    assert_eq!(
        next_delay(INTERVAL, JITTER_SECS, 0),
        day - Duration::from_secs(3600)
    );
    assert_eq!(next_delay(INTERVAL, JITTER_SECS, 3600), day);
    assert_eq!(
        next_delay(INTERVAL, JITTER_SECS, 7200),
        day + Duration::from_secs(3600)
    );
    // No jitter: exactly the interval, whatever the random number.
    for r in [0, 1, u64::MAX] {
        assert_eq!(
            next_delay(Duration::from_secs(10), 0, r),
            Duration::from_secs(10)
        );
    }
    // A short interval with a long jitter, or absurd inputs, must not panic or underflow.
    assert_eq!(next_delay(Duration::from_secs(10), 3600, 0), Duration::ZERO);
    let _ = next_delay(Duration::MAX, u64::MAX, u64::MAX);
    let _ = next_delay(Duration::ZERO, u64::MAX, 0);
}

#[tokio::test(start_paused = true)]
async fn upd_004_schedule_fields_drive_first_check_and_interval() {
    if std::env::var_os(crate::updates::DISABLE_ENV).is_some() {
        return;
    }
    let (mut state, source) = test_state(InstallKind::Portable, "0.2.0");
    state.first_check = Duration::from_secs(5);
    state.interval = Duration::from_secs(20);
    state.jitter_secs = 0;
    let f = launch(Config::default(), false, state, source);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        f.source.requests(),
        0,
        "nothing before the first-check delay"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    status_until(&f.hub, Duration::from_secs(1), |s| {
        matches!(s, UpdateStatus::Available { .. })
    })
    .await;
    assert_eq!(f.source.requests(), 1, "first check at 5 s");
    tokio::time::sleep(Duration::from_secs(18)).await;
    assert_eq!(f.source.requests(), 1, "next one 20 s after the first");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.source.requests(), 2);
}

fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| map.get(name).cloned()
}

fn embedded_keys() -> Vec<String> {
    dk_update::keys::PUBLIC_KEYS
        .iter()
        .map(|k| (*k).to_owned())
        .collect()
}

/// The state production code starts with, built from a lookup that knows nothing.
fn assert_production_defaults(state: &State) {
    assert_eq!(state.keys, embedded_keys());
    assert_eq!(state.first_check, FIRST_CHECK);
    assert_eq!(state.interval, INTERVAL);
    assert_eq!(state.jitter_secs, JITTER_SECS);
    assert_eq!(
        state.current,
        semver::Version::parse(crate::app_version()).expect("app version is semver")
    );
}

#[test]
fn upd_005_e2e_seam_inert_without_env() {
    let state = State::production_with(&env_map(&[])).expect("production state");
    assert_production_defaults(&state);
    // Empty and whitespace-only values are as good as unset.
    let blank = env_map(&[
        ("DOCKERING_UPDATE_E2E_URL", ""),
        ("DOCKERING_UPDATE_E2E_KEY", "  "),
        ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", ""),
        ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", " "),
        ("DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS", ""),
    ]);
    assert_production_defaults(&State::production_with(&blank).expect("production state"));
}

/// Without the Cargo feature the variables are not even looked at.
#[cfg(not(feature = "update-e2e"))]
#[test]
fn upd_005_e2e_seam_ignored_without_the_feature() {
    let all = env_map(&[
        ("DOCKERING_UPDATE_E2E_URL", "http://127.0.0.1:9"),
        ("DOCKERING_UPDATE_E2E_KEY", "RWQ0123456789abcdef"),
        ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", "1"),
        ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", "2"),
        ("DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS", "3"),
    ]);
    assert_production_defaults(&State::production_with(&all).expect("production state"));
}

#[cfg(feature = "update-e2e")]
mod seam {
    use dk_update::source::testing::{LoopbackServer, Reply};

    use super::*;

    fn server_path(url: &str) -> String {
        url.trim_start_matches("https://github.com").to_owned()
    }

    #[tokio::test]
    async fn upd_005_e2e_seam_applies_injected_env() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
        let (manifest, signature) = signed_manifest(&kp, "0.2.0", b"installer bytes");
        let server = LoopbackServer::start(vec![
            (
                server_path(dk_update::MANIFEST_URL),
                Reply::Body(manifest.clone()),
            ),
            (
                server_path(dk_update::SIGNATURE_URL),
                Reply::Body(signature.clone().into_bytes()),
            ),
        ])
        .expect("loopback server");
        let public_key = kp.pk.to_base64();
        let env = env_map(&[
            ("DOCKERING_UPDATE_E2E_URL", &server.base_url()),
            ("DOCKERING_UPDATE_E2E_KEY", &public_key),
            ("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", "1"),
            ("DOCKERING_UPDATE_E2E_INTERVAL_SECS", "2"),
        ]);
        let state = State::production_with(&env).expect("seam state");
        assert_eq!(state.keys, vec![public_key.clone()]);
        assert_eq!(state.first_check, Duration::from_secs(1));
        assert_eq!(state.interval, Duration::from_secs(2));
        assert_eq!(
            state.jitter_secs, 0,
            "a seconds-scale interval carries no jitter"
        );

        // The real client fetches the signed manifest from the loopback server ...
        let (json, sig) = state.source.fetch_manifest().await.expect("fetch");
        assert_eq!((&json, &sig), (&manifest, &signature));
        // ... the seam key verifies it, and the embedded keys don't.
        let keys: Vec<&str> = state.keys.iter().map(String::as_str).collect();
        dk_update::verify::verify_manifest(&json, &sig, &keys).expect("signed by the seam key");
        let embedded = embedded_keys();
        let embedded: Vec<&str> = embedded.iter().map(String::as_str).collect();
        assert!(dk_update::verify::verify_manifest(&json, &sig, &embedded).is_err());
        // The User-Agent carries app_version() (UPD-010).
        let expected_agent = format!(
            "Dockering/{} ({}; {})",
            crate::app_version(),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|r| r.header("user-agent") == Some(expected_agent.as_str()))
        );
    }

    #[test]
    fn upd_005_e2e_seam_unusable_base_fails_closed() {
        // An explicit but invalid base must not fall back to talking to GitHub.
        let env = env_map(&[("DOCKERING_UPDATE_E2E_URL", "not a url")]);
        assert!(State::production_with(&env).is_none());
    }
}
