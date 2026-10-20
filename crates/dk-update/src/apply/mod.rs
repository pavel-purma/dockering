//! Applying a verified update: starting the Windows installer (UPD-007) and replacing a Linux
//! AppImage in place (UPD-014, in [`appimage`]).

#[cfg(windows)]
use std::path::Path;

use crate::UpdateError;
use crate::install_kind::InstallKind;

pub mod appimage;
mod env;
mod locked;
mod sweep;

pub use appimage::{
    AppImageJob, AppImageOutcome, RELAUNCH_FLAG, apply_appimage, restart_installed,
};
pub use env::{EnvPlan, current_env, scrubbed_env};
pub use locked::{
    LockedFallback, LockedInstaller, locked_start_fallback, spawn_installer_locked,
    spawn_installer_unlocked_after_verify,
};
pub use sweep::{STAGING_PREFIX, sweep_stale_staging};

/// Inno Setup argv for an in-app update: progress window only (`/SILENT`, `/UPDATE`), no
/// reboot, relaunch afterwards (`/RELAUNCH`, see `packaging/windows/dockering.iss`), same scope as
/// the current install.
#[must_use]
pub fn installer_args(kind: InstallKind) -> Vec<&'static str> {
    let mut args = vec![
        "/SILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/SP-",
        "/UPDATE",
        "/RELAUNCH",
    ];
    args.push(if kind == InstallKind::InnoMachine {
        "/ALLUSERS"
    } else {
        "/CURRENTUSER"
    });
    args
}

/// Only the Inno kinds start an installer. [`InstallKind::AppImage`] also answers
/// [`InstallKind::can_install`], but it is applied by [`apply_appimage`], never by running a
/// downloaded file as an installer, so it is rejected explicitly.
fn require_inno(kind: InstallKind) -> Result<(), UpdateError> {
    if matches!(kind, InstallKind::InnoUser | InstallKind::InnoMachine) {
        Ok(())
    } else {
        Err(UpdateError::Unsupported)
    }
}

/// Per-user installs: a plain detached child. All-users installs: `ShellExecuteExW` with the
/// `runas` verb, so Windows shows the UAC prompt (Inno doesn't elevate itself for `/ALLUSERS`
/// when `PrivilegesRequired=lowest`). Arguments are fixed switches, never user input (NFR-022).
/// `held` says the installer is held share-locked by this process, which makes a sharing
/// violation a [`UpdateError::LockedStartFailed`] instead of a plain [`UpdateError::Spawn`].
#[cfg(windows)]
fn spawn(installer: &Path, args: &[&str], elevate: bool, held: bool) -> Result<(), UpdateError> {
    start(installer, args, elevate).map_err(|error| locked::start_failure(error, held))
}

#[cfg(windows)]
fn start(installer: &Path, args: &[&str], elevate: bool) -> Result<(), locked::StartError> {
    if elevate {
        return runas::spawn(installer, &args.join(" "));
    }
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new(installer)
        .args(args)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map(drop)
        .map_err(|e| locked::StartError {
            message: e.to_string(),
            os_error: e.raw_os_error(),
            shell_error: None,
        })
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod runas {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    use super::locked::StartError;

    const SW_SHOWNORMAL: i32 = 1;
    /// `ShellExecuteExW` sets `hInstApp` to an `SE_ERR_*` (at most 32) when it fails.
    const MAX_SE_ERR: usize = 32;

    pub(super) fn spawn(installer: &Path, params: &str) -> Result<(), StartError> {
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let verb = wide("runas".as_ref());
        let file = wide(installer.as_os_str());
        let params = wide(params.as_ref());
        // SAFETY: SHELLEXECUTEINFOW is a plain C struct; zero is a valid start and every field
        // the call reads is set below.
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        // SAFETY: the three NUL-terminated buffers outlive the call; no process handle is
        // requested (no SEE_MASK_NOCLOSEPROCESS), so nothing needs closing.
        let ok = unsafe { ShellExecuteExW(&mut info) };
        if ok == 0 {
            // Includes the user declining the UAC prompt (ERROR_CANCELLED).
            let error = std::io::Error::last_os_error();
            let shell = info.hInstApp as usize;
            return Err(StartError {
                message: error.to_string(),
                os_error: error.raw_os_error(),
                shell_error: (shell <= MAX_SE_ERR).then_some(shell as i32),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upd_007_installer_args() {
        let user = installer_args(InstallKind::InnoUser);
        assert!(user.contains(&"/UPDATE") && user.contains(&"/RELAUNCH"));
        assert_eq!(user.last(), Some(&"/CURRENTUSER"));
        assert_eq!(
            installer_args(InstallKind::InnoMachine).last(),
            Some(&"/ALLUSERS")
        );
        assert_eq!(
            require_inno(InstallKind::Portable),
            Err(UpdateError::Unsupported)
        );
        assert_eq!(require_inno(InstallKind::InnoUser), Ok(()));
        assert_eq!(require_inno(InstallKind::InnoMachine), Ok(()));
    }

    /// UPD-007, UPD-014: an AppImage payload is never started as an installer, even though
    /// `InstallKind::AppImage.can_install()` is true.
    #[test]
    fn require_inno_rejects_non_inno_kinds() {
        assert!(InstallKind::AppImage.can_install());
        for kind in [
            InstallKind::AppImage,
            InstallKind::Portable,
            InstallKind::Other,
        ] {
            assert_eq!(
                require_inno(kind),
                Err(UpdateError::Unsupported),
                "{kind:?}"
            );
        }
    }
}
