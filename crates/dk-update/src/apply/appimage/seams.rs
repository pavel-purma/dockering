//! The small seams of the AppImage apply (UPD-014): every real side effect that a unit test needs
//! to fail or observe goes through one of these traits, with a production implementation next to
//! it. The tests inject disk-full errors, a payload that changes while it is staged, a failing
//! spawn, a failing directory flush, and `ETXTBSY`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use crate::install_kind::{AppImageProbe, FsProbe};

/// `ETXTBSY`: "text file busy", the error `execve` gives while any process still has the file
/// open for writing, including the copy of a descriptor a concurrently forking thread holds
/// between `fork` and `exec`.
pub(super) const ETXTBSY: i32 = 26;

/// Why a child is being started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    /// `--version` of the staged or installed file (UPD-014 step 3).
    SelfTest,
    /// The new process with the relaunch flag (UPD-014 step 5).
    Relaunch,
}

/// Starts a configured child.
pub(super) trait Spawner {
    fn spawn(&self, command: &mut Command, purpose: Purpose) -> io::Result<Child>;
}

/// The production [`Spawner`]: `Command::spawn`.
pub(super) struct RealSpawner;

impl Spawner for RealSpawner {
    fn spawn(&self, command: &mut Command, _purpose: Purpose) -> io::Result<Child> {
        command.spawn()
    }
}

/// Creates the staging file. Hashing happens in the caller, on the bytes it hands to the
/// [`StageFile`], so a writer that fails (disk full) is a seam of its own.
pub(super) trait StageSink {
    /// Creates `path` exclusively (it must not exist) with exactly the permission bits `mode`.
    fn create(&self, path: &Path, mode: u32) -> io::Result<Box<dyn StageFile>>;
}

/// An open staging file.
pub(super) trait StageFile {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// `fsync`: data and metadata reach the disk.
    fn sync_all(&mut self) -> io::Result<()>;
}

/// The production [`StageSink`]: `create_new` with the mode, then an explicit permission set so
/// the umask can't narrow the target's bits.
pub(super) struct FsStage;

impl StageSink for FsStage {
    fn create(&self, path: &Path, mode: u32) -> io::Result<Box<dyn StageFile>> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)?;
        // `mode()` is masked by the umask; the staged file must carry the target's bits.
        if let Err(error) = file.set_permissions(fs::Permissions::from_mode(mode)) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(Box::new(DiskFile(file)))
    }
}

/// A [`StageFile`] on a real file.
struct DiskFile(File);

impl StageFile for DiskFile {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
}

/// Makes a `rename` durable.
pub(super) trait Fsync {
    fn sync_dir(&self, dir: &Path) -> io::Result<()>;
}

/// The production [`Fsync`]: open the directory and `fsync` it.
pub(super) struct RealFsync;

impl Fsync for RealFsync {
    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        File::open(dir)?.sync_all()
    }
}

/// All seams of one apply.
pub(super) struct Seams<'a> {
    /// The AppImage eligibility questions (UPD-013), re-asked in step 1.
    pub probe: &'a dyn AppImageProbe,
    pub sink: &'a dyn StageSink,
    pub spawner: &'a dyn Spawner,
    pub fsync: &'a dyn Fsync,
    /// The pause between `ETXTBSY` retries (100 ms in production).
    pub etxtbsy_delay: Duration,
}

/// The real thing.
pub(super) const PRODUCTION: Seams<'static> = Seams {
    probe: &FsProbe,
    sink: &FsStage,
    spawner: &RealSpawner,
    fsync: &RealFsync,
    etxtbsy_delay: Duration::from_millis(100),
};
