//! Stale AppImage staging files (UPD-012).

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Every staging file of an AppImage update starts with this, so a crash between staging and the
/// `rename` leaves something the next start can recognise and remove (UPD-012, UPD-014). The
/// directory-writable probe of `detect_appimage` (UPD-013) uses the same prefix.
pub const STAGING_PREFIX: &str = ".dockering-update-";

/// UPD-012: removes the `.dockering-update-*` regular files in `dir` whose modification time is
/// older than `max_age` before `now` (the hub passes one hour), and returns how many it removed.
///
/// Only that name pattern is touched: other files, directories and symbolic links are never
/// removed (a link is not followed either), and a file modified in the future counts as new. A
/// missing or unreadable `dir`, or a file that can't be removed, is skipped silently: this is
/// best-effort housekeeping at start-up.
pub fn sweep_stale_staging(dir: &Path, max_age: Duration, now: SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .as_encoded_bytes()
            .starts_with(STAGING_PREFIX.as_bytes())
        {
            continue;
        }
        // `symlink_metadata`: a link is neither followed nor removed.
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if stale && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn make(dir: &Path, name: &str, age: Duration, now: SystemTime) -> std::path::PathBuf {
        let path = dir.join(name);
        let file = File::create(&path).expect("create");
        file.set_modified(now - age).expect("set mtime");
        path
    }

    #[test]
    fn upd_012_staging_sweep_by_age() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let old = make(dir.path(), ".dockering-update-1", 2 * HOUR, now);
        let old_probe = make(dir.path(), ".dockering-update-probe-9-0", 5 * HOUR, now);
        let fresh = make(
            dir.path(),
            ".dockering-update-2",
            Duration::from_secs(60),
            now,
        );
        let exactly = make(
            dir.path(),
            ".dockering-update-3",
            HOUR - Duration::from_secs(1),
            now,
        );
        let future = dir.path().join(".dockering-update-4");
        File::create(&future)
            .expect("create")
            .set_modified(now + HOUR)
            .expect("set mtime");
        // Never touched, however old: other names, near misses, and a directory.
        let appimage = make(dir.path(), "Dockering.AppImage", 30 * 24 * HOUR, now);
        let lookalike = make(dir.path(), ".dockering-updated", 2 * HOUR, now);
        let unrelated = make(dir.path(), "dockering-update-1", 2 * HOUR, now);
        let subdir = dir.path().join(".dockering-update-dir");
        fs::create_dir(&subdir).expect("mkdir");

        assert_eq!(sweep_stale_staging(dir.path(), HOUR, now), 2);

        assert!(!old.exists() && !old_probe.exists());
        for kept in [&fresh, &exactly, &future, &appimage, &lookalike, &unrelated] {
            assert!(kept.exists(), "{}", kept.display());
        }
        assert!(subdir.is_dir());
        // A second sweep finds nothing, and a missing directory is not an error.
        assert_eq!(sweep_stale_staging(dir.path(), HOUR, now), 0);
        assert_eq!(
            sweep_stale_staging(&dir.path().join("missing"), HOUR, now),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn upd_012_staging_sweep_never_follows_or_removes_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let outside = tempfile::tempdir().expect("tempdir");
        let victim = make(outside.path(), "victim", 9 * HOUR, now);
        let link = dir.path().join(".dockering-update-link");
        std::os::unix::fs::symlink(&victim, &link).expect("symlink");
        assert_eq!(sweep_stale_staging(dir.path(), HOUR, now), 0);
        assert!(victim.exists());
        assert!(link.symlink_metadata().is_ok());
    }
}
