//! How Dockering was installed, which decides the update mode (UPD-006).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Installation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallKind {
    /// Inno Setup, per user (`%LocalAppData%\Programs\Dockering`): in-app install.
    InnoUser,
    /// Inno Setup, all users (`%ProgramFiles%\Dockering`): in-app install with UAC.
    InnoMachine,
    /// Portable zip (Windows): notify only.
    Portable,
    /// macOS, Linux, or anything else: notify only.
    Other,
}

impl InstallKind {
    /// Whether the app can download and install updates itself.
    #[must_use]
    pub fn can_install(self) -> bool {
        matches!(self, Self::InnoUser | Self::InnoMachine)
    }
}

/// Pure detection, testable on every OS: `exe` is the running binary, `local_app_data` is
/// `%LocalAppData%`, and `program_files` lists `%ProgramFiles%`-style roots. An Inno install
/// always has `unins000.exe` next to the binary.
#[must_use]
pub fn detect(
    exe: &Path,
    is_windows: bool,
    local_app_data: Option<&Path>,
    program_files: &[PathBuf],
) -> InstallKind {
    if !is_windows {
        return InstallKind::Other;
    }
    let Some(dir) = exe.parent() else {
        return InstallKind::Portable;
    };
    if !dir.join("unins000.exe").is_file() {
        return InstallKind::Portable;
    }
    let dir_key = normalize(dir);
    if let Some(lad) = local_app_data
        && dir_key == normalize(&lad.join("Programs").join("Dockering"))
    {
        return InstallKind::InnoUser;
    }
    if program_files
        .iter()
        .any(|pf| dir_key.starts_with(&(normalize(pf) + "\\")))
    {
        return InstallKind::InnoMachine;
    }
    InstallKind::Portable
}

/// Lowercase, backslash-separated, without a trailing separator (Windows paths are
/// case-insensitive).
fn normalize(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// [`detect`] for the running process.
#[must_use]
pub fn detect_current() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Other;
    };
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .collect();
    detect(
        &exe,
        cfg!(windows),
        local_app_data.as_deref(),
        &program_files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &Path, with_uninstaller: bool) -> PathBuf {
        std::fs::create_dir_all(root).expect("mkdir");
        if with_uninstaller {
            std::fs::write(root.join("unins000.exe"), b"").expect("write");
        }
        root.join("dockering.exe")
    }

    #[test]
    fn upd_006_install_kind_from_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let lad = tmp.path().join("Local");
        let pf = tmp.path().join("Program Files");
        let pfs = vec![pf.clone()];

        let user = install(&lad.join("Programs").join("Dockering"), true);
        assert_eq!(detect(&user, true, Some(&lad), &pfs), InstallKind::InnoUser);
        // Case-insensitive match.
        let upper = PathBuf::from(user.to_string_lossy().to_uppercase());
        assert_eq!(
            detect(&upper, true, Some(&lad), &pfs),
            if cfg!(windows) {
                InstallKind::InnoUser
            } else {
                InstallKind::Portable // the uninstaller lookup is case-sensitive off Windows
            }
        );

        let machine = install(&pf.join("Dockering"), true);
        assert_eq!(
            detect(&machine, true, Some(&lad), &pfs),
            InstallKind::InnoMachine
        );

        let unzipped = install(&tmp.path().join("Downloads").join("Dockering-x64"), false);
        assert_eq!(
            detect(&unzipped, true, Some(&lad), &pfs),
            InstallKind::Portable
        );
        // An Inno-looking dir without the uninstaller is portable too.
        let copied = install(&tmp.path().join("Local2"), false);
        assert_eq!(
            detect(&copied, true, Some(&lad), &pfs),
            InstallKind::Portable
        );

        assert_eq!(detect(&user, false, Some(&lad), &pfs), InstallKind::Other);
        assert!(InstallKind::InnoUser.can_install());
        assert!(!InstallKind::Portable.can_install());
    }
}
