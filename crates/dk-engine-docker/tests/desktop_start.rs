//! Docker Desktop discovery, start and status against a fake install tree (ENG-143…147, 160).
//!
//! Never runs the real `docker.exe` or `Docker Desktop.exe`: the install folder is a temp dir
//! holding copies of the `fake-docker` binary, and the pipes are unique test pipes.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dk_core::engine::preference;
use dk_core::{
    EngineConfig, EngineEndpoint, EngineError, EngineFactory, EngineId, EngineOrigin, EngineState,
    ProbeResult,
};
use dk_engine_docker::{DesktopTestConfig, DockerFactory};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-docker");

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    /// `with_docker`: a fake `resources\bin\docker.exe`; `Docker Desktop.exe` always exists.
    fn new(with_docker: bool, mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let tree = Self { dir };
        std::fs::copy(FAKE, tree.root().join("Docker Desktop.exe")).unwrap();
        if with_docker {
            let bin = tree.root().join("resources").join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::copy(FAKE, bin.join("docker.exe")).unwrap();
            tree.mode(mode);
        }
        tree
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn docker(&self) -> PathBuf {
        self.root().join("resources").join("bin").join("docker.exe")
    }

    fn desktop(&self) -> PathBuf {
        self.root().join("Docker Desktop.exe")
    }

    fn mode(&self, text: &str) {
        std::fs::write(sibling(&self.docker(), ".mode"), text).unwrap();
    }

    fn docker_log(&self) -> Vec<String> {
        read_log(&sibling(&self.docker(), ".log"))
    }

    fn desktop_log(&self) -> Vec<String> {
        read_log(&sibling(&self.desktop(), ".log"))
    }

    fn config(&self) -> DesktopTestConfig {
        let mut c = DesktopTestConfig::new(self.root());
        c.start_timeout = Duration::from_secs(10);
        c
    }
}

fn sibling(exe: &Path, ext: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap().to_os_string();
    name.push(ext);
    exe.with_file_name(name)
}

fn read_log(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn cfg(pipe: &str) -> EngineConfig {
    EngineConfig {
        id: EngineId::new("docker-desktop"),
        name: "Docker Desktop".into(),
        endpoint: EngineEndpoint::NamedPipe { path: pipe.into() },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
}

/// Serves `HTTP 200 OK` to every request on `pipe` until dropped.
struct PipeServer(tokio::task::JoinHandle<()>);

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn serve(pipe: String) -> PipeServer {
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe)
        .unwrap();
    PipeServer(tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                return;
            }
            let conn = server;
            let Ok(next) = ServerOptions::new().create(&pipe) else {
                return;
            };
            server = next;
            tokio::spawn(async move {
                let mut conn = conn;
                let mut buf = [0u8; 4096];
                while let Ok(n) = conn.read(&mut buf).await {
                    if n == 0 {
                        return;
                    }
                    let reply = "HTTP/1.1 200 OK\r\nApi-Version: 1.41\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK";
                    if conn.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    }))
}

fn unreachable_error() -> EngineError {
    EngineError::unreachable("pipe not found")
}

#[tokio::test]
async fn eng_143_installed_desktop_without_pipe_is_stopped() {
    let tree = Tree::new(true, "");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    assert!(
        f.start_prompt(&cfg(&pipe)).is_none(),
        "unknown before discovery"
    );

    let found = f.discover().await;
    assert_eq!(found.len(), 1, "{found:?}");
    let d = &found[0];
    assert_eq!(d.config.id.as_str(), "docker-desktop");
    assert_eq!(d.config.name, "Docker Desktop");
    assert_eq!(
        d.config.endpoint,
        EngineEndpoint::NamedPipe { path: pipe.clone() }
    );
    assert_eq!(d.initial_state, Some(EngineState::Stopped));
    assert_eq!(d.preference, preference::LOCAL_SOCKET);

    let prompt = f.start_prompt(&d.config).expect("prompt");
    assert_eq!(prompt.title, "Docker Desktop isn't running");
    assert_eq!(prompt.action_label, "Start Docker Desktop");
    assert!(matches!(f.probe(&d.config).await, ProbeResult::Stopped));
    // Another engine kind never gets the prompt.
    let mut other = cfg(&pipe);
    other.endpoint = EngineEndpoint::NamedPipe {
        path: r"\\.\pipe\someone_else".into(),
    };
    assert!(f.start_prompt(&other).is_none());
}

#[tokio::test]
async fn eng_143_pipe_present_keeps_current_behaviour() {
    let tree = Tree::new(true, "");
    let c = tree.config();
    let _server = serve(c.engine_pipe.clone());
    let f = DockerFactory::for_desktop_tests(c);
    let found = f.discover().await;
    assert_eq!(found.len(), 1, "no duplicate: {found:?}");
    assert_eq!(found[0].config.id.as_str(), "docker-desktop");
    assert_eq!(found[0].initial_state, None);
    assert!(matches!(
        f.probe(&found[0].config).await,
        ProbeResult::Reachable
    ));
}

#[tokio::test]
async fn eng_143_no_install_lists_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let f = DockerFactory::for_desktop_tests(DesktopTestConfig::new(dir.path()));
    assert!(f.discover().await.is_empty());
    let c = DesktopTestConfig::new(dir.path());
    assert!(f.start_prompt(&cfg(&c.engine_pipe)).is_none());
    // `Docker Desktop.exe` missing is "not installed" even when the folder exists.
    let tree = Tree::new(true, "");
    std::fs::remove_file(tree.desktop()).unwrap();
    let f = DockerFactory::for_desktop_tests(tree.config());
    assert!(f.discover().await.is_empty());
}

#[tokio::test]
async fn eng_143_discovery_spawns_no_process() {
    let tree = Tree::new(true, "");
    let f = DockerFactory::for_desktop_tests(tree.config());
    for _ in 0..2 {
        assert_eq!(f.discover().await.len(), 1);
    }
    assert!(tree.docker_log().is_empty());
    assert!(tree.desktop_log().is_empty());
}

#[tokio::test]
async fn eng_144_start_runs_one_fixed_argv() {
    for mode in ["start=ok", "start=already"] {
        let tree = Tree::new(true, mode);
        let c = tree.config();
        let pipe = c.engine_pipe.clone();
        let f = DockerFactory::for_desktop_tests(c);
        f.discover().await;
        f.start(&cfg(&pipe)).await.unwrap();
        assert_eq!(
            tree.docker_log(),
            vec!["desktop|start|--timeout|120"],
            "{mode}"
        );
        assert!(tree.desktop_log().is_empty(), "no fallback on success");
    }
}

#[tokio::test]
async fn eng_144_missing_plugin_falls_back_to_exe_and_polls() {
    let tree = Tree::new(true, "start=unknown");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;

    // The pipe answers only after `Docker Desktop.exe` was launched.
    let log = sibling(&tree.desktop(), ".log");
    let server_pipe = pipe.clone();
    let late = tokio::spawn(async move {
        while !log.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        serve(server_pipe)
    });
    f.start(&cfg(&pipe)).await.unwrap();
    let _server = late.await.unwrap();
    assert_eq!(tree.docker_log(), vec!["desktop|start|--timeout|120"]);
    assert_eq!(
        tree.desktop_log(),
        vec![String::new()],
        "launched once, no args"
    );
}

#[tokio::test]
async fn eng_144_missing_docker_exe_falls_back_too() {
    let tree = Tree::new(false, "");
    let mut c = tree.config();
    c.poll_timeout = Duration::from_millis(400);
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    let err = f.start(&cfg(&pipe)).await.unwrap_err();
    assert_eq!(tree.desktop_log().len(), 1);
    assert!(
        matches!(err, EngineError::Unreachable { hint: Some(_), .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn eng_144_never_started_by_connect_probe_or_retry() {
    let tree = Tree::new(true, "start=ok\nstatus_out={\"Status\":\"stopped\"}");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    let found = f.discover().await;
    let config = &found[0].config;
    for _ in 0..3 {
        assert!(f.connect(config).await.is_err());
        assert!(matches!(f.probe(config).await, ProbeResult::Stopped));
        assert!(f.stopped_after(config, &unreachable_error()).await);
    }
    let _ = f.start_prompt(&cfg(&pipe));
    let log = tree.docker_log();
    assert!(!log.is_empty(), "stopped_after ran status");
    assert!(
        log.iter().all(|l| l == "desktop|status|--format|json"),
        "{log:?}"
    );
    assert!(tree.desktop_log().is_empty());
}

#[tokio::test]
async fn eng_144_start_refuses_other_engines() {
    let tree = Tree::new(true, "");
    let f = DockerFactory::for_desktop_tests(tree.config());
    f.discover().await;
    let mut other = cfg(r"\\.\pipe\someone_else");
    other.id = EngineId::new("x");
    assert!(f.start(&other).await.is_err());
    assert!(tree.docker_log().is_empty());
}

#[tokio::test]
async fn eng_145_status_json_parsing() {
    let cases: [(&str, bool); 7] = [
        (
            "status_out={\"PauseType\":\"\",\"SessionID\":\"x\",\"Status\":\"running\"}",
            false,
        ),
        ("status_out=", true),
        ("status_out=garbage", true),
        ("status_out={\"Status\":\"stopped\"}", true),
        (
            "status_out={\"PauseType\":\"manual\",\"Status\":\"running\"}",
            false,
        ),
        ("status_out={\"Status\":\"running\"}\nstatus_code=1", true),
        (
            "status_out={\"Status\":\"running\"}\nstatus_sleep_ms=3000",
            true,
        ),
    ];
    for (mode, stopped) in cases {
        let tree = Tree::new(true, mode);
        let mut c = tree.config();
        c.status_timeout = Duration::from_millis(800);
        let pipe = c.engine_pipe.clone();
        let f = DockerFactory::for_desktop_tests(c);
        f.discover().await;
        let t = Instant::now();
        let got = f.stopped_after(&cfg(&pipe), &unreachable_error()).await;
        assert_eq!(got, stopped, "{mode}");
        assert!(t.elapsed() < Duration::from_secs(3), "bounded: {mode}");
    }
}

#[tokio::test]
async fn eng_145_status_edge_cases() {
    // Missing docker.exe counts as not running.
    let tree = Tree::new(false, "");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    assert!(f.stopped_after(&cfg(&pipe), &unreachable_error()).await);

    // Only daemon-down errors and only the Docker Desktop engine are considered.
    let tree = Tree::new(true, "status_out={\"Status\":\"stopped\"}");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    let engine = cfg(&pipe);
    let api = |status| EngineError::Api {
        status,
        message: "daemon not running".into(),
    };
    assert!(f.stopped_after(&engine, &api(503)).await);
    assert!(f.stopped_after(&engine, &api(500)).await);
    assert!(!f.stopped_after(&engine, &api(404)).await);
    assert!(
        !f.stopped_after(&engine, &EngineError::Timeout(Duration::from_secs(1)))
            .await
    );
    let mut other = cfg(r"\\.\pipe\someone_else");
    other.id = EngineId::new("x");
    assert!(!f.stopped_after(&other, &unreachable_error()).await);
    assert_eq!(
        tree.docker_log().len(),
        2,
        "status ran only for the two daemon-down errors"
    );
}

#[tokio::test]
async fn eng_145_resource_saver_is_not_stopped() {
    let tree = Tree::new(
        true,
        "status_out={\"PauseType\":\"resource-saver\",\"Status\":\"paused\"}",
    );
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    assert!(!f.stopped_after(&cfg(&pipe), &unreachable_error()).await);
}

#[tokio::test]
async fn eng_145_probe_never_runs_status() {
    let tree = Tree::new(true, "status_out={\"Status\":\"stopped\"}");
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    for _ in 0..3 {
        assert!(matches!(f.probe(&cfg(&pipe)).await, ProbeResult::Stopped));
    }
    {
        let _server = serve(pipe.clone());
        assert!(matches!(f.probe(&cfg(&pipe)).await, ProbeResult::Reachable));
    }
    assert!(tree.docker_log().is_empty());
}

#[tokio::test]
async fn eng_147_start_failure_is_failed_with_hint() {
    let tree = Tree::new(
        true,
        "start=fail\nmessage=licence not accepted\\nsecond line",
    );
    let c = tree.config();
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    let err = f.start(&cfg(&pipe)).await.unwrap_err();
    let EngineError::Unreachable { reason, hint } = &err else {
        panic!("{err:?}");
    };
    assert!(reason.contains("licence not accepted"), "{reason}");
    assert!(!reason.contains("second line"));
    assert_eq!(
        hint.as_deref(),
        Some(
            "Finish any prompt in the Docker Desktop window (licence, sign-in or update), then try again."
        )
    );
    assert!(
        tree.desktop_log().is_empty(),
        "a real failure is not retried via the exe"
    );
}

#[tokio::test]
async fn eng_147_start_timeout_hint() {
    let tree = Tree::new(true, "start=slow\nsleep_ms=20000");
    let mut c = tree.config();
    c.start_timeout = Duration::from_millis(400);
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    let t = Instant::now();
    let err = f.start(&cfg(&pipe)).await.unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(
        matches!(&err, EngineError::Unreachable { hint: Some(h), .. } if h.contains("Docker Desktop window")),
        "{err:?}"
    );

    // The fallback poll running out reads the same.
    let tree = Tree::new(true, "start=unknown");
    let mut c = tree.config();
    c.poll_timeout = Duration::from_millis(300);
    let pipe = c.engine_pipe.clone();
    let f = DockerFactory::for_desktop_tests(c);
    f.discover().await;
    let err = f.start(&cfg(&pipe)).await.unwrap_err();
    assert!(
        matches!(&err, EngineError::Unreachable { hint: Some(h), .. } if h.contains("Docker Desktop window")),
        "{err:?}"
    );
}

#[test]
fn eng_160_argv_only_and_registry_paths() {
    // The only path source is the install location; nothing resolves `docker` through PATH.
    let src = include_str!("../src/desktop/run.rs");
    assert!(src.contains("install.docker_exe()"));
    assert!(!src.contains("Command::new(\"docker"));
    assert!(!src.contains("\"cmd\"") && !src.contains("powershell"));
}
