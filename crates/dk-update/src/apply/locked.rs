//! The share-locked Windows installer (UPD-007): no gap between the checks and the run.
//!
//! [`spawn_installer_locked`] opens the verified installer `FILE_SHARE_READ` only (others may
//! read it, nobody may write, rename or delete it), checks its size, SHA-256 and Authenticode
//! signature **through that handle**, starts it, and keeps the handle in a process-lifetime
//! `static` until the app exits. Other code of the same user can therefore not swap the file
//! after the check, which matters most for the all-users install, where the file is run
//! elevated.
//!
//! If the lock can't be held or the installer won't start while it is held (a sharing violation
//! from the open, from the child process or from `ShellExecuteExW`, ⚠ spike W1) the error is
//! [`UpdateError::LockedStartFailed`] and [`locked_start_fallback`] says what the hub does: a
//! per-user install retries once (after another hash check), an all-users install degrades the
//! offer to *Download* rather than elevating an unlocked file.
//!
//! Everything here is safe Rust (`OpenOptionsExt::share_mode`); the one `unsafe` call that
//! reads through the handle is `WinVerifyTrust` in [`crate::authenticode`]. The decisions are
//! pure and run on every OS; the handle itself is Windows-only, and everywhere else the
//! installer entry points answer [`UpdateError::Unsupported`].

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::require_inno;
use crate::UpdateError;
use crate::install_kind::InstallKind;

pub use platform::LockedInstaller;

/// What the hub does when [`UpdateError::LockedStartFailed`] comes back (UPD-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedFallback {
    /// Per-user install: try once more (the entry point verifies again first). The installer
    /// runs as the user who owns the file, so a start that isn't locked adds no privilege.
    RetryOnce,
    /// Degrade the offer to *Download*: an all-users installer is run elevated, and a file
    /// that isn't held could be swapped for another one after the check.
    Download,
}

/// The decision for a failed locked start of an installer of `kind`: only a per-user Inno
/// install may retry; everything else, the all-users install above all, falls back to
/// *Download*.
#[must_use]
pub fn locked_start_fallback(kind: InstallKind) -> LockedFallback {
    match kind {
        InstallKind::InnoUser => LockedFallback::RetryOnce,
        InstallKind::InnoMachine
        | InstallKind::Portable
        | InstallKind::AppImage
        | InstallKind::Other => LockedFallback::Download,
    }
}

/// The one entry point of the Windows apply, for a blocking worker: open the installer
/// share-locked → verify size and SHA-256 through the handle → pin the Authenticode signer
/// against `running_exe` (`None` skips the pin) through the same handle → start it → keep the
/// lock until the app exits.
///
/// The errors, for the hub:
/// - [`UpdateError::UntrustedInstaller`]: a signed running exe rejects the installer (or its own
///   signature is unusable). Degrade the offer to *Download*.
/// - [`UpdateError::Integrity`] (size or SHA-256 changed) and [`UpdateError::Io`] (the installer
///   is missing, unreadable or blocked): the install is broken for this version. Degrade the
///   offer to *Download*.
/// - [`UpdateError::LockedStartFailed`]: see [`locked_start_fallback`].
/// - [`UpdateError::Spawn`]: the start failed, UAC declined included. Nothing changed; keep the
///   offer *Ready* so the user can retry.
/// - [`UpdateError::Unsupported`]: not a Windows Inno install, or not Windows.
///
/// Returns once the installer has started; the caller then saves its state and quits.
pub fn spawn_installer_locked(
    installer: &Path,
    kind: InstallKind,
    expected_sha256: &str,
    expected_size: u64,
    running_exe: Option<&Path>,
) -> Result<(), UpdateError> {
    require_inno(kind)?;
    let mut locked = LockedInstaller::open(installer)?;
    locked.verify(expected_sha256, expected_size)?;
    if let Some(running_exe) = running_exe {
        locked.check_trust(running_exe)?;
    }
    locked.spawn(kind)
}

/// The per-user retry of [`UpdateError::LockedStartFailed`] ([`LockedFallback::RetryOnce`]):
/// verify again, then start the installer **without** keeping it locked.
///
/// The file is opened share-locked for the checks when that works (and read plainly when the
/// lock can't be held, the very case this serves), size, SHA-256 and signature are checked
/// through that handle, and the handle is closed right before the start. Only a per-user
/// install may do this; an all-users install gets [`UpdateError::LockedStartFailed`] without
/// anything being started, because it must never elevate a file that isn't held.
pub fn spawn_installer_unlocked_after_verify(
    installer: &Path,
    kind: InstallKind,
    expected_sha256: &str,
    expected_size: u64,
    running_exe: Option<&Path>,
) -> Result<(), UpdateError> {
    require_inno(kind)?;
    platform::start_unlocked(installer, kind, expected_sha256, expected_size, running_exe)
}

/// Win32 `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION`.
const SHARING_ERRORS: [i32; 2] = [32, 33];
/// `ShellExecuteExW`'s `SE_ERR_SHARE` (in `hInstApp`).
const SE_ERR_SHARE: i32 = 26;

/// Why starting the installer failed, with the codes that tell a sharing violation apart from
/// a declined UAC prompt or a missing file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct StartError {
    pub(super) message: String,
    /// The Win32 error (`GetLastError`) of the failed call.
    pub(super) os_error: Option<i32>,
    /// `SHELLEXECUTEINFOW.hInstApp` (an `SE_ERR_*`) when `ShellExecuteExW` failed.
    pub(super) shell_error: Option<i32>,
}

fn is_sharing_violation(os_error: Option<i32>) -> bool {
    os_error.is_some_and(|code| SHARING_ERRORS.contains(&code))
}

/// A failed start: with the installer held, a sharing violation is
/// [`UpdateError::LockedStartFailed`]; every other failure, and any failure of an unlocked
/// start, is [`UpdateError::Spawn`] (UAC declined, the file is gone, …).
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn start_failure(error: StartError, held: bool) -> UpdateError {
    let sharing = is_sharing_violation(error.os_error) || error.shell_error == Some(SE_ERR_SHARE);
    if held && sharing {
        UpdateError::LockedStartFailed(error.message)
    } else {
        UpdateError::Spawn(error.message)
    }
}

/// Opening the installer failed: a sharing violation means the lock can't be held
/// ([`UpdateError::LockedStartFailed`]); anything else (missing, denied, …) is
/// [`UpdateError::Io`], which ends this version's install.
#[cfg_attr(not(windows), allow(dead_code))]
fn open_failure(error: &io::Error) -> UpdateError {
    if is_sharing_violation(error.raw_os_error()) {
        UpdateError::LockedStartFailed(format!("the installer can't be locked: {error}"))
    } else {
        UpdateError::Io(error.to_string())
    }
}

/// Size and SHA-256 of whatever `reader` yields from the start, read through the reader's own
/// handle (never by path). Stops at the first byte past `expected_size`, so a grown file is
/// rejected without hashing all of it. A mismatch is [`UpdateError::Integrity`], a read failure
/// [`UpdateError::Io`].
#[cfg_attr(not(windows), allow(dead_code))]
fn verify_reader<R: Read + Seek>(
    reader: &mut R,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<(), UpdateError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        total += read as u64;
        if total > expected_size {
            return Err(UpdateError::Integrity);
        }
        hasher.update(&buf[..read]);
    }
    let digest = hex::encode(hasher.finalize());
    if total == expected_size && digest.eq_ignore_ascii_case(expected_sha256.trim()) {
        Ok(())
    } else {
        Err(UpdateError::Integrity)
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    use super::{open_failure, verify_reader};
    use crate::UpdateError;
    use crate::apply::{installer_args, require_inno, spawn};
    use crate::authenticode::{self, TrustCheck};
    use crate::install_kind::InstallKind;

    /// `FILE_SHARE_READ`: others may read the file, nobody may write, rename or delete it.
    const FILE_SHARE_READ: u32 = 0x1;
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`: what a plain open allows.
    const FILE_SHARE_ALL: u32 = 0x7;

    /// The handles of started installers, **for the life of this process**: the lock must last
    /// until the app has exited and the installer has taken over, so no destructor may close
    /// it. A `static` is never dropped; the OS closes the handle when the process ends.
    static HELD: Mutex<Vec<(PathBuf, File)>> = Mutex::new(Vec::new());

    /// An installer opened for reading with `FILE_SHARE_READ` only, kept open until
    /// [`spawn`](Self::spawn) has started it and then until the app exits (UPD-007).
    #[derive(Debug)]
    pub struct LockedInstaller {
        file: File,
        path: PathBuf,
    }

    impl LockedInstaller {
        /// Opens `path` (an absolute path to a regular file) share-locked. A sharing
        /// violation (someone has it open for writing, or an antivirus scanner holds it
        /// exclusively) is [`UpdateError::LockedStartFailed`]; a missing or unreadable file is
        /// [`UpdateError::Io`].
        pub fn open(path: &Path) -> Result<Self, UpdateError> {
            Self::open_with(path, FILE_SHARE_READ)
        }

        fn open_with(path: &Path, share: u32) -> Result<Self, UpdateError> {
            if !path.is_absolute() {
                return Err(not_an_installer(path));
            }
            let file = OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(path)
                .map_err(|e| open_failure(&e))?;
            if !file.metadata()?.is_file() {
                return Err(not_an_installer(path));
            }
            Ok(Self {
                file,
                path: path.to_path_buf(),
            })
        }

        /// Checks the file's size and SHA-256 through the held handle (it seeks to the start
        /// first, so it can be called any number of times).
        pub fn verify(
            &mut self,
            expected_sha256: &str,
            expected_size: u64,
        ) -> Result<(), UpdateError> {
            verify_reader(&mut self.file, expected_sha256, expected_size)
        }

        /// The Authenticode pin of this installer against `running_exe`, read through the
        /// held handle (UPD-003). ⚠ W1: unverified on Windows; see
        /// [`authenticode::check_installer_open`].
        pub fn check_trust(&self, running_exe: &Path) -> Result<TrustCheck, UpdateError> {
            authenticode::check_installer_open(&self.file, &self.path, running_exe)
        }

        /// Starts the installer for `kind` (per-user: a detached child; all-users:
        /// `ShellExecuteExW` with `runas`), and on success moves the handle into a
        /// process-lifetime `static`, so the lock lasts until the app exits. A sharing
        /// violation from the start is [`UpdateError::LockedStartFailed`]; any other failure
        /// (UAC declined included) is [`UpdateError::Spawn`], and the handle is closed.
        pub fn spawn(self, kind: InstallKind) -> Result<(), UpdateError> {
            require_inno(kind)?;
            spawn(
                &self.path,
                &installer_args(kind),
                kind == InstallKind::InnoMachine,
                true,
            )?;
            self.hold_until_exit();
            Ok(())
        }

        /// Moves the handle into [`HELD`].
        fn hold_until_exit(self) {
            HELD.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((self.path, self.file));
        }

        /// Closes the handle and returns the path (the unlocked retry only).
        fn release(self) -> PathBuf {
            self.path
        }
    }

    fn not_an_installer(path: &Path) -> UpdateError {
        UpdateError::Spawn(format!("{} is not an installer file", path.display()))
    }

    /// The per-user retry: checks through a handle (locked when possible), closes it, starts
    /// the installer.
    pub(super) fn start_unlocked(
        installer: &Path,
        kind: InstallKind,
        expected_sha256: &str,
        expected_size: u64,
        running_exe: Option<&Path>,
    ) -> Result<(), UpdateError> {
        if super::locked_start_fallback(kind) != super::LockedFallback::RetryOnce {
            return Err(UpdateError::LockedStartFailed(
                "an all-users installer is only started while it is locked".into(),
            ));
        }
        let mut checked = match LockedInstaller::open(installer) {
            Err(UpdateError::LockedStartFailed(_)) => {
                LockedInstaller::open_with(installer, FILE_SHARE_ALL)?
            }
            other => other?,
        };
        checked.verify(expected_sha256, expected_size)?;
        if let Some(running_exe) = running_exe {
            checked.check_trust(running_exe)?;
        }
        let path = checked.release();
        spawn(&path, &installer_args(kind), false, false)
    }

    /// Releases the handles kept for `path`, as the OS does when the process ends. Tests only.
    #[cfg(test)]
    pub(super) fn release_held_for_tests(path: &Path) {
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(held, _)| held != path);
    }

    /// Keeps `installer`'s handle for the life of the process without starting anything.
    /// Tests only.
    #[cfg(test)]
    pub(super) fn hold_for_tests(installer: LockedInstaller) {
        installer.hold_until_exit();
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use crate::UpdateError;
    use crate::authenticode::TrustCheck;
    use crate::install_kind::InstallKind;

    /// Not available off Windows: every method answers [`UpdateError::Unsupported`].
    #[derive(Debug)]
    pub struct LockedInstaller(());

    impl LockedInstaller {
        /// Always [`UpdateError::Unsupported`].
        pub fn open(_path: &Path) -> Result<Self, UpdateError> {
            Err(UpdateError::Unsupported)
        }

        /// Always [`UpdateError::Unsupported`].
        pub fn verify(
            &mut self,
            _expected_sha256: &str,
            _expected_size: u64,
        ) -> Result<(), UpdateError> {
            Err(UpdateError::Unsupported)
        }

        /// Always [`UpdateError::Unsupported`].
        pub fn check_trust(&self, _running_exe: &Path) -> Result<TrustCheck, UpdateError> {
            Err(UpdateError::Unsupported)
        }

        /// Always [`UpdateError::Unsupported`].
        pub fn spawn(self, _kind: InstallKind) -> Result<(), UpdateError> {
            Err(UpdateError::Unsupported)
        }
    }

    pub(super) fn start_unlocked(
        _installer: &Path,
        _kind: InstallKind,
        _expected_sha256: &str,
        _expected_size: u64,
        _running_exe: Option<&Path>,
    ) -> Result<(), UpdateError> {
        Err(UpdateError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn sha256(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn upd_007_per_machine_falls_back_to_download_without_lock() {
        // The hub's decision for a failed locked start (UPD-007, spike W1): a per-user install
        // retries once; an all-users install never elevates a file that isn't held.
        assert_eq!(
            locked_start_fallback(InstallKind::InnoUser),
            LockedFallback::RetryOnce
        );
        assert_eq!(
            locked_start_fallback(InstallKind::InnoMachine),
            LockedFallback::Download
        );
        for kind in [
            InstallKind::Portable,
            InstallKind::AppImage,
            InstallKind::Other,
        ] {
            assert_eq!(
                locked_start_fallback(kind),
                LockedFallback::Download,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn upd_007_sharing_violation_maps_to_locked_start_failed_only_when_held() {
        let violation = |os_error, shell_error| StartError {
            message: "boom".into(),
            os_error,
            shell_error,
        };
        for code in [32, 33] {
            assert_eq!(
                start_failure(violation(Some(code), None), true),
                UpdateError::LockedStartFailed("boom".into()),
                "os error {code}"
            );
            // The same error from an unlocked start has nothing to do with the lock.
            assert_eq!(
                start_failure(violation(Some(code), None), false),
                UpdateError::Spawn("boom".into())
            );
        }
        // ShellExecuteExW reports a sharing violation in hInstApp (SE_ERR_SHARE).
        assert_eq!(
            start_failure(violation(None, Some(26)), true),
            UpdateError::LockedStartFailed("boom".into())
        );
        // A declined UAC prompt (ERROR_CANCELLED 1223), access denied, a missing file, and the
        // other SE_ERR codes are plain start failures: the offer stays Ready.
        for (code, shell) in [
            (Some(1223), None),
            (Some(5), None),
            (Some(2), None),
            (None, Some(5)),
            (None, Some(2)),
            (None, None),
        ] {
            assert_eq!(
                start_failure(violation(code, shell), true),
                UpdateError::Spawn("boom".into()),
                "{code:?} {shell:?}"
            );
        }
    }

    #[test]
    fn upd_007_open_failure_mapping() {
        for code in [32, 33] {
            assert!(matches!(
                open_failure(&io::Error::from_raw_os_error(code)),
                UpdateError::LockedStartFailed(_)
            ));
        }
        // A missing or unreadable installer ends this version's install (the hub maps `Io`).
        assert!(matches!(
            open_failure(&io::Error::from(io::ErrorKind::NotFound)),
            UpdateError::Io(_)
        ));
        assert!(matches!(
            open_failure(&io::Error::from_raw_os_error(5)),
            UpdateError::Io(_)
        ));
    }

    #[test]
    fn upd_007_verify_reads_through_the_handle_from_the_start() {
        let body = b"installer bytes".repeat(10_000);
        let sha = sha256(&body);
        let size = body.len() as u64;
        let mut reader = Cursor::new(body.clone());
        assert_eq!(verify_reader(&mut reader, &sha, size), Ok(()));
        // Position-independent and repeatable: it seeks to the start itself.
        reader.set_position(1234);
        assert_eq!(verify_reader(&mut reader, &sha, size), Ok(()));
        assert_eq!(
            verify_reader(&mut reader, &sha.to_uppercase(), size),
            Ok(()),
            "hex case doesn't matter"
        );
        assert_eq!(
            verify_reader(&mut reader, &format!("  {sha}\n"), size),
            Ok(()),
            "surrounding whitespace doesn't matter"
        );
        // A changed size or digest is an integrity failure.
        assert_eq!(
            verify_reader(&mut reader, &sha, size - 1),
            Err(UpdateError::Integrity)
        );
        assert_eq!(
            verify_reader(&mut reader, &sha, size + 1),
            Err(UpdateError::Integrity)
        );
        assert_eq!(
            verify_reader(&mut reader, &sha256(b"other"), size),
            Err(UpdateError::Integrity)
        );
        assert_eq!(
            verify_reader(&mut reader, "", size),
            Err(UpdateError::Integrity)
        );
        assert_eq!(
            verify_reader(&mut Cursor::new(Vec::new()), &sha256(b""), 0),
            Ok(())
        );
    }

    /// A file that keeps growing is rejected at the first byte past the expected size, without
    /// reading it all.
    #[test]
    fn upd_007_verify_stops_at_the_expected_size() {
        struct Endless(u64);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += buf.len() as u64;
                assert!(self.0 < 1 << 20, "read far past the expected size");
                buf.fill(7);
                Ok(buf.len())
            }
        }
        impl Seek for Endless {
            fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
                self.0 = 0;
                Ok(0)
            }
        }
        assert_eq!(
            verify_reader(&mut Endless(0), &sha256(b""), 1000),
            Err(UpdateError::Integrity)
        );
    }

    #[test]
    fn upd_007_verify_read_failure_is_io() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disk gone"))
            }
        }
        impl Seek for Broken {
            fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
                Ok(0)
            }
        }
        assert!(matches!(
            verify_reader(&mut Broken, &sha256(b""), 0),
            Err(UpdateError::Io(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn upd_007_non_windows_is_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("Setup.exe");
        std::fs::write(&file, b"installer").expect("write");
        let sha = sha256(b"installer");
        let size = 9;
        for kind in [
            InstallKind::InnoUser,
            InstallKind::InnoMachine,
            InstallKind::Portable,
            InstallKind::AppImage,
            InstallKind::Other,
        ] {
            assert_eq!(
                spawn_installer_locked(&file, kind, &sha, size, None),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
            assert_eq!(
                spawn_installer_locked(&file, kind, &sha, size, Some(Path::new("/x"))),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
            assert_eq!(
                spawn_installer_unlocked_after_verify(&file, kind, &sha, size, None),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
        }
        assert_eq!(
            LockedInstaller::open(&file).unwrap_err(),
            UpdateError::Unsupported
        );
    }

    /// Never reached with a non-Inno kind, even on Windows: those are rejected before the
    /// file is opened.
    #[test]
    fn upd_007_locked_entry_points_reject_non_inno_kinds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("payload");
        std::fs::write(&file, b"not an installer").expect("write");
        let sha = sha256(b"not an installer");
        for kind in [
            InstallKind::AppImage,
            InstallKind::Portable,
            InstallKind::Other,
        ] {
            assert_eq!(
                spawn_installer_locked(&file, kind, &sha, 16, None),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
            assert_eq!(
                spawn_installer_unlocked_after_verify(&file, kind, &sha, 16, None),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
        }
    }

    // ---- Windows: the handle itself (⚠ W1 where marked) ------------------------------------

    #[cfg(windows)]
    mod windows {
        use std::fs::OpenOptions;
        use std::path::PathBuf;

        use super::*;

        const ERROR_SHARING_VIOLATION: i32 = 32;

        fn installer(dir: &Path, body: &[u8]) -> PathBuf {
            let path = dir.join("Setup.exe");
            std::fs::write(&path, body).expect("write");
            path
        }

        #[test]
        fn upd_007_installer_held_share_locked_until_exit() {
            let dir = tempfile::tempdir().expect("tempdir");
            let body = b"MZ pretend installer".repeat(100);
            let path = installer(dir.path(), &body);
            let mut locked = LockedInstaller::open(&path).expect("open");

            // Others may read (a second locked open and a plain read), nobody may write,
            // rename or delete.
            let denied = |path: &Path| -> Option<i32> {
                OpenOptions::new()
                    .write(true)
                    .open(path)
                    .err()
                    .and_then(|e| e.raw_os_error())
            };
            assert_eq!(denied(&path), Some(ERROR_SHARING_VIOLATION));
            assert!(std::fs::remove_file(&path).is_err(), "delete is refused");
            assert!(
                std::fs::rename(&path, path.with_extension("moved")).is_err(),
                "rename is refused"
            );
            assert_eq!(std::fs::read(&path).expect("others may read"), body);
            drop(LockedInstaller::open(&path).expect("a second reader may lock it too"));

            // The checks run through the held handle, repeatably.
            let sha = sha256(&body);
            assert_eq!(locked.verify(&sha, body.len() as u64), Ok(()));
            assert_eq!(locked.verify(&sha, body.len() as u64), Ok(()));
            assert_eq!(
                locked.verify(&sha, body.len() as u64 + 1),
                Err(UpdateError::Integrity)
            );
            assert_eq!(
                locked.verify(&sha256(b"other"), body.len() as u64),
                Err(UpdateError::Integrity)
            );

            // Moved into the process-lifetime static, it stays locked after every owner is
            // gone: that is what lasts "until the app exits".
            platform::hold_for_tests(locked);
            assert_eq!(denied(&path), Some(ERROR_SHARING_VIOLATION));
            assert!(std::fs::remove_file(&path).is_err());

            // The OS would close it at exit; the test does it for the temp dir.
            platform::release_held_for_tests(&path);
            assert_eq!(denied(&path), None, "released handles unlock the file");
            std::fs::remove_file(&path).expect("deletable again");
        }

        #[test]
        fn upd_007_open_fails_with_locked_start_failed_while_a_writer_exists() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = installer(dir.path(), b"MZ");
            let _writer = OpenOptions::new().write(true).open(&path).expect("writer");
            assert!(matches!(
                LockedInstaller::open(&path),
                Err(UpdateError::LockedStartFailed(_))
            ));
            // The unlocked retry can still read it plainly, checks it and (here) fails the
            // check rather than the open.
            assert_eq!(
                spawn_installer_unlocked_after_verify(
                    &path,
                    InstallKind::InnoUser,
                    &sha256(b"other"),
                    2,
                    None
                ),
                Err(UpdateError::Integrity)
            );
        }

        #[test]
        fn upd_007_open_rejects_missing_relative_and_non_files() {
            let dir = tempfile::tempdir().expect("tempdir");
            assert!(matches!(
                LockedInstaller::open(Path::new("relative.exe")),
                Err(UpdateError::Spawn(_))
            ));
            assert!(matches!(
                LockedInstaller::open(&dir.path().join("gone.exe")),
                Err(UpdateError::Io(_))
            ));
            // A directory can't be opened as a file by `OpenOptions` on Windows without
            // backup semantics: an I/O error, never a started process.
            assert!(LockedInstaller::open(dir.path()).is_err());
        }

        #[test]
        fn upd_007_changed_payload_is_integrity_and_nothing_starts() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = installer(dir.path(), b"MZ original");
            let sha = sha256(b"MZ original");
            std::fs::write(&path, b"MZ tampered").expect("tamper before the lock");
            for kind in [InstallKind::InnoUser, InstallKind::InnoMachine] {
                assert_eq!(
                    spawn_installer_locked(&path, kind, &sha, 11, None),
                    Err(UpdateError::Integrity),
                    "{kind:?}"
                );
            }
            // Nothing was kept locked by the failed attempts.
            std::fs::remove_file(&path).expect("not held after a failed check");
        }

        #[test]
        fn upd_007_all_users_install_is_never_started_unlocked() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = installer(dir.path(), b"MZ");
            assert!(matches!(
                spawn_installer_unlocked_after_verify(
                    &path,
                    InstallKind::InnoMachine,
                    &sha256(b"MZ"),
                    2,
                    None
                ),
                Err(UpdateError::LockedStartFailed(_))
            ));
        }

        /// ⚠ W1 probe: a per-user start from a `FILE_SHARE_READ`-locked image. The "installer"
        /// is a copy of `hostname.exe` (it exits at once and ignores the switches). If this
        /// fails with `LockedStartFailed`, Windows refuses to start an image that is held this
        /// way, and the UPD-007 fallbacks apply (see [`locked_start_fallback`]). It also runs the
        /// unsigned-trust path (the test binary is unsigned, so the pin is skipped).
        #[test]
        fn upd_007_locked_per_user_start_works_w1() {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            let source = PathBuf::from(root).join(r"System32\hostname.exe");
            let Ok(body) = std::fs::read(&source) else {
                eprintln!("{} is not readable here; skipping", source.display());
                return;
            };
            let dir = tempfile::tempdir().expect("tempdir");
            let path = installer(dir.path(), &body);
            let exe = std::env::current_exe().expect("test exe");
            let started = spawn_installer_locked(
                &path,
                InstallKind::InnoUser,
                &sha256(&body),
                body.len() as u64,
                Some(&exe),
            );
            // Held for the life of the process after a successful start.
            let still_locked = OpenOptions::new().write(true).open(&path).is_err();
            platform::release_held_for_tests(&path);
            assert_eq!(
                started,
                Ok(()),
                "W1: per-user start while FILE_SHARE_READ-locked"
            );
            assert!(still_locked, "the lock outlives the start");
        }
    }
}
