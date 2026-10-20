//! The one place a child process of an AppImage update is configured (UPD-014): the self-test and
//! the relaunch get the same scrubbed environment from here.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::apply::env::{EnvPlan, scrubbed_env};

/// The environment both children get: the process environment minus the scrub.
pub(super) struct ChildEnv<'a> {
    vars: &'a [(OsString, OsString)],
    plan: EnvPlan,
}

impl<'a> ChildEnv<'a> {
    /// Plans the scrub of `vars` for the `APPDIR` they name.
    pub(super) fn new(vars: &'a [(OsString, OsString)]) -> Self {
        let appdir = vars
            .iter()
            .rev()
            .find(|(name, _)| name == "APPDIR")
            .map(|(_, value)| value.as_os_str());
        Self {
            vars,
            plan: scrubbed_env(vars, appdir),
        }
    }

    /// A builder for `program` whose environment is exactly `vars` with the plan applied. The
    /// child does not inherit from the running process: in production `vars` *is* that
    /// environment, and a test controls what the child sees. Nothing here changes the running
    /// process (no `set_var`/`remove_var`).
    pub(super) fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env_clear();
        // The runtime's mount descriptor must not reach the new app (see `fds`).
        super::fds::close_inherited_fds(&mut command);
        command.envs(
            self.vars
                .iter()
                .map(|(name, value)| (name.as_os_str(), value.as_os_str())),
        );
        self.plan.apply_to(&mut command);
        command
    }

    /// The plan, for tests.
    #[cfg(test)]
    pub(super) fn plan(&self) -> &EnvPlan {
        &self.plan
    }
}

/// The value `command` ends up with for `name`, for tests: `None` is unset.
#[cfg(test)]
pub(super) fn env_of(command: &Command, name: &str) -> Option<OsString> {
    use std::ffi::OsStr;
    command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(name))
        .and_then(|(_, value)| value.map(OsStr::to_owned))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::process::Stdio;

    use super::*;

    /// UPD-014: both children of an update (the self-test and the relaunch) come from `command`,
    /// so a descriptor without close-on-exec, like the one the AppImage runtime keeps on its
    /// mount directory, must not reach them. Dropping the call in `command` fails this test.
    #[test]
    #[allow(unsafe_code)]
    fn upd_014_command_children_do_not_inherit_open_descriptors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("the-old-mount");
        std::fs::write(&marker, b"x").expect("write");
        let file = File::open(&marker).expect("open");
        // SAFETY: `dup` of a descriptor this test owns returns a new descriptor it then owns.
        let inheritable = unsafe { OwnedFd::from_raw_fd(libc::dup(file.as_raw_fd())) };
        assert!(inheritable.as_raw_fd() >= 3);

        let vars = vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
        let env = ChildEnv::new(&vars);
        let output = env
            .command(Path::new("/bin/sh"))
            .args(["-c", "for f in /proc/$$/fd/*; do readlink \"$f\"; done"])
            .stdin(Stdio::null())
            .output()
            .expect("sh runs");
        let seen = String::from_utf8_lossy(&output.stdout);
        // The shell ran: its stdin is `/dev/null`. (Its exit status isn't checked: the last `readlink`
        // can name the descriptor the glob itself had open.)
        assert!(seen.contains("/dev/null"), "the shell did not run: {seen}");
        assert!(
            !seen.contains(marker.to_str().expect("utf-8")),
            "the descriptor reached the child: {seen}"
        );
    }
}
