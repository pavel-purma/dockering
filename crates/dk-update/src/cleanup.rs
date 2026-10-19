//! Cleanup of the updates directory (UPD-012).
//!
//! The hub runs this at start and after a check, under its check lock, with the versions that
//! must survive (the [`KeepSet`]). It removes only what is provably dead:
//!
//! - `*.part` files (an interrupted download), in the directory itself and in every kept version
//!   directory,
//! - a stale [`OFFER_TMP_FILE`] left by a crashed `offer.json` write,
//! - version directories (a directory named like a SemVer version) outside the keep set.
//!
//! It **never** removes `offer.json` and never touches anything it doesn't recognise, so a user's
//! own file in the directory survives. It doesn't follow symlinks. Errors are ignored: cleanup
//! must never block a start or a check. It is synchronous; the hub calls it from a blocking
//! worker.

use std::fs;
use std::path::Path;

use semver::Version;

use crate::offer::OFFER_TMP_FILE;

/// The version directories cleanup must keep (UPD-012): the *offered* one, the one being
/// *applied* (nothing deletes the payload of a running apply, UPD-011) and every one that is
/// being *downloaded* or whose finished download is held until an apply ends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeepSet {
    /// The version of the retained offer.
    pub offered: Option<String>,
    /// The version an apply is running for.
    pub applying: Option<String>,
    /// Versions being downloaded, or downloaded and waiting to become the offer.
    pub downloading: Vec<String>,
}

impl KeepSet {
    /// Whether the directory named `version` must stay.
    #[must_use]
    pub fn keeps(&self, version: &str) -> bool {
        self.offered.as_deref() == Some(version)
            || self.applying.as_deref() == Some(version)
            || self.downloading.iter().any(|v| v == version)
    }
}

/// Cleans `dir` (`<data-local>/updates`) as described in the module documentation.
pub fn cleanup(dir: &Path, keep: &KeepSet) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // `file_type` doesn't follow symlinks: a link is neither a file nor a directory here.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if keep.keeps(name) {
                remove_partials(&path);
            } else if Version::parse(name).is_ok() {
                let _ = fs::remove_dir_all(&path);
            }
        } else if kind.is_file() && (name == OFFER_TMP_FILE || is_partial(name)) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn is_partial(name: &str) -> bool {
    name.ends_with(".part")
}

fn remove_partials(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_file && entry.file_name().to_str().is_some_and(is_partial) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offer::OFFER_FILE;

    fn put(root: &Path, files: &[(&str, &str)]) {
        for (path, body) in files {
            let p = root.join(path);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, body).expect("write");
        }
    }

    fn keep(offered: Option<&str>, applying: Option<&str>, downloading: &[&str]) -> KeepSet {
        KeepSet {
            offered: offered.map(str::to_owned),
            applying: applying.map(str::to_owned),
            downloading: downloading.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    #[test]
    fn upd_012_cleanup_removes_only_part_tmp_and_other_version_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        put(
            root,
            &[
                ("0.2.0/Dockering-x86_64.AppImage", "offered"),
                ("0.2.0/Dockering-x86_64.AppImage.part", "partial"),
                ("0.3.0/Dockering-x86_64.AppImage", "applying"),
                ("0.4.0/Dockering-x86_64.AppImage", "downloading"),
                ("0.4.0/Dockering-x86_64.AppImage.part", "partial"),
                ("0.5.0/Dockering-x86_64.AppImage", "held"),
                ("0.1.9/Dockering-x86_64.AppImage", "old"),
                ("0.1.9-rc.1/Dockering-x86_64.AppImage", "old pre-release"),
                (OFFER_FILE, "{}"),
                (OFFER_TMP_FILE, "half a write"),
                ("stray.part", "x"),
                ("notes.txt", "the user's own file"),
                ("not-a-version/keep.txt", "a directory that isn't ours"),
            ],
        );
        cleanup(
            root,
            &keep(Some("0.2.0"), Some("0.3.0"), &["0.4.0", "0.5.0"]),
        );
        for kept in [
            "0.2.0/Dockering-x86_64.AppImage",
            "0.3.0/Dockering-x86_64.AppImage",
            "0.4.0/Dockering-x86_64.AppImage",
            "0.5.0/Dockering-x86_64.AppImage",
            OFFER_FILE,
            "notes.txt",
            "not-a-version/keep.txt",
        ] {
            assert!(root.join(kept).exists(), "{kept} must stay");
        }
        for gone in [
            "0.2.0/Dockering-x86_64.AppImage.part",
            "0.4.0/Dockering-x86_64.AppImage.part",
            "0.1.9",
            "0.1.9-rc.1",
            OFFER_TMP_FILE,
            "stray.part",
        ] {
            assert!(!root.join(gone).exists(), "{gone} must go");
        }

        // An empty keep set drops every version directory but still never `offer.json`.
        cleanup(root, &KeepSet::default());
        assert!(!root.join("0.2.0").exists());
        assert!(root.join(OFFER_FILE).exists());
        // A missing directory is fine.
        cleanup(&root.join("missing"), &KeepSet::default());
    }

    #[cfg(unix)]
    #[test]
    fn upd_012_cleanup_does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        put(outside.path(), &[("precious.txt", "keep me")]);
        let root = tmp.path().join("updates");
        fs::create_dir_all(&root).expect("mkdir");
        std::os::unix::fs::symlink(outside.path(), root.join("0.1.9")).expect("symlink");
        cleanup(&root, &KeepSet::default());
        assert!(outside.path().join("precious.txt").exists());
    }
}
