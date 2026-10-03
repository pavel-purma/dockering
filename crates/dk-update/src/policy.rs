//! Administrator policy that turns updates off (UPD-005).

/// Windows: `DisableUpdates` (DWORD 1) under `HKLM` or `HKCU\Software\Policies\Dockering`.
/// Always `false` elsewhere. Read once at hub start.
#[must_use]
pub fn updates_disabled_by_policy() -> bool {
    imp::disabled()
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn disabled() -> bool {
        false
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::ptr::null_mut;

    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
    };

    pub(super) fn disabled() -> bool {
        [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
            .into_iter()
            .any(|root| read_dword(root) == Some(1))
    }

    fn read_dword(root: HKEY) -> Option<u32> {
        let key: Vec<u16> = "Software\\Policies\\Dockering\0".encode_utf16().collect();
        let value: Vec<u16> = "DisableUpdates\0".encode_utf16().collect();
        let mut data = 0u32;
        let mut len = size_of::<u32>() as u32;
        // SAFETY: both names are NUL-terminated UTF-16 that outlive the call; `data` has room
        // for the DWORD the flags restrict the value to, and `len` says so.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut data as *mut u32).cast(),
                &mut len,
            )
        };
        (status == 0).then_some(data)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn upd_005_policy_read_does_not_panic() {
        // The policy key normally doesn't exist on dev machines and CI runners.
        let _ = super::updates_disabled_by_policy();
    }
}
