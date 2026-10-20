//! How Dockering was installed, which decides the update mode (UPD-006, UPD-013).

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::manifest::AssetKind;

/// Installation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallKind {
    /// Inno Setup, per user (`%LocalAppData%\Programs\Dockering`): in-app install.
    InnoUser,
    /// Inno Setup, all users (`%ProgramFiles%\Dockering`): in-app install with UAC.
    InnoMachine,
    /// Portable zip (Windows): notify only.
    Portable,
    /// A real Linux AppImage in a writable directory (UPD-013): in-app install by replacing the
    /// AppImage file.
    AppImage,
    /// macOS (in-app install is deferred, UPD-016), `.deb`, `.tar.gz`, a read-only AppImage, or
    /// anything else: notify only.
    Other,
}

impl InstallKind {
    /// Whether the app can download and install updates itself.
    #[must_use]
    pub fn can_install(self) -> bool {
        matches!(self, Self::InnoUser | Self::InnoMachine | Self::AppImage)
    }
}

/// Whether an install of `kind` can apply a manifest asset of kind `asset` (UPD-013: the
/// `linux-<arch>` entry must be an `appimage`; the Windows installer path needs an `inno` entry).
/// Anything else is a *Download* offer (UPD-006).
#[must_use]
pub fn asset_supports_install(kind: InstallKind, asset: AssetKind) -> bool {
    match kind {
        InstallKind::AppImage => asset == AssetKind::Appimage,
        InstallKind::InnoUser | InstallKind::InnoMachine => asset == AssetKind::Inno,
        InstallKind::Portable | InstallKind::Other => false,
    }
}

/// The process facts [`detect_appimage`] needs, read by the caller so the rules stay pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppImageEnv {
    /// The `APPIMAGE` environment variable.
    pub appimage: Option<OsString>,
    /// The `APPDIR` environment variable.
    pub appdir: Option<OsString>,
    /// The running executable (`std::env::current_exe`).
    pub exe: PathBuf,
}

/// The file-system questions behind [`detect_appimage`], so the rules run on every OS.
pub trait AppImageProbe {
    /// The real path of `path` with every symlink resolved; `None` when it doesn't exist.
    fn canonicalize(&self, path: &Path) -> Option<PathBuf>;
    /// Whether `path` is a regular file that starts with the ELF magic (`7F 45 4C 46`) and has
    /// the AppImage type 2 marker (`'A' 'I' 0x02`) at offset 8.
    fn is_appimage_file(&self, path: &Path) -> bool;
    /// Whether a file can be created (and removed) in `dir`.
    fn dir_writable(&self, dir: &Path) -> bool;
}

/// ELF magic, then the AppImage marker `AI\x02` at offset 8 (type 2).
fn is_appimage_header(head: &[u8]) -> bool {
    head.len() >= 11 && head[..4] == [0x7F, b'E', b'L', b'F'] && head[8..11] == [b'A', b'I', 0x02]
}

/// The real [`AppImageProbe`], on the local file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct FsProbe;

impl AppImageProbe for FsProbe {
    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok()
    }

    fn is_appimage_file(&self, path: &Path) -> bool {
        // `metadata` first: opening a FIFO or a device could block.
        if !fs::metadata(path).is_ok_and(|m| m.is_file()) {
            return false;
        }
        let mut head = [0u8; 11];
        fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok()
            && is_appimage_header(&head)
    }

    fn dir_writable(&self, dir: &Path) -> bool {
        // Named like the staging files so a probe that outlives a crash is swept with them
        // (UPD-012: `.dockering-update-*`).
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..8 {
            let name = format!(
                ".dockering-update-probe-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let probe = dir.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&probe) {
                Ok(file) => {
                    drop(file);
                    return fs::remove_file(&probe).is_ok();
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return false,
            }
        }
        false
    }
}

/// UPD-013, pure rules: the real path of the AppImage this process runs from, or `None` when
/// this is not an AppImage that can be replaced in place. All of:
///
/// 1. `APPIMAGE` is set, absolute, and resolves to a regular file with the ELF magic and the
///    AppImage type 2 marker, so a stray variable can't aim the updater at another file;
/// 2. `APPDIR` is set (and absolute) and the running executable lies under it (both resolved,
///    compared by path component);
/// 3. the directory of the real AppImage file is writable.
///
/// The result is the canonical path, so a symlinked install (`~/bin/dockering -> …AppImage`)
/// updates the real file.
#[must_use]
pub fn detect_appimage(env: &AppImageEnv, probe: &dyn AppImageProbe) -> Option<PathBuf> {
    let appimage = Path::new(env.appimage.as_deref().filter(|v| !v.is_empty())?);
    if !appimage.is_absolute() {
        return None;
    }
    let real = probe.canonicalize(appimage)?;
    if !probe.is_appimage_file(&real) {
        return None;
    }

    let appdir = Path::new(env.appdir.as_deref().filter(|v| !v.is_empty())?);
    if !appdir.is_absolute() {
        return None;
    }
    let appdir = probe.canonicalize(appdir)?;
    // A root names no directory of the AppImage: every executable lies under `/`.
    if !appdir
        .components()
        .any(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let exe = probe.canonicalize(&env.exe)?;
    if exe == appdir || !exe.starts_with(&appdir) {
        return None;
    }

    if !probe.dir_writable(real.parent()?) {
        return None;
    }
    Some(real)
}

/// Pure detection, testable on every OS: `exe` is the running binary, `local_app_data` is
/// `%LocalAppData%`, and `program_files` lists `%ProgramFiles%`-style roots. An Inno install
/// always has `unins000.exe` next to the binary. On Linux (`is_linux`), a replaceable AppImage
/// (see [`detect_appimage`] for `appimage` and `probe`) is [`InstallKind::AppImage`]; every
/// other non-Windows install, macOS included, is [`InstallKind::Other`].
#[must_use]
pub fn detect(
    exe: &Path,
    is_windows: bool,
    is_linux: bool,
    local_app_data: Option<&Path>,
    program_files: &[PathBuf],
    appimage: &AppImageEnv,
    probe: &dyn AppImageProbe,
) -> InstallKind {
    if !is_windows {
        if is_linux && detect_appimage(appimage, probe).is_some() {
            return InstallKind::AppImage;
        }
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
    let appimage = process_appimage_env(exe.clone());
    detect(
        &exe,
        cfg!(windows),
        cfg!(target_os = "linux"),
        local_app_data.as_deref(),
        &program_files,
        &appimage,
        &FsProbe,
    )
}

/// The real path of the AppImage the running process was started from, or `None` (not Linux,
/// not an AppImage, or not replaceable). Same rules, real environment and real probe as
/// [`detect_current`], so the hub can re-derive the apply target on a blocking worker (UPD-013).
#[must_use]
pub fn current_appimage_path() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    detect_appimage(&process_appimage_env(exe), &FsProbe)
}

fn process_appimage_env(exe: PathBuf) -> AppImageEnv {
    AppImageEnv {
        appimage: std::env::var_os("APPIMAGE"),
        appdir: std::env::var_os("APPDIR"),
        exe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Windows-side rules with no AppImage environment.
    fn detect_on(
        exe: &Path,
        is_windows: bool,
        local_app_data: Option<&Path>,
        program_files: &[PathBuf],
    ) -> InstallKind {
        let env = AppImageEnv {
            appimage: None,
            appdir: None,
            exe: exe.to_path_buf(),
        };
        detect(
            exe,
            is_windows,
            false,
            local_app_data,
            program_files,
            &env,
            &FsProbe,
        )
    }

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
        assert_eq!(
            detect_on(&user, true, Some(&lad), &pfs),
            InstallKind::InnoUser
        );
        // Case-insensitive match. Only the root is uppercased so the uninstaller lookup doesn't
        // depend on the host filesystem's case sensitivity (APFS is insensitive, ext4 isn't).
        let upper_lad = PathBuf::from(lad.to_string_lossy().to_uppercase());
        assert_eq!(
            detect_on(&user, true, Some(&upper_lad), &pfs),
            InstallKind::InnoUser
        );

        let machine = install(&pf.join("Dockering"), true);
        assert_eq!(
            detect_on(&machine, true, Some(&lad), &pfs),
            InstallKind::InnoMachine
        );

        let unzipped = install(&tmp.path().join("Downloads").join("Dockering-x64"), false);
        assert_eq!(
            detect_on(&unzipped, true, Some(&lad), &pfs),
            InstallKind::Portable
        );
        // An Inno-looking dir without the uninstaller is portable too.
        let copied = install(&tmp.path().join("Local2"), false);
        assert_eq!(
            detect_on(&copied, true, Some(&lad), &pfs),
            InstallKind::Portable
        );

        assert_eq!(
            detect_on(&user, false, Some(&lad), &pfs),
            InstallKind::Other
        );
        assert!(InstallKind::InnoUser.can_install());
        assert!(InstallKind::InnoMachine.can_install());
        assert!(!InstallKind::Portable.can_install());
        assert!(!InstallKind::Other.can_install());
    }

    // ---- UPD-013 -------------------------------------------------------------------------

    use std::collections::{HashMap, HashSet};

    /// An in-memory file system: nothing here touches the disk, so the rules are exercised
    /// identically on every OS.
    #[derive(Default)]
    struct FakeProbe {
        /// `path -> real path` for symlinks.
        links: HashMap<PathBuf, PathBuf>,
        files: HashMap<PathBuf, Vec<u8>>,
        dirs: HashSet<PathBuf>,
        writable: bool,
    }

    impl FakeProbe {
        fn file(&mut self, path: &Path, bytes: &[u8]) {
            self.files.insert(path.to_path_buf(), bytes.to_vec());
        }
    }

    impl AppImageProbe for FakeProbe {
        fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
            let real = self
                .links
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.to_path_buf());
            (self.files.contains_key(&real) || self.dirs.contains(&real)).then_some(real)
        }

        fn is_appimage_file(&self, path: &Path) -> bool {
            self.files.get(path).is_some_and(|b| is_appimage_header(b))
        }

        fn dir_writable(&self, _dir: &Path) -> bool {
            self.writable
        }
    }

    /// A type 2 AppImage stand-in: ELF magic, then `AI\x02` at offset 8.
    fn fake_appimage_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        bytes[8..11].copy_from_slice(&[b'A', b'I', 0x02]);
        bytes
    }

    struct Layout {
        appimage: PathBuf,
        appdir: PathBuf,
        exe: PathBuf,
    }

    impl Layout {
        fn env(&self) -> AppImageEnv {
            AppImageEnv {
                appimage: Some(self.appimage.clone().into_os_string()),
                appdir: Some(self.appdir.clone().into_os_string()),
                exe: self.exe.clone(),
            }
        }
    }

    /// `<root>/apps/Dockering.AppImage` mounted at `<root>/mnt/.mount_x`, in a writable place.
    /// `root` is absolute on every OS but never created.
    fn layout() -> (Layout, FakeProbe) {
        let root = std::env::temp_dir().join("dk-upd-013-fake-root");
        let appimage = root.join("apps").join("Dockering.AppImage");
        let appdir = root.join("mnt").join(".mount_dockerX");
        let exe = appdir.join("usr").join("bin").join("dockering");
        let mut probe = FakeProbe {
            writable: true,
            ..FakeProbe::default()
        };
        probe.file(&appimage, &fake_appimage_bytes());
        probe.file(&exe, b"exe");
        probe.dirs.insert(appdir.clone());
        (
            Layout {
                appimage,
                appdir,
                exe,
            },
            probe,
        )
    }

    fn detect_linux(layout: &Layout, probe: &FakeProbe) -> InstallKind {
        detect(&layout.exe, false, true, None, &[], &layout.env(), probe)
    }

    #[test]
    fn upd_013_appimage_detected_from_env() {
        let (layout, probe) = layout();
        assert_eq!(
            detect_appimage(&layout.env(), &probe),
            Some(layout.appimage.clone())
        );
        assert_eq!(detect_linux(&layout, &probe), InstallKind::AppImage);
        assert!(InstallKind::AppImage.can_install());

        // No APPIMAGE (a plain binary, a `.deb`, a `cargo run`), an empty one, or a relative one.
        for appimage in [
            None,
            Some(OsString::new()),
            Some("Dockering.AppImage".into()),
        ] {
            let env = AppImageEnv {
                appimage,
                ..layout.env()
            };
            assert_eq!(detect_appimage(&env, &probe), None);
        }
        // APPIMAGE names a file that doesn't exist.
        let env = AppImageEnv {
            appimage: Some(layout.appimage.with_extension("gone").into_os_string()),
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);
    }

    #[test]
    fn upd_013_requires_appdir_and_exe_inside() {
        let (layout, mut probe) = layout();

        for appdir in [None, Some(OsString::new()), Some(".mount_dockerX".into())] {
            let env = AppImageEnv {
                appdir,
                ..layout.env()
            };
            assert_eq!(detect_appimage(&env, &probe), None, "{:?}", env.appdir);
        }
        // APPDIR names a directory that doesn't exist.
        let env = AppImageEnv {
            appdir: Some(layout.appdir.join("gone").into_os_string()),
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);

        // APPDIR is the root: every executable lies under `/`, so a `.deb` build started with
        // `APPDIR=/` and a stray APPIMAGE must not aim the updater at that AppImage.
        probe.dirs.insert(PathBuf::from("/"));
        let env = AppImageEnv {
            appdir: Some("/".into()),
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);

        // The running exe is somewhere else (a stray APPIMAGE/APPDIR in a shell profile).
        let outside = layout.appimage.with_file_name("other-dockering");
        probe.file(&outside, b"exe");
        let env = AppImageEnv {
            exe: outside,
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);

        // A sibling whose name merely starts with the APPDIR name is not inside it.
        let sibling = layout.appdir.with_file_name(".mount_dockerXY");
        let sibling_exe = sibling.join("dockering");
        probe.file(&sibling_exe, b"exe");
        let env = AppImageEnv {
            exe: sibling_exe,
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);

        // The exe is reached through a symlink that resolves into APPDIR: inside.
        let link = layout.appimage.with_file_name("dockering-link");
        probe.links.insert(link.clone(), layout.exe.clone());
        let env = AppImageEnv {
            exe: link,
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), Some(layout.appimage.clone()));

        // The exe can't be resolved.
        let env = AppImageEnv {
            exe: layout.appdir.join("usr").join("bin").join("gone"),
            ..layout.env()
        };
        assert_eq!(detect_appimage(&env, &probe), None);
    }

    #[test]
    fn upd_013_rejects_a_file_that_is_not_an_appimage() {
        // The header rule itself.
        let good = fake_appimage_bytes();
        assert!(is_appimage_header(&good));
        let mut no_elf = good.clone();
        no_elf[0] = b'#'; // a shell script
        let mut type1 = good.clone();
        type1[10] = 0x01; // AppImage type 1 has no `AI\x02`
        let mut no_ai = good.clone();
        no_ai[8..11].copy_from_slice(&[0, 0, 0]); // a plain ELF binary
        let mut shifted = good.clone();
        shifted[8..11].copy_from_slice(&[0, 0, 0]);
        shifted[9..12].copy_from_slice(&[b'A', b'I', 0x02]); // marker at the wrong offset
        for bad in [&no_elf[..], &type1, &no_ai, &shifted, &good[..10], &[]] {
            assert!(!is_appimage_header(bad), "{bad:?}");
        }

        // The rules refuse it, however the other conditions hold.
        let (layout, mut probe) = layout();
        probe.file(&layout.appimage, &no_ai);
        assert_eq!(detect_appimage(&layout.env(), &probe), None);
        assert_eq!(detect_linux(&layout, &probe), InstallKind::Other);

        // And the real probe, on real files and a directory.
        let tmp = tempfile::tempdir().expect("tempdir");
        let write = |name: &str, bytes: &[u8]| {
            let path = tmp.path().join(name);
            std::fs::write(&path, bytes).expect("write");
            path
        };
        assert!(FsProbe.is_appimage_file(&write("ok.AppImage", &good)));
        assert!(!FsProbe.is_appimage_file(&write("script.sh", &no_elf)));
        assert!(!FsProbe.is_appimage_file(&write("type1.AppImage", &type1)));
        assert!(!FsProbe.is_appimage_file(&write("plain-elf", &no_ai)));
        assert!(!FsProbe.is_appimage_file(&write("short", &good[..10])));
        assert!(!FsProbe.is_appimage_file(&write("empty", b"")));
        assert!(!FsProbe.is_appimage_file(tmp.path()), "a directory");
        assert!(!FsProbe.is_appimage_file(&tmp.path().join("missing")));
    }

    #[test]
    fn upd_013_read_only_dir_is_download() {
        let (layout, mut probe) = layout();
        probe.writable = false;
        assert_eq!(detect_appimage(&layout.env(), &probe), None);
        assert_eq!(detect_linux(&layout, &probe), InstallKind::Other);
        probe.writable = true;
        assert_eq!(detect_linux(&layout, &probe), InstallKind::AppImage);
    }

    /// The create-and-remove probe on a real directory leaves nothing behind and reports a
    /// missing directory as not writable.
    #[test]
    fn upd_013_writable_probe_leaves_no_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(FsProbe.dir_writable(tmp.path()));
        assert_eq!(std::fs::read_dir(tmp.path()).expect("read").count(), 0);
        assert!(!FsProbe.dir_writable(&tmp.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn upd_013_read_only_dir_is_download_on_disk() {
        use std::os::unix::fs::PermissionsExt;

        /// Restores write access so the temp dir can be removed, even when an assert fails.
        struct Unlock<'a>(&'a Path);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let apps = tmp.path().join("apps");
        let appdir = tmp.path().join("mnt");
        std::fs::create_dir_all(appdir.join("usr")).expect("mkdir");
        std::fs::create_dir_all(&apps).expect("mkdir");
        let appimage = apps.join("Dockering.AppImage");
        std::fs::write(&appimage, fake_appimage_bytes()).expect("write");
        let exe = appdir.join("usr").join("dockering");
        std::fs::write(&exe, b"exe").expect("write");
        let env = AppImageEnv {
            appimage: Some(appimage.clone().into_os_string()),
            appdir: Some(appdir.into_os_string()),
            exe,
        };

        let real = std::fs::canonicalize(&appimage).expect("canonicalize");
        assert_eq!(detect_appimage(&env, &FsProbe), Some(real));

        let _unlock = Unlock(&apps);
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        if std::fs::File::create(apps.join("root-check")).is_ok() {
            // Running as root: permission bits don't apply, so there is nothing to observe.
            return;
        }
        assert_eq!(detect_appimage(&env, &FsProbe), None);
    }

    #[test]
    fn upd_013_non_linux_is_other() {
        let (layout, probe) = layout();
        // A perfect AppImage environment on macOS, or any non-Linux, non-Windows OS.
        assert_eq!(
            detect(&layout.exe, false, false, None, &[], &layout.env(), &probe),
            InstallKind::Other
        );
        assert_eq!(detect_linux(&layout, &probe), InstallKind::AppImage);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(current_appimage_path(), None);
    }

    #[test]
    fn upd_013_asset_kind_must_be_appimage() {
        use AssetKind::{Appimage, Deb, Dmg, Inno, TarGz, Unknown, Zip};

        for kind in [Inno, Zip, Dmg, Deb, TarGz, Unknown] {
            assert!(
                !asset_supports_install(InstallKind::AppImage, kind),
                "{kind:?}"
            );
        }
        assert!(asset_supports_install(InstallKind::AppImage, Appimage));

        for install in [InstallKind::InnoUser, InstallKind::InnoMachine] {
            assert!(asset_supports_install(install, Inno));
            for kind in [Zip, Dmg, Appimage, Deb, TarGz, Unknown] {
                assert!(
                    !asset_supports_install(install, kind),
                    "{install:?} {kind:?}"
                );
            }
        }
        for install in [InstallKind::Portable, InstallKind::Other] {
            for kind in [Inno, Zip, Dmg, Appimage, Deb, TarGz, Unknown] {
                assert!(
                    !asset_supports_install(install, kind),
                    "{install:?} {kind:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn upd_013_symlinked_install_resolves_to_real_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        // `~/Applications/Dockering.AppImage` is the real file; `~/bin/dockering` links to it.
        let apps = tmp.path().join("Applications");
        let bin = tmp.path().join("bin");
        let mount = tmp.path().join("mnt").join(".mount_dockerX");
        for dir in [&apps, &bin, &mount.join("usr").join("bin")] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let real = apps.join("Dockering.AppImage");
        std::fs::write(&real, fake_appimage_bytes()).expect("write");
        let link = bin.join("dockering");
        symlink(&real, &link).expect("symlink");
        let exe = mount.join("usr").join("bin").join("dockering");
        std::fs::write(&exe, b"exe").expect("write");
        // The mount point itself is reached through a symlink too.
        let mount_link = tmp.path().join("mnt-link");
        symlink(&mount, &mount_link).expect("symlink");

        let expected = std::fs::canonicalize(&real).expect("canonicalize");
        for (appimage, appdir) in [
            (&link, &mount),
            (&real, &mount),
            (&link, &mount_link),
            (&real, &mount_link),
        ] {
            let env = AppImageEnv {
                appimage: Some(appimage.clone().into_os_string()),
                appdir: Some(appdir.clone().into_os_string()),
                exe: exe.clone(),
            };
            assert_eq!(
                detect_appimage(&env, &FsProbe),
                Some(expected.clone()),
                "{appimage:?} {appdir:?}"
            );
            assert_eq!(
                detect(&exe, false, true, None, &[], &env, &FsProbe),
                InstallKind::AppImage
            );
        }

        // The directory that must be writable is the real file's, not the link's.
        assert_eq!(
            expected.parent(),
            std::fs::canonicalize(&apps).ok().as_deref()
        );

        // A link to something that isn't an AppImage is refused.
        let other = bin.join("not-an-appimage");
        std::fs::write(&other, b"#!/bin/sh\n").expect("write");
        let other_link = bin.join("other");
        symlink(&other, &other_link).expect("symlink");
        let env = AppImageEnv {
            appimage: Some(other_link.into_os_string()),
            appdir: Some(mount.into_os_string()),
            exe,
        };
        assert_eq!(detect_appimage(&env, &FsProbe), None);
    }
}
