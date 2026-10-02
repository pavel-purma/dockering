//! The settings table (SET-001…060): one entry per setting, with how to read and write it in
//! [`Config`], its range, and its control. The page builds its controls from this table, and
//! every change goes through `AppState::update_config` (the hub persists it, debounced).

use dk_core::grouping::GroupBy;
use dk_hub::Config;
use dk_hub::config::{LogLevel, StartPage, ThemeMode};

use crate::strings as s;

/// Settings edited through a text/number input or a select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    Theme,
    StartPage,
    GroupBy,
    LabelKey,
    Polling,
    LogsTail,
    LogsMax,
    FontFamily,
    FontSize,
    Shell,
    Scrollback,
    External,
    StatsHistory,
    LogLevel,
}

/// On/off settings (GPUI Kit `Switch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoolKey {
    ConfirmStopped,
    ShowNetworks,
    ShowStats,
    LogsTimestamps,
    LogsWrap,
    AllCores,
    ShowAllDistros,
    ShowAllSessions,
}

impl BoolKey {
    pub fn get(self, c: &Config) -> bool {
        match self {
            BoolKey::ConfirmStopped => c.general.confirm_delete_stopped,
            BoolKey::ShowNetworks => c.general.show_networks_page,
            BoolKey::ShowStats => c.containers.show_cpu_mem_columns,
            BoolKey::LogsTimestamps => c.logs.timestamps,
            BoolKey::LogsWrap => c.logs.wrap,
            BoolKey::AllCores => c.stats.cpu_relative_to_all_cores,
            BoolKey::ShowAllDistros => c.engines.show_all_wsl_distros,
            BoolKey::ShowAllSessions => c.engines.show_all_wslc_sessions,
        }
    }

    pub fn set(self, c: &mut Config, v: bool) {
        match self {
            BoolKey::ConfirmStopped => c.general.confirm_delete_stopped = v,
            BoolKey::ShowNetworks => c.general.show_networks_page = v,
            BoolKey::ShowStats => c.containers.show_cpu_mem_columns = v,
            BoolKey::LogsTimestamps => c.logs.timestamps = v,
            BoolKey::LogsWrap => c.logs.wrap = v,
            BoolKey::AllCores => c.stats.cpu_relative_to_all_cores = v,
            BoolKey::ShowAllDistros => c.engines.show_all_wsl_distros = v,
            BoolKey::ShowAllSessions => c.engines.show_all_wslc_sessions = v,
        }
    }

    /// Stable element id (GPUI Kit keys the switch's focus handle by it).
    pub fn id(self) -> &'static str {
        match self {
            BoolKey::ConfirmStopped => "set-confirm-stopped",
            BoolKey::ShowNetworks => "set-show-networks",
            BoolKey::ShowStats => "set-show-stats",
            BoolKey::LogsTimestamps => "set-logs-timestamps",
            BoolKey::LogsWrap => "set-logs-wrap",
            BoolKey::AllCores => "set-all-cores",
            BoolKey::ShowAllDistros => "set-all-distros",
            BoolKey::ShowAllSessions => "set-all-sessions",
        }
    }
}

/// A numeric setting: range, unit, and accessors.
#[derive(Clone, Copy)]
pub struct NumSpec {
    pub min: f64,
    pub max: f64,
    pub integer: bool,
    pub unit: &'static str,
    pub get: fn(&Config) -> f64,
    pub set: fn(&mut Config, f64),
}

/// A free-text setting.
#[derive(Clone, Copy)]
pub struct TextSpec {
    pub placeholder: Option<&'static str>,
    pub get: fn(&Config) -> String,
    pub set: fn(&mut Config, String),
    pub validate: fn(&str) -> Option<&'static str>,
}

/// What kind of control a [`Key`] uses.
pub enum Spec {
    Number(NumSpec),
    Text(TextSpec),
    Choice(&'static [&'static str]),
}

fn no_check(_: &str) -> Option<&'static str> {
    None
}

/// TRM-009: the command template must say where the exec command goes.
pub fn check_external(v: &str) -> Option<&'static str> {
    let v = v.trim();
    (!v.is_empty() && !v.contains("{cmd}")).then_some(s::SET_EXTERNAL_NEEDS_CMD)
}

pub const THEMES: &[&str] = &[s::THEME_SYSTEM, s::THEME_LIGHT, s::THEME_DARK];
pub const START_PAGES: &[&str] = &[s::PAGE_CONTAINERS, s::PAGE_IMAGES, s::PAGE_VOLUMES];
pub const GROUP_BYS: &[&str] = &[s::GROUP_BY_COMPOSE, s::GROUP_BY_NONE, s::GROUP_BY_LABEL];
pub const LOG_LEVELS: &[&str] = &[s::LOG_INFO, s::LOG_DEBUG];

pub fn spec(key: Key) -> Spec {
    match key {
        Key::Theme => Spec::Choice(THEMES),
        Key::StartPage => Spec::Choice(START_PAGES),
        Key::GroupBy => Spec::Choice(GROUP_BYS),
        Key::LogLevel => Spec::Choice(LOG_LEVELS),
        Key::LabelKey => Spec::Text(TextSpec {
            placeholder: Some("app"),
            get: |c| match &c.containers.group_by {
                GroupBy::Label(k) => k.clone(),
                _ => String::new(),
            },
            // Written through `group_by` (see `SettingsPage::commit_group_by`).
            set: |_, _| {},
            validate: no_check,
        }),
        Key::Polling => Spec::Number(NumSpec {
            min: 1.,
            max: 300.,
            integer: true,
            unit: s::UNIT_SECONDS,
            get: |c| c.containers.polling_interval_s as f64,
            set: |c, v| c.containers.polling_interval_s = v as u32,
        }),
        Key::LogsTail => Spec::Number(NumSpec {
            min: 0.,
            max: 100_000.,
            integer: true,
            unit: "",
            get: |c| c.logs.initial_tail as f64,
            set: |c, v| c.logs.initial_tail = v as u32,
        }),
        Key::LogsMax => Spec::Number(NumSpec {
            min: 1_000.,
            max: 1_000_000.,
            integer: true,
            unit: "",
            get: |c| c.logs.max_lines as f64,
            set: |c, v| c.logs.max_lines = v as u32,
        }),
        Key::FontFamily => Spec::Text(TextSpec {
            placeholder: Some(s::SET_TERM_FONT_PLACEHOLDER),
            get: |c| c.terminal.font_family.clone(),
            set: |c, v| c.terminal.font_family = v,
            validate: no_check,
        }),
        Key::FontSize => Spec::Number(NumSpec {
            min: 6.,
            max: 48.,
            integer: false,
            unit: s::UNIT_PX,
            get: |c| c.terminal.font_size as f64,
            set: |c, v| c.terminal.font_size = v as f32,
        }),
        Key::Shell => Spec::Text(TextSpec {
            placeholder: Some(s::SET_TERM_SHELL_PLACEHOLDER),
            get: |c| c.terminal.default_shell.clone(),
            set: |c, v| c.terminal.default_shell = v,
            validate: no_check,
        }),
        Key::Scrollback => Spec::Number(NumSpec {
            min: 0.,
            max: 1_000_000.,
            integer: true,
            unit: "",
            get: |c| c.terminal.scrollback_lines as f64,
            set: |c, v| c.terminal.scrollback_lines = v as u32,
        }),
        Key::External => Spec::Text(TextSpec {
            placeholder: Some(s::SET_TERM_EXTERNAL_PLACEHOLDER),
            get: |c| c.terminal.external_terminal.clone(),
            set: |c, v| c.terminal.external_terminal = v,
            validate: check_external,
        }),
        Key::StatsHistory => Spec::Number(NumSpec {
            min: 1.,
            max: 60.,
            integer: true,
            unit: s::UNIT_MINUTES,
            get: |c| c.stats.history_minutes as f64,
            set: |c, v| c.stats.history_minutes = v as u32,
        }),
    }
}

/// Index of the current value of a choice setting.
pub fn choice_index(key: Key, c: &Config) -> usize {
    match key {
        Key::Theme => match c.general.theme {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        },
        Key::StartPage => match c.general.start_page {
            StartPage::Containers => 0,
            StartPage::Images => 1,
            StartPage::Volumes => 2,
        },
        Key::GroupBy => match c.containers.group_by {
            GroupBy::Compose => 0,
            GroupBy::None => 1,
            GroupBy::Label(_) => 2,
        },
        Key::LogLevel => match c.diagnostics.log_level {
            LogLevel::Info => 0,
            LogLevel::Debug => 1,
        },
        _ => 0,
    }
}

pub fn theme_at(ix: usize) -> ThemeMode {
    match ix {
        1 => ThemeMode::Light,
        2 => ThemeMode::Dark,
        _ => ThemeMode::System,
    }
}

/// Writes a choice setting. `GroupBy` with *Label* is handled by the page (it needs the key).
pub fn set_choice(key: Key, c: &mut Config, ix: usize) {
    match key {
        Key::Theme => c.general.theme = theme_at(ix),
        Key::StartPage => {
            c.general.start_page = match ix {
                1 => StartPage::Images,
                2 => StartPage::Volumes,
                _ => StartPage::Containers,
            }
        }
        Key::GroupBy => match ix {
            1 => c.containers.group_by = GroupBy::None,
            0 => c.containers.group_by = GroupBy::Compose,
            _ => {}
        },
        Key::LogLevel => {
            c.diagnostics.log_level = if ix == 1 {
                LogLevel::Debug
            } else {
                LogLevel::Info
            }
        }
        _ => {}
    }
}

/// Text shown for a number (`5`, `13.5`).
pub fn fmt_num(v: f64, integer: bool) -> String {
    if integer || v.fract() == 0. {
        format!("{}", v.round() as i64)
    } else {
        format!("{v}")
    }
}

/// Parses and range-checks a number input (inline validation, SET-020…050).
pub fn parse_num(text: &str, spec: &NumSpec) -> Result<f64, String> {
    let t = text.trim();
    let v = if spec.integer {
        t.parse::<i64>()
            .map(|v| v as f64)
            .map_err(|_| s::NOT_A_WHOLE_NUMBER.to_owned())?
    } else {
        t.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| s::NOT_A_NUMBER.to_owned())?
    };
    if v < spec.min || v > spec.max {
        return Err(s::out_of_range(spec.min, spec.max, spec.unit));
    }
    Ok(v)
}

/// Keys with a control on each section.
pub fn keys_for(section: crate::nav::SettingsSection) -> &'static [Key] {
    use crate::nav::SettingsSection as S;
    match section {
        S::General => &[Key::Theme, Key::StartPage],
        S::Containers => &[Key::GroupBy, Key::LabelKey, Key::Polling],
        S::Logs => &[Key::LogsTail, Key::LogsMax],
        S::Terminal => &[
            Key::FontFamily,
            Key::FontSize,
            Key::Shell,
            Key::Scrollback,
            Key::External,
        ],
        S::Stats => &[Key::StatsHistory],
        S::Diagnostics => &[Key::LogLevel],
        S::Engines | S::Keyboard => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(key: Key) -> NumSpec {
        match spec(key) {
            Spec::Number(n) => n,
            _ => panic!("not a number"),
        }
    }

    #[test]
    fn set_020_polling_interval_at_least_one_second() {
        let p = num(Key::Polling);
        assert_eq!(parse_num("5", &p), Ok(5.));
        assert!(parse_num("0", &p).is_err());
        assert_eq!(parse_num("1.5", &p), Err(s::NOT_A_WHOLE_NUMBER.to_owned()));
        assert_eq!(parse_num("abc", &p), Err(s::NOT_A_WHOLE_NUMBER.to_owned()));
    }

    #[test]
    fn set_050_history_window_1_to_60_minutes() {
        let h = num(Key::StatsHistory);
        assert!(parse_num("0", &h).is_err());
        assert_eq!(parse_num("60", &h), Ok(60.));
        assert_eq!(
            parse_num("61", &h),
            Err(s::out_of_range(1., 60., s::UNIT_MINUTES))
        );
    }

    #[test]
    fn set_040_font_size_accepts_decimals() {
        let f = num(Key::FontSize);
        assert_eq!(parse_num("13.5", &f), Ok(13.5));
        assert_eq!(fmt_num(13.5, false), "13.5");
        assert_eq!(fmt_num(13., false), "13");
        assert!(check_external("wt.exe").is_some());
        assert!(check_external("wt.exe {cmd}").is_none());
        assert!(check_external("").is_none());
    }

    #[test]
    fn set_choices_roundtrip() {
        for key in [Key::Theme, Key::StartPage, Key::LogLevel] {
            let Spec::Choice(opts) = spec(key) else {
                panic!()
            };
            for ix in 0..opts.len() {
                let mut c = Config::default();
                set_choice(key, &mut c, ix);
                assert_eq!(choice_index(key, &c), ix, "{key:?}");
            }
        }
        let mut c = Config::default();
        set_choice(Key::GroupBy, &mut c, 1);
        assert_eq!(c.containers.group_by, GroupBy::None);
    }

    #[test]
    fn bool_keys_roundtrip() {
        use BoolKey::*;
        for k in [
            ConfirmStopped,
            ShowNetworks,
            ShowStats,
            LogsTimestamps,
            LogsWrap,
            AllCores,
            ShowAllDistros,
            ShowAllSessions,
        ] {
            let mut c = Config::default();
            let v = !k.get(&c);
            k.set(&mut c, v);
            assert_eq!(k.get(&c), v, "{k:?}");
        }
    }
}
