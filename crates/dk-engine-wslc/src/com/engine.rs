//! `WslcComEngine`: `dk_core::Engine` over the internal WSLC COM API (spec 20 §5.4).
//!
//! All vtable calls go through [`Com`], which only exists once a verified ABI module was
//! selected (or, in tests, for an in-process fake). Short calls run on the [`RpcPool`];
//! long-lived streams (events, logs, stats polling, exec TTY) each own a dedicated MTA thread.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use dk_core::stats::{RawStats, StatsNormalizer};
use dk_core::{
    Capabilities, ContainerAction, ContainerCounts, ContainerDetails, ContainerQuery,
    ContainerSummary, DiskUsage, Engine, EngineError, EngineEvent, EngineId, EngineInfo,
    EngineKind, EngineResult, EngineStream, EventFilter, ExecRequest, ImageDeleteItem,
    ImageDetails, ImageLayer, ImageSummary, LogChunk, LogOpts, LogStream, NetworkDetails,
    NetworkSummary, ProcessList, PruneReport, PullProgress, RegistryAuth, RemoveContainerOpts,
    ResourceKind, RunSpec, StatsSample, TerminalSession, VolumeDetails, VolumeSpec, VolumeSummary,
    docker_json, error_stream, validate,
};
use futures::channel::{mpsc, oneshot};
use serde_json::Value;
use time::OffsetDateTime;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::core::{BOOL, IUnknown, Interface, PCSTR, PCWSTR};

use super::abi::AbiModule;
use super::abi::v3_0 as abi;
use super::convert::{self, RawContainerEntry, RawImage, RawPort};
use super::ffi::{
    self, CoTaskMemArray, CoTaskMemStr, CoTaskMemWStr, Subject, blanket, cstring, fixed_cstr,
    fixed_wstr, free_field, hr, map_err, opt_pcstr, pcstr,
};
use super::pool::{DEFAULT_THREADS, RpcPool};
use super::stream::{Cancel, CancelOnDrop, LineSplitter, pump_handle, send_blocking};
use super::win32::{self, OwnedHandle};
use crate::version::WslVersion;

/// Capabilities of the COM transport (spec 20 §5.6).
pub const COM_CAPABILITIES: Capabilities = Capabilities::EVENTS
    .union(Capabilities::LOGS_FOLLOW)
    .union(Capabilities::NETWORK_MGMT)
    .union(Capabilities::EXEC_TTY)
    .union(Capabilities::EXEC_RESIZE)
    .union(Capabilities::PULL_PROGRESS);

/// Stats polling period on COM (spec 20 §5.4).
pub const STATS_INTERVAL: Duration = Duration::from_secs(2);

/// `EngineInfo.transport_note` of the COM transport (diagnostics, ENG-110).
pub const COM_TRANSPORT_NOTE: &str = "Run via COM is not verified for this WSL version";

/// `EngineError::Protocol` message for `WSLC_E_EVENTS_LOST` (spec 20 §5.4): the hub maps exactly
/// this message to `Feed::Lagged`, so consumers do a full refetch and keep the subscription.
// Mirrors `dk_core::EVENTS_LOST_MESSAGE` (added on the dk-core/dk-hub branch); switch to that
// constant once it is merged.
pub const EVENTS_LOST: &str = "events lost";

/// Message returned by `run_image` (CreateContainer is not verified live, see the note).
pub const RUN_NOT_VERIFIED: &str = "Run via COM is not verified for this WSL version";

// ───────────────────────────── COM pointer wrappers ─────────────────────────────

/// An interface pointer that may cross threads.
///
/// SAFETY of `Send`/`Sync`: every pointer stored here is either a proxy obtained on an MTA
/// thread for an out-of-proc server (free-threaded within the MTA) or an agile in-proc object
/// (`#[implement]` objects are agile by default). All calls happen on MTA threads (RPC pool /
/// dedicated stream threads), never on STA/UI threads.
#[derive(Clone)]
pub(crate) struct Agile<I: Interface>(pub(crate) I);
// SAFETY: see type docs: MTA proxies / agile objects, used only from MTA threads.
unsafe impl<I: Interface> Send for Agile<I> {}
// SAFETY: as above; COM interface methods take `&self` and are thread-safe in the MTA.
unsafe impl<I: Interface> Sync for Agile<I> {}

/// The verified-ABI COM entry points for one engine. Constructing it is the gate.
pub(crate) struct Com {
    manager: Agile<abi::IWSLCSessionManager>,
    /// `None` = the caller's default session (`OpenSessionByName(NULL)`).
    session_name: Option<String>,
    session: Mutex<Option<Agile<abi::IWSLCSession>>>,
}

fn com_err(e: windows::core::Error, subject: Option<Subject<'_>>) -> EngineError {
    map_err(&e, subject)
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Com {
    /// `CoCreateInstance(WSLCSessionManager, CLSCTX_LOCAL_SERVER)`. Call on an MTA thread.
    fn create_manager() -> windows::core::Result<abi::IWSLCSessionManager> {
        // SAFETY: standard activation of a registered local-server class; the returned proxy
        // is only used through the verified v3_0 vtable (caller checked `abi::select`).
        let mgr: abi::IWSLCSessionManager = unsafe {
            CoCreateInstance(&abi::CLSID_WSLC_SESSION_MANAGER, None, CLSCTX_LOCAL_SERVER)
        }?;
        blanket(&mgr);
        Ok(mgr)
    }

    fn open_session(&self) -> EngineResult<abi::IWSLCSession> {
        let name = self.session_name.as_deref().map(wide_z);
        let mut out: Option<abi::IWSLCSession> = None;
        // SAFETY: verified vtable slot 5; name is NUL-terminated UTF-16 or null (= default).
        let r = unsafe {
            self.manager.0.OpenSessionByName(
                name.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
                &mut out,
            )
        };
        let subject = Subject::new(
            ResourceKind::Session,
            self.session_name.as_deref().unwrap_or("default"),
        );
        ffi::check(r).map_err(|e| com_err(e, Some(subject)))?;
        let s = out.ok_or_else(|| EngineError::protocol("OpenSessionByName returned null"))?;
        blanket(&s);
        Ok(s)
    }

    /// Cached session proxy (opened on first use).
    fn session(&self) -> EngineResult<abi::IWSLCSession> {
        let mut g = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_ref() {
            return Ok(s.0.clone());
        }
        let s = self.open_session()?;
        *g = Some(Agile(s.clone()));
        Ok(s)
    }

    fn drop_session(&self) {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    /// Runs `f` with the session; on a disconnect HRESULT reopens the session once and
    /// retries (spec 20 §5.4 Lifetime). Second failure → `Unreachable`.
    fn with_session<T>(
        &self,
        mut f: impl FnMut(&abi::IWSLCSession) -> Result<T, windows::core::Error>,
        subject: Option<Subject<'_>>,
    ) -> EngineResult<T> {
        let s = self.session()?;
        match f(&s) {
            Ok(v) => Ok(v),
            Err(e) if ffi::is_disconnect(e.code().0) => {
                tracing::info!(
                    hr = format!("0x{:08X}", e.code().0 as u32),
                    "WSLC session disconnected; reopening once"
                );
                self.drop_session();
                let s = self.session()?;
                f(&s).map_err(|e| {
                    if ffi::is_disconnect(e.code().0) {
                        EngineError::unreachable(format!(
                            "WSL containers session disconnected (0x{:08X})",
                            e.code().0 as u32
                        ))
                    } else {
                        com_err(e, subject)
                    }
                })
            }
            Err(e) => Err(com_err(e, subject)),
        }
    }

    /// `BeginContainerOperation` token: keeps the VM alive for the duration of a container
    /// mutation (IDL comment on slot 44). Failure to obtain one is not fatal.
    fn begin_op(s: &abi::IWSLCSession) -> Option<IUnknown> {
        let mut tok: Option<IUnknown> = None;
        // SAFETY: verified vtable slot 44, out-param only.
        let r = unsafe { s.BeginContainerOperation(&mut tok) };
        if r.is_err() {
            tracing::debug!(
                hr = format!("0x{:08X}", r.0 as u32),
                "BeginContainerOperation failed"
            );
        }
        tok
    }

    fn open_container(
        s: &abi::IWSLCSession,
        id: &str,
    ) -> windows::core::Result<abi::IWSLCContainer> {
        let c = std::ffi::CString::new(id).map_err(|_| {
            windows::core::Error::from_hresult(windows::core::HRESULT(hr::E_INVALIDARG))
        })?;
        let mut out: Option<abi::IWSLCContainer> = None;
        // SAFETY: verified slot 18; `c` is NUL-terminated and outlives the call.
        ffi::check(unsafe { s.OpenContainer(PCSTR(c.as_ptr() as *const u8), &mut out) })?;
        let c = out.ok_or_else(|| {
            windows::core::Error::from_hresult(windows::core::HRESULT(hr::E_FAIL))
        })?;
        blanket(&c);
        Ok(c)
    }

    /// Opens container `id` (holding a container-operation token) and runs `f` on it.
    fn with_container<T>(
        &self,
        id: &str,
        mut f: impl FnMut(&abi::IWSLCContainer) -> Result<T, windows::core::Error>,
    ) -> EngineResult<T> {
        let subject = Subject::new(ResourceKind::Container, id);
        self.with_session(
            |s| {
                let _op = Self::begin_op(s);
                let c = Self::open_container(s, id)?;
                f(&c)
            },
            Some(subject),
        )
    }

    fn version(&self) -> EngineResult<abi::WSLCVersion> {
        let mut v = abi::WSLCVersion::default();
        // SAFETY: verified slot 0, plain out struct.
        ffi::check(unsafe { self.manager.0.GetVersion(&mut v) }).map_err(|e| com_err(e, None))?;
        Ok(v)
    }

    fn list_sessions(&self) -> EngineResult<Vec<(u32, Option<String>)>> {
        Ok(list_sessions_on(&self.manager.0)?
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect())
    }
}

/// One `ListSessions` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: u32,
    pub creator_pid: u32,
    /// `None` when the server returned an unterminated name (self-check failure).
    pub name: Option<String>,
    pub sid: Option<String>,
}

fn list_sessions_on(mgr: &abi::IWSLCSessionManager) -> EngineResult<Vec<SessionEntry>> {
    let mut arr = CoTaskMemArray::<abi::WSLCSessionListEntry>::new();
    let (p, n) = arr.out();
    // SAFETY: verified slot 3; MIDL `size_is(, *Count)` out array freed by `arr`.
    ffi::check(unsafe { mgr.ListSessions(p, n) }).map_err(|e| com_err(e, None))?;
    Ok(arr
        .as_slice()
        .iter()
        .map(|e| SessionEntry {
            id: e.SessionId,
            creator_pid: e.CreatorPid,
            name: fixed_wstr(&e.DisplayName),
            sid: fixed_wstr(&e.Sid),
        })
        .collect())
}

/// Lists WSLC sessions over COM through the ABI module selected for `v` (ENG-109), plus the
/// caller's default session name (`OpenSessionByName(NULL)` → `GetDisplayName`; opening a
/// session does not boot its VM). `None` module → `Err` (caller uses the CLI).
pub async fn list_sessions(v: WslVersion) -> EngineResult<(Vec<SessionEntry>, Option<String>)> {
    if crate::com::abi::select(&v).is_none() {
        return Err(EngineError::unreachable(format!(
            "WSL {v} has no verified COM ABI"
        )));
    }
    let pool = RpcPool::new(1);
    pool.run(|| {
        let mgr = Com::create_manager().map_err(|e| com_err(e, None))?;
        let sessions = list_sessions_on(&mgr)?;
        let mut out: Option<abi::IWSLCSession> = None;
        // SAFETY: verified slot 5; null name = caller's default session.
        let default = if unsafe { mgr.OpenSessionByName(PCWSTR::null(), &mut out) }.is_ok() {
            out.and_then(|s| {
                blanket(&s);
                let mut name = CoTaskMemWStr::null();
                // SAFETY: verified slot 1; LPWSTR freed by `name`.
                unsafe { s.GetDisplayName(name.out()) }.ok().ok()?;
                name.to_string_opt()
            })
        } else {
            None
        };
        Ok((sessions, default))
    })
    .await
}

// ───────────────────────────── engine ─────────────────────────────

/// Shared state reachable from pool jobs and stream threads.
pub(crate) struct Inner {
    pub(crate) id: EngineId,
    pub(crate) com: Com,
    pub(crate) wsl_version: Option<WslVersion>,
    pub(crate) abi: Option<AbiModule>,
}

/// The COM-transport engine.
pub struct WslcComEngine {
    inner: Arc<Inner>,
    pool: Arc<RpcPool>,
}

/// Result of the post-selection self-check (spec 20 §5.3).
#[derive(Debug, Clone, PartialEq)]
pub struct SelfCheck {
    pub com_version: (u32, u32, u32),
    pub sessions: usize,
    pub default_session: Option<String>,
}

impl WslcComEngine {
    /// Connects through the ABI module `abi_module` selected for `wsl_version` (caller ran
    /// `abi::select`), then runs the self-check. Any failure → `Err` (factory falls back to CLI,
    /// except `WSLC_E_CONTAINER_DISABLED`, which surfaces as `Unreachable{policy hint}`).
    pub async fn connect(
        id: EngineId,
        session: Option<String>,
        wsl_version: WslVersion,
        abi_module: AbiModule,
    ) -> EngineResult<(WslcComEngine, SelfCheck)> {
        // Only one module exists; the match keeps future modules explicit.
        match abi_module {
            AbiModule::V3_0 => {}
        }
        let pool = RpcPool::new(DEFAULT_THREADS);
        let sess = session.clone();
        let inner = pool
            .run(move || {
                let mgr = Com::create_manager().map_err(|e| com_err(e, None))?;
                Ok(Arc::new(Inner {
                    id,
                    com: Com {
                        manager: Agile(mgr),
                        session_name: sess,
                        session: Mutex::new(None),
                    },
                    wsl_version: Some(wsl_version),
                    abi: Some(abi_module),
                }))
            })
            .await?;
        let engine = WslcComEngine { inner, pool };
        let check = engine.self_check(Some(wsl_version)).await?;
        Ok((engine, check))
    }

    /// Test/diagnostic constructor over an existing manager (e.g. an in-process fake server).
    /// Skips version gating and the self-check.
    #[doc(hidden)]
    pub fn from_manager_for_tests(
        id: EngineId,
        manager: abi::IWSLCSessionManager,
        session: Option<String>,
    ) -> WslcComEngine {
        WslcComEngine {
            inner: Arc::new(Inner {
                id,
                com: Com {
                    manager: Agile(manager),
                    session_name: session,
                    session: Mutex::new(None),
                },
                wsl_version: None,
                abi: None,
            }),
            pool: RpcPool::new(2),
        }
    }

    /// Self-check through the selected module: `GetVersion` == file version (major.minor.patch),
    /// `ListSessions` sane (< 1000, NUL-terminated names), `OpenSessionByName` succeeds.
    pub async fn self_check(&self, expected: Option<WslVersion>) -> EngineResult<SelfCheck> {
        let inner = self.inner.clone();
        self.pool
            .run(move || {
                let v = inner.com.version()?;
                let got = (v.Major, v.Minor, v.Revision);
                if let Some(exp) = expected.filter(|e| e.triple() != got) {
                    return Err(EngineError::protocol(format!(
                        "self-check: GetVersion {}.{}.{} != wslservice.exe {exp}",
                        got.0, got.1, got.2
                    )));
                }
                let sessions = inner.com.list_sessions()?;
                if sessions.len() >= 1000 {
                    return Err(EngineError::protocol(format!(
                        "self-check: implausible session count {}",
                        sessions.len()
                    )));
                }
                if sessions.iter().any(|(_, n)| n.is_none()) {
                    return Err(EngineError::protocol(
                        "self-check: session name not NUL-terminated",
                    ));
                }
                let s = inner.com.session()?;
                let mut name = CoTaskMemWStr::null();
                // SAFETY: verified slot 1; callee-allocated LPWSTR freed by `name`.
                let _ = unsafe { s.GetDisplayName(name.out()) };
                Ok(SelfCheck {
                    com_version: got,
                    sessions: sessions.len(),
                    default_session: name.to_string_opt(),
                })
            })
            .await
    }

    /// Session display names (ENG-109), via `ListSessions`.
    pub async fn list_session_names(&self) -> EngineResult<Vec<String>> {
        let inner = self.inner.clone();
        self.pool
            .run(move || {
                Ok(inner
                    .com
                    .list_sessions()?
                    .into_iter()
                    .filter_map(|(_, n)| n)
                    .collect())
            })
            .await
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Inner) -> EngineResult<T> + Send + 'static,
    ) -> EngineResult<T> {
        let inner = self.inner.clone();
        self.pool.run(move || f(&inner)).await
    }

    async fn json_call(
        &self,
        what: &'static str,
        subject: Option<(ResourceKind, String)>,
        f: impl Fn(&abi::IWSLCSession, *mut windows::core::PSTR) -> windows::core::HRESULT
        + Send
        + 'static,
    ) -> EngineResult<Value> {
        let text = self
            .run(move |inner| {
                let subj = subject.as_ref().map(|(k, id)| Subject::new(*k, id));
                inner.com.with_session(
                    |s| {
                        let mut out = CoTaskMemStr::null();
                        ffi::check(f(s, out.out()))?;
                        Ok(out.to_string_lossy())
                    },
                    subj,
                )
            })
            .await?;
        serde_json::from_str(&text)
            .map_err(|e| EngineError::protocol(format!("{what}: invalid JSON: {e}")))
    }
}

// ───────────────────────────── list containers ─────────────────────────────

/// Owns a `ListContainers` result: frees per-entry strings, then the arrays.
struct ContainerList {
    entries: CoTaskMemArray<abi::WSLCContainerEntry>,
    ports: CoTaskMemArray<abi::WSLCContainerPortMapping>,
}

impl Drop for ContainerList {
    fn drop(&mut self) {
        for e in self.entries.as_mut_slice() {
            free_field(&mut e.Command.0);
            free_field(&mut e.Status.0);
            free_field(&mut e.Labels.0);
            free_field(&mut e.Networks.0);
            free_field(&mut e.Mounts.0);
        }
    }
}

fn pstr_opt(p: windows::core::PSTR) -> Option<String> {
    if p.0.is_null() {
        None
    } else {
        // SAFETY: non-null `[string] LPSTR` field from the server, NUL-terminated, alive while
        // the owning `ContainerList` lives.
        Some(
            unsafe { std::ffi::CStr::from_ptr(p.0 as *const std::ffi::c_char) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn raw_entries(list: &ContainerList) -> (Vec<RawContainerEntry>, Vec<RawPort>) {
    let entries = list
        .entries
        .as_slice()
        .iter()
        .map(|e| RawContainerEntry {
            id: fixed_cstr(&e.Id),
            name: fixed_cstr(&e.Name),
            image: fixed_cstr(&e.Image),
            command: pstr_opt(e.Command),
            status: pstr_opt(e.Status),
            labels: pstr_opt(e.Labels),
            networks: pstr_opt(e.Networks),
            mounts: pstr_opt(e.Mounts),
            state_changed_at: e.StateChangedAt,
            created_at: e.CreatedAt,
            size_rw: e.SizeRw,
            size_root_fs: e.SizeRootFs,
            local_volumes: e.LocalVolumes,
            state: e.State,
        })
        .collect();
    let ports = list
        .ports
        .as_slice()
        .iter()
        .map(|p| RawPort {
            container_id: fixed_cstr(&p.Id),
            host_port: p.PortMapping.HostPort,
            container_port: p.PortMapping.ContainerPort,
            family: p.PortMapping.Family,
            protocol: p.PortMapping.Protocol,
            binding_address: fixed_cstr(&p.PortMapping.BindingAddress),
        })
        .collect();
    (entries, ports)
}

fn list_containers_blocking(
    inner: &Inner,
    all: bool,
    size: bool,
    filters: &[(String, String)],
) -> EngineResult<Vec<ContainerSummary>> {
    let keys: Vec<std::ffi::CString> = filters
        .iter()
        .map(|(k, _)| cstring(k))
        .collect::<Result<_, _>>()?;
    let vals: Vec<std::ffi::CString> = filters
        .iter()
        .map(|(_, v)| cstring(v))
        .collect::<Result<_, _>>()?;
    let kv: Vec<abi::WSLCFilter> = keys
        .iter()
        .zip(&vals)
        .map(|(k, v)| abi::WSLCFilter {
            Key: pcstr(k),
            Value: pcstr(v),
        })
        .collect();
    let mut flags = 0;
    if all {
        flags |= abi::WSLC_LIST_CONTAINERS_FLAGS_ALL;
    }
    if size {
        flags |= abi::WSLC_LIST_CONTAINERS_FLAGS_SIZE;
    }
    let opts = abi::WSLCListContainersOptions {
        Flags: flags,
        Limit: -1,
        Filters: if kv.is_empty() {
            std::ptr::null()
        } else {
            kv.as_ptr()
        },
        FiltersCount: kv.len() as u32,
    };
    let (entries, ports) = inner.com.with_session(
        |s| {
            let mut list = ContainerList {
                entries: CoTaskMemArray::new(),
                ports: CoTaskMemArray::new(),
            };
            let (c, cn) = list.entries.out();
            let (p, pn) = list.ports.out();
            // SAFETY: verified slot 19; `opts`/filters outlive the call; out arrays owned by
            // `list` (strings freed in its Drop).
            ffi::check(unsafe { s.ListContainers(&opts, c, cn, p, pn) })?;
            Ok(raw_entries(&list))
        },
        None,
    )?;
    Ok(entries
        .iter()
        .map(|e| convert::container_summary(e, &ports))
        .collect())
}

fn label_filters(q: &ContainerQuery) -> EngineResult<Vec<(String, String)>> {
    q.label_filter
        .iter()
        .map(|(k, v)| {
            validate::validate_env_key(k)?;
            Ok((
                "label".to_owned(),
                match v {
                    Some(v) => format!("{k}={v}"),
                    None => k.clone(),
                },
            ))
        })
        .collect()
}

// ───────────────────────────── Engine impl ─────────────────────────────

fn signal_of(action_signal: Option<&str>) -> EngineResult<i32> {
    match action_signal {
        None => Ok(abi::WSLC_SIGNAL_SIGKILL),
        Some(s) => {
            validate::validate_signal(s)?;
            convert::signal_number(s).ok_or_else(|| EngineError::Api {
                status: 400,
                message: format!("unsupported signal: {s}"),
            })
        }
    }
}

fn stop_timeout(t: Option<u32>) -> i32 {
    t.map(|s| i32::try_from(s).unwrap_or(i32::MAX))
        .unwrap_or(abi::WSLC_STOP_TIMEOUT_DEFAULT)
}

fn unsupported(cap: Capabilities) -> EngineError {
    EngineError::Unsupported(cap)
}

#[async_trait]
impl Engine for WslcComEngine {
    fn id(&self) -> &EngineId {
        &self.inner.id
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Wslc
    }

    fn capabilities(&self) -> Capabilities {
        COM_CAPABILITIES
    }

    async fn ping(&self) -> EngineResult<()> {
        self.run(|inner| {
            inner.com.with_session(
                |s| {
                    let mut st = 0i32;
                    // SAFETY: verified slot 2, plain out-param.
                    ffi::check(unsafe { s.GetState(&mut st) })?;
                    if st == abi::WSLC_SESSION_STATE_TERMINATED {
                        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                            hr::RPC_E_DISCONNECTED,
                        )));
                    }
                    Ok(())
                },
                None,
            )
        })
        .await
    }

    async fn info(&self) -> EngineResult<EngineInfo> {
        let wsl = self.inner.wsl_version;
        self.run(move |inner| {
            let v = inner.com.version()?;
            let summaries = list_containers_blocking(inner, true, false, &[])?;
            let mut counts = ContainerCounts::default();
            for c in &summaries {
                if c.state.is_running() {
                    counts.running += 1;
                } else if c.state == dk_core::ContainerState::Paused {
                    counts.paused += 1;
                } else {
                    counts.stopped += 1;
                }
            }
            let images = inner.com.with_session(
                |s| {
                    let mut arr = CoTaskMemArray::<abi::WSLCImageInformation>::new();
                    let (p, n) = arr.out();
                    // SAFETY: verified slot 12; null options = defaults.
                    ffi::check(unsafe { s.ListImages(std::ptr::null(), p, n) })?;
                    let mut ids: Vec<String> =
                        arr.as_slice().iter().map(|i| fixed_cstr(&i.Hash)).collect();
                    ids.sort();
                    ids.dedup();
                    Ok(ids.len() as u32)
                },
                None,
            )?;
            let session = inner.com.with_session(
                |s| {
                    let mut name = CoTaskMemWStr::null();
                    // SAFETY: verified slot 1; LPWSTR freed by `name`.
                    ffi::check(unsafe { s.GetDisplayName(name.out()) })?;
                    Ok(name.to_string_opt())
                },
                None,
            )?;
            let server_version = format!("{}.{}.{}", v.Major, v.Minor, v.Revision);
            Ok(EngineInfo {
                name: match session {
                    Some(s) => format!("WSL containers ({s})"),
                    None => "WSL containers".into(),
                },
                kind: EngineKind::Wslc,
                transport: Some("com".into()),
                // ENG-110 diagnostics: COM is fully in use except Run (CreateContainer's
                // 30-field options struct is not verified live, so run_image returns 501).
                transport_note: Some(COM_TRANSPORT_NOTE.into()),
                server_version: wsl.map(|w| w.to_string()).unwrap_or(server_version),
                // Diagnostics: which verified ABI module is in use (none for the test fake).
                api_version: inner.abi.map(|m| format!("COM ABI {}", m.name())),
                os: "linux".into(),
                arch: std::env::consts::ARCH
                    .replace("aarch64", "arm64")
                    .replace("x86_64", "amd64"),
                kernel: None,
                cpus: None,
                mem_total: None,
                containers: counts,
                images,
                storage_driver: None,
                root_dir: None,
                daemon_id: None,
                list_stats_limit: 20,
                capabilities: COM_CAPABILITIES,
            })
        })
        .await
    }

    fn events(&self, filter: EventFilter) -> EngineStream<EngineEvent> {
        events_stream(self.inner.clone(), filter)
    }

    async fn list_containers(&self, q: ContainerQuery) -> EngineResult<Vec<ContainerSummary>> {
        let filters = label_filters(&q)?;
        self.run(move |inner| list_containers_blocking(inner, q.all, q.size, &filters))
            .await
    }

    async fn inspect_container(&self, id: &str) -> EngineResult<ContainerDetails> {
        let v = self.inspect_container_json(id).await?;
        docker_json::container_details(&v)
    }

    async fn container_action(&self, id: &str, action: ContainerAction) -> EngineResult<()> {
        validate::validate_id_or_name(id)?;
        let signal = match &action {
            ContainerAction::Kill { signal } => signal_of(signal.as_deref())?,
            ContainerAction::Pause | ContainerAction::Unpause => {
                return Err(unsupported(Capabilities::PAUSE));
            }
            _ => 0,
        };
        let id = id.to_owned();
        self.run(move |inner| {
            inner.com.with_container(&id, |c| {
                // SAFETY (all arms): verified IWSLCContainer slots; null start options and
                // callbacks are allowed by the IDL (`[in, unique]`).
                let r = unsafe {
                    match &action {
                        ContainerAction::Start => c.Start(
                            abi::WSLC_CONTAINER_START_FLAGS_NONE,
                            std::ptr::null(),
                            std::ptr::null_mut(),
                        ),
                        ContainerAction::Stop { timeout_s } => {
                            c.Stop(abi::WSLC_SIGNAL_NONE, stop_timeout(*timeout_s))
                        }
                        ContainerAction::Restart { timeout_s } => c.Restart(
                            abi::WSLC_SIGNAL_NONE,
                            stop_timeout(*timeout_s),
                            std::ptr::null_mut(),
                        ),
                        ContainerAction::Kill { .. } => c.Kill(signal),
                        ContainerAction::Pause | ContainerAction::Unpause => {
                            windows::core::HRESULT(hr::E_INVALIDARG)
                        }
                    }
                };
                // Idempotent like `wslc container start/stop` (THROW_IF_FAILED_EXCEPT).
                match (&action, r.0) {
                    (ContainerAction::Start, hr::WSLC_E_CONTAINER_IS_RUNNING) => Ok(()),
                    (ContainerAction::Stop { .. }, hr::WSLC_E_CONTAINER_NOT_RUNNING) => Ok(()),
                    _ => ffi::check(r),
                }
            })
        })
        .await
    }

    async fn remove_container(&self, id: &str, opts: RemoveContainerOpts) -> EngineResult<()> {
        validate::validate_id_or_name(id)?;
        let id = id.to_owned();
        let mut flags = 0;
        if opts.force {
            flags |= abi::WSLC_DELETE_FLAGS_FORCE;
        }
        if opts.volumes {
            flags |= abi::WSLC_DELETE_FLAGS_DELETE_VOLUMES;
        }
        self.run(move |inner| {
            // SAFETY: verified slot 3 (Delete), flags within WSLCDeleteFlagsValid.
            inner
                .com
                .with_container(&id, |c| ffi::check(unsafe { c.Delete(flags) }))
        })
        .await
    }

    async fn prune_containers(&self) -> EngineResult<PruneReport> {
        self.run(|inner| {
            inner.com.with_session(
                |s| {
                    let _op = Com::begin_op(s);
                    let mut res = abi::WSLCPruneContainersResults::default();
                    // SAFETY: verified slot 20; `res.Containers` is callee-allocated and freed
                    // right below via CoTaskMemArray ownership.
                    ffi::check(unsafe { s.PruneContainers(std::ptr::null(), 0, &mut res) })?;
                    let mut arr = CoTaskMemArray::<abi::WSLCContainerId>::new();
                    {
                        let (p, n) = arr.out();
                        // SAFETY: plain writes into our own out slots (take ownership).
                        unsafe {
                            *p = res.Containers;
                            *n = res.ContainersCount;
                        }
                    }
                    Ok(PruneReport {
                        deleted: arr.as_slice().iter().map(|id| fixed_cstr(id)).collect(),
                        space_reclaimed: res.SpaceReclaimed,
                    })
                },
                None,
            )
        })
        .await
    }

    fn logs(&self, id: &str, opts: LogOpts) -> EngineStream<LogChunk> {
        if let Err(e) = validate::validate_id_or_name(id) {
            return error_stream(e);
        }
        logs_stream(self.inner.clone(), id.to_owned(), opts)
    }

    fn stats(&self, id: &str) -> EngineStream<StatsSample> {
        if let Err(e) = validate::validate_id_or_name(id) {
            return error_stream(e);
        }
        stats_stream(self.inner.clone(), id.to_owned())
    }

    async fn top(&self, _id: &str) -> EngineResult<ProcessList> {
        Err(unsupported(Capabilities::TOP))
    }

    async fn exec(&self, id: &str, req: ExecRequest) -> EngineResult<Box<dyn TerminalSession>> {
        validate::validate_id_or_name(id)?;
        if req.cmd.is_empty() {
            return Err(EngineError::Api {
                status: 400,
                message: "exec: empty command".into(),
            });
        }
        let id = id.to_owned();
        let session = self.run(move |inner| start_exec(inner, &id, &req)).await?;
        Ok(Box::new(session))
    }

    async fn list_images(&self) -> EngineResult<Vec<ImageSummary>> {
        self.run(|inner| {
            let rows = inner.com.with_session(
                |s| {
                    let opts = abi::WSLCListImagesOptions {
                        Flags: abi::WSLC_LIST_IMAGES_DIGESTS
                            | abi::WSLC_LIST_IMAGES_CONTAINER_COUNTS,
                        Filters: std::ptr::null(),
                        FiltersCount: 0,
                    };
                    let mut arr = CoTaskMemArray::<abi::WSLCImageInformation>::new();
                    let (p, n) = arr.out();
                    // SAFETY: verified slot 12; options valid; array freed by `arr`.
                    ffi::check(unsafe { s.ListImages(&opts, p, n) })?;
                    Ok(arr
                        .as_slice()
                        .iter()
                        .map(|i| RawImage {
                            image: fixed_cstr(&i.Image),
                            hash: fixed_cstr(&i.Hash),
                            digest: fixed_cstr(&i.Digest),
                            size: i.Size,
                            created: i.Created,
                            containers: i.Containers,
                        })
                        .collect::<Vec<_>>())
                },
                None,
            )?;
            Ok(convert::image_summaries(&rows))
        })
        .await
    }

    async fn inspect_image(&self, id: &str) -> EngineResult<ImageDetails> {
        let v = self.inspect_image_json(id).await?;
        docker_json::image_details(&v)
    }

    async fn image_history(&self, _id: &str) -> EngineResult<Vec<ImageLayer>> {
        Err(unsupported(Capabilities::IMAGE_HISTORY))
    }

    fn pull_image(
        &self,
        reference: &str,
        auth: Option<RegistryAuth>,
    ) -> EngineStream<PullProgress> {
        if let Err(e) = validate::validate_image_ref(reference) {
            return error_stream(e);
        }
        pull_stream(self.inner.clone(), reference.to_owned(), auth)
    }

    async fn remove_image(&self, id: &str, force: bool) -> EngineResult<Vec<ImageDeleteItem>> {
        validate::validate_image_ref(id)?;
        let id = id.to_owned();
        self.run(move |inner| {
            let img = cstring(&id)?;
            let subj = Subject::new(ResourceKind::Image, &id);
            inner.com.with_session(
                |s| {
                    let opts = abi::WSLCDeleteImageOptions {
                        Image: pcstr(&img),
                        Flags: if force {
                            abi::WSLC_DELETE_IMAGE_FLAGS_FORCE
                        } else {
                            0
                        },
                    };
                    let mut arr = CoTaskMemArray::<abi::WSLCDeletedImageInformation>::new();
                    let (p, n) = arr.out();
                    // SAFETY: verified slot 13; `img` outlives the call; array freed by `arr`.
                    ffi::check(unsafe { s.DeleteImage(&opts, p, n) })?;
                    Ok(deleted_items(arr.as_slice()))
                },
                Some(subj),
            )
        })
        .await
    }

    async fn prune_images(&self, dangling_only: bool) -> EngineResult<PruneReport> {
        self.run(move |inner| {
            let k = cstring("dangling")?;
            let v = cstring(if dangling_only { "true" } else { "false" })?;
            let filters = [abi::WSLCFilter {
                Key: pcstr(&k),
                Value: pcstr(&v),
            }];
            inner.com.with_session(
                |s| {
                    let mut arr = CoTaskMemArray::<abi::WSLCDeletedImageInformation>::new();
                    let (p, n) = arr.out();
                    let mut space = 0u64;
                    // SAFETY: verified slot 16; filter strings outlive the call.
                    ffi::check(unsafe { s.PruneImages(filters.as_ptr(), 1, p, n, &mut space) })?;
                    Ok(PruneReport {
                        deleted: arr
                            .as_slice()
                            .iter()
                            .map(|d| fixed_cstr(&d.Image))
                            .collect(),
                        space_reclaimed: space,
                    })
                },
                None,
            )
        })
        .await
    }

    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> EngineResult<()> {
        validate::validate_image_ref(id)?;
        let (repo, tag) = convert::split_tag_target(repo, tag);
        validate::validate_image_ref(&format!("{repo}:{tag}"))?;
        let id = id.to_owned();
        self.run(move |inner| {
            let (i, r, t) = (cstring(&id)?, cstring(&repo)?, cstring(&tag)?);
            let subj = Subject::new(ResourceKind::Image, &id);
            inner.com.with_session(
                |s| {
                    let opts = abi::WSLCTagImageOptions {
                        Image: pcstr(&i),
                        Repo: pcstr(&r),
                        Tag: pcstr(&t),
                    };
                    // SAFETY: verified slot 14; strings outlive the call.
                    ffi::check(unsafe { s.TagImage(&opts) })
                },
                Some(subj),
            )
        })
        .await
    }

    async fn run_image(&self, spec: RunSpec) -> EngineResult<String> {
        // `CreateContainer(WSLCContainerOptions)` is declared in the ABI module but its 30-field
        // layout has not been exercised against a live server (spec 20 §5.4 ⚠). Never call an
        // unverified layout: the factory routes Run to the CLI note instead.
        let _ = spec;
        Err(EngineError::Api {
            status: 501,
            message: RUN_NOT_VERIFIED.into(),
        })
    }

    async fn list_volumes(&self) -> EngineResult<Vec<VolumeSummary>> {
        let v = self
            .json_call("ListVolumes", None, |s, out| {
                // SAFETY: verified slot 32; no filters.
                unsafe { s.ListVolumes(std::ptr::null(), 0, out) }
            })
            .await?;
        Ok(v.as_array()
            .map(|a| a.iter().map(docker_json::volume_summary).collect())
            .unwrap_or_default())
    }

    async fn inspect_volume(&self, name: &str) -> EngineResult<VolumeDetails> {
        validate::validate_name(name)?;
        let v = self.inspect_volume_json(name).await?;
        let summary = docker_json::volume_summary(&v);
        let options = v
            .get("Options")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let containers = self
            .list_containers(ContainerQuery::default())
            .await
            .unwrap_or_default();
        Ok(VolumeDetails {
            used_by: docker_json::volume_used_by(&summary.name, &containers),
            status: v.get("Status").cloned(),
            options,
            summary,
            raw: v,
        })
    }

    async fn create_volume(&self, spec: VolumeSpec) -> EngineResult<VolumeSummary> {
        let name = self.create_volume_raw(spec).await?;
        let v = self.inspect_volume_json(&name).await?;
        Ok(docker_json::volume_summary(&v))
    }

    async fn remove_volume(&self, name: &str, force: bool) -> EngineResult<()> {
        // WSLC DeleteVolume has no force flag; a volume in use fails with a server error.
        let _ = force;
        validate::validate_name(name)?;
        let name = name.to_owned();
        self.run(move |inner| {
            let n = cstring(&name)?;
            inner.com.with_session(
                // SAFETY: verified slot 31; `n` outlives the call.
                |s| ffi::check(unsafe { s.DeleteVolume(pcstr(&n)) }),
                Some(Subject::new(ResourceKind::Volume, &name)),
            )
        })
        .await
    }

    async fn prune_volumes(&self) -> EngineResult<PruneReport> {
        self.run(|inner| {
            // Like Docker ≥ 1.42 + `all=true` (the Docker backend does the same): prune every
            // unused volume, not only anonymous ones.
            let k = cstring("all")?;
            let v = cstring("true")?;
            let filters = [abi::WSLCFilter {
                Key: pcstr(&k),
                Value: pcstr(&v),
            }];
            inner.com.with_session(
                |s| {
                    let mut arr = CoTaskMemArray::<abi::WSLCVolumeName>::new();
                    let (p, n) = arr.out();
                    let mut space = 0u64;
                    // SAFETY: verified slot 36; filter strings outlive the call.
                    ffi::check(unsafe {
                        s.PruneVolumes(filters.as_ptr(), 1, std::ptr::null_mut(), p, n, &mut space)
                    })?;
                    Ok(PruneReport {
                        deleted: arr.as_slice().iter().map(|n| fixed_cstr(n)).collect(),
                        space_reclaimed: space,
                    })
                },
                None,
            )
        })
        .await
    }

    async fn disk_usage(&self) -> EngineResult<DiskUsage> {
        Err(unsupported(Capabilities::DISK_USAGE))
    }

    async fn list_networks(&self) -> EngineResult<Vec<NetworkSummary>> {
        let v = self
            .json_call("ListNetworks", None, |s, out| {
                // SAFETY: verified slot 39 (spike F-6); no filters.
                unsafe { s.ListNetworks(std::ptr::null(), 0, out) }
            })
            .await?;
        Ok(v.as_array()
            .map(|a| a.iter().map(docker_json::network_summary).collect())
            .unwrap_or_default())
    }

    async fn inspect_network(&self, id: &str) -> EngineResult<NetworkDetails> {
        validate::validate_id_or_name(id)?;
        let v = self.inspect_network_json(id).await?;
        docker_json::network_details(&v)
    }

    async fn remove_network(&self, id: &str) -> EngineResult<()> {
        validate::validate_id_or_name(id)?;
        let name = id.to_owned();
        self.run(move |inner| {
            let n = cstring(&name)?;
            inner.com.with_session(
                // SAFETY: verified slot 38; `n` outlives the call.
                |s| ffi::check(unsafe { s.DeleteNetwork(pcstr(&n)) }),
                Some(Subject::new(ResourceKind::Network, &name)),
            )
        })
        .await
    }

    async fn prune_networks(&self) -> EngineResult<PruneReport> {
        self.run(|inner| {
            inner.com.with_session(
                |s| {
                    let mut arr = CoTaskMemArray::<abi::WSLCNetworkName>::new();
                    let (p, n) = arr.out();
                    // SAFETY: verified slot 41; no filters.
                    ffi::check(unsafe { s.PruneNetworks(std::ptr::null(), 0, p, n) })?;
                    Ok(PruneReport {
                        deleted: arr.as_slice().iter().map(|n| fixed_cstr(n)).collect(),
                        space_reclaimed: 0,
                    })
                },
                None,
            )
        })
        .await
    }
}

fn deleted_items(items: &[abi::WSLCDeletedImageInformation]) -> Vec<ImageDeleteItem> {
    items
        .iter()
        .map(|d| {
            let name = fixed_cstr(&d.Image);
            if d.Type == abi::WSLC_DELETED_IMAGE_TYPE_UNTAGGED {
                ImageDeleteItem::Untagged(name)
            } else {
                ImageDeleteItem::Deleted(name)
            }
        })
        .collect()
}

// ───────────────────────────── raw JSON accessors ─────────────────────────────

impl WslcComEngine {
    /// `CreateVolume` → the created volume's name (no JSON mapping).
    pub async fn create_volume_raw(&self, spec: VolumeSpec) -> EngineResult<String> {
        if let Some(n) = &spec.name {
            validate::validate_name(n)?;
        }
        for k in spec.labels.keys().chain(spec.driver_opts.keys()) {
            validate::validate_env_key(k)?;
        }
        self.run(move |inner| {
                let name = spec.name.as_deref().map(cstring).transpose()?;
                let driver = spec.driver.as_deref().map(cstring).transpose()?;
                let to_kv = |m: &BTreeMap<String, String>| -> EngineResult<Vec<(std::ffi::CString, std::ffi::CString)>> {
                    m.iter().map(|(k, v)| Ok((cstring(k)?, cstring(v)?))).collect()
                };
                let labels = to_kv(&spec.labels)?;
                let opts_kv = to_kv(&spec.driver_opts)?;
                let lab: Vec<abi::KeyValuePair> = labels.iter().map(|(k, v)| abi::KeyValuePair { Key: pcstr(k), Value: pcstr(v) }).collect();
                let dop: Vec<abi::KeyValuePair> = opts_kv.iter().map(|(k, v)| abi::KeyValuePair { Key: pcstr(k), Value: pcstr(v) }).collect();
                let opts = abi::WSLCVolumeOptions {
                    Name: opt_pcstr(name.as_ref()),
                    Driver: opt_pcstr(driver.as_ref()),
                    DriverOpts: if dop.is_empty() { std::ptr::null() } else { dop.as_ptr() },
                    DriverOptsCount: dop.len() as u32,
                    Labels: if lab.is_empty() { std::ptr::null() } else { lab.as_ptr() },
                    LabelsCount: lab.len() as u32,
                };
                inner.com.with_session(
                    |s| {
                        // SAFETY: plain-data out struct (char arrays), zero-initialised.
                        let mut info: abi::WSLCVolumeInformation = unsafe { std::mem::zeroed() };
                        // SAFETY: verified slot 30; all option strings outlive the call.
                        ffi::check(unsafe { s.CreateVolume(&opts, &mut info) })?;
                        Ok(fixed_cstr(&info.Name))
                    },
                    None,
                )
            })
            .await
    }

    /// `IWSLCContainer::Inspect(FALSE)` as raw JSON (Inspect tab, tests).
    pub async fn inspect_container_json(&self, id: &str) -> EngineResult<Value> {
        validate::validate_id_or_name(id)?;
        let id = id.to_owned();
        let text = self
            .run(move |inner| {
                inner.com.with_container(&id, |c| {
                    let mut out = CoTaskMemStr::null();
                    // SAFETY: verified slot 8; LPSTR freed by `out`.
                    ffi::check(unsafe { c.Inspect(BOOL(0), out.out()) })?;
                    Ok(out.to_string_lossy())
                })
            })
            .await?;
        serde_json::from_str(&text)
            .map_err(|e| EngineError::protocol(format!("Inspect: invalid JSON: {e}")))
    }

    /// `IWSLCContainer::Stats()` raw JSON (one sample; ~1 s server-side).
    pub async fn stats_json(&self, id: &str) -> EngineResult<Value> {
        validate::validate_id_or_name(id)?;
        let id = id.to_owned();
        self.run(move |inner| stats_once(inner, &id)).await
    }

    pub async fn inspect_image_json(&self, id: &str) -> EngineResult<Value> {
        validate::validate_image_ref(id)?;
        let img = id.to_owned();
        self.json_call(
            "InspectImage",
            Some((ResourceKind::Image, id.to_owned())),
            move |s, out| {
                match std::ffi::CString::new(img.as_str()) {
                    // SAFETY: verified slot 15; `c` outlives the call.
                    Ok(c) => unsafe { s.InspectImage(PCSTR(c.as_ptr() as *const u8), out) },
                    Err(_) => windows::core::HRESULT(hr::E_INVALIDARG),
                }
            },
        )
        .await
    }

    pub async fn inspect_volume_json(&self, name: &str) -> EngineResult<Value> {
        let n = name.to_owned();
        self.json_call(
            "InspectVolume",
            Some((ResourceKind::Volume, name.to_owned())),
            move |s, out| {
                match std::ffi::CString::new(n.as_str()) {
                    // SAFETY: verified slot 33; `c` outlives the call.
                    Ok(c) => unsafe { s.InspectVolume(PCSTR(c.as_ptr() as *const u8), out) },
                    Err(_) => windows::core::HRESULT(hr::E_INVALIDARG),
                }
            },
        )
        .await
    }

    pub async fn inspect_network_json(&self, name: &str) -> EngineResult<Value> {
        let n = name.to_owned();
        self.json_call(
            "InspectNetwork",
            Some((ResourceKind::Network, name.to_owned())),
            move |s, out| {
                match std::ffi::CString::new(n.as_str()) {
                    // SAFETY: verified slot 40; `c` outlives the call.
                    Ok(c) => unsafe { s.InspectNetwork(PCSTR(c.as_ptr() as *const u8), out) },
                    Err(_) => windows::core::HRESULT(hr::E_INVALIDARG),
                }
            },
        )
        .await
    }

    /// `ListVolumes` / `ListNetworks` raw JSON arrays (tests, diagnostics).
    pub async fn list_volumes_json(&self) -> EngineResult<Value> {
        // SAFETY: verified slot 32; no filters.
        self.json_call("ListVolumes", None, |s, out| unsafe {
            s.ListVolumes(std::ptr::null(), 0, out)
        })
        .await
    }

    pub async fn list_networks_json(&self) -> EngineResult<Value> {
        // SAFETY: verified slot 39; no filters.
        self.json_call("ListNetworks", None, |s, out| unsafe {
            s.ListNetworks(std::ptr::null(), 0, out)
        })
        .await
    }

    /// Raw `GetEvents` JSON objects (before `docker_json::engine_event`), for tests/diagnostics.
    pub fn events_json(&self, since_unix: i64) -> EngineStream<Value> {
        events_raw_stream(self.inner.clone(), since_unix)
    }
}

fn stats_once(inner: &Inner, id: &str) -> EngineResult<Value> {
    let text = inner.com.with_container(id, |c| {
        let mut out = CoTaskMemStr::null();
        // SAFETY: verified slot 14; LPSTR freed by `out`.
        ffi::check(unsafe { c.Stats(out.out()) })?;
        Ok(out.to_string_lossy())
    })?;
    serde_json::from_str(&text)
        .map_err(|e| EngineError::protocol(format!("Stats: invalid JSON: {e}")))
}

/// Raw stats JSON → normalised sample (pure; tested with fixtures).
pub fn stats_sample(
    norm: &mut StatsNormalizer,
    v: &Value,
    now: OffsetDateTime,
) -> EngineResult<StatsSample> {
    Ok(norm.push(RawStats::from_docker_json(v)?, now))
}

// ───────────────────────────── streams ─────────────────────────────

fn spawn_or_err<T: Send + 'static>(
    name: &str,
    cancel: Cancel,
    rx: mpsc::Receiver<EngineResult<T>>,
    body: impl FnOnce() + Send + 'static,
) -> EngineStream<T> {
    match super::pool::spawn_stream_thread(name, body) {
        Ok(_detached) => Box::pin(CancelOnDrop::new(rx, cancel)),
        Err(e) => error_stream(EngineError::protocol(format!(
            "failed to spawn {name} thread: {e}"
        ))),
    }
}

fn events_raw_stream(inner: Arc<Inner>, since_unix: i64) -> EngineStream<Value> {
    let cancel = match Cancel::new() {
        Ok(c) => c,
        Err(e) => return error_stream(e),
    };
    let (mut tx, rx) = mpsc::channel::<EngineResult<Value>>(64);
    let c2 = cancel.clone();
    spawn_or_err("events", cancel, rx, move || {
        // Hold a container-operation token like `wslc system events` does, so the VM stays
        // up while someone is watching.
        let stream = inner.com.with_session(
            |s| {
                let _op = Com::begin_op(s);
                let mut out: Option<abi::IWSLCEventStream> = None;
                // SAFETY: verified slot 5; no filters; out interface.
                ffi::check(unsafe {
                    s.GetEvents(since_unix.max(0), 0, std::ptr::null(), 0, &mut out)
                })?;
                Ok((out, _op))
            },
            None,
        );
        let (stream, _op) = match stream {
            Ok((Some(st), op)) => (st, op),
            Ok((None, _)) => {
                send_blocking(
                    &mut tx,
                    Err(EngineError::protocol("GetEvents returned null")),
                );
                return;
            }
            Err(e) => {
                send_blocking(&mut tx, Err(e));
                return;
            }
        };
        blanket(&stream);
        loop {
            let mut json = CoTaskMemStr::null();
            // SAFETY: verified IWSLCEventStream slot 0. The cancel event is manual-reset and
            // stays signalled until GetNext returns (IDL requirement); we own it via `c2`.
            let r = unsafe { stream.GetNext(c2.event().raw(), json.out()) };
            match r.0 {
                0 => {
                    let text = json.to_string_lossy();
                    let item = serde_json::from_str::<Value>(&text)
                        .map_err(|e| EngineError::protocol(format!("event JSON: {e}")));
                    if !send_blocking(&mut tx, item) {
                        return;
                    }
                }
                hr::E_ABORT => return, // cancelled / session terminating / caller exit
                hr::WSLC_E_EVENT_STREAM_FINISHED => return,
                hr::WSLC_E_EVENTS_LOST => {
                    // Spec 20 §5.4: report the gap as exactly `Protocol("events lost")` (the hub
                    // turns it into `Feed::Lagged` → full refetch). The server resets the
                    // reader, so keep streaming afterwards.
                    if !send_blocking(&mut tx, Err(EngineError::Protocol(EVENTS_LOST.into()))) {
                        return;
                    }
                }
                _ => {
                    if c2.is_fired() {
                        return;
                    }
                    let e = ffi::check(r)
                        .err()
                        .map(|e| com_err(e, None))
                        .unwrap_or_else(|| EngineError::protocol("GetNext failed"));
                    send_blocking(&mut tx, Err(e));
                    return;
                }
            }
        }
    })
}

fn event_kind_filter(kinds: &[ResourceKind]) -> impl Fn(&EngineEvent) -> bool + Send + 'static {
    let kinds = kinds.to_vec();
    move |e| kinds.is_empty() || kinds.contains(&e.kind)
}

fn events_stream(inner: Arc<Inner>, filter: EventFilter) -> EngineStream<EngineEvent> {
    use futures::StreamExt;
    // `since == 0` would replay the whole buffered ring (EventStore.cpp), so default to now.
    let since = filter
        .since
        .unwrap_or_else(OffsetDateTime::now_utc)
        .unix_timestamp();
    let keep = event_kind_filter(&filter.kinds);
    let raw = events_raw_stream(inner, since);
    Box::pin(raw.filter_map(move |item| {
        let out = match item {
            Ok(v) => docker_json::engine_event(&convert::adapt_event_json(v))
                .filter(|e| keep(e))
                .map(Ok),
            Err(e) => Some(Err(e)),
        };
        futures::future::ready(out)
    }))
}

fn logs_stream(inner: Arc<Inner>, id: String, opts: LogOpts) -> EngineStream<LogChunk> {
    let cancel = match Cancel::new() {
        Ok(c) => c,
        Err(e) => return error_stream(e),
    };
    let (tx, rx) = mpsc::channel::<EngineResult<LogChunk>>(256);
    let c2 = cancel.clone();
    spawn_or_err("logs", cancel, rx, move || {
        let mut flags = 0;
        if opts.follow {
            flags |= abi::WSLC_LOGS_FLAGS_FOLLOW;
        }
        if opts.timestamps {
            flags |= abi::WSLC_LOGS_FLAGS_TIMESTAMPS;
        }
        let since = opts.since.map(|t| t.unix_timestamp().max(0)).unwrap_or(0);
        let tail = opts.tail.map(u64::from).unwrap_or(0);
        let mut tx = tx;
        // Token held for the whole stream (logs are a container operation in wslc).
        let opened = inner.com.with_session(
            |s| {
                let op = Com::begin_op(s);
                let c = Com::open_container(s, &id)?;
                let (mut so, mut se) = (abi::WSLCHandle::default(), abi::WSLCHandle::default());
                // SAFETY: verified slot 9; out handles become ours (wrapped below).
                ffi::check(unsafe { c.Logs(flags, &mut so, &mut se, since, 0, tail) })?;
                Ok((op, so, se))
            },
            Some(Subject::new(ResourceKind::Container, &id)),
        );
        let (_op, so, se) = match opened {
            Ok(v) => v,
            Err(e) => {
                send_blocking(&mut tx, Err(e));
                return;
            }
        };
        let stdout = OwnedHandle::new(so.Handle);
        let stderr = OwnedHandle::new(se.Handle);
        // TTY containers: only stdout is returned (raw, not multiplexed) → Console (LOG-008).
        let out_kind = if stderr.is_none() {
            LogStream::Console
        } else {
            LogStream::Stdout
        };
        let ts = opts.timestamps;
        // stderr on its own thread so both pipes drain concurrently.
        let err_thread = stderr.map(|h| {
            let mut tx2 = tx.clone();
            let c3 = c2.clone();
            super::pool::spawn_stream_thread("logs-err", move || {
                pump_lines(&h, &c3, LogStream::Stderr, ts, &mut tx2);
            })
        });
        if let Some(h) = stdout {
            pump_lines(&h, &c2, out_kind, ts, &mut tx);
        }
        if let Some(Ok(t)) = err_thread {
            let _ = t.join();
        }
    })
}

fn pump_lines(
    h: &OwnedHandle,
    cancel: &Cancel,
    kind: LogStream,
    ts: bool,
    tx: &mut mpsc::Sender<EngineResult<LogChunk>>,
) {
    let mut split = LineSplitter::default();
    let r = pump_handle(h, cancel, |data| {
        for line in split.push(&data) {
            if !send_blocking(tx, Ok(convert::log_line(kind, line, ts))) {
                return false;
            }
        }
        true
    });
    match r {
        Ok(true) => {
            if let Some(rest) = split.finish() {
                send_blocking(tx, Ok(convert::log_line(kind, rest, ts)));
            }
        }
        Ok(false) => {}
        Err(e) => {
            send_blocking(tx, Err(e));
        }
    }
}

fn stats_stream(inner: Arc<Inner>, id: String) -> EngineStream<StatsSample> {
    let cancel = match Cancel::new() {
        Ok(c) => c,
        Err(e) => return error_stream(e),
    };
    let (mut tx, rx) = mpsc::channel::<EngineResult<StatsSample>>(4);
    let c2 = cancel.clone();
    spawn_or_err("stats", cancel, rx, move || {
        let mut norm = StatsNormalizer::new();
        loop {
            if c2.is_fired() {
                return;
            }
            let item = stats_once(&inner, &id)
                .and_then(|v| stats_sample(&mut norm, &v, OffsetDateTime::now_utc()));
            let fatal = matches!(
                &item,
                Err(EngineError::NotFound { .. }) | Err(EngineError::Unreachable { .. })
            );
            if !send_blocking(&mut tx, item) || fatal {
                return;
            }
            // Cancellable sleep: returns early when the consumer drops the stream.
            if c2.event().wait_timeout(STATS_INTERVAL) {
                return;
            }
        }
    })
}

// ───────────────────────────── pull ─────────────────────────────

/// Rust implementation of `IProgressCallback`, forwarding structured progress.
#[windows::core::implement(abi::IProgressCallback)]
struct ProgressSink {
    tx: Mutex<mpsc::Sender<EngineResult<PullProgress>>>,
    digest: Mutex<Option<String>>,
}

impl abi::IProgressCallback_Impl for ProgressSink_Impl {
    unsafe fn OnProgress(
        &self,
        status: PCSTR,
        id: PCSTR,
        current: u64,
        total: u64,
    ) -> windows::core::HRESULT {
        let s = |p: PCSTR| {
            if p.is_null() {
                String::new()
            } else {
                // SAFETY: non-null `[unique] LPCSTR` args are NUL-terminated and valid for the
                // duration of this callback.
                unsafe { p.to_string() }.unwrap_or_default()
            }
        };
        let (status, id) = (s(status), s(id));
        if let Some(d) = convert::digest_from_status(&status) {
            *self.digest.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
        }
        let mut tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        // Never block the server's callback thread: drop progress if the consumer lags.
        let _ = tx.try_send(Ok(convert::pull_progress(&status, &id, current, total)));
        // Consumer gone → ask the server to stop (non-success aborts the pull).
        if tx.is_closed() {
            windows::core::HRESULT(hr::E_ABORT)
        } else {
            windows::core::HRESULT(0)
        }
    }
}

fn pull_stream(
    inner: Arc<Inner>,
    reference: String,
    auth: Option<RegistryAuth>,
) -> EngineStream<PullProgress> {
    let cancel = match Cancel::new() {
        Ok(c) => c,
        Err(e) => return error_stream(e),
    };
    let (tx, rx) = mpsc::channel::<EngineResult<PullProgress>>(512);
    spawn_or_err("pull", cancel, rx, move || {
        let mut done_tx = tx.clone();
        let sink = ProgressSink {
            tx: Mutex::new(tx),
            digest: Mutex::new(None),
        };
        let obj = windows::core::ComObject::new(sink);
        let cb: abi::IProgressCallback = obj.to_interface();
        let auth_header = auth.as_ref().map(convert::registry_auth_header);
        let r = (|| -> EngineResult<()> {
            let img = cstring(&reference)?;
            let a = auth_header.as_deref().map(cstring).transpose()?;
            inner.com.with_session(
                |s| {
                    // SAFETY: verified slot 6; strings outlive the call; `cb` is a live agile
                    // COM object (AddRef'd by the proxy for the call's duration).
                    ffi::check(unsafe {
                        s.PullImage(
                            pcstr(&img),
                            opt_pcstr(a.as_ref()),
                            BOOL(0),
                            cb.as_raw(),
                            std::ptr::null_mut(),
                        )
                    })
                },
                Some(Subject::new(ResourceKind::Image, &reference)),
            )
        })();
        let digest = obj.digest.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let last = match r {
            Ok(()) => Ok(PullProgress::Done { digest }),
            Err(e) => Err(e),
        };
        let _ = done_tx.try_send(last);
    })
}

// ───────────────────────────── exec ─────────────────────────────

struct ExecShared {
    process: Agile<abi::IWSLCProcess>,
    tty: OwnedHandle,
    exit_event: Option<OwnedHandle>,
    _op: Option<Agile<IUnknown>>,
}

/// Interactive exec over `IWSLCProcess` with a real TTY (spec 21 §5).
///
/// Writes run on their own single-thread pool (`io`, keeps keystrokes ordered) and wait on
/// `write_cancel`; COM control calls (`ResizeTty`, `Signal`) run on `ctl`, so `close()` never
/// queues behind a write stuck on a peer that stopped reading.
pub struct WslcComTerminal {
    shared: Arc<ExecShared>,
    out_rx: Option<mpsc::Receiver<EngineResult<Bytes>>>,
    /// Stops the output reader thread.
    cancel: Cancel,
    /// Interrupts pending and future writes (fired by `close()` / drop).
    write_cancel: Cancel,
    exit_rx: Mutex<Option<oneshot::Receiver<Option<i64>>>>,
    io: Arc<RpcPool>,
    ctl: Arc<RpcPool>,
}

fn start_exec(inner: &Inner, id: &str, req: &ExecRequest) -> EngineResult<WslcComTerminal> {
    let args: Vec<std::ffi::CString> = req
        .cmd
        .iter()
        .map(|a| cstring(a))
        .collect::<Result<_, _>>()?;
    let env: Vec<std::ffi::CString> = req
        .env
        .iter()
        .map(|a| cstring(a))
        .collect::<Result<_, _>>()?;
    let user = req.user.as_deref().map(cstring).transpose()?;
    let cwd = req.working_dir.as_deref().map(cstring).transpose()?;
    let argv: Vec<PCSTR> = args.iter().map(pcstr).collect();
    let envp: Vec<PCSTR> = env.iter().map(pcstr).collect();
    let mut flags = abi::WSLC_PROCESS_FLAGS_STDIN;
    if req.tty {
        flags |= abi::WSLC_PROCESS_FLAGS_TTY;
    }
    let opts = abi::WSLCProcessOptions {
        CurrentDirectory: opt_pcstr(cwd.as_ref()),
        User: opt_pcstr(user.as_ref()),
        CommandLine: abi::WSLCStringArray {
            Values: argv.as_ptr(),
            Count: argv.len() as u32,
        },
        Environment: if envp.is_empty() {
            abi::WSLCStringArray::EMPTY
        } else {
            abi::WSLCStringArray {
                Values: envp.as_ptr(),
                Count: envp.len() as u32,
            }
        },
        Flags: flags,
    };
    let start = abi::WSLCProcessStartOptions {
        TtyRows: u32::from(req.rows.max(1)),
        TtyColumns: u32::from(req.cols.max(1)),
        DetachKeys: PCSTR::null(),
    };
    let (op, process) = inner.com.with_session(
        |s| {
            let op = Com::begin_op(s);
            let c = Com::open_container(s, id)?;
            let mut p: Option<abi::IWSLCProcess> = None;
            // SAFETY: verified slot 7 (F-7); option strings/arrays outlive the call.
            ffi::check(unsafe { c.Exec(&opts, &start, &mut p) })?;
            Ok((op, p))
        },
        Some(Subject::new(ResourceKind::Container, id)),
    )?;
    let process = process.ok_or_else(|| EngineError::protocol("Exec returned no process"))?;
    blanket(&process);
    let fd = if req.tty {
        abi::WSLC_FD_TTY
    } else {
        abi::WSLC_FD_STDOUT
    };
    let mut h = abi::WSLCHandle::default();
    // SAFETY: verified IWSLCProcess slot 2; handle ownership transfers to us.
    ffi::check(unsafe { process.GetStdHandle(fd, &mut h) }).map_err(|e| com_err(e, None))?;
    let tty =
        OwnedHandle::new(h.Handle).ok_or_else(|| EngineError::protocol("Exec: no TTY handle"))?;
    let mut ev = HANDLE::default();
    // SAFETY: verified slot 1; event handle ownership transfers to us.
    let exit_event = if unsafe { process.GetExitEvent(&mut ev) }.is_ok() {
        OwnedHandle::new(ev)
    } else {
        None
    };
    let shared = Arc::new(ExecShared {
        process: Agile(process),
        tty,
        exit_event,
        _op: op.map(Agile),
    });
    let cancel = Cancel::new()?;
    let (mut tx, out_rx) = mpsc::channel::<EngineResult<Bytes>>(256);
    let (exit_tx, exit_rx) = oneshot::channel::<Option<i64>>();
    let sh = shared.clone();
    let c2 = cancel.clone();
    super::pool::spawn_stream_thread("exec-tty", move || {
        let r = pump_handle(&sh.tty, &c2, |data| send_blocking(&mut tx, Ok(data)));
        if let Err(e) = r {
            send_blocking(&mut tx, Err(e));
        }
        // Output ended: wait for the exit event (bounded) then read the exit code.
        if let Some(ev) = &sh.exit_event {
            let _ = win32::wait_handle_or_cancel(ev, c2.event());
        }
        let (mut state, mut code) = (0i32, 0i32);
        // SAFETY: verified slot 5, plain out-params.
        let ok = unsafe { sh.process.0.GetState(&mut state, &mut code) }.is_ok();
        let exit = ok
            .then_some(match state {
                abi::WSLC_PROCESS_STATE_EXITED => Some(i64::from(code)),
                abi::WSLC_PROCESS_STATE_SIGNALLED => Some(128 + i64::from(code)),
                _ => None,
            })
            .flatten();
        let _ = exit_tx.send(exit);
    })
    .map_err(|e| EngineError::protocol(format!("failed to spawn exec reader: {e}")))?;
    Ok(WslcComTerminal {
        shared,
        out_rx: Some(out_rx),
        cancel,
        write_cancel: Cancel::new()?,
        exit_rx: Mutex::new(Some(exit_rx)),
        io: RpcPool::new(1),
        ctl: RpcPool::new(1),
    })
}

#[async_trait]
impl TerminalSession for WslcComTerminal {
    fn output(&mut self) -> EngineStream<Bytes> {
        match self.out_rx.take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::empty()),
        }
    }

    async fn write(&self, data: Bytes) -> EngineResult<()> {
        let sh = self.shared.clone();
        let cancel = self.write_cancel.clone();
        self.io
            .run(move || {
                win32::write_all(&sh.tty, &data, cancel.event()).map_err(|e| {
                    if cancel.is_fired() {
                        EngineError::protocol("TTY closed")
                    } else {
                        EngineError::protocol(format!(
                            "TTY write failed: 0x{:08X}",
                            e.code().0 as u32
                        ))
                    }
                })
            })
            .await
    }

    async fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
        let sh = self.shared.clone();
        self.ctl
            .run(move || {
                // SAFETY: verified IWSLCProcess slot 6 (F-7: `stty size` reflected it).
                ffi::check(unsafe {
                    sh.process
                        .0
                        .ResizeTty(u32::from(rows.max(1)), u32::from(cols.max(1)))
                })
                .map_err(|e| com_err(e, None))
            })
            .await
    }

    async fn wait(&self) -> EngineResult<Option<i64>> {
        let rx = self
            .exit_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match rx {
            Some(rx) => Ok(rx.await.unwrap_or(None)),
            None => Err(EngineError::protocol("wait() called twice")),
        }
    }

    async fn close(&self) -> EngineResult<()> {
        // Interrupt a stuck write right here on the calling thread (non-blocking Win32 calls,
        // no COM): the write wakes on `write_cancel`, and `CancelIoEx` also aborts a write
        // blocked synchronously and the reader's pending read.
        self.write_cancel.fire();
        win32::cancel_io(&self.shared.tty);
        let sh = self.shared.clone();
        let _ = self
            .ctl
            .run(move || {
                // SIGHUP like a closed terminal; ignore "already exited".
                // SAFETY: verified IWSLCProcess slot 0.
                let _ = unsafe { sh.process.0.Signal(1) };
                Ok(())
            })
            .await;
        self.cancel.fire();
        Ok(())
    }
}

impl Drop for WslcComTerminal {
    fn drop(&mut self) {
        self.write_cancel.fire();
        self.cancel.fire();
        win32::cancel_io(&self.shared.tty);
    }
}
