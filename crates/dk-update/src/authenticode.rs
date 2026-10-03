//! Authenticode check of a downloaded installer (UPD-003): when the running `dockering.exe` is
//! signed, the installer must carry a valid signature from the same signer. "Same" means the
//! full subject DN *and* the issuer DN of the signing certificate, not just the display name
//! (e.g. every SignPath Foundation project shares the CN "SignPath Foundation").

use std::path::Path;

use crate::UpdateError;

/// Outcome of [`check_installer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustCheck {
    /// Valid signature, same subject as the running app.
    Trusted,
    /// The running app is unsigned (dev or private-phase build), so there's nothing to pin to.
    /// The minisign manifest and SHA-256 checks still apply.
    Skipped,
}

/// The decision, separated from the Win32 calls so it can be tested everywhere.
pub fn decide(
    running_subject: Option<&str>,
    installer_subject: Option<&str>,
) -> Result<TrustCheck, UpdateError> {
    match (running_subject, installer_subject) {
        (None, _) => Ok(TrustCheck::Skipped),
        (Some(running), Some(installer)) if running == installer => Ok(TrustCheck::Trusted),
        (Some(_), _) => Err(UpdateError::UntrustedInstaller),
    }
}

/// Checks `installer` against the running executable `running_exe`.
///
/// The pin is skipped only when the running exe has **no** embedded signature (dev and
/// private-phase builds). If it has one that doesn't verify, the check fails closed.
pub fn check_installer(installer: &Path, running_exe: &Path) -> Result<TrustCheck, UpdateError> {
    let running = match imp::signature(running_exe) {
        Signature::None => None,
        Signature::Invalid => return Err(UpdateError::UntrustedInstaller),
        Signature::Valid(id) => Some(id),
    };
    let installer = match imp::signature(installer) {
        Signature::Valid(id) => Some(id),
        Signature::None | Signature::Invalid => None,
    };
    decide(running.as_deref(), installer.as_deref())
}

/// The signer identity (`subject DN` + `issuer DN`) when `path` has a valid, trusted embedded
/// Authenticode signature; `None` when it is unsigned or the signature doesn't verify.
pub fn signer_subject(path: &Path) -> Result<Option<String>, UpdateError> {
    Ok(match imp::signature(path) {
        Signature::Valid(id) => Some(id),
        Signature::None | Signature::Invalid => None,
    })
}

/// Embedded Authenticode signature state of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Signature {
    /// No embedded signature at all.
    None,
    /// Has one, but WinVerifyTrust rejects it (tampered, untrusted chain, …).
    Invalid,
    /// Trusted; the signer identity.
    Valid(String),
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub(super) fn signature(_path: &Path) -> Signature {
        Signature::None
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_NAME_ISSUER_FLAG, CERT_NAME_RDN_TYPE,
        CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY,
        CERT_QUERY_OBJECT_FILE, CERT_X500_NAME_STR, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM,
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW,
        CryptMsgClose, CryptMsgGetParam, CryptQueryObject, HCERTSTORE, PKCS_7_ASN_ENCODING,
        X509_ASN_ENCODING,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };

    use super::*;

    pub(super) fn signature(path: &Path) -> Signature {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // `identity` reads the embedded PKCS#7 without judging it: absent → unsigned.
        let Some(id) = identity(&wide) else {
            return Signature::None;
        };
        if verify_trust(&wide) {
            Signature::Valid(id)
        } else {
            Signature::Invalid
        }
    }

    /// WinVerifyTrust with the generic Authenticode policy, no UI, no revocation round-trips.
    fn verify_trust(wide_path: &[u16]) -> bool {
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide_path.as_ptr(),
            hFile: null_mut(),
            pgKnownSubject: null_mut(),
        };
        // SAFETY: WINTRUST_DATA is a plain C struct; all-zero is a valid starting value and
        // every field the call reads is set below.
        let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` and `file` (and the path buffer it points to) outlive both calls; the
        // second call releases the state the first one allocated.
        unsafe {
            let status = WinVerifyTrust(
                null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(
                null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
            status == 0
        }
    }

    struct Store(HCERTSTORE);
    impl Drop for Store {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a store handle returned by CryptQueryObject, closed once.
                unsafe { CertCloseStore(self.0, 0) };
            }
        }
    }

    struct Msg(*mut core::ffi::c_void);
    impl Drop for Msg {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a message handle returned by CryptQueryObject, closed once.
                unsafe { CryptMsgClose(self.0) };
            }
        }
    }

    struct Cert(*mut CERT_CONTEXT);
    impl Drop for Cert {
        fn drop(&mut self) {
            // SAFETY: a context returned by CertFindCertificateInStore, freed once.
            unsafe { CertFreeCertificateContext(self.0) };
        }
    }

    /// `subject DN | issuer DN` of the embedded signature's signer certificate.
    fn identity(wide_path: &[u16]) -> Option<String> {
        let encoding = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;
        let mut store: HCERTSTORE = null_mut();
        let mut msg: *mut core::ffi::c_void = null_mut();
        // SAFETY: the path is NUL-terminated; out-pointers are valid locals; unused outputs null.
        let ok = unsafe {
            CryptQueryObject(
                CERT_QUERY_OBJECT_FILE,
                wide_path.as_ptr().cast(),
                CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
                CERT_QUERY_FORMAT_FLAG_BINARY,
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut store,
                &mut msg,
                null_mut(),
            )
        };
        let (store, msg) = (Store(store), Msg(msg));
        if ok == 0 {
            return None;
        }

        let mut len = 0u32;
        // SAFETY: size query with a null buffer.
        if unsafe { CryptMsgGetParam(msg.0, CMSG_SIGNER_INFO_PARAM, 0, null_mut(), &mut len) } == 0
            || (len as usize) < size_of::<CMSG_SIGNER_INFO>()
        {
            return None;
        }
        // u64 storage keeps the buffer aligned for CMSG_SIGNER_INFO.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `len` bytes.
        if unsafe {
            CryptMsgGetParam(
                msg.0,
                CMSG_SIGNER_INFO_PARAM,
                0,
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        } == 0
        {
            return None;
        }
        // SAFETY: the call filled `buf` with a CMSG_SIGNER_INFO whose blobs point into `buf`.
        let signer = unsafe { &*buf.as_ptr().cast::<CMSG_SIGNER_INFO>() };
        // SAFETY: CERT_INFO is plain data; only Issuer and SerialNumber are read by the find.
        let mut info: CERT_INFO = unsafe { std::mem::zeroed() };
        info.Issuer = signer.Issuer;
        info.SerialNumber = signer.SerialNumber;
        // SAFETY: `store` is open; `info` and `buf` outlive the call.
        let cert = unsafe {
            CertFindCertificateInStore(
                store.0,
                encoding,
                0,
                CERT_FIND_SUBJECT_CERT,
                (&info as *const CERT_INFO).cast(),
                null(),
            )
        };
        if cert.is_null() {
            return None;
        }
        let cert = Cert(cert);

        let subject = name_string(&cert, 0)?;
        let issuer = name_string(&cert, CERT_NAME_ISSUER_FLAG)?;
        Some(format!("{subject} | {issuer}"))
    }

    /// The full X.500 name (`CN=…, O=…, C=…`) of the subject, or the issuer with
    /// `CERT_NAME_ISSUER_FLAG`.
    fn name_string(cert: &Cert, flags: u32) -> Option<String> {
        let ty = CERT_X500_NAME_STR;
        let para = (&ty as *const u32).cast();
        // SAFETY: size query; `para` points at the string type for CERT_NAME_RDN_TYPE.
        let n =
            unsafe { CertGetNameStringW(cert.0, CERT_NAME_RDN_TYPE, flags, para, null_mut(), 0) };
        if n <= 1 {
            return None;
        }
        let mut name = vec![0u16; n as usize];
        // SAFETY: `name` has room for `n` units including the terminator.
        unsafe {
            CertGetNameStringW(
                cert.0,
                CERT_NAME_RDN_TYPE,
                flags,
                para,
                name.as_mut_ptr(),
                n,
            )
        };
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..end]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upd_003_untrusted_installer_rejected() {
        assert_eq!(decide(None, None), Ok(TrustCheck::Skipped));
        assert_eq!(decide(None, Some("Anyone")), Ok(TrustCheck::Skipped));
        assert_eq!(
            decide(Some("SignPath Foundation"), Some("SignPath Foundation")),
            Ok(TrustCheck::Trusted)
        );
        assert_eq!(
            decide(Some("SignPath Foundation"), None),
            Err(UpdateError::UntrustedInstaller)
        );
        assert_eq!(
            decide(Some("SignPath Foundation"), Some("Mallory")),
            Err(UpdateError::UntrustedInstaller)
        );
    }

    #[test]
    fn authenticode_unsigned_file_has_no_subject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("unsigned.exe");
        std::fs::write(&path, b"MZ not really an executable").expect("write");
        assert_eq!(signer_subject(&path), Ok(None));
    }

    /// Needs a file with an *embedded* signature (most of System32 is catalog-signed).
    #[cfg(windows)]
    #[test]
    fn authenticode_embedded_signature_has_subject() {
        let candidates = [
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r"C:\Program Files\Git\cmd\git.exe",
        ];
        let Some(path) = candidates.iter().map(Path::new).find(|p| p.is_file()) else {
            eprintln!("no embedded-signed test file on this machine; skipping");
            return;
        };
        let subject = signer_subject(path).expect("query");
        assert!(subject.is_some_and(|s| !s.is_empty()), "{}", path.display());
    }
}
