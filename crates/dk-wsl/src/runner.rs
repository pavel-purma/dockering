//! Private `wsl.exe` process runner and output decoders (spec 20 §4.2).
//!
//! Every invocation gets an argv vector (NFR-022), `CREATE_NO_WINDOW` (no console is created or
//! inherited), a null stdin, and a 10 s timeout. The child is killed when the timeout fires,
//! because `kill_on_drop` is set and the output future is dropped.
//!
//! Encodings: `wsl.exe --list …` and `wsl.exe`'s own error messages are UTF-16LE. Output of a
//! Linux program run through `--exec` is passed through unchanged (UTF-8).

use std::io;
use std::time::Duration;

use async_trait::async_trait;

use crate::discovery::BridgeTool;

#[cfg(windows)]
pub(crate) use platform::command;

/// The `wsl.exe` seam: production runs the real binary, tests inject a fake so no unit test
/// touches a distro.
#[async_trait]
pub(crate) trait Wsl: std::fmt::Debug + Send + Sync + 'static {
    /// False where WSL can't exist (non-Windows); callers then fail fast.
    fn supported(&self) -> bool {
        true
    }
    /// Runs `wsl.exe <args>` to completion (argv vector, no shell, timeout).
    async fn run(&self, args: &[&str]) -> io::Result<WslOutput>;
    /// [`Wsl::run`] with a caller-chosen timeout (an import takes seconds to minutes).
    async fn run_timeout(&self, args: &[&str], timeout: Duration) -> io::Result<WslOutput>;
    /// Every registered distro name, whatever its WSL version or state.
    async fn registered_distros(&self) -> io::Result<Vec<String>>;
    /// The installed WSL package version (spec 20 §5.2, no COM); `None` when it isn't there.
    async fn wsl_version(&self) -> Option<WslVersion>;
    /// Raw HTTP reply to `GET /_ping` sent through `tool` inside `distro`, the path the bridge
    /// uses (ENG-149). Empty when Docker doesn't answer.
    async fn ping(&self, distro: &str, tool: BridgeTool) -> io::Result<String>;
}

/// `(major, minor, patch, build)` of the WSL package.
pub(crate) type WslVersion = (u32, u32, u32, u32);

/// The real `wsl.exe` (Windows only; elsewhere every call is `Unsupported`).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RealWsl;

#[cfg(windows)]
#[async_trait]
impl Wsl for RealWsl {
    async fn run(&self, args: &[&str]) -> io::Result<WslOutput> {
        platform::run(args).await
    }

    async fn run_timeout(&self, args: &[&str], timeout: Duration) -> io::Result<WslOutput> {
        platform::run_timeout(args, timeout).await
    }

    async fn registered_distros(&self) -> io::Result<Vec<String>> {
        platform::registered_distros().await
    }

    async fn wsl_version(&self) -> Option<WslVersion> {
        tokio::task::spawn_blocking(crate::win32::wsl_service_version)
            .await
            .ok()
            .flatten()
    }

    async fn ping(&self, distro: &str, tool: BridgeTool) -> io::Result<String> {
        platform::ping(distro, tool).await
    }
}

#[cfg(not(windows))]
#[async_trait]
impl Wsl for RealWsl {
    fn supported(&self) -> bool {
        false
    }

    async fn run(&self, _args: &[&str]) -> io::Result<WslOutput> {
        Err(not_windows())
    }

    async fn run_timeout(&self, _args: &[&str], _timeout: Duration) -> io::Result<WslOutput> {
        Err(not_windows())
    }

    async fn registered_distros(&self) -> io::Result<Vec<String>> {
        Err(not_windows())
    }

    async fn wsl_version(&self) -> Option<WslVersion> {
        None
    }

    async fn ping(&self, _distro: &str, _tool: BridgeTool) -> io::Result<String> {
        Err(not_windows())
    }
}

#[cfg(not(windows))]
fn not_windows() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "WSL is only available on Windows",
    )
}

/// Whether `reply` starts with a `200` status line (Docker's answer to `GET /_ping`).
pub(crate) fn ping_ok(reply: &str) -> bool {
    reply
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("HTTP/1."))
        .is_some_and(|rest| rest.split_whitespace().nth(1) == Some("200"))
}

/// `wsl.exe --list` emits UTF-16LE, with or without a BOM. An odd trailing byte is ignored.
pub(crate) fn decode_utf16le(bytes: &[u8]) -> String {
    let mut units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    if units.first() == Some(&0xfeff) {
        units.remove(0);
    }
    String::from_utf16_lossy(&units).replace(['\0', '\r'], "")
}

/// Decodes `--exec` output (UTF-8, passed through from the Linux process).
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

/// Decodes a message whose encoding is unknown: `wsl.exe` writes its own errors (for example
/// `WSL_E_DISTRO_NOT_FOUND`) as UTF-16LE, while the Linux side writes UTF-8.
pub(crate) fn decode_message(bytes: &[u8]) -> String {
    if looks_utf16le(bytes) {
        decode_utf16le(bytes)
    } else {
        decode_utf8(bytes)
    }
}

/// A BOM, or NUL in at least half of the odd byte positions (ASCII text as UTF-16LE).
fn looks_utf16le(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xff, 0xfe]) {
        return true;
    }
    if bytes.len() < 2 {
        return false;
    }
    let odd = bytes.len() / 2;
    let nul_odd = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    nul_odd * 2 >= odd
}

#[derive(Debug)]
pub(crate) struct WslOutput {
    pub(crate) code: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl WslOutput {
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout of an `--exec` child.
    pub(crate) fn stdout_utf8(&self) -> String {
        decode_utf8(&self.stdout)
    }

    /// A single-line, human-readable failure message from stderr, else stdout.
    pub(crate) fn error_message(&self) -> String {
        let stderr = decode_message(&self.stderr);
        let text = if stderr.trim().is_empty() {
            decode_message(&self.stdout)
        } else {
            stderr
        };
        let text = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            format!("wsl.exe exited with {:?}", self.code)
        } else {
            text
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;
    use tokio::time::timeout;

    use super::WslOutput;
    use crate::discovery::BridgeTool;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    pub(crate) const WSL_TIMEOUT: Duration = Duration::from_secs(10);
    const PING_TIMEOUT: Duration = Duration::from_secs(5);
    const PING_REQUEST: &[u8] = b"GET /_ping HTTP/1.1
Host: docker
Connection: close

";

    /// A `wsl.exe` command with no window, null stdin, and kill-on-drop. Callers add argv items
    /// with `.arg`/`.args` only — never a shell string built from user input (NFR-022).
    pub(crate) fn command() -> Command {
        let mut command = Command::new("wsl.exe");
        command
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Runs `wsl.exe <args>` to completion with the 10 s timeout.
    pub(crate) async fn run(args: &[&str]) -> io::Result<WslOutput> {
        run_timeout(args, WSL_TIMEOUT).await
    }

    pub(crate) async fn run_timeout(args: &[&str], limit: Duration) -> io::Result<WslOutput> {
        let mut command = command();
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Dropping the `output()` future on timeout drops the child, which kills it (kill_on_drop).
        let output = timeout(limit, command.output()).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("wsl.exe timed out after {} s", limit.as_secs()),
            )
        })??;

        Ok(WslOutput {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    /// Registry first; `wsl.exe --list --quiet` (every distro, UTF-16LE) when it isn't readable.
    pub(crate) async fn registered_distros() -> io::Result<Vec<String>> {
        let registry = tokio::task::spawn_blocking(crate::win32::enumerate_distros)
            .await
            .map_err(io::Error::other)?;
        match registry {
            Ok(entries) => Ok(entries.into_iter().map(|entry| entry.name).collect()),
            Err(error) => {
                tracing::debug!(%error, "WSL registry enumeration failed; using wsl.exe --list --quiet");
                let output = run(&["--list", "--quiet"]).await?;
                let text = super::decode_utf16le(&output.stdout);
                Ok(text
                    .lines()
                    .map(str::trim)
                    .filter(|name| crate::discovery::validate_distro_name(name))
                    .map(str::to_owned)
                    .collect())
            }
        }
    }

    /// One `GET /_ping` through `docker system dial-stdio` (or socat) in `distro`.
    pub(crate) async fn ping(distro: &str, tool: BridgeTool) -> io::Result<String> {
        let mut command = command();
        command
            .args(["-d", distro, "--exec"])
            .args(tool.dial_argv())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(io::Error::other("wsl.exe child stdio unavailable"));
        };
        // The status line is enough; the child is killed on drop.
        let exchange = async {
            stdin.write_all(PING_REQUEST).await?;
            let (mut reply, mut chunk) = (Vec::new(), [0u8; 512]);
            while !reply.contains(&b'\n') && reply.len() < 4096 {
                let read = stdout.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                reply.extend_from_slice(&chunk[..read]);
            }
            Ok::<_, io::Error>(reply)
        };
        let reply = timeout(PING_TIMEOUT, exchange)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Docker ping timed out"))??;
        Ok(super::decode_utf8(&reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = if bom { vec![0xff, 0xfe] } else { Vec::new() };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn eng_007_decodes_utf16le_list_output_with_or_without_bom() {
        for bom in [false, true] {
            let mut bytes = utf16le("Ubuntu-22.04\r\n", bom);
            bytes.extend_from_slice(&[0, 0]);
            assert_eq!(decode_utf16le(&bytes), "Ubuntu-22.04\n");
        }
    }

    #[test]
    fn eng_007_ignores_an_incomplete_final_code_unit() {
        let mut bytes = utf16le("Ubuntu", false);
        bytes.push(0xaa);
        assert_eq!(decode_utf16le(&bytes), "Ubuntu");
    }

    #[test]
    fn eng_007_decodes_recorded_running_list_fixture() {
        let bytes = include_bytes!("../fixtures/list-running-quiet.utf16le.bin");
        assert_eq!(decode_utf16le(bytes), "Ubuntu-22.04\ndocker-desktop\n");
    }

    #[test]
    fn eng_007_exec_output_is_utf8() {
        let bytes = include_bytes!("../fixtures/probe-ubuntu-docker.stdout.bin");
        assert_eq!(decode_utf8(bytes), "/usr/bin/docker\n");
        assert_eq!(decode_utf8("Ärger\r\n".as_bytes()), "Ärger\n");
    }

    #[test]
    fn eng_149_ping_ok_needs_a_200_status_line() {
        assert!(ping_ok(
            "HTTP/1.1 200 OK
Api-Version: 1.50
"
        ));
        assert!(!ping_ok(
            "HTTP/1.1 500 Internal Server Error
"
        ));
        assert!(!ping_ok(
            "HTTP/1.1 503 Service Unavailable
"
        ));
        assert!(!ping_ok(""));
        assert!(!ping_ok("OK 200"));
    }

    #[test]
    fn eng_107_decodes_wsl_errors_in_either_encoding() {
        let bytes = include_bytes!("../fixtures/no-such-distro.utf16le.bin");
        let text = decode_message(bytes);
        assert!(text.contains("WSL_E_DISTRO_NOT_FOUND"), "{text}");
        assert_eq!(
            decode_message(b"sh: permission denied\n"),
            "sh: permission denied\n"
        );
        assert_eq!(decode_message(&utf16le("x", true)), "x");
        assert_eq!(decode_message(b""), "");
    }
}

/// A scripted `wsl.exe`: records every argv and answers from queues. Never touches a distro.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;

    use super::{Wsl, WslOutput, WslVersion};
    use crate::discovery::{BridgeTool, PROBE_SCRIPT};

    #[derive(Debug, Default)]
    struct State {
        running: Vec<String>,
        probes: VecDeque<i32>,
        pings: VecDeque<bool>,
        systemd: bool,
        start_exit: i32,
        calls: Vec<Vec<String>>,
        pinged: Vec<String>,
        /// Registered distro names (`--import` adds, `--unregister` removes).
        distros: Vec<String>,
        version: Option<WslVersion>,
        unsupported: bool,
        /// Scripted reply of `--import` (code, UTF-16LE message); `None` = success.
        import_fail: Option<(i32, String)>,
        import_delay: Duration,
        timeouts: Vec<Duration>,
    }

    #[derive(Debug, Default)]
    pub(crate) struct FakeWsl {
        state: Mutex<State>,
    }

    fn pop_or_last<T: Copy>(queue: &mut VecDeque<T>, default: T) -> T {
        match queue.len() {
            0 => default,
            1 => queue[0],
            _ => queue.pop_front().unwrap_or(default),
        }
    }

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> io::Result<WslOutput> {
        Ok(WslOutput {
            code: Some(code),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        })
    }

    impl FakeWsl {
        pub(crate) fn new(running: &[&str]) -> Arc<Self> {
            let fake = Self::default();
            {
                let mut state = fake.lock();
                state.running = running.iter().map(|name| (*name).to_owned()).collect();
                state.version = Some((3, 0, 3, 0));
            }
            Arc::new(fake)
        }

        pub(crate) fn distros(&self, names: &[&str]) -> &Self {
            self.lock().distros = names.iter().map(|name| (*name).to_owned()).collect();
            self
        }

        pub(crate) fn registered(&self) -> Vec<String> {
            self.lock().distros.clone()
        }

        pub(crate) fn version(&self, version: Option<WslVersion>) -> &Self {
            self.lock().version = version;
            self
        }

        pub(crate) fn unsupported(&self) -> &Self {
            self.lock().unsupported = true;
            self
        }

        /// `--import` fails with `code` and `message` (printed as UTF-16LE, like `wsl.exe`).
        pub(crate) fn import_fails(&self, code: i32, message: &str) -> &Self {
            self.lock().import_fail = Some((code, message.to_owned()));
            self
        }

        pub(crate) fn import_delay(&self, delay: Duration) -> &Self {
            self.lock().import_delay = delay;
            self
        }

        /// Timeouts passed to `run_timeout`, in call order.
        pub(crate) fn timeouts(&self) -> Vec<Duration> {
            self.lock().timeouts.clone()
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Probe-script exit codes, in order; the last one repeats (default 11).
        pub(crate) fn probes(&self, codes: &[i32]) -> &Self {
            self.lock().probes = codes.iter().copied().collect();
            self
        }

        /// `GET /_ping` outcomes, in order; the last one repeats (default: no answer).
        pub(crate) fn pings(&self, answers: &[bool]) -> &Self {
            self.lock().pings = answers.iter().copied().collect();
            self
        }

        pub(crate) fn systemd(&self, on: bool) -> &Self {
            self.lock().systemd = on;
            self
        }

        pub(crate) fn start_exit(&self, code: i32) -> &Self {
            self.lock().start_exit = code;
            self
        }

        pub(crate) fn calls(&self) -> Vec<Vec<String>> {
            self.lock().calls.clone()
        }

        /// Distros a `GET /_ping` was sent to.
        pub(crate) fn pinged(&self) -> Vec<String> {
            self.lock().pinged.clone()
        }
    }

    #[async_trait]
    impl Wsl for FakeWsl {
        fn supported(&self) -> bool {
            !self.lock().unsupported
        }

        async fn run_timeout(&self, args: &[&str], timeout: Duration) -> io::Result<WslOutput> {
            self.lock().timeouts.push(timeout);
            let delay = if args.first() == Some(&"--import") {
                self.lock().import_delay
            } else {
                Duration::ZERO
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            self.run(args).await
        }

        async fn registered_distros(&self) -> io::Result<Vec<String>> {
            Ok(self.lock().distros.clone())
        }

        async fn wsl_version(&self) -> Option<WslVersion> {
            self.lock().version
        }

        async fn run(&self, args: &[&str]) -> io::Result<WslOutput> {
            let mut state = self.lock();
            state
                .calls
                .push(args.iter().map(|arg| (*arg).to_owned()).collect());
            match args {
                ["--list", "--running", "--quiet"] => {
                    let names = state.running.join(
                        "
",
                    );
                    output(i32::from(names.is_empty()), &utf16le(&names), b"")
                }
                ["-d", _, "--exec", "sh", "-c", script] if *script == PROBE_SCRIPT => {
                    let code = pop_or_last(&mut state.probes, 11);
                    let stdout: &[u8] = if matches!(code, 0 | 13) {
                        b"/usr/bin/docker
"
                    } else {
                        b""
                    };
                    output(code, stdout, b"")
                }
                ["-d", _, "--exec", "test", "-d", "/run/systemd/system"] => {
                    output(i32::from(!state.systemd), b"", b"")
                }
                [
                    "-d",
                    _,
                    "-u",
                    "root",
                    "--exec",
                    "systemctl",
                    "start",
                    "docker",
                ]
                | [
                    "-d",
                    _,
                    "-u",
                    "root",
                    "--exec",
                    "/usr/sbin/service",
                    "docker",
                    "start",
                ] => {
                    let code = state.start_exit;
                    output(
                        code,
                        b"",
                        if code == 0 {
                            b""
                        } else {
                            b"boom: unit failed
"
                        },
                    )
                }
                ["-d", _, "--exec", "true"] => output(0, b"", b""),
                ["--import", name, _, _, "--version", "2"] => {
                    if let Some((code, message)) = state.import_fail.clone() {
                        return output(code, &utf16le(&message), b"");
                    }
                    state.distros.push((*name).to_owned());
                    output(0, &utf16le("The operation completed successfully."), b"")
                }
                ["--unregister", name] => {
                    let before = state.distros.len();
                    state
                        .distros
                        .retain(|existing| !existing.eq_ignore_ascii_case(name));
                    if state.distros.len() == before {
                        output(
                            -1,
                            &utf16le(
                                "There is no distribution with the supplied name. Error code: Wsl/Service/WSL_E_DISTRO_NOT_FOUND",
                            ),
                            b"",
                        )
                    } else {
                        output(0, &utf16le("The operation completed successfully."), b"")
                    }
                }
                _ => Err(io::Error::other(format!(
                    "unexpected wsl.exe argv: {args:?}"
                ))),
            }
        }

        async fn ping(&self, distro: &str, _tool: BridgeTool) -> io::Result<String> {
            let mut state = self.lock();
            state.pinged.push(distro.to_owned());
            let up = pop_or_last(&mut state.pings, false);
            Ok(if up {
                "HTTP/1.1 200 OK
"
                .to_owned()
            } else {
                String::new()
            })
        }
    }
}
