//! `WslcFactory` (ENG-008, ENG-013, ENG-020, ENG-109, ENG-110; spec 20 §5.2–5.3).
//!
//! - **Discovery** without COM: WSL version from `wslservice.exe` ≥ 2.9.3 and (CLSID registered
//!   or `wslc.exe` present) → one `wslc-default` engine. Extra sessions (ENG-109) come from COM
//!   `ListSessions` when a verified ABI module exists, else `wslc system session list`.
//! - **Connect** (`Auto`): select the ABI module from the version; COM + self-check when one
//!   matches; otherwise, or on any COM failure, the CLI with an ENG-110 note.
//!   `WSLC_E_CONTAINER_DISABLED` is final (policy) and never falls back.
//! - **Probe** (ENG-020): no COM; version present + `wslcsession.exe` running.

use std::sync::Arc;

use async_trait::async_trait;
use dk_core::engine::preference;
use dk_core::{
    ConfigField, ConfigFieldKind, DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema,
    EngineEndpoint, EngineError, EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult,
    ProbeResult, WslcTransportPref,
};

use crate::version::{self, WslVersion};

/// Engine id of the caller's default WSLC session.
pub const DEFAULT_ENGINE_ID: &str = "wslc-default";
/// User-visible name of the default engine.
pub const DEFAULT_ENGINE_NAME: &str = "WSL containers";
/// ENG-111 / spec 20 §5.2: shown when WSL is present but too old for WSLC.
pub const UPDATE_HINT: &str = "WSL containers available with `wsl --update`";
/// ENG-110 note when the COM self-check (or COM activation) failed.
pub const NOTE_SELF_CHECK_FAILED: &str = "COM self-check failed — using CLI";

/// ENG-110 note for a WSL version without a verified ABI module.
pub fn note_unverified(v: &WslVersion) -> String {
    format!("WSL {v} not yet verified — using CLI")
}

pub struct WslcFactory {
    _private: (),
}

impl WslcFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Hint for the first-run screen (ENG-111): WSL is installed but older than 2.9.3.
    pub fn absence_hint() -> Option<String> {
        absence_hint_for(version::detect())
    }
}

impl Default for WslcFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure part of [`WslcFactory::absence_hint`].
pub fn absence_hint_for(v: Option<WslVersion>) -> Option<String> {
    v.filter(|v| !v.supports_wslc())
        .map(|_| UPDATE_HINT.to_owned())
}

/// Whether WSLC is usable on this machine per spec 20 §5.2 step 1 (pure).
pub fn wslc_present(v: Option<WslVersion>, clsid_registered: bool, wslc_exe: bool) -> bool {
    v.is_some_and(|v| v.supports_wslc()) && (clsid_registered || wslc_exe)
}

/// `wslc-<slug>` engine id for a non-default session.
pub fn session_engine_id(session: &str) -> EngineId {
    let mut out = String::with_capacity(session.len());
    let mut dash = false;
    for c in session.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    EngineId::new(format!("wslc-{out}"))
}

fn wslc_config(id: EngineId, name: String, session: Option<String>) -> EngineConfig {
    EngineConfig {
        id,
        name,
        endpoint: EngineEndpoint::Wslc {
            session,
            transport: WslcTransportPref::Auto,
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
}

/// Builds the discovery list (pure, unit-tested). `sessions` = every session display name
/// visible to the caller; `default_session` = the caller's default (skipped as a duplicate).
pub fn discovered_engines(
    present: bool,
    sessions: &[String],
    default_session: Option<&str>,
) -> Vec<DiscoveredEngine> {
    if !present {
        return Vec::new();
    }
    let mut out = vec![DiscoveredEngine::new(
        wslc_config(
            EngineId::new(DEFAULT_ENGINE_ID),
            DEFAULT_ENGINE_NAME.to_owned(),
            None,
        ),
        preference::WSLC,
    )];
    let mut seen = vec![EngineId::new(DEFAULT_ENGINE_ID)];
    for s in sessions {
        if s.is_empty() || Some(s.as_str()) == default_session {
            continue;
        }
        let id = session_engine_id(s);
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        let mut d = DiscoveredEngine::new(
            wslc_config(id, format!("{DEFAULT_ENGINE_NAME} ({s})"), Some(s.clone())),
            preference::WSLC,
        );
        d.show_only_when_all = true;
        out.push(d);
    }
    out
}

/// ENG-135: SID strings must prove ownership; missing/malformed evidence is not permission.
#[cfg(any(windows, test))]
pub(crate) fn owned_sid(owner: Option<&str>, caller: Option<&str>) -> bool {
    fn valid(s: &str) -> bool {
        let parts: Vec<_> = s.split('-').collect();
        parts.len() >= 4
            && parts.len() <= 18
            && parts[0] == "S"
            && parts[1] == "1"
            && parts[2].parse::<u64>().is_ok_and(|n| n < (1u64 << 48))
            && parts[3..].iter().all(|p| {
                !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok()
            })
    }
    matches!((owner, caller), (Some(a), Some(b)) if valid(a) && valid(b) && a == b)
}

/// What `connect` will do, decided from inputs only (pure, unit-tested).
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectPlan {
    /// Use COM through `module`; on failure fall back to the CLI unless `com_only`.
    Com {
        module: crate::com::abi::AbiModule,
        com_only: bool,
    },
    /// Use the CLI with this ENG-110 note.
    Cli { note: Option<String> },
    /// Refuse (e.g. COM forced but no verified module).
    Fail(EngineError),
}

pub fn plan_connect(pref: WslcTransportPref, v: Option<WslVersion>) -> ConnectPlan {
    let module = v.as_ref().and_then(crate::com::abi::select);
    match (pref, v, module) {
        (WslcTransportPref::Cli, _, _) => ConnectPlan::Cli { note: None },
        (WslcTransportPref::Com, _, Some(m)) => ConnectPlan::Com {
            module: m,
            com_only: true,
        },
        (WslcTransportPref::Com, Some(v), None) => {
            ConnectPlan::Fail(EngineError::unreachable_with_hint(
                format!("WSL {v} has no verified COM ABI module"),
                "Set the WSLC transport to Auto or CLI.",
            ))
        }
        (WslcTransportPref::Com, None, None) => ConnectPlan::Fail(
            EngineError::unreachable_with_hint("WSL is not installed", UPDATE_HINT),
        ),
        (WslcTransportPref::Auto, _, Some(m)) => ConnectPlan::Com {
            module: m,
            com_only: false,
        },
        (WslcTransportPref::Auto, Some(v), None) if !v.supports_wslc() => {
            ConnectPlan::Fail(EngineError::unreachable_with_hint(
                format!("WSL {v} does not support containers"),
                UPDATE_HINT,
            ))
        }
        (WslcTransportPref::Auto, Some(v), None) => ConnectPlan::Cli {
            note: Some(note_unverified(&v)),
        },
        // Version unreadable: the CLI may still work (it reports its own errors).
        (WslcTransportPref::Auto, None, None) => ConnectPlan::Cli {
            note: Some("WSL version unknown — using CLI".into()),
        },
    }
}

fn endpoint_parts(cfg: &EngineConfig) -> EngineResult<(Option<String>, WslcTransportPref)> {
    match &cfg.endpoint {
        EngineEndpoint::Wslc { session, transport } => {
            let session = session.clone().filter(|s| !s.trim().is_empty());
            if let Some(s) = &session {
                validate_session_name(s)?;
            }
            Ok((session, *transport))
        }
        other => Err(EngineError::Api {
            status: 400,
            message: format!("not a WSLC endpoint: {}", other.display()),
        }),
    }
}

/// Session display names are passed as COM strings / CLI argv; reject control characters
/// and leading dashes (NFR-022). WSLC caps names at 255 UTF-16 units.
pub fn validate_session_name(s: &str) -> EngineResult<()> {
    let ok = !s.is_empty()
        && s.encode_utf16().count() < 256
        && !s.starts_with('-')
        && !s.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(EngineError::Api {
            status: 400,
            message: format!("invalid WSLC session name: {s:?}"),
        })
    }
}

#[async_trait]
impl EngineFactory for WslcFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }

    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        endpoint.kind() == Some(EngineKind::Wslc)
    }

    async fn discover(&self) -> Vec<DiscoveredEngine> {
        #[cfg(windows)]
        {
            let v = version::detect();
            let present = wslc_present(
                v,
                version::wslc_registered(),
                crate::cli::wslc_exe().is_some(),
            );
            if !present {
                return Vec::new();
            }
            let (sessions, default) = discover_sessions(v).await;
            discovered_engines(true, &sessions, default.as_deref())
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    async fn connect(&self, cfg: &EngineConfig) -> EngineResult<Arc<dyn Engine>> {
        let (session, pref) = endpoint_parts(cfg)?;
        #[cfg(windows)]
        {
            connect_windows(cfg.id.clone(), session, pref).await
        }
        #[cfg(not(windows))]
        {
            let _ = (session, pref);
            Err(EngineError::unreachable_with_hint(
                "WSL containers are only available on Windows",
                "Use a Docker engine on this OS.",
            ))
        }
    }

    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        vec![EngineConfigSchema {
            kind: EngineKind::Wslc,
            label: "WSL containers".into(),
            fields: vec![
                ConfigField {
                    key: "session".into(),
                    label: "Session".into(),
                    kind: ConfigFieldKind::Text,
                    required: false,
                    placeholder: Some("Default session".into()),
                },
                ConfigField {
                    key: "transport".into(),
                    label: "Transport".into(),
                    kind: ConfigFieldKind::Choice {
                        options: vec!["Auto".into(), "COM".into(), "CLI".into()],
                    },
                    required: false,
                    placeholder: Some("Auto".into()),
                },
            ],
        }]
    }

    async fn probe(&self, cfg: &EngineConfig) -> ProbeResult {
        if endpoint_parts(cfg).is_err() {
            return ProbeResult::Unreachable(EngineError::Api {
                status: 400,
                message: "not a WSLC endpoint".into(),
            });
        }
        #[cfg(windows)]
        {
            let Some(v) = version::detect() else {
                return ProbeResult::Unreachable(EngineError::unreachable_with_hint(
                    "WSL is not installed",
                    UPDATE_HINT,
                ));
            };
            if !v.supports_wslc() {
                return ProbeResult::Unreachable(EngineError::unreachable_with_hint(
                    format!("WSL {v} does not support containers"),
                    UPDATE_HINT,
                ));
            }
            if crate::com::win32::process_running("wslcsession.exe") {
                ProbeResult::Reachable
            } else {
                ProbeResult::Unreachable(EngineError::unreachable(
                    "WSL containers session is not running",
                ))
            }
        }
        #[cfg(not(windows))]
        {
            ProbeResult::Unreachable(EngineError::unreachable(
                "WSL containers are only available on Windows",
            ))
        }
    }
}

#[cfg(windows)]
async fn discover_sessions(v: Option<WslVersion>) -> (Vec<String>, Option<String>) {
    if let Some(v) = v
        && crate::com::abi::select(&v).is_some()
    {
        return owned_discovery(
            crate::com::list_sessions(v),
            crate::com::win32::current_user_sid(),
        )
        .await;
    }
    // CLI tables contain no owner SID. Do not enumerate at all, including on policy failure.
    (Vec::new(), None)
}

#[cfg(windows)]
async fn owned_discovery(
    probe: impl Future<Output = EngineResult<(Vec<crate::com::engine::SessionEntry>, Option<String>)>>,
    me: Option<String>,
) -> (Vec<String>, Option<String>) {
    match probe.await {
        Ok((sessions, default)) => {
            // ListSessions returns every session the service tracks; keep the caller's.
            let names = sessions
                .into_iter()
                .filter(|s| owned_sid(s.sid.as_deref(), me.as_deref()))
                .filter_map(|s| s.name)
                .collect();
            return (names, default);
        }
        Err(e) => tracing::debug!(%e, "COM ListSessions failed; extra sessions excluded"),
    }
    // CLI tables contain no owner SID. Do not enumerate at all, including on policy failure.
    (Vec::new(), None)
}

#[cfg(windows)]
async fn connected_or_cli(
    connected: EngineResult<(crate::com::WslcComEngine, crate::com::engine::SelfCheck)>,
    failure: Option<crate::com::dispatch::TransportFailure>,
    classification: Option<crate::com::dispatch::ConnectFailure>,
    com_only: bool,
    version: Option<String>,
    cli: impl FnOnce() -> futures::future::BoxFuture<'static, EngineResult<Arc<dyn Engine>>>,
) -> EngineResult<Arc<dyn Engine>> {
    match connected {
        Ok((engine, _)) => Ok(Arc::new(crate::router::WslcEngine::native(
            engine, com_only, version,
        ))),
        Err(e)
            if com_only || classification == Some(crate::com::dispatch::ConnectFailure::Policy) =>
        {
            Err(e)
        }
        Err(e @ EngineError::NotFound { .. }) => Err(e),
        Err(e)
            if classification == Some(crate::com::dispatch::ConnectFailure::Final)
                || (failure.is_none()
                    && classification
                        != Some(crate::com::dispatch::ConnectFailure::SelfCheckMismatch)) =>
        {
            Err(e)
        }
        Err(native) => cli()
            .await
            .map_err(|cli| crate::router::fallback_error(&native, cli)),
    }
}

#[cfg(windows)]
async fn connect_windows(
    id: EngineId,
    session: Option<String>,
    pref: WslcTransportPref,
) -> EngineResult<Arc<dyn Engine>> {
    let v = version::detect();
    let version_str = v.map(|v| v.to_string());
    match plan_connect(pref, v) {
        ConnectPlan::Fail(e) => Err(e),
        ConnectPlan::Cli { note } => cli_engine(id, session, version_str, note).await,
        ConnectPlan::Com { module, com_only } => {
            let Some(ver) = v else {
                return Err(EngineError::protocol("COM plan without a version"));
            };
            let (connected, failure, classification) = crate::com::dispatch::capture_connect(
                crate::com::WslcComEngine::connect(id.clone(), session.clone(), ver, module),
            )
            .await;
            let cli_version = version_str.clone();
            connected_or_cli(
                connected,
                failure,
                classification,
                com_only,
                version_str,
                move || {
                    Box::pin(cli_engine(
                        id,
                        session,
                        cli_version,
                        Some(NOTE_SELF_CHECK_FAILED.into()),
                    ))
                },
            )
            .await
        }
    }
}

#[cfg(windows)]
async fn cli_engine(
    id: EngineId,
    session: Option<String>,
    wsl_version: Option<String>,
    note: Option<String>,
) -> EngineResult<Arc<dyn Engine>> {
    let e = crate::cli::WslcCliEngine::connect(id, session, wsl_version, note).await?;
    Ok(cli_into_engine(e))
}

/// The CLI engine implements `Engine` (engine-integrator). Kept in one place so the factory
/// compiles against the fixed CLI entry points.
#[cfg(windows)]
fn cli_into_engine(e: crate::cli::WslcCliEngine) -> Arc<dyn Engine> {
    Arc::new(crate::router::WslcEngine::cli(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::abi::AbiModule;

    #[cfg(windows)]
    #[test]
    fn explicit_target_contradiction_final_zero_cli_and_protected_calls() {
        use crate::com::{WslcComEngine, dispatch, fake};
        use std::sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        let state = Arc::new(Mutex::new(fake::FakeState {
            version: (3, 0, 1),
            sessions: vec![(7, "requested".into()), (8, "wrong".into())],
            default_session: "requested".into(),
            opened_session_override: Some("wrong".into()),
            ..Default::default()
        }));
        let (connected, fault, classification) =
            futures::executor::block_on(dispatch::capture_connect(
                WslcComEngine::connect_fake_pipeline(Some("requested".into()), state.clone()),
            ));
        assert!(connected.is_err());
        assert!(fault.is_none());
        assert_eq!(classification, Some(dispatch::ConnectFailure::Final));
        let preparations = Arc::new(AtomicUsize::new(0));
        let count = preparations.clone();
        let result = futures::executor::block_on(connected_or_cli(
            connected,
            fault,
            classification,
            false,
            None,
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(EngineError::protocol("CLI must never prepare")) })
            },
        ));
        assert!(result.is_err());
        assert_eq!(preparations.load(Ordering::SeqCst), 0);
        let calls = &state.lock().unwrap().calls;
        assert!(calls.iter().any(|c| c == "GetDisplayName"));
        assert!(!calls.iter().any(|c| matches!(
            c.as_str(),
            "BeginContainerOperation"
                | "OpenContainer"
                | "ListContainers"
                | "Stop"
                | "Exec"
                | "PullImage"
        )));
        assert!(
            !calls.iter().any(|c| c == "GetSessionId"),
            "contradiction is final before further identity calls"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn eng_135_factory_policy_zero_cli_preparation_and_default_visible() {
        use crate::com::ffi::{hr, map_hresult};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for strict in [false, true] {
            let attempts = Arc::new(AtomicUsize::new(0));
            let count = attempts.clone();
            let policy = map_hresult(hr::WSLC_E_CONTAINER_DISABLED, "policy", None);
            let result = connected_or_cli(
                Err(policy.clone()),
                None,
                Some(crate::com::dispatch::ConnectFailure::Policy),
                strict,
                None,
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Err(EngineError::protocol("must not prepare CLI")) })
                },
            )
            .await;
            assert!(matches!(result, Err(e) if e == policy));
            assert_eq!(attempts.load(Ordering::SeqCst), 0);
        }
        let sessions = owned_discovery(
            async { Err(map_hresult(hr::WSLC_E_CONTAINER_DISABLED, "policy", None)) },
            Some("S-1-5-21-1-2-3-1001".into()),
        )
        .await;
        assert!(sessions.0.is_empty());
        let found = discovered_engines(true, &sessions.0, sessions.1.as_deref());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].config.id.as_str(), DEFAULT_ENGINE_ID);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn eng_129_factory_only_typed_faults_or_selfcheck_mismatch_prepare_cli() {
        use crate::com::{
            dispatch::{DispatchPhase, TransportFailure},
            ffi::{hr, map_hresult},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for (error, fault, eligible) in [
            (map_hresult(hr::E_ACCESSDENIED, "access", None), None, false),
            (EngineError::Cancelled, None, false),
            (EngineError::protocol("malformed payload"), None, false),
            (EngineError::unreachable("generic"), None, false),
            (
                EngineError::Api {
                    status: 501,
                    message: "unverified".into(),
                },
                None,
                false,
            ),
            (
                EngineError::protocol("self-check: version mismatch"),
                None,
                true,
            ),
            (
                EngineError::unreachable("RPC"),
                Some(TransportFailure {
                    hresult: hr::RPC_E_DISCONNECTED,
                    phase: DispatchPhase::NotDispatched,
                }),
                true,
            ),
        ] {
            for strict in [false, true] {
                let attempts = Arc::new(AtomicUsize::new(0));
                let count = attempts.clone();
                let classification = if fault.is_none() && eligible {
                    Some(crate::com::dispatch::ConnectFailure::SelfCheckMismatch)
                } else {
                    None
                };
                let _ = connected_or_cli(
                    Err(error.clone()),
                    fault,
                    classification,
                    strict,
                    None,
                    move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Err(EngineError::protocol("CLI probe")) })
                    },
                )
                .await;
                assert_eq!(
                    attempts.load(Ordering::SeqCst),
                    usize::from(eligible && !strict)
                );
            }
        }
    }

    #[test]
    fn eng_135_missing_sid_excluded() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert!(owned_sid(Some(sid), Some(sid)));
        for bad in [
            None,
            Some(""),
            Some("S-1-"),
            Some("S-1-5-4294967296"),
            Some("S-1-5-+1"),
        ] {
            assert!(!owned_sid(bad, Some(sid)));
            assert!(!owned_sid(Some(sid), bad));
        }
        assert!(!owned_sid(Some("S-1-5-21-2"), Some(sid)));
    }

    fn v(s: &str) -> WslVersion {
        WslVersion::parse(s).expect("valid")
    }

    #[test]
    fn presence_rules() {
        assert!(wslc_present(Some(v("3.0.1")), true, false));
        assert!(wslc_present(Some(v("2.9.3")), false, true));
        assert!(!wslc_present(Some(v("2.9.2")), true, true));
        assert!(!wslc_present(Some(v("3.0.1")), false, false));
        assert!(!wslc_present(None, true, true));
    }

    #[test]
    fn absence_hint_only_for_old_wsl() {
        assert_eq!(absence_hint_for(Some(v("2.6.1"))), Some(UPDATE_HINT.into()));
        assert_eq!(absence_hint_for(Some(v("3.0.1"))), None);
        assert_eq!(absence_hint_for(None), None);
    }

    #[test]
    fn discovery_default_plus_hidden_sessions() {
        let sessions = vec![
            "wslc-cli-pavel".to_owned(),
            "wslc-cli-admin-pavel".to_owned(),
            "My Dev Session".to_owned(),
        ];
        let d = discovered_engines(true, &sessions, Some("wslc-cli-pavel"));
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].config.id.as_str(), DEFAULT_ENGINE_ID);
        assert_eq!(d[0].config.name, DEFAULT_ENGINE_NAME);
        assert_eq!(d[0].preference, preference::WSLC);
        assert!(!d[0].show_only_when_all);
        assert_eq!(
            d[0].config.endpoint,
            EngineEndpoint::Wslc {
                session: None,
                transport: WslcTransportPref::Auto
            }
        );
        assert_eq!(d[1].config.id.as_str(), "wslc-wslc-cli-admin-pavel");
        assert!(d[1].show_only_when_all);
        assert_eq!(d[2].config.id.as_str(), "wslc-my-dev-session");
        assert_eq!(
            d[2].config.endpoint,
            EngineEndpoint::Wslc {
                session: Some("My Dev Session".into()),
                transport: WslcTransportPref::Auto
            }
        );
        assert!(discovered_engines(false, &sessions, None).is_empty());
    }

    #[test]
    fn plan_matrix() {
        assert_eq!(
            plan_connect(WslcTransportPref::Auto, Some(v("3.0.1"))),
            ConnectPlan::Com {
                module: AbiModule::V3_0,
                com_only: false
            }
        );
        assert_eq!(
            plan_connect(WslcTransportPref::Auto, Some(v("3.1.0"))),
            ConnectPlan::Cli {
                note: Some("WSL 3.1.0 not yet verified — using CLI".into())
            }
        );
        assert_eq!(
            plan_connect(WslcTransportPref::Auto, Some(v("2.9.13"))),
            ConnectPlan::Cli {
                note: Some("WSL 2.9.13 not yet verified — using CLI".into())
            }
        );
        assert!(matches!(
            plan_connect(WslcTransportPref::Auto, Some(v("2.6.0"))),
            ConnectPlan::Fail(_)
        ));
        assert_eq!(
            plan_connect(WslcTransportPref::Cli, Some(v("3.0.1"))),
            ConnectPlan::Cli { note: None }
        );
        assert_eq!(
            plan_connect(WslcTransportPref::Com, Some(v("3.0.1"))),
            ConnectPlan::Com {
                module: AbiModule::V3_0,
                com_only: true
            }
        );
        assert!(matches!(
            plan_connect(WslcTransportPref::Com, Some(v("3.1.0"))),
            ConnectPlan::Fail(_)
        ));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn eng_129_factory_selfcheck_text_does_not_authorize_fallback() {
        let result = connected_or_cli(
            Err(EngineError::protocol("self-check: attacker text")),
            None,
            None,
            false,
            None,
            || panic!("text is not typed self-check evidence"),
        )
        .await;
        assert!(matches!(result, Err(EngineError::Protocol(_))));
    }

    #[test]
    fn session_ids_and_validation() {
        assert_eq!(session_engine_id("Foo Bar!").as_str(), "wslc-foo-bar");
        assert_eq!(session_engine_id("***").as_str(), "wslc-unnamed");
        assert!(validate_session_name("wslc-cli-pavel").is_ok());
        assert!(validate_session_name("-x").is_err());
        assert!(validate_session_name("a\nb").is_err());
        assert!(validate_session_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn schema_and_handles() {
        let f = WslcFactory::new();
        let s = f.config_schema();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].fields[0].key, "session");
        assert_eq!(
            s[0].fields[1].kind,
            ConfigFieldKind::Choice {
                options: vec!["Auto".into(), "COM".into(), "CLI".into()]
            }
        );
        assert!(f.handles(&EngineEndpoint::Wslc {
            session: None,
            transport: WslcTransportPref::Cli
        }));
        assert!(!f.handles(&EngineEndpoint::NamedPipe {
            path: r"\\.\pipe\docker_engine".into()
        }));
    }
}
