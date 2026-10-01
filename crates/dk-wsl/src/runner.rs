//! Private `wsl.exe` process runner and output decoders (spec 20 §4.2).
//!
//! Every invocation gets an argv vector (NFR-022), `CREATE_NO_WINDOW` (no console is created or
//! inherited), a null stdin, and a 10 s timeout. The child is killed when the timeout fires,
//! because `kill_on_drop` is set and the output future is dropped.
//!
//! Encodings: `wsl.exe --list …` and `wsl.exe`'s own error messages are UTF-16LE. Output of a
//! Linux program run through `--exec` is passed through unchanged (UTF-8).

#[cfg(windows)]
pub(crate) use platform::{command, run};

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

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::process::Command;
    use tokio::time::timeout;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    pub(crate) const WSL_TIMEOUT: Duration = Duration::from_secs(10);

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
            super::decode_utf8(&self.stdout)
        }

        /// A single-line, human-readable failure message from stderr, else stdout.
        pub(crate) fn error_message(&self) -> String {
            let stderr = super::decode_message(&self.stderr);
            let text = if stderr.trim().is_empty() {
                super::decode_message(&self.stdout)
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
    pub(crate) async fn run<I, S>(args: I) -> io::Result<WslOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command();
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Dropping the `output()` future on timeout drops the child, which kills it (kill_on_drop).
        let output = timeout(WSL_TIMEOUT, command.output()).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("wsl.exe timed out after {} s", WSL_TIMEOUT.as_secs()),
            )
        })??;

        Ok(WslOutput {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
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
