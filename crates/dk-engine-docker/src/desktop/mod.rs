//! Docker Desktop on Windows: install detection, start and status (ENG-142…147, ENG-160).
//!
//! The registry read and the Windows process flags are `cfg(windows)`; elsewhere no install is
//! ever found, so there is no prompt, no stopped listing and no start.

#[cfg(windows)]
mod registry;
mod run;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use dk_core::{EngineConfig, EngineEndpoint, StartPrompt};
use serde_json::Value;

use crate::discovery::normalize_pipe_path;

pub(crate) const ENGINE_PIPE: &str = r"\\.\pipe\docker_engine";
pub(crate) const LINUX_PIPE: &str = r"\\.\pipe\dockerDesktopLinuxEngine";

const DESKTOP_EXE: &str = "Docker Desktop.exe";
const START_ARGV: [&str; 4] = ["desktop", "start", "--timeout", "120"];
const STATUS_ARGV: [&str; 4] = ["desktop", "status", "--format", "json"];

pub(crate) const HINT: &str =
    "Finish any prompt in the Docker Desktop window (licence, sign-in or update), then try again.";

/// A Docker Desktop install folder whose `Docker Desktop.exe` exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopInstall {
    root: PathBuf,
}

impl DesktopInstall {
    /// Pure: an absolute `InstallLocation` value (a trailing separator is dropped).
    pub(crate) fn from_location(location: &str) -> Option<Self> {
        let trimmed = location.trim().trim_end_matches(['\\', '/']);
        if trimmed.is_empty() || trimmed.contains('\0') {
            return None;
        }
        let root = PathBuf::from(trimmed);
        (root.is_absolute() && root.has_root()).then_some(Self { root })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn desktop_exe(&self) -> PathBuf {
        self.root.join(DESKTOP_EXE)
    }

    /// Never a `PATH` lookup (ENG-160).
    pub(crate) fn docker_exe(&self) -> PathBuf {
        self.root.join("resources").join("bin").join("docker.exe")
    }
}

/// Outcome of `docker desktop status --format json` (ENG-145).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopStatus {
    Running,
    /// Paused or Resource Saver: Docker Desktop wakes itself, so no start is offered.
    Paused,
    NotRunning,
}

/// Tolerant parse: anything but `{"Status":"running", ...}` is not running (only `running`
/// was observed). A non-empty `PauseType` or a paused-looking `Status` is `Paused`.
pub(crate) fn parse_status(success: bool, stdout: &str) -> DesktopStatus {
    if !success {
        return DesktopStatus::NotRunning;
    }
    let (Some(start), Some(end)) = (stdout.find('{'), stdout.rfind('}')) else {
        return DesktopStatus::NotRunning;
    };
    let Some(Value::Object(map)) = stdout
        .get(start..=end)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    else {
        return DesktopStatus::NotRunning;
    };
    let text = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default()
    };
    let (status, pause) = (text("Status"), text("PauseType"));
    if !pause.is_empty() || status.contains("paus") || status.contains("saver") {
        DesktopStatus::Paused
    } else if status == "running" {
        DesktopStatus::Running
    } else {
        DesktopStatus::NotRunning
    }
}

pub(crate) fn start_prompt() -> StartPrompt {
    StartPrompt {
        title: "Docker Desktop isn't running".into(),
        body: "Start it to see your containers and images.".into(),
        action_label: "Start Docker Desktop".into(),
    }
}

#[derive(Debug, Clone)]
enum Locations {
    Registry,
    #[cfg(feature = "test-support")]
    Fixed(Vec<String>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// Outer bound of `docker desktop start --timeout 120`.
    pub start: Duration,
    /// Fallback: how long to poll the engine pipe after launching `Docker Desktop.exe`.
    pub poll: Duration,
    pub poll_interval: Duration,
    pub status: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            start: Duration::from_secs(125),
            poll: Duration::from_secs(120),
            poll_interval: Duration::from_secs(1),
            status: Duration::from_secs(5),
        }
    }
}

/// Per-factory Docker Desktop state: where it is installed (cached) and which pipes it owns.
pub(crate) struct Desktop {
    locations: Locations,
    /// `None` = not looked up yet, `Some(None)` = looked up, not installed.
    install: Mutex<Option<Option<DesktopInstall>>>,
    pub(crate) engine_pipe: String,
    pub(crate) linux_pipe: String,
    pub(crate) timeouts: Timeouts,
    /// Tests: discovery lists only the Docker Desktop pipes.
    pub(crate) pipes_only: bool,
}

impl Desktop {
    pub(crate) fn new() -> Self {
        Self {
            locations: Locations::Registry,
            install: Mutex::new(None),
            engine_pipe: ENGINE_PIPE.into(),
            linux_pipe: LINUX_PIPE.into(),
            timeouts: Timeouts::default(),
            pipes_only: false,
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn for_tests(t: crate::DesktopTestConfig) -> Self {
        Self {
            locations: Locations::Fixed(vec![t.install_location.to_string_lossy().into_owned()]),
            install: Mutex::new(None),
            engine_pipe: t.engine_pipe,
            linux_pipe: t.linux_pipe,
            timeouts: Timeouts {
                start: t.start_timeout,
                poll: t.poll_timeout,
                poll_interval: t.poll_interval,
                status: t.status_timeout,
            },
            pipes_only: true,
        }
    }

    fn slot(&self) -> MutexGuard<'_, Option<Option<DesktopInstall>>> {
        self.install.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Sync and never blocks: `Some` only once an install was found.
    pub(crate) fn installed(&self) -> Option<DesktopInstall> {
        self.slot().clone().flatten()
    }

    /// Looks the install up again (a `Rescan` can find a newly installed Desktop).
    pub(crate) async fn refresh(&self) -> Option<DesktopInstall> {
        let found = self.detect().await;
        *self.slot() = Some(found.clone());
        found
    }

    /// The cached result, looked up once if still unknown.
    pub(crate) async fn load(&self) -> Option<DesktopInstall> {
        let known = self.slot().clone();
        match known {
            Some(found) => found,
            None => self.refresh().await,
        }
    }

    /// Registry (`HKLM`, then `HKCU`) and a file check; spawns no process (ENG-143).
    async fn detect(&self) -> Option<DesktopInstall> {
        let locations = match &self.locations {
            Locations::Registry => read_registry().await,
            #[cfg(feature = "test-support")]
            Locations::Fixed(v) => v.clone(),
        };
        for location in locations {
            let Some(install) = DesktopInstall::from_location(&location) else {
                continue;
            };
            if tokio::fs::try_exists(install.desktop_exe())
                .await
                .unwrap_or(false)
            {
                return Some(install);
            }
        }
        None
    }

    /// Is `endpoint` one of the two Docker Desktop engine pipes?
    pub(crate) fn is_desktop_endpoint(&self, endpoint: &EngineEndpoint) -> bool {
        let EngineEndpoint::NamedPipe { path } = endpoint else {
            return false;
        };
        let path = normalize_pipe_path(path);
        [&self.engine_pipe, &self.linux_pipe]
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&path))
    }

    pub(crate) fn prompt_for(&self, cfg: &EngineConfig) -> Option<StartPrompt> {
        (self.is_desktop_endpoint(&cfg.endpoint) && self.installed().is_some()).then(start_prompt)
    }
}

#[cfg(windows)]
async fn read_registry() -> Vec<String> {
    tokio::task::spawn_blocking(registry::install_locations)
        .await
        .unwrap_or_default()
}

#[cfg(not(windows))]
async fn read_registry() -> Vec<String> {
    Vec::new()
}

/// First useful line of a child's output: lossy UTF-8, control characters removed, bounded.
pub(crate) fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| l.chars().filter(|c| !c.is_control()).collect::<String>())
        .map(|l| l.trim().chars().take(200).collect::<String>())
        .find(|l| !l.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_145_status_json_parsing() {
        let running = r#"{"PauseType":"","SessionID":"x","Status":"running"}"#;
        assert_eq!(parse_status(true, running), DesktopStatus::Running);
        assert_eq!(
            parse_status(true, &format!("  \n{running}\n")),
            DesktopStatus::Running
        );
        assert_eq!(
            parse_status(true, r#"{"Status":"RUNNING"}"#),
            DesktopStatus::Running
        );
        for bad in [
            "",
            "garbage",
            "[]",
            "{",
            r#"{"Status":"stopped"}"#,
            r#"{"Status":"starting"}"#,
            r#"{"Status":7}"#,
            r#"{"Other":"running"}"#,
        ] {
            assert_eq!(parse_status(true, bad), DesktopStatus::NotRunning, "{bad}");
        }
        assert_eq!(parse_status(false, running), DesktopStatus::NotRunning);
        assert_eq!(
            parse_status(true, r#"{"Status":"running","PauseType":"manual"}"#),
            DesktopStatus::Paused
        );
        assert_eq!(
            parse_status(true, r#"{"Status":"paused"}"#),
            DesktopStatus::Paused
        );
    }

    #[test]
    fn eng_145_resource_saver_is_not_stopped() {
        let s = r#"{"PauseType":"resource-saver","Status":"paused"}"#;
        assert_eq!(parse_status(true, s), DesktopStatus::Paused);
    }

    #[cfg(windows)]
    #[test]
    fn eng_160_argv_only_and_registry_paths() {
        let install = DesktopInstall::from_location(r"C:\Program Files\Docker\Docker\").unwrap();
        assert_eq!(
            install.docker_exe(),
            Path::new(r"C:\Program Files\Docker\Docker")
                .join("resources")
                .join("bin")
                .join("docker.exe")
        );
        assert_eq!(
            install.desktop_exe().file_name().unwrap(),
            "Docker Desktop.exe"
        );
        assert_eq!(START_ARGV, ["desktop", "start", "--timeout", "120"]);
        assert_eq!(STATUS_ARGV, ["desktop", "status", "--format", "json"]);
        // Anything that could resolve through `PATH` or the working directory is refused.
        for bad in [
            "",
            "   ",
            "docker",
            r"..\Docker",
            r"Docker\Docker",
            "C:\\x\0y",
        ] {
            assert!(DesktopInstall::from_location(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn first_line_is_lossy_and_clean() {
        assert_eq!(
            first_line(b"\n  \nfailed \x1b[31mbad\x07\nnext"),
            Some("failed [31mbad".into())
        );
        assert_eq!(first_line(b"caf\xff"), Some("caf\u{fffd}".into()));
        assert_eq!(first_line(b" \r\n"), None);
        assert_eq!(
            first_line("é".repeat(500).as_bytes())
                .unwrap()
                .chars()
                .count(),
            200
        );
    }

    #[test]
    fn eng_143_prompt_wording_has_no_process_words() {
        let p = start_prompt();
        assert_eq!(p.title, "Docker Desktop isn't running");
        assert_eq!(p.body, "Start it to see your containers and images.");
        assert_eq!(p.action_label, "Start Docker Desktop");
    }

    #[test]
    fn desktop_endpoint_matches_both_pipes_only() {
        let d = Desktop::new();
        let pipe = |p: &str| EngineEndpoint::NamedPipe { path: p.into() };
        assert!(d.is_desktop_endpoint(&pipe(r"\\.\pipe\docker_engine")));
        assert!(d.is_desktop_endpoint(&pipe("npipe:////./pipe/dockerDesktopLinuxEngine")));
        assert!(d.is_desktop_endpoint(&pipe(r"\\.\PIPE\DOCKER_ENGINE")));
        assert!(!d.is_desktop_endpoint(&pipe(r"\\.\pipe\other")));
        assert!(!d.is_desktop_endpoint(&EngineEndpoint::UnixSocket {
            path: "/var/run/docker.sock".into()
        }));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads the real registry (read-only)"]
    fn live_registry_read_is_read_only() {
        let found = registry::install_locations();
        eprintln!("install locations: {found:?}");
        for l in found {
            assert!(DesktopInstall::from_location(&l).is_some(), "{l}");
        }
    }
}
