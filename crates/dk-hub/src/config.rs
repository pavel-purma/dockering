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
