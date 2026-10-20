//! Steps 1 and 2 of the AppImage apply (UPD-014): verify the payload, stage a copy next to the
//! target.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::AppImageJob;
use super::seams::StageSink;
use crate::UpdateError;
use crate::apply::sweep::STAGING_PREFIX;

const CHUNK: usize = 64 * 1024;

/// How often a taken staging name is skipped before giving up.
const NAME_ATTEMPTS: u32 = 100;

/// The `<n>` of `.dockering-update-<n>`; a name that already exists (a stale file, another
/// process) is skipped because the file is created exclusively.
static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

/// The staging file, removed when this is dropped unless [`StagedFile::commit`] was called
/// (after the `rename`): every error path before the swap leaves nothing behind.
#[derive(Debug)]
pub(super) struct StagedFile {
    path: PathBuf,
    armed: bool,
}

impl StagedFile {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// The file has been renamed away; there is nothing to remove any more.
    pub(super) fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = fs::remove_file(&self.path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(%error, "couldn't remove the staged update file");
        }
    }
}

/// The permission bits of the staged file: the target's `rwx` bits (no setuid, setgid or sticky
/// bit), plus execute wherever read is allowed, so `0o700` stays `0o700`, `0o755` stays `0o755`
/// and `0o644` becomes `0o755`. The owner can always execute.
pub(super) fn staged_mode(target_mode: u32) -> u32 {
    let mode = target_mode & 0o777;
    mode | 0o100 | ((mode & 0o444) >> 2)
}

/// Step 1: opens the payload and checks its size and SHA-256 against the manifest values. The
/// returned handle is the one [`stage`] copies from, so a replaced file can't slip in between.
/// A payload that cannot be read is [`UpdateError::Io`]; a mismatch is [`UpdateError::Integrity`].
pub(super) fn open_verified_payload(job: &AppImageJob<'_>) -> Result<File, UpdateError> {
    let mut file = File::open(job.payload)?;
    let (size, hex) = hash(&mut file)?;
    if size != job.expected_size || !same_digest(&hex, job.expected_sha256) {
        return Err(UpdateError::Integrity);
    }
    Ok(file)
}

fn hash(reader: &mut impl Read) -> io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut size = 0u64;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                size += n as u64;
                hasher.update(&buffer[..n]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok((size, hex::encode(hasher.finalize())))
}

fn same_digest(actual_hex: &str, expected_hex: &str) -> bool {
    actual_hex.eq_ignore_ascii_case(expected_hex.trim())
}

/// Step 2: copies `payload` to a new `.dockering-update-<n>` file in `dir` with permission bits
/// `mode`, hashing the bytes as they are written, then `fsync`s and closes it.
///
/// A write, flush or create failure is [`UpdateError::Stage`]. A payload that no longer has the
/// expected size or digest (it changed after step 1) is [`UpdateError::Integrity`]. On every
/// error the staging file is removed.
pub(super) fn stage(
    payload: &mut File,
    job: &AppImageJob<'_>,
    dir: &Path,
    mode: u32,
    sink: &dyn StageSink,
) -> Result<StagedFile, UpdateError> {
    let mut attempts = 0;
    let (staged, mut file) = loop {
        let n = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{STAGING_PREFIX}{n}"));
        match sink.create(&path, mode) {
            Ok(file) => {
                break (StagedFile { path, armed: true }, file);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists && attempts < NAME_ATTEMPTS => {
                attempts += 1;
            }
            Err(error) => {
                return Err(UpdateError::Stage(format!(
                    "couldn't create a file next to the app: {error}"
                )));
            }
        }
    };

    payload.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        let n = match payload.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        copied += n as u64;
        if copied > job.expected_size {
            return Err(UpdateError::Integrity);
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n])
            .map_err(|error| UpdateError::Stage(format!("couldn't write the update: {error}")))?;
    }
    if copied != job.expected_size
        || !same_digest(&hex::encode(hasher.finalize()), job.expected_sha256)
    {
        return Err(UpdateError::Integrity);
    }
    file.sync_all()
        .map_err(|error| UpdateError::Stage(format!("couldn't flush the update: {error}")))?;
    // Closed before anything runs it, or `execve` fails with `ETXTBSY`.
    drop(file);
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upd_014_staged_mode_is_the_targets_plus_execute() {
        assert_eq!(staged_mode(0o755), 0o755);
        assert_eq!(staged_mode(0o700), 0o700);
        assert_eq!(staged_mode(0o775), 0o775);
        assert_eq!(staged_mode(0o644), 0o755);
        assert_eq!(staged_mode(0o600), 0o700);
        assert_eq!(staged_mode(0o640), 0o750);
        // Never more than the target allowed to be read, and no special bits.
        assert_eq!(staged_mode(0o4755), 0o755);
        assert_eq!(staged_mode(0o2700), 0o700);
        assert_eq!(staged_mode(0o1777), 0o777);
        assert_eq!(staged_mode(0o000), 0o100);
    }
}
