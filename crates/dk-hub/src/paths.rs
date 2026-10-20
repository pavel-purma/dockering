//! Config/data/log directories (spec 10 §6), via `directories::ProjectDirs("dev", "dockering", "Dockering")`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `config.toml`
    pub config_dir: PathBuf,
    /// `state.json`, crash files
    pub data_dir: PathBuf,
    /// `logs/dockering.log*`
    pub log_dir: PathBuf,
    /// `<data-local>/updates`: the downloaded payload and `offer.json` (UPD-012). Machine-local,
    /// never the roaming profile.
    pub updates_dir: PathBuf,
}

impl Paths {
    /// Per-user OS directories. `None` if no home directory can be determined.
    pub fn for_user() -> Option<Paths> {
        let dirs = directories::ProjectDirs::from("dev", "dockering", "Dockering")?;
        Some(Paths {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
            log_dir: dirs.data_local_dir().join("logs"),
            updates_dir: dirs.data_local_dir().join("updates"),
        })
    }

    /// Everything under one root (tests, portable mode).
    pub fn in_dir(root: impl Into<PathBuf>) -> Paths {
        let root = root.into();
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            log_dir: root.join("logs"),
            updates_dir: root.join("updates"),
        }
    }

    /// `<data-local>/updates` (UPD-012).
    pub fn updates_dir(&self) -> &Path {
        &self.updates_dir
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UPD-012: the updates directory is the one the hub used to derive from `log_dir`'s
    /// parent (the machine-local data directory), never the roaming `data_dir`.
    #[test]
    fn upd_012_updates_dir_is_next_to_the_logs() {
        let paths = Paths::in_dir("/root");
        assert_eq!(paths.updates_dir, PathBuf::from("/root").join("updates"));
        assert_eq!(paths.updates_dir(), paths.updates_dir.as_path());
        assert_eq!(
            paths.log_dir.parent().map(|p| p.join("updates")),
            Some(paths.updates_dir.clone())
        );
        assert_ne!(paths.updates_dir, paths.data_dir);

        if let Some(user) = Paths::for_user() {
            assert_eq!(
                user.log_dir.parent().map(|p| p.join("updates")),
                Some(user.updates_dir.clone())
            );
        }
    }
}
