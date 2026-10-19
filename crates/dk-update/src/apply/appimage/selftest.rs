//! Step 3 of the AppImage apply (UPD-014): the `--version` self-test.
//!
//! The staged (or installed) file must run, print exactly `dockering <version>` and exit with
//! status 0, within a time limit. That output format and the exit status are **frozen** from
//! the first updater release (UPD-017): an old binary starts and tests a new one.

use std::io::Read;
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::child::ChildEnv;
use super::seams::{ETXTBSY, Purpose, Seams};
use crate::UpdateError;

/// `ETXTBSY` is retried this many times (so up to six attempts), `Seams::etxtbsy_delay` apart.
const ETXTBSY_RETRIES: u32 = 5;

/// How often the child is polled; also the granularity of the time limit.
const POLL: Duration = Duration::from_millis(10);

/// A well-behaved `--version` prints one short line.
const OUTPUT_LIMIT: u64 = 4096;

/// How long to wait for the output after the child has exited, in case a grandchild keeps the
/// pipe open.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// Runs `file --version` and checks the answer against `expected_version`.
///
/// argv vector, closed stdin, stdout piped, stderr discarded, the scrubbed environment of `env`.
/// Any failure (it can't run, a timeout, a crash, a non-zero exit, a different output) is
/// [`UpdateError::SelfTest`]. On a timeout the child is killed (a descendant it started may
/// outlive it).
pub(super) fn self_test(
    file: &Path,
    expected_version: &str,
    env: &ChildEnv<'_>,
    timeout: Duration,
    seams: &Seams<'_>,
) -> Result<(), UpdateError> {
    let mut child = spawn_retrying(file, env, seams)?;
    let output = collect_output(&mut child);
    let status = wait(&mut child, timeout)?;
    if !status.success() {
        return Err(UpdateError::SelfTest(format!(
            "--version ended with {status}"
        )));
    }
    let output = output
        .and_then(|receiver| receiver.recv_timeout(OUTPUT_GRACE).ok())
        .ok_or_else(|| UpdateError::SelfTest("--version printed nothing readable".to_owned()))?;
    if !version_output_matches(&output, expected_version) {
        return Err(UpdateError::SelfTest(format!(
            "--version printed `{}`, expected `dockering {expected_version}`",
            snippet(&output)
        )));
    }
    Ok(())
}

/// UPD-014 / UPD-017 (frozen): the output is `dockering <version>` and nothing else; only
/// leading and trailing whitespace (the final newline) is ignored.
pub(super) fn version_output_matches(output: &[u8], expected_version: &str) -> bool {
    std::str::from_utf8(output)
        .is_ok_and(|text| text.trim() == format!("dockering {expected_version}"))
}

fn snippet(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .trim()
        .chars()
        .take(80)
        .collect()
}

/// Spawns the child; `ETXTBSY` (the file was just written, and a descriptor of it may still be
/// open in a process that is forking) is retried [`ETXTBSY_RETRIES`] times.
fn spawn_retrying(
    file: &Path,
    env: &ChildEnv<'_>,
    seams: &Seams<'_>,
) -> Result<Child, UpdateError> {
    let mut retries = 0;
    loop {
        let mut command = env.command(file);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        match seams.spawner.spawn(&mut command, Purpose::SelfTest) {
            Ok(child) => return Ok(child),
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && retries < ETXTBSY_RETRIES => {
                retries += 1;
                thread::sleep(seams.etxtbsy_delay);
            }
            Err(error) => {
                return Err(UpdateError::SelfTest(format!(
                    "the new version couldn't be run: {error}"
                )));
            }
        }
    }
}

/// Reads at most [`OUTPUT_LIMIT`] bytes of the child's stdout on a helper thread, so a chatty
/// child can't block on a full pipe while we wait for it.
fn collect_output(child: &mut Child) -> Option<mpsc::Receiver<Vec<u8>>> {
    let stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("dk-update-self-test".to_owned())
        .spawn(move || {
            let mut buffer = Vec::new();
            let _ = stdout.take(OUTPUT_LIMIT).read_to_end(&mut buffer);
            let _ = sender.send(buffer);
        })
        .ok()?;
    Some(receiver)
}

/// Polls `try_wait` until the child exits or `timeout` passes (then it is killed and reaped).
fn wait(child: &mut Child, timeout: Duration) -> Result<ExitStatus, UpdateError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UpdateError::SelfTest(format!(
                    "couldn't wait for --version: {error}"
                )));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::SelfTest(format!(
                "--version didn't finish within {} s",
                timeout.as_secs_f32().ceil()
            )));
        }
        thread::sleep(POLL.min(deadline - now));
    }
}
