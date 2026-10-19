//! The one place the running app's version is read (REL-086).
//!
//! Every production read of the version goes through [`app_version`]: the `--version` output, the
//! log header, the crash file, the About text, the updater's "am I older?" comparison and its
//! `User-Agent`. A scan test in `xtask` (`app_version_has_no_direct_uses`) fails on any other
//! use of the compile-time package-version macro, so the test seam below can never be bypassed
//! by one forgotten call site.

/// The running app's version: the Cargo package version.
///
/// Only a build with the Cargo feature `update-e2e` (REL-086, never a shipped build) may
/// override it, with the **build-time** variable `DOCKERING_E2E_VERSION` (an empty value counts
/// as unset). That is how the end-to-end test makes the *old* AppImage report `0.0.1`. The
/// variable is read with `option_env!` and only under that feature, so a normal build ignores
/// it even when it is set.
pub fn app_version() -> &'static str {
    #[cfg(feature = "update-e2e")]
    if let Some(version) = option_env!("DOCKERING_E2E_VERSION")
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return version;
    }
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_086_app_version_is_the_package_version_without_the_seam() {
        #[cfg(not(feature = "update-e2e"))]
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
        // With the seam, only the build-time variable may change it.
        #[cfg(feature = "update-e2e")]
        assert_eq!(
            app_version(),
            option_env!("DOCKERING_E2E_VERSION")
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn rel_086_app_version_looks_like_a_version() {
        let v = app_version();
        assert!(
            !v.is_empty() && v.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "{v}"
        );
        assert!(!v.contains(char::is_whitespace), "{v}");
    }
}
