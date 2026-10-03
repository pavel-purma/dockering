//! Manifest signatures and downloaded-file integrity (UPD-003).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::UpdateError;

/// Checks `json` against a minisign `signature` (the `.minisig` file text). Any key in `keys`
/// may match. A key is either the bare base64 line or the full two-line `.pub` file. No keys,
/// a malformed signature, or no matching key all mean [`UpdateError::Signature`].
pub fn verify_manifest(json: &[u8], signature: &str, keys: &[&str]) -> Result<(), UpdateError> {
    let signature = Signature::decode(signature).map_err(|_| UpdateError::Signature)?;
    let verified = keys
        .iter()
        .filter_map(|k| PublicKey::from_base64(key_line(k)).ok())
        .any(|pk| pk.verify(json, &signature, false).is_ok());
    if verified {
        Ok(())
    } else {
        Err(UpdateError::Signature)
    }
}

/// The base64 line of a public key given either bare or as a whole `.pub` file.
fn key_line(key: &str) -> &str {
    key.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .unwrap_or("")
}

/// Checks that the file at `path` has exactly `size` bytes and the SHA-256 `sha256_hex`.
pub async fn verify_file(path: &Path, sha256_hex: &str, size: u64) -> Result<(), UpdateError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > size {
            return Err(UpdateError::Integrity);
        }
        hasher.update(&buf[..n]);
    }
    if total != size || !digest_matches(hasher, sha256_hex) {
        return Err(UpdateError::Integrity);
    }
    Ok(())
}

fn digest_matches(hasher: Sha256, expected_hex: &str) -> bool {
    hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected_hex.trim())
}

/// Progress callbacks fire at most every 250 ms or 1 %, and always once at the end (UPD-008).
struct Throttle {
    last: Option<Instant>,
    last_done: u64,
}

impl Throttle {
    fn should_report(&mut self, done: u64, total: u64) -> bool {
        let step = (total / 100).max(1);
        let due = match self.last {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_millis(250) || done >= self.last_done + step,
        };
        if due {
            self.last = Some(Instant::now());
            self.last_done = done;
        }
        due
    }
}

/// Writes `stream` to `dir/<file_name>.part` while hashing it, then renames it to
/// `dir/<file_name>` if it is exactly `size` bytes with SHA-256 `sha256_hex`. On any failure the
/// partial file is removed. `progress(done, total)` is throttled.
pub async fn write_stream_verified<S, B, E>(
    mut stream: S,
    dir: &Path,
    file_name: &str,
    size: u64,
    sha256_hex: &str,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, UpdateError>
where
    S: Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    if file_name.is_empty() || file_name.contains(['/', '\\']) || file_name.starts_with('.') {
        return Err(UpdateError::Io(format!("invalid file name `{file_name}`")));
    }
    tokio::fs::create_dir_all(dir).await?;
    let part = dir.join(format!("{file_name}.part"));
    let target = dir.join(file_name);

    let result = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut throttle = Throttle {
            last: None,
            last_done: 0,
        };
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| UpdateError::Network(e.to_string()))?;
            let bytes = chunk.as_ref();
            done += bytes.len() as u64;
            if done > size {
                return Err(UpdateError::Integrity);
            }
            hasher.update(bytes);
            file.write_all(bytes).await?;
            if throttle.should_report(done, size) {
                progress(done, size);
            }
        }
        file.flush().await?;
        drop(file);
        progress(done, size);
        if done != size || !digest_matches(hasher, sha256_hex) {
            return Err(UpdateError::Integrity);
        }
        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            tokio::fs::remove_file(&target).await?;
        }
        tokio::fs::rename(&part, &target).await?;
        Ok(target.clone())
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn keypair() -> minisign::KeyPair {
        minisign::KeyPair::generate_unencrypted_keypair().expect("keypair")
    }

    fn sign(kp: &minisign::KeyPair, data: &[u8]) -> String {
        minisign::sign(None, &kp.sk, Cursor::new(data), None, None)
            .expect("sign")
            .into_string()
    }

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    #[test]
    fn upd_003_rejects_bad_signature() {
        let kp = keypair();
        let key = kp.pk.to_base64();
        let sig = sign(&kp, b"{\"schema\":1}");
        assert_eq!(verify_manifest(b"{\"schema\":1}", &sig, &[&key]), Ok(()));
        assert_eq!(
            verify_manifest(b"{\"schema\":2}", &sig, &[&key]),
            Err(UpdateError::Signature)
        );
        assert_eq!(
            verify_manifest(b"{\"schema\":1}", "not a signature", &[&key]),
            Err(UpdateError::Signature)
        );
    }

    #[test]
    fn upd_003_accepts_next_key() {
        let (a, b) = (keypair(), keypair());
        let sig = sign(&b, b"manifest");
        let (ka, kb) = (a.pk.to_base64(), b.pk.to_base64());
        assert_eq!(verify_manifest(b"manifest", &sig, &[&ka, &kb]), Ok(()));
        assert_eq!(
            verify_manifest(b"manifest", &sig, &[&ka]),
            Err(UpdateError::Signature)
        );
        // The full `.pub` file form works too.
        let pub_file = b.pk.to_box().expect("box").into_string();
        assert_eq!(verify_manifest(b"manifest", &sig, &[&pub_file]), Ok(()));
    }

    #[test]
    fn upd_003_no_keys_fails_closed() {
        let kp = keypair();
        let sig = sign(&kp, b"manifest");
        assert_eq!(
            verify_manifest(b"manifest", &sig, &[]),
            Err(UpdateError::Signature)
        );
        assert_eq!(
            verify_manifest(b"manifest", &sig, crate::keys::PUBLIC_KEYS),
            Err(UpdateError::Signature)
        );
    }

    #[tokio::test]
    async fn upd_003_rejects_hash_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("setup.exe");
        std::fs::write(&path, b"installer").expect("write");
        assert_eq!(verify_file(&path, &sha(b"installer"), 9).await, Ok(()));
        assert_eq!(
            verify_file(&path, &sha(b"other"), 9).await,
            Err(UpdateError::Integrity)
        );
    }

    #[tokio::test]
    async fn upd_003_rejects_size_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("setup.exe");
        std::fs::write(&path, b"installer").expect("write");
        assert_eq!(
            verify_file(&path, &sha(b"installer"), 8).await,
            Err(UpdateError::Integrity)
        );
        assert_eq!(
            verify_file(&path, &sha(b"installer"), 10).await,
            Err(UpdateError::Integrity)
        );
    }

    #[tokio::test]
    async fn upd_003_stream_hash_mismatch_leaves_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let chunks = || {
            futures::stream::iter(vec![
                Ok::<_, std::io::Error>(b"insta".to_vec()),
                Ok(b"ller".to_vec()),
            ])
        };

        let mut calls = Vec::new();
        let ok = write_stream_verified(
            chunks(),
            dir.path(),
            "setup.exe",
            9,
            &sha(b"installer"),
            |d, t| calls.push((d, t)),
        )
        .await
        .expect("valid stream");
        assert_eq!(std::fs::read(&ok).expect("read"), b"installer");
        assert_eq!(calls.last(), Some(&(9, 9)));

        let bad = write_stream_verified(
            chunks(),
            dir.path(),
            "bad.exe",
            9,
            &sha(b"something else"),
            |_, _| {},
        )
        .await;
        assert_eq!(bad, Err(UpdateError::Integrity));
        assert!(!dir.path().join("bad.exe").exists());
        assert!(!dir.path().join("bad.exe.part").exists());

        let too_long = write_stream_verified(
            chunks(),
            dir.path(),
            "long.exe",
            4,
            &sha(b"installer"),
            |_, _| {},
        )
        .await;
        assert_eq!(too_long, Err(UpdateError::Integrity));
        assert!(!dir.path().join("long.exe.part").exists());
    }
}
