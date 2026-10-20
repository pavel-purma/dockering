//! Keeping the app's own descriptors out of the children of an update (UPD-014).
//!
//! The AppImage runtime keeps a descriptor on its mount point open for as long as the app runs,
//! without `FD_CLOEXEC`: the mount lives while anything holds it. Every child the app starts
//! inherits it, so a relaunched app would keep the *old* mount, its FUSE daemon and the replaced
//! AppImage's disk space alive until it quits (observed 2026-10-20 in the FUSE leg of the update
//! end-to-end test: `fd 1023` of the new process pointed at the old mount). A child of an update
//! needs none of the parent's descriptors above stdin, stdout and stderr, so they are marked
//! close-on-exec in the child between `fork` and `exec`.
//!
//! This is the only `unsafe` in the AppImage strategy: `CommandExt::pre_exec` is unsafe because
//! the closure runs in a forked child, where only async-signal-safe calls are allowed.

#![allow(unsafe_code)]

use std::os::unix::process::CommandExt;
use std::process::Command;

/// First descriptor a child keeps: 0, 1 and 2 are its standard streams.
const FIRST_INHERITED_FD: libc::c_uint = 3;

/// Upper bound of the fallback loop. The syscall below covers every descriptor; the loop only
/// runs on kernels without `close_range` and would never need to look beyond a sane limit.
const FALLBACK_MAX_FD: libc::c_int = 65_536;

/// `CLOSE_RANGE_CLOEXEC` (Linux 5.11): mark the range close-on-exec instead of closing it.
const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;

/// Makes `command`'s child close every descriptor above stderr when it executes the program.
///
/// The descriptors are marked close-on-exec, not closed, so the pipe `std` uses to report a
/// failed `exec` to the parent (it is close-on-exec itself) still works.
pub(super) fn close_inherited_fds(command: &mut Command) {
    // SAFETY: the closure runs in the forked child before `exec` and calls only the
    // async-signal-safe `syscall` and `fcntl` (no allocation, no locks, no Rust runtime state).
    unsafe {
        command.pre_exec(|| {
            mark_close_on_exec_from(FIRST_INHERITED_FD);
            Ok(())
        });
    }
}

/// Marks every descriptor from `first` up close-on-exec. Best effort: a descriptor that can't be
/// marked is just inherited, as it was before; nothing here may fail the spawn.
fn mark_close_on_exec_from(first: libc::c_uint) {
    // SAFETY: `close_range` takes plain integers and no pointers.
    let done = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            first,
            libc::c_uint::MAX,
            CLOSE_RANGE_CLOEXEC,
        )
    } == 0;
    if done {
        return;
    }
    // Kernels before 5.11 (or a seccomp filter that refuses the call): one `fcntl` per descriptor.
    for fd in first as libc::c_int..FALLBACK_MAX_FD {
        // SAFETY: `fcntl(F_SETFD)` on a number that may not be open is harmless (EBADF).
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::process::{Command, Stdio};

    use super::*;

    /// An open descriptor on `path` WITHOUT close-on-exec, like the one the AppImage runtime keeps
    /// on its mount point (Rust's own `File::open` always sets it, so `dup` makes the copy).
    fn inheritable_fd_on(path: &std::path::Path) -> (File, OwnedFd) {
        let file = File::open(path).expect("open");
        // SAFETY: `dup` of a descriptor this test owns returns a new descriptor it then owns.
        let fd = unsafe { OwnedFd::from_raw_fd(libc::dup(file.as_raw_fd())) };
        (file, fd)
    }

    /// What a `sh` child sees open: the targets of its descriptors above stderr, one per line.
    fn open_in_child(command: &mut Command) -> String {
        let out = command
            .args(["-c", "for f in /proc/$$/fd/*; do readlink \"$f\"; done"])
            .stdin(Stdio::null())
            .output()
            .expect("sh runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn upd_014_children_do_not_inherit_the_runtime_mount_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("the-old-mount");
        std::fs::write(&marker, b"x").expect("write");
        let (_file, fd) = inheritable_fd_on(&marker);
        assert!(fd.as_raw_fd() >= 3);

        // The failure the end-to-end test found: a plain child inherits the descriptor ...
        let plain = open_in_child(&mut Command::new("sh"));
        assert!(
            plain.contains(marker.to_str().expect("utf-8")),
            "the control child must see the descriptor, or this test proves nothing: {plain}"
        );
        // ... and a child of an update does not.
        let mut update_child = Command::new("sh");
        close_inherited_fds(&mut update_child);
        let seen = open_in_child(&mut update_child);
        assert!(
            !seen.contains(marker.to_str().expect("utf-8")),
            "the old mount's descriptor reached the child: {seen}"
        );
    }

    #[test]
    fn upd_014_standard_streams_are_kept() {
        let mut command = Command::new("sh");
        close_inherited_fds(&mut command);
        let out = command
            .args(["-c", "echo out; echo err >&2; read line; echo \"in:$line\""])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut child = out;
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("stdin");
            writeln!(stdin, "hello").expect("write");
        }
        let output = child.wait_with_output().expect("wait");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out\nin:hello\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
    }

    #[test]
    fn upd_014_a_failed_exec_is_still_reported_to_the_parent() {
        // `std` reports an `exec` failure through a close-on-exec pipe above stderr: marking the
        // range close-on-exec (rather than closing it) must leave that pipe working.
        let mut command = Command::new("/nonexistent/dockering-update-test");
        close_inherited_fds(&mut command);
        let error = command.spawn().expect_err("exec must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
    }
}
