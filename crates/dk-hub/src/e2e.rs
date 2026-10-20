//! The update end-to-end test seam (REL-086, REL-085).
//!
//! Everything here is behind the Cargo feature `update-e2e`, which is off by default and never
//! part of a shipped build. Without the feature this module only offers [`e2e_quit_delay`], a
//! no-op, so `main()` needs no `cfg`. With it, the `DOCKERING_UPDATE_E2E_*` variables can point
//! the updater at a loopback server, give it a test key and shorten its schedule, and a marker
//! string in the binary lets the release workflow prove a shipped binary has none of this.
//! Every variable is read through an injectable lookup ([`seam::EnvFn`]); production code passes
//! the process environment and tests pass a map, so no test sets a process variable.

/// Delays the process exit by `DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS` seconds (S4: the old app
/// leaves late). `main()` calls it as its **last** statement, after `run()` has returned and the
/// hub and the single-instance guard are gone, so a relaunch handshake that only worked because
/// shutdown was fast shows up as a failure. Does nothing without the feature or the variable.
///
/// It sleeps on the calling thread on purpose: the process is already past its UI, and keeping
/// the sleep here keeps blocking calls out of `crates/dockering/src` (NFR-001).
pub fn e2e_quit_delay() {
    #[cfg(feature = "update-e2e")]
    seam::quit_delay(&seam::process_env);
}

#[cfg(feature = "update-e2e")]
pub(crate) mod seam {
    use std::time::Duration;

    pub(crate) use crate::updates::EnvFn;

    /// Base URL replacing scheme and host of every update fetch.
    pub(crate) const URL_VAR: &str = "DOCKERING_UPDATE_E2E_URL";
    /// Minisign public key (base64) replacing the embedded keys.
    pub(crate) const KEY_VAR: &str = "DOCKERING_UPDATE_E2E_KEY";
    /// Seconds until the first automatic check.
    pub(crate) const FIRST_CHECK_VAR: &str = "DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS";
    /// Seconds between automatic checks (no jitter).
    pub(crate) const INTERVAL_VAR: &str = "DOCKERING_UPDATE_E2E_INTERVAL_SECS";
    /// Seconds the process lingers after the app has shut down.
    pub(crate) const QUIT_DELAY_VAR: &str = "DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS";

    /// The release workflow's scan target (REL-084, REL-086): `#[used]` keeps it in the object
    /// file and [`quit_delay`] reads it, so neither the compiler nor the linker's garbage
    /// collection drops it from the unpackaged binary. Do not write this string anywhere that is
    /// compiled without the feature.
    #[used]
    static MARKER: [u8; 28] = *b"dockering-update-e2e-seam-v1";

    /// The process environment (the only place the seam touches it); an [`EnvFn`].
    pub(crate) fn process_env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn text(env: EnvFn<'_>, name: &str) -> Option<String> {
        env(name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }

    /// A whole number of seconds; anything else is ignored (with a warning).
    fn secs(env: EnvFn<'_>, name: &str) -> Option<Duration> {
        let raw = text(env, name)?;
        match raw.parse::<u64>() {
            Ok(n) => Some(Duration::from_secs(n)),
            Err(_) => {
                tracing::warn!(
                    variable = name,
                    "update e2e seam: ignoring a non-numeric value"
                );
                None
            }
        }
    }

    /// The updater overrides read from the environment. `Default` (all `None`) is the
    /// production behaviour.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub(crate) struct Seam {
        pub(crate) base_url: Option<String>,
        pub(crate) key: Option<String>,
        pub(crate) first_check: Option<Duration>,
        pub(crate) interval: Option<Duration>,
    }

    impl Seam {
        pub(crate) fn from_env(env: EnvFn<'_>) -> Self {
            Self {
                base_url: text(env, URL_VAR),
                key: text(env, KEY_VAR),
                first_check: secs(env, FIRST_CHECK_VAR),
                interval: secs(env, INTERVAL_VAR),
            }
        }
    }

    pub(crate) fn quit_delay_from(env: EnvFn<'_>) -> Option<Duration> {
        secs(env, QUIT_DELAY_VAR)
    }

    pub(crate) fn quit_delay(env: EnvFn<'_>) {
        std::hint::black_box(&MARKER);
        if let Some(delay) = quit_delay_from(env) {
            tracing::info!(secs = delay.as_secs(), "update e2e seam: delaying exit");
            std::thread::sleep(delay);
        }
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;

        use super::*;

        fn map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
            let m: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            move |k| m.get(k).cloned()
        }

        #[test]
        fn upd_005_e2e_seam_parses_injected_env() {
            let env = map(&[
                (URL_VAR, " http://127.0.0.1:9 "),
                (KEY_VAR, "RWQabc"),
                (FIRST_CHECK_VAR, "3"),
                (INTERVAL_VAR, "10"),
                (QUIT_DELAY_VAR, "7"),
            ]);
            assert_eq!(
                Seam::from_env(&env),
                Seam {
                    base_url: Some("http://127.0.0.1:9".into()),
                    key: Some("RWQabc".into()),
                    first_check: Some(Duration::from_secs(3)),
                    interval: Some(Duration::from_secs(10)),
                }
            );
            assert_eq!(quit_delay_from(&env), Some(Duration::from_secs(7)));
        }

        #[test]
        fn upd_005_e2e_seam_ignores_empty_and_malformed_values() {
            assert_eq!(Seam::from_env(&|_: &str| None), Seam::default());
            assert_eq!(quit_delay_from(&|_: &str| None), None);
            let env = map(&[
                (URL_VAR, "  "),
                (KEY_VAR, ""),
                (FIRST_CHECK_VAR, "soon"),
                (INTERVAL_VAR, "-5"),
                (QUIT_DELAY_VAR, "1.5"),
            ]);
            assert_eq!(Seam::from_env(&env), Seam::default());
            assert_eq!(quit_delay_from(&env), None);
        }

        #[test]
        fn upd_005_e2e_quit_delay_without_variable_returns_at_once() {
            // A zero delay sleeps for no time; an absent or bad value never sleeps.
            quit_delay(&map(&[(QUIT_DELAY_VAR, "0")]));
            quit_delay(&map(&[(QUIT_DELAY_VAR, "later")]));
            quit_delay(&|_: &str| None);
        }

        #[test]
        fn rel_086_marker_is_exactly_the_string_the_release_scan_greps_for() {
            assert_eq!(&MARKER, b"dockering-update-e2e-seam-v1");
        }
    }
}
