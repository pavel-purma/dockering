//! UPD-017 / SHL-022: the relaunch wait.
//!
//! These tests run on plain `std` threads, so they use real (short) time instead of the paused
//! tokio clock the async hub tests use. Every assertion is on an ordering or a lower bound, never
//! on an upper bound, so a slow machine can't make one flaky.

use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn upd_017_is_relaunch_matches_the_exact_flag_only() {
    assert_eq!(RELAUNCH_FLAG, "--update-relaunch=1");
    assert!(is_relaunch(&args(&["--update-relaunch=1"])));
    assert!(is_relaunch(&args(&["--demo", "--update-relaunch=1", "x"])));
    for ignored in [
        "--update-relaunch=2",
        "--update-relaunch=0",
        "--update-relaunch=",
        "--update-relaunch",
        "--update-relaunch=11",
        "--update-relaunch=1 ",
        "update-relaunch=1",
        "--Update-Relaunch=1",
        "-update-relaunch=1",
        "",
    ] {
        assert!(
            !is_relaunch(&args(&[ignored])),
            "{ignored:?} must be ignored"
        );
    }
    assert!(!is_relaunch(&[]));
}

#[test]
fn upd_017_wait_outcome_gone_means_eof_or_read_error() {
    assert!(WaitOutcome::Eof.predecessor_gone());
    assert!(WaitOutcome::ReadError.predecessor_gone());
    for other in [
        WaitOutcome::NotRequested,
        WaitOutcome::StdinIsTerminal,
        WaitOutcome::TimedOut,
    ] {
        assert!(!other.predecessor_gone(), "{other:?}");
    }
}

/// Windows has no handshake: nothing is waited for and the reader is never read.
#[cfg(not(unix))]
#[test]
fn upd_017_wait_is_not_applicable_off_unix() {
    struct Untouchable;
    impl std::io::Read for Untouchable {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("the reader must not be read on this platform");
        }
    }
    let result = wait_for_predecessor(Untouchable, Duration::from_secs(30));
    assert_eq!(result.outcome, WaitOutcome::NotRequested);
    assert!(!result.timed_out);
    assert_eq!(result.waited_ms, 0);
    let result = wait_for_predecessor_from_args(&args(&[RELAUNCH_FLAG]));
    assert_eq!(result.outcome, WaitOutcome::NotRequested);
}

#[cfg(unix)]
mod unix {
    use std::io::{self, BufRead, BufReader, Read};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc as std_mpsc;
    use std::time::Instant;

    use futures::StreamExt;

    use super::*;

    /// A reader that blocks in `read` until the paired sender is sent to or dropped, then reads as
    /// end-of-file: the shape of a pipe whose write end the predecessor holds. It tells `started`
    /// when `read` is first entered and `finished` when it returns.
    struct PipeLike {
        release: std_mpsc::Receiver<()>,
        started: std_mpsc::Sender<()>,
        finished: std_mpsc::Sender<()>,
    }

    struct PipeProbe {
        /// Drop (or send to) this to let the reader reach end-of-file.
        writer: std_mpsc::Sender<()>,
        started: std_mpsc::Receiver<()>,
        finished: std_mpsc::Receiver<()>,
    }

    fn pipe_like() -> (PipeProbe, PipeLike) {
        let (writer, release) = std_mpsc::channel();
        let (started_tx, started) = std_mpsc::channel();
        let (finished_tx, finished) = std_mpsc::channel();
        (
            PipeProbe {
                writer,
                started,
                finished,
            },
            PipeLike {
                release,
                started: started_tx,
                finished: finished_tx,
            },
        )
    }

    impl Read for PipeLike {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            let _ = self.started.send(());
            let _ = self.release.recv();
            let _ = self.finished.send(());
            Ok(0)
        }
    }

    /// Generous bound for "this must happen", never for "this must be fast".
    const PATIENCE: Duration = Duration::from_secs(10);

    #[test]
    fn upd_017_wait_returns_on_stdin_eof() {
        let (probe, reader) = pipe_like();
        let PipeProbe {
            writer, started, ..
        } = probe;
        let predecessor = std::thread::spawn(move || {
            // The predecessor exits a little after the wait began: 20 ms is a floor for
            // `waited_ms`, so it is above 0 however fast the machine is.
            started
                .recv_timeout(PATIENCE)
                .expect("the wait must start reading");
            std::thread::sleep(Duration::from_millis(20));
            drop(writer);
        });

        let result = wait_for_predecessor(reader, PATIENCE);

        predecessor.join().unwrap();
        assert_eq!(result.outcome, WaitOutcome::Eof);
        assert!(!result.timed_out);
        assert!(result.waited_ms >= 20, "waited {} ms", result.waited_ms);
        assert!(result.outcome.predecessor_gone());
    }

    #[test]
    fn upd_017_wait_returns_on_read_error() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("the pipe broke"))
            }
        }
        let result = wait_for_predecessor(Failing, PATIENCE);
        assert_eq!(result.outcome, WaitOutcome::ReadError);
        assert!(!result.timed_out);
        assert!(
            result.outcome.predecessor_gone(),
            "a read error counts as EOF"
        );
    }

    #[test]
    fn upd_017_wait_times_out_and_continues() {
        let (probe, reader) = pipe_like();
        let timeout = Duration::from_millis(50);

        // The pipe never closes: the call must still return, after the timeout.
        let result = wait_for_predecessor(reader, timeout);

        assert_eq!(result.outcome, WaitOutcome::TimedOut);
        assert!(result.timed_out);
        assert!(result.waited_ms >= 50, "waited {} ms", result.waited_ms);
        assert!(!result.outcome.predecessor_gone());
        // The helper was left blocked in `read` (it did start) and ends on its own once the
        // pipe closes, with nobody listening to its result.
        assert!(probe.started.try_recv().is_ok());
        assert!(
            probe.finished.try_recv().is_err(),
            "still blocked after the timeout"
        );
        drop(probe.writer);
        probe
            .finished
            .recv_timeout(PATIENCE)
            .expect("the blocked helper ends when the pipe closes");
    }

    #[test]
    fn upd_017_wait_discards_data_and_retries_interrupted_reads() {
        struct Script(Vec<io::Result<Vec<u8>>>);
        impl Read for Script {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let bytes = self.0.remove(0)?;
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
        }
        let script = Script(vec![
            Ok(b"stray bytes".to_vec()),
            Err(io::Error::from(io::ErrorKind::Interrupted)),
            Ok(b"more".to_vec()),
        ]);
        let result = wait_for_predecessor(script, PATIENCE);
        assert_eq!(result.outcome, WaitOutcome::Eof);
    }

    #[test]
    fn upd_017_wait_with_a_dead_helper_is_a_read_error() {
        struct Panicking;
        impl Read for Panicking {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("reader failure (expected in this test)");
            }
        }
        let result = wait_for_predecessor(Panicking, PATIENCE);
        assert_eq!(result.outcome, WaitOutcome::ReadError);
    }

    /// Panics if called: proves the code under test never reached for it.
    fn never<T>() -> T {
        panic!("must not be called")
    }

    #[test]
    fn upd_017_wait_ignores_terminal_stdin_and_unknown_flag() {
        let thirty = Duration::from_secs(30);

        // No flag, or any other value of it: nothing is looked at and nothing is waited for.
        for argv in [
            args(&[]),
            args(&["--demo"]),
            args(&["--update-relaunch=2"]),
            args(&["--update-relaunch"]),
            args(&["--update-relaunch=0", "extra"]),
        ] {
            let result = wait_when_relaunched(&argv, never::<bool>, never::<PipeLike>, thirty);
            assert_eq!(result.outcome, WaitOutcome::NotRequested, "{argv:?}");
            assert_eq!(result.waited_ms, 0);
            assert!(!result.timed_out);
        }

        // The flag with a terminal stdin (somebody ran it by hand): no wait, stdin not opened.
        let result =
            wait_when_relaunched(&args(&[RELAUNCH_FLAG]), || true, never::<PipeLike>, thirty);
        assert_eq!(result.outcome, WaitOutcome::StdinIsTerminal);
        assert_eq!(result.waited_ms, 0);
        assert!(!result.timed_out);

        // The flag with a pipe: it waits, and the reader is the one that was opened.
        let (probe, reader) = pipe_like();
        drop(probe.writer);
        let result = wait_when_relaunched(
            &args(&["--demo", RELAUNCH_FLAG]),
            || false,
            || reader,
            thirty,
        );
        assert_eq!(result.outcome, WaitOutcome::Eof);
    }

    /// A desktop launch has stdin on `/dev/null`, which reads end-of-file at once: harmless
    /// only because the wait is gated on the flag. With the flag it returns at once as `Eof`.
    #[test]
    fn upd_017_dev_null_never_waits_without_the_flag() {
        let open = || std::fs::File::open("/dev/null").expect("open /dev/null");
        let result = wait_when_relaunched(&[], || false, open, Duration::from_secs(30));
        assert_eq!(result.outcome, WaitOutcome::NotRequested);
        assert_eq!(result.waited_ms, 0);

        let result = wait_when_relaunched(
            &args(&[RELAUNCH_FLAG]),
            || false,
            open,
            Duration::from_secs(30),
        );
        assert_eq!(result.outcome, WaitOutcome::Eof);
    }

    fn next_focus(mut rx: mpsc::UnboundedReceiver<()>) -> Option<()> {
        let (done_tx, done_rx) = std_mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(futures::executor::block_on(rx.next()));
        });
        done_rx.recv_timeout(PATIENCE).ok().flatten()
    }

    /// SHL-022: while a relaunched process waits it never touches the single-instance socket,
    /// so the old (primary) instance gets no focus request. When the wait times out, the
    /// ordinary rule applies: the new launch focuses the primary and exits.
    #[test]
    fn shl_022_relaunch_never_sends_focus_while_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let Instance::Primary(mut primary) = acquire(&paths) else {
            panic!("the first launch must be the primary instance");
        };
        let mut focus = primary.take_focus_requests().unwrap();

        // The relaunched process: waits on a pipe that stays open until the timeout.
        let (probe, reader) = pipe_like();
        let waiter = std::thread::spawn(move || {
            wait_when_relaunched(
                &args(&[RELAUNCH_FLAG]),
                || false,
                || reader,
                Duration::from_millis(300),
            )
        });
        probe
            .started
            .recv_timeout(PATIENCE)
            .expect("the wait must be blocked in read");
        // Mid-wait: no connection, no focus request (the channel is open and empty).
        assert_eq!(
            focus.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "a focus request arrived while waiting"
        );

        let result = waiter.join().unwrap();
        assert_eq!(result.outcome, WaitOutcome::TimedOut);
        assert!(result.waited_ms >= 300);
        // Neither did the end of the wait send one.
        assert_eq!(
            focus.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "the timeout must not send a focus request"
        );

        // After the timeout the normal single-instance rule applies.
        assert!(matches!(acquire(&paths), Instance::Secondary));
        assert_eq!(
            next_focus(focus),
            Some(()),
            "the ordinary launch must focus the primary"
        );
        drop(probe.writer);
        drop(primary);
    }

    // ---- real processes -------------------------------------------------------------------
    //
    // The wait is exercised on a real pipe by re-executing this test binary: the child runs
    // `upd_017_relaunch_child_helper` (a no-op in a normal run) with its stdin on a pipe the
    // parent holds. That is the shape of the updater's relaunch (UPD-014 step 5) without a GUI.

    const CHILD_MODE: &str = "DK_TEST_RELAUNCH_CHILD";

    /// Re-exec target. Does nothing unless the parent test set `DK_TEST_RELAUNCH_CHILD`.
    #[test]
    fn upd_017_relaunch_child_helper() {
        use std::io::IsTerminal;

        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        let argv = match mode.as_str() {
            "flag" => args(&[RELAUNCH_FLAG]),
            "no-flag" => args(&[]),
            other => panic!("unknown child mode {other:?}"),
        };
        println!("RELAUNCH_WAITING");
        // The real stdin and the real terminal check, with a 20 s ceiling so a broken
        // handshake fails the parent's assertion instead of hanging the run.
        let result = wait_when_relaunched(
            &argv,
            || std::io::stdin().is_terminal(),
            real_stdin,
            Duration::from_secs(20),
        );
        println!(
            "RELAUNCH_RESULT outcome={:?} timed_out={} waited_ms={}",
            result.outcome, result.timed_out, result.waited_ms
        );
    }

    /// Kills the child if a failed assertion unwinds before the test reaps it.
    struct Reaper(Child);

    impl Drop for Reaper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn_child(mode: &str, stdin: Stdio) -> Reaper {
        let exe = std::env::current_exe().expect("test binary path");
        // `module_path!()` is `dk_hub::single_instance::relaunch_tests::unix`; the harness
        // filter has no crate prefix.
        let module = module_path!().split_once("::").expect("crate prefix").1;
        let child = Command::new(exe)
            .arg(format!("{module}::upd_017_relaunch_child_helper"))
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_MODE, mode)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .spawn()
            .expect("re-exec the test binary");
        Reaper(child)
    }

    /// Reads the child's stdout up to the first line containing `marker` and returns that line
    /// from the marker on. (The harness prints `test <name> ... ` without a newline before a
    /// `--nocapture` test runs, so the child's first `println!` shares a line with it.)
    fn read_until(lines: &mut impl Iterator<Item = String>, marker: &str) -> String {
        lines
            .find_map(|line| line.find(marker).map(|at| line[at..].to_string()))
            .unwrap_or_else(|| panic!("the child ended without printing {marker:?}"))
    }

    fn child_lines(child: &mut Reaper) -> impl Iterator<Item = String> + use<> {
        let out = child.0.stdout.take().expect("piped stdout");
        BufReader::new(out).lines().map_while(Result::ok)
    }

    #[test]
    fn upd_017_real_pipe_eof_arrives_when_the_writer_is_dropped() {
        let mut child = spawn_child("flag", Stdio::piped());
        let writer = child.0.stdin.take().expect("piped stdin");
        let mut lines = child_lines(&mut child);

        read_until(&mut lines, "RELAUNCH_WAITING");
        // While the writer is open the child stays in the wait: it neither printed a result
        // nor exited. (A wait that returned early would have by now.)
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "the child left the wait early"
        );

        // The old process exits: the kernel closes the write end and the child sees EOF.
        let closed = Instant::now();
        drop(writer);
        let result = read_until(&mut lines, "RELAUNCH_RESULT");
        assert!(
            result.starts_with("RELAUNCH_RESULT outcome=Eof timed_out=false"),
            "{result}"
        );
        assert!(closed.elapsed() < PATIENCE);
        assert!(child.0.wait().unwrap().success());
    }

    #[test]
    fn upd_017_real_process_without_the_flag_never_waits() {
        // stdin stays open the whole time, so a wait that wasn't gated on the flag would
        // block until the child's 20 s ceiling.
        let mut child = spawn_child("no-flag", Stdio::piped());
        let writer = child.0.stdin.take().expect("piped stdin");
        let mut lines = child_lines(&mut child);

        let result = read_until(&mut lines, "RELAUNCH_RESULT");
        assert!(
            result.starts_with("RELAUNCH_RESULT outcome=NotRequested timed_out=false waited_ms=0"),
            "{result}"
        );
        assert!(child.0.wait().unwrap().success());
        drop(writer);
    }

    #[test]
    fn upd_017_real_dev_null_stdin_with_the_flag_reads_eof_at_once() {
        let mut child = spawn_child("flag", Stdio::null());
        let mut lines = child_lines(&mut child);
        let result = read_until(&mut lines, "RELAUNCH_RESULT");
        assert!(
            result.starts_with("RELAUNCH_RESULT outcome=Eof timed_out=false"),
            "{result}"
        );
        assert!(child.0.wait().unwrap().success());
    }
}
