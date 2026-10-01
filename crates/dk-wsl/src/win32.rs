//! Win32 registry and named-pipe security helpers.
//!
//! All direct Win32 FFI for this crate is confined to this module.

#![cfg(windows)]

use std::ffi::OsStr;
#[cfg(test)]
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
#[cfg(test)]
use std::mem::zeroed;
use std::os::windows::ffi::OsStrExt;
#[cfg(test)]
use std::os::windows::io::RawHandle;
use std::ptr::{null, null_mut};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE, HLOCAL,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_ACE_TYPE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetTokenInformation,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const LXSS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryDistro {
    pub(crate) name: String,
    pub(crate) version: u32,
    pub(crate) state: Option<u32>,
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by OpenProcessToken and remains owned here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct OwnedLocal(HLOCAL);

impl Drop for OwnedLocal {
    fn drop(&mut self) {
        // SAFETY: The pointer was allocated by a Win32 API documented to require LocalFree.
        unsafe {
            LocalFree(self.0);
        }
    }
}

struct OwnedRegKey(HKEY);

impl Drop for OwnedRegKey {
    fn drop(&mut self) {
        // SAFETY: The key was opened successfully by RegOpenKeyExW and remains owned here.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide_nul(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn wide_ptr_to_string(mut ptr: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: Callers pass Win32-owned, NUL-terminated strings that remain valid for this call.
    unsafe {
        while *ptr != 0 {
            len += 1;
            ptr = ptr.add(1);
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr.sub(len), len))
    }
}

fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut value = null_mut();
    // SAFETY: `sid` is supplied by a Win32 token/security descriptor and `value` is a valid out pointer.
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = OwnedLocal(value.cast());
    let result = wide_ptr_to_string(value);
    drop(owned);
    Ok(result)
}

pub(crate) fn current_user_sid() -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle valid for OpenProcessToken; token is an out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);

    let mut bytes = 0u32;
    // SAFETY: A null first-pass buffer is the documented way to obtain the required size.
    let first = unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes) };
    if first != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(io::Error::last_os_error());
    }

    // usize storage provides sufficient alignment for TOKEN_USER and its trailing SID.
    let words = (bytes as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    // SAFETY: `storage` is aligned, writable, and at least `bytes` long; token is valid.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: GetTokenInformation(TokenUser) initialized the buffer as TOKEN_USER.
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    sid_to_string(user.User.Sid)
}

pub(crate) fn sddl_for_sid(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: ConvertStringSecurityDescriptor... allocated this descriptor with LocalAlloc.
        unsafe {
            LocalFree(self.0.cast());
        }
    }
}

fn security_descriptor(sddl: &str) -> io::Result<SecurityDescriptor> {
    let wide = wide_nul(sddl);
    let mut descriptor = null_mut();
    // SAFETY: `wide` is NUL-terminated and descriptor is a valid out pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(SecurityDescriptor(descriptor))
}

/// Create one ACL-restricted pipe instance. The descriptor may be freed after CreateNamedPipe returns.
pub(crate) fn create_pipe(path: &str, first_instance: bool) -> io::Result<NamedPipeServer> {
    let sid = current_user_sid()?;
    let descriptor = security_descriptor(&sddl_for_sid(&sid))?;
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first_instance)
        .reject_remote_clients(true);

    // SAFETY: `attrs` and its security descriptor remain valid through this synchronous create call.
    unsafe {
        options.create_with_security_attributes_raw(
            path,
            (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
        )
    }
}

pub(crate) fn enumerate_distros() -> io::Result<Vec<RegistryDistro>> {
    let root_name = wide_nul(LXSS_KEY);
    let mut root = 0;
    // SAFETY: The path is NUL-terminated and root is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            root_name.as_ptr(),
            0,
            KEY_READ,
            &mut root,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let root = OwnedRegKey(root);
    let mut result = Vec::new();
    let mut index = 0u32;

    loop {
        let mut name = vec![0u16; 260];
        let mut name_len = (name.len() - 1) as u32;
        // SAFETY: The output buffer and length pointer are valid; all optional outputs are null.
        let status = unsafe {
            RegEnumKeyExW(
                root.0,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        index += 1;
        name.truncate(name_len as usize);
        let key_name = String::from_utf16_lossy(&name);
        let key_path = wide_nul(format!(r"{LXSS_KEY}\{key_name}"));
        let mut key = 0;
        // SAFETY: The path is NUL-terminated and key is a valid out pointer.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_READ, &mut key) };
        if status != ERROR_SUCCESS {
            continue;
        }
        let key = OwnedRegKey(key);
        let Some(distribution_name) = query_string(key.0, "DistributionName")? else {
            continue;
        };
        let Some(version) = query_dword(key.0, "Version")? else {
            continue;
        };
        let state = query_dword(key.0, "State")?;
        result.push(RegistryDistro {
            name: distribution_name,
            version,
            state,
        });
    }
    Ok(result)
}

fn query_string(key: HKEY, name: &str) -> io::Result<Option<String>> {
    let name = wide_nul(name);
    let mut kind = 0u32;
    let mut bytes = 0u32;
    // SAFETY: Name is NUL-terminated and size/type out pointers are valid.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null(),
            &mut kind,
            null_mut(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Ok(None);
    }
    if kind != REG_SZ || bytes < 2 {
        return Ok(None);
    }
    let mut value = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: The byte-sized output buffer is valid for `bytes`, and other pointers are valid.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    while value.last() == Some(&0) {
        value.pop();
    }
    Ok(Some(String::from_utf16_lossy(&value)))
}

fn query_dword(key: HKEY, name: &str) -> io::Result<Option<u32>> {
    let name = wide_nul(name);
    let mut kind = 0u32;
    let mut value = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: The output points to a writable u32 and its exact byte size is supplied.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null(),
            &mut kind,
            (&mut value as *mut u32).cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Ok(None);
    }
    Ok((kind == REG_DWORD && bytes == size_of::<u32>() as u32).then_some(value))
}

/// Return each allow-ACE SID from a pipe DACL. Used by the ignored live ACL test.
#[cfg(test)]
pub(crate) fn pipe_dacl_sids(handle: RawHandle) -> io::Result<Vec<String>> {
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `handle` is a live named-pipe handle and output pointers remain valid for this call.
    let status = unsafe {
        GetSecurityInfo(
            handle as HANDLE,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let descriptor = OwnedLocal(descriptor.cast());
    if dacl.is_null() {
        return Err(io::Error::other("named pipe has a null DACL"));
    }

    // SAFETY: zero is a valid initial bit-pattern and Win32 initializes the whole structure.
    let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    // SAFETY: `dacl` belongs to the live descriptor and info is a correctly sized output buffer.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    let mut result = Vec::new();
    for index in 0..info.AceCount {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: The index is below the ACE count returned for this DACL; ace is an out pointer.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetAce returned a pointer to at least an ACE header in this live descriptor.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        // SAFETY: For ACCESS_ALLOWED_ACE, SidStart is the first DWORD of the embedded SID.
        let sid = unsafe {
            (&allowed.SidStart as *const u32)
                .cast_mut()
                .cast::<c_void>()
        };
        result.push(sid_to_string(sid)?);
    }
    drop(descriptor);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::sddl_for_sid;

    #[test]
    fn nfr_021_sddl_is_protected_and_grants_only_the_supplied_sid() {
        assert_eq!(
            sddl_for_sid("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
    }
}
