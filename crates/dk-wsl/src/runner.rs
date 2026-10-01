//! Private `wsl.exe` process runner.

#![cfg(windows)]

use std::ffi::OsStr;
use std::io;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WSL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct WslOutput {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl WslOutput {
    pub(crate) fn stdout_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stdout)
            .replace('\0', "")
            .replace('\r', "")
    }

    pub(crate) fn stderr_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stderr)
            .replace('\0', "")
            .replace('\r', "")
    }
}

pub(crate) fn command() -> Command {
    let mut command = Command::new("wsl.exe");
    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

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

    let output = timeout(WSL_TIMEOUT, command.output()).await.map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "wsl.exe timed out after 10 seconds",
        )
    })??;

    Ok(WslOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// `wsl.exe --list` emits UTF-16LE, with or without a BOM.
pub(crate) fn decode_utf16le(bytes: &[u8]) -> String {
    let mut units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    if units.first() == Some(&0xfeff) {
        units.remove(0);
    }
    String::from_utf16_lossy(&units)
        .replace('\0', "")
        .replace('\r', "")
}

#[cfg(test)]
mod tests {
    use super::decode_utf16le;

    fn utf16le(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = if bom { vec![0xff, 0xfe] } else { Vec::new() };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_utf16le_list_output_with_or_without_bom() {
        for bom in [false, true] {
            let mut bytes = utf16le("Ubuntu-22.04\r\n", bom);
            bytes.extend_from_slice(&[0, 0]);
            assert_eq!(decode_utf16le(&bytes), "Ubuntu-22.04\n");
        }
    }

    #[test]
    fn ignores_an_incomplete_final_code_unit() {
        let mut bytes = utf16le("Ubuntu", false);
        bytes.push(0xaa);
        assert_eq!(decode_utf16le(&bytes), "Ubuntu");
    }
}
