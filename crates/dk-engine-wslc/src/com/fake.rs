//! In-process **fake WSLC COM server** (spec 21 §7) implementing the v3_0 `IWSLC*` vtables
//! with `#[implement]`. It hands out memory exactly like the real server: `CoTaskMemAlloc`ed
//! `[out]` strings and arrays, per-entry strings inside `WSLCContainerEntry`, and owned pipe
//! handles for `Logs`. Tests exercise our vtable and struct declarations, memory freeing, and
//! HRESULT mapping without WSL. Marshalling is not covered.
//!
//! Compiled for this crate's tests and with feature `test-support` (used by `tests/com_fake.rs`).
//! Every slot of every interface is implemented, because `#[implement]` requires it; slots the
//! engine never calls return `E_NOTIMPL`.

#![allow(non_snake_case, clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, S_OK};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    WriteFile,
};
use windows::Win32::System::Com::{CoRegisterMallocSpy, IMallocSpy, IMallocSpy_Impl};
use windows::Win32::System::Pipes::{
    CreateNamedPipeW, CreatePipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::{BOOL, HRESULT, IUnknown, PCSTR, PCWSTR, PSTR, PWSTR, implement};

/// The interface `FakeManager::new_interface` hands out (re-exported for tests only; the
/// vtable module itself stays crate-private, ADR-0003).
pub use super::abi::v3_0::IWSLCSessionManager;
use super::abi::v3_0::*;
use super::ffi::{CoTaskMemArray, CoTaskMemStr, CoTaskMemWStr, hr, write_fixed};

const E_NOTIMPL: HRESULT = HRESULT(0x8000_4001_u32 as i32);
const E_POINTER: HRESULT = HRESULT(0x8000_4003_u32 as i32);

/// One fake container.
#[derive(Debug, Clone, Default)]
pub struct FakeContainerData {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    pub status: String,
    /// `k=v,…` as the real server joins them.
    pub labels: String,
    pub networks: String,
    pub mounts: String,
    pub state: i32,
    pub created_at: i64,
    /// (host, container, IPPROTO, binding address)
    pub ports: Vec<(u16, u16, i32, String)>,
    pub inspect_json: String,
    pub stats_json: String,
    pub logs_stdout: String,
    pub logs_stderr: String,
}

/// Server state shared by all fake objects; tests inspect and mutate it.
#[derive(Debug, Default)]
pub struct FakeState {
    pub containers: Vec<FakeContainerData>,
    pub volumes_json: String,
    pub networks_json: String,
    pub sessions: Vec<(u32, String)>,
    pub default_session: String,
    /// Fault injection: positive contradiction after an explicitly named open.
    pub opened_session_override: Option<String>,
    pub version: (u32, u32, u32),
    pub events: Vec<String>,
    /// Every recorded call, in order.
    pub calls: Vec<String>,
    /// Forced HRESULT for the next call of this name (consumed once).
    pub fail_next: BTreeMap<String, i32>,
    /// Fault after observable commit (ENG-130), consumed once.
    pub commit_fault: BTreeMap<String, i32>,
    pub pull_progress_count: usize,
    /// Blocking RPC delay; caller timeout must not cause a second dispatch.
    pub delay_next: BTreeMap<String, std::time::Duration>,
    pub stopped: Vec<(String, i32, i32)>,
    pub killed: Vec<(String, i32)>,
    pub deleted: Vec<(String, i32)>,
    pub created_volumes: Vec<String>,
    /// Signals delivered to exec'd processes (`IWSLCProcess::Signal`).
    pub signalled: Vec<i32>,
}

pub type Shared = Arc<Mutex<FakeState>>;

/// An entry of [`FakeState::events`] that makes `GetNext` return `WSLC_E_EVENTS_LOST` (the
/// server dropped events for a slow reader) instead of an event.
pub const EVENTS_LOST_MARKER: &str = "<events-lost>";

/// Live `BeginContainerOperation` tokens (incremented on hand-out, decremented on release).
pub static OPEN_OPERATIONS: AtomicI64 = AtomicI64::new(0);
/// `CoTaskMemAlloc` blocks handed to the client.
pub static ALLOCATIONS: AtomicU32 = AtomicU32::new(0);

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, FakeState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Records `call`; returns a forced failure if one is queued.
fn record(s: &Shared, call: &str) -> Option<HRESULT> {
    let mut g = lock(s);
    g.calls.push(call.to_owned());
    g.fail_next.remove(call).map(HRESULT)
}

/// # Safety
/// `p` is null or a NUL-terminated string valid for the call.
unsafe fn cstr(p: PCSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: per the function contract.
        unsafe { p.to_string() }.unwrap_or_default()
    }
}

// ───────────── allocation tracking (IMallocSpy) ─────────────

/// Blocks handed out by the fake and not yet freed by the client.
fn live() -> &'static Mutex<HashSet<usize>> {
    static LIVE: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn track(p: *const c_void) {
    if !p.is_null() {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        live()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(p as usize);
    }
}

/// Process-wide COM allocator spy: removes freed blocks from [`live`]. Pass-through otherwise
/// (no header adjustments), so it is safe to leave registered for the process lifetime.
#[implement(IMallocSpy)]
struct Spy;

impl IMallocSpy_Impl for Spy_Impl {
    fn PreAlloc(&self, cbrequest: usize) -> usize {
        cbrequest
    }
    fn PostAlloc(&self, pactual: *const c_void) -> *mut c_void {
        pactual as *mut c_void
    }
    fn PreFree(&self, prequest: *const c_void, _fspyed: BOOL) -> *mut c_void {
        if !prequest.is_null() {
            live()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(prequest as usize));
        }
        prequest as *mut c_void
    }
    fn PostFree(&self, _fspyed: BOOL) {}
    fn PreRealloc(
        &self,
        prequest: *const c_void,
        cbrequest: usize,
        ppnewrequest: *mut *mut c_void,
        _fspyed: BOOL,
    ) -> usize {
        if !ppnewrequest.is_null() {
            // SAFETY: COM passes a valid out slot.
            unsafe { *ppnewrequest = prequest as *mut c_void };
        }
        cbrequest
    }
    fn PostRealloc(&self, pactual: *const c_void, _fspyed: BOOL) -> *mut c_void {
        pactual as *mut c_void
    }
    fn PreGetSize(&self, prequest: *const c_void, _fspyed: BOOL) -> *mut c_void {
        prequest as *mut c_void
    }
    fn PostGetSize(&self, cbactual: usize, _fspyed: BOOL) -> usize {
        cbactual
    }
    fn PreDidAlloc(&self, prequest: *const c_void, _fspyed: BOOL) -> *mut c_void {
        prequest as *mut c_void
    }
    fn PostDidAlloc(&self, _prequest: *const c_void, _fspyed: BOOL, factual: i32) -> i32 {
        factual
    }
    fn PreHeapMinimize(&self) {}
    fn PostHeapMinimize(&self) {}
}

/// Registers the allocator spy once per process (never revoked). Returns whether it is active.
pub fn install_malloc_spy() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let spy: IMallocSpy = Spy.into();
        // SAFETY: registers a pass-through spy; COM keeps a reference for the process lifetime.
        unsafe { CoRegisterMallocSpy(&spy) }.is_ok()
    })
}

/// Number of blocks the fake handed out that the client has not freed yet. Only meaningful
/// after [`install_malloc_spy`] returned `true`.
pub fn live_cotaskmem_blocks() -> usize {
    live().lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn out_str(s: &str) -> PSTR {
    let p = CoTaskMemStr::alloc(s);
    track(p as *const c_void);
    PSTR(p)
}

/// # Safety
/// `out` is null or a writable out-param slot.
unsafe fn put_str(out: *mut PSTR, s: &str) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: per the function contract.
    unsafe { *out = out_str(s) };
    S_OK
}

/// # Safety
/// `out`/`count` are null or writable out-param slots.
unsafe fn put_array<T: Copy>(out: *mut *mut T, count: *mut u32, items: &[T]) -> HRESULT {
    if out.is_null() || count.is_null() {
        return E_POINTER;
    }
    let (p, n) = CoTaskMemArray::alloc(items);
    track(p as *const c_void);
    // SAFETY: per the function contract.
    unsafe {
        *out = p;
        *count = n;
    }
    S_OK
}

/// An anonymous pipe pre-filled with `data` with its write end closed (readers hit EOF after
/// the data), as a `WSLCHandle` of type Pipe, like the real `Logs`.
fn pipe_with(data: &[u8]) -> WSLCHandle {
    let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
    let size = u32::try_from(data.len()).unwrap_or(u32::MAX).max(4096);
    // SAFETY: plain anonymous pipe; both handles owned here.
    if unsafe { CreatePipe(&mut r, &mut w, None, size) }.is_err() {
        return WSLCHandle::default();
    }
    let mut n = 0u32;
    if !data.is_empty() {
        // SAFETY: fresh pipe write handle whose buffer holds `data`.
        let _ = unsafe { WriteFile(w, Some(data), Some(&mut n), None) };
    }
    // SAFETY: closing our write end exactly once so the reader sees EOF.
    let _ = unsafe { CloseHandle(w) };
    WSLCHandle {
        Type: WSLC_HANDLE_TYPE_PIPE,
        Handle: r,
    }
}

/// A connected, **overlapped** duplex named pipe like the real exec TTY handle (spike F-7).
/// Returns `(ours, peer)`: `ours` goes to the client; the fake keeps `peer` and never reads or
/// writes it, so client writes fill the pipe buffer and then pend, and client reads pend until
/// cancelled. Small buffers make "the other side stopped reading" quick to reach.
fn stalled_duplex_pipe() -> Option<(HANDLE, HANDLE)> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        r"\\.\pipe\dk-fake-wslc-tty-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; the handle is owned by the
    // returned tuple (closed by the client / `FakeProcess::drop`).
    let server = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            1024,
            1024,
            0,
            None,
        )
    };
    if server.is_invalid() {
        return None;
    }
    // SAFETY: opens the client end of the pipe created above (same NUL-terminated name).
    let peer = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
    };
    match peer {
        Ok(peer) => Some((server, peer)),
        Err(_) => {
            // SAFETY: closing the server handle created above exactly once.
            let _ = unsafe { CloseHandle(server) };
            None
        }
    }
}

/// An exec'd process (`IWSLCContainer::Exec`): a TTY whose other side never drains input and
/// never produces output (so writes and reads block until cancelled), no exit event, and a
/// state that becomes "signalled" after `Signal`.
#[implement(IWSLCProcess)]
pub struct FakeProcess {
    state: Shared,
    /// Handed out once by `GetStdHandle` (ownership moves to the client).
    tty: Mutex<Option<usize>>,
    /// The never-drained peer end; closed when the process object is released.
    peer: usize,
    signal: Mutex<Option<i32>>,
}

impl Drop for FakeProcess {
    fn drop(&mut self) {
        let leftover = self.tty.lock().unwrap_or_else(|e| e.into_inner()).take();
        for h in leftover.into_iter().chain(std::iter::once(self.peer)) {
            // SAFETY: handles created by `stalled_duplex_pipe`, still owned by us, closed once.
            let _ = unsafe { CloseHandle(HANDLE(h as *mut c_void)) };
        }
    }
}

impl IWSLCProcess_Impl for FakeProcess_Impl {
    unsafe fn Signal(&self, Signal: i32) -> HRESULT {
        record(&self.state, "Signal");
        lock(&self.state).signalled.push(Signal);
        *self.signal.lock().unwrap_or_else(|e| e.into_inner()) = Some(Signal);
        S_OK
    }
    unsafe fn GetExitEvent(&self, _EventHandle: *mut HANDLE) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetStdHandle(&self, Fd: i32, Handle: *mut WSLCHandle) -> HRESULT {
        if let Some(h) = record(&self.state, "GetStdHandle") {
            return h;
        }
        if Handle.is_null() {
            return E_POINTER;
        }
        if Fd != WSLC_FD_TTY && Fd != WSLC_FD_STDOUT {
            return E_NOTIMPL;
        }
        let Some(h) = self.tty.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return E_POINTER;
        };
        // SAFETY: out-param per IDL; handle ownership transfers to the caller.
        unsafe {
            *Handle = WSLCHandle {
                Type: WSLC_HANDLE_TYPE_PIPE,
                Handle: HANDLE(h as *mut c_void),
            }
        };
        lock(&self.state)
            .commit_fault
            .remove("GetStdHandle")
            .map_or(S_OK, HRESULT)
    }
    unsafe fn GetFlags(&self, _Flags: *mut i32) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetPid(&self, _Pid: *mut i32) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetState(&self, State: *mut i32, Code: *mut i32) -> HRESULT {
        if State.is_null() || Code.is_null() {
            return E_POINTER;
        }
        let sig = *self.signal.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: out-params per IDL.
        unsafe {
            match sig {
                Some(n) => {
                    *State = WSLC_PROCESS_STATE_SIGNALLED;
                    *Code = n;
                }
                None => {
                    *State = WSLC_PROCESS_STATE_RUNNING;
                    *Code = 0;
                }
            }
        }
        S_OK
    }
    unsafe fn ResizeTty(&self, _Rows: u32, _Columns: u32) -> HRESULT {
        record(&self.state, "ResizeTty");
        S_OK
    }
}

/// `BeginContainerOperation` token.
#[implement]
struct OpToken;

fn op_token() -> IUnknown {
    OPEN_OPERATIONS.fetch_add(1, Ordering::SeqCst);
    OpToken.into()
}

impl Drop for OpToken {
    fn drop(&mut self) {
        OPEN_OPERATIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[implement(IWSLCSessionManager)]
pub struct FakeManager {
    state: Shared,
}

#[implement(IWSLCSession)]
pub struct FakeSession {
    state: Shared,
    name: String,
}

#[implement(IWSLCContainer)]
pub struct FakeContainer {
    state: Shared,
    id: String,
}

#[implement(IWSLCEventStream)]
pub struct FakeEvents {
    state: Shared,
    next: AtomicU32,
}

impl FakeManager {
    /// The fake manager as the interface `WslcComEngine` consumes.
    pub fn new_interface(state: Shared) -> IWSLCSessionManager {
        FakeManager { state }.into()
    }
}

fn find(state: &Shared, id: &str) -> Option<FakeContainerData> {
    lock(state)
        .containers
        .iter()
        .find(|c| c.id == id || (id.len() >= 4 && c.id.starts_with(id)) || c.name == id)
        .cloned()
}

// ───────────────────────────── IWSLCSessionManager ─────────────────────────────

impl IWSLCSessionManager_Impl for FakeManager_Impl {
    unsafe fn GetVersion(&self, Version: *mut WSLCVersion) -> HRESULT {
        if let Some(h) = record(&self.state, "GetVersion") {
            return h;
        }
        if Version.is_null() {
            return E_POINTER;
        }
        let (a, b, c) = lock(&self.state).version;
        // SAFETY: valid out struct per IDL.
        unsafe {
            *Version = WSLCVersion {
                Major: a,
                Minor: b,
                Revision: c,
            }
        };
        S_OK
    }
    unsafe fn CreateSession(
        &self,
        _Settings: *const c_void,
        _Flags: i32,
        _WarningCallback: *mut c_void,
        _Session: *mut Option<IWSLCSession>,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn EnterSession(
        &self,
        _DisplayName: PCWSTR,
        _StoragePath: PCWSTR,
        _WarningCallback: *mut c_void,
        _Session: *mut Option<IWSLCSession>,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn ListSessions(
        &self,
        Sessions: *mut *mut WSLCSessionListEntry,
        SessionsCount: *mut u32,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "ListSessions") {
            return h;
        }
        let items: Vec<WSLCSessionListEntry> = lock(&self.state)
            .sessions
            .iter()
            .map(|(id, name)| {
                let mut e = WSLCSessionListEntry {
                    SessionId: *id,
                    CreatorPid: 1234,
                    DisplayName: [0; 256],
                    Sid: [0; 257],
                };
                for (i, u) in name.encode_utf16().take(255).enumerate() {
                    e.DisplayName[i] = u;
                }
                for (i, u) in "S-1-5-21-1-2-3-1001".encode_utf16().enumerate() {
                    e.Sid[i] = u;
                }
                e
            })
            .collect();
        // SAFETY: out-params per IDL.
        unsafe { put_array(Sessions, SessionsCount, &items) }
    }
    unsafe fn OpenSession(&self, _Id: u32, _Session: *mut Option<IWSLCSession>) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn OpenSessionByName(
        &self,
        DisplayName: PCWSTR,
        Session: *mut Option<IWSLCSession>,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "OpenSessionByName") {
            return h;
        }
        let wanted = if DisplayName.is_null() {
            lock(&self.state).default_session.clone()
        } else {
            // SAFETY: NUL-terminated wide string per IDL.
            unsafe { DisplayName.to_string() }.unwrap_or_default()
        };
        if !lock(&self.state).sessions.iter().any(|(_, n)| *n == wanted) {
            return HRESULT(hr::WSLC_E_SESSION_NOT_FOUND);
        }
        let s: IWSLCSession = FakeSession {
            state: self.state.clone(),
            name: lock(&self.state)
                .opened_session_override
                .clone()
                .unwrap_or(wanted),
        }
        .into();
        // SAFETY: out-param per IDL.
        unsafe { *Session = Some(s) };
        S_OK
    }
}

// ───────────────────────────── IWSLCSession ─────────────────────────────

impl IWSLCSession_Impl for FakeSession_Impl {
    unsafe fn GetId(&self, Id: *mut u32) -> HRESULT {
        if let Some(h) = record(&self.state, "GetSessionId") {
            return h;
        }
        if Id.is_null() {
            return E_POINTER;
        }
        let id = lock(&self.state)
            .sessions
            .iter()
            .find(|(_, name)| name == &self.name)
            .map(|(id, _)| *id);
        let Some(id) = id else {
            return HRESULT(hr::WSLC_E_SESSION_NOT_FOUND);
        };
        // SAFETY: writable out slot per IDL.
        unsafe {
            *Id = id;
        }
        S_OK
    }
    unsafe fn GetDisplayName(&self, DisplayName: *mut PWSTR) -> HRESULT {
        if let Some(h) = record(&self.state, "GetDisplayName") {
            return h;
        }
        if DisplayName.is_null() {
            return E_POINTER;
        }
        let p = CoTaskMemWStr::alloc(&self.name);
        track(p as *const c_void);
        // SAFETY: out-param per IDL.
        unsafe { *DisplayName = PWSTR(p) };
        S_OK
    }
    unsafe fn GetState(&self, State: *mut i32) -> HRESULT {
        if let Some(h) = record(&self.state, "GetState") {
            return h;
        }
        // SAFETY: out-param per IDL.
        unsafe { *State = WSLC_SESSION_STATE_RUNNING };
        S_OK
    }
    unsafe fn GetTerminationEvent(&self, _Event: *mut HANDLE) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetTerminationReason(&self, _Reason: *mut i32, _Details: *mut PWSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetEvents(
        &self,
        _SinceTime: i64,
        _UntilTime: i64,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        Stream: *mut Option<IWSLCEventStream>,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "GetEvents") {
            return h;
        }
        let s: IWSLCEventStream = FakeEvents {
            state: self.state.clone(),
            next: AtomicU32::new(0),
        }
        .into();
        // SAFETY: out-param per IDL.
        unsafe { *Stream = Some(s) };
        S_OK
    }
    unsafe fn PullImage(
        &self,
        _Image: PCSTR,
        _RegistryAuthenticationInformation: PCSTR,
        _AllTags: BOOL,
        ProgressCallback: *mut c_void,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "PullImage") {
            return h;
        }
        let count = lock(&self.state).pull_progress_count;
        if !ProgressCallback.is_null() {
            // SAFETY: borrowed callback pointer per IDL remains alive for this RPC.
            let cb = unsafe {
                <IProgressCallback as windows::core::Interface>::from_raw_borrowed(
                    &ProgressCallback,
                )
            }
            .expect("non-null callback pointer per IDL");
            for i in 0..count {
                // SAFETY: static NUL terminated strings, borrowed callback valid through call.
                let r = unsafe {
                    cb.OnProgress(
                        PCSTR(c"Downloading".as_ptr().cast()),
                        PCSTR(c"layer".as_ptr().cast()),
                        i as u64,
                        count as u64,
                    )
                };
                if r.is_err() {
                    return r;
                }
            }
        }
        lock(&self.state).calls.push("PullImageFinished".into());
        if let Some(h) = lock(&self.state).commit_fault.remove("PullImage") {
            return HRESULT(h);
        }
        S_OK
    }
    unsafe fn BuildImage(
        &self,
        _Options: *const c_void,
        _ProgressCallback: *mut c_void,
        _CancelEvent: HANDLE,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn LoadImage(
        &self,
        _ImageHandle: WSLCHandle,
        _ContentLength: u64,
        _WarningCallback: *mut c_void,
        _LoadCallback: *mut c_void,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn ImportImage(
        &self,
        _ImageHandle: WSLCHandle,
        _ImageName: PCSTR,
        _ContentLength: u64,
        _WarningCallback: *mut c_void,
        _ImageId: *mut PSTR,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn SaveImage(
        &self,
        _OutputHandle: WSLCHandle,
        _ImageNameOrID: PCSTR,
        _ProgressCallback: *mut c_void,
        _CancelEvent: HANDLE,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn SaveImages(
        &self,
        _OutputHandle: WSLCHandle,
        _ImageNames: *const WSLCStringArray,
        _ProgressCallback: *mut c_void,
        _CancelEvent: HANDLE,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn ListImages(
        &self,
        _Options: *const WSLCListImagesOptions,
        Images: *mut *mut WSLCImageInformation,
        Count: *mut u32,
    ) -> HRESULT {
        let rows = [
            ("nginx:alpine", "sha256:3dd08163706a", 62_941_990i64),
            ("nginx:latest", "sha256:3dd08163706a", 62_941_990),
            ("busybox:1.37", "sha256:30ecbe150909", 4_420_000),
        ];
        let items: Vec<WSLCImageInformation> = rows
            .iter()
            .map(|(img, hash, size)| {
                // SAFETY: all-zero is a valid value of this plain struct.
                let mut i: WSLCImageInformation = unsafe { std::mem::zeroed() };
                write_fixed(&mut i.Image, img);
                write_fixed(&mut i.Hash, hash);
                i.Size = *size;
                i.Created = 1_758_000_000;
                i.Containers = 1;
                i
            })
            .collect();
        // SAFETY: out-params per IDL.
        unsafe { put_array(Images, Count, &items) }
    }
    unsafe fn DeleteImage(
        &self,
        _Options: *const WSLCDeleteImageOptions,
        _DeletedImages: *mut *mut WSLCDeletedImageInformation,
        _Count: *mut u32,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn TagImage(&self, _Options: *const WSLCTagImageOptions) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn InspectImage(&self, _ImageNameOrId: PCSTR, _Output: *mut PSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn PruneImages(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        _DeletedImages: *mut *mut WSLCDeletedImageInformation,
        _DeletedImagesCount: *mut u32,
        _SpaceReclaimed: *mut u64,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn CreateContainer(
        &self,
        _Options: *const WSLCContainerOptions,
        _WarningCallback: *mut c_void,
        _Container: *mut Option<IWSLCContainer>,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn OpenContainer(&self, Id: PCSTR, Container: *mut Option<IWSLCContainer>) -> HRESULT {
        // SAFETY: NUL-terminated id per IDL `[in, ref] LPCSTR`.
        let id = unsafe { cstr(Id) };
        if let Some(h) = record(&self.state, "OpenContainer") {
            return h;
        }
        match find(&self.state, &id) {
            Some(c) => {
                let obj: IWSLCContainer = FakeContainer {
                    state: self.state.clone(),
                    id: c.id,
                }
                .into();
                // SAFETY: out-param per IDL.
                unsafe { *Container = Some(obj) };
                S_OK
            }
            None => HRESULT(hr::WSLC_E_CONTAINER_NOT_FOUND),
        }
    }
    unsafe fn ListContainers(
        &self,
        Options: *const WSLCListContainersOptions,
        Containers: *mut *mut WSLCContainerEntry,
        Count: *mut u32,
        Ports: *mut *mut WSLCContainerPortMapping,
        PortsCount: *mut u32,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "ListContainers") {
            return h;
        }
        // SAFETY: `Options` is null or valid per IDL `[in, unique]`.
        let all =
            Options.is_null() || unsafe { (*Options).Flags } & WSLC_LIST_CONTAINERS_FLAGS_ALL != 0;
        let g = lock(&self.state);
        let mut entries = Vec::new();
        let mut ports = Vec::new();
        for c in g
            .containers
            .iter()
            .filter(|c| all || c.state == WSLC_CONTAINER_STATE_RUNNING)
        {
            // SAFETY: all-zero is valid (char arrays, null pointers, integers).
            let mut e: WSLCContainerEntry = unsafe { std::mem::zeroed() };
            write_fixed(&mut e.Name, &c.name);
            write_fixed(&mut e.Image, &c.image);
            write_fixed(&mut e.Id, &c.id);
            e.Command = out_str(&c.command);
            e.Status = out_str(&c.status);
            e.Labels = out_str(&c.labels);
            e.Networks = out_str(&c.networks);
            // Real servers may leave fields null; exercise that path for empty mounts.
            e.Mounts = if c.mounts.is_empty() {
                PSTR::null()
            } else {
                out_str(&c.mounts)
            };
            e.CreatedAt = c.created_at;
            e.StateChangedAt = c.created_at + 60;
            e.SizeRw = -1;
            e.State = c.state;
            entries.push(e);
            for (host, cont, proto, addr) in &c.ports {
                // SAFETY: all-zero is valid for this plain struct.
                let mut m: WSLCContainerPortMapping = unsafe { std::mem::zeroed() };
                write_fixed(&mut m.Id, &c.id);
                m.PortMapping.HostPort = *host;
                m.PortMapping.ContainerPort = *cont;
                m.PortMapping.Family = 2;
                m.PortMapping.Protocol = *proto;
                write_fixed(&mut m.PortMapping.BindingAddress, addr);
                ports.push(m);
            }
        }
        drop(g);
        // SAFETY: out-params per IDL.
        unsafe {
            let r = put_array(Containers, Count, &entries);
            if r != S_OK {
                return r;
            }
            put_array(Ports, PortsCount, &ports)
        }
    }
    unsafe fn PruneContainers(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        Result: *mut WSLCPruneContainersResults,
    ) -> HRESULT {
        record(&self.state, "PruneContainers");
        // SAFETY: all-zero is a valid id buffer.
        let mut id: WSLCContainerId = unsafe { std::mem::zeroed() };
        write_fixed(&mut id, "da29ef747f13");
        let (p, n) = CoTaskMemArray::alloc(&[id]);
        track(p as *const c_void);
        // SAFETY: out struct per IDL.
        unsafe {
            *Result = WSLCPruneContainersResults {
                Containers: p,
                ContainersCount: n,
                SpaceReclaimed: 4096,
            };
        }
        S_OK
    }
    unsafe fn CreateRootNamespaceProcess(
        &self,
        _Executable: PCSTR,
        _Options: *const WSLCProcessOptions,
        _TtyRows: u32,
        _TtyColumns: u32,
        _AcquireVmLease: BOOL,
        _Process: *mut Option<IWSLCProcess>,
        _Errno: *mut i32,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn FormatVirtualDisk(&self, _Path: PCWSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Terminate(&self) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn MountWindowsFolder(
        &self,
        _WindowsPath: PCWSTR,
        _LinuxPath: PCSTR,
        _ReadOnly: BOOL,
        _AcquireVmLease: BOOL,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn UnmountWindowsFolder(&self, _LinuxPath: PCSTR, _AcquireVmLease: BOOL) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn MapVmPort(&self, _Family: i32, _WindowsPort: u16, _LinuxPort: u16) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn UnmapVmPort(&self, _Family: i32, _WindowsPort: u16, _LinuxPort: u16) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetProcessHandle(&self, _ProcessHandle: *mut HANDLE) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Initialize(
        &self,
        _Settings: *const c_void,
        _VmFactory: *mut c_void,
        _PluginNotifier: *mut c_void,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn CreateVolume(
        &self,
        Options: *const WSLCVolumeOptions,
        VolumeInfo: *mut WSLCVolumeInformation,
    ) -> HRESULT {
        if Options.is_null() || VolumeInfo.is_null() {
            return E_POINTER;
        }
        // SAFETY: valid in struct per IDL; Name is `[unique]`.
        let name = unsafe { cstr((*Options).Name) };
        let name = if name.is_empty() {
            "anon0001".to_owned()
        } else {
            name
        };
        lock(&self.state).created_volumes.push(name.clone());
        // SAFETY: valid out struct per IDL.
        unsafe {
            write_fixed(&mut (*VolumeInfo).Name, &name);
            write_fixed(&mut (*VolumeInfo).Driver, "guest");
        }
        S_OK
    }
    unsafe fn DeleteVolume(&self, _Name: PCSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn ListVolumes(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        Output: *mut PSTR,
    ) -> HRESULT {
        let j = lock(&self.state).volumes_json.clone();
        // SAFETY: out-param per IDL.
        unsafe { put_str(Output, &j) }
    }
    unsafe fn InspectVolume(&self, Name: PCSTR, Output: *mut PSTR) -> HRESULT {
        // SAFETY: NUL-terminated name per IDL.
        let name = unsafe { cstr(Name) };
        if name == "webdata" {
            // SAFETY: out-param per IDL.
            unsafe {
                put_str(
                    Output,
                    r#"{"CreatedAt":"2026-10-01T22:00:00Z","Driver":"guest","Labels":null,"Mountpoint":"/var/lib/docker/volumes/webdata/_data","Name":"webdata","Options":null,"Scope":"local"}"#,
                )
            }
        } else {
            HRESULT(hr::WSLC_E_VOLUME_NOT_FOUND)
        }
    }
    unsafe fn Authenticate(
        &self,
        _ServerAddress: PCSTR,
        _Username: PCSTR,
        _Password: PCSTR,
        _IdentityToken: *mut PSTR,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn PushImage(
        &self,
        _Image: PCSTR,
        _RegistryAuthenticationInformation: PCSTR,
        _AllTags: BOOL,
        _ProgressCallback: *mut c_void,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn PruneVolumes(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        _WarningCallback: *mut c_void,
        _Volumes: *mut *mut WSLCVolumeName,
        _VolumesCount: *mut u32,
        _SpaceReclaimed: *mut u64,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn CreateNetwork(
        &self,
        _Options: *const c_void,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn DeleteNetwork(&self, _Name: PCSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn ListNetworks(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        Output: *mut PSTR,
    ) -> HRESULT {
        let j = lock(&self.state).networks_json.clone();
        // SAFETY: out-param per IDL.
        unsafe { put_str(Output, &j) }
    }
    unsafe fn InspectNetwork(&self, _Name: PCSTR, _Output: *mut PSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn PruneNetworks(
        &self,
        _Filters: *const WSLCFilter,
        _FiltersCount: u32,
        _Networks: *mut *mut WSLCNetworkName,
        _NetworksCount: *mut u32,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn RegisterCrashDumpCallback(
        &self,
        _Callback: *mut c_void,
        _Subscription: *mut *mut c_void,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn TriggerIdleTermination(&self, _WasAlreadyIdle: *mut BOOL) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn BeginContainerOperation(&self, Operation: *mut Option<IUnknown>) -> HRESULT {
        if let Some(h) = record(&self.state, "BeginContainerOperation") {
            return h;
        }
        // SAFETY: out-param per IDL.
        unsafe { *Operation = Some(op_token()) };
        S_OK
    }
    unsafe fn SetNetworkFaultsForTest(&self, _FailCreateInspect: BOOL) -> HRESULT {
        E_NOTIMPL
    }
}

// ───────────────────────────── IWSLCContainer ─────────────────────────────

fn is_running(state: &Shared, id: &str) -> bool {
    lock(state)
        .containers
        .iter()
        .any(|c| c.id == id && c.state == WSLC_CONTAINER_STATE_RUNNING)
}

impl IWSLCContainer_Impl for FakeContainer_Impl {
    unsafe fn Attach(
        &self,
        _DetachKeys: PCSTR,
        _StdIn: *mut WSLCHandle,
        _StdOut: *mut WSLCHandle,
        _StdErr: *mut WSLCHandle,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Stop(&self, Signal: i32, TimeoutSeconds: i32) -> HRESULT {
        if let Some(h) = record(&self.state, "Stop") {
            return h;
        }
        let delay = lock(&self.state).delay_next.remove("Stop");
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
        let running = is_running(&self.state, &self.id);
        lock(&self.state)
            .stopped
            .push((self.id.clone(), Signal, TimeoutSeconds));
        if let Some(h) = lock(&self.state).commit_fault.remove("Stop") {
            return HRESULT(h);
        }
        if running {
            S_OK
        } else {
            HRESULT(hr::WSLC_E_CONTAINER_NOT_RUNNING)
        }
    }
    unsafe fn Start(
        &self,
        _Flags: i32,
        _StartOptions: *const WSLCProcessStartOptions,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        record(&self.state, "Start");
        if is_running(&self.state, &self.id) {
            HRESULT(hr::WSLC_E_CONTAINER_IS_RUNNING)
        } else {
            S_OK
        }
    }
    unsafe fn Delete(&self, Flags: i32) -> HRESULT {
        record(&self.state, "Delete");
        if is_running(&self.state, &self.id) && Flags & WSLC_DELETE_FLAGS_FORCE == 0 {
            return HRESULT(hr::WSLC_E_CONTAINER_IS_RUNNING);
        }
        lock(&self.state).deleted.push((self.id.clone(), Flags));
        S_OK
    }
    unsafe fn Export(&self, _TarHandle: WSLCHandle) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetState(&self, _State: *mut i32) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetInitProcess(&self, _Process: *mut Option<IWSLCProcess>) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Exec(
        &self,
        _Options: *const WSLCProcessOptions,
        _StartOptions: *const WSLCProcessStartOptions,
        Process: *mut Option<IWSLCProcess>,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "Exec") {
            return h;
        }
        if Process.is_null() {
            return E_POINTER;
        }
        if !is_running(&self.state, &self.id) {
            return HRESULT(hr::WSLC_E_CONTAINER_NOT_RUNNING);
        }
        let Some((ours, peer)) = stalled_duplex_pipe() else {
            return HRESULT(hr::E_FAIL);
        };
        let p: IWSLCProcess = FakeProcess {
            state: self.state.clone(),
            tty: Mutex::new(Some(ours.0 as usize)),
            peer: peer.0 as usize,
            signal: Mutex::new(None),
        }
        .into();
        // SAFETY: out-param per IDL.
        unsafe { *Process = Some(p) };
        S_OK
    }
    unsafe fn Inspect(&self, _Size: BOOL, Output: *mut PSTR) -> HRESULT {
        match find(&self.state, &self.id) {
            // SAFETY: out-param per IDL.
            Some(c) => unsafe { put_str(Output, &c.inspect_json) },
            None => HRESULT(hr::WSLC_E_CONTAINER_NOT_FOUND),
        }
    }
    unsafe fn Logs(
        &self,
        _Flags: i32,
        Stdout: *mut WSLCHandle,
        Stderr: *mut WSLCHandle,
        _Since: i64,
        _Until: i64,
        _Tail: u64,
    ) -> HRESULT {
        if let Some(h) = record(&self.state, "Logs") {
            return h;
        }
        let Some(c) = find(&self.state, &self.id) else {
            return HRESULT(hr::WSLC_E_CONTAINER_NOT_FOUND);
        };
        // SAFETY: out-params per IDL; handle ownership transfers to the caller.
        unsafe {
            *Stdout = pipe_with(c.logs_stdout.as_bytes());
            *Stderr = pipe_with(c.logs_stderr.as_bytes());
        }
        S_OK
    }
    unsafe fn GetId(&self, _Id: *mut u8) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetName(&self, _Name: *mut PSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn GetLabels(
        &self,
        _Labels: *mut *mut KeyValuePairInformation,
        _Count: *mut u32,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Kill(&self, Signal: i32) -> HRESULT {
        record(&self.state, "Kill");
        lock(&self.state).killed.push((self.id.clone(), Signal));
        S_OK
    }
    unsafe fn Stats(&self, Output: *mut PSTR) -> HRESULT {
        if let Some(h) = record(&self.state, "Stats") {
            return h;
        }
        match find(&self.state, &self.id) {
            // SAFETY: out-param per IDL.
            Some(c) => unsafe { put_str(Output, &c.stats_json) },
            None => HRESULT(hr::WSLC_E_CONTAINER_NOT_FOUND),
        }
    }
    unsafe fn ConnectToNetwork(&self, _Options: *const WSLCNetworkConnectionOptions) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn DisconnectFromNetwork(&self, _NetworkName: PCSTR) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn UploadArchive(
        &self,
        _TarHandle: WSLCHandle,
        _DestPath: PCSTR,
        _ContentSize: u64,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn DownloadArchive(
        &self,
        _SrcPath: PCSTR,
        _FollowLink: BOOL,
        _OutHandle: WSLCHandle,
    ) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn Restart(
        &self,
        _Signal: i32,
        _TimeoutSeconds: i32,
        _WarningCallback: *mut c_void,
    ) -> HRESULT {
        record(&self.state, "Restart");
        S_OK
    }
}

// ───────────────────────────── IWSLCEventStream ─────────────────────────────

impl IWSLCEventStream_Impl for FakeEvents_Impl {
    unsafe fn GetNext(&self, CancelEvent: HANDLE, EventJson: *mut PSTR) -> HRESULT {
        let i = self.next.fetch_add(1, Ordering::SeqCst) as usize;
        let ev = lock(&self.state).events.get(i).cloned();
        if ev.as_deref() == Some(EVENTS_LOST_MARKER) {
            return HRESULT(hr::WSLC_E_EVENTS_LOST);
        }
        if let Some(ev) = ev {
            // SAFETY: out-param per IDL.
            return unsafe { put_str(EventJson, &ev) };
        }
        if CancelEvent.0.is_null() {
            return HRESULT(hr::WSLC_E_EVENT_STREAM_FINISHED);
        }
        // Block until cancelled, like the real server (bounded so a broken test can't hang).
        // SAFETY: the caller keeps the event handle valid for the call's duration.
        unsafe { WaitForSingleObject(CancelEvent, 10_000) };
        HRESULT(hr::E_ABORT)
    }
}

// ───────────────────────────── fixture ─────────────────────────────

/// Canned state resembling the dev machine (WSL 3.0.1 with one running nginx and one exited
/// container). Shapes come from the real server: spike F-6/F-7 and the `tests/com_live.rs` run.
pub fn sample_state() -> Shared {
    let inspect = r#"{"Id":"2e4fac8842183f22f337722c3648e5fd5cd5012ae01ea7651948b59f70f87365","Name":"/dk-fixture-nginx","Created":"2026-10-01T22:41:52.118Z","Image":"nginx:alpine","State":{"Status":"running","Running":true,"ExitCode":0,"StartedAt":"2026-10-01T23:10:05Z","FinishedAt":"0001-01-01T00:00:00Z"},"HostConfig":{"NetworkMode":"bridge","Memory":0,"NanoCpus":0},"Config":{"Image":"nginx:alpine","Env":["PATH=/usr/bin"],"Cmd":["nginx","-g","daemon off;"],"Labels":{"com.example.fixture":"dockering"}},"Ports":{"80/tcp":[{"HostIp":"127.0.0.1","HostPort":"18081"}]},"Mounts":[],"Labels":{"com.example.fixture":"dockering"},"NetworkSettings":{"Networks":{"bridge":{"IPAddress":"172.17.0.2"}}}}"#;
    let stats = r#"{"read":"2026-10-02T01:15:07.7733477Z","id":"2e4fac884218","name":"dk-fixture-nginx","cpu_stats":{"cpu_usage":{"total_usage":200000000},"system_cpu_usage":1000000000000,"online_cpus":20},"precpu_stats":{"cpu_usage":{"total_usage":100000000},"system_cpu_usage":999000000000,"online_cpus":20},"memory_stats":{"usage":20000000,"limit":16670859264,"stats":{"inactive_file":1903872}},"networks":{"eth0":{"rx_bytes":1436,"tx_bytes":0}},"blkio_stats":{"io_service_bytes_recursive":[{"op":"read","value":8192},{"op":"write","value":8192}]},"pids_stats":{"current":21}}"#;
    Arc::new(Mutex::new(FakeState {
        containers: vec![
            FakeContainerData {
                id: "2e4fac8842183f22f337722c3648e5fd5cd5012ae01ea7651948b59f70f87365".into(),
                name: "dk-fixture-nginx".into(),
                image: "nginx:alpine".into(),
                command: "/docker-entrypoint.sh nginx -g \"daemon off;\"".into(),
                status: "Up 2 hours".into(),
                labels: "com.example.fixture=dockering,com.docker.compose.project=web".into(),
                networks: "bridge".into(),
                mounts: "webdata".into(),
                state: WSLC_CONTAINER_STATE_RUNNING,
                created_at: 1_759_358_512,
                ports: vec![(18081, 80, 6, "127.0.0.1".into())],
                inspect_json: inspect.into(),
                stats_json: stats.into(),
                logs_stdout: "2026-10-01T23:10:05.105706664Z hello\n2026-10-01T23:10:06Z world\n"
                    .into(),
                logs_stderr: "2026-10-01T23:10:05.2Z warn\n".into(),
            },
            FakeContainerData {
                id: "da29ef747f13aaaabbbbccccddddeeeeffff0000111122223333444455556666".into(),
                name: "stopped-one".into(),
                image: "busybox:1.37".into(),
                command: "sh".into(),
                status: "Exited (137) 5 minutes ago".into(),
                state: WSLC_CONTAINER_STATE_EXITED,
                created_at: 1_759_300_000,
                inspect_json: "{}".into(),
                stats_json: "{}".into(),
                ..Default::default()
            },
        ],
        volumes_json: r#"[{"Driver":"guest","Labels":{},"Mountpoint":"/var/lib/docker/volumes/webdata/_data","Name":"webdata","Scope":"local"}]"#.into(),
        networks_json: r#"[{"Created":"2026-10-01T22:08:52.323746043Z","Driver":"bridge","EnableIPv4":true,"EnableIPv6":false,"Id":"7986d99c1b8a","Internal":false,"Labels":{},"Name":"bridge","Scope":"local"}]"#.into(),
        sessions: vec![
            (1, "wslc-cli-admin-tester".into()),
            (2, "wslc-cli-tester".into()),
        ],
        default_session: "wslc-cli-tester".into(),
        version: (3, 0, 1),
        events: vec![
            r#"{"Type":"container","Action":"start","Actor":{"ID":"2e4fac884218","Attributes":{"name":"dk-fixture-nginx"}},"time":1759400000}"#.into(),
        ],
        ..Default::default()
    }))
}
