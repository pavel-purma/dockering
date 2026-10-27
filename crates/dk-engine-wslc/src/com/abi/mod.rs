//! Version-gated ABI modules for the internal `IWSLC*` interfaces (spec 20 §5.3, ADR-0003).
//!
//! One submodule per verified exact WSL version. [`select`] maps the WSL version read from
//! `wslservice.exe` (no COM) to a module; anything else returns `None` and the caller must use
//! the CLI. Code outside `com/` never names a vtable directly.

use crate::version::WslVersion;

// The module mirrors the IDL completely (every constant, slot and size table) so offsets are
// exact and diffs against the next vendored IDL are mechanical; much of it is unused on
// purpose, and it is crate-private (ADR-0003), hence `dead_code`.
#[cfg(windows)]
#[allow(dead_code)]
pub mod v3_0;

/// A verified ABI module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiModule {
    /// IDL 3.0.1 and 3.0.3 (the same vtables; only `BuildImageOptions`, which is never called,
    /// differs). Trusted for exactly the versions listed in [`MODULES`] (ENG-132).
    V3_0,
}

/// Static description of one trusted version (also used on non-Windows for diagnostics/tests).
#[derive(Debug, Clone, Copy)]
pub struct AbiInfo {
    pub module: AbiModule,
    /// The one service FileVersion this entry trusts (exact, no range).
    pub version: WslVersion,
    /// Vendored IDL directory (`crates/dk-engine-wslc/idl/<tag>`).
    pub idl_tag: &'static str,
}

/// Every trusted version, newest first. Each needs its own vendored IDL and live evidence
/// (spec 20 §5.7).
pub const MODULES: &[AbiInfo] = &[
    AbiInfo {
        module: AbiModule::V3_0,
        version: WslVersion::new(3, 0, 3, 0),
        idl_tag: "3.0.3",
    },
    AbiInfo {
        module: AbiModule::V3_0,
        version: WslVersion::new(3, 0, 1, 0),
        idl_tag: "3.0.1",
    },
];

impl AbiModule {
    pub fn name(self) -> &'static str {
        match self {
            AbiModule::V3_0 => "v3_0",
        }
    }
}

/// The ABI module verified for exactly `v`, or `None` (→ CLI fallback). Never guesses.
pub fn select(v: &WslVersion) -> Option<AbiModule> {
    MODULES.iter().find(|m| m.version == *v).map(|m| m.module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_132_exact_allowlist() {
        let v = |s| WslVersion::parse(s).expect("valid");
        assert_eq!(select(&v("3.0.1.0")), Some(AbiModule::V3_0));
        assert_eq!(select(&v("3.0.3.0")), Some(AbiModule::V3_0));
        assert_eq!(select(&v("3.0.0")), None);
        assert_eq!(select(&v("3.0.17.3")), None);
        assert_eq!(select(&v("3.0.1.1")), None);
        // Between two admitted versions: never inferred from its neighbours.
        assert_eq!(select(&v("3.0.2.0")), None);
        assert_eq!(select(&v("3.0.3.1")), None);
        assert_eq!(select(&v("3.0.4.0")), None);
        // Newer / older than anything verified → CLI.
        assert_eq!(select(&v("3.1.0")), None);
        assert_eq!(select(&v("4.0.0")), None);
        assert_eq!(select(&v("2.9.13")), None);
        assert_eq!(select(&v("2.9.3")), None);
    }

    #[test]
    fn eng_132_one_entry_per_exact_version() {
        for (i, a) in MODULES.iter().enumerate() {
            for b in &MODULES[i + 1..] {
                assert_ne!(a.version, b.version, "duplicate allowlist entry");
                assert_ne!(a.idl_tag, b.idl_tag, "one vendored IDL per entry");
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn module_metadata_matches_submodule() {
        let listed: Vec<_> = MODULES
            .iter()
            .filter(|m| m.module == AbiModule::V3_0)
            .map(|m| (m.version, m.idl_tag))
            .collect();
        assert_eq!(listed, v3_0::VERIFIED);
    }

    #[test]
    fn vendored_idl_exists_for_every_module() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("idl");
        for m in MODULES {
            for f in ["wslc.idl", "WSLCShared.idl"] {
                assert!(root.join(m.idl_tag).join(f).is_file(), "{} {f}", m.idl_tag);
            }
        }
    }
}
