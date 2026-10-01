//! `wslc.exe` process runner (spec 20 §5.5): argv vectors only, `CREATE_NO_WINDOW`,
//! `NO_COLOR=1`, null stdin, lossy UTF-8 capture, timeouts, `kill_on_drop`, and at most
//! [`MAX_CONCURRENT`] concurrent request/response invocations per engine.
//!
//! Long-running children (events, logs, pull, exec) are spawned through [`Runner::command`]
//! and are not counted against the semaphore: they live as long as their view, and counting
//! them would starve short requests (a detail page holds events + logs + terminal).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use dk_core::{EngineError, EngineResult, ResourceKind};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Per-engine process limit (spec 20 §5.5).
pub(crate) const MAX_CONCURRENT: usize = 4;
/// Default timeout for request/response invocations.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// `container run` may pull the image first.
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `image pull`.
pub(crate) const PULL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `CREATE_NO_WINDOW`: no console window flashes up for each child.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) const UPDATE_HINT: &str = "WSL containers need WSL ≥ 2.9.3 — run `wsl --update`";
pub(crate) const POLICY_HINT: &str = "WSL containers are disabled by Group Policy";
const ELEVATION_HINT: &str =
    "This WSLC session belongs to an elevated wslc; run Dockering as administrator to manage it";

/// What the caller was operating on; used when stderr doesn't name the object (NotFound).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ErrCtx<'a> {
    pub kind: ResourceKind,
    pub id: &'a str,
}

impl<'a> ErrCtx<'a> {
    pub fn new(kind: ResourceKind, id: &'a str) -> Self {
        Self { kind, id }
    }
}

/// Captured output of a successful invocation.
#[derive(Debug, Clone, Default)]
pub(crate) struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone)]
pub(crate) struct Runner {
    exe: PathBuf,
    session: Option<String>,
    sem: Arc<Semaphore>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("exe", &self.exe)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Runner {
    pub fn new(exe: PathBuf, session: Option<String>) -> Self {
        Self {
            exe,
            session,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Full argv after the program name: `[--session <s>] <args…>`. The global
    /// `--session` option must come before the command (verified on wslc 3.0.1).
    pub fn argv<S: AsRef<str>>(&self, args: &[S]) -> Vec<String> {
        let mut v = Vec::with_capacity(args.len() + 2);
        if let Some(s) = &self.session {
            v.push("--session".to_owned());
            v.push(s.clone());
        }
        v.extend(args.iter().map(|a| a.as_ref().to_owned()));
        v
    }

    /// A configured, not yet spawned command (stdout/stderr piped, stdin null).
    pub fn command<S: AsRef<str>>(&self, args: &[S]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args(self.argv(args))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    /// Wait for one of the [`MAX_CONCURRENT`] slots.
    pub async fn permit(&self) -> EngineResult<OwnedSemaphorePermit> {
        self.sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| EngineError::Cancelled)
    }

    /// Run to completion. Non-zero exit → [`map_cli_error`].
    pub async fn run<S: AsRef<str>>(
        &self,
        args: &[S],
        timeout: Duration,
        ctx: Option<ErrCtx<'_>>,
    ) -> EngineResult<CmdOutput> {
        let _permit = self.permit().await?;
        let mut cmd = self.command(args);
        let label = args
            .first()
            .map(|a| a.as_ref().to_owned())
            .unwrap_or_default();
        tracing::debug!(target: "dk_engine_wslc::cli", cmd = %label, "wslc invocation");
        let child = cmd.spawn().map_err(|e| spawn_error(&self.exe, &e))?;
        // Dropping the future on timeout drops the child, which kills it (kill_on_drop).
        let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| EngineError::unreachable(format!("wslc.exe failed: {e}")))?,
            Err(_) => return Err(EngineError::Timeout(timeout)),
        };
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if out.status.success() {
            Ok(CmdOutput { stdout, stderr })
        } else {
            Err(map_cli_error(out.status.code(), &stderr, ctx))
        }
    }
}

/// Error for a child that couldn't be spawned at all.
pub(crate) fn spawn_error(exe: &Path, e: &std::io::Error) -> EngineError {
    EngineError::unreachable_with_hint(format!("can't run {}: {e}", exe.display()), UPDATE_HINT)
}

/// First non-empty, trimmed stderr line (the human message).
pub(crate) fn first_line(stderr: &str) -> &str {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// The `Error code: <SYMBOL>` line wslc prints for service errors, if any.
pub(crate) fn error_code(stderr: &str) -> Option<&str> {
    stderr.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Error code:")
            .map(str::trim)
            .filter(|c| !c.is_empty())
    })
}

/// The first `'quoted'` value in `s`.
fn quoted(s: &str) -> Option<&str> {
    let start = s.find('\'')? + 1;
    let len = s[start..].find('\'')?;
    Some(&s[start..start + len])
}

/// Resource kind from a message such as `Container 'x' not found.` / `Volume not found: 'x'`.
fn kind_from_text(line: &str) -> Option<ResourceKind> {
    let l = line.trim_start().to_ascii_lowercase();
    [
        ("container", ResourceKind::Container),
        ("image", ResourceKind::Image),
        ("volume", ResourceKind::Volume),
        ("network", ResourceKind::Network),
        ("session", ResourceKind::Session),
    ]
    .into_iter()
    .find_map(|(p, k)| l.starts_with(p).then_some(k))
}

/// Resource kind from a `WSLC_E_<KIND>_NOT_FOUND` code.
fn kind_from_code(code: &str) -> Option<ResourceKind> {
    match code {
        "WSLC_E_CONTAINER_NOT_FOUND" => Some(ResourceKind::Container),
        "WSLC_E_IMAGE_NOT_FOUND" => Some(ResourceKind::Image),
        "WSLC_E_VOLUME_NOT_FOUND" => Some(ResourceKind::Volume),
        "WSLC_E_NETWORK_NOT_FOUND" => Some(ResourceKind::Network),
        _ => None,
    }
}

/// Map a failed invocation to an `EngineError` (spec 20 §5.5; codes as in §5.4).
///
/// wslc 3.0.1 prints a human message, then (for service errors) `Error code: <SYMBOL>`, then a
/// boilerplate "file an issue" line. Lookups (`inspect`) print only the message.
pub(crate) fn map_cli_error(
    code: Option<i32>,
    stderr: &str,
    ctx: Option<ErrCtx<'_>>,
) -> EngineError {
    let msg = first_line(stderr).to_owned();
    let sym = error_code(stderr).unwrap_or("");
    let lower = stderr.to_ascii_lowercase();

    // Policy first: never retried, never falls back (spec 20 §5.2).
    if sym == "WSLC_E_CONTAINER_DISABLED"
        || (lower.contains("disabled") && lower.contains("policy"))
    {
        return EngineError::unreachable_with_hint(msg, POLICY_HINT);
    }
    if sym == "WSLC_E_SESSION_NOT_FOUND" || lower.contains("session not found") {
        return EngineError::unreachable_with_hint(
            msg,
            "The WSLC session doesn't exist; start one with `wslc` or pick another session",
        );
    }
    if matches!(
        sym,
        "WSLC_E_VM_NOT_RUNNING" | "RPC_E_DISCONNECTED" | "RPC_S_SERVER_UNAVAILABLE"
    ) || lower.contains("vm not running")
        || lower.contains("virtual machine is not running")
    {
        return EngineError::unreachable(msg);
    }
    if sym == "ERROR_ELEVATION_REQUIRED" || lower.contains("requires elevation") {
        return EngineError::unreachable_with_hint(msg, ELEVATION_HINT);
    }

    let code_kind = kind_from_code(sym);
    let text_nf = stderr
        .lines()
        .map(str::trim)
        .find(|l| l.to_ascii_lowercase().contains("not found"));
    if code_kind.is_some() || text_nf.is_some() {
        let line = text_nf.unwrap_or(&msg);
        let kind = code_kind
            .or_else(|| kind_from_text(line))
            .or(ctx.map(|c| c.kind))
            .unwrap_or(ResourceKind::Container);
        let id = quoted(line)
            .map(str::to_owned)
            .or_else(|| ctx.map(|c| c.id.to_owned()))
            .unwrap_or_default();
        return EngineError::not_found(kind, id);
    }

    if matches!(
        sym,
        "WSLC_E_CONTAINER_IS_RUNNING"
            | "WSLC_E_CONTAINER_NOT_RUNNING"
            | "WSLC_E_CONTAINER_PREFIX_AMBIGUOUS"
            | "ERROR_ALREADY_EXISTS"
            | "ERROR_SHARING_VIOLATION"
    ) || lower.contains("is running")
        || lower.contains("not running")
        || lower.contains("ambiguous")
        || lower.starts_with("conflict")
        || lower.contains("is in use")
        || lower.contains("already in use")
    {
        return EngineError::Conflict(msg);
    }

    let status = code.unwrap_or(-1).clamp(0, i32::from(u16::MAX)) as u16;
    let message = if msg.is_empty() {
        format!("wslc exited with code {}", code.unwrap_or(-1))
    } else {
        msg
    };
    EngineError::Api { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                "../../tests/fixtures/cli/errors/",
                $name,
                ".stderr"
            ))
        };
    }

    #[test]
    fn s2_inspect_not_found_without_code() {
        let e = map_cli_error(Some(1), fixture!("inspect_not_found"), None);
        assert_eq!(
            e,
            EngineError::not_found(ResourceKind::Container, "deadbeefdeadbeef")
        );
    }

    #[test]
    fn s2_not_found_with_code_and_text() {
        let e = map_cli_error(Some(1), fixture!("stop_not_found"), None);
        assert_eq!(
            e,
            EngineError::not_found(ResourceKind::Container, "dk-cli-nosuch")
        );
        let e = map_cli_error(Some(1), fixture!("volume_not_found"), None);
        assert_eq!(e, EngineError::not_found(ResourceKind::Volume, "nosuchvol"));
        let e = map_cli_error(Some(1), fixture!("network_not_found"), None);
        assert_eq!(
            e,
            EngineError::not_found(ResourceKind::Network, "nosuchnet")
        );
        let e = map_cli_error(Some(1), fixture!("image_not_found"), None);
        assert_eq!(e, EngineError::not_found(ResourceKind::Image, "nosuch:img"));
    }

    #[test]
    fn s2_conflicts() {
        for f in [
            fixture!("remove_running"),
            fixture!("kill_not_running"),
            fixture!("name_conflict"),
            fixture!("volume_in_use"),
        ] {
            let e = map_cli_error(Some(1), f, None);
            assert!(matches!(e, EngineError::Conflict(_)), "{e:?}");
        }
        let EngineError::Conflict(m) = map_cli_error(Some(1), fixture!("remove_running"), None)
        else {
            unreachable!()
        };
        assert!(m.starts_with("Container 'da29"), "{m}");
        assert!(!m.contains("Error code"));
    }

    #[test]
    fn s2_unreachable() {
        let e = map_cli_error(Some(1), fixture!("session_not_found"), None);
        assert!(e.is_unreachable() && e.hint().is_some(), "{e:?}");
        let e = map_cli_error(Some(1), fixture!("elevation_required"), None);
        assert!(e.is_unreachable(), "{e:?}");
        let e = map_cli_error(
            Some(1),
            "WSL containers are disabled.\nError code: WSLC_E_CONTAINER_DISABLED\n",
            None,
        );
        assert_eq!(e.hint(), Some(POLICY_HINT));
        let e = map_cli_error(Some(1), "boom\nError code: WSLC_E_VM_NOT_RUNNING\n", None);
        assert!(e.is_unreachable());
    }

    #[test]
    fn s2_other_errors_are_api_with_first_line() {
        let e = map_cli_error(Some(1), fixture!("bad_volume_driver"), None);
        assert_eq!(
            e,
            EngineError::Api {
                status: 1,
                message: "Unsupported volume type: 'local'".into()
            }
        );
        let e = map_cli_error(Some(1), fixture!("unknown_command"), None);
        assert!(
            matches!(e, EngineError::Api { status: 1, ref message } if message == "Unrecognized command: 'pause'")
        );
        // Exit codes are clamped into u16.
        let e = map_cli_error(Some(-2147024891), "x", None);
        assert!(matches!(e, EngineError::Api { status: 0, .. }));
        let e = map_cli_error(Some(70000), "", None);
        assert!(matches!(
            e,
            EngineError::Api {
                status: u16::MAX,
                ..
            }
        ));
    }

    #[test]
    fn not_found_uses_ctx_when_message_is_vague() {
        let e = map_cli_error(
            Some(1),
            "\nError code: WSLC_E_CONTAINER_NOT_FOUND\n",
            Some(ErrCtx::new(ResourceKind::Container, "abc")),
        );
        assert_eq!(e, EngineError::not_found(ResourceKind::Container, "abc"));
        // `container stats` prints an empty message line before the code.
        let e = map_cli_error(
            Some(1),
            fixture!("stats_not_found"),
            Some(ErrCtx::new(ResourceKind::Container, "nosuch")),
        );
        assert_eq!(e, EngineError::not_found(ResourceKind::Container, "nosuch"));
    }

    #[test]
    fn argv_puts_session_first() {
        let r = Runner::new("wslc.exe".into(), Some("s1".into()));
        assert_eq!(
            r.argv(&["container", "list"]),
            ["--session", "s1", "container", "list"]
        );
        let r = Runner::new("wslc.exe".into(), None);
        assert_eq!(r.argv(&["version"]), ["version"]);
    }
}
