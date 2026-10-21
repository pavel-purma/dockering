//! Child processes and the start flow of Docker Desktop (ENG-144, 145, 147, 160).
//!
//! Every child gets an argv vector (no shell), an absolute path under the install folder, no
//! console window, null stdin and `NO_COLOR=1`. Output is never logged (NFR-020).

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dk_core::{EngineConfig, EngineError, EngineResult};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

use super::{
    Desktop, DesktopInstall, DesktopStatus, HINT, START_ARGV, STATUS_ARGV, first_line, parse_status,
};
use crate::DockerTarget;
use crate::connect::{self, PROBE_TIMEOUT};
use crate::discovery::normalize_pipe_path;
use crate::errors::{ErrCtx, HintCtx};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Captured output is bounded; the rest is read and dropped.
const MAX_OUTPUT: usize = 64 * 1024;
/// After the child exits, how long to wait for its pipes to close (a grandchild may hold them).
const DRAIN_GRACE: Duration = Duration::from_millis(500);

pub(super) struct Output {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(super) enum RunError {
    Spawn(std::io::Error),
    Timeout,
}

fn command(exe: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(exe);
    c.args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

type Buffer = Arc<Mutex<Vec<u8>>>;

fn collect<R: AsyncRead + Unpin + Send + 'static>(reader: Option<R>) -> (Buffer, JoinHandle<()>) {
    let buf = Buffer::default();
    let sink = buf.clone();
    let task = tokio::spawn(async move {
        let Some(mut reader) = reader else { return };
        let mut chunk = [0u8; 4096];
        while let Ok(n) = reader.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            let mut v = sink.lock().unwrap_or_else(|p| p.into_inner());
            let room = MAX_OUTPUT.saturating_sub(v.len());
            v.extend_from_slice(&chunk[..n.min(room)]);
        }
    });
    (buf, task)
}

async fn finish((buf, mut task): (Buffer, JoinHandle<()>)) -> Vec<u8> {
    if timeout(DRAIN_GRACE, &mut task).await.is_err() {
        task.abort();
    }
    std::mem::take(&mut *buf.lock().unwrap_or_else(|p| p.into_inner()))
}

/// Runs `exe args` and waits for it, at most `limit`. A timeout kills the child.
pub(super) async fn run_captured(
    exe: &Path,
    args: &[&str],
    limit: Duration,
) -> Result<Output, RunError> {
    let mut child = command(exe, args).spawn().map_err(RunError::Spawn)?;
    let out = collect(child.stdout.take());
    let err = collect(child.stderr.take());
    let status = match timeout(limit, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(RunError::Spawn(e)),
        Err(_) => {
            let _ = child.kill().await;
            out.1.abort();
            err.1.abort();
            return Err(RunError::Timeout);
        }
    };
    Ok(Output {
        success: status.success(),
        code: status.code(),
        stdout: finish(out).await,
        stderr: finish(err).await,
    })
}

/// Spawns `Docker Desktop.exe` and does not wait for it (it outlives Dockering).
fn launch_detached(install: &DesktopInstall) -> std::io::Result<()> {
    let mut c = Command::new(install.desktop_exe());
    c.current_dir(install.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    #[cfg(windows)]
    c.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    c.spawn().map(drop)
}

fn start_error(detail: &str) -> EngineError {
    EngineError::unreachable_with_hint(format!("Docker Desktop didn't start: {detail}"), HINT)
}

/// The invocation failed because this Docker Desktop has no `desktop` command (older version).
fn is_unknown_command(out: &Output) -> bool {
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_ascii_lowercase();
    text.contains("unknown command") || text.contains("is not a docker command")
}

async fn ping(pipe: &str) -> bool {
    let target = DockerTarget::NamedPipe(pipe.to_owned());
    let ctx = ErrCtx {
        hint: HintCtx::Pipe,
        timeout: PROBE_TIMEOUT,
    };
    let Ok(docker) = connect::build_client(&target, PROBE_TIMEOUT, &connect::min_version(), &ctx)
    else {
        return false;
    };
    matches!(timeout(PROBE_TIMEOUT, docker.ping()).await, Ok(Ok(_)))
}

impl Desktop {
    /// ENG-144: explicit user action only.
    pub(crate) async fn start(&self, cfg: &EngineConfig) -> EngineResult<()> {
        let install = match self.prepare(cfg).await {
            Some(i) => i,
            None => {
                return Err(EngineError::Api {
                    status: 0,
                    message: "this engine can't be started from Dockering".into(),
                });
            }
        };
        let docker = install.docker_exe();
        match run_captured(&docker, &START_ARGV, self.timeouts.start).await {
            Ok(out) if out.success => {
                tracing::debug!("docker desktop start finished");
                Ok(())
            }
            Ok(out) if is_unknown_command(&out) => self.fallback(&install, cfg).await,
            Ok(out) => {
                tracing::debug!(code = ?out.code, "docker desktop start failed");
                let detail = first_line(&out.stderr)
                    .or_else(|| first_line(&out.stdout))
                    .unwrap_or_else(|| match out.code {
                        Some(c) => format!("exit code {c}"),
                        None => "it was stopped".into(),
                    });
                Err(start_error(&detail))
            }
            Err(RunError::Timeout) => Err(start_error("it didn't finish in time")),
            Err(RunError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                self.fallback(&install, cfg).await
            }
            Err(RunError::Spawn(e)) => Err(start_error(&e.to_string())),
        }
    }

    /// Older Docker Desktop: launch it directly and wait for the engine pipe to answer.
    async fn fallback(&self, install: &DesktopInstall, cfg: &EngineConfig) -> EngineResult<()> {
        tracing::debug!("docker desktop command missing; launching the app");
        if let Err(e) = launch_detached(install) {
            return Err(start_error(&e.to_string()));
        }
        let dk_core::EngineEndpoint::NamedPipe { path } = &cfg.endpoint else {
            return Err(start_error("no pipe to wait for"));
        };
        let pipe = normalize_pipe_path(path);
        let t = self.timeouts;
        let deadline = Instant::now() + t.poll;
        loop {
            if ping(&pipe).await {
                return Ok(());
            }
            if Instant::now() + t.poll_interval >= deadline {
                return Err(start_error("it didn't answer in time"));
            }
            sleep(t.poll_interval).await;
        }
    }

    /// ENG-145: `docker desktop status --format json`, bounded. Never from `probe`.
    pub(crate) async fn status(&self, install: &DesktopInstall) -> DesktopStatus {
        match run_captured(&install.docker_exe(), &STATUS_ARGV, self.timeouts.status).await {
            Ok(out) => parse_status(out.success, &String::from_utf8_lossy(&out.stdout)),
            Err(_) => DesktopStatus::NotRunning,
        }
    }

    /// The install, when `cfg` is a Docker Desktop engine.
    async fn prepare(&self, cfg: &EngineConfig) -> Option<DesktopInstall> {
        if !self.is_desktop_endpoint(&cfg.endpoint) {
            return None;
        }
        self.load().await
    }
}
