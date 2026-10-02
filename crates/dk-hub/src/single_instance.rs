//! Single instance per user (SHL-022). A second launch asks the running instance to focus its
//! window and exits. Windows: named pipe `\\.\pipe\dockering-instance-<user SID>`, created with
//! a current-user-only DACL; a secondary exits only after verifying that the pipe's server
//! process runs as the same user. Unix: socket `<data_dir>/instance.sock`.

use futures::channel::mpsc;

use crate::paths::Paths;

/// Held by the primary instance for the app's lifetime.
pub struct InstanceGuard {
    _private: (),
    focus: Option<mpsc::UnboundedReceiver<()>>,
    #[cfg(unix)]
    _socket: Option<unix::SocketCleanup>,
}

impl InstanceGuard {
    /// Focus requests from later launches. Can be taken once; later calls return `None`.
    pub fn take_focus_requests(&mut self) -> Option<mpsc::UnboundedReceiver<()>> {
        self.focus.take()
    }

    /// A primary guard that listens to nothing (any setup error).
    fn detached() -> Self {
        Self {
            _private: (),
            focus: None,
            #[cfg(unix)]
            _socket: None,
        }
    }
}

pub enum Instance {
    Primary(InstanceGuard),
    /// Another instance is running and was asked to focus; the caller should exit.
    Secondary,
}

/// Message sent by a secondary instance.
const FOCUS_MSG: &[u8] = b"focus\n";

/// Startup only (before GPUI). Never fails hard: on any error behaves as `Primary`.
pub fn acquire(paths: &Paths) -> Instance {
    imp_acquire(paths)
}

#[cfg(unix)]
fn imp_acquire(paths: &Paths) -> Instance {
    unix::acquire(paths)
}

#[cfg(windows)]
fn imp_acquire(_paths: &Paths) -> Instance {
    match win::current_user_sid() {
        Ok(sid) => win::acquire(&win::pipe_name(&sid), &sid),
        Err(e) => {
            tracing::warn!(error = %e, "single-instance: couldn't read the user SID");
            Instance::Primary(InstanceGuard::detached())
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn imp_acquire(_paths: &Paths) -> Instance {
    Instance::Primary(InstanceGuard::detached())
}

#[cfg(unix)]
mod unix {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    use super::*;

    /// Removes the socket file when the primary exits.
    pub(super) struct SocketCleanup(PathBuf);

    impl Drop for SocketCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    pub(super) fn acquire(paths: &Paths) -> Instance {
        let path = paths.data_dir.join("instance.sock");
        if let Ok(mut s) = UnixStream::connect(&path)
            && s.write_all(FOCUS_MSG).is_ok()
        {
            return Instance::Secondary;
        }
        let _ = std::fs::create_dir_all(&paths.data_dir);
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "single-instance socket unavailable");
                return Instance::Primary(InstanceGuard::detached());
            }
        };
        let (tx, rx) = mpsc::unbounded();
        let spawned = std::thread::Builder::new()
            .name("dk-instance".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(mut conn) = conn else { continue };
                    let mut buf = [0u8; 16];
                    let _ = conn.read(&mut buf);
                    if tx.unbounded_send(()).is_err() {
                        return;
                    }
                }
            });
        if spawned.is_err() {
            return Instance::Primary(InstanceGuard::detached());
        }
        Instance::Primary(InstanceGuard {
            _private: (),
            focus: Some(rx),
            _socket: Some(SocketCleanup(path)),
        })
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::io::{self, Write};
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_DATA, ERROR_PIPE_BUSY,
        ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, ReadFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
        PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::*;

    /// Pipe name scoped to the user SID (unforgeable, unlike `%USERNAME%`).
    pub(super) fn pipe_name(sid: &str) -> String {
        let sid: String = sid
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!(r"\\.\pipe\dockering-instance-{sid}")
    }

    /// Protected DACL with a single allow ACE: `GENERIC_ALL` for `sid` (the same approach as
    /// `dk-wsl`'s bridge pipes, NFR-021).
    pub(super) fn sddl_for_sid(sid: &str) -> String {
        format!("D:P(A;;GA;;;{sid})")
    }

    /// Owned kernel handle, closed on drop.
    struct OwnedHandle(HANDLE);

    // SAFETY: a kernel object HANDLE is usable from any thread.
    unsafe impl Send for OwnedHandle {}

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid handle we own, closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Memory a Win32 API documents as freed with `LocalFree`.
    struct OwnedLocal(HLOCAL);

    impl Drop for OwnedLocal {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from an API that requires LocalFree; freed once.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    fn wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Copies a LocalAlloc'ed NUL-terminated UTF-16 string and frees it.
    ///
    /// # Safety
    /// `ptr` must be non-null, NUL-terminated, readable, and owned by the caller (LocalFree).
    unsafe fn take_local_wide(ptr: *mut u16) -> String {
        let owned = OwnedLocal(ptr.cast());
        let mut len = 0usize;
        // SAFETY: guaranteed by the caller: every unit up to and including the NUL is readable.
        let out = unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        };
        drop(owned);
        out
    }

    /// `S-1-5-…` string form of the user SID in `token`.
    fn token_user_sid(token: &OwnedHandle) -> io::Result<String> {
        let mut bytes = 0u32;
        // SAFETY: a null buffer of length 0 is the documented way to query the required size.
        let first = unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes) };
        if first != 0 {
            return Err(io::Error::other("GetTokenInformation size query succeeded"));
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(err);
        }
        // usize storage gives pointer alignment, enough for TOKEN_USER and its trailing SID.
        let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: `storage` is aligned, writable and at least `bytes` long; the token is valid.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                storage.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation(TokenUser) initialised the buffer as a TOKEN_USER whose
        // SID pointer points into `storage`, which outlives this read.
        let sid: PSID = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = null_mut();
        // SAFETY: `sid` is a valid SID inside `storage`; `text` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success `text` is a LocalAlloc'ed NUL-terminated string we now own.
        Ok(unsafe { take_local_wide(text) })
    }

    /// The user SID of `process` (a handle with at least `PROCESS_QUERY_LIMITED_INFORMATION`).
    fn process_user_sid(process: HANDLE) -> io::Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: `process` is a valid process handle; `token` is a valid out pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        token_user_sid(&OwnedHandle(token))
    }

    /// The current process token's user SID.
    pub(super) fn current_user_sid() -> io::Result<String> {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing.
        process_user_sid(unsafe { GetCurrentProcess() })
    }

    /// Whether the server end of the connected client pipe `pipe` runs as `sid`. Any error
    /// counts as "no" (fail safe: the pipe isn't trusted).
    fn server_is_user(pipe: HANDLE, sid: &str) -> bool {
        let check = || -> io::Result<bool> {
            let mut pid = 0u32;
            // SAFETY: `pipe` is a live client pipe handle; `pid` is a valid out pointer.
            if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: plain call with a pid; a null return is handled below.
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let process = OwnedHandle(process);
            Ok(process_user_sid(process.0)? == sid)
        };
        match check() {
            Ok(same) => same,
            Err(e) => {
                tracing::warn!(error = %e, "single-instance: couldn't verify the pipe owner");
                false
            }
        }
    }

    /// A self-relative security descriptor built from SDDL (freed with LocalFree).
    struct SecurityDescriptor(OwnedLocal);

    impl SecurityDescriptor {
        fn from_sddl(sddl: &str) -> io::Result<Self> {
            let wide = wide_nul(sddl);
            let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
            // SAFETY: `wide` is NUL-terminated; `descriptor` is a valid out pointer; the size
            // output is optional.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(OwnedLocal(descriptor.cast())))
        }
    }

    /// Server pipe instance, closed on drop.
    struct Pipe(OwnedHandle);

    /// Creates a server instance only the SID in `sddl` can open (protected DACL).
    fn create(name: &[u16], sddl: &str, first: bool) -> Option<Pipe> {
        let descriptor = match SecurityDescriptor::from_sddl(sddl) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "single-instance: invalid pipe security descriptor");
                return None;
            }
        };
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: 0,
        };
        let mut mode = PIPE_ACCESS_INBOUND;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; `attrs`
        // points to a valid descriptor (`descriptor`) that stays alive until this synchronous
        // call returns (the kernel copies it).
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64,
                64,
                0,
                &attrs,
            )
        };
        drop(descriptor);
        (h != INVALID_HANDLE_VALUE && !h.is_null()).then(|| Pipe(OwnedHandle(h)))
    }

    /// `sid` is the current user's SID: the pipe's DACL grants only it, and a running
    /// instance is trusted only if its server process runs as it.
    pub(super) fn acquire(name: &str, sid: &str) -> Instance {
        // A busy pipe (primary between instances) is retried briefly.
        for _ in 0..5 {
            match std::fs::OpenOptions::new().write(true).open(name) {
                Ok(mut f) => {
                    if !server_is_user(f.as_raw_handle(), sid) {
                        // Squatted by another user: never exit on its behalf, and don't
                        // listen on a name we don't own.
                        tracing::warn!("single-instance pipe is owned by another user");
                        return Instance::Primary(InstanceGuard::detached());
                    }
                    if f.write_all(FOCUS_MSG).is_ok() {
                        return Instance::Secondary;
                    }
                    break;
                }
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        let wide = wide_nul(name);
        let sddl = sddl_for_sid(sid);
        let Some(first) = create(&wide, &sddl, true) else {
            tracing::warn!("single-instance pipe unavailable");
            return Instance::Primary(InstanceGuard::detached());
        };
        let (tx, rx) = mpsc::unbounded();
        let spawned = std::thread::Builder::new()
            .name("dk-instance".into())
            .spawn(move || serve(first, wide, sddl, tx));
        if spawned.is_err() {
            return Instance::Primary(InstanceGuard::detached());
        }
        Instance::Primary(InstanceGuard {
            _private: (),
            focus: Some(rx),
        })
    }

    fn serve(first: Pipe, wide: Vec<u16>, sddl: String, tx: mpsc::UnboundedSender<()>) {
        let mut pipe = Some(first);
        loop {
            let p = match pipe.take().or_else(|| create(&wide, &sddl, false)) {
                Some(p) => p,
                None => return,
            };
            let h = p.0.0;
            // SAFETY: `h` is a valid pipe handle; synchronous (no OVERLAPPED).
            let ok = unsafe { ConnectNamedPipe(h, null_mut()) } != 0
                // SAFETY: reading the calling thread's last-error value.
                || matches!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED | ERROR_NO_DATA);
            if ok {
                let mut buf = [0u8; 16];
                let mut read = 0u32;
                // SAFETY: `buf` is valid for `buf.len()` bytes and `read` for one u32.
                unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut read, null_mut()) };
                // SAFETY: `h` is a connected server pipe handle.
                unsafe { DisconnectNamedPipe(h) };
                // Only an explicit request counts: a client that connects and leaves without
                // writing (e.g. one that rejected us as owner) doesn't steal focus.
                let msg = &buf[..(read as usize).min(buf.len())];
                if msg == FOCUS_MSG && tx.unbounded_send(()).is_err() {
                    return;
                }
            }
            drop(p);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures::StreamExt;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

        fn test_name() -> String {
            format!(
                r"\\.\pipe\dockering-instance-test-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            )
        }

        /// Next focus request within `t` (on a helper thread).
        fn recv_within(mut rx: mpsc::UnboundedReceiver<()>, t: std::time::Duration) -> Option<()> {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = done_tx.send(futures::executor::block_on(rx.next()));
            });
            done_rx.recv_timeout(t).ok().flatten()
        }

        #[test]
        fn shl_022_second_instance_focuses_first() {
            let sid = current_user_sid().unwrap();
            let name = test_name();
            let Instance::Primary(mut g) = acquire(&name, &sid) else {
                panic!("first acquire must be primary");
            };
            let rx = g.take_focus_requests().unwrap();
            assert!(g.take_focus_requests().is_none());
            assert!(matches!(acquire(&name, &sid), Instance::Secondary));
            assert_eq!(recv_within(rx, std::time::Duration::from_secs(2)), Some(()));
        }

        #[test]
        fn shl_022_pipe_name_uses_the_user_sid() {
            let sid = current_user_sid().unwrap();
            assert!(sid.starts_with("S-1-5-"), "{sid}");
            assert_eq!(
                pipe_name(&sid),
                format!(r"\\.\pipe\dockering-instance-{sid}")
            );
            assert_eq!(
                pipe_name(r"S-1\..\x"),
                r"\\.\pipe\dockering-instance-S-1____x"
            );
        }

        /// NFR-021: the pipe's DACL is protected and grants only the current user.
        #[test]
        fn shl_022_pipe_dacl_grants_only_the_current_user() {
            let sid = current_user_sid().unwrap();
            let wide = wide_nul(&test_name());
            let pipe = create(&wide, &sddl_for_sid(&sid), true).expect("create pipe");
            // FILE_FLAG_FIRST_PIPE_INSTANCE: a second "first" instance fails (no squatting).
            assert!(create(&wide, &sddl_for_sid(&sid), true).is_none());

            let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
            // SAFETY: `pipe` is a live handle we created (owner → READ_CONTROL); the out
            // pointers are valid and the optional ones null.
            let status = unsafe {
                GetSecurityInfo(
                    pipe.0.0,
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut descriptor,
                )
            };
            assert_eq!(status, 0);
            let descriptor = OwnedLocal(descriptor.cast());
            let mut text: *mut u16 = null_mut();
            // SAFETY: `descriptor` is a valid descriptor; `text` is a valid out pointer.
            let ok = unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor.0.cast(),
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    null_mut(),
                )
            };
            assert_ne!(ok, 0);
            // SAFETY: on success `text` is a LocalAlloc'ed NUL-terminated string we now own.
            let sddl = unsafe { take_local_wide(text) };
            // `GA` on a pipe maps to FILE_ALL_ACCESS (`FA`); `P` = protected (no inheritance).
            assert_eq!(sddl, format!("D:P(A;;FA;;;{sid})"));
        }

        /// A running instance whose server process isn't the expected user isn't trusted:
        /// we stay primary (detached) and send it nothing.
        #[test]
        fn shl_022_pipe_of_another_user_is_not_trusted() {
            let sid = current_user_sid().unwrap();
            let name = test_name();
            let Instance::Primary(mut g) = acquire(&name, &sid) else {
                panic!("first acquire must be primary");
            };
            let rx = g.take_focus_requests().unwrap();
            // Pretend to be LocalSystem: the server (this process) is then "another user".
            match acquire(&name, "S-1-5-18") {
                Instance::Primary(mut g2) => assert!(g2.take_focus_requests().is_none()),
                Instance::Secondary => panic!("must not exit for a foreign pipe owner"),
            }
            assert_eq!(recv_within(rx, std::time::Duration::from_millis(300)), None);
        }
    }
}
