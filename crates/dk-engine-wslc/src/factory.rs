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
        if s.is_empty()
            || Some(s.as_str()) == default_session
            || is_cli_default_session(s, default_session)
        {
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

/// Without a resolved default name (CLI listing), treat the non-elevated CLI default
/// `wslc-cli-<user>` as the default session (spec 20 §5.2 step 3).
fn is_cli_default_session(s: &str, default_session: Option<&str>) -> bool {
    if default_session.is_some() {
        return false;
    }
    let user = std::env::var("USERNAME").unwrap_or_default();
    !user.is_empty() && s.eq_ignore_ascii_case(&format!("wslc-cli-{user}"))
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

/// Whether a COM connect error must NOT fall back to the CLI (policy, spec 20 §5.2 step 2).
pub fn is_final_com_error(e: &EngineError) -> bool {
    e.hint() == Some(crate::com::ffi::POLICY_HINT)
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
        match crate::com::list_sessions(v).await {
            Ok((sessions, default)) => {
                // ListSessions returns every session the service tracks; keep the caller's.
                let me = crate::com::win32::current_user_sid();
                let names = sessions
                    .into_iter()
                    .filter(|s| me.is_none() || s.sid.is_none() || s.sid == me)
                    .filter_map(|s| s.name)
                    .collect();
                return (names, default);
            }
            Err(e) => tracing::debug!(%e, "COM ListSessions failed; trying the CLI"),
        }
    }
    match crate::cli::list_sessions().await {
        Ok(names) => (names, None),
        Err(e) => {
            tracing::debug!(%e, "wslc session list failed");
            (Vec::new(), None)
        }
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
            match crate::com::WslcComEngine::connect(id.clone(), session.clone(), ver, module).await
            {
                Ok((engine, check)) => {
                    tracing::info!(
                        engine = %id,
                        abi = module.name(),
                        sessions = check.sessions,
                        default_session = ?check.default_session,
                        "WSLC COM transport connected"
                    );
                    Ok(Arc::new(engine))
                }
                Err(e) if com_only || is_final_com_error(&e) => Err(e),
                // A missing session is not a transport problem; the CLI would fail the same way.
                Err(e @ EngineError::NotFound { .. }) => Err(e),
                Err(e) => {
                    tracing::warn!(engine = %id, error = %e, "WSLC COM unavailable; falling back to CLI");
                    cli_engine(
                        id,
                        session,
                        version_str,
                        Some(NOTE_SELF_CHECK_FAILED.into()),
                    )
                    .await
                }
            }
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
    Arc::new(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::abi::AbiModule;

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

    #[test]
    fn policy_error_is_final() {
        let policy =
            crate::com::ffi::map_hresult(crate::com::ffi::hr::WSLC_E_CONTAINER_DISABLED, "", None);
        assert!(is_final_com_error(&policy));
        assert!(!is_final_com_error(&EngineError::protocol("self-check")));
        assert!(!is_final_com_error(&EngineError::unreachable("x")));
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
