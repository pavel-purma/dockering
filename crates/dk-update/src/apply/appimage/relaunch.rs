//! Step 5 of the AppImage apply (UPD-014): start the new file detached, keep the write end of
//! its stdin pipe alive for the rest of this process, and wait briefly to catch an immediate
//! crash.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ChildStdin, Stdio};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::RELAUNCH_FLAG;
use super::child::ChildEnv;
use super::seams::{Purpose, Seams};

/// How often the new process is polled during the grace period.
const POLL: Duration = Duration::from_millis(10);

/// The write ends of the stdin pipes of started new processes, **for the life of this process**.
///
/// The new process blocks until its stdin reaches end-of-file (UPD-017,
/// `dk_hub::single_instance::wait_for_predecessor`), which must happen only when the kernel
/// closes this process, after the single-instance socket is gone. A `static` is never dropped,
/// so no destructor can close a pipe while the old process is still shutting down: the entries
/// are only ever removed again for a child that already died, and by a test helper.
static RELAUNCH_PIPES: Mutex<Vec<(u32, ChildStdin)>> = Mutex::new(Vec::new());

fn keep_pipe(pid: u32, stdin: ChildStdin) {
    RELAUNCH_PIPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((pid, stdin));
}

/// Releases the pipe of a child that has exited (nobody is left to see its end-of-file).
fn forget_pipe(pid: u32) {
    RELAUNCH_PIPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(owner, _)| *owner != pid);
}

/// Closes every kept pipe, as the kernel does when the old process exits. Tests only.
#[cfg(test)]
pub(super) fn close_relaunch_pipes_for_tests() {
    RELAUNCH_PIPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// How many pipes are kept. Tests only.
#[cfg(test)]
pub(super) fn kept_pipes_for_tests() -> usize {
    RELAUNCH_PIPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

/// Starts `target` with the single argument [`RELAUNCH_FLAG`], in its own process group, stdout
/// and stderr discarded, stdin a pipe whose write end is kept in [`RELAUNCH_PIPES`], in the
/// scrubbed environment of `env`; then waits up to `grace` with `try_wait`.
///
/// `Ok` means the process was still running after `grace`. `Err` carries the reason for the
/// user-visible `InstalledRestartManually`: it could not be started, or it exited at once.
pub(super) fn start(
    target: &Path,
    env: &ChildEnv<'_>,
    grace: Duration,
    seams: &Seams<'_>,
) -> Result<(), String> {
    let mut command = env.command(target);
    command
        .arg(RELAUNCH_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = seams
        .spawner
        .spawn(&mut command, Purpose::Relaunch)
        .map_err(|error| format!("the new version couldn't be started: {error}"))?;
    let pid = child.id();
    // Immediately, before anything else can return or unwind: a pipe dropped early would let
    // end-of-file reach a live child while this process still owns the single-instance socket.
    if let Some(stdin) = child.stdin.take() {
        keep_pipe(pid, stdin);
    }

    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                forget_pipe(pid);
                return Err(format!("the new version exited at once ({status})"));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(format!("couldn't check the new version: {error}"));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        thread::sleep(POLL.min(deadline - now));
    }
    // Reap it when it ends, so a long-lived old process doesn't keep a zombie.
    let _ = thread::Builder::new()
        .name("dk-update-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}
