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

/// Starts `installer` detached, without a shell (NFR-022). The caller then quits the app; the
/// installer waits for it through `AppMutex` (REL-024).
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
    spawn(installer, &installer_args(kind))
}

#[cfg(windows)]
fn spawn(installer: &Path, args: &[&str]) -> Result<(), UpdateError> {
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

#[cfg(not(windows))]
fn spawn(_installer: &Path, _args: &[&str]) -> Result<(), UpdateError> {
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
