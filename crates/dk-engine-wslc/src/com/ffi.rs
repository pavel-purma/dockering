//! COM FFI helpers (ADR-0003): RAII for `[out]` CoTaskMem allocations, proxy blankets,
//! process-wide COM security, per-thread apartment init, and `HRESULT` → `EngineError` mapping.
//!
//! The HRESULT table (`hr` module + [`map_hresult`]) is platform-neutral so it is unit-tested
//! everywhere; everything that calls Win32 is `#[cfg(windows)]`.

use dk_core::{EngineError, ResourceKind};

/// HRESULT constants used by the mapping (spec 20 §5.4).
///
/// `WSLC_E_*` are defined at the end of `idl/3.0.1/wslc.idl` as
/// `MAKE_HRESULT(SEVERITY_ERROR, FACILITY_ITF, WSLC_E_BASE + n)` with `WSLC_E_BASE = 0x0600`;
/// the hex values below are copied from the comments next to each definition there.
pub mod hr {
    pub const WSLC_E_IMAGE_NOT_FOUND: i32 = 0x8004_0601_u32 as i32;
    pub const WSLC_E_CONTAINER_PREFIX_AMBIGUOUS: i32 = 0x8004_0602_u32 as i32;
    pub const WSLC_E_CONTAINER_NOT_FOUND: i32 = 0x8004_0603_u32 as i32;
    pub const WSLC_E_VOLUME_NOT_FOUND: i32 = 0x8004_0604_u32 as i32;
    pub const WSLC_E_CONTAINER_NOT_RUNNING: i32 = 0x8004_0605_u32 as i32;
    pub const WSLC_E_CONTAINER_IS_RUNNING: i32 = 0x8004_0606_u32 as i32;
    pub const WSLC_E_SESSION_RESERVED: i32 = 0x8004_0607_u32 as i32;
    pub const WSLC_E_INVALID_SESSION_NAME: i32 = 0x8004_0608_u32 as i32;
    pub const WSLC_E_NETWORK_NOT_FOUND: i32 = 0x8004_0609_u32 as i32;
    pub const WSLC_E_WU_SEARCH_FAILED: i32 = 0x8004_060A_u32 as i32;
    pub const WSLC_E_SDK_UPDATE_NEEDED: i32 = 0x8004_060B_u32 as i32;
    pub const WSLC_E_CONTAINER_DISABLED: i32 = 0x8004_060C_u32 as i32;
    pub const WSLC_E_REGISTRY_BLOCKED_BY_POLICY: i32 = 0x8004_060D_u32 as i32;
    pub const WSLC_E_VOLUME_NOT_AVAILABLE: i32 = 0x8004_060E_u32 as i32;
    pub const WSLC_E_SESSION_NOT_FOUND: i32 = 0x8004_060F_u32 as i32;
    pub const WSLC_E_VM_NOT_RUNNING: i32 = 0x8004_0610_u32 as i32;
    pub const WSLC_E_EVENTS_LOST: i32 = 0x8004_0611_u32 as i32;
    pub const WSLC_E_EVENT_STREAM_FINISHED: i32 = 0x8004_0612_u32 as i32;
    pub const WSLC_E_CONTAINER_DELETED: i32 = 0x8004_0613_u32 as i32;

    pub const E_ABORT: i32 = 0x8000_4004_u32 as i32;
    pub const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
    pub const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
    pub const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    pub const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;
    pub const CO_E_SERVER_EXEC_FAILURE: i32 = 0x8008_0005_u32 as i32;
    pub const RPC_E_DISCONNECTED: i32 = 0x8001_0108_u32 as i32;
    pub const RPC_E_SERVER_DIED: i32 = 0x8001_0007_u32 as i32;
    pub const RPC_E_SERVER_DIED_DNE: i32 = 0x8001_0012_u32 as i32;
    pub const RPC_E_TOO_LATE: i32 = 0x8001_0119_u32 as i32;
    /// `HRESULT_FROM_WIN32(RPC_S_SERVER_UNAVAILABLE = 1722)`.
    pub const RPC_S_SERVER_UNAVAILABLE: i32 = 0x8007_06BA_u32 as i32;
    /// `HRESULT_FROM_WIN32(RPC_S_CALL_FAILED = 1726)`.
    pub const RPC_S_CALL_FAILED: i32 = 0x8007_06BE_u32 as i32;
    /// `HRESULT_FROM_WIN32(ERROR_INVALID_STATE = 5023)`: session terminating / no docker.
    pub const ERROR_INVALID_STATE: i32 = 0x8007_139F_u32 as i32;
    /// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS = 183)`.
    pub const ERROR_ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;
    /// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND = 1168)`.
    pub const ERROR_NOT_FOUND: i32 = 0x8007_0490_u32 as i32;
    pub const E_FAIL: i32 = 0x8000_4005_u32 as i32;
}

/// Hint shown when WSLC is disabled by Group Policy (spec 20 §5.2 step 2).
pub const POLICY_HINT: &str = "WSL containers are disabled by Group Policy";

/// Whether `code` means the session/VM connection dropped, so the cached session proxy should
/// be reopened once and the call retried (spec 20 §5.4 Lifetime).
pub fn is_disconnect(code: i32) -> bool {
    matches!(
        code,
        hr::RPC_E_DISCONNECTED
            | hr::RPC_S_SERVER_UNAVAILABLE
            | hr::RPC_E_SERVER_DIED
            | hr::RPC_E_SERVER_DIED_DNE
            | hr::RPC_S_CALL_FAILED
    )
}

/// Context for mapping a failed call: which resource the call was about, for `NotFound`.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    pub kind: ResourceKind,
    pub id: &'a str,
}

impl<'a> Subject<'a> {
    pub fn new(kind: ResourceKind, id: &'a str) -> Self {
        Self { kind, id }
    }
}

/// Maps a failed `HRESULT` (+ `IErrorInfo` text, if any) to an `EngineError` per spec 20 §5.4.
///
/// `NotFound` uses the HRESULT's own resource kind when it names one (e.g. an image-not-found
/// during a container create), else `subject`.
pub fn map_hresult(code: i32, message: &str, subject: Option<Subject<'_>>) -> EngineError {
    let msg = message.trim();
    let text = if msg.is_empty() {
        format!("HRESULT 0x{:08X}", code as u32)
    } else {
        format!("{msg} (0x{:08X})", code as u32)
    };
    let id = subject.map(|s| s.id.to_owned()).unwrap_or_default();
    let not_found = |kind: ResourceKind| EngineError::NotFound {
        kind,
        id: id.clone(),
    };
    match code {
        hr::WSLC_E_CONTAINER_NOT_FOUND | hr::WSLC_E_CONTAINER_DELETED => {
            not_found(ResourceKind::Container)
        }
        hr::WSLC_E_IMAGE_NOT_FOUND => not_found(ResourceKind::Image),
        hr::WSLC_E_VOLUME_NOT_FOUND => not_found(ResourceKind::Volume),
        hr::WSLC_E_NETWORK_NOT_FOUND => not_found(ResourceKind::Network),
        hr::WSLC_E_SESSION_NOT_FOUND => EngineError::NotFound {
            kind: ResourceKind::Session,
            id: subject
                .filter(|s| s.kind == ResourceKind::Session)
                .map(|s| s.id.to_owned())
                .unwrap_or_else(|| "default".into()),
        },
        hr::WSLC_E_CONTAINER_IS_RUNNING
        | hr::WSLC_E_CONTAINER_NOT_RUNNING
        | hr::WSLC_E_CONTAINER_PREFIX_AMBIGUOUS
        | hr::ERROR_ALREADY_EXISTS => EngineError::Conflict(text),
        hr::WSLC_E_CONTAINER_DISABLED => EngineError::Unreachable {
            reason: text,
            hint: Some(POLICY_HINT.into()),
        },
        hr::WSLC_E_VM_NOT_RUNNING => EngineError::Unreachable {
            reason: text,
            hint: Some("The WSL containers VM is not running.".into()),
        },
        c if is_disconnect(c) => EngineError::Unreachable {
            reason: text,
            hint: None,
        },
        hr::REGDB_E_CLASSNOTREG | hr::CO_E_SERVER_EXEC_FAILURE => EngineError::Unreachable {
            reason: text,
            hint: Some("WSL containers available with `wsl --update`".into()),
        },
        hr::ERROR_INVALID_STATE => EngineError::Unreachable {
            reason: text,
            hint: Some("The WSL containers session is terminating or not ready.".into()),
        },
        hr::E_ABORT => EngineError::Cancelled,
        hr::E_INVALIDARG => EngineError::Api {
            status: 400,
            message: text,
        },
        hr::E_ACCESSDENIED => EngineError::Api {
            status: 403,
            message: text,
        },
        // Spec: `Api { status: hr }`. `status` is u16 in the DTO, so the full HRESULT goes
        // into the message and the status carries 500 (server-side failure) — see deviations.
        _ => EngineError::Api {
            status: 500,
            message: text,
        },
    }
}

/// Clamp helper: copy a `char[N]` IDL field into a `String` (up to the first NUL).
pub fn fixed_cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Same for `wchar_t[N]`.
pub fn fixed_wstr(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&c| c == 0)?;
    Some(String::from_utf16_lossy(&units[..end]))
}

/// Copies `s` into a NUL-terminated `char[N]` field; `false` if it doesn't fit.
pub fn write_fixed(dst: &mut [u8], s: &str) -> bool {
    if s.len() >= dst.len() {
        return false;
    }
    dst.fill(0);
    dst[..s.len()].copy_from_slice(s.as_bytes());
    true
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::ffi::{CString, c_void};
    use std::marker::PhantomData;

    use windows::Win32::Networking::WinSock::{WSADATA, WSAStartup};
    use windows::Win32::System::Com::{
        COINIT_MULTITHREADED, CoIncrementMTAUsage, CoInitializeEx, CoInitializeSecurity,
        CoSetProxyBlanket, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, EOAC_STATIC_CLOAKING,
        RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Rpc::{RPC_C_AUTHN_DEFAULT, RPC_C_AUTHZ_DEFAULT};
    use windows::core::{IUnknown, Interface, PCSTR, PSTR, PWSTR};

    use super::{Subject, map_hresult};
    use dk_core::EngineError;

    // ───────────── CoTaskMem RAII ─────────────

    /// An `[out, string] LPSTR` (UTF-8/ANSI) owned by us; freed with `CoTaskMemFree`.
    #[derive(Debug)]
    pub struct CoTaskMemStr(*mut u8);

    impl CoTaskMemStr {
        /// Empty slot to pass as `&mut` out-param (`.out()`).
        pub fn null() -> Self {
            Self(std::ptr::null_mut())
        }
        /// Out-param pointer for the callee to fill.
        pub fn out(&mut self) -> *mut PSTR {
            // `PSTR` is `#[repr(transparent)]` over `*mut u8`.
            (&mut self.0 as *mut *mut u8).cast()
        }
        pub fn is_null(&self) -> bool {
            self.0.is_null()
        }
        /// Lossy UTF-8 copy (`None` when null).
        pub fn to_string_opt(&self) -> Option<String> {
            if self.0.is_null() {
                return None;
            }
            // SAFETY: non-null `[out, string]` LPSTR from COM is NUL-terminated and stays
            // alive until `self` is dropped.
            let s = unsafe { std::ffi::CStr::from_ptr(self.0 as *const std::ffi::c_char) };
            Some(s.to_string_lossy().into_owned())
        }
        pub fn to_string_lossy(&self) -> String {
            self.to_string_opt().unwrap_or_default()
        }
        /// Takes ownership of a callee-allocated LPSTR (e.g. a struct field).
        ///
        /// # Safety
        /// `p` must be null or a `CoTaskMemAlloc`ated NUL-terminated string not owned elsewhere.
        pub unsafe fn from_raw(p: *mut u8) -> Self {
            Self(p)
        }
        /// Allocates a copy of `s` with `CoTaskMemAlloc` (used by the fake server in tests to
        /// hand out memory exactly like a real server). Interior NULs truncate.
        pub fn alloc(s: &str) -> *mut u8 {
            let bytes = s.as_bytes();
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            // SAFETY: allocating `end + 1` bytes; null checked before writing.
            let p = unsafe { CoTaskMemAlloc(end + 1) } as *mut u8;
            if !p.is_null() {
                // SAFETY: `p` has `end + 1` bytes; source has at least `end` bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, end);
                    *p.add(end) = 0;
                }
            }
            p
        }
    }

    impl Drop for CoTaskMemStr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: callee-allocated with CoTaskMemAlloc per COM `[out]` rules; freed once.
                unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
            }
        }
    }

    /// An `[out, string] LPWSTR` owned by us.
    #[derive(Debug)]
    pub struct CoTaskMemWStr(*mut u16);

    impl CoTaskMemWStr {
        pub fn null() -> Self {
            Self(std::ptr::null_mut())
        }
        pub fn out(&mut self) -> *mut PWSTR {
            (&mut self.0 as *mut *mut u16).cast()
        }
        pub fn to_string_opt(&self) -> Option<String> {
            if self.0.is_null() {
                return None;
            }
            // SAFETY: non-null NUL-terminated wide string from COM, alive until drop.
            unsafe { PWSTR(self.0).to_string() }.ok()
        }
        /// `CoTaskMemAlloc` copy of `s` (fake server).
        pub fn alloc(s: &str) -> *mut u16 {
            let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: allocating `w.len() * 2` bytes; null checked before writing.
            let p = unsafe { CoTaskMemAlloc(w.len() * 2) } as *mut u16;
            if !p.is_null() {
                // SAFETY: `p` has room for `w.len()` u16s.
                unsafe { std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len()) };
            }
            p
        }
    }

    impl Drop for CoTaskMemWStr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: callee-allocated with CoTaskMemAlloc; freed once.
                unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
            }
        }
    }

    /// A callee-allocated `[out, size_is(, *Count)] T**` array, freed with `CoTaskMemFree`.
    /// Elements that themselves own CoTaskMem pointers must be released by the caller first
    /// (see `ContainerEntries` in the engine).
    pub struct CoTaskMemArray<T> {
        ptr: *mut T,
        len: u32,
        _t: PhantomData<T>,
    }

    impl<T> CoTaskMemArray<T> {
        pub fn new() -> Self {
            Self {
                ptr: std::ptr::null_mut(),
                len: 0,
                _t: PhantomData,
            }
        }
        /// `(T**, ULONG*)` out-params.
        pub fn out(&mut self) -> (*mut *mut T, *mut u32) {
            (&mut self.ptr, &mut self.len)
        }
        pub fn len(&self) -> usize {
            if self.ptr.is_null() {
                0
            } else {
                self.len as usize
            }
        }
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
        pub fn as_slice(&self) -> &[T] {
            if self.ptr.is_null() || self.len == 0 {
                &[]
            } else {
                // SAFETY: the callee allocated `len` contiguous, initialised `T`s at `ptr`
                // (MIDL `size_is(, *Count)` contract); alive until drop.
                unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
            }
        }
        pub fn as_mut_slice(&mut self) -> &mut [T] {
            if self.ptr.is_null() || self.len == 0 {
                &mut []
            } else {
                // SAFETY: as `as_slice`, uniquely borrowed through `&mut self`.
                unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len as usize) }
            }
        }
        /// Allocates a CoTaskMem array holding `items` and returns `(ptr, count)` exactly as a
        /// real server would hand it out (fake server / tests).
        pub fn alloc(items: &[T]) -> (*mut T, u32)
        where
            T: Copy,
        {
            if items.is_empty() {
                return (std::ptr::null_mut(), 0);
            }
            let bytes = std::mem::size_of_val(items);
            // SAFETY: allocating `bytes`; CoTaskMemAlloc returns memory aligned for any
            // fundamental type (8/16 bytes), which covers every IDL struct.
            let p = unsafe { CoTaskMemAlloc(bytes) } as *mut T;
            if p.is_null() {
                return (p, 0);
            }
            // SAFETY: `p` has room for `items.len()` `T`s; `T: Copy`.
            unsafe { std::ptr::copy_nonoverlapping(items.as_ptr(), p, items.len()) };
            (p, items.len() as u32)
        }
    }

    impl<T> Default for CoTaskMemArray<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<T> Drop for CoTaskMemArray<T> {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                // SAFETY: callee-allocated with CoTaskMemAlloc; freed once.
                unsafe { CoTaskMemFree(Some(self.ptr as *const c_void)) };
            }
        }
    }

    /// Frees a CoTaskMem pointer held in a struct field and nulls it.
    pub fn free_field<T>(p: &mut *mut T) {
        if !p.is_null() {
            // SAFETY: field was CoTaskMemAlloc'ed by the callee and is owned by us now.
            unsafe { CoTaskMemFree(Some(*p as *const c_void)) };
            *p = std::ptr::null_mut();
        }
    }

    /// `CString` for `[in] LPCSTR` arguments. Interior NULs → `Api(400)`.
    pub fn cstring(s: &str) -> Result<CString, EngineError> {
        CString::new(s).map_err(|_| EngineError::Api {
            status: 400,
            message: "argument contains a NUL byte".into(),
        })
    }

    pub fn pcstr(c: &CString) -> PCSTR {
        PCSTR(c.as_ptr() as *const u8)
    }

    pub fn opt_pcstr(c: Option<&CString>) -> PCSTR {
        c.map(pcstr).unwrap_or(PCSTR::null())
    }

    // ───────────── security / apartments ─────────────

    /// `CoSetProxyBlanket(… RPC_C_IMP_LEVEL_IMPERSONATE, EOAC_STATIC_CLOAKING)` on a proxy,
    /// mirroring `wslc.exe`'s `ConfigureForCOMImpersonation` (spec 20 §5.4). Fails with
    /// `E_NOINTERFACE` on in-process (non-proxy) objects such as the test fake; that is fine.
    pub fn set_proxy_blanket<I: Interface>(i: &I) -> windows::core::Result<()> {
        let unk: IUnknown = i.cast()?;
        // SAFETY: `unk` is a live interface pointer; all other args are constants/None.
        unsafe {
            CoSetProxyBlanket(
                &unk,
                RPC_C_AUTHN_DEFAULT as u32,
                RPC_C_AUTHZ_DEFAULT,
                windows::Win32::System::Com::COLE_DEFAULT_PRINCIPAL,
                RPC_C_AUTHN_LEVEL_DEFAULT,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_STATIC_CLOAKING,
            )
        }
    }

    /// [`set_proxy_blanket`], ignoring "not a proxy" (in-proc objects) and logging others.
    pub fn blanket<I: Interface>(i: &I) {
        if let Err(e) = set_proxy_blanket(i)
            && e.code().0 != super::hr::E_NOINTERFACE
        {
            tracing::debug!(
                hr = format!("0x{:08X}", e.code().0 as u32),
                "CoSetProxyBlanket failed"
            );
        }
    }

    /// RAII apartment membership for the current thread (`CoInitializeEx(MTA)`).
    pub struct MtaGuard {
        initialized: bool,
    }

    impl MtaGuard {
        pub fn enter() -> Self {
            // SAFETY: joins (or creates) the process MTA for this thread; paired with
            // CoUninitialize in Drop only when this call succeeded (incl. S_FALSE).
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Self {
                initialized: hr.is_ok(),
            }
        }
        pub fn ok(&self) -> bool {
            self.initialized
        }
    }

    impl Drop for MtaGuard {
        fn drop(&mut self) {
            if self.initialized {
                // SAFETY: balances the successful CoInitializeEx on this same thread.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// Process-wide setup that must run in `main()` before GPUI (spec 10 §7, F-7/F-8).
    pub fn init_process_com_security() -> bool {
        let mut wsa = WSADATA::default();
        // SAFETY: plain Winsock init; never cleaned up (process lifetime), as specified.
        let rc = unsafe { WSAStartup(0x0202, &mut wsa) };
        if rc != 0 {
            tracing::warn!(
                rc,
                "WSAStartup(2.2) failed; WSLC logs/exec handles will not work"
            );
        }
        // CoInitializeSecurity is per process but needs an apartment on the calling thread;
        // use a throwaway MTA thread so main/GPUI threads stay untouched (spike F-8).
        let joined = std::thread::Builder::new()
            .name("dk-com-security".into())
            .spawn(|| {
                // Keep the process MTA alive for the process lifetime (cookie never released):
                // if COM fully uninitialises (last apartment gone), the process security set
                // below is discarded and later activations get the default IDENTIFY level,
                // which makes `OpenSessionByName` fail with 0x80070542 (verified live).
                // SAFETY: process-lifetime MTA usage; the cookie is intentionally leaked.
                if let Err(e) = unsafe { CoIncrementMTAUsage() } {
                    tracing::warn!(
                        hr = format!("0x{:08X}", e.code().0 as u32),
                        "CoIncrementMTAUsage failed"
                    );
                }
                let mta = MtaGuard::enter();
                if !mta.ok() {
                    return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        super::hr::E_FAIL,
                    )));
                }
                // SAFETY: process-wide security init with default auth/impersonate/static
                // cloaking, mirroring wslc.exe. Must precede any COM proxy creation.
                unsafe {
                    CoInitializeSecurity(
                        None,
                        -1,
                        None,
                        None,
                        RPC_C_AUTHN_LEVEL_DEFAULT,
                        RPC_C_IMP_LEVEL_IMPERSONATE,
                        None,
                        EOAC_STATIC_CLOAKING,
                        None,
                    )
                }
            })
            .map(|h| h.join());
        match joined {
            Ok(Ok(Ok(()))) => true,
            Ok(Ok(Err(e))) => {
                if e.code().0 == super::hr::RPC_E_TOO_LATE {
                    tracing::warn!(
                        "CoInitializeSecurity: RPC_E_TOO_LATE (called after COM was used); \
                         WSLC uses per-proxy blankets only"
                    );
                } else {
                    tracing::warn!(
                        hr = format!("0x{:08X}", e.code().0 as u32),
                        "CoInitializeSecurity failed"
                    );
                }
                false
            }
            _ => {
                tracing::warn!("CoInitializeSecurity thread failed");
                false
            }
        }
    }

    // ───────────── error mapping ─────────────

    /// Maps a `windows::core::Error` (which carries `IErrorInfo` captured at the failing
    /// call, if the server set one) to an `EngineError`.
    pub fn map_err(e: &windows::core::Error, subject: Option<Subject<'_>>) -> EngineError {
        let code = e.code().0;
        // `Error::message` prefers IErrorInfo, else the system text for the code; the latter
        // is useless for FACILITY_ITF codes, so drop it there.
        let msg = e.message();
        let msg = if (code as u32 >> 16) & 0x1FFF == 4 && msg.starts_with("Unknown") {
            String::new()
        } else {
            msg
        };
        map_hresult(code, &msg, subject)
    }

    /// `HRESULT` from a vtable call → `Result`, capturing `IErrorInfo` on this thread.
    pub fn check(hr: windows::core::HRESULT) -> windows::core::Result<()> {
        hr.ok()
    }
}

/// Non-Windows: COM is unavailable.
#[cfg(not(windows))]
pub fn init_process_com_security() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wslc_codes_match_idl_formula() {
        // MAKE_HRESULT(1, FACILITY_ITF=4, 0x0600 + n)
        let make = |n: u32| (0x8000_0000u32 | (4 << 16) | (0x0600 + n)) as i32;
        assert_eq!(hr::WSLC_E_IMAGE_NOT_FOUND, make(1));
        assert_eq!(hr::WSLC_E_CONTAINER_DISABLED, make(12));
        assert_eq!(hr::WSLC_E_SESSION_NOT_FOUND, make(15));
        assert_eq!(hr::WSLC_E_EVENTS_LOST, make(17));
        assert_eq!(hr::WSLC_E_EVENT_STREAM_FINISHED, make(18));
        assert_eq!(hr::WSLC_E_CONTAINER_DELETED, make(19));
    }

    #[test]
    fn mapping_table() {
        let c = Some(Subject::new(ResourceKind::Container, "abc"));
        assert_eq!(
            map_hresult(hr::WSLC_E_CONTAINER_NOT_FOUND, "", c),
            EngineError::not_found(ResourceKind::Container, "abc")
        );
        assert_eq!(
            map_hresult(
                hr::WSLC_E_IMAGE_NOT_FOUND,
                "x",
                Some(Subject::new(ResourceKind::Image, "nginx"))
            ),
            EngineError::not_found(ResourceKind::Image, "nginx")
        );
        assert!(matches!(
            map_hresult(
                hr::WSLC_E_VOLUME_NOT_FOUND,
                "",
                Some(Subject::new(ResourceKind::Volume, "v"))
            ),
            EngineError::NotFound {
                kind: ResourceKind::Volume,
                ..
            }
        ));
        assert!(matches!(
            map_hresult(hr::WSLC_E_NETWORK_NOT_FOUND, "", None),
            EngineError::NotFound {
                kind: ResourceKind::Network,
                ..
            }
        ));
        assert_eq!(
            map_hresult(hr::WSLC_E_SESSION_NOT_FOUND, "", None),
            EngineError::not_found(ResourceKind::Session, "default")
        );
        for code in [
            hr::WSLC_E_CONTAINER_IS_RUNNING,
            hr::WSLC_E_CONTAINER_NOT_RUNNING,
            hr::WSLC_E_CONTAINER_PREFIX_AMBIGUOUS,
        ] {
            assert!(
                matches!(map_hresult(code, "m", c), EngineError::Conflict(_)),
                "{code:#x}"
            );
        }
        for code in [
            hr::WSLC_E_VM_NOT_RUNNING,
            hr::RPC_E_DISCONNECTED,
            hr::RPC_S_SERVER_UNAVAILABLE,
        ] {
            assert!(map_hresult(code, "", None).is_unreachable(), "{code:#x}");
        }
        let policy = map_hresult(hr::WSLC_E_CONTAINER_DISABLED, "", None);
        assert_eq!(policy.hint(), Some(POLICY_HINT));
        assert!(matches!(
            map_hresult(hr::WSLC_E_REGISTRY_BLOCKED_BY_POLICY, "blocked", None),
            EngineError::Api { .. }
        ));
        assert_eq!(map_hresult(hr::E_ABORT, "", None), EngineError::Cancelled);
        assert!(matches!(
            map_hresult(hr::E_INVALIDARG, "bad", None),
            EngineError::Api { status: 400, .. }
        ));
    }

    #[test]
    fn unknown_code_keeps_hresult_and_error_info_text() {
        let e = map_hresult(hr::E_FAIL, "Docker said no", None);
        match e {
            EngineError::Api { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("Docker said no"), "{message}");
                assert!(message.contains("0x80004005"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        match map_hresult(0x8004_06FF_u32 as i32, "", None) {
            EngineError::Api { message, .. } => assert_eq!(message, "HRESULT 0x800406FF"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn disconnect_codes() {
        assert!(is_disconnect(hr::RPC_E_DISCONNECTED));
        assert!(is_disconnect(hr::RPC_S_SERVER_UNAVAILABLE));
        assert!(!is_disconnect(hr::E_FAIL));
        assert!(!is_disconnect(hr::WSLC_E_CONTAINER_NOT_FOUND));
    }

    #[test]
    fn fixed_strings() {
        assert_eq!(fixed_cstr(b"abc\0def"), "abc");
        assert_eq!(fixed_cstr(b"abc"), "abc");
        let w: Vec<u16> = "hi\0x".encode_utf16().collect();
        assert_eq!(fixed_wstr(&w).as_deref(), Some("hi"));
        assert_eq!(fixed_wstr(&[0x41, 0x42]), None, "unterminated → None");
        let mut buf = [0xFFu8; 4];
        assert!(write_fixed(&mut buf, "abc"));
        assert_eq!(&buf, b"abc\0");
        assert!(!write_fixed(&mut buf, "abcd"));
    }
}
