//! Authenticode check of a downloaded installer (UPD-003): when the running `dockering.exe` is
//! signed, the installer must carry a valid signature from the same signer. "Same" means the
//! full subject DN *and* the issuer DN of the signing certificate, not just the display name
//! (a shared signing service can give unrelated projects the same CN).
//!
//! The *decision* (what counts as unsigned, invalid or trusted, and the PE certificate-table
//! probe) is pure and runs on every OS; only the `WinVerifyTrust` / `CryptQueryObject` calls in
//! `imp` are Windows-only.

// Off Windows only the pure decision runs (tests); the Win32 side is not built.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::UpdateError;

/// Outcome of [`check_installer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustCheck {
    /// Valid signature, same subject as the running app.
    Trusted,
    /// The running app has no embedded signature (an unsigned release or a dev build), so there
    /// is nothing to pin to. The minisign manifest and SHA-256 checks still apply (UPD-003).
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
/// The pin is skipped only when the running exe has **no** embedded signature, meaning no
/// certificate table (an unsigned release: it installs on the signed manifest, size and SHA-256
/// alone, UPD-003). A signature that can't be read, parsed or verified is *invalid* and fails
/// closed; so does a signed running exe next to an unsigned or differently-signed installer.
pub fn check_installer(installer: &Path, running_exe: &Path) -> Result<TrustCheck, UpdateError> {
    check_with(imp::signature(running_exe), || imp::signature(installer))
}

/// [`check_installer`] for an installer that is already open, `FILE_SHARE_READ`-locked, in
/// `installer_file` (UPD-007; `installer_path` names the same file). The certificate table and
/// `WinVerifyTrust` read through that handle (`WINTRUST_FILE_INFO.hFile`, ⚠ W1: unverified on
/// Windows); only the signer's name is read from the path, which the lock keeps unchanged.
///
/// Off Windows nothing is signed, so the pin is always skipped, as in [`check_installer`].
pub fn check_installer_open(
    installer_file: &File,
    installer_path: &Path,
    running_exe: &Path,
) -> Result<TrustCheck, UpdateError> {
    check_with(imp::signature(running_exe), || {
        imp::signature_of_open(installer_file, installer_path)
    })
}

/// The trust decision over already-read signature states. The installer is only examined when
/// the running exe has a valid signature, so an unsigned release never calls `WinVerifyTrust`
/// on the installer.
fn check_with(
    running: Signature,
    installer: impl FnOnce() -> Signature,
) -> Result<TrustCheck, UpdateError> {
    let running = match running {
        Signature::None => return decide(None, None),
        Signature::Invalid => return Err(UpdateError::UntrustedInstaller),
        Signature::Valid(id) => id,
    };
    let installer = match installer() {
        Signature::Valid(id) => Some(id),
        Signature::None | Signature::Invalid => None,
    };
    decide(Some(&running), installer.as_deref())
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
    /// No embedded signature: no certificate table, and `WinVerifyTrust` agrees (see
    /// [`classify`]). Nothing else is ever `None`.
    None,
    /// Anything else that isn't trusted: a signature that is unparsable, tampered or untrusted,
    /// a file that can't be read, any `WinVerifyTrust` result not in [`NO_SIGNATURE_STATUSES`].
    Invalid,
    /// Trusted; the signer identity.
    Valid(String),
}

/// `TRUST_E_NOSIGNATURE`: `WinVerifyTrust`'s "no signature was present in the subject".
const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;

/// The `WinVerifyTrust` results that mean *no signature* (UPD-003: "no certificate table"),
/// for a file whose PE headers say it has no certificate table. **W1: verify on Windows** which
/// results an unsigned `dockering.exe` and installer really produce (unobserved candidates:
/// `TRUST_E_SUBJECT_FORM_UNKNOWN` 0x800B0003, `TRUST_E_PROVIDER_UNKNOWN` 0x800B0001) and what a
/// corrupted certificate table returns; widen this list only from a recorded result. Every
/// result outside it, on a file without a table, is *invalid*, which fails closed.
const NO_SIGNATURE_STATUSES: &[u32] = &[TRUST_E_NOSIGNATURE];

/// Whether a PE file has an embedded certificate table (the security data directory), judged by
/// reading its headers ourselves, because a failed `CryptQueryObject` can't tell an unsigned file
/// from a corrupted signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CertTable {
    /// The security directory is non-empty.
    Present,
    /// A well-formed PE whose security directory is empty.
    Absent,
    /// Not readable as a PE: an I/O error, a truncated or malformed header.
    Unreadable,
}

/// What a `WinVerifyTrust` status means for a file with this certificate table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Unsigned,
    Trusted,
    Invalid,
}

/// UPD-003: `Unsigned` **only** when there is no certificate table *and* the status is in
/// [`NO_SIGNATURE_STATUSES`]. A table that is present but not verified (including `NOSIGNATURE`
/// for a table `WinVerifyTrust` couldn't parse), an unreadable file and every other status are
/// `Invalid`. `Trusted` needs a table and success; the caller still has to read the signer.
fn classify(status: u32, table: CertTable) -> Verdict {
    match table {
        CertTable::Unreadable => Verdict::Invalid,
        CertTable::Present if status == 0 => Verdict::Trusted,
        CertTable::Present => Verdict::Invalid,
        CertTable::Absent if NO_SIGNATURE_STATUSES.contains(&status) => Verdict::Unsigned,
        CertTable::Absent => Verdict::Invalid,
    }
}

/// Assembles a [`Signature`] from the three independent facts, evaluating each lazily: an
/// unreadable file never reaches `WinVerifyTrust`, and the signer is read only for a trusted
/// file. Trusted but unreadable signer is invalid.
fn judge(
    table: CertTable,
    verify_trust: impl FnOnce() -> u32,
    identity: impl FnOnce() -> Option<String>,
) -> Signature {
    if table == CertTable::Unreadable {
        return Signature::Invalid;
    }
    match classify(verify_trust(), table) {
        Verdict::Unsigned => Signature::None,
        Verdict::Invalid => Signature::Invalid,
        Verdict::Trusted => identity().map_or(Signature::Invalid, Signature::Valid),
    }
}

/// Index of the certificate table in a PE's data directories (`IMAGE_DIRECTORY_ENTRY_SECURITY`).
const SECURITY_DIRECTORY: usize = 4;

/// Reads the PE headers of `reader` (any position; it seeks) and reports whether the
/// certificate table is there. Never panics and never reads more than a few hundred bytes.
fn cert_table<R: Read + Seek>(reader: &mut R) -> CertTable {
    probe_cert_table(reader).unwrap_or(CertTable::Unreadable)
}

fn probe_cert_table<R: Read + Seek>(reader: &mut R) -> io::Result<CertTable> {
    let le16 = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);

    // DOS header: "MZ", and `e_lfanew` (the PE header's offset) at 0x3C.
    let mut dos = [0u8; 64];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Ok(CertTable::Unreadable);
    }
    // "PE\0\0", the 20-byte COFF header (its `SizeOfOptionalHeader` is at +20 of the whole).
    let mut coff = [0u8; 24];
    reader.seek(SeekFrom::Start(u64::from(le32(&dos, 60))))?;
    reader.read_exact(&mut coff)?;
    if &coff[..4] != b"PE\0\0" {
        return Ok(CertTable::Unreadable);
    }
    let declared = usize::from(le16(&coff, 20));
    // The directories end by offset 112 + 8 × 16 = 240; the security entry is far before that.
    let mut optional = vec![0u8; declared.min(256)];
    reader.read_exact(&mut optional)?;
    // `NumberOfRvaAndSizes` and the first directory entry sit at fixed places per magic.
    let (count_at, directories_at) = match optional.get(..2).map(|m| le16(m, 0)) {
        Some(0x010B) => (92, 96),   // PE32
        Some(0x020B) => (108, 112), // PE32+
        _ => return Ok(CertTable::Unreadable),
    };
    if optional.len() < count_at + 4 {
        return Ok(CertTable::Unreadable);
    }
    if le32(&optional, count_at) as usize <= SECURITY_DIRECTORY {
        return Ok(CertTable::Absent);
    }
    let entry = directories_at + 8 * SECURITY_DIRECTORY;
    if optional.len() < entry + 8 {
        // It claims a security entry the header is too small to hold.
        return Ok(CertTable::Unreadable);
    }
    // For this directory only, "virtual address" is a file offset.
    let (offset, size) = (le32(&optional, entry), le32(&optional, entry + 4));
    Ok(if offset == 0 && size == 0 {
        CertTable::Absent
    } else {
        CertTable::Present
    })
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub(super) fn signature(_path: &Path) -> Signature {
        Signature::None
    }

    pub(super) fn signature_of_open(_file: &File, _path: &Path) -> Signature {
        Signature::None
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
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
        let table = match File::open(path) {
            Ok(mut file) => cert_table(&mut file),
            Err(_) => CertTable::Unreadable,
        };
        let wide = wide_path(path);
        judge(table, || verify_trust(&wide, None), || identity(&wide))
    }

    /// The same for a file that is already open (UPD-007): the certificate table and
    /// `WinVerifyTrust` read through `file`. The signer's name is read from `path`, which names
    /// the same file; the caller's `FILE_SHARE_READ` lock keeps it from changing meanwhile.
    pub(super) fn signature_of_open(file: &File, path: &Path) -> Signature {
        // `&File` reads and seeks the shared handle; both probes position it themselves.
        let table = cert_table(&mut &*file);
        let wide = wide_path(path);
        judge(
            table,
            || verify_trust(&wide, Some(file)),
            || identity(&wide),
        )
    }

    pub(super) fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// WinVerifyTrust with the generic Authenticode policy, no UI, no revocation round-trips.
    /// Returns its raw status (0 = trusted). With `Some(file)` the file's own handle is passed
    /// as `WINTRUST_FILE_INFO.hFile` and `pcwszFilePath` stays valid beside it. ⚠ W1: that
    /// `WinVerifyTrust` uses the handle instead of reopening the path is what the field is for,
    /// but it is unverified here (no Windows machine); the locked path can't change either way.
    pub(super) fn verify_trust(wide_path: &[u16], file: Option<&File>) -> u32 {
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide_path.as_ptr(),
            // A borrowed handle: WinVerifyTrust doesn't close it and `file` outlives the call.
            hFile: file.map_or(null_mut(), |f| f.as_raw_handle()),
            pgKnownSubject: null_mut(),
        };
        // SAFETY: WINTRUST_DATA is a plain C struct; all-zero is a valid starting value and
        // every field the call reads is set below.
        let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous = WINTRUST_DATA_0 { pFile: &mut info };
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` and `info` (the path buffer it points to, and the open file whose
        // handle it carries) outlive both calls; the second call releases the state the first
        // one allocated.
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
            // A LONG holding an HRESULT-style code (0x800B0100 is negative as an i32).
            status as u32
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
            decide(Some("Test Publisher"), Some("Test Publisher")),
            Ok(TrustCheck::Trusted)
        );
        assert_eq!(
            decide(Some("Test Publisher"), None),
            Err(UpdateError::UntrustedInstaller)
        );
        assert_eq!(
            decide(Some("Test Publisher"), Some("Mallory")),
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

    // ---- UPD-003: the trust decision, pure, on every OS ---------------------------------

    const SUBJECT: &str = "CN=Test Publisher | CN=Test Root";

    /// A `WinVerifyTrust` status that is neither success nor `TRUST_E_NOSIGNATURE`.
    const OTHER_STATUSES: [u32; 10] = [
        0x800B_0001, // TRUST_E_PROVIDER_UNKNOWN
        0x800B_0003, // TRUST_E_SUBJECT_FORM_UNKNOWN
        0x800B_0004, // TRUST_E_SUBJECT_NOT_TRUSTED
        0x8009_6010, // TRUST_E_BAD_DIGEST
        0x800B_0109, // CERT_E_UNTRUSTEDROOT
        0x800B_010A, // CERT_E_CHAINING
        0x8009_2003, // CRYPT_E_FILE_ERROR
        0x8009_2026, // arbitrary
        1,
        u32::MAX,
    ];

    #[test]
    fn upd_003_unsigned_exe_accepts_manifest_trust() {
        assert_eq!(decide(None, None), Ok(TrustCheck::Skipped));
        assert_eq!(decide(None, Some(SUBJECT)), Ok(TrustCheck::Skipped));
        // Through the composed check: an unsigned running exe doesn't even examine the installer.
        assert_eq!(
            check_with(Signature::None, || unreachable!("the installer isn't read")),
            Ok(TrustCheck::Skipped)
        );
        // The unsigned verdict itself needs *both* signs: no certificate table and NOSIGNATURE.
        assert_eq!(
            classify(TRUST_E_NOSIGNATURE, CertTable::Absent),
            Verdict::Unsigned
        );
        assert_eq!(
            judge(
                CertTable::Absent,
                || TRUST_E_NOSIGNATURE,
                || unreachable!("no signer to read")
            ),
            Signature::None
        );
    }

    #[test]
    fn upd_003_signed_exe_rejects_unsigned_installer() {
        assert_eq!(
            decide(Some(SUBJECT), None),
            Err(UpdateError::UntrustedInstaller)
        );
        let signed = || Signature::Valid(SUBJECT.into());
        for installer in [
            Signature::None,
            Signature::Invalid,
            Signature::Valid("CN=Mallory | CN=Mallory".into()),
        ] {
            assert_eq!(
                check_with(signed(), || installer.clone()),
                Err(UpdateError::UntrustedInstaller),
                "{installer:?}"
            );
        }
        assert_eq!(
            check_with(signed(), signed),
            Ok(TrustCheck::Trusted),
            "the same signer is trusted"
        );
    }

    #[test]
    fn upd_003_unparsable_signature_on_running_exe_fails_closed() {
        // Only "no certificate table" + NOSIGNATURE is unsigned; everything else is invalid.
        for status in OTHER_STATUSES {
            assert_eq!(
                classify(status, CertTable::Absent),
                Verdict::Invalid,
                "{status:#x}"
            );
            assert_eq!(
                classify(status, CertTable::Present),
                Verdict::Invalid,
                "{status:#x}"
            );
            assert_eq!(
                classify(status, CertTable::Unreadable),
                Verdict::Invalid,
                "{status:#x}"
            );
        }
        // A table is there but `WinVerifyTrust` found no signature in it: unparsable, not unsigned.
        assert_eq!(
            classify(TRUST_E_NOSIGNATURE, CertTable::Present),
            Verdict::Invalid
        );
        // A file that can't be read as a PE is never unsigned, whatever the status says.
        assert_eq!(
            classify(TRUST_E_NOSIGNATURE, CertTable::Unreadable),
            Verdict::Invalid
        );
        assert_eq!(classify(0, CertTable::Unreadable), Verdict::Invalid);
        // Success needs a table; success without one is a catalog signature we can't pin.
        assert_eq!(classify(0, CertTable::Present), Verdict::Trusted);
        assert_eq!(classify(0, CertTable::Absent), Verdict::Invalid);
        assert!(NO_SIGNATURE_STATUSES.contains(&TRUST_E_NOSIGNATURE));

        // `judge`: an unreadable file never reaches WinVerifyTrust; trusted but unreadable
        // signer is invalid; a trusted file yields its signer.
        assert_eq!(
            judge(
                CertTable::Unreadable,
                || unreachable!("not verified"),
                || unreachable!("no signer")
            ),
            Signature::Invalid
        );
        assert_eq!(judge(CertTable::Present, || 0, || None), Signature::Invalid);
        assert_eq!(
            judge(
                CertTable::Present,
                || TRUST_E_NOSIGNATURE,
                || unreachable!()
            ),
            Signature::Invalid
        );
        assert_eq!(
            judge(CertTable::Present, || 0, || Some(SUBJECT.into())),
            Signature::Valid(SUBJECT.into())
        );

        // And the check: an invalid running exe is an untrusted installer, whatever the
        // installer is, and the installer isn't examined.
        assert_eq!(
            check_with(Signature::Invalid, || unreachable!(
                "the installer isn't read"
            )),
            Err(UpdateError::UntrustedInstaller)
        );
    }

    // ---- the PE certificate-table probe ---------------------------------------------------

    const PE32: u16 = 0x010B;
    const PE32_PLUS: u16 = 0x020B;
    const DOS_LEN: usize = 64;
    /// Where `SizeOfOptionalHeader` sits in the bytes [`pe`] builds.
    const SIZE_OF_OPTIONAL_AT: usize = DOS_LEN + 4 + 16;

    /// The headers of a PE with `directories` data directories, the certificate table
    /// `(offset, size)` in the fifth.
    fn pe(magic: u16, directories: u32, security: (u32, u32)) -> Vec<u8> {
        let (count_at, directories_at, len) = if magic == PE32_PLUS {
            (108, 112, 240)
        } else {
            (92, 96, 224)
        };
        let mut bytes = vec![0u8; DOS_LEN];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&(DOS_LEN as u32).to_le_bytes());
        bytes.extend_from_slice(b"PE\0\0");
        let mut coff = [0u8; 20];
        coff[16..18].copy_from_slice(&(len as u16).to_le_bytes());
        bytes.extend_from_slice(&coff);
        let mut optional = vec![0u8; len];
        optional[..2].copy_from_slice(&magic.to_le_bytes());
        optional[count_at..count_at + 4].copy_from_slice(&directories.to_le_bytes());
        let entry = directories_at + 8 * SECURITY_DIRECTORY;
        optional[entry..entry + 4].copy_from_slice(&security.0.to_le_bytes());
        optional[entry + 4..entry + 8].copy_from_slice(&security.1.to_le_bytes());
        bytes.extend_from_slice(&optional);
        bytes
    }

    fn table(bytes: &[u8]) -> CertTable {
        cert_table(&mut std::io::Cursor::new(bytes))
    }

    #[test]
    fn upd_003_pe_certificate_table_present_or_absent() {
        for magic in [PE32, PE32_PLUS] {
            assert_eq!(
                table(&pe(magic, 16, (0x4_0000, 0x1A00))),
                CertTable::Present
            );
            assert_eq!(table(&pe(magic, 16, (0, 0))), CertTable::Absent);
            // Half an entry is not "no table": the verdict is left to WinVerifyTrust, which
            // then fails it.
            assert_eq!(table(&pe(magic, 16, (0, 0x40))), CertTable::Present);
            assert_eq!(table(&pe(magic, 16, (0x40, 0))), CertTable::Present);
            // A header with too few directories has no security directory at all.
            assert_eq!(table(&pe(magic, 4, (0x4_0000, 0x1A00))), CertTable::Absent);
            assert_eq!(table(&pe(magic, 0, (0x4_0000, 0x1A00))), CertTable::Absent);
            assert_eq!(table(&pe(magic, 5, (0x4_0000, 0x1A00))), CertTable::Present);
        }
    }

    #[test]
    fn upd_003_pe_certificate_table_unreadable_is_not_absent() {
        let valid = pe(PE32_PLUS, 16, (0, 0));
        // Every truncation, an I/O failure in disguise.
        for len in 0..valid.len() {
            assert_eq!(table(&valid[..len]), CertTable::Unreadable, "{len} bytes");
        }
        assert_eq!(table(&valid), CertTable::Absent);

        let mut not_mz = valid.clone();
        not_mz[0] = b'Z';
        assert_eq!(table(&not_mz), CertTable::Unreadable);
        let mut not_pe = valid.clone();
        not_pe[DOS_LEN] = b'X';
        assert_eq!(table(&not_pe), CertTable::Unreadable);
        let mut rom = valid.clone();
        rom[DOS_LEN + 24..DOS_LEN + 26].copy_from_slice(&0x0107u16.to_le_bytes());
        assert_eq!(
            table(&rom),
            CertTable::Unreadable,
            "unknown optional-header magic"
        );
        let mut far = valid.clone();
        far[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(table(&far), CertTable::Unreadable, "e_lfanew past the end");

        // Claims sixteen directories, but its optional header is too small to hold the entry.
        let mut small = pe(PE32, 16, (0x4_0000, 0x1A00));
        small[SIZE_OF_OPTIONAL_AT..SIZE_OF_OPTIONAL_AT + 2].copy_from_slice(&100u16.to_le_bytes());
        assert_eq!(table(&small), CertTable::Unreadable);
        small[SIZE_OF_OPTIONAL_AT..SIZE_OF_OPTIONAL_AT + 2].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(table(&small), CertTable::Unreadable, "no optional header");
        assert_eq!(table(b"MZ not really an executable"), CertTable::Unreadable);
        assert_eq!(table(b""), CertTable::Unreadable);
    }

    #[test]
    fn upd_003_pe_certificate_table_reads_a_file_from_any_position() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installer.exe");
        std::fs::write(&path, pe(PE32_PLUS, 16, (0x4_0000, 0x1A00))).expect("write");
        let mut file = File::open(&path).expect("open");
        file.seek(SeekFrom::Start(100)).expect("seek");
        assert_eq!(cert_table(&mut file), CertTable::Present);
        // Through a shared reference too, which is how a locked handle is read.
        let mut file = File::open(&path).expect("open");
        file.seek(SeekFrom::End(0)).expect("seek");
        assert_eq!(cert_table(&mut &file), CertTable::Present);
    }

    proptest::proptest! {
        /// Hostile bytes never panic the probe and never claim a table that isn't in the header.
        #[test]
        fn upd_003_pe_probe_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..600)) {
            let _ = table(&bytes);
        }
    }

    // ---- Windows: the real WinVerifyTrust (spike W1 probes) ---------------------------------

    /// ⚠ W1: an unsigned PE (the test binary has no certificate table) must come out as *no
    /// signature*. If WinVerifyTrust answers with another status the message shows it: extend
    /// [`NO_SIGNATURE_STATUSES`] from that recorded result, never from a guess.
    #[cfg(windows)]
    #[test]
    fn upd_003_windows_unsigned_exe_is_no_signature_w1() {
        let exe = std::env::current_exe().expect("test exe");
        let table = cert_table(&mut File::open(&exe).expect("open"));
        let wide = imp::wide_path(&exe);
        let status = imp::verify_trust(&wide, None);
        assert_eq!(table, CertTable::Absent, "the test binary is unsigned");
        assert_eq!(
            imp::signature(&exe),
            Signature::None,
            "W1: WinVerifyTrust status {status:#010x} for an unsigned PE"
        );
    }

    /// An unparsable file can't be a running exe that skips the pin: deterministic, because the
    /// header probe fails before `WinVerifyTrust` is asked.
    #[cfg(windows)]
    #[test]
    fn upd_003_windows_garbage_running_exe_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let garbage = dir.path().join("running.exe");
        std::fs::write(&garbage, b"MZ not really an executable").expect("write");
        let installer = dir.path().join("installer.exe");
        std::fs::write(&installer, pe(PE32_PLUS, 16, (0, 0))).expect("write");
        assert_eq!(imp::signature(&garbage), Signature::Invalid);
        assert_eq!(
            check_installer(&installer, &garbage),
            Err(UpdateError::UntrustedInstaller)
        );
        let missing = dir.path().join("gone.exe");
        assert_eq!(
            imp::signature(&missing),
            Signature::Invalid,
            "unreadable is invalid"
        );
    }

    /// ⚠ W1: reading through an open handle gives the same verdict as the path, for an unsigned
    /// and (where one exists on this machine) an embedded-signed file.
    #[cfg(windows)]
    #[test]
    fn upd_003_windows_open_handle_verdict_matches_path_w1() {
        let exe = std::env::current_exe().expect("test exe");
        let mut paths = vec![exe.clone()];
        paths.extend(
            [
                r"C:\Program Files\PowerShell\7\pwsh.exe",
                r"C:\Program Files\Git\cmd\git.exe",
            ]
            .iter()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file()),
        );
        for path in paths {
            let file = File::open(&path).expect("open");
            assert_eq!(
                imp::signature_of_open(&file, &path),
                imp::signature(&path),
                "{}",
                path.display()
            );
        }
        // An unsigned running exe skips the pin whatever the (open) installer is.
        let file = File::open(&exe).expect("open");
        assert_eq!(
            check_installer_open(&file, &exe, &exe),
            Ok(TrustCheck::Skipped)
        );
    }
}
