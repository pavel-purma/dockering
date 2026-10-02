//! Shared helpers for xtask subcommands.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

/// Workspace root (the parent of the `xtask` crate).
pub fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map_or_else(|| manifest_dir.to_path_buf(), Path::to_path_buf)
}

/// The cargo binary driving this xtask (falls back to `cargo` on `PATH`).
pub fn cargo() -> Command {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(workspace_root());
    cmd
}

/// Runs `cmd`, echoing it first, and fails on a non-zero exit status.
pub fn run(cmd: &mut Command) -> anyhow::Result<()> {
    eprintln!("xtask: running {}", describe(cmd));
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn {}", describe(cmd)))?;
    if !status.success() {
        bail!("{} exited with {status}", describe(cmd));
    }
    Ok(())
}

/// Runs `cmd` and returns its stdout as UTF-8.
pub fn output(cmd: &mut Command) -> anyhow::Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("failed to spawn {}", describe(cmd)))?;
    if !out.status.success() {
        bail!(
            "{} exited with {}: {}",
            describe(cmd),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).context("command output is not UTF-8")
}

fn describe(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().into_owned();
    for arg in cmd.get_args() {
        s.push(' ');
        s.push_str(&arg.to_string_lossy());
    }
    s
}

/// Parses `--flag value` / `--flag=value` pairs and bare `--switch`es.
pub struct Args<'a> {
    args: &'a [String],
}

impl<'a> Args<'a> {
    pub fn new(args: &'a [String]) -> Self {
        Self { args }
    }

    /// Value of `--name <v>` or `--name=<v>`.
    pub fn value(&self, name: &str) -> anyhow::Result<Option<&'a str>> {
        let prefix = format!("{name}=");
        let mut iter = self.args.iter();
        while let Some(arg) = iter.next() {
            if arg == name {
                return match iter.next() {
                    Some(v) => Ok(Some(v.as_str())),
                    None => bail!("{name} requires a value"),
                };
            }
            if let Some(v) = arg.strip_prefix(&prefix) {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.args.iter().any(|a| a == name)
    }

    /// Fails on any `--option` not in `known` (values following a known option are skipped).
    pub fn reject_unknown(
        &self,
        known_values: &[&str],
        known_flags: &[&str],
    ) -> anyhow::Result<()> {
        let mut iter = self.args.iter();
        while let Some(arg) = iter.next() {
            let name = arg.split('=').next().unwrap_or(arg);
            if known_values.contains(&name) {
                if !arg.contains('=') {
                    iter.next();
                }
            } else if !known_flags.contains(&arg.as_str()) {
                bail!("unexpected argument `{arg}`");
            }
        }
        Ok(())
    }
}

/// NFR-022: identifiers passed on to child processes are restricted to a safe charset.
pub fn validate_ident(kind: &str, value: &str) -> anyhow::Result<()> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!("invalid {kind} `{value}` (allowed: A-Z a-z 0-9 - _ .)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_values_and_flags() {
        let a = strings(&[
            "--target",
            "x86_64-pc-windows-msvc",
            "--formats=wix",
            "--no-archive",
        ]);
        let args = Args::new(&a);
        assert_eq!(
            args.value("--target").unwrap(),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(args.value("--formats").unwrap(), Some("wix"));
        assert!(args.flag("--no-archive"));
        args.reject_unknown(&["--target", "--formats"], &["--no-archive"])
            .unwrap();
        assert!(args.reject_unknown(&["--target"], &[]).is_err());
    }

    #[test]
    fn validates_identifiers() {
        assert!(validate_ident("engine", "docker-local_1.x").is_ok());
        assert!(validate_ident("engine", "a b").is_err());
        assert!(validate_ident("engine", "x;rm").is_err());
        assert!(validate_ident("engine", "").is_err());
    }
}
