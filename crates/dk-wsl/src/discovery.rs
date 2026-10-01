//! WSL distribution enumeration and non-booting bridge-tool probes (ENG-007/106).

use std::collections::BTreeSet;
use std::io;

#[cfg(windows)]
use crate::runner;

pub(crate) const PROBE_SCRIPT: &str =
    "test -S /var/run/docker.sock && (command -v docker || command -v socat)";
#[cfg(windows)]
const SOCKET_SCRIPT: &str = "test -S /var/run/docker.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Distro {
    pub(crate) name: String,
    pub(crate) running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTool {
    Docker,
    Socat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeClassification {
    Available(BridgeTool),
    NoDocker,
    SocketNoClient,
    PermissionDenied,
}

pub(crate) fn validate_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn excluded(name: &str) -> bool {
    name.eq_ignore_ascii_case("docker-desktop") || name.eq_ignore_ascii_case("docker-desktop-data")
}

pub(crate) fn classify_probe_output(
    success: bool,
    stdout: &str,
    stderr: &str,
    socket_exists: bool,
) -> ProbeClassification {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if combined.contains("permission denied") || combined.contains("access denied") {
        return ProbeClassification::PermissionDenied;
    }
    if success {
        let executable = stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        if executable
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("docker"))
        {
            ProbeClassification::Available(BridgeTool::Docker)
        } else if executable
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("socat"))
        {
            ProbeClassification::Available(BridgeTool::Socat)
        } else {
            ProbeClassification::SocketNoClient
        }
    } else if socket_exists {
        ProbeClassification::SocketNoClient
    } else {
        ProbeClassification::NoDocker
    }
}

/// Tolerant fallback parser. It does not depend on the localised header or state words: a data row
/// is identified by a final WSL version (`1` or `2`), and its first field is the validated name.
pub(crate) fn parse_verbose_list(text: &str) -> Vec<(String, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.trim().split_whitespace();
            let mut name = fields.next()?;
            if name == "*" {
                name = fields.next()?;
            } else if let Some(stripped) = name.strip_prefix('*') {
                if !stripped.is_empty() {
                    name = stripped;
                }
            }
            let version = line.split_whitespace().next_back()?.parse::<u32>().ok()?;
            (matches!(version, 1 | 2) && validate_distro_name(name))
                .then(|| (name.to_owned(), version))
        })
        .collect()
}

#[cfg(windows)]
pub(crate) async fn enumerate() -> io::Result<Vec<Distro>> {
    let entries = match crate::win32::enumerate_distros() {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| entry.version == 2)
            .map(|entry| entry.name)
            .collect::<Vec<_>>(),
        Err(registry_error) => {
            tracing::debug!(error = %registry_error, "WSL registry enumeration failed; using wsl.exe fallback");
            let output = runner::run(["--list", "--verbose"]).await?;
            if !output.success {
                return Err(io::Error::other(format!(
                    "wsl.exe --list --verbose failed: {}",
                    output.stderr_utf8().trim()
                )));
            }
            parse_verbose_list(&runner::decode_utf16le(&output.stdout))
                .into_iter()
                .filter_map(|(name, version)| (version == 2).then_some(name))
                .collect()
        }
    };

    let running = running_names().await?;
    let mut seen = BTreeSet::new();
    let mut result = entries
        .into_iter()
        .filter(|name| validate_distro_name(name) && !excluded(name))
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .map(|name| Distro {
            running: running.contains(&name.to_ascii_lowercase()),
            name,
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|distro| distro.name.to_ascii_lowercase());
    Ok(result)
}

#[cfg(not(windows))]
pub(crate) async fn enumerate() -> io::Result<Vec<Distro>> {
    Ok(Vec::new())
}

#[cfg(windows)]
pub(crate) async fn running_names() -> io::Result<BTreeSet<String>> {
    let output = runner::run(["--list", "--running", "--quiet"]).await?;
    if !output.success {
        return Err(io::Error::other(format!(
            "wsl.exe --list --running --quiet failed: {}",
            output.stderr_utf8().trim()
        )));
    }
    Ok(runner::decode_utf16le(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| validate_distro_name(name))
        .map(str::to_ascii_lowercase)
        .collect())
}

#[cfg(windows)]
pub(crate) async fn is_running(distro: &str) -> io::Result<bool> {
    Ok(running_names()
        .await?
        .contains(&distro.to_ascii_lowercase()))
}

#[cfg(not(windows))]
pub(crate) async fn is_running(_distro: &str) -> io::Result<bool> {
    Ok(false)
}

#[cfg(windows)]
pub(crate) async fn probe(distro: &str) -> io::Result<ProbeClassification> {
    if !validate_distro_name(distro) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid WSL distro name",
        ));
    }
    let output = runner::run(["-d", distro, "--exec", "sh", "-c", PROBE_SCRIPT]).await?;
    let stdout = output.stdout_utf8();
    let stderr = output.stderr_utf8();
    if output.success || stderr.to_ascii_lowercase().contains("permission denied") {
        return Ok(classify_probe_output(
            output.success,
            &stdout,
            &stderr,
            output.success,
        ));
    }

    // The normative probe intentionally has one failure exit for both "no socket" and "no tool".
    // This second fixed script distinguishes those states without interpolating user input.
    let socket = runner::run(["-d", distro, "--exec", "sh", "-c", SOCKET_SCRIPT]).await?;
    Ok(classify_probe_output(
        false,
        &stdout,
        &stderr,
        socket.success,
    ))
}

#[cfg(not(windows))]
pub(crate) async fn probe(_distro: &str) -> io::Result<ProbeClassification> {
    Ok(ProbeClassification::NoDocker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_007_parses_verbose_list_without_using_header_or_state_text() {
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
    fn eng_007_parses_attached_default_marker() {
        assert_eq!(
            parse_verbose_list("*Ubuntu_24.04 Running 2\n"),
            vec![("Ubuntu_24.04".into(), 2)]
        );
    }

    #[test]
    fn classifies_probe_outputs() {
        assert_eq!(
            classify_probe_output(true, "/usr/bin/docker\n", "", true),
            ProbeClassification::Available(BridgeTool::Docker)
        );
        assert_eq!(
            classify_probe_output(true, "/usr/bin/socat\n", "", true),
            ProbeClassification::Available(BridgeTool::Socat)
        );
        assert_eq!(
            classify_probe_output(false, "", "", false),
            ProbeClassification::NoDocker
        );
        assert_eq!(
            classify_probe_output(false, "", "", true),
            ProbeClassification::SocketNoClient
        );
        assert_eq!(
            classify_probe_output(false, "", "docker: permission denied", true),
            ProbeClassification::PermissionDenied
        );
    }

    #[test]
    fn nfr_022_validates_distro_names() {
        for name in ["Ubuntu-22.04", "openSUSE_Tumbleweed", "a", "9distro"] {
            assert!(validate_distro_name(name), "{name}");
        }
        for name in ["", "-d", "Ubuntu 22.04", "x;echo", "x/y", "Übuntu"] {
            assert!(!validate_distro_name(name), "{name}");
        }
    }
}
