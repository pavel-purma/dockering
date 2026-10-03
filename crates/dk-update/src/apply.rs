//! Starting the verified installer (UPD-007).

use std::path::Path;

use crate::UpdateError;
use crate::install_kind::InstallKind;

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

/// Starts `installer` detached, without a shell (NFR-022); all-users installs ask for UAC
/// first. Returns once the installer has started (or the user declined elevation, which is an
/// error, so the app keeps running). The caller then quits; the installer waits for it through
/// `AppMutex` (REL-024).
pub fn spawn_installer(installer: &Path, kind: InstallKind) -> Result<(), UpdateError> {
    if !kind.can_install() {
        return Err(UpdateError::Unsupported);
    }
    if !installer.is_absolute() || !installer.is_file() {
        return Err(UpdateError::Spawn(format!(
            "{} is not an installer file",
            installer.display()
        )));
    }
    spawn(
        installer,
        &installer_args(kind),
        kind == InstallKind::InnoMachine,
    )
}

/// Per-user installs: a plain detached child. All-users installs: `ShellExecuteExW` with the
/// `runas` verb, so Windows shows the UAC prompt (Inno doesn't elevate itself for `/ALLUSERS`
/// when `PrivilegesRequired=lowest`). Arguments are fixed switches, never user input (NFR-022).
#[cfg(windows)]
fn spawn(installer: &Path, args: &[&str], elevate: bool) -> Result<(), UpdateError> {
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
        .map_err(|e| UpdateError::Spawn(e.to_string()))
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod runas {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    use crate::UpdateError;

    const SW_SHOWNORMAL: i32 = 1;

    pub(super) fn spawn(installer: &Path, params: &str) -> Result<(), UpdateError> {
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
            return Err(UpdateError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn spawn(_installer: &Path, _args: &[&str], _elevate: bool) -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported)
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
            spawn_installer(Path::new("/nope.exe"), InstallKind::Portable),
            Err(UpdateError::Unsupported)
        );
        assert!(matches!(
            spawn_installer(Path::new("relative.exe"), InstallKind::InnoUser),
            Err(UpdateError::Spawn(_))
        ));
    }
}
