//! The platform strategies through the hub (UPD-006, 007, 012, 014): the AppImage apply against
//! **real** `dk-update` calls on temp directories (Linux), the Windows installer's fallbacks and
//! the AppImage error mapping against faked `dk-update` calls (every OS), and the start-up staging
//! sweep. The fixture lives in `updates_tests`, the offer helpers in `updates_offer_tests`.
//!
//! # The "AppImages" of the real tests
//!
//! `dk-update` only replaces a file that has an AppImage header (ELF magic and `AI\x02` at offset
//! 8, UPD-013), so a shell script can't stand in for one. The stand-in is a **real ELF**: a tiny
//! Rust program compiled once per test process with the toolchain that builds the tests, with the
//! marker patched into `e_ident` (the bytes the kernel ignores, as for a real AppImage). Per
//! file, a text trailer after the ELF data says what the program is: its version (`--version`
//! prints `dockering <version>`), whether the relaunch dies at once, and where the relaunch writes
//! its marker (`--update-relaunch=1`: its environment, then wait for end-of-file on stdin).
//! Nothing touches the network.

use std::path::{Path, PathBuf};

use dk_update::InstallKind;
use dk_update::UpdateError;
use dk_update::apply::AppImageOutcome;
use dk_update::offer::OfferStage as Stored;

use crate::hub::lock;
use crate::updates::Host;
use crate::updates::fakes::{Call, fake_env};
use crate::updates_offer_tests::{
    available, offer_json, offered, protocol, settled, stored, the_offer,
};
use crate::updates_tests::{Fixture, SHORT, current, sha2_hex, until, updater_env_off};
use crate::{ApplyOutcome, OfferMode, OfferStage};

#[cfg(target_os = "linux")]
pub(crate) mod stub {
    //! The compiled stand-in AppImage (see the module documentation above).

    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    const SOURCE: &str = r#"
use std::io::Read;

fn field(text: &str, key: &str) -> String {
    text.lines().rev().find_map(|l| l.strip_prefix(key)).unwrap_or("").to_owned()
}

fn main() {
    let me = std::fs::read(std::env::current_exe().expect("exe")).expect("read self");
    let tail = String::from_utf8_lossy(&me[me.len().saturating_sub(2048)..]).into_owned();
    let version = field(&tail, "DK-STUB-VERSION=");
    let marker = field(&tail, "DK-STUB-MARKER=");
    let relaunch = field(&tail, "DK-STUB-RELAUNCH=");
    match std::env::args().nth(1).as_deref() {
        Some("--version") => println!("dockering {version}"),
        Some("--update-relaunch=1") => {
            if relaunch == "die" {
                std::process::exit(3);
            }
            let mut body = format!("version={version}\n");
            for (k, v) in std::env::vars_os() {
                body.push_str(&format!("{}={}\n", k.to_string_lossy(), v.to_string_lossy()));
            }
            if !marker.is_empty() {
                std::fs::write(&marker, body).expect("marker");
            }
            let mut sink = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut sink);
        }
        _ => {}
    }
}
"#;

    static BINARY: OnceLock<Vec<u8>> = OnceLock::new();

    /// The ELF, compiled on first use (about a second) and patched to look like a type 2 AppImage.
    fn binary() -> &'static [u8] {
        BINARY.get_or_init(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let source = dir.path().join("stub.rs");
            let out = dir.path().join("stub");
            fs::write(&source, SOURCE).expect("write the stub's source");
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let built = Command::new(&rustc)
                .args([
                    "--edition",
                    "2021",
                    "-C",
                    "opt-level=1",
                    "-C",
                    "debuginfo=0",
                ])
                .arg("-o")
                .arg(&out)
                .arg(&source)
                .output()
                .unwrap_or_else(|e| {
                    panic!("can't run {rustc:?} to build the stand-in AppImage: {e}")
                });
            assert!(
                built.status.success(),
                "building the stand-in AppImage failed:\n{}",
                String::from_utf8_lossy(&built.stderr)
            );
            let mut bytes = fs::read(&out).expect("the compiled stub");
            // `AI\x02` at offset 8 (UPD-013): padding in `e_ident`, ignored by the kernel.
            bytes[8..11].copy_from_slice(b"AI\x02");
            bytes
        })
    }

    /// A stand-in AppImage of `version`. `marker` is where its relaunch records itself; with
    /// `relaunch_dies` the relaunch exits at once (the new process "crashes").
    pub(crate) fn appimage(version: &str, marker: Option<&Path>, relaunch_dies: bool) -> Vec<u8> {
        let mut bytes = binary().to_vec();
        bytes.extend_from_slice(format!("\nDK-STUB-VERSION={version}\n").as_bytes());
        if let Some(marker) = marker {
            bytes.extend_from_slice(format!("DK-STUB-MARKER={}\n", marker.display()).as_bytes());
        }
        if relaunch_dies {
            bytes.extend_from_slice(b"DK-STUB-RELAUNCH=die\n");
        }
        bytes
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────

/// An AppImage install whose target is `<tmp>/app/Dockering.AppImage` (the host seam says so).
fn appimage_fixture(payload: impl FnOnce(&Path) -> Vec<u8>) -> (Fixture, PathBuf) {
    let f = Fixture::with_payload(InstallKind::AppImage, payload);
    let app = f.dir.path().join("app");
    std::fs::create_dir_all(&app).expect("mkdir");
    let target = app.join("Dockering.AppImage");
    *lock(&f.host.appimage) = Some(target.clone());
    (f, target)
}

#[cfg(target_os = "linux")]
fn write_executable(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    // Through a temp name and a `rename`, like the real swap: no `ETXTBSY` on a file that was run.
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, bytes).expect("write");
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::rename(&tmp, path).expect("rename");
}

/// Entries of the directory of `target`.
#[cfg(target_os = "linux")]
fn listing(target: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(target.parent().expect("dir"))
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ── UPD-006 ─────────────────────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_006_linux_appimage_downloads_and_is_ready() {
    if updater_env_off() {
        return;
    }
    // The host seam says: an AppImage in a writable place. The hub fetches the payload (and only
    // for such an install) and the offer is installable.
    let (f, target) = appimage_fixture(|_| b"appimage bytes".to_vec());
    std::fs::write(&target, b"the installed app").expect("target");
    let hub = f.start();
    f.publish("0.2.0");
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    let offer = the_offer(&until(&hub, SHORT, |s| s.offer.is_some()).await);
    assert_eq!(
        (
            offer.mode,
            offer.stage,
            offer.needs_elevation,
            offer.install_failed
        ),
        (OfferMode::Install, OfferStage::Ready, false, false)
    );
    assert_eq!((f.source.requests(), f.source.downloads()), (2, 1));
    // The payload is the verified asset, in `<updates>/<version>/`, and the target is untouched.
    assert_eq!(
        std::fs::read(f.payload("0.2.0")).expect("payload"),
        b"appimage bytes"
    );
    assert_eq!(
        std::fs::read(&target).expect("target"),
        b"the installed app"
    );
    assert_eq!(stored(&f).expect("offer.json").stage, Stored::Ready);
    // A second check reuses it.
    assert_eq!(hub.check_for_updates().await, available("0.2.0"));
    assert_eq!(f.source.downloads(), 1);
}

// ── UPD-014: the real AppImage apply through the hub (Linux) ────────────────────────────────

/// An AppImage install on temp directories whose releases serve a stand-in AppImage of
/// `payload_version`, and whose installed file is a stand-in of version 0.1.0.
#[cfg(target_os = "linux")]
struct Real {
    f: Fixture,
    target: PathBuf,
    /// The installed file's bytes before the apply.
    old: Vec<u8>,
    /// Where a relaunched stand-in records itself.
    marker: PathBuf,
}

#[cfg(target_os = "linux")]
fn real(payload_version: &'static str, relaunch_dies: bool) -> Real {
    let (f, target) = appimage_fixture(|dir| {
        let marker = dir.join("out").join("relaunch.marker");
        stub::appimage(payload_version, Some(&marker), relaunch_dies)
    });
    std::fs::create_dir_all(f.dir.path().join("out")).expect("mkdir");
    let old = stub::appimage("0.1.0", None, false);
    write_executable(&target, &old);
    let marker = f.dir.path().join("out").join("relaunch.marker");
    Real {
        f,
        target,
        old,
        marker,
    }
}

/// Waits in real time (a blocking worker: the paused clock can't skip it) for a file that
/// another process writes.
#[cfg(target_os = "linux")]
async fn wait_for_file(path: &Path) {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        for _ in 0..500 {
            if path.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{} never appeared", path.display());
    })
    .await
    .expect("join");
}

#[cfg(target_os = "linux")]
#[test]
fn upd_013_the_stand_in_passes_the_appimage_probe() {
    use dk_update::install_kind::{AppImageProbe, FsProbe};
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("Dockering.AppImage");
    write_executable(&path, &stub::appimage("1.2.3", None, false));
    assert!(FsProbe.is_appimage_file(&path), "ELF magic and `AI\\x02`");
    // Another test's fork can hold the just-written file's descriptor for a moment (ETXTBSY).
    let mut attempts = 0;
    let output = loop {
        match std::process::Command::new(&path).arg("--version").output() {
            Err(error) if error.raw_os_error() == Some(26) && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => break result.expect("the stand-in runs"),
        }
    };
    assert_eq!(String::from_utf8_lossy(&output.stdout), "dockering 1.2.3\n");
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_apply_replaces_target_and_returns_restarting() {
    use std::os::unix::fs::PermissionsExt;
    if updater_env_off() {
        return;
    }
    let r = real("0.2.0", false);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;
    let json = offer_json(&r.f);
    assert_eq!(std::fs::read(&r.target).expect("target"), r.old);

    // The REAL apply: stage next to the target, self-test, `rename`, relaunch.
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    assert_eq!(
        std::fs::read(&r.target).expect("target"),
        r.f.file,
        "the target is now the payload"
    );
    assert_eq!(
        std::fs::metadata(&r.target)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777,
        0o755,
        "the target's mode is kept"
    );
    assert_eq!(
        listing(&r.target),
        ["Dockering.AppImage"],
        "no staging file is left"
    );
    // A handoff leaves the offer as it was, in memory and on disk (UPD-011).
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (
            offer.version.as_str(),
            offer.mode,
            offer.stage,
            offer.install_failed
        ),
        ("0.2.0", OfferMode::Install, OfferStage::Ready, false)
    );
    assert_eq!(offer_json(&r.f), json);
    // The new process was started with the full environment of this one (the scrub removes
    // only the AppImage's own variables, which a plain test run doesn't have).
    wait_for_file(&r.marker).await;
    let marker = std::fs::read_to_string(&r.marker).expect("marker");
    assert!(marker.starts_with("version=0.2.0\n"), "{marker}");
    if std::env::var_os("APPDIR").is_none() {
        for name in ["HOME", "PATH"] {
            if let Some(value) = std::env::var_os(name) {
                let line = format!("{name}={}", value.to_string_lossy());
                assert!(
                    marker.lines().any(|l| l == line),
                    "{name} reached the child"
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_self_test_failure_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    // The manifest says 0.2.0, but the payload answers `--version` as 9.9.9.
    let r = real("9.9.9", false);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;

    let Err(dk_core::EngineError::Protocol(message)) = hub.apply_update().await else {
        panic!("a failed self-test must fail the apply");
    };
    assert!(message.contains("self-test"), "{message}");
    // The old file is byte-identical and nothing was left behind or started.
    assert_eq!(std::fs::read(&r.target).expect("target"), r.old);
    assert_eq!(listing(&r.target), ["Dockering.AppImage"]);
    assert!(!r.marker.exists());
    // The offer is a *Download* one for that version, on disk too, and it isn't retried.
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Download, OfferStage::Ready, true)
    );
    assert!(stored(&r.f).expect("offer.json").install_failed);
    assert_eq!(
        hub.apply_update().await,
        Err(protocol("no update is ready to install"))
    );
    assert_eq!(std::fs::read(&r.target).expect("target"), r.old);
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_changed_payload_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    let r = real("0.2.0", false);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;
    // The payload sits in a user-writable directory: one flipped byte after the check.
    let mut bytes = std::fs::read(r.f.payload("0.2.0")).expect("payload");
    *bytes.last_mut().expect("not empty") ^= 0xff;
    std::fs::write(r.f.payload("0.2.0"), bytes).expect("tamper");

    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&UpdateError::Integrity.to_string())),
        "a changed payload is an integrity error, whoever notices"
    );
    assert_eq!(std::fs::read(&r.target).expect("target"), r.old);
    assert_eq!(listing(&r.target), ["Dockering.AppImage"]);
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_target_that_stopped_being_an_appimage_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    let r = real("0.2.0", false);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;
    // Something else took the AppImage's place: `dk-update` refuses to replace it.
    std::fs::write(&r.target, b"#!/bin/sh\necho not an AppImage\n").expect("replace");
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&UpdateError::Unsupported.to_string()))
    );
    assert_eq!(
        std::fs::read(&r.target).expect("target"),
        b"#!/bin/sh\necho not an AppImage\n"
    );
    assert_eq!(listing(&r.target), ["Dockering.AppImage"]);
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_read_only_target_dir_degrades_to_download() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if updater_env_off() {
        return;
    }
    // root ignores permission bits: nothing to prove there.
    if std::fs::metadata("/proc/self").expect("stat").uid() == 0 {
        return;
    }
    let r = real("0.2.0", false);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;
    let dir = r.target.parent().expect("dir").to_path_buf();
    // Put the permissions back whatever happens, so the temp dir can be removed.
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let _restore = Restore(dir.clone());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");

    // A directory the app can no longer write to is a lost eligibility (UPD-013): *Download*.
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&UpdateError::Unsupported.to_string()))
    );
    assert_eq!(std::fs::read(&r.target).expect("target"), r.old);
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn upd_014_hub_installed_restart_retry_starts_the_installed_file_only() {
    if updater_env_off() {
        return;
    }
    // The new process dies at once: installed, but not started.
    let r = real("0.2.0", true);
    let hub = r.f.launch().system_on_real().go();
    offered(&r.f, &hub, "0.2.0").await;
    assert_eq!(
        hub.apply_update().await,
        Ok(ApplyOutcome::InstalledRestartManually)
    );
    assert_eq!(
        std::fs::read(&r.target).expect("target"),
        r.f.file,
        "it is installed"
    );
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::InstalledRestart
    );
    assert_eq!(
        stored(&r.f).expect("offer.json").stage,
        Stored::InstalledRestart
    );

    // The user fixes what was wrong; the installed file starts now. The payload is gone: the
    // retry must not stage or hash anything, it only restarts the installed file.
    let fixed = stub::appimage("0.2.0", Some(&r.marker), false);
    write_executable(&r.target, &fixed);
    std::fs::remove_file(r.f.payload("0.2.0")).expect("remove the payload");
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    wait_for_file(&r.marker).await;
    assert_eq!(
        std::fs::read(&r.target).expect("target"),
        fixed,
        "nothing was staged over it"
    );
    assert_eq!(listing(&r.target), ["Dockering.AppImage"]);
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::InstalledRestart
    );
}

// ── UPD-014 through the strategy on faked `dk-update` calls (every OS) ──────────────────────

#[tokio::test(start_paused = true)]
async fn upd_014_hub_stage_error_keeps_ready() {
    if updater_env_off() {
        return;
    }
    let (f, target) = appimage_fixture(|_| crate::updates_tests::FILE.to_vec());
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    let json = offer_json(&f);

    // Disk full while staging: nothing changed, the user can retry.
    f.installers
        .script_appimage(Err(UpdateError::Stage("no space left on device".into())));
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
    assert!(!stored(&f).expect("offer.json").install_failed);
    assert_eq!(offer_json(&f), json);

    // The retry works, with the arguments the plan names.
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    let call = Call::AppImage {
        payload: f.payload("0.2.0"),
        target,
        version: "0.2.0".to_owned(),
        sha256: sha2_hex(&f.file),
        size: f.file.len() as u64,
        env: fake_env(),
    };
    assert_eq!(f.installers.calls(), [call.clone(), call]);
}

#[tokio::test(start_paused = true)]
async fn upd_014_hub_installed_restart_retry() {
    if updater_env_off() {
        return;
    }
    let (f, target) = appimage_fixture(|_| crate::updates_tests::FILE.to_vec());
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;

    f.installers
        .script_appimage(Ok(AppImageOutcome::InstalledRestartManually(
            "the new process died".into(),
        )));
    assert_eq!(
        hub.apply_update().await,
        Ok(ApplyOutcome::InstalledRestartManually)
    );
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::InstalledRestart
    );
    assert_eq!(
        stored(&f).expect("offer.json").stage,
        Stored::InstalledRestart
    );

    // The click in that stage only restarts the installed file: `restart_installed`, no
    // `apply_appimage`, and the payload isn't needed (it may be gone).
    std::fs::remove_file(f.payload("0.2.0")).expect("remove the payload");
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    let calls = f.installers.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(matches!(calls[0], Call::AppImage { .. }));
    assert_eq!(
        calls[1],
        Call::Restart {
            target: target.clone(),
            version: "0.2.0".to_owned(),
            env: fake_env(),
        }
    );
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::InstalledRestart
    );

    // It can fail to start again without losing the stage ...
    f.installers
        .script_restart(Ok(AppImageOutcome::InstalledRestartManually(
            "still no".into(),
        )));
    assert_eq!(
        hub.apply_update().await,
        Ok(ApplyOutcome::InstalledRestartManually)
    );
    assert_eq!(
        the_offer(&current(&hub).await).stage,
        OfferStage::InstalledRestart
    );
    // ... but an installed file that no longer answers `--version` is a failed install.
    f.installers
        .script_restart(Err(UpdateError::SelfTest("no output".into())));
    assert!(hub.apply_update().await.is_err());
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    assert_eq!(f.installers.calls().len(), 4);
}

// ── UPD-007 through the strategy on faked `dk-update` calls (every OS) ──────────────────────

#[tokio::test(start_paused = true)]
async fn upd_007_hub_locked_start_failed_all_users_install_degrades_to_download() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoMachine);
    let hub = f.launch().system_on_fakes().go();
    let offer = the_offer(&offered(&f, &hub, "0.2.0").await);
    assert!(offer.needs_elevation);

    let locked = UpdateError::LockedStartFailed("sharing violation".into());
    f.installers.script_locked(Err(locked.clone()));
    assert_eq!(hub.apply_update().await, Err(protocol(&locked.to_string())));
    // Never an unlocked start of a file that gets elevated: one locked call, nothing else.
    let calls = f.installers.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(matches!(
        calls[0],
        Call::Locked {
            kind: InstallKind::InnoMachine,
            ..
        }
    ));
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (
            offer.mode,
            offer.stage,
            offer.install_failed,
            offer.needs_elevation
        ),
        (OfferMode::Download, OfferStage::Ready, true, false)
    );
    assert!(stored(&f).expect("offer.json").install_failed);
    assert!(hub.apply_update().await.is_err(), "not retried");
    assert_eq!(f.installers.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn upd_007_hub_locked_start_failed_per_user_install_retries_once() {
    if updater_env_off() {
        return;
    }
    let locked = UpdateError::LockedStartFailed("sharing violation".into());

    // The unlocked retry (verified again first) starts it: a handoff, the offer unchanged.
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    f.installers.script_locked(Err(locked.clone()));
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
    let (kind, sha256, size) = (
        InstallKind::InnoUser,
        sha2_hex(&f.file),
        f.file.len() as u64,
    );
    assert_eq!(
        f.installers.calls(),
        [
            Call::Locked {
                installer: f.payload("0.2.0"),
                kind,
                sha256: sha256.clone(),
                size,
                running_exe: None,
            },
            Call::Unlocked {
                installer: f.payload("0.2.0"),
                kind,
                sha256,
                size,
                running_exe: None,
            },
        ]
    );
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Install, OfferStage::Ready, false)
    );

    // A second failure to lock ends it: *Download*, and no third attempt.
    let f = Fixture::new(InstallKind::InnoUser);
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    f.installers.script_locked(Err(locked.clone()));
    f.installers.script_unlocked(Err(locked.clone()));
    assert_eq!(hub.apply_update().await, Err(protocol(&locked.to_string())));
    assert_eq!(f.installers.calls().len(), 2);
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.install_failed),
        (OfferMode::Download, true)
    );
    assert!(hub.apply_update().await.is_err());
    assert_eq!(f.installers.calls().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn upd_007_hub_declined_uac_keeps_the_offer_ready() {
    if updater_env_off() {
        return;
    }
    let f = Fixture::new(InstallKind::InnoMachine);
    let hub = f.launch().system_on_fakes().go();
    offered(&f, &hub, "0.2.0").await;
    let declined = UpdateError::Spawn("the operation was canceled by the user".into());
    f.installers.script_locked(Err(declined.clone()));
    assert_eq!(
        hub.apply_update().await,
        Err(protocol(&declined.to_string()))
    );
    let offer = the_offer(&current(&hub).await);
    assert_eq!(
        (offer.mode, offer.stage, offer.install_failed),
        (OfferMode::Install, OfferStage::Ready, false)
    );
    assert_eq!(
        f.installers.calls().len(),
        1,
        "a declined start isn't retried unlocked"
    );
    // The user tries again, and accepts.
    assert_eq!(hub.apply_update().await, Ok(ApplyOutcome::Restarting));
}

// ── UPD-012: the staging sweep ──────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn upd_012_sweep_wired_for_appimage_only() {
    use std::time::Duration;
    if updater_env_off() {
        return;
    }
    // (install kind, the staging files in the AppImage's directory are swept)
    for (kind, swept) in [
        (InstallKind::AppImage, true),
        (InstallKind::InnoUser, false),
        (InstallKind::InnoMachine, false),
        (InstallKind::Portable, false),
        (InstallKind::Other, false),
    ] {
        let (f, target) = appimage_fixture(|_| crate::updates_tests::FILE.to_vec());
        f.host.set_kind(kind);
        let dir = target.parent().expect("dir").to_path_buf();
        let now = f.host.now();
        let make = |name: &str, age: Duration| {
            let path = dir.join(name);
            std::fs::write(&path, b"x").expect("write");
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open")
                .set_modified(now - age)
                .expect("mtime");
            path
        };
        let stale = make(".dockering-update-1", Duration::from_secs(2 * 3600));
        let fresh = make(".dockering-update-2", Duration::from_secs(600));
        let other = make("notes.txt", Duration::from_secs(2 * 3600));
        let app = make("Dockering.AppImage", Duration::from_secs(2 * 3600));

        let hub = f.start();
        settled(&hub).await;
        assert_eq!(!stale.exists(), swept, "{kind:?}");
        assert!(
            fresh.exists(),
            "{kind:?}: a staging file of the last hour may be in use"
        );
        assert!(
            other.exists() && app.exists(),
            "{kind:?}: only `.dockering-update-*` goes"
        );
        hub.shutdown();
    }

    // An AppImage whose real path can't be found has nothing to sweep next to.
    let (f, target) = appimage_fixture(|_| crate::updates_tests::FILE.to_vec());
    let stale = target.parent().expect("dir").join(".dockering-update-1");
    std::fs::write(&stale, b"x").expect("write");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .expect("open")
        .set_modified(f.host.now() - Duration::from_secs(7200))
        .expect("mtime");
    *lock(&f.host.appimage) = None;
    let hub = f.start();
    settled(&hub).await;
    assert!(stale.exists());
    hub.shutdown();

    // A hard disable touches no file at all (UPD-017 (8)), the sweep included.
    let (f, target) = appimage_fixture(|_| crate::updates_tests::FILE.to_vec());
    let stale = target.parent().expect("dir").join(".dockering-update-1");
    std::fs::write(&stale, b"x").expect("write");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .expect("open")
        .set_modified(f.host.now() - Duration::from_secs(7200))
        .expect("mtime");
    let hub = f.launch().no_keys().go();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(stale.exists());
    hub.shutdown();
}
