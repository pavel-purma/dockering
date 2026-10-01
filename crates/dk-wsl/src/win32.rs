//! Win32 FFI for this crate: the Lxss registry and named-pipe security (NFR-021).
//!
//! All `unsafe` in dk-wsl is confined to this module. Every block has a `// SAFETY:` comment.

#![cfg(windows)]

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const LXSS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

/// One `HKCU\…\Lxss\{GUID}` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryDistro {
    pub(crate) name: String,
    pub(crate) version: u32,
    pub(crate) state: Option<u32>,
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by OpenProcessToken, is owned here, and closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Memory allocated by a Win32 API that documents `LocalFree` as its deallocator.
struct OwnedLocal(HLOCAL);

impl Drop for OwnedLocal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: The pointer came from an API documented to require LocalFree; freed once.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

struct OwnedRegKey(HKEY);

impl Drop for OwnedRegKey {
    fn drop(&mut self) {
        // SAFETY: The key was opened successfully by RegOpenKeyExW, is owned here, closed once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide_nul(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Copies a NUL-terminated UTF-16 string.
///
/// # Safety
/// `ptr` must be non-null and point to a NUL-terminated UTF-16 string valid for reads.
unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: Guaranteed by the caller: every unit up to and including the NUL is readable.
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

/// `S-1-5-…` string form of a SID.
///
/// # Safety
/// `sid` must point to a valid SID that stays alive for the call.
unsafe fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut value: *mut u16 = null_mut();
    // SAFETY: `sid` is valid (caller contract) and `value` is a valid out pointer.
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = OwnedLocal(value.cast());
    // SAFETY: On success, `value` is a LocalAlloc'ed NUL-terminated string owned by `owned`.
    let result = unsafe { wide_ptr_to_string(value) };
    drop(owned);
    Ok(result)
}

/// The current process token's user SID as a string.
pub(crate) fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle valid for OpenProcessToken; `token` is
    // a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);

    let mut bytes = 0u32;
    // SAFETY: A null buffer of length 0 is the documented way to query the required size.
    let first = unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes) };
    if first != 0 {
        return Err(io::Error::other(
            "GetTokenInformation size query unexpectedly succeeded",
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }

    // usize storage gives pointer alignment, enough for TOKEN_USER and its trailing SID.
    let words = (bytes as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    // SAFETY: `storage` is aligned, writable, and at least `bytes` long; the token is valid.
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

    // SAFETY: GetTokenInformation(TokenUser) initialised the buffer as a TOKEN_USER whose SID
    // pointer points into `storage`, which outlives this read.
    let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` points into `storage`, which is alive for the call.
    unsafe { sid_to_string(sid) }
}

/// Protected DACL with a single allow ACE: `GENERIC_ALL` for `sid` (NFR-021).
pub(crate) fn sddl_for_sid(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// A self-relative security descriptor allocated by `ConvertStringSecurityDescriptor…`.
struct SecurityDescriptor(OwnedLocal);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let wide = wide_nul(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated; `descriptor` is a valid out pointer; the size output
        // is optional.
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
        Ok(Self(OwnedLocal(descriptor.cast())))
    }

    fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0.0.cast()
    }
}

/// Creates bridge pipe instances that only the current user can open (NFR-021).
pub(crate) struct PipeSecurity {
    sddl: String,
}

impl PipeSecurity {
    pub(crate) fn current_user() -> io::Result<Self> {
        let sddl = sddl_for_sid(&current_user_sid()?);
        // Validate once so later instance creation can't fail on the descriptor.
        SecurityDescriptor::from_sddl(&sddl)?;
        Ok(Self { sddl })
    }

    /// One pipe instance. `first` sets `FILE_FLAG_FIRST_PIPE_INSTANCE` (fails if the name is
    /// already taken — no squatting). Remote clients are always rejected.
    pub(crate) fn create_pipe(&self, path: &str, first: bool) -> io::Result<NamedPipeServer> {
        let descriptor = SecurityDescriptor::from_sddl(&self.sddl)?;
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: 0,
        };
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);

        // SAFETY: `attrs` is a valid SECURITY_ATTRIBUTES whose descriptor (`descriptor`) stays
        // alive until after this synchronous CreateNamedPipeW call returns; the kernel copies it.
        let server =
            unsafe { options.create_with_security_attributes_raw(path, (&raw mut attrs).cast()) };
        drop(descriptor);
        server
    }
}

/// Enumerates `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss\{GUID}` entries.
pub(crate) fn enumerate_distros() -> io::Result<Vec<RegistryDistro>> {
    let root_name = wide_nul(LXSS_KEY);
    let mut root: HKEY = null_mut();
    // SAFETY: HKEY_CURRENT_USER is a predefined key; the path is NUL-terminated; `root` is a
    // valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            root_name.as_ptr(),
            0,
            KEY_READ,
            &mut root,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        // WSL was never used by this user: no distros, not an error.
        return Ok(Vec::new());
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let root = OwnedRegKey(root);
    let mut result = Vec::new();

    for index in 0u32.. {
        // Subkeys are GUIDs (38 chars); 256 is the registry key-name limit.
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        // SAFETY: `root` is open; the name buffer holds `name_len` u16s; optional outputs are null.
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
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let subkey = wide_nul(String::from_utf16_lossy(&name[..name_len as usize]));
        let mut key: HKEY = null_mut();
        // SAFETY: `root` is open; the subkey name is NUL-terminated; `key` is a valid out pointer.
        let status = unsafe { RegOpenKeyExW(root.0, subkey.as_ptr(), 0, KEY_READ, &mut key) };
        if status != ERROR_SUCCESS {
            continue;
        }
        let key = OwnedRegKey(key);
        let (Some(distribution_name), Some(version)) = (
            query_string(&key, "DistributionName"),
            query_dword(&key, "Version"),
        ) else {
            continue;
        };
        result.push(RegistryDistro {
            name: distribution_name,
            version,
            state: query_dword(&key, "State"),
        });
    }
    Ok(result)
}

fn query_string(key: &OwnedRegKey, name: &str) -> Option<String> {
    let name = wide_nul(name);
    let mut kind = 0u32;
    let mut bytes = 0u32;
    // SAFETY: `key` is open; the name is NUL-terminated; a null data pointer queries the size.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            null_mut(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ || !(2..=64 * 1024).contains(&bytes) {
        return None;
    }
    let mut value = vec![0u16; (bytes as usize).div_ceil(2)];
    let mut capacity = (value.len() * 2) as u32;
    // SAFETY: `value` is writable for `capacity` bytes; other pointers are valid.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut capacity,
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ {
        return None;
    }
    value.truncate((capacity as usize) / 2);
    while value.last() == Some(&0) {
        value.pop();
    }
    Some(String::from_utf16_lossy(&value))
}

fn query_dword(key: &OwnedRegKey, name: &str) -> Option<u32> {
    let name = wide_nul(name);
    let mut kind = 0u32;
    let mut value = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: `key` is open; the output points to a writable u32 of the supplied size.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            (&raw mut value).cast(),
            &mut bytes,
        )
    };
    (status == ERROR_SUCCESS && kind == REG_DWORD && bytes == size_of::<u32>() as u32)
        .then_some(value)
}

/// Test-only DACL inspection (NFR-021 ACL test).
#[cfg(test)]
pub(crate) use dacl::object_dacl;

#[cfg(test)]
mod dacl {
    use std::ffi::c_void;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::RawHandle;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };

    use super::{OwnedLocal, sid_to_string};

    /// `ACCESS_ALLOWED_ACE_TYPE` from winnt.h (windows-sys has it as `u32` under
    /// `SystemServices`; `ACE_HEADER::AceType` is a `u8`).
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    #[derive(Debug)]
    pub(crate) struct Dacl {
        pub(crate) protected: bool,
        /// SIDs of the access-allowed ACEs, in order.
        pub(crate) allowed_sids: Vec<String>,
        /// Access masks of the access-allowed ACEs, in order.
        pub(crate) allowed_masks: Vec<u32>,
        /// Number of ACEs of any other type.
        pub(crate) other_aces: usize,
    }

    /// Reads the DACL of a kernel object (the handle needs `READ_CONTROL`).
    pub(crate) fn object_dacl(handle: RawHandle) -> io::Result<Dacl> {
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `handle` is a live kernel-object handle owned by the caller; output pointers
        // are valid; owner/group/SACL outputs are optional and null.
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
        // `dacl` points into `descriptor`, which must outlive every use below.
        let descriptor = OwnedLocal(descriptor.cast());
        if dacl.is_null() {
            return Err(io::Error::other(
                "object has a NULL DACL (everyone allowed)",
            ));
        }

        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `descriptor` is a live descriptor returned above; outputs are valid.
        if unsafe { GetSecurityDescriptorControl(descriptor.0.cast(), &mut control, &mut revision) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: All-zero is a valid ACL_SIZE_INFORMATION (plain integers).
        let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        // SAFETY: `dacl` is valid inside `descriptor`; `info` is a correctly sized output.
        if unsafe {
            GetAclInformation(
                dacl,
                (&raw mut info).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let mut result = Dacl {
            protected: control & SE_DACL_PROTECTED != 0,
            allowed_sids: Vec::new(),
            allowed_masks: Vec::new(),
            other_aces: 0,
        };
        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `index` < AceCount of this live DACL; `ace` is a valid out pointer.
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: GetAce returned a pointer to an ACE, which starts with an ACE_HEADER.
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                result.other_aces += 1;
                continue;
            }
            // SAFETY: The type says this is an ACCESS_ALLOWED_ACE.
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            // SAFETY: For ACCESS_ALLOWED_ACE, the SID starts at `SidStart` and lies within the
            // ACE inside the live descriptor.
            let sid = unsafe { sid_to_string((&raw const allowed.SidStart).cast_mut().cast())? };
            result.allowed_sids.push(sid);
            result.allowed_masks.push(allowed.Mask);
        }
        drop(descriptor);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_021_sddl_is_protected_and_grants_only_the_supplied_sid() {
        assert_eq!(
            sddl_for_sid("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
    }

    #[test]
    fn nfr_021_current_user_sid_is_a_user_sid() {
        let sid = current_user_sid().expect("SID");
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert!(PipeSecurity::current_user().is_ok());
    }

    /// A pipe created by `PipeSecurity` has a protected DACL with exactly one allow ACE: the
    /// current user, full access (`GA` maps to `FILE_ALL_ACCESS` for pipes). No `wsl.exe` involved.
    #[tokio::test]
    async fn nfr_021_pipe_dacl_grants_only_the_current_user() {
        use std::os::windows::io::AsRawHandle;

        const FILE_ALL_ACCESS: u32 = 0x001F_01FF;
        let security = PipeSecurity::current_user().expect("security");
        let path = format!(
            r"\\.\pipe\dockering-wsl-acl-test-{:016x}",
            rand::random::<u64>()
        );
        let server = security.create_pipe(&path, true).expect("create pipe");
        // FILE_FLAG_FIRST_PIPE_INSTANCE: a second "first" instance must fail (no name squatting).
        assert!(security.create_pipe(&path, true).is_err());

        let dacl = object_dacl(server.as_raw_handle()).expect("read DACL");
        let me = current_user_sid().expect("SID");
        assert!(dacl.protected);
        assert_eq!(dacl.allowed_sids, vec![me]);
        assert_eq!(dacl.allowed_masks, vec![FILE_ALL_ACCESS]);
        assert_eq!(dacl.other_aces, 0);

        // The current user can open it.
        let client = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&path)
            .expect("current user can connect");
        drop(client);
    }

    #[test]
    fn nfr_021_invalid_sddl_is_rejected() {
        assert!(SecurityDescriptor::from_sddl("D:P(A;;GA;;;not-a-sid)").is_err());
    }

    #[test]
    fn eng_007_registry_enumeration_does_not_fail() {
        // On machines without WSL this is empty; with WSL every entry has a name.
        let distros = enumerate_distros().expect("enumerate");
        assert!(distros.iter().all(|distro| !distro.name.is_empty()));
    }
}
