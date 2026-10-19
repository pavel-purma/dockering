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
