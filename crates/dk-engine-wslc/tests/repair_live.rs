//! T14/T15 normal-user acceptance; never creates sessions, elevates, prunes or removes images.
//! Run exactly (outer timeout >= 240 seconds):
//! `pwsh -NoProfile scripts/dev.ps1 test -p dk-engine-wslc --test repair_live -- --ignored --exact eng_127_128_normal_user_hello_world_acceptance --nocapture`
//! Each mutation has a unique name. Run is never retried, including after timeout.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dk_core::{
    ContainerDetails, ContainerQuery, ContainerState, Engine, EngineConfig, EngineEndpoint,
    EngineError, EngineFactory, EngineId, EngineOrigin, LogOpts, RemoveContainerOpts, RunSpec,
    WslcTransportPref,
};
use dk_engine_wslc::{WslcFactory, com, version};
use futures::{FutureExt, StreamExt};

const CALL: Duration = Duration::from_secs(15);

fn security_rejection(err: &EngineError) -> bool {
    // Live COM currently maps ERROR_ELEVATION_REQUIRED to Api 500. Admit only that
    // exact HRESULT, not arbitrary 500s; this is evidence assertion, not routing policy.
    matches!(err, EngineError::Api { status: 403, .. })
        || matches!(err, EngineError::Api { status: 500, message } if message.contains("0x800702E4"))
        || matches!(err, EngineError::Unreachable { hint: Some(hint), .. } if hint.to_lowercase().contains("elevat") || hint.to_lowercase().contains("policy") || hint.to_lowercase().contains("permission"))
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(CALL, future)
        .await
        .expect("bounded WSLC call")
}

fn config(session: Option<&str>, transport: WslcTransportPref) -> EngineConfig {
    EngineConfig {
        id: EngineId::new(format!(
            "repair-{transport:?}-{}",
            session.unwrap_or("default")
        )),
        name: "WSLC acceptance".into(),
        endpoint: EngineEndpoint::Wslc {
            session: session.map(str::to_owned),
            transport,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    }
}

async fn connect(session: Option<&str>, pref: WslcTransportPref) -> Arc<dyn Engine> {
    bounded(WslcFactory::new().connect(&config(session, pref)))
        .await
        .expect("factory connect")
}

fn hello(details: &ContainerDetails, id: &str, name: &str) {
    assert_eq!(
        details.summary.id, id,
        "returned ID, never name/prefix rebinding"
    );
    assert_eq!(details.summary.name, name);
    assert_eq!(details.summary.state, ContainerState::Exited);
    assert_eq!(details.summary.exit_code, Some(0));
    assert_eq!(details.cmd, ["/hello"]);
}

async fn logs(engine: &dyn Engine, id: &str) {
    let text = bounded(async {
        let mut stream = engine.logs(
            id,
            LogOpts {
                follow: false,
                timestamps: false,
                ..Default::default()
            },
        );
        let mut bytes = Vec::new();
        while let Some(item) = stream.next().await {
            bytes.extend_from_slice(&item.expect("hello logs").bytes);
            assert!(bytes.len() < 64 * 1024, "bounded hello output");
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
    .await;
    assert!(text.contains("Hello from Docker"), "hello output missing");
    eprintln!(
        "logs transport={:?} id={id}: Hello from Docker",
        bounded(engine.info()).await.expect("info").transport
    );
}

async fn verify_hello(native: &com::WslcComEngine, cli: &dyn Engine, id: &str, name: &str) {
    assert_eq!(id.len(), 64);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let d = bounded(native.inspect_container(id))
            .await
            .expect("COM returned-ID inspect");
        if d.summary.state == ContainerState::Exited {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "hello exit deadline"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let a = bounded(native.inspect_container(id))
        .await
        .expect("COM inspect");
    let b = bounded(cli.inspect_container(id))
        .await
        .expect("CLI inspect");
    hello(&a, id, name);
    hello(&b, id, name);
    assert_eq!(
        a.raw,
        bounded(native.inspect_container_json(id))
            .await
            .expect("original COM raw")
    );
    assert_eq!(a.summary.ports, b.summary.ports);
    assert_eq!(a.port_bindings, b.port_bindings);
    logs(native, id).await;
    logs(cli, id).await;
}

fn raw_run_id(stdout: &[u8]) -> String {
    let text = std::str::from_utf8(stdout).expect("raw Run stdout UTF-8");
    // Pull notices can precede the ID. Never infer an ID from a name or truncated output.
    let ids: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "raw Run must return exactly one full ID; do not retry"
    );
    ids[0].to_owned()
}

#[test]
fn img_005_raw_run_id_requires_one_full_id() {
    let id = "a".repeat(64);
    assert_eq!(
        raw_run_id(format!("pull notice\r\n{id}\r\n").as_bytes()),
        id
    );
    for invalid in [
        "created but ID lost".into(),
        "a".repeat(12),
        format!("{id}\n{id}\n"),
    ] {
        assert!(std::panic::catch_unwind(|| raw_run_id(invalid.as_bytes())).is_err());
    }
}

/// Armed before Run. A panic/error still reaches cleanup via catch_unwind; Drop is a backup.
/// Cleanup only removes an exact unique name + full ID + expected image not in the baseline.
struct OwnedProbes {
    cli: Arc<dyn Engine>,
    names: Vec<String>,
    baseline: BTreeSet<String>,
    armed: bool,
}

impl OwnedProbes {
    async fn cleanup(&self) -> Result<(), String> {
        // Also reconcile an ambiguous Run by name, without ever rerunning it. Repeated reads
        // allow a child/RPC already in flight to settle. Only one remove per identified ID.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        let mut removed = BTreeSet::new();
        loop {
            let rows =
                tokio::time::timeout(CALL, self.cli.list_containers(ContainerQuery::default()))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
            for row in rows {
                if !self.names.contains(&row.name)
                    || self.baseline.contains(&row.id)
                    || removed.contains(&row.id)
                {
                    continue;
                }
                let d = tokio::time::timeout(CALL, self.cli.inspect_container(&row.id))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                if d.summary.id != row.id
                    || d.summary.name != row.name
                    || !d.summary.image.starts_with("hello-world")
                {
                    return Err(format!(
                        "ownership not proved; refusing cleanup of {}",
                        row.id
                    ));
                }
                tokio::time::timeout(
                    CALL,
                    self.cli.remove_container(
                        &row.id,
                        RemoveContainerOpts {
                            force: true,
                            ..Default::default()
                        },
                    ),
                )
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                eprintln!("cleanup own container name={} id={}", row.name, row.id);
                removed.insert(row.id);
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let rows = tokio::time::timeout(CALL, self.cli.list_containers(ContainerQuery::default()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if rows.iter().any(|r| self.names.contains(&r.name)) {
            return Err("own resource remains".into());
        }
        let remaining: BTreeSet<_> = rows.into_iter().map(|r| r.id).collect();
        if !self.baseline.is_subset(&remaining) {
            return Err("unrelated baseline container disappeared".into());
        }
        eprintln!(
            "cleanup verified: zero own containers; {} unrelated baseline containers retained; image left intact",
            self.baseline.len()
        );
        Ok(())
    }
}

impl Drop for OwnedProbes {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let guard = Self {
            cli: self.cli.clone(),
            names: self.names.clone(),
            baseline: self.baseline.clone(),
            armed: false,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            let result = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(60), guard.cleanup())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
            });
            let _ = tx.send(result);
        });
        match rx.recv_timeout(Duration::from_secs(65)) {
            Ok(Ok(())) => {}
            result => eprintln!(
                "CLEANUP FAILURE — inspect only these unique names manually: {:?}: {result:?}",
                self.names
            ),
        }
    }
}

#[tokio::test]
#[ignore = "mutates only own hello-world containers; needs normal-user WSL service 3.0.1.0"]
async fn eng_127_128_normal_user_hello_world_acceptance() {
    let token = bounded(tokio::process::Command::new("pwsh")
        .args(["-NoProfile", "-Command", "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"])
        .kill_on_drop(true).output()).await.expect("normal-user token check");
    assert!(token.status.success());
    assert_eq!(
        String::from_utf8_lossy(&token.stdout).trim(),
        "False",
        "acceptance must not run elevated"
    );
    eprintln!("process token: non-elevated (Administrator role False)");
    assert!(
        dk_engine_wslc::init_process_com_security(),
        "process COM security + Winsock"
    );
    assert!(
        std::env::var_os("DOCKERING_WSLC_EXE").is_none(),
        "must use installed CLI, not override"
    );
    let v = version::detect().expect("WSL service version");
    assert_eq!(v, version::WslVersion::new(3, 0, 1, 0));
    let module = com::select(&v).expect("exact trusted ABI");
    let (native, check) = bounded(com::WslcComEngine::connect(
        EngineId::new("acceptance-native"),
        None,
        v,
        module,
    ))
    .await
    .expect("COM self-check");
    let session = check.default_session.expect("resolved caller default");
    eprintln!(
        "acceptance WSL={v} session={session:?} self-check sessions={}",
        check.sessions
    );
    let cli = connect(Some(&session), WslcTransportPref::Cli).await;
    let baseline = bounded(cli.list_containers(ContainerQuery::default()))
        .await
        .expect("baseline")
        .into_iter()
        .map(|r| r.id)
        .collect();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let mut owned = OwnedProbes {
        cli: cli.clone(),
        names: (0..4)
            .map(|n| format!("dk-repair-{}-{nonce}-{n}", std::process::id()))
            .collect(),
        baseline,
        armed: true,
    };
    let result = std::panic::AssertUnwindSafe(async {
        // Explicit/default × Auto/COM/CLI; forced COM's rejected Run cannot create anything.
        for target in [None, Some(session.as_str())] {
            for pref in [WslcTransportPref::Auto, WslcTransportPref::Com, WslcTransportPref::Cli] {
                let e = connect(target, pref).await;
                let info = bounded(e.info()).await.expect("matrix info");
                let expected = if pref == WslcTransportPref::Cli { "cli" } else { "com" };
                assert_eq!(info.transport.as_deref(), Some(expected));
                if pref == WslcTransportPref::Auto { assert_eq!(info.transport_note, None); }
                if pref == WslcTransportPref::Com { assert_eq!(info.transport_note.as_deref(), Some("COM only — Run unavailable")); }
                bounded(e.ping()).await.expect("target health");
                if pref == WslcTransportPref::Com {
                    let r = bounded(e.run_image(RunSpec { image: "hello-world:latest".into(), name: Some(owned.names[0].clone()), ..Default::default() })).await;
                    assert!(matches!(r, Err(EngineError::Api { status: 501, .. })), "strict COM: {r:?}");
                }
                eprintln!("preference matrix target={target:?} pref={pref:?} primary={expected} PASS");
            }
        }
        let (sessions, default) = bounded(com::list_sessions(v)).await.expect("session inventory");
        assert_eq!(default.as_deref(), Some(session.as_str()));
        for (n, target, pref) in [(0, None, WslcTransportPref::Auto), (1, Some(session.as_str()), WslcTransportPref::Auto), (2, Some(session.as_str()), WslcTransportPref::Cli)] {
            let e = connect(target, pref).await;
            let id = tokio::time::timeout(Duration::from_secs(45), e.run_image(RunSpec { image: "hello-world:latest".into(), name: Some(owned.names[n].clone()), ..Default::default() })).await.expect("Run timed out; do not retry").expect("Run once");
            verify_hello(&native, cli.as_ref(), &id, &owned.names[n]).await;
            if pref == WslcTransportPref::Auto {
                let info = bounded(e.info()).await.expect("info after Run");
                assert_eq!(info.transport.as_deref(), Some("com"));
                assert_eq!(info.transport_note, None);
            }
            eprintln!("Run target={target:?} pref={pref:?} returnedID={id} both inspect: Exited/0 /hello PASS");
            // Other existing sessions are read-only. A security rejection is acceptable;
            // never elevate/create a session or inspect/remove an administrator resource.
            for other in sessions.iter().filter_map(|s| s.name.as_deref()).filter(|s| *s != session) {
                for isolation_pref in [WslcTransportPref::Auto, WslcTransportPref::Com, WslcTransportPref::Cli] {
                match bounded(WslcFactory::new().connect(&config(Some(other), isolation_pref))).await {
                    Ok(other_engine) => match bounded(other_engine.list_containers(ContainerQuery::default())).await {
                        Ok(rows) => { assert!(!rows.iter().any(|r| r.id == id || r.name == owned.names[n])); eprintln!("isolation other={other:?}: own probe invisible PASS"); },
                        Err(err) => { assert!(security_rejection(&err), "unexpected read rejection: {err:?}"); eprintln!("isolation other={other:?}: read rejected, no elevation: {err}"); },
                    },
                    Err(err) => { assert!(security_rejection(&err), "other={other:?} unexpected rejection: {err:?}"); eprintln!("isolation other={other:?}: connect rejected, no elevation: {err}"); },
                }
                }
            }
        }
        // Independent raw CLI acceptance: bypass Factory/RunSpec, use argv only and pin
        // the already resolved owned session. The fourth name was armed before any spawn.
        // Timeout/error/lost-ID panics enter the same name-based reconciliation cleanup;
        // the mutating command is never repeated.
        let exe = dk_engine_wslc::cli::wslc_exe().expect("installed wslc.exe");
        let output = tokio::time::timeout(Duration::from_secs(45),
            tokio::process::Command::new(exe)
                .args(["--session", session.as_str(), "container", "run", "--detach", "--name", owned.names[3].as_str(), "hello-world:latest"])
                .kill_on_drop(true).output()).await
            .expect("raw Run timed out; do not retry").expect("raw Run spawn/output; do not retry");
        assert!(output.status.success(), "raw Run exit {:?}; inspect only own name before manual retry", output.status.code());
        let id = raw_run_id(&output.stdout);
        verify_hello(&native, cli.as_ref(), &id, &owned.names[3]).await;
        eprintln!("raw CLI Run explicit session={session:?} returnedID={id} both inspect: Exited/0 /hello and both logs PASS");
        assert!(bounded(cli.list_images()).await.expect("retained image").iter().any(|i| i.repo_tags.iter().any(|t| t == "hello-world:latest")));
    }).catch_unwind().await;
    let cleanup = tokio::time::timeout(Duration::from_secs(60), owned.cleanup())
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    if cleanup.is_ok() {
        owned.armed = false;
    }
    cleanup.expect("guaranteed own-resource cleanup failed; Drop retries cleanup only, never Run");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
