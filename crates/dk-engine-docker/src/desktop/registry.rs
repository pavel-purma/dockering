//! Read-only registry lookup of the Docker Desktop install folder (ENG-143).
//!
//! The only `unsafe` in this crate. Every block has a `// SAFETY:` comment.

#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Docker Desktop";
const VALUE: &str = "InstallLocation";
/// Upper bound for the value (bytes); a path is far shorter.
const MAX_BYTES: u32 = 32 * 1024;

fn wide_nul(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// `InstallLocation` strings, `HKLM` first, then `HKCU`. Unvalidated.
pub(super) fn install_locations() -> Vec<String> {
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .filter_map(read_location)
        .collect()
}

fn read_location(root: HKEY) -> Option<String> {
    let key = wide_nul(UNINSTALL_KEY);
    let value = wide_nul(VALUE);
    let mut bytes = 0u32;
    // SAFETY: `root` is a predefined key; both names are NUL-terminated; a null data pointer
    // queries the size into `bytes`.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS || !(2..=MAX_BYTES).contains(&bytes) {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    let mut capacity = (buf.len() * 2) as u32;
    // SAFETY: `buf` is writable for `capacity` bytes; the names are NUL-terminated and
    // `capacity` is a valid in/out pointer.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut capacity,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    buf.truncate((capacity as usize / 2).min(buf.len()));
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Some(String::from_utf16_lossy(&buf))
}
