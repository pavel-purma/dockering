//! Version-gated ABI modules for the internal `IWSLC*` interfaces (spec 20 §5.3, ADR-0003).
//!
//! One submodule per **verified** WSL range. [`select`] maps the WSL version read from
//! `wslservice.exe` (no COM) to a module; anything else returns `None` and the caller must use
//! the CLI. Code outside `com/` never names a vtable directly.

use crate::version::WslVersion;

#[cfg(windows)]
pub mod v3_0;

/// A verified ABI module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiModule {
    /// `idl/3.0.1`, WSL 3.0.0 ..= 3.0.x.
    V3_0,
}

/// Static description of a module (also used on non-Windows for diagnostics/tests).
#[derive(Debug, Clone, Copy)]
pub struct AbiInfo {
    pub module: AbiModule,
    pub min: WslVersion,
    /// Inclusive.
    pub max: WslVersion,
    /// Vendored IDL directory (`crates/dk-engine-wslc/idl/<tag>`).
    pub idl_tag: &'static str,
}

/// Every verified module, newest first.
pub const MODULES: &[AbiInfo] = &[AbiInfo {
    module: AbiModule::V3_0,
    min: WslVersion::new(3, 0, 0, 0),
    max: WslVersion::new(3, 0, u32::MAX, u32::MAX),
    idl_tag: "3.0.1",
}];

impl AbiModule {
    pub fn info(self) -> AbiInfo {
        // MODULES has exactly one entry per variant (tested).
        *MODULES
            .iter()
            .find(|m| m.module == self)
            .unwrap_or(&MODULES[0])
    }
    pub fn name(self) -> &'static str {
        match self {
            AbiModule::V3_0 => "v3_0",
        }
    }
}

/// The ABI module verified for `v`, or `None` (→ CLI fallback). Never guesses.
pub fn select(v: &WslVersion) -> Option<AbiModule> {
    MODULES
        .iter()
        .find(|m| *v >= m.min && *v <= m.max)
        .map(|m| m.module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_verified_ranges() {
        let v = |s| WslVersion::parse(s).expect("valid");
        assert_eq!(select(&v("3.0.1.0")), Some(AbiModule::V3_0));
        assert_eq!(select(&v("3.0.0")), Some(AbiModule::V3_0));
        assert_eq!(select(&v("3.0.17.3")), Some(AbiModule::V3_0));
        // Newer / older than anything verified → CLI.
        assert_eq!(select(&v("3.1.0")), None);
        assert_eq!(select(&v("4.0.0")), None);
        assert_eq!(select(&v("2.9.13")), None);
        assert_eq!(select(&v("2.9.3")), None);
    }

    #[cfg(windows)]
    #[test]
    fn module_metadata_matches_submodule() {
        let info = AbiModule::V3_0.info();
        assert_eq!(info.min, v3_0::VERIFIED_MIN);
        assert_eq!(info.max, v3_0::VERIFIED_MAX);
        assert_eq!(info.idl_tag, v3_0::IDL_TAG);
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
