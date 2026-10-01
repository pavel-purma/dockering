//! Config/data/log directories (spec 10 §6), via `directories::ProjectDirs("dev", "dockering", "Dockering")`.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `config.toml`
    pub config_dir: PathBuf,
    /// `state.json`, crash files
    pub data_dir: PathBuf,
    /// `logs/dockering.log*`
    pub log_dir: PathBuf,
}

impl Paths {
    /// Per-user OS directories. `None` if no home directory can be determined.
    pub fn for_user() -> Option<Paths> {
        unimplemented!("rust-core")
    }

    /// Everything under one root (tests, portable mode).
    pub fn in_dir(root: impl Into<PathBuf>) -> Paths {
        let root = root.into();
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            log_dir: root.join("logs"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
}
