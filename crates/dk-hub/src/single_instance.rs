//! Single instance per user (SHL-022). A second launch asks the running instance to focus its
//! window and exits. Windows: named pipe `\\.\pipe\dockering-instance-<user>`;
//! Unix: socket `<data_dir>/instance.sock`.

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
    win::acquire(&win::pipe_name())
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
    use std::io::Write;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, ReadFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    use super::*;

    pub(super) fn pipe_name() -> String {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        format!(r"\\.\pipe\dockering-instance-{user}")
    }

    /// Owned pipe handle, closed on drop.
    struct Pipe(HANDLE);

    // SAFETY: a pipe HANDLE is a kernel object handle usable from any thread.
    unsafe impl Send for Pipe {}

    impl Drop for Pipe {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid handle we own, closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn create(name: &[u16], first: bool) -> Option<Pipe> {
        let mut mode = PIPE_ACCESS_INBOUND;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; a null
        // security-attributes pointer selects the default DACL (creator/owner only).
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64,
                64,
                0,
                std::ptr::null(),
            )
        };
        (h != INVALID_HANDLE_VALUE && !h.is_null()).then_some(Pipe(h))
    }

    pub(super) fn acquire(name: &str) -> Instance {
        // A busy pipe (primary between instances) is retried briefly.
        for _ in 0..5 {
            match std::fs::OpenOptions::new().write(true).open(name) {
                Ok(mut f) => {
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
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let Some(first) = create(&wide, true) else {
            tracing::warn!("single-instance pipe unavailable");
            return Instance::Primary(InstanceGuard::detached());
        };
        let (tx, rx) = mpsc::unbounded();
        let spawned = std::thread::Builder::new()
            .name("dk-instance".into())
            .spawn(move || serve(first, wide, tx));
        if spawned.is_err() {
            return Instance::Primary(InstanceGuard::detached());
        }
        Instance::Primary(InstanceGuard {
            _private: (),
            focus: Some(rx),
        })
    }

    fn serve(first: Pipe, wide: Vec<u16>, tx: mpsc::UnboundedSender<()>) {
        let mut pipe = Some(first);
        loop {
            let p = match pipe.take().or_else(|| create(&wide, false)) {
                Some(p) => p,
                None => return,
            };
            // SAFETY: `p.0` is a valid pipe handle; synchronous (no OVERLAPPED).
            let ok = unsafe { ConnectNamedPipe(p.0, std::ptr::null_mut()) } != 0
                // SAFETY: reading the calling thread's last-error value.
                || matches!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED | ERROR_NO_DATA);
            // Any connection is a focus request; the payload is informational.
            if ok {
                let mut buf = [0u8; 16];
                let mut read = 0u32;
                // SAFETY: `buf` is valid for `buf.len()` bytes and `read` for one u32.
                unsafe {
                    ReadFile(
                        p.0,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                };
                // SAFETY: `p.0` is a connected server pipe handle.
                unsafe { DisconnectNamedPipe(p.0) };
                if tx.unbounded_send(()).is_err() {
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

        #[test]
        fn shl_022_second_instance_focuses_first() {
            let name = format!(r"\\.\pipe\dockering-instance-test-{}", std::process::id());
            let Instance::Primary(mut g) = acquire(&name) else {
                panic!("first acquire must be primary");
            };
            let mut rx = g.take_focus_requests().unwrap();
            assert!(g.take_focus_requests().is_none());
            assert!(matches!(acquire(&name), Instance::Secondary));
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = done_tx.send(futures::executor::block_on(rx.next()));
            });
            let got = done_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .ok()
                .flatten();
            assert_eq!(got, Some(()));
        }
    }
}
