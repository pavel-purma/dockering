//! Configuration & persistence (spec 10 §6): `config.toml` (settings + manual engines) and
//! `state.json` (window/UI state). Loaded synchronously before the first window; saved
//! asynchronously on the hub runtime with a 500 ms debounce, atomically (temp + rename).

use std::collections::BTreeMap;

use dk_core::grouping::GroupBy;
use dk_core::{EngineConfig, EngineId};
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPage {
    #[default]
    Containers,
    Images,
    Volumes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    #[default]
    Info,
    Debug,
}

/// User settings (`config.toml`). Every section has forward-compatible defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub general: GeneralSettings,
    pub engines: EngineSettings,
    pub containers: ContainerSettings,
    pub logs: LogSettings,
    pub terminal: TerminalSettings,
    pub stats: StatsSettings,
    pub diagnostics: DiagnosticsSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            general: Default::default(),
            engines: Default::default(),
            containers: Default::default(),
            logs: Default::default(),
            terminal: Default::default(),
            stats: Default::default(),
            diagnostics: Default::default(),
        }
    }
}

/// SET-001
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub theme: ThemeMode,
    pub start_page: StartPage,
    pub confirm_delete_stopped: bool,
    pub show_networks_page: bool,
    /// UI zoom factor (SHL-024), 1.0 = 100 %.
    pub ui_scale: f32,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            start_page: StartPage::Containers,
            confirm_delete_stopped: true,
            show_networks_page: true,
            ui_scale: 1.0,
        }
    }
}

/// SET-010 + manual engines + per-engine overrides of discovered ones.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub show_all_wsl_distros: bool,
    pub show_all_wslc_sessions: bool,
    /// Manually added engines and user overrides (rename/disable/hide) of discovered ones.
    /// Manual entries win over discovered ones with the same id.
    pub entries: Vec<EngineConfig>,
    /// Engine ids the user un-merged from daemon-id de-duplication (ENG-009).
    pub unmerged: Vec<EngineId>,
}

/// SET-020
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerSettings {
    pub group_by: GroupBy,
    pub show_cpu_mem_columns: bool,
    /// Polling interval fallback in seconds (spec 10 §4.2).
    pub polling_interval_s: u32,
    /// Ungrouped containers interleaved with groups by sort key (spec 21 §4 rule 3).
    pub interleave_ungrouped: bool,
}

impl Default for ContainerSettings {
    fn default() -> Self {
        Self {
            group_by: GroupBy::Compose,
            show_cpu_mem_columns: false,
            polling_interval_s: 5,
            interleave_ungrouped: false,
        }
    }
}

/// SET-030
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    pub initial_tail: u32,
    pub max_lines: u32,
    pub timestamps: bool,
    pub wrap: bool,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            initial_tail: 1000,
            max_lines: 50_000,
            timestamps: false,
            wrap: false,
        }
    }
}

/// SET-040
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    /// Empty = platform monospace.
    pub font_family: String,
    pub font_size: f32,
    /// Empty = auto-detect (TRM-004).
    pub default_shell: String,
    pub scrollback_lines: u32,
    /// External terminal command template (TRM-009), argv-split, `{cmd}` placeholder.
    pub external_terminal: String,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 13.0,
            default_shell: String::new(),
            scrollback_lines: 10_000,
            external_terminal: String::new(),
        }
    }
}

/// SET-050
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsSettings {
    pub history_minutes: u32,
    pub cpu_relative_to_all_cores: bool,
}

impl Default for StatsSettings {
    fn default() -> Self {
        Self {
            history_minutes: 15,
            cpu_relative_to_all_cores: false,
        }
    }
}

/// SET-060
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsSettings {
    pub log_level: LogLevel,
}

/// Window & UI state (`state.json`, SHL-011).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub version: u32,
    pub window: Option<WindowBounds>,
    pub sidebar_collapsed: bool,
    pub last_engine: Option<EngineId>,
    /// Serialized route of the last list page.
    pub last_route: Option<String>,
    /// Column widths per table id.
    pub column_widths: BTreeMap<String, Vec<f32>>,
    /// Per engine: per-group expanded state (CON-011).
    pub expanded_groups: BTreeMap<String, BTreeMap<String, bool>>,
    /// Per engine: container status filter (CON-004).
    pub container_filter: BTreeMap<String, String>,
    /// First-run screen dismissed.
    pub first_run_done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

// ── persistence (sync; startup load + saves from a blocking thread) ─────────────────────────

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::paths::Paths;

/// A fresh temp file name next to `path` (same directory → `rename` is atomic). The random
/// suffix keeps concurrent saves to the same path (in this or another process) from sharing
/// a temp file; callers open it with `create_new` so a collision fails instead of clobbering.
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    path.with_file_name(format!(
        ".{name}.{}.{:016x}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ))
}

/// Atomic write: temp file in the same directory + fsync + rename; creates parent dirs.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

pub(crate) fn save_config(paths: &Paths, c: &Config) -> std::io::Result<()> {
    let text = toml::to_string_pretty(c).map_err(std::io::Error::other)?;
    write_atomic(&paths.config_file(), text.as_bytes())
}

pub(crate) fn save_ui_state(paths: &Paths, s: &UiState) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    write_atomic(&paths.state_file(), &text)
}

/// Moves a corrupt file aside to `<file>.bak` (replacing an older backup).
fn back_up(path: &Path, err: &dyn std::fmt::Display) {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    tracing::warn!(file = %path.display(), error = %err, backup = %bak.display(),
        "corrupt settings file; using defaults");
    let _ = std::fs::remove_file(&bak);
    if let Err(e) = std::fs::rename(path, &bak) {
        tracing::warn!(file = %path.display(), error = %e, "couldn't back up corrupt file");
    }
}

fn load_file<T: Default>(path: &Path, parse: impl FnOnce(&str) -> Result<T, String>) -> T {
    let text = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "couldn't read settings file");
            return T::default();
        }
    };
    let parsed = String::from_utf8(text)
        .map_err(|e| e.to_string())
        .and_then(|t| parse(&t));
    match parsed {
        Ok(v) => v,
        Err(e) => {
            back_up(path, &e);
            T::default()
        }
    }
}

/// See `crate::load_config`.
pub(crate) fn load(paths: &Paths) -> (Config, UiState) {
    let config: Config = load_file(&paths.config_file(), |t| {
        toml::from_str(t).map_err(|e| e.to_string())
    });
    let mut state: UiState = load_file(&paths.state_file(), |t| {
        serde_json::from_str(t).map_err(|e| e.to_string())
    });
    if state.version == 0 {
        state.version = CONFIG_VERSION;
    }
    (config, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::{EngineEndpoint, EngineOrigin};

    #[test]
    fn config_roundtrip_and_corrupt_file_backup() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());

        // Missing → defaults.
        let (c, s) = load(&paths);
        assert_eq!(c, Config::default());
        assert_eq!(s.version, CONFIG_VERSION);

        // Round-trip.
        let mut c = Config::default();
        c.general.theme = ThemeMode::Dark;
        c.stats.history_minutes = 5;
        c.engines.entries.push(EngineConfig {
            id: EngineId::new("remote"),
            name: "Remote".into(),
            endpoint: EngineEndpoint::Tcp {
                host: "10.0.0.2".into(),
                port: 2376,
                tls: None,
            },
            origin: EngineOrigin::Manual,
            enabled: true,
            hidden: false,
        });
        let s = UiState {
            last_engine: Some(EngineId::new("remote")),
            sidebar_collapsed: true,
            ..Default::default()
        };
        save_config(&paths, &c).unwrap();
        save_ui_state(&paths, &s).unwrap();
        let (c2, s2) = load(&paths);
        assert_eq!(c2, c);
        assert_eq!(s2.last_engine, s.last_engine);
        assert!(s2.sidebar_collapsed);

        // Forward-compatible defaults for missing sections/fields.
        std::fs::write(paths.config_file(), "[general]\ntheme = \"light\"\n").unwrap();
        let (c3, _) = load(&paths);
        assert_eq!(c3.general.theme, ThemeMode::Light);
        assert_eq!(c3.stats, StatsSettings::default());

        // Corrupt → `.bak` + defaults.
        std::fs::write(paths.config_file(), "this is = = not toml [").unwrap();
        std::fs::write(paths.state_file(), "{ nope").unwrap();
        let (c4, s4) = load(&paths);
        assert_eq!(c4, Config::default());
        assert_eq!(s4.last_engine, None);
        assert!(!paths.config_file().exists());
        let bak = paths.config_dir.join("config.toml.bak");
        assert_eq!(
            std::fs::read_to_string(bak).unwrap(),
            "this is = = not toml ["
        );
        assert!(paths.data_dir.join("state.json.bak").exists());
    }

    #[test]
    fn atomic_write_creates_dirs_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn temp_paths_are_unique_hidden_siblings() {
        let p = Path::new("/x/y/config.toml");
        let a = temp_path(p);
        let b = temp_path(p);
        assert_ne!(a, b);
        assert_eq!(a.parent(), p.parent());
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(".config.toml.") && name.ends_with(".tmp"),
            "{name}"
        );
    }

    #[test]
    fn concurrent_atomic_writes_to_one_path_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let payloads: Vec<Vec<u8>> = (0..8u8).map(|i| vec![b'a' + i; 4096]).collect();
        std::thread::scope(|s| {
            for bytes in &payloads {
                let p = &p;
                s.spawn(move || {
                    for _ in 0..10 {
                        write_atomic(p, bytes).unwrap();
                    }
                });
            }
        });
        let got = std::fs::read(&p).unwrap();
        assert!(payloads.contains(&got), "torn or mixed content");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("state.json")]);
    }
}
