//! Safe wrappers over Win32 calls that need `unsafe` (ADR-0003: `unsafe` lives only under
//! `src/com/`). No COM here: file version resources, the registry, the process list, events,
//! and handle I/O. Every function is infallible-by-`Option`/`Result` and never panics.

#![cfg(windows)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, ReadFile, VS_FIXEDFILEINFO, VerQueryValueW,
    WriteFile,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{PCWSTR, w};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

// ───────────────────────────── version resources ─────────────────────────────

/// `VS_FIXEDFILEINFO.dwFileVersionMS/LS` of `path` → `(a, b, c, d)`.
pub fn file_version(path: &Path) -> Option<(u32, u32, u32, u32)> {
    let name = wide_path(path);
    let name = PCWSTR(name.as_ptr());
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call.
    let size = unsafe { GetFileVersionInfoSizeW(name, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` has exactly `size` writable bytes, as the API requires.
    unsafe { GetFileVersionInfoW(name, None, size, data.as_mut_ptr().cast()) }.ok()?;
    let mut info: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `data` holds a version resource filled above; "\" selects the root
    // VS_FIXEDFILEINFO. On success `info` points *into* `data` (no separate allocation).
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut len) };
    if !ok.as_bool() || info.is_null() || (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: `info` points to at least `size_of::<VS_FIXEDFILEINFO>()` bytes inside `data`,
    // which is alive; `read_unaligned` avoids assuming alignment of the resource blob.
    let fixed = unsafe { std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO) };
    if fixed.dwSignature != 0xFEEF_04BD {
        return None;
    }
    Some((
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xFFFF,
        fixed.dwFileVersionLS >> 16,
        fixed.dwFileVersionLS & 0xFFFF,
    ))
}

// ───────────────────────────── registry ─────────────────────────────

/// `REG_SZ` value `value` under `HKLM\subkey`, if present.
pub fn reg_string_hklm(subkey: &str, value: &str) -> Option<String> {
    let k = wide(subkey);
    let v = wide(value);
    let mut bytes = 0u32;
    // SAFETY: size query: no data buffer, `bytes` receives the required size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if rc != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    let mut cb = (buf.len() * 2) as u32;
    // SAFETY: `buf` has `cb` writable bytes; RegGetValueW NUL-terminates REG_SZ data.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut cb),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// Whether `HKLM\subkey` exists and is readable.
pub fn reg_key_exists_hklm(subkey: &str) -> bool {
    let k = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: `k` is NUL-terminated; `key` receives an owned handle closed below.
    let rc: WIN32_ERROR = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if rc == ERROR_SUCCESS {
        // SAFETY: `key` was opened successfully above and is closed exactly once.
        let _ = unsafe { RegCloseKey(key) };
        true
    } else {
        false
    }
}

// ───────────────────────────── processes ─────────────────────────────

/// Whether any process with the image name `exe` (case-insensitive, e.g. `wslcsession.exe`)
/// is running. Uses a Toolhelp snapshot: no COM, no process handles opened (ENG-020).
pub fn process_running(exe: &str) -> bool {
    // SAFETY: plain snapshot creation; the handle is wrapped in `OwnedHandle` right away.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let Some(snap) = OwnedHandle::new(snap) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `snap` is a valid snapshot handle; `entry.dwSize` is initialised as required.
    let mut more = unsafe { Process32FirstW(snap.raw(), &mut entry) }.is_ok();
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if name.eq_ignore_ascii_case(exe) {
            return true;
        }
        // SAFETY: as above; the snapshot stays alive for the loop.
        more = unsafe { Process32NextW(snap.raw(), &mut entry) }.is_ok();
    }
    false
}

// ───────────────────────────── identity ─────────────────────────────

/// String SID of the current process user (`S-1-5-21-…`), used to keep only the caller's
/// WSLC sessions (ENG-109). `None` on failure.
pub fn current_user_sid() -> Option<String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle of the current process; `token` is owned below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle::new(token)?;
    let mut len = 0u32;
    // SAFETY: size query (expected to fail with ERROR_INSUFFICIENT_BUFFER, setting `len`).
    let _ = unsafe { GetTokenInformation(token.raw(), TokenUser, None, 0, &mut len) };
    if len == 0 {
        return None;
    }
    // u64 buffer for pointer alignment of TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` has at least `len` bytes, suitably aligned for TOKEN_USER.
    unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
    }
    .ok()?;
    // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER at the start of `buf`; the SID
    // it points to lives inside `buf`.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let mut out = windows::core::PWSTR::null();
    // SAFETY: valid SID pointer; `out` is LocalAlloc'ed by the API and freed below.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut out) }.ok()?;
    // SAFETY: `out` is a NUL-terminated wide string from the API.
    let s = unsafe { out.to_string() }.ok();
    // SAFETY: frees the LocalAlloc'ed string exactly once.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.0.cast())));
    }
    s
}

// ───────────────────────────── handles & events ─────────────────────────────

/// Type-correct owned resource. Kernel events/files/pipes use CloseHandle; WSLC sockets
/// MUST use closesocket. Borrowers retain ownership until overlapped completion is drained.
#[derive(Debug)]
pub struct OwnedHandle {
    handle: HANDLE,
    socket: bool,
}

// SAFETY: a kernel handle value is just an index into the process handle table; Win32 handle
// operations are thread-safe. Ownership (who closes) is tracked by this type.
unsafe impl Send for OwnedHandle {}
// SAFETY: `&OwnedHandle` only exposes the raw value for thread-safe Win32 calls.
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub(crate) fn pair(
        a: super::abi::v3_0::WSLCHandle,
        mut b: super::abi::v3_0::WSLCHandle,
    ) -> dk_core::EngineResult<(Option<Self>, Option<Self>)> {
        use super::abi::v3_0::*;
        fn category(tag: i32) -> Option<bool> {
            match tag {
                WSLC_HANDLE_TYPE_FILE | WSLC_HANDLE_TYPE_PIPE => Some(false),
                WSLC_HANDLE_TYPE_SOCKET => Some(true),
                _ => None,
            }
        }
        if a.Handle == b.Handle && !a.Handle.0.is_null() && !a.Handle.is_invalid() {
            if category(a.Type).is_none() || category(a.Type) != category(b.Type) {
                // Contradictory union metadata cannot authorize either destructor. Quarantine
                // this invalid output; never close a possibly unrelated handle-table entry.
                return Err(dk_core::EngineError::protocol(
                    "Contradictory aliased WSLC handle tags",
                ));
            }
            b = WSLCHandle::default();
        }
        // Adopt both independently before propagating errors so valid partial outputs close.
        let a = Self::from_wslc(a);
        let b = Self::from_wslc(b);
        Ok((a?, b?))
    }
    /// Takes ownership of `h`. Null / `INVALID_HANDLE_VALUE` → `None`.
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_invalid() || h.0.is_null() {
            None
        } else {
            Some(Self {
                handle: h,
                socket: false,
            })
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.handle
    }

    pub(crate) fn from_wslc(
        h: super::abi::v3_0::WSLCHandle,
    ) -> dk_core::EngineResult<Option<Self>> {
        use super::abi::v3_0::*;
        if h.Handle.is_invalid() || h.Handle.0.is_null() {
            return Ok(None);
        }
        match h.Type {
            WSLC_HANDLE_TYPE_SOCKET => Ok(Some(Self {
                handle: h.Handle,
                socket: true,
            })),
            WSLC_HANDLE_TYPE_FILE | WSLC_HANDLE_TYPE_PIPE => Ok(Self::new(h.Handle)),
            // Unknown union discriminant cannot be safely closed by guessing an API.
            _ => Err(dk_core::EngineError::protocol(
                "Unknown WSLC handle tag; refused I/O",
            )),
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.socket {
            // SAFETY: tag identifies an owned Winsock socket, closed once after borrowers end.
            let _ = unsafe {
                windows::Win32::Networking::WinSock::closesocket(
                    windows::Win32::Networking::WinSock::SOCKET(self.handle.0 as usize),
                )
            };
        } else {
            // SAFETY: owned kernel handle, closed exactly once.
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
}

/// A manual-reset, initially non-signalled, unnamed event (cancel signals, overlapped I/O).
#[derive(Debug)]
pub struct Event(OwnedHandle);

impl Event {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: unnamed event with default security; handle owned by `OwnedHandle`.
        let h = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
        OwnedHandle::new(h)
            .map(Self)
            .ok_or_else(windows::core::Error::from_thread)
    }
    pub fn set(&self) {
        // SAFETY: valid event handle owned by `self`.
        let _ = unsafe { SetEvent(self.0.raw()) };
    }
    pub fn reset(&self) {
        // SAFETY: valid event handle owned by `self`.
        let _ = unsafe { ResetEvent(self.0.raw()) };
    }
    /// Waits up to `d` for the event; `true` if it was (or became) signalled.
    pub fn wait_timeout(&self, d: std::time::Duration) -> bool {
        let ms = u32::try_from(d.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: bounded wait on a valid handle owned by `self`.
        unsafe { WaitForSingleObject(self.0.raw(), ms) == WAIT_OBJECT_0 }
    }
    pub fn is_set(&self) -> bool {
        // SAFETY: zero-timeout wait on a valid handle.
        unsafe { WaitForSingleObject(self.0.raw(), 0) == WAIT_OBJECT_0 }
    }
    pub fn raw(&self) -> HANDLE {
        self.0.raw()
    }
}

/// Waits until `h` or `cancel` is signalled. `true` = `h`, `false` = cancelled / error.
pub fn wait_handle_or_cancel(h: &OwnedHandle, cancel: &Event) -> bool {
    let handles = [h.raw(), cancel.raw()];
    // SAFETY: both handles are valid for the duration of the wait.
    let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
    r == WAIT_OBJECT_0
}

/// Outcome of one cancellable read.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Data(usize),
    /// EOF / broken pipe / peer closed.
    Eof,
    Cancelled,
}

/// Reads up to `buf.len()` bytes from `h`, interruptible by `cancel`.
///
/// Works for overlapped and non-overlapped handles (WSLC hands out overlapped pipes and
/// sockets; spike F-7): the read is issued with an `OVERLAPPED`; if it pends we wait on the
/// I/O event *and* `cancel`; on cancel we `CancelIoEx` and drain the result before returning,
/// so `buf` is never written after return (mirrors `relay::InterruptableRead` in WSL).
pub fn read_cancellable(
    h: &OwnedHandle,
    buf: &mut [u8],
    cancel: &Event,
) -> windows::core::Result<ReadOutcome> {
    read_cancellable_pending(h, buf, cancel, || {})
}

fn read_cancellable_pending(
    h: &OwnedHandle,
    buf: &mut [u8],
    cancel: &Event,
    pending: impl FnOnce(),
) -> windows::core::Result<ReadOutcome> {
    if cancel.is_set() {
        return Ok(ReadOutcome::Cancelled);
    }
    let io_event = Event::new()?;
    let mut ov = OVERLAPPED {
        hEvent: io_event.raw(),
        ..Default::default()
    };
    let mut n = 0u32;
    let len = buf.len().min(u32::MAX as usize);
    // SAFETY: `buf[..len]` and `ov` stay alive and unmoved until the I/O completes: every
    // pending path below either observes completion or cancels and waits for it
    // (`GetOverlappedResult(.., bWait = TRUE)`).
    let r = unsafe { ReadFile(h.raw(), Some(&mut buf[..len]), Some(&mut n), Some(&mut ov)) };
    match r {
        Ok(()) => {
            // Completed synchronously (non-overlapped handle, or data already available).
            return Ok(if n == 0 {
                ReadOutcome::Eof
            } else {
                ReadOutcome::Data(n as usize)
            });
        }
        Err(e) if is_eof(&e) => return Ok(ReadOutcome::Eof),
        Err(e) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => {
            return Ok(ReadOutcome::Cancelled);
        }
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            pending();
        }
        Err(e) => return Err(e),
    }
    let handles = [io_event.raw(), cancel.raw()];
    // SAFETY: both handles are valid for the wait.
    let w = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
    if w != WAIT_OBJECT_0 {
        // Cancel (or wait failure): abort the read and wait for it to settle so `buf`/`ov`
        // are no longer referenced by the kernel.
        // SAFETY: `ov` identifies our pending read on `h`.
        unsafe {
            let _ = CancelIoEx(h.raw(), Some(&ov));
            let _ = GetOverlappedResult(h.raw(), &ov, &mut n, true);
        }
        return Ok(ReadOutcome::Cancelled);
    }
    // SAFETY: the I/O completed (event signalled); fetch the byte count without waiting.
    match unsafe { GetOverlappedResult(h.raw(), &ov, &mut n, false) } {
        Ok(()) if n == 0 => Ok(ReadOutcome::Eof),
        Ok(()) => Ok(ReadOutcome::Data(n as usize)),
        Err(e) if is_eof(&e) => Ok(ReadOutcome::Eof),
        Err(e) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => Ok(ReadOutcome::Cancelled),
        Err(e) => Err(e),
    }
}

/// Writes all of `data` to `h`, interruptible by `cancel`.
///
/// Mirrors [`read_cancellable`]: each chunk is issued with an `OVERLAPPED`; if it pends we wait
/// on the I/O event *and* `cancel`; on cancel we `CancelIoEx` the write and wait for it to settle
/// so `data`/`ov` are never referenced after return. A write that blocks synchronously (a
/// non-overlapped handle whose buffer is full) is interrupted by [`cancel_io`] from another
/// thread. Cancellation → `Err(ERROR_OPERATION_ABORTED)`.
pub fn write_all(h: &OwnedHandle, mut data: &[u8], cancel: &Event) -> windows::core::Result<()> {
    let aborted = || windows::core::Error::from_hresult(ERROR_OPERATION_ABORTED.to_hresult());
    let io_event = Event::new()?;
    while !data.is_empty() {
        if cancel.is_set() {
            return Err(aborted());
        }
        io_event.reset();
        let mut ov = OVERLAPPED {
            hEvent: io_event.raw(),
            ..Default::default()
        };
        let mut n = 0u32;
        let len = data.len().min(u32::MAX as usize);
        // SAFETY: `data[..len]` and `ov` outlive the I/O: every pending path below either
        // observes completion or cancels and waits for it (`GetOverlappedResult(.., TRUE)`).
        let r = unsafe { WriteFile(h.raw(), Some(&data[..len]), Some(&mut n), Some(&mut ov)) };
        match r {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                let handles = [io_event.raw(), cancel.raw()];
                // SAFETY: both handles are valid for the wait.
                let w = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                if w != WAIT_OBJECT_0 {
                    // SAFETY: `ov` identifies our pending write on `h`; waiting for it to
                    // settle guarantees the kernel no longer references `data`/`ov`.
                    unsafe {
                        let _ = CancelIoEx(h.raw(), Some(&ov));
                        let _ = GetOverlappedResult(h.raw(), &ov, &mut n, true);
                    }
                    return Err(aborted());
                }
                // SAFETY: the write completed (event signalled); fetch the count, no wait.
                unsafe { GetOverlappedResult(h.raw(), &ov, &mut n, false) }?;
            }
            Err(e) => return Err(e),
        }
        if n == 0 {
            return Err(windows::core::Error::from_hresult(
                ERROR_NO_DATA.to_hresult(),
            ));
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// Aborts all pending I/O on `h` issued by any thread of this process (unblocks a reader or a
/// writer thread; non-blocking, safe to call from any thread).
pub fn cancel_io(h: &OwnedHandle) {
    // SAFETY: `h` is valid; cancelling with no OVERLAPPED cancels all I/O of this process on it.
    let _ = unsafe { CancelIoEx(h.raw(), None) };
}

fn is_eof(e: &windows::core::Error) -> bool {
    let c = e.code();
    c == ERROR_HANDLE_EOF.to_hresult()
        || c == ERROR_BROKEN_PIPE.to_hresult()
        || c == ERROR_NO_DATA.to_hresult()
        // WSAECONNRESET / WSAESHUTDOWN / WSAEDISCON surfaced through ReadFile on a socket.
        || c == windows::core::HRESULT::from_win32(10054)
        || c == windows::core::HRESULT::from_win32(10058)
        || c == windows::core::HRESULT::from_win32(10101)
}

/// Owned thread handle for interrupting a registered synchronous ConPTY writer.
pub(crate) fn current_io_thread() -> windows::core::Result<OwnedHandle> {
    use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};
    let mut out = HANDLE::default();
    // SAFETY: duplicate current pseudo handle into an independently owned real handle.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentThread(),
            GetCurrentProcess(),
            &mut out,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }?;
    OwnedHandle::new(out).ok_or_else(windows::core::Error::from_thread)
}
pub(crate) fn cancel_sync_thread(thread: &OwnedHandle) {
    // SAFETY: registration cannot finish/reuse its thread until this call returns.
    let _ = unsafe { windows::Win32::System::IO::CancelSynchronousIo(thread.raw()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_require_matching_destructor_category() {
        use super::super::abi::v3_0::*;
        let event = Event::new().unwrap();
        let a = WSLCHandle {
            Type: WSLC_HANDLE_TYPE_PIPE,
            Handle: event.raw(),
        };
        let b = WSLCHandle {
            Type: WSLC_HANDLE_TYPE_SOCKET,
            Handle: event.raw(),
        };
        assert!(OwnedHandle::pair(a, b).is_err());
        event.set();
        assert!(event.is_set());
        // SAFETY: owned event transfers to the compatible-tag pair below.
        let raw = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.unwrap();
        let (one, two) = OwnedHandle::pair(
            WSLCHandle {
                Type: WSLC_HANDLE_TYPE_FILE,
                Handle: raw,
            },
            WSLCHandle {
                Type: WSLC_HANDLE_TYPE_PIPE,
                Handle: raw,
            },
        )
        .unwrap();
        assert!(one.is_some());
        assert!(two.is_none());
        drop(one);
        // SAFETY: closed handle probe, never take ownership or close it again.
        assert!(unsafe { SetEvent(raw) }.is_err());
    }

    #[test]
    fn eng_134_socket_cancel_drains_overlapped_before_close() {
        use super::super::abi::v3_0::*;
        use std::os::windows::io::IntoRawSocket;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let socket = client.into_raw_socket();
        let owner = OwnedHandle::from_wslc(WSLCHandle {
            Type: WSLC_HANDLE_TYPE_SOCKET,
            Handle: HANDLE(socket as *mut c_void),
        })
        .unwrap()
        .unwrap();
        let cancel = std::sync::Arc::new(Event::new().unwrap());
        let signal = cancel.clone();
        let (pending_tx, pending_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut buf = [0u8; 128];
            let result = read_cancellable_pending(&owner, &mut buf, &cancel, || {
                pending_tx.send(()).unwrap();
            })
            .unwrap();
            assert_eq!(result, ReadOutcome::Cancelled);
            // Both OVERLAPPED/event and buffer may now be freed; resource drops on this thread.
            drop(owner);
        });
        pending_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("actual ERROR_IO_PENDING admission");
        signal.set();
        worker.join().unwrap();
    }

    #[test]
    fn eng_134_socket_close_once_and_kernel_tags() {
        use super::super::abi::v3_0::*;
        use windows::Win32::Networking::WinSock::{
            AF_INET, INVALID_SOCKET, IPPROTO_TCP, SOCK_STREAM, WSA_FLAG_OVERLAPPED, WSADATA,
            WSASocketW, WSAStartup,
        };
        let mut data = WSADATA::default();
        // SAFETY: valid WSADATA; process test startup, sockets need Winsock initialized.
        assert_eq!(unsafe { WSAStartup(0x202, &mut data) }, 0);
        // SAFETY: creates one owned overlapped TCP socket; never connected.
        let socket = unsafe {
            WSASocketW(
                AF_INET.0 as i32,
                SOCK_STREAM.0,
                IPPROTO_TCP.0,
                None,
                0,
                WSA_FLAG_OVERLAPPED,
            )
        }
        .unwrap();
        assert_ne!(socket, INVALID_SOCKET);
        let owner = OwnedHandle::from_wslc(WSLCHandle {
            Type: WSLC_HANDLE_TYPE_SOCKET,
            Handle: HANDLE(socket.0 as *mut c_void),
        })
        .unwrap()
        .unwrap();
        assert!(owner.socket);
        drop(owner);
        // SAFETY: querying a closed socket must fail; no second ownership/close.
        let mut kind = [0u8; 4];
        let mut len = 4;
        assert_ne!(
            unsafe {
                windows::Win32::Networking::WinSock::getsockopt(
                    socket,
                    windows::Win32::Networking::WinSock::SOL_SOCKET,
                    windows::Win32::Networking::WinSock::SO_TYPE,
                    windows::core::PSTR(kind.as_mut_ptr()),
                    &mut len,
                )
            },
            0
        );
        let event = Event::new().unwrap();
        assert!(!event.0.socket);
        assert!(
            OwnedHandle::from_wslc(WSLCHandle {
                Type: 99,
                Handle: event.raw()
            })
            .is_err()
        );
        event.set();
        assert!(
            event.is_set(),
            "unknown tag did not close borrowed resource"
        );
    }

    #[test]
    fn file_version_of_kernel32() {
        let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let v = file_version(&Path::new(&sysroot).join("System32").join("kernel32.dll"));
        let v = v.expect("kernel32 has a version resource");
        assert!(v.0 >= 6, "{v:?}");
    }

    #[test]
    fn file_version_missing_file() {
        assert_eq!(file_version(Path::new(r"C:\definitely\not\here.exe")), None);
    }

    #[test]
    fn registry_reads() {
        assert!(reg_key_exists_hklm(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"
        ));
        assert!(!reg_key_exists_hklm(r"SOFTWARE\Dockering\Nope\Nope"));
        let pn = reg_string_hklm(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        );
        assert!(pn.is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn process_list_contains_self() {
        let exe = std::env::current_exe().expect("exe");
        let name = exe.file_name().and_then(|n| n.to_str()).expect("name");
        assert!(process_running(name));
        assert!(!process_running("dockering-no-such-process-xyz.exe"));
    }

    #[test]
    fn current_user_sid_is_a_sid() {
        let sid = current_user_sid().expect("sid");
        assert!(sid.starts_with("S-1-"), "{sid}");
    }

    #[test]
    fn event_set_reset() {
        let e = Event::new().expect("event");
        assert!(!e.is_set());
        e.set();
        assert!(e.is_set());
        e.reset();
        assert!(!e.is_set());
    }
}
