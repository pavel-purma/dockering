//! WSL version detection without COM (spec 20 §5.2 step 1, spike F-9). Owner: `windows-platform`.
//!
//! The version picks the internal-ABI module (`com::abi::select`) **before** any `IWSLC*`
//! vtable is touched, so it must never come from COM itself. Sources, in order:
//! 1. FileVersion of `%ProgramFiles%\WSL\wslservice.exe` (the COM server binary).
//! 2. `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss\MSI` → `Version` (MSI install).
//!
//! On non-Windows targets every function returns `None` / `false`.

use std::fmt;
use std::str::FromStr;

/// A WSL package version, e.g. `3.0.1.0`. Ordering is numeric, field by field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WslVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub build: u32,
}

/// First WSL release with WSL containers (WSLC GA requires ≥ 2.9.3, spec 20 §5.1).
pub const MIN_WSLC: WslVersion = WslVersion::new(2, 9, 3, 0);

/// CLSID of the `WSLCSessionManager` COM class (spec 20 §5.1).
pub const WSLC_SESSION_MANAGER_CLSID: &str = "{a9b7a1b9-0671-405c-95f1-e0612cb4ce8f}";

impl WslVersion {
    pub const fn new(major: u32, minor: u32, patch: u32, build: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            build,
        }
    }

    /// Parses `"3.0.1.0"`, `"3.0.1"` or `"3.0"` (missing parts are 0). Surrounding whitespace is
    /// ignored; anything else (empty parts, signs, more than 4 parts, overflow) is rejected.
    pub fn parse(s: &str) -> Option<WslVersion> {
        let s = s.trim();
        let mut parts = [0u32; 4];
        let mut n = 0;
        for part in s.split('.') {
            if n == 4 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            parts[n] = part.parse().ok()?;
            n += 1;
        }
        if n < 2 {
            return None;
        }
        Some(Self::new(parts[0], parts[1], parts[2], parts[3]))
    }

    /// `major.minor.patch` only, the triple reported by `IWSLCSessionManager::GetVersion`.
    pub fn triple(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }

    /// Whether WSL containers can exist on this version (≥ [`MIN_WSLC`]).
    pub fn supports_wslc(&self) -> bool {
        *self >= MIN_WSLC
    }
}

impl fmt::Display for WslVersion {
    /// `3.0.1`, or `3.0.1.7` when the build part is non-zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.build != 0 {
            write!(f, ".{}", self.build)?;
        }
        Ok(())
    }
}

impl FromStr for WslVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("invalid WSL version: {s:?}"))
    }
}

/// Where a detected version came from (diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// FileVersion of `wslservice.exe`.
    ServiceBinary,
    /// `HKLM\…\Lxss\MSI\Version`.
    MsiRegistry,
}

/// The installed WSL version, without COM. `None` when WSL (the Store/MSI package) is absent.
pub fn detect() -> Option<WslVersion> {
    detect_with_source().map(|(v, _)| v)
}

/// [`detect`] plus the source that answered.
pub fn detect_with_source() -> Option<(WslVersion, VersionSource)> {
    #[cfg(windows)]
    {
        if let Some(v) = service_binary_version() {
            return Some((v, VersionSource::ServiceBinary));
        }
        crate::com::win32::reg_string_hklm(MSI_KEY, "Version")
            .and_then(|s| WslVersion::parse(&s))
            .map(|v| (v, VersionSource::MsiRegistry))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// `HKLM` key written by the WSL MSI (spike F-9).
pub const MSI_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss\MSI";

/// Path of `wslservice.exe`: `%ProgramFiles%\WSL\wslservice.exe`, or the MSI `InstallLocation`.
#[cfg(windows)]
pub fn wslservice_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let program_files = std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let default = program_files.join("WSL").join("wslservice.exe");
    if default.is_file() {
        return Some(default);
    }
    crate::com::win32::reg_string_hklm(MSI_KEY, "InstallLocation")
        .map(|dir| PathBuf::from(dir).join("wslservice.exe"))
        .filter(|p| p.is_file())
}

#[cfg(windows)]
pub(crate) fn service_binary_version() -> Option<WslVersion> {
    let path = wslservice_path()?;
    let (a, b, c, d) = crate::com::win32::file_version(&path)?;
    let v = WslVersion::new(a, b, c, d);
    // A zeroed resource is not a version.
    (v != WslVersion::default()).then_some(v)
}

/// Whether the `WSLCSessionManager` COM class is registered (`HKLM\SOFTWARE\Classes\CLSID\{…}`).
pub fn wslc_registered() -> bool {
    #[cfg(windows)]
    {
        crate::com::win32::reg_key_exists_hklm(&format!(
            r"SOFTWARE\Classes\CLSID\{WSLC_SESSION_MANAGER_CLSID}"
        ))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_two_to_four_parts() {
        assert_eq!(
            WslVersion::parse("3.0.1.0"),
            Some(WslVersion::new(3, 0, 1, 0))
        );
        assert_eq!(
            WslVersion::parse(" 3.0.1 "),
            Some(WslVersion::new(3, 0, 1, 0))
        );
        assert_eq!(WslVersion::parse("2.9"), Some(WslVersion::new(2, 9, 0, 0)));
        assert_eq!(
            WslVersion::parse("2.10.3.4"),
            Some(WslVersion::new(2, 10, 3, 4))
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        for s in [
            "",
            "3",
            "3.",
            ".3",
            "3..1",
            "3.0.1.0.0",
            "a.b",
            "-3.0",
            "3.0.x",
            "99999999999.0",
        ] {
            assert_eq!(WslVersion::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn ordering_is_numeric() {
        let v = |s| WslVersion::parse(s).expect("valid");
        assert!(v("2.10.0") > v("2.9.13"));
        assert!(v("3.0.1") > v("3.0.0.9"));
        assert!(v("2.9.3") == MIN_WSLC);
        assert!(v("2.9.2.9") < MIN_WSLC);
        assert!(v("3.0.1").supports_wslc());
        assert!(!v("2.6.1").supports_wslc());
    }

    #[test]
    fn display_omits_zero_build() {
        assert_eq!(WslVersion::new(3, 0, 1, 0).to_string(), "3.0.1");
        assert_eq!(WslVersion::new(3, 0, 1, 7).to_string(), "3.0.1.7");
        assert_eq!(
            "3.0.1".parse::<WslVersion>(),
            Ok(WslVersion::new(3, 0, 1, 0))
        );
    }

    /// Live: this machine has WSL 3.0.1 (spike F-9). Skips silently where WSL isn't installed.
    #[cfg(windows)]
    #[test]
    fn detect_reads_wslservice_file_version() {
        let Some((v, source)) = detect_with_source() else {
            eprintln!("WSL not installed; skipping");
            return;
        };
        eprintln!("detected WSL {v} via {source:?}");
        assert!(v.major >= 2);
        // CI/dev machines pin the expectation (this repo's dev box: 3.0.1, spike F-9).
        if let Ok(expected) = std::env::var("DK_EXPECT_WSL_VERSION") {
            let expected = WslVersion::parse(&expected).expect("DK_EXPECT_WSL_VERSION");
            assert_eq!(v.triple(), expected.triple());
        }
        if wslservice_path().is_some() {
            assert_eq!(source, VersionSource::ServiceBinary);
        }
        // Both sources agree when both exist.
        if let Some(msi) = crate::com::win32::reg_string_hklm(MSI_KEY, "Version")
            .and_then(|s| WslVersion::parse(&s))
        {
            assert_eq!(msi.triple(), v.triple());
        }
    }

    #[cfg(windows)]
    #[test]
    fn wslc_registration_matches_version() {
        if detect().is_some_and(|v| v.supports_wslc()) {
            assert!(
                wslc_registered(),
                "WSL ≥ 2.9.3 should register WSLCSessionManager"
            );
        }
    }
}
