//! Configuration & persistence (spec 10 §6): `config.toml` (settings + manual engines) and
//! `state.json` (window/UI state). Loaded synchronously before the first window; saved
//! asynchronously on the hub runtime with a 500 ms debounce, atomically (temp + rename).

use std::collections::BTreeMap;

use dk_core::grouping::{DEFAULT_GROUP_LABEL_KEY, GroupBy};
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
    pub updates: UpdateSettings,
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
            updates: Default::default(),
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
    /// The pinned default engine (ENG-116): connects first at startup (ENG-103). Changing the
    /// active engine at runtime never touches it.
    pub default: Option<EngineId>,
}

/// SET-020
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredContainerSettings")]
pub struct ContainerSettings {
    pub group_by: GroupBy,
    /// Label key used by *Combined* and *Label* (CON-014). Never empty after loading.
    pub group_label_key: String,
    pub show_cpu_mem_columns: bool,
    /// Polling interval fallback in seconds (spec 10 §4.2).
    pub polling_interval_s: u32,
    /// Ungrouped containers interleaved with groups by sort key (spec 21 §4 rule 3).
    pub interleave_ungrouped: bool,
}

impl Default for ContainerSettings {
    fn default() -> Self {
        Self {
            group_by: GroupBy::Combined,
            group_label_key: DEFAULT_GROUP_LABEL_KEY.to_owned(),
            show_cpu_mem_columns: false,
            polling_interval_s: 5,
            interleave_ungrouped: false,
        }
    }
}

/// On-disk shape of `[containers]`, tolerant of files written before `group_label_key`, when
/// the *Label* mode carried its key inside `[containers.group_by]` (`type = "label"`, `key`).
#[derive(Deserialize)]
#[serde(default)]
struct StoredContainerSettings {
    group_by: StoredGroupBy,
    group_label_key: Option<String>,
    show_cpu_mem_columns: bool,
    polling_interval_s: u32,
    interleave_ungrouped: bool,
}

#[derive(Deserialize)]
struct StoredGroupBy {
    #[serde(flatten)]
    by: GroupBy,
    key: Option<String>,
}

impl Default for StoredContainerSettings {
    fn default() -> Self {
        let d = ContainerSettings::default();
        Self {
            group_by: StoredGroupBy {
                by: d.group_by,
                key: None,
            },
            group_label_key: None,
            show_cpu_mem_columns: d.show_cpu_mem_columns,
            polling_interval_s: d.polling_interval_s,
            interleave_ungrouped: d.interleave_ungrouped,
        }
    }
}

fn non_empty_key(key: Option<&str>) -> Option<&str> {
    key.map(str::trim).filter(|key| !key.is_empty())
}

impl From<StoredContainerSettings> for ContainerSettings {
    fn from(s: StoredContainerSettings) -> Self {
        let legacy_key = match s.group_by.by {
            GroupBy::Label => non_empty_key(s.group_by.key.as_deref()),
            _ => None,
        };
        let group_label_key = non_empty_key(s.group_label_key.as_deref())
            .or(legacy_key)
            .unwrap_or(DEFAULT_GROUP_LABEL_KEY)
            .to_owned();
        Self {
            group_by: s.group_by.by,
            group_label_key,
            show_cpu_mem_columns: s.show_cpu_mem_columns,
            polling_interval_s: s.polling_interval_s,
            interleave_ungrouped: s.interleave_ungrouped,
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

/// SET-090 / UPD-005
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// *Check for updates automatically* (default on).
    pub check: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check: true }
    }
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
    /// Updater bookkeeping (UPD-012).
    pub updates: crate::updates::UpdateState,
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
        c.engines.default = Some(EngineId::new("remote"));
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

        // A file written before ENG-114 still carries `unmerged`; it's ignored and dropped.
        std::fs::write(
            paths.config_file(),
            "[engines]
unmerged = [\"x\"]
default = \"a\"
",
        )
        .unwrap();
        let (c3b, _) = load(&paths);
        assert_eq!(c3b.engines.default, Some(EngineId::new("a")));
        save_config(&paths, &c3b).unwrap();
        assert!(
            !std::fs::read_to_string(paths.config_file())
                .unwrap()
                .contains("unmerged")
        );

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

    fn load_containers(toml: &str) -> ContainerSettings {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.config_file(), toml).unwrap();
        load(&paths).0.containers
    }

    #[test]
    fn con_014_default_is_combined_with_part_of_key() {
        let d = ContainerSettings::default();
        assert_eq!(d.group_by, GroupBy::Combined);
        assert_eq!(d.group_label_key, "app.kubernetes.io/part-of");
        assert_eq!(d.group_label_key, DEFAULT_GROUP_LABEL_KEY);
        assert_eq!(load_containers("[general]\ntheme = \"dark\"\n"), d);
        assert_eq!(
            load_containers("[containers]\npolling_interval_s = 9\n").group_by,
            d.group_by
        );
    }

    #[test]
    fn con_014_group_by_roundtrips_in_the_new_shape() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        for by in [
            GroupBy::Combined,
            GroupBy::Compose,
            GroupBy::None,
            GroupBy::Label,
        ] {
            let mut c = Config::default();
            c.containers.group_by = by;
            c.containers.group_label_key = "com.example.stack".into();
            save_config(&paths, &c).unwrap();
            let text = std::fs::read_to_string(paths.config_file()).unwrap();
            assert!(
                text.contains("group_label_key = \"com.example.stack\""),
                "{text}"
            );
            assert_eq!(load(&paths).0, c, "{by:?}");
        }
    }

    #[test]
    fn con_014_legacy_label_group_by_migrates() {
        let legacy = "version = 1

[containers]
show_cpu_mem_columns = true
polling_interval_s = 7
interleave_ungrouped = true

[containers.group_by]
type = \"label\"
key = \"app\"

[logs]
initial_tail = 200
";
        let c = load_containers(legacy);
        assert_eq!(c.group_by, GroupBy::Label);
        assert_eq!(c.group_label_key, "app");
        assert!(c.show_cpu_mem_columns && c.interleave_ungrouped);
        assert_eq!(c.polling_interval_s, 7);

        // A legacy label without a usable key keeps the default key.
        let blank = "[containers.group_by]\ntype = \"label\"\nkey = \"  \"\n";
        let c = load_containers(blank);
        assert_eq!(
            (c.group_by, c.group_label_key.as_str()),
            (GroupBy::Label, DEFAULT_GROUP_LABEL_KEY)
        );
        let missing = "[containers.group_by]\ntype = \"label\"\n";
        let c = load_containers(missing);
        assert_eq!(
            (c.group_by, c.group_label_key.as_str()),
            (GroupBy::Label, DEFAULT_GROUP_LABEL_KEY)
        );

        // The migrated file is written back in the new shape.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.config_file(), legacy).unwrap();
        let (migrated, _) = load(&paths);
        save_config(&paths, &migrated).unwrap();
        let text = std::fs::read_to_string(paths.config_file()).unwrap();
        assert!(text.contains("group_label_key = \"app\""), "{text}");
        let table: toml::Table = text.parse().unwrap();
        let group_by = table["containers"]["group_by"].as_table().unwrap();
        assert_eq!(group_by.len(), 1, "{text}");
        assert_eq!(group_by["type"].as_str(), Some("label"));
        assert_eq!(load(&paths).0, migrated);
    }

    #[test]
    fn con_014_legacy_compose_and_none_load_unchanged() {
        for (kind, by) in [("compose", GroupBy::Compose), ("none", GroupBy::None)] {
            let c = load_containers(&format!("[containers.group_by]\ntype = \"{kind}\"\n"));
            assert_eq!(c.group_by, by);
            assert_eq!(c.group_label_key, DEFAULT_GROUP_LABEL_KEY);
        }
        let c = load_containers("[containers.group_by]\ntype = \"combined\"\n");
        assert_eq!(c.group_by, GroupBy::Combined);
    }

    #[test]
    fn con_014_stale_legacy_key_never_overrides_the_new_key() {
        let c = load_containers(
            "[containers]\ngroup_label_key = \"stack\"\n[containers.group_by]\ntype = \"label\"\nkey = \"app\"\n",
        );
        assert_eq!(
            (c.group_by, c.group_label_key.as_str()),
            (GroupBy::Label, "stack")
        );
        let c = load_containers("[containers.group_by]\ntype = \"compose\"\nkey = \"app\"\n");
        assert_eq!(c.group_label_key, DEFAULT_GROUP_LABEL_KEY);
    }

    #[test]
    fn con_014_empty_label_key_normalised() {
        for key in ["", "   ", "\\t"] {
            let c = load_containers(&format!("[containers]\ngroup_label_key = \"{key}\"\n"));
            assert_eq!(c.group_label_key, DEFAULT_GROUP_LABEL_KEY, "{key:?}");
        }
        let c = load_containers("[containers]\ngroup_label_key = \"  com.example.stack \"\n");
        assert_eq!(c.group_label_key, "com.example.stack");
    }

    #[test]
    fn con_014_corrupt_group_by_is_backed_up_and_defaulted() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let bad = "[containers.group_by]\ntype = \"sideways\"\n";
        std::fs::write(paths.config_file(), bad).unwrap();
        let (c, _) = load(&paths);
        assert_eq!(c, Config::default());
        assert!(!paths.config_file().exists());
        assert_eq!(
            std::fs::read_to_string(paths.config_dir.join("config.toml.bak")).unwrap(),
            bad
        );
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
