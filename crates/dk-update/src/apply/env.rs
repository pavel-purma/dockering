//! The scrubbed child environment of an AppImage update (UPD-014).
//!
//! The running app lives inside an AppImage mount, and its `AppRun` has put that mount into the
//! environment (`APPDIR`, `PATH`, `LD_LIBRARY_PATH`, …). A child started from the old process
//! must not inherit any of it: the new AppImage's own `AppRun` sets up its own. The rules are
//! pure ([`scrubbed_env`]) so they run, and are tested, on every OS; the self-test and the
//! relaunch apply the same [`EnvPlan`] to the child's `Command`, never to the running process
//! (Rust 2024: `set_var`/`remove_var` are `unsafe`, and a changed process environment would leak
//! into terminal shells and `docker` children).

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

/// Variables removed whatever their value: they describe the old AppImage instance.
const REMOVED: [&str; 4] = ["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

/// Colon-separated lists the old `AppRun` prepends its own directories to.
const LISTS: [&str; 5] = [
    "PATH",
    "LD_LIBRARY_PATH",
    "XDG_DATA_DIRS",
    "PYTHONPATH",
    "QT_PLUGIN_PATH",
];

/// Every variable whose name starts with this is a list too (`GST_PLUGIN_SYSTEM_PATH_1_0`, …).
const GST_PREFIX: &str = "GST_PLUGIN_SYSTEM_PATH";

/// What to change in a child's environment, from [`scrubbed_env`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvPlan {
    /// Variables to remove.
    pub remove: Vec<OsString>,
    /// Variables to set to a new value.
    pub set: Vec<(OsString, OsString)>,
}

impl EnvPlan {
    /// Applies the plan to a child's builder (`env_remove` and `env`); the running process is
    /// never touched.
    pub fn apply_to(&self, command: &mut Command) {
        for name in &self.remove {
            command.env_remove(name);
        }
        for (name, value) in &self.set {
            command.env(name, value);
        }
    }
}

/// The environment of this process, as the `env` argument of the AppImage apply functions.
#[must_use]
pub fn current_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}

/// UPD-014: what to change in `vars` (the current environment) for a child of an update.
///
/// * `APPIMAGE`, `APPDIR`, `ARGV0` and `OWD` are always removed.
/// * In `PATH`, `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`, `PYTHONPATH`, `QT_PLUGIN_PATH` and every
///   variable whose name starts with `GST_PLUGIN_SYSTEM_PATH`, each colon-separated entry that
///   lies under `appdir` is dropped. The test is by path component (`/tmp/.mount_x` matches
///   `/tmp/.mount_x/usr/bin` and itself, not `/tmp/.mount_xy`); nothing is resolved on disk.
///   A variable left with nothing (no entry, or one empty entry, which could mean the current
///   directory) is **removed**; otherwise it is set to the remaining entries in their order. A
///   variable with no entry under `appdir` is not touched at all, and neither is any other
///   variable (`APPIMAGE_EXTRACT_AND_RUN` stays).
/// * Without a usable `appdir` (`None`, empty, or no directory name in it, such as `/`) no list
///   is changed; matching everything would wipe the whole `PATH`.
///
/// The lists are split on `:` on every OS, since AppImages only exist on Linux.
#[must_use]
pub fn scrubbed_env(vars: &[(OsString, OsString)], appdir: Option<&OsStr>) -> EnvPlan {
    let mut plan = EnvPlan {
        remove: REMOVED.iter().map(OsString::from).collect(),
        set: Vec::new(),
    };
    let Some(appdir) = appdir.map(Path::new).filter(|dir| usable_appdir(dir)) else {
        return plan;
    };
    // The last occurrence of a name wins, as in the environment a child inherits.
    let mut seen = HashSet::new();
    for (name, value) in vars.iter().rev() {
        if !seen.insert(name.as_os_str()) || !is_list(name) {
            continue;
        }
        let entries = split_entries(value);
        let kept: Vec<&OsStr> = entries
            .iter()
            .map(OsString::as_os_str)
            .filter(|entry| !Path::new(entry).starts_with(appdir))
            .collect();
        if kept.len() == entries.len() {
            continue;
        }
        let joined = join_entries(&kept);
        if joined.is_empty() {
            plan.remove.push(name.clone());
        } else {
            plan.set.push((name.clone(), joined));
        }
    }
    // Reverse iteration produced the later variables first; keep the plan in `vars` order.
    plan.set.reverse();
    plan.remove[REMOVED.len()..].reverse();
    plan
}

/// An `APPDIR` that names a directory: not empty and not just a root, since every absolute entry
/// lies under `/`.
fn usable_appdir(dir: &Path) -> bool {
    dir.components()
        .any(|c| matches!(c, std::path::Component::Normal(_)))
}

fn is_list(name: &OsStr) -> bool {
    LISTS.iter().any(|list| name == *list)
        || name.as_encoded_bytes().starts_with(GST_PREFIX.as_bytes())
}

#[cfg(unix)]
fn split_entries(value: &OsStr) -> Vec<OsString> {
    use std::os::unix::ffi::OsStrExt;
    value
        .as_bytes()
        .split(|byte| *byte == b':')
        .map(|entry| OsStr::from_bytes(entry).to_owned())
        .collect()
}

/// Non-Unix fallback (compiled everywhere, used nowhere that matters): a value that isn't
/// Unicode is kept as one entry.
#[cfg(not(unix))]
fn split_entries(value: &OsStr) -> Vec<OsString> {
    match value.to_str() {
        Some(text) => text.split(':').map(OsString::from).collect(),
        None => vec![value.to_owned()],
    }
}

fn join_entries(entries: &[&OsStr]) -> OsString {
    let mut joined = OsString::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            joined.push(":");
        }
        joined.push(entry);
    }
    joined
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    fn set_value<'a>(plan: &'a EnvPlan, name: &str) -> Option<&'a OsStr> {
        plan.set
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_os_str())
    }

    fn removes(plan: &EnvPlan, name: &str) -> bool {
        plan.remove.iter().any(|k| k == name)
    }

    #[test]
    fn upd_014_scrub_removes_the_appimage_variables_always() {
        for appdir in [None, Some(OsStr::new("/tmp/.mount_x"))] {
            let plan = scrubbed_env(&vars(&[("HOME", "/home/u")]), appdir);
            for name in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
                assert!(removes(&plan, name), "{name} with {appdir:?}");
            }
            assert!(plan.set.is_empty());
        }
    }

    #[test]
    fn upd_014_scrub_removes_emptied_variables() {
        let env = vars(&[
            ("LD_LIBRARY_PATH", "/tmp/.mount_x/usr/lib"),
            ("PYTHONPATH", "/tmp/.mount_x/lib:/tmp/.mount_x/lib64"),
            // AppRun prepends `$APPDIR/usr/lib:` even when the variable was empty: the trailing
            // empty entry would mean the current directory.
            ("XDG_DATA_DIRS", "/tmp/.mount_x/usr/share:"),
            ("PATH", "/tmp/.mount_x/usr/bin:/usr/bin"),
            ("QT_PLUGIN_PATH", "/usr/lib/qt"),
        ]);
        let plan = scrubbed_env(&env, Some(OsStr::new("/tmp/.mount_x")));
        assert!(removes(&plan, "LD_LIBRARY_PATH"));
        assert!(removes(&plan, "PYTHONPATH"));
        assert!(removes(&plan, "XDG_DATA_DIRS"));
        assert!(set_value(&plan, "LD_LIBRARY_PATH").is_none());
        assert!(set_value(&plan, "XDG_DATA_DIRS").is_none());
        // A variable with entries left is set, in order, and one with nothing under the
        // AppDir is not mentioned.
        assert_eq!(set_value(&plan, "PATH"), Some(OsStr::new("/usr/bin")));
        assert!(!removes(&plan, "PATH"));
        assert!(!removes(&plan, "QT_PLUGIN_PATH") && set_value(&plan, "QT_PLUGIN_PATH").is_none());
    }

    #[test]
    fn upd_014_scrub_matches_path_components_not_prefixes() {
        let env = vars(&[(
            "PATH",
            "/tmp/.mount_xy/usr/bin:/tmp/.mount_x/usr/bin:/tmp/.mount_x:/tmp/.mount_x/:/opt/tmp/.mount_x:/usr/bin:relative/.mount_x",
        )]);
        let plan = scrubbed_env(&env, Some(OsStr::new("/tmp/.mount_x")));
        assert_eq!(
            set_value(&plan, "PATH"),
            Some(OsStr::new(
                "/tmp/.mount_xy/usr/bin:/opt/tmp/.mount_x:/usr/bin:relative/.mount_x"
            ))
        );
        // A trailing slash on APPDIR changes nothing.
        let slash = scrubbed_env(&env, Some(OsStr::new("/tmp/.mount_x/")));
        assert_eq!(slash, plan);
    }

    #[test]
    fn upd_014_scrub_covers_every_listed_variable_and_gstreamer_prefix() {
        let appdir = "/tmp/.mount_x";
        let under = "/tmp/.mount_x/usr/lib";
        let env = vars(&[
            ("PATH", under),
            ("LD_LIBRARY_PATH", under),
            ("XDG_DATA_DIRS", under),
            ("PYTHONPATH", under),
            ("QT_PLUGIN_PATH", under),
            ("GST_PLUGIN_SYSTEM_PATH", under),
            (
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                &format!("{under}:/usr/lib/gst"),
            ),
            ("GST_PLUGIN_PATH", under),
            ("APPIMAGE_EXTRACT_AND_RUN", "1"),
            ("LD_PRELOAD", under),
            ("HOME", under),
        ]);
        let plan = scrubbed_env(&env, Some(OsStr::new(appdir)));
        for name in [
            "PATH",
            "LD_LIBRARY_PATH",
            "XDG_DATA_DIRS",
            "PYTHONPATH",
            "QT_PLUGIN_PATH",
            "GST_PLUGIN_SYSTEM_PATH",
        ] {
            assert!(removes(&plan, name), "{name}");
        }
        assert_eq!(
            set_value(&plan, "GST_PLUGIN_SYSTEM_PATH_1_0"),
            Some(OsStr::new("/usr/lib/gst"))
        );
        // Other variables are never touched, even when they point into the AppDir.
        for name in [
            "GST_PLUGIN_PATH",
            "APPIMAGE_EXTRACT_AND_RUN",
            "LD_PRELOAD",
            "HOME",
        ] {
            assert!(
                !removes(&plan, name) && set_value(&plan, name).is_none(),
                "{name}"
            );
        }
    }

    #[test]
    fn upd_014_scrub_without_a_usable_appdir_leaves_lists_alone() {
        let env = vars(&[("PATH", "/tmp/.mount_x/usr/bin:/usr/bin")]);
        for appdir in [None, Some(OsStr::new("")), Some(OsStr::new("/"))] {
            let plan = scrubbed_env(&env, appdir);
            assert!(plan.set.is_empty(), "{appdir:?}");
            assert_eq!(plan.remove.len(), 4, "{appdir:?}");
        }
    }

    #[test]
    fn upd_014_scrub_last_duplicate_wins_and_apply_to_sets_the_builder_only() {
        let env = vars(&[
            ("PATH", "/usr/bin"),
            ("PATH", "/tmp/.mount_x/bin:/bin"),
            ("APPDIR", "/tmp/.mount_x"),
        ]);
        let plan = scrubbed_env(&env, Some(OsStr::new("/tmp/.mount_x")));
        assert_eq!(set_value(&plan, "PATH"), Some(OsStr::new("/bin")));
        let mut command = Command::new("true");
        plan.apply_to(&mut command);
        let changes: Vec<_> = command
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
            .collect();
        assert!(changes.contains(&(OsString::from("APPDIR"), None)));
        assert!(changes.contains(&(OsString::from("PATH"), Some(OsString::from("/bin")))));
        // Nothing here changed this process.
        assert!(std::env::var_os("PATH").is_none_or(|p| p != "/bin"));
    }

    proptest! {
        /// No surviving entry is under the AppDir, no value is empty, untouched variables are
        /// not mentioned, and entries that were kept keep their order.
        #[test]
        fn upd_014_scrub_properties(
            entries in prop::collection::vec("[a-z._]{0,6}(/[a-z._]{1,6}){0,3}", 0..8),
            absolute in prop::collection::vec(any::<bool>(), 8),
        ) {
            let appdir = "/tmp/.mount_x";
            let entries: Vec<String> = entries
                .iter()
                .zip(absolute.iter().cycle())
                .map(|(e, abs)| if *abs { format!("/tmp/{e}") } else { e.clone() })
                .chain(std::iter::once("/tmp/.mount_x/usr/lib".to_owned()))
                .collect();
            let value = entries.join(":");
            let env = vars(&[("LD_LIBRARY_PATH", &value), ("HOME", "/home/u")]);
            let plan = scrubbed_env(&env, Some(OsStr::new(appdir)));
            prop_assert!(plan.set.iter().all(|(_, v)| !v.is_empty()));
            prop_assert!(!removes(&plan, "HOME") && set_value(&plan, "HOME").is_none());
            let expected: Vec<&str> = entries
                .iter()
                .map(String::as_str)
                .filter(|e| !Path::new(e).starts_with(appdir))
                .collect();
            match set_value(&plan, "LD_LIBRARY_PATH") {
                Some(v) => prop_assert_eq!(v.to_str().expect("utf-8"), expected.join(":")),
                None => {
                    prop_assert!(removes(&plan, "LD_LIBRARY_PATH"));
                    prop_assert!(expected.join(":").is_empty());
                }
            }
        }
    }
}
