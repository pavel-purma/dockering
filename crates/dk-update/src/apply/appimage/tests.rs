//! UPD-003, UPD-014 and UPD-012 tests of the AppImage apply, on temp directories with shell-script
//! "AppImages": executable `/bin/sh` scripts that answer `--version` with `dockering <version>`
//! and record what they see for `--update-relaunch=1`. Linux only (real `exec`, `/proc`).
//!
//! # How `ETXTBSY` is tested
//!
//! Two ways. (1) Injected: [`TestSpawner::etxtbsy_failures`] makes the first *n* self-test spawns
//! fail with raw OS error 26, which pins the retry count (five retries, so six attempts) without
//! any timing. (2) Real: the kernel refuses `execve` with `ETXTBSY` while any process has the
//! file open for writing, so [`upd_014_etxtbsy_retried_for_a_file_still_open_for_writing`] keeps a
//! writer open on the script and the test spawner releases it when the real spawn reports
//! `ETXTBSY`; the second attempt then runs. No sleeping decides the outcome.
//!
//! The child's environment is the `env` of the job (see `ChildEnv`), so the scripts find their
//! output directory in `$DK_OUT` and the scrub is observable end to end.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::child::{ChildEnv, env_of};
use super::flow::{self, Limits};
use super::relaunch::{close_relaunch_pipes_for_tests, kept_pipes_for_tests};
use super::seams::{
    ETXTBSY, FsStage, Fsync, PRODUCTION, Purpose, RealFsync, RealSpawner, Seams, Spawner,
    StageFile, StageSink,
};
use super::selftest::{self, version_output_matches};
use super::{AppImageJob, AppImageOutcome, RELAUNCH_FLAG, apply_appimage, restart_installed};
use crate::UpdateError;
use crate::install_kind::{AppImageProbe, FsProbe};

const OLD: &str = "1.0.0";
const NEW: &str = "2.0.0";

/// The names the scripts report. A value prints as `NAME=[value]`, an unset name as
/// `NAME=<unset>`; only shell built-ins run, so an emptied `PATH` can't break the scripts.
const DUMP: &str = r#"dump() {
  for v in APPDIR APPIMAGE ARGV0 OWD PATH LD_LIBRARY_PATH XDG_DATA_DIRS PYTHONPATH QT_PLUGIN_PATH GST_PLUGIN_SYSTEM_PATH_1_0 APPIMAGE_EXTRACT_AND_RUN KEEP_ME; do
    if eval "[ -n \"\${$v+x}\" ]"; then eval "printf '%s=[%s]\\n' $v \"\$$v\""; else printf '%s=<unset>\n' "$v"; fi
  done
}"#;

/// What `dump` prints in the fixture environment after the scrub.
const SCRUBBED_DUMP: &str = "APPDIR=<unset>
APPIMAGE=<unset>
ARGV0=<unset>
OWD=<unset>
PATH=[/usr/bin:/bin]
LD_LIBRARY_PATH=<unset>
XDG_DATA_DIRS=[/usr/share]
PYTHONPATH=[/tmp/.mount_testy/lib]
QT_PLUGIN_PATH=<unset>
GST_PLUGIN_SYSTEM_PATH_1_0=<unset>
APPIMAGE_EXTRACT_AND_RUN=[1]
KEEP_ME=[kept]
";

/// Records how it was started (argv, stdin kind, process group, environment), then waits for
/// end-of-file on stdin like the real new process does.
const RECORD_RELAUNCH: &str = r#"{ echo "argc=$#"; echo "arg1=$1"
    if [ -p /dev/stdin ]; then echo stdin=pipe; else echo stdin=other; fi
    read -r line < /proc/self/stat; rest=${line##*) }; set -- $rest
    if [ "$3" = "$$" ]; then echo own_group=yes; else echo own_group=no; fi; } > "$DK_OUT/relaunch.rec"
  dump > "$DK_OUT/relaunch.env"
  : > "$DK_OUT/relaunch.done"
  read ignored
  exit 0"#;

struct Script {
    version: String,
    on_version: String,
    on_relaunch: String,
    padding: usize,
}

impl Script {
    fn new(version: &str) -> Self {
        Self {
            version: version.to_owned(),
            on_version: String::new(),
            on_relaunch: "read ignored\n  exit 0".to_owned(),
            padding: 0,
        }
    }

    /// The new version, recording its self-test environment and its relaunch.
    fn recording_new() -> Self {
        Self::new(NEW)
            .on_version(r#"dump > "$DK_OUT/selftest.env""#)
            .on_relaunch(RECORD_RELAUNCH)
    }

    fn on_version(mut self, body: &str) -> Self {
        self.on_version = body.to_owned();
        self
    }

    fn on_relaunch(mut self, body: &str) -> Self {
        self.on_relaunch = body.to_owned();
        self
    }

    /// Bytes of comment after the final `exit`, so the file spans several copy chunks.
    fn padded(mut self, bytes: usize) -> Self {
        self.padding = bytes;
        self
    }

    fn render(&self) -> Vec<u8> {
        let mut text = format!(
            "#!/bin/sh\n{DUMP}\ncase \"$1\" in\n--version)\n  {}\n  echo \"dockering {}\"\n  exit 0 ;;\n--update-relaunch=1)\n  {}\n  ;;\nesac\nexit 0\n",
            self.on_version, self.version, self.on_relaunch
        );
        while text.len() < self.padding {
            text.push_str("# padding padding padding padding padding padding padding\n");
        }
        text.into_bytes()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    target: PathBuf,
    payload: PathBuf,
    old: Vec<u8>,
    new: Vec<u8>,
    sha: String,
    env: Vec<(OsString, OsString)>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn make_executable(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

impl Fixture {
    fn new(old: &Script, new: &Script) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let app = root.path().join("app");
        let updates = root.path().join("updates");
        let out = root.path().join("out");
        for dir in [&app, &updates, &out] {
            fs::create_dir(dir).expect("mkdir");
        }
        let target = app.join("Dockering.AppImage");
        let payload = updates.join("Dockering-x86_64.AppImage");
        let (old, new) = (old.render(), new.render());
        fs::write(&target, &old).expect("write target");
        make_executable(&target, 0o755);
        fs::write(&payload, &new).expect("write payload");
        // Executable, so a test can run or copy it directly.
        make_executable(&payload, 0o755);
        let env = [
            ("PATH", "/tmp/.mount_test/usr/bin:/usr/bin:/bin"),
            ("APPDIR", "/tmp/.mount_test"),
            ("APPIMAGE", target.to_str().expect("utf-8")),
            ("ARGV0", "dockering"),
            ("OWD", "/home/u"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_test/usr/lib"),
            ("XDG_DATA_DIRS", "/tmp/.mount_test/usr/share:/usr/share"),
            // Not under the AppDir: `/tmp/.mount_test` is a string prefix of it, not a parent.
            ("PYTHONPATH", "/tmp/.mount_testy/lib"),
            (
                "QT_PLUGIN_PATH",
                "/tmp/.mount_test/usr/plugins:/tmp/.mount_test/usr/lib/qt",
            ),
            (
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                "/tmp/.mount_test/usr/lib/gstreamer",
            ),
            ("APPIMAGE_EXTRACT_AND_RUN", "1"),
            ("KEEP_ME", "kept"),
            ("DK_OUT", out.to_str().expect("utf-8")),
        ]
        .iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
        Self {
            sha: sha256_hex(&new),
            root,
            target,
            payload,
            old,
            new,
            env,
        }
    }

    /// A fixture whose new version records its relaunch.
    fn recording() -> Self {
        Self::new(&Script::new(OLD), &Script::recording_new())
    }

    fn dir(&self) -> PathBuf {
        self.root.path().join("app")
    }

    fn out(&self, name: &str) -> PathBuf {
        self.root.path().join("out").join(name)
    }

    fn job(&self) -> AppImageJob<'_> {
        let mut job = AppImageJob::new(
            &self.payload,
            &self.target,
            NEW,
            &self.sha,
            self.new.len() as u64,
            &self.env,
        );
        job.self_test_timeout = Duration::from_secs(10);
        job.relaunch_grace = Duration::from_millis(150);
        job
    }

    fn target_bytes(&self) -> Vec<u8> {
        fs::read(&self.target).expect("read target")
    }

    /// Every entry of the target's directory.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir())
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The old file, alone in its directory: nothing was replaced, nothing was left behind.
    fn assert_untouched(&self) {
        assert_eq!(self.target_bytes(), self.old, "the target changed");
        assert_eq!(
            self.listing(),
            ["Dockering.AppImage"],
            "staging left behind"
        );
        assert_eq!(
            fs::metadata(&self.target).expect("stat").mode() & 0o7777,
            0o755
        );
    }

    fn assert_replaced(&self) {
        assert_eq!(
            self.target_bytes(),
            self.new,
            "the target isn't the new file"
        );
        assert_eq!(
            self.listing(),
            ["Dockering.AppImage"],
            "staging left behind"
        );
    }
}

/// Waits (real time, bounded) for a file a script creates.
fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

// ----- seams -------------------------------------------------------------------------------

struct TestProbe {
    writable: bool,
}

impl AppImageProbe for TestProbe {
    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok()
    }

    /// Scripts have no ELF header: any regular file is an AppImage here.
    fn is_appimage_file(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.is_file())
    }

    fn dir_writable(&self, dir: &Path) -> bool {
        self.writable && FsProbe.dir_writable(dir)
    }
}

enum SinkMode {
    Real,
    /// `ENOSPC` once more than this many bytes were written.
    DiskFullAfter(usize),
    /// Creating the file fails.
    CreateDenied,
    /// `fsync` of the staged file fails.
    SyncFails,
    /// The payload is overwritten (in place, then these bytes) right before the staging file is
    /// created, i.e. after the step 1 check.
    ChangePayload(PathBuf, Vec<u8>),
}

struct TestSink(SinkMode);

struct Failing {
    inner: Box<dyn StageFile>,
    left: usize,
    sync_fails: bool,
}

impl StageFile for Failing {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > self.left {
            // ENOSPC
            return Err(io::Error::from_raw_os_error(28));
        }
        self.left -= bytes.len();
        self.inner.write_all(bytes)
    }

    fn sync_all(&mut self) -> io::Result<()> {
        if self.sync_fails {
            return Err(io::Error::other("injected fsync failure"));
        }
        self.inner.sync_all()
    }
}

impl StageSink for TestSink {
    fn create(&self, path: &Path, mode: u32) -> io::Result<Box<dyn StageFile>> {
        match &self.0 {
            SinkMode::Real => FsStage.create(path, mode),
            SinkMode::DiskFullAfter(left) => Ok(Box::new(Failing {
                inner: FsStage.create(path, mode)?,
                left: *left,
                sync_fails: false,
            })),
            SinkMode::SyncFails => Ok(Box::new(Failing {
                inner: FsStage.create(path, mode)?,
                left: usize::MAX,
                sync_fails: true,
            })),
            SinkMode::CreateDenied => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            SinkMode::ChangePayload(payload, bytes) => {
                let mut file = OpenOptions::new().write(true).open(payload)?;
                file.write_all(bytes)?;
                FsStage.create(path, mode)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    purpose: Purpose,
    program: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, Option<OsString>)>,
}

#[derive(Default)]
struct TestSpawner {
    /// The relaunch spawn fails.
    fail_relaunch: bool,
    /// The next self-test spawns fail with `ETXTBSY`, this many.
    etxtbsy_failures: AtomicU32,
    /// A writer kept open until the real spawn first reports `ETXTBSY`.
    hold: Mutex<Option<File>>,
    etxtbsy_seen: AtomicU32,
    calls: Mutex<Vec<Call>>,
}

impl TestSpawner {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }

    /// The last spawn made for `purpose`.
    fn call(&self, purpose: Purpose) -> Call {
        self.calls()
            .into_iter()
            .rfind(|call| call.purpose == purpose)
            .expect("a spawn for that purpose")
    }

    /// Every spawn in order. Besides the injected failures, a real `ETXTBSY` can add an attempt
    /// no test asked for: another test thread forking while the staged file's writer was still
    /// open (exactly why the retry exists).
    fn attempts(&self) -> Vec<Purpose> {
        self.calls().iter().map(|c| c.purpose).collect()
    }

    /// The spawns with consecutive repeats (retries) collapsed: self-test, then relaunch.
    fn purposes(&self) -> Vec<Purpose> {
        let mut purposes = self.attempts();
        purposes.dedup();
        purposes
    }
}

impl Spawner for TestSpawner {
    fn spawn(&self, command: &mut Command, purpose: Purpose) -> io::Result<Child> {
        self.calls.lock().expect("calls").push(Call {
            purpose,
            program: command.get_program().into(),
            args: command.get_args().map(Into::into).collect(),
            envs: command
                .get_envs()
                .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
                .collect(),
        });
        match purpose {
            Purpose::SelfTest => {
                if self
                    .etxtbsy_failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
                {
                    return Err(io::Error::from_raw_os_error(ETXTBSY));
                }
                let result = RealSpawner.spawn(command, purpose);
                if let Err(error) = &result
                    && error.raw_os_error() == Some(ETXTBSY)
                {
                    self.etxtbsy_seen.fetch_add(1, Ordering::SeqCst);
                    drop(self.hold.lock().expect("hold").take());
                }
                result
            }
            Purpose::Relaunch if self.fail_relaunch => {
                Err(io::Error::other("no display available"))
            }
            Purpose::Relaunch => RealSpawner.spawn(command, purpose),
        }
    }
}

#[derive(Default)]
struct TestFsync {
    fail: bool,
    calls: AtomicU32,
}

impl Fsync for TestFsync {
    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(io::Error::other("injected directory fsync failure"));
        }
        RealFsync.sync_dir(dir)
    }
}

struct Rig {
    probe: TestProbe,
    sink: TestSink,
    spawner: TestSpawner,
    fsync: TestFsync,
}

impl Rig {
    fn new() -> Self {
        Self {
            probe: TestProbe { writable: true },
            sink: TestSink(SinkMode::Real),
            spawner: TestSpawner::default(),
            fsync: TestFsync::default(),
        }
    }

    /// Nothing is really started after the swap (the relaunch spawn fails), for tests about the
    /// files.
    fn without_relaunch() -> Self {
        let mut rig = Self::new();
        rig.spawner.fail_relaunch = true;
        rig
    }

    fn seams(&self) -> Seams<'_> {
        Seams {
            probe: &self.probe,
            sink: &self.sink,
            spawner: &self.spawner,
            fsync: &self.fsync,
            etxtbsy_delay: Duration::from_millis(5),
        }
    }

    fn apply(&self, fx: &Fixture) -> Result<AppImageOutcome, UpdateError> {
        flow::apply(&fx.job(), &self.seams())
    }
}

fn limits() -> Limits {
    Limits {
        self_test_timeout: Duration::from_secs(10),
        relaunch_grace: Duration::from_millis(150),
    }
}

fn is_restart_manually(outcome: &Result<AppImageOutcome, UpdateError>) -> Option<&str> {
    match outcome {
        Ok(AppImageOutcome::InstalledRestartManually(reason)) => Some(reason),
        _ => None,
    }
}

// ----- UPD-014: the happy path and the swap ------------------------------------------------

#[test]
fn upd_014_swap_replaces_target_atomically() {
    let new = Script::new(NEW)
        .on_relaunch(RECORD_RELAUNCH)
        .padded(300 * 1024);
    let fx = Fixture::new(&Script::new(OLD).padded(250 * 1024), &new);
    let rig = Rig::new();
    let old_inode = fs::metadata(&fx.target).expect("stat").ino();
    let stop = AtomicBool::new(false);

    let outcome = thread::scope(|scope| {
        // A reader that never sees a missing, empty or half-written target while the swap runs.
        let reader = scope.spawn(|| {
            let mut reads = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let bytes = fs::read(&fx.target).expect("the target path always resolves");
                assert!(
                    bytes == fx.old || bytes == fx.new,
                    "a torn read of {} bytes",
                    bytes.len()
                );
                reads += 1;
            }
            reads
        });
        let outcome = rig.apply(&fx);
        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader");
        outcome
    });

    assert_eq!(outcome, Ok(AppImageOutcome::Restarting));
    fx.assert_replaced();
    // A new file took the old one's place (a `rename`), it wasn't rewritten in place.
    assert_ne!(fs::metadata(&fx.target).expect("stat").ino(), old_inode);
    assert_eq!(rig.fsync.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        rig.spawner.purposes(),
        [Purpose::SelfTest, Purpose::Relaunch]
    );
    wait_for(&fx.out("relaunch.done"));
}

#[test]
fn upd_014_the_relaunch_is_one_flag_a_pipe_and_its_own_process_group() {
    let fx = Fixture::recording();
    let rig = Rig::new();
    assert_eq!(rig.apply(&fx), Ok(AppImageOutcome::Restarting));
    wait_for(&fx.out("relaunch.done"));
    let record = fs::read_to_string(fx.out("relaunch.rec")).expect("record");
    assert_eq!(
        record,
        "argc=1\narg1=--update-relaunch=1\nstdin=pipe\nown_group=yes\n"
    );
    let calls = rig.spawner.calls();
    let (self_test, relaunch) = (
        rig.spawner.call(Purpose::SelfTest),
        rig.spawner.call(Purpose::Relaunch),
    );
    assert_eq!(self_test.args, [OsString::from("--version")]);
    assert_eq!(relaunch.args, [OsString::from(RELAUNCH_FLAG)]);
    assert!(calls.len() >= 2);
    // The staged file runs the self-test, the real target is started.
    assert_eq!(relaunch.program, fx.target);
    assert_ne!(self_test.program, fx.target);
    assert_eq!(self_test.program.parent(), Some(fx.dir().as_path()));
    assert!(
        self_test
            .program
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".dockering-update-"))
    );
}

#[test]
fn upd_014_mode_preserved() {
    // (target mode, staged mode): the target's bits plus execute, nothing special; 0o775 shows
    // the explicit chmod (a umask of 022 would have narrowed it).
    for (target_mode, expected) in [
        (0o755, 0o755),
        (0o700, 0o700),
        (0o775, 0o775),
        (0o644, 0o755),
        (0o600, 0o700),
        (0o4755, 0o755),
    ] {
        let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
        make_executable(&fx.target, target_mode);
        let rig = Rig::without_relaunch();
        let outcome = rig.apply(&fx);
        assert!(
            is_restart_manually(&outcome).is_some(),
            "{target_mode:o}: {outcome:?}"
        );
        let mode = fs::metadata(&fx.target).expect("stat").mode() & 0o7777;
        assert_eq!(mode, expected, "target {target_mode:o}");
        fx.assert_replaced();
    }
}

// ----- UPD-014: failures before the rename -------------------------------------------------

#[test]
fn upd_014_failed_self_test_keeps_old_and_degrades_offer() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW).on_version("exit 1"));
    let rig = Rig::new();
    let result = rig.apply(&fx);
    // `SelfTest` is what the hub degrades to a *Download* offer for this version.
    assert!(
        matches!(&result, Err(UpdateError::SelfTest(m)) if m.contains("exit status: 1")),
        "{result:?}"
    );
    fx.assert_untouched();
    assert_eq!(rig.spawner.purposes(), [Purpose::SelfTest], "never started");
    assert_eq!(rig.fsync.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn upd_014_self_test_crash_is_rejected() {
    let fx = Fixture::new(
        &Script::new(OLD),
        &Script::new(NEW).on_version("kill -9 $$"),
    );
    let result = Rig::new().apply(&fx);
    assert!(
        matches!(&result, Err(UpdateError::SelfTest(m)) if m.contains("signal: 9")),
        "{result:?}"
    );
    fx.assert_untouched();
}

#[test]
fn upd_014_wrong_version_rejected() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new("9.9.9"));
    let result = Rig::new().apply(&fx);
    assert!(
        matches!(&result, Err(UpdateError::SelfTest(m)) if m.contains("dockering 9.9.9") && m.contains("dockering 2.0.0")),
        "{result:?}"
    );
    fx.assert_untouched();
}

#[test]
fn upd_003_appimage_self_test_is_required() {
    // The hash and size are right, but the file doesn't answer `--version`: the signed manifest
    // alone never replaces the installed AppImage.
    let silent = b"#!/bin/sh\nexit 0\n".to_vec();
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    fs::write(&fx.payload, &silent).expect("write");
    let sha = sha256_hex(&silent);
    let mut job = fx.job();
    job.expected_sha256 = &sha;
    job.expected_size = silent.len() as u64;
    let result = flow::apply(&job, &Rig::new().seams());
    assert!(
        matches!(result, Err(UpdateError::SelfTest(_))),
        "{result:?}"
    );
    fx.assert_untouched();
}

#[test]
fn upd_014_self_test_timeout() {
    // `exec` makes the shell itself the sleeper, so the kill ends the whole thing.
    let fx = Fixture::new(
        &Script::new(OLD),
        &Script::new(NEW).on_version("exec sleep 30"),
    );
    let mut job = fx.job();
    job.self_test_timeout = Duration::from_millis(300);
    let started = Instant::now();
    let result = flow::apply(&job, &Rig::new().seams());
    assert!(
        matches!(&result, Err(UpdateError::SelfTest(m)) if m.contains("didn't finish")),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the child wasn't killed"
    );
    fx.assert_untouched();
}

#[test]
fn upd_014_self_test_output_is_capped_and_a_chatty_child_cannot_hang_it() {
    let chatty = r#"i=0
  while [ $i -lt 4000 ]; do echo "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"; i=$((i+1)); done"#;
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW).on_version(chatty));
    let started = Instant::now();
    let result = Rig::new().apply(&fx);
    assert!(
        matches!(result, Err(UpdateError::SelfTest(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    fx.assert_untouched();
}

#[test]
fn upd_014_etxtbsy_retried() {
    // Five retries after the first attempt, so five injected failures still end in success...
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::without_relaunch();
    rig.spawner.etxtbsy_failures.store(5, Ordering::SeqCst);
    let outcome = rig.apply(&fx);
    assert!(is_restart_manually(&outcome).is_some(), "{outcome:?}");
    let self_tests = rig
        .spawner
        .attempts()
        .into_iter()
        .filter(|p| *p == Purpose::SelfTest)
        .count();
    // Six attempts at least: a real `ETXTBSY` on top would only add more.
    assert!(self_tests >= 6, "{self_tests}");
    fx.assert_replaced();

    // ...and a sixth failure is the end: the file stays busy, the old one stays in place. Every
    // attempt is an injected failure, so the count is exact.
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::new();
    rig.spawner.etxtbsy_failures.store(6, Ordering::SeqCst);
    let result = rig.apply(&fx);
    assert!(
        matches!(&result, Err(UpdateError::SelfTest(m)) if m.contains("couldn't be run")),
        "{result:?}"
    );
    assert_eq!(rig.spawner.attempts(), [Purpose::SelfTest; 6]);
    fx.assert_untouched();
}

#[test]
fn upd_014_etxtbsy_retried_for_a_file_still_open_for_writing() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::new();
    // The kernel refuses to execute a file that is open for writing.
    let writer = OpenOptions::new()
        .write(true)
        .open(&fx.payload)
        .expect("writer");
    *rig.spawner.hold.lock().expect("hold") = Some(writer);
    let env = ChildEnv::new(&fx.env);
    let seams = Seams {
        etxtbsy_delay: Duration::from_millis(20),
        ..rig.seams()
    };
    let result = selftest::self_test(&fx.payload, NEW, &env, Duration::from_secs(10), &seams);
    assert_eq!(result, Ok(()));
    // At least once: a test thread that forks while the writer is open keeps a copy of the
    // descriptor until its own `exec`, which can add another busy answer after the release.
    assert!(
        rig.spawner.etxtbsy_seen.load(Ordering::SeqCst) >= 1,
        "the first attempt should have met ETXTBSY"
    );
}

#[test]
fn upd_014_hash_mismatch_while_staging() {
    // The payload changes after the step 1 check, while the copy is made: the same length with
    // other content, and a longer file.
    let original = Script::new(NEW).render();
    let same_length = {
        let mut bytes = original.clone();
        bytes[2] ^= 0x01;
        bytes
    };
    // `ChangePayload` overwrites from the start, so the longer file is the whole new content.
    let longer = {
        let mut bytes = original;
        bytes.extend_from_slice(b"# appended\n");
        bytes
    };
    for changed in [same_length, longer] {
        let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
        let mut rig = Rig::new();
        rig.sink = TestSink(SinkMode::ChangePayload(fx.payload.clone(), changed));
        let result = rig.apply(&fx);
        assert_eq!(result, Err(UpdateError::Integrity));
        fx.assert_untouched();
        assert!(
            rig.spawner.calls().is_empty(),
            "nothing may run a changed file"
        );
    }
}

#[test]
fn upd_003_appimage_staged_hash_checked() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::new();
    // Before staging: a wrong digest, or a wrong size.
    let mut job = fx.job();
    let wrong = "0".repeat(64);
    job.expected_sha256 = &wrong;
    assert_eq!(flow::apply(&job, &rig.seams()), Err(UpdateError::Integrity));
    let mut job = fx.job();
    job.expected_size += 1;
    assert_eq!(flow::apply(&job, &rig.seams()), Err(UpdateError::Integrity));
    fx.assert_untouched();
    assert!(rig.spawner.calls().is_empty());
    // A missing payload can't be hashed.
    let mut job = fx.job();
    let missing = fx.root.path().join("updates").join("gone");
    job.payload = &missing;
    assert!(matches!(
        flow::apply(&job, &rig.seams()),
        Err(UpdateError::Io(_))
    ));
    fx.assert_untouched();
    // The digest is hex, whatever its case, and may carry whitespace.
    let upper = format!(" {} \n", fx.sha.to_uppercase());
    let mut job = fx.job();
    job.expected_sha256 = &upper;
    let outcome = flow::apply(&job, &Rig::without_relaunch().seams());
    assert!(is_restart_manually(&outcome).is_some(), "{outcome:?}");
}

#[test]
fn upd_014_disk_full_keeps_old_and_offer_ready() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW).padded(300 * 1024));
    let mut rig = Rig::new();
    rig.sink = TestSink(SinkMode::DiskFullAfter(100 * 1024));
    let result = rig.apply(&fx);
    // `Stage` is what keeps the offer *Ready* for a retry.
    assert!(
        matches!(&result, Err(UpdateError::Stage(m)) if m.contains("couldn't write") && m.contains("No space left")),
        "{result:?}"
    );
    fx.assert_untouched();
    assert!(rig.spawner.calls().is_empty());
}

#[test]
fn upd_014_stage_failures_are_stage_errors_that_leave_nothing_behind() {
    for mode in [SinkMode::CreateDenied, SinkMode::SyncFails] {
        let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
        let mut rig = Rig::new();
        rig.sink = TestSink(mode);
        let result = rig.apply(&fx);
        assert!(matches!(result, Err(UpdateError::Stage(_))), "{result:?}");
        fx.assert_untouched();
    }
}

#[test]
fn upd_014_rename_failure_keeps_old_and_removes_the_staged_file() {
    // The target is replaced by a non-empty directory after the checks (the staged file can't
    // be renamed over a directory): `rename` fails, which is a `Stage` error like a full disk.
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let sink_target = fx.target.clone();
    struct Swapper(PathBuf);
    impl Spawner for Swapper {
        fn spawn(&self, command: &mut Command, purpose: Purpose) -> io::Result<Child> {
            // After the self-test has run, turn the target into a directory.
            let child = RealSpawner.spawn(command, purpose)?;
            fs::remove_file(&self.0)?;
            fs::create_dir(&self.0)?;
            fs::write(self.0.join("keep"), b"x")?;
            Ok(child)
        }
    }
    let swapper = Swapper(sink_target);
    let rig = Rig::new();
    let seams = Seams {
        spawner: &swapper,
        ..rig.seams()
    };
    let result = flow::apply(&fx.job(), &seams);
    assert!(
        matches!(&result, Err(UpdateError::Stage(m)) if m.contains("couldn't replace")),
        "{result:?}"
    );
    assert_eq!(
        fx.listing(),
        ["Dockering.AppImage"],
        "the staged file was left behind"
    );
}

#[test]
fn upd_014_lost_eligibility_is_unsupported() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    // The real probe wants an ELF header with the AppImage marker; a script has neither.
    assert_eq!(
        flow::apply(&fx.job(), &PRODUCTION),
        Err(UpdateError::Unsupported)
    );
    assert_eq!(apply_appimage(fx.job()), Err(UpdateError::Unsupported));
    assert_eq!(
        restart_installed(&fx.target, NEW, &fx.env),
        Err(UpdateError::Unsupported)
    );
    fx.assert_untouched();

    // A read-only directory.
    let mut rig = Rig::new();
    rig.probe.writable = false;
    assert_eq!(rig.apply(&fx), Err(UpdateError::Unsupported));
    fx.assert_untouched();

    // A symbolic link is never replaced, a missing file and a relative path aren't targets.
    let rig = Rig::new();
    let link = fx.dir().join("link");
    std::os::unix::fs::symlink(&fx.target, &link).expect("symlink");
    let missing = fx.dir().join("missing");
    for target in [
        link.as_path(),
        missing.as_path(),
        Path::new("Dockering.AppImage"),
    ] {
        let mut job = fx.job();
        job.target = target;
        assert_eq!(
            flow::apply(&job, &rig.seams()),
            Err(UpdateError::Unsupported),
            "{target:?}"
        );
    }
    fs::remove_file(&link).expect("rm link");
    fx.assert_untouched();
    assert!(rig.spawner.calls().is_empty());
}

#[test]
fn upd_014_the_version_must_be_semver() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    for bad in ["", "2.0", "v2.0.0", "2.0.0 extra", "latest"] {
        let mut job = fx.job();
        job.expected_version = bad;
        assert!(
            matches!(
                flow::apply(&job, &Rig::new().seams()),
                Err(UpdateError::Manifest(_))
            ),
            "{bad:?}"
        );
    }
    fx.assert_untouched();
}

// ----- UPD-014: failures after the rename --------------------------------------------------

#[test]
fn upd_014_relaunch_failure_is_installed_restart() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::without_relaunch();
    let outcome = rig.apply(&fx);
    let reason = is_restart_manually(&outcome).expect("InstalledRestartManually");
    assert!(
        reason.contains("couldn't be started") && reason.contains("no display available"),
        "{reason}"
    );
    // The file on disk is the new one: this is never reported as 'nothing changed'.
    fx.assert_replaced();
    assert_eq!(
        rig.spawner.purposes(),
        [Purpose::SelfTest, Purpose::Relaunch]
    );
}

#[test]
fn upd_014_fsync_failure_after_rename_is_installed_restart() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let mut rig = Rig::new();
    rig.fsync.fail = true;
    let outcome = rig.apply(&fx);
    let reason = is_restart_manually(&outcome).expect("InstalledRestartManually");
    assert!(reason.contains("folder couldn't be flushed"), "{reason}");
    fx.assert_replaced();
    // Nothing was started: the retry (`restart_installed`) does that.
    assert_eq!(rig.spawner.purposes(), [Purpose::SelfTest]);
}

#[test]
fn upd_014_immediate_child_death_is_installed_restart() {
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW).on_relaunch("exit 3"));
    let mut job = fx.job();
    job.relaunch_grace = Duration::from_secs(5);
    let started = Instant::now();
    let outcome = flow::apply(&job, &Rig::new().seams());
    let reason = is_restart_manually(&outcome).expect("InstalledRestartManually");
    assert!(
        reason.contains("exited at once") && reason.contains("exit status: 3"),
        "{reason}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "death is noticed before the grace ends"
    );
    fx.assert_replaced();
}

// ----- UPD-014: the *Installed, restart* retry ---------------------------------------------

#[test]
fn upd_014_installed_restart_retry_starts_without_staging() {
    // The new file is installed already; the payload is gone (cleanup) and mustn't be needed.
    let fx = Fixture::new(&Script::new(OLD), &Script::recording_new());
    fs::copy(&fx.payload, &fx.target).expect("install");
    fs::remove_file(&fx.payload).expect("rm payload");
    let before = fx.listing();
    let rig = Rig::new();
    let result = flow::restart(&fx.target, NEW, &fx.env, limits(), &rig.seams());
    assert_eq!(result, Ok(AppImageOutcome::Restarting));
    assert_eq!(
        rig.spawner.purposes(),
        [Purpose::SelfTest, Purpose::Relaunch],
        "self-test and start only"
    );
    assert_eq!(
        rig.spawner.calls()[0].program,
        fx.target,
        "the installed file is tested"
    );
    assert_eq!(fx.listing(), before, "nothing was staged");
    assert_eq!(rig.fsync.calls.load(Ordering::SeqCst), 0);
    wait_for(&fx.out("relaunch.done"));
    assert_eq!(
        fs::read_to_string(fx.out("selftest.env")).expect("env"),
        SCRUBBED_DUMP
    );
}

#[test]
fn upd_014_installed_restart_retry_reports_a_bad_file_and_a_failed_start() {
    // The installed file doesn't answer as the offered version: nothing is started.
    let fx = Fixture::new(&Script::new(OLD), &Script::new(NEW));
    let rig = Rig::new();
    let result = flow::restart(&fx.target, NEW, &fx.env, limits(), &rig.seams());
    assert!(
        matches!(result, Err(UpdateError::SelfTest(_))),
        "{result:?}"
    );
    assert_eq!(rig.spawner.purposes(), [Purpose::SelfTest]);

    // It answers, but the start fails again: still `InstalledRestartManually`, no error.
    fs::copy(&fx.payload, &fx.target).expect("install");
    let rig = Rig::without_relaunch();
    let outcome = flow::restart(&fx.target, NEW, &fx.env, limits(), &rig.seams());
    assert!(is_restart_manually(&outcome).is_some(), "{outcome:?}");
}

// ----- UPD-014: the scrubbed environment ---------------------------------------------------

#[test]
fn upd_014_self_test_and_relaunch_share_scrubbed_env() {
    let process_appdir = std::env::var_os("APPDIR");
    let fx = Fixture::recording();
    let rig = Rig::new();
    assert_eq!(rig.apply(&fx), Ok(AppImageOutcome::Restarting));
    wait_for(&fx.out("relaunch.done"));

    // What the two real children saw: APPDIR, APPIMAGE, ARGV0 and OWD gone; the entries under
    // the old APPDIR gone from PATH and XDG_DATA_DIRS; the emptied LD_LIBRARY_PATH, QT_PLUGIN_PATH
    // and GST_PLUGIN_SYSTEM_PATH_1_0 removed (not set to ''); an entry that only shares a string
    // prefix with APPDIR kept; unrelated variables and APPIMAGE_EXTRACT_AND_RUN untouched.
    let selftest = fs::read_to_string(fx.out("selftest.env")).expect("selftest.env");
    let relaunch = fs::read_to_string(fx.out("relaunch.env")).expect("relaunch.env");
    assert_eq!(selftest, SCRUBBED_DUMP);
    assert_eq!(relaunch, SCRUBBED_DUMP);

    // And the two builders were configured identically.
    let (self_test, relaunch) = (
        rig.spawner.call(Purpose::SelfTest),
        rig.spawner.call(Purpose::Relaunch),
    );
    assert_eq!(self_test.envs, relaunch.envs);
    // `env_clear` first, so a removed variable is simply absent from the builder.
    for gone in ["APPDIR", "APPIMAGE", "ARGV0", "OWD", "LD_LIBRARY_PATH"] {
        assert!(
            !self_test.envs.iter().any(|(name, _)| name == gone),
            "{gone}"
        );
    }
    assert!(self_test.envs.contains(&(
        OsString::from("PATH"),
        Some(OsString::from("/usr/bin:/bin"))
    )));

    // Nothing was changed in this process: the variables only ever existed in the job.
    assert_eq!(std::env::var_os("APPDIR"), process_appdir);
    assert!(std::env::var_os("KEEP_ME").is_none() && std::env::var_os("DK_OUT").is_none());
}

#[test]
fn upd_014_child_env_is_the_snapshot_minus_the_plan() {
    let vars: Vec<(OsString, OsString)> = [
        ("PATH", "/tmp/.mount_a/bin:/usr/bin"),
        ("APPDIR", "/tmp/.mount_a"),
        ("LD_LIBRARY_PATH", "/tmp/.mount_a/lib"),
        ("HOME", "/home/u"),
    ]
    .iter()
    .map(|(k, v)| (OsString::from(k), OsString::from(v)))
    .collect();
    let env = ChildEnv::new(&vars);
    let command = env.command(Path::new("/x"));
    assert_eq!(env_of(&command, "PATH"), Some(OsString::from("/usr/bin")));
    assert_eq!(env_of(&command, "HOME"), Some(OsString::from("/home/u")));
    assert_eq!(env_of(&command, "APPDIR"), None);
    assert_eq!(env_of(&command, "LD_LIBRARY_PATH"), None);
    assert_eq!(env.plan().remove.len(), 5);
    // A snapshot without APPDIR can't say what to drop from the lists.
    let vars = vars[..1].to_vec();
    let command = ChildEnv::new(&vars).command(Path::new("/x"));
    assert_eq!(
        env_of(&command, "PATH"),
        Some(OsString::from("/tmp/.mount_a/bin:/usr/bin"))
    );
}

// ----- UPD-014: the stdin pipe -------------------------------------------------------------

#[test]
fn upd_014_stdin_pipe_outlives_function_scope() {
    let fx = Fixture::new(
        &Script::new(OLD),
        &Script::new(NEW).on_relaunch(
            r#": > "$DK_OUT/started"
  if read line; then : > "$DK_OUT/got_data"; else : > "$DK_OUT/eof"; fi
  exit 0"#,
        ),
    );
    let rig = Rig::new();
    assert_eq!(rig.apply(&fx), Ok(AppImageOutcome::Restarting));
    // `apply` has returned and everything it owned is dropped. The child has not seen
    // end-of-file: the write end lives in a `static`.
    wait_for(&fx.out("started"));
    assert!(kept_pipes_for_tests() >= 1);
    thread::sleep(Duration::from_millis(400));
    assert!(
        !fx.out("eof").exists(),
        "the pipe was closed when the function returned"
    );
    // The kernel closing the old process is what ends it; the test stands in for that.
    close_relaunch_pipes_for_tests();
    wait_for(&fx.out("eof"));
    assert!(!fx.out("got_data").exists());
}

// ----- the frozen formats ------------------------------------------------------------------

#[test]
fn upd_014_version_output_format_frozen() {
    for ok in [
        "dockering 2.0.0",
        "dockering 2.0.0\n",
        "  dockering 2.0.0 \r\n",
    ] {
        assert!(version_output_matches(ok.as_bytes(), "2.0.0"), "{ok:?}");
    }
    for bad in [
        "",
        "2.0.0",
        "Dockering 2.0.0",
        "dockering v2.0.0",
        "dockering  2.0.0",
        "dockering\t2.0.0",
        "dockering 2.0.0 (abc1234)",
        "dockering 2.0.0\nsecond line",
        "dockering 2.0.1",
        "dockering 2.0",
        "dockering 2.0.0-rc.1",
        "dockering 12.0.0",
        "xdockering 2.0.0",
        "dockering 2.0.0\u{0}",
    ] {
        assert!(!version_output_matches(bad.as_bytes(), "2.0.0"), "{bad:?}");
    }
    assert!(!version_output_matches(b"dockering 2.0.0\xff", "2.0.0"));
    // Pre-release and build metadata are part of the version, and exact.
    assert!(version_output_matches(
        b"dockering 2.0.0-rc.1+b5\n",
        "2.0.0-rc.1+b5"
    ));
    assert!(!version_output_matches(
        b"dockering 2.0.0-rc.1\n",
        "2.0.0-rc.1+b5"
    ));

    // End to end: an extra word fails the self-test, the exact line passes it.
    let fx = Fixture::new(
        &Script::new(OLD),
        &Script::new(NEW).on_version(r#"echo "dockering 2.0.0 extra""#),
    );
    // (the script prints that line, then `dockering 2.0.0`: two lines)
    assert!(matches!(
        Rig::new().apply(&fx),
        Err(UpdateError::SelfTest(_))
    ));
    fx.assert_untouched();
}

#[test]
fn upd_014_relaunch_flag_is_the_frozen_literal() {
    assert_eq!(RELAUNCH_FLAG, "--update-relaunch=1");
    assert_eq!(crate::apply::RELAUNCH_FLAG, RELAUNCH_FLAG);
}
