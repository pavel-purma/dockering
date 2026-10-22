//! WSL distribution enumeration and non-booting Docker probes (spec 20 §4.2, ENG-007/106).

use std::collections::BTreeSet;
use std::io;

use crate::runner::{self, Wsl};

/// The spec 20 §4.2 probe (spike W-2.3, verified in 19 states), one `wsl.exe` spawn per distro:
///
/// | exit | meaning |
/// |---|---|
/// | 0  | socket owned by a live `dockerd` (or another engine), prints the bridge tool path, the user can open it |
/// | 11 | no `dockerd`/init script/unit and no socket (`no-docker`); also a bare `docker` client |
/// | 12 | socket, but neither `docker` nor `socat` (`socket-no-client`) |
/// | 13 | tool found, but the socket isn't readable/writable (not in the `docker` group) |
/// | 14 | Docker is installed but no live `dockerd` (and no socket activation): `installed-not-running` |
///
/// The script is a constant; the distro name is a separate, validated argv item (NFR-022).
pub(crate) const PROBE_SCRIPT: &str = include_str!("probe.sh");

const EXIT_NO_SOCKET: i32 = 11;
const EXIT_NO_CLIENT: i32 = 12;
const EXIT_PERMISSION: i32 = 13;
const EXIT_NOT_RUNNING: i32 = 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Distro {
    pub(crate) name: String,
    pub(crate) running: bool,
}

/// The in-distro program that `PipeBridge` runs per connection (spec 20 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTool {
    /// `docker system dial-stdio` (preferred).
    Docker,
    /// `socat - UNIX-CONNECT:/var/run/docker.sock` (fallback).
    Socat,
}

impl BridgeTool {
    /// The `--exec` argv that connects stdio to the Docker socket.
    pub(crate) fn dial_argv(self) -> &'static [&'static str] {
        match self {
            Self::Docker => &["docker", "system", "dial-stdio"],
            Self::Socat => &["socat", "-", "UNIX-CONNECT:/var/run/docker.sock"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeClassification {
    /// `available`
    Available(BridgeTool),
    /// `no-docker`: hidden unless "Show all WSL distros".
    NoDocker,
    /// `socket-no-client`: offer TCP mode or `apt install socat`.
    SocketNoClient,
    /// The user isn't in the in-distro `docker` group.
    PermissionDenied,
    /// `installed-not-running` (ENG-148): listed as stopped, startable.
    InstalledNotRunning,
}

/// WSL distro names as accepted on an argv: `[A-Za-z0-9._-]+`, not starting with `-` (NFR-022).
pub(crate) fn validate_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Docker Desktop's own distros are covered by the `docker_engine` pipe (ENG-006).
pub(crate) fn excluded(name: &str) -> bool {
    name.eq_ignore_ascii_case("docker-desktop") || name.eq_ignore_ascii_case("docker-desktop-data")
}

fn tool_from_path(line: &str) -> Option<BridgeTool> {
    match line.trim().rsplit('/').next()? {
        "docker" => Some(BridgeTool::Docker),
        "socat" => Some(BridgeTool::Socat),
        _ => None,
    }
}

/// Classifies the result of [`PROBE_SCRIPT`]. `Err` carries a message for an unexpected result
/// (for example, `wsl.exe` failed before the script ran).
pub(crate) fn classify_probe(
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Result<ProbeClassification, String> {
    match code {
        Some(0) => stdout
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .and_then(tool_from_path)
            .map(ProbeClassification::Available)
            .ok_or_else(|| format!("unexpected probe output: {}", stdout.trim())),
        Some(EXIT_NO_SOCKET) => Ok(ProbeClassification::NoDocker),
        Some(EXIT_NO_CLIENT) => Ok(ProbeClassification::SocketNoClient),
        Some(EXIT_PERMISSION) => Ok(ProbeClassification::PermissionDenied),
        Some(EXIT_NOT_RUNNING) => Ok(ProbeClassification::InstalledNotRunning),
        _ => {
            let text = format!("{stdout}\n{stderr}");
            if text.to_ascii_lowercase().contains("permission denied") {
                Ok(ProbeClassification::PermissionDenied)
            } else {
                let message = text.split_whitespace().collect::<Vec<_>>().join(" ");
                Err(if message.is_empty() {
                    format!("probe exited with {code:?}")
                } else {
                    message
                })
            }
        }
    }
}

/// Tolerant `wsl.exe --list --verbose` parser (fallback when the registry is unavailable). It
/// doesn't depend on the localised header or state words: a data row ends in the WSL version
/// (`1` or `2`) and its first field, after an optional `*` default marker, is the name.
pub(crate) fn parse_verbose_list(text: &str) -> Vec<(String, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let mut name = fields.next()?;
            if name == "*" {
                name = fields.next()?;
            } else if let Some(stripped) = name.strip_prefix('*') {
                name = stripped;
            }
            let version = line.split_whitespace().next_back()?.parse::<u32>().ok()?;
            (matches!(version, 1 | 2) && validate_distro_name(name))
                .then(|| (name.to_owned(), version))
        })
        .collect()
}

/// Parses `wsl.exe --list --running --quiet` (already decoded) into lowercase names.
pub(crate) fn parse_running_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|name| validate_distro_name(name))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Version-2, non-Docker-Desktop, valid names; de-duplicated case-insensitively and sorted.
pub(crate) fn select_distros(names: Vec<String>, running: &BTreeSet<String>) -> Vec<Distro> {
    let mut seen = BTreeSet::new();
    let mut result = names
        .into_iter()
        .filter(|name| validate_distro_name(name) && !excluded(name))
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .map(|name| Distro {
            running: running.contains(&name.to_ascii_lowercase()),
            name,
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|distro| distro.name.to_ascii_lowercase());
    result
}

/// Probes a **running** distro. Callers check [`is_running`] first: `wsl.exe -d` boots a
/// stopped distro (ENG-106).
pub(crate) async fn probe(wsl: &dyn Wsl, distro: &str) -> io::Result<ProbeClassification> {
    if !validate_distro_name(distro) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid WSL distro name",
        ));
    }
    let output = wsl
        .run(&["-d", distro, "--exec", "sh", "-c", PROBE_SCRIPT])
        .await?;
    classify_probe(
        output.code,
        &output.stdout_utf8(),
        &runner::decode_message(&output.stderr),
    )
    .map_err(|message| {
        // `wsl.exe` prints its own errors (WSL_E_DISTRO_NOT_FOUND, …) as UTF-16LE on stdout.
        let wsl = output.error_message();
        io::Error::other(if wsl.is_empty() { message } else { wsl })
    })
}

pub(crate) async fn running_names(wsl: &dyn Wsl) -> io::Result<BTreeSet<String>> {
    let output = wsl.run(&["--list", "--running", "--quiet"]).await?;
    let text = runner::decode_utf16le(&output.stdout);
    if !output.success() {
        // `wsl.exe --list --running` exits non-zero when nothing is running ("There are no
        // running distributions."); that's an empty set, not an error.
        if parse_running_list(&text).is_empty() {
            return Ok(BTreeSet::new());
        }
        return Err(io::Error::other(format!(
            "wsl.exe --list --running --quiet failed: {}",
            output.error_message()
        )));
    }
    Ok(parse_running_list(&text))
}

pub(crate) async fn is_running(wsl: &dyn Wsl, distro: &str) -> io::Result<bool> {
    Ok(running_names(wsl)
        .await?
        .contains(&distro.to_ascii_lowercase()))
}

#[cfg(windows)]
mod platform {
    use std::collections::BTreeSet;
    use std::io;

    use super::Distro;
    use crate::runner::{self, RealWsl, Wsl};

    /// Registry `State` value of a fully installed distro (others: installing, uninstalling…).
    const STATE_INSTALLED: u32 = 1;

    async fn registered_names() -> io::Result<Vec<String>> {
        match crate::win32::enumerate_distros() {
            Ok(entries) => Ok(entries
                .into_iter()
                .filter(|entry| entry.version == 2)
                .filter(|entry| entry.state.is_none_or(|state| state == STATE_INSTALLED))
                .map(|entry| entry.name)
                .collect()),
            Err(registry_error) => {
                tracing::debug!(error = %registry_error, "WSL registry enumeration failed; using wsl.exe --list --verbose");
                let output = RealWsl.run(&["--list", "--verbose"]).await?;
                if !output.success() {
                    return Err(io::Error::other(format!(
                        "wsl.exe --list --verbose failed: {}",
                        output.error_message()
                    )));
                }
                Ok(
                    super::parse_verbose_list(&runner::decode_utf16le(&output.stdout))
                        .into_iter()
                        .filter_map(|(name, version)| (version == 2).then_some(name))
                        .collect(),
                )
            }
        }
    }

    pub(crate) async fn enumerate() -> io::Result<Vec<Distro>> {
        let names = registered_names().await?;
        if names.is_empty() {
            return Ok(Vec::new());
        }
        // Without a running list, report every distro as stopped: never probe (= boot) blindly.
        let running = super::running_names(&RealWsl)
            .await
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "wsl.exe --list --running failed; treating distros as stopped");
                BTreeSet::new()
            });
        Ok(super::select_distros(names, &running))
    }
}

#[cfg(windows)]
pub(crate) use platform::enumerate;

#[cfg(not(windows))]
pub(crate) async fn enumerate() -> io::Result<Vec<Distro>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::decode_utf16le;

    #[test]
    fn eng_007_parses_recorded_verbose_list_with_header_and_default_marker() {
        let text = decode_utf16le(include_bytes!("../fixtures/list-verbose.en.utf16le.bin"));
        assert_eq!(
            parse_verbose_list(&text),
            vec![("Ubuntu-22.04".into(), 2), ("docker-desktop".into(), 2)]
        );
    }

    #[test]
    fn eng_007_parses_verbose_list_without_using_localised_header_or_state() {
        let input = "  NAME                   ÉTAT             VERSION\n* Ubuntu-22.04           En cours         2\n  legacy                 Arrêté           1\n  docker-desktop         Running          2\n";
        assert_eq!(
            parse_verbose_list(input),
            vec![
                ("Ubuntu-22.04".into(), 2),
                ("legacy".into(), 1),
                ("docker-desktop".into(), 2)
            ]
        );
    }

    #[test]
    fn eng_007_parses_attached_default_marker_and_skips_garbage() {
        assert_eq!(
            parse_verbose_list("*Ubuntu_24.04 Running 2\nWindows Subsystem for Linux 3\n\n*\n"),
            vec![("Ubuntu_24.04".into(), 2)]
        );
    }

    #[test]
    fn eng_007_running_list_is_case_insensitive() {
        let text = decode_utf16le(include_bytes!("../fixtures/list-running-quiet.utf16le.bin"));
        let running = parse_running_list(&text);
        assert!(running.contains("ubuntu-22.04"));
        assert!(running.contains("docker-desktop"));
        assert_eq!(running.len(), 2);
    }

    #[test]
    fn eng_007_selection_skips_docker_desktop_and_dedupes() {
        let running = parse_running_list("Ubuntu-22.04\n");
        let distros = select_distros(
            vec![
                "docker-desktop".into(),
                "Ubuntu-22.04".into(),
                "Debian".into(),
                "docker-desktop-data".into(),
                "ubuntu-22.04".into(),
                "bad name".into(),
            ],
            &running,
        );
        assert_eq!(
            distros,
            vec![
                Distro {
                    name: "Debian".into(),
                    running: false
                },
                Distro {
                    name: "Ubuntu-22.04".into(),
                    running: true
                },
            ]
        );
    }

    #[test]
    fn eng_007_classifies_probe_results() {
        let recorded =
            String::from_utf8_lossy(include_bytes!("../fixtures/probe-ubuntu-docker.stdout.bin"))
                .into_owned();
        assert_eq!(
            classify_probe(Some(0), &recorded, ""),
            Ok(ProbeClassification::Available(BridgeTool::Docker))
        );
        assert_eq!(
            classify_probe(Some(0), "/usr/bin/socat\n", ""),
            Ok(ProbeClassification::Available(BridgeTool::Socat))
        );
        assert_eq!(
            classify_probe(Some(11), "", ""),
            Ok(ProbeClassification::NoDocker)
        );
        assert_eq!(
            classify_probe(Some(12), "", ""),
            Ok(ProbeClassification::SocketNoClient)
        );
        assert_eq!(
            classify_probe(Some(13), "/usr/bin/docker\n", ""),
            Ok(ProbeClassification::PermissionDenied)
        );
        assert_eq!(
            classify_probe(Some(1), "", "sh: 1: permission denied"),
            Ok(ProbeClassification::PermissionDenied)
        );
        assert!(classify_probe(Some(0), "/usr/bin/dockerd\n", "").is_err());
        let error = classify_probe(Some(127), "", "There is no distribution\n").unwrap_err();
        assert_eq!(error, "There is no distribution");
        assert!(classify_probe(None, "", "").is_err());
    }

    #[test]
    fn eng_007_probe_script_contains_the_spec_probe_semantics() {
        assert!(PROBE_SCRIPT.contains("test -S $d"));
        assert!(PROBE_SCRIPT.contains("command -v docker || command -v socat"));
    }

    #[test]
    fn eng_148_probe_script_classifies_spike_states() {
        // The script is the spike W-2.3 text, verified in 19 states, with only the `read`
        // hardening added; the exits are the contract `classify_probe` relies on.
        for exit in ["exit 11", "exit 12", "exit 13", "exit 14"] {
            assert!(PROBE_SCRIPT.contains(exit), "{exit}");
        }
        assert!(PROBE_SCRIPT.contains("{ read -r n < \"$c\"; } 2>/dev/null"));
        assert!(PROBE_SCRIPT.contains("systemctl is-active --quiet docker.socket"));
        assert!(PROBE_SCRIPT.contains("/etc/init.d/docker"));
        assert!(PROBE_SCRIPT.contains("/lib/systemd/system/docker.service"));
        assert!(!PROBE_SCRIPT.contains("pgrep"));

        let docker = Ok(ProbeClassification::Available(BridgeTool::Docker));
        assert_eq!(
            classify_probe(
                Some(0),
                "/usr/bin/docker
",
                ""
            ),
            docker
        );
        assert_eq!(
            classify_probe(Some(11), "", ""),
            Ok(ProbeClassification::NoDocker)
        );
        assert_eq!(
            classify_probe(Some(12), "", ""),
            Ok(ProbeClassification::SocketNoClient)
        );
        assert_eq!(
            classify_probe(
                Some(13),
                "/usr/bin/docker
",
                ""
            ),
            Ok(ProbeClassification::PermissionDenied)
        );
        assert_eq!(
            classify_probe(Some(14), "", ""),
            Ok(ProbeClassification::InstalledNotRunning)
        );
        // stdout is ignored for 11-14, and a stray shell error doesn't change an exit code.
        assert_eq!(
            classify_probe(Some(14), "x", "sh: 1: cannot open /proc/1/comm"),
            Ok(ProbeClassification::InstalledNotRunning)
        );
        assert!(classify_probe(Some(15), "", "").is_err());
    }

    #[test]
    fn nfr_022_validates_distro_names() {
        for name in ["Ubuntu-22.04", "openSUSE_Tumbleweed", "a", "9distro", "x.y"] {
            assert!(validate_distro_name(name), "{name}");
        }
        for name in [
            "",
            "-d",
            "--exec",
            "Ubuntu 22.04",
            "x;echo",
            "x/y",
            "x\\y",
            "Übuntu",
            "a\"b",
        ] {
            assert!(!validate_distro_name(name), "{name}");
        }
        assert!(!validate_distro_name(&"a".repeat(256)));
    }
}
