"""REL-085, REL-086: the host-side pieces of the Linux update end-to-end test, checked without Docker, a build or a network.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.

Lowest layer that proves each piece:
- `scripts/linux-update/serve.py` is started for real on a free loopback port and spoken to over HTTP: what it serves, the
  request log, the fault switch, path traversal, the bind address and SIGTERM are its behaviour, not a contract of text;
- `scripts/linux-update/build-appimages.sh` runs under bash against a fake `cargo` (and a fake `git`) that come first on PATH and
  record their argv, working directory and the environment variables that matter, so which features and which build-time
  version each AppImage gets is a real code path with scripted answers;
- contract tests read the release workflow and the script headers. That the AppImages themselves build and print their versions
  needs a full build: see the `--version` checks inside build-appimages.sh.
`scripts/linux-update/run.sh` and the scenarios S1-S4 need Docker and are tests of their own (plan tasks 13, 14).
"""

import http.client
import json
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
UPDATE = Path(os.environ.get("UPDATE_SCRIPT_DIR") or ROOT / "scripts/linux-update")
SERVE = UPDATE / "serve.py"
BUILD = UPDATE / "build-appimages.sh"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
BASH = shutil.which("bash")
WINDOWS = os.name == "nt"
UA = "Dockering/0.0.1 (linux; x86_64)"
MANIFEST_PATH = "/pavel-purma/dockering/releases/latest/download/dockering-update.json"
ASSET_PATH = "/pavel-purma/dockering/releases/download/v1.2.3/Dockering-x86_64.AppImage"
LOG_KEYS = {"ts", "method", "path", "user_agent", "status"}


def read(path):
    return Path(path).read_text(encoding="utf-8")


def workspace_version():
    table = re.search(r"^\[workspace\.package\]\n((?:(?!\[).*\n)+)", read(ROOT / "Cargo.toml"), flags=re.M)
    return re.search(r'^version = "([^"]+)"', table.group(1), flags=re.M).group(1)


class Serve:
    """A running serve.py on a free port, with its request log and control file in a scratch directory."""

    def __init__(self, test, *, files=None, extra=(), log=True, control=True, log_path=None, tmp=None):
        self.tmp = tmp or Path(tempfile.mkdtemp(prefix="dockering-serve-test-"))
        if tmp is None:
            test.addCleanup(shutil.rmtree, self.tmp, True)
        self.root = self.tmp / "root"
        self.root.mkdir(exist_ok=True)
        for name, data in (files or {}).items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.log = log_path or self.tmp / "requests.jsonl"
        self.control = self.tmp / "control"
        self.stderr_path = self.tmp / f"stderr-{len(list(self.tmp.glob('stderr-*')))}.txt"
        self.stderr = open(self.stderr_path, "w", encoding="utf-8")
        test.addCleanup(self.stderr.close)
        command = [sys.executable, "-B", str(SERVE), "--root", str(self.root), "--port", "0"]
        if log:
            command += ["--log", str(self.log)]
        if control:
            command += ["--control", str(self.control)]
        self.proc = subprocess.Popen([*command, *extra], stdout=subprocess.PIPE, stderr=self.stderr, text=True)
        test.addCleanup(self.kill)
        self.first_line = self.read_first_line()
        found = re.fullmatch(r"serve\.py: listening on 127\.0\.0\.1:(\d+)", self.first_line)
        if not found:
            test.fail(f"serve.py did not announce a loopback port: {self.first_line!r} {self.stderr_path.read_text()}")
        self.port = int(found.group(1))

    def read_first_line(self):
        lines = queue.Queue()
        threading.Thread(target=lambda: lines.put(self.proc.stdout.readline()), daemon=True).start()
        try:
            return lines.get(timeout=30).rstrip("\n")
        except queue.Empty:
            return "(no output within 30 s)"

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait(timeout=30)
        self.proc.stdout.close()

    def request(self, method, target, headers=None):
        """(status, headers, body) of one request; the target goes out exactly as written, `..` included."""
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=60)
        try:
            connection.putrequest(method, target, skip_accept_encoding=True)
            for name, value in (headers or {}).items():
                connection.putheader(name, value)
            connection.endheaders()
            response = connection.getresponse()
            return response.status, {k.lower(): v for k, v in response.getheaders()}, response.read()
        finally:
            connection.close()

    def get(self, target, headers=None):
        return self.request("GET", target, headers)

    def log_entries(self):
        if not self.log.exists():
            return []
        return [json.loads(line) for line in read(self.log).splitlines() if line]

    def set_mode(self, text):
        self.control.write_text(text, encoding="utf-8")


class ServeTests(unittest.TestCase):
    """REL-085: scripts/linux-update/serve.py, the static server inside the test container."""

    def setUp(self):
        self.payload = os.urandom(300_000)  # larger than one chunk and one TCP segment
        self.files = {
            MANIFEST_PATH.lstrip("/"): b'{"schema":1}\n',
            ASSET_PATH.lstrip("/"): self.payload,
        }

    def serve(self, **kwargs):
        return Serve(self, files=self.files, **kwargs)

    def test_rel_085_serve_serves_the_files_of_its_root_like_github_does(self):
        server = self.serve()
        status, headers, body = server.get(MANIFEST_PATH)
        self.assertEqual((status, body), (200, b'{"schema":1}\n'))
        self.assertEqual(headers["content-type"], "application/json")
        status, headers, body = server.get(ASSET_PATH)
        self.assertEqual(status, 200)
        self.assertEqual(body, self.payload, "the AppImage is sent byte for byte")
        self.assertEqual(headers["content-length"], str(len(self.payload)))
        self.assertEqual(headers["content-type"], "application/octet-stream")
        # HEAD: the same headers and no body; a query string is not part of the file name
        status, headers, body = server.request("HEAD", ASSET_PATH)
        self.assertEqual((status, body), (200, b""))
        self.assertEqual(headers["content-length"], str(len(self.payload)))
        self.assertEqual(server.get(MANIFEST_PATH + "?cache=1")[0], 200)
        self.assertEqual(server.get("/pavel-purma//dockering/./releases/latest/download/dockering-update.json")[0], 200)

    def test_rel_085_serve_does_not_invent_files_or_list_directories(self):
        server = self.serve()
        for target in ("/", "/pavel-purma", "/pavel-purma/dockering/releases/", "/nothing/here.json"):
            status, _, body = server.get(target)
            self.assertEqual(status, 404, target)
            self.assertNotIn(b"dockering-update.json", body, f"{target} must not list a directory")
        status, headers, _ = server.request("POST", MANIFEST_PATH)
        self.assertEqual(status, 405)
        self.assertEqual(headers["allow"], "GET, HEAD")
        self.assertEqual(server.request("DELETE", MANIFEST_PATH)[0], 405)

    def test_rel_085_serve_logs_one_json_line_per_request_with_the_exact_user_agent(self):
        server = self.serve()
        server.get(MANIFEST_PATH, {"User-Agent": UA})
        server.get(MANIFEST_PATH + ".minisig", {"User-Agent": UA})  # not in the fixture here: 404
        server.request("HEAD", ASSET_PATH, {"User-Agent": "curl/8.0 (odd; chars)"})
        server.get(MANIFEST_PATH)  # a client that sends no User-Agent
        entries = server.log_entries()  # read as soon as the answers are in: the line is written before the response
        self.assertEqual(len(entries), 4)
        for entry in entries:
            self.assertEqual(set(entry), LOG_KEYS)
            self.assertRegex(entry["ts"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")
        self.assertEqual(
            [(e["method"], e["path"], e["user_agent"], e["status"]) for e in entries],
            [
                ("GET", MANIFEST_PATH, UA, 200),
                ("GET", MANIFEST_PATH + ".minisig", UA, 404),
                ("HEAD", ASSET_PATH, "curl/8.0 (odd; chars)", 200),
                ("GET", MANIFEST_PATH, None, 200),
            ],
        )
        self.assertEqual(
            [e["ts"] for e in entries], sorted(e["ts"] for e in entries), "the lines are in the order the requests came"
        )
        self.assertEqual(len(read(server.log).splitlines()), 4, "exactly one line per request, no other output in the file")

    def test_rel_085_serve_log_is_appended_across_restarts_and_both_files_are_optional(self):
        first = self.serve()
        first.get(MANIFEST_PATH, {"User-Agent": UA})
        first.proc.send_signal(signal.SIGTERM if not WINDOWS else signal.SIGINT)
        first.proc.wait(timeout=30)
        second = Serve(self, extra=(), tmp=first.tmp)  # the same root and log, a new process
        second.get(MANIFEST_PATH, {"User-Agent": UA})
        self.assertEqual(len(second.log_entries()), 2, "the log is appended to, never truncated")

        bare = Serve(self, files=self.files, log=False, control=False)
        self.assertEqual(bare.get(MANIFEST_PATH)[0], 200)
        self.assertEqual(list(bare.tmp.glob("*.jsonl")), [], "without --log nothing is written")
        self.assertEqual(bare.get(MANIFEST_PATH)[0], 200, "without --control the switch is always ok")

    def test_rel_085_serve_fault_switch_is_read_on_every_request(self):
        server = self.serve()
        self.assertEqual(server.get(MANIFEST_PATH)[0], 200, "no control file: ok")
        server.set_mode("down\n")
        status, _, body = server.get(MANIFEST_PATH)
        self.assertEqual(status, 503)
        self.assertNotIn(b"schema", body)
        self.assertEqual(server.request("HEAD", ASSET_PATH)[0], 503, "every request, not only the manifest")
        self.assertEqual(server.get("/nothing")[0], 503)
        server.set_mode("ok")
        self.assertEqual(server.get(MANIFEST_PATH)[::2], (200, b'{"schema":1}\n'))
        server.set_mode("slow")
        start = time.monotonic()
        status, _, body = server.get(MANIFEST_PATH)
        elapsed = time.monotonic() - start
        self.assertEqual((status, body), (200, b'{"schema":1}\n'), "slow still answers")
        self.assertGreaterEqual(elapsed, 1.9, "slow delays the answer by 2 s")
        self.assertLess(elapsed, 20)
        server.set_mode("")
        start = time.monotonic()
        self.assertEqual(server.get(MANIFEST_PATH)[0], 200, "an empty file is ok again")
        self.assertLess(time.monotonic() - start, 1.5)
        server.set_mode("dwon")
        status, _, body = server.get(MANIFEST_PATH)
        self.assertEqual(status, 500, "a typo is loud, never a silent ok")
        self.assertIn(b"dwon", body)
        server.control.unlink()
        self.assertEqual(server.get(MANIFEST_PATH)[0], 200)
        self.assertEqual(
            [e["status"] for e in server.log_entries()],
            [200, 503, 503, 503, 200, 200, 200, 500, 200],
            "the log carries the status the client saw",
        )

    def test_rel_085_serve_refuses_path_traversal(self):
        server = self.serve()
        (server.tmp / "secret.txt").write_text("outside the root", encoding="utf-8")
        (server.root / "inside.txt").write_text("inside", encoding="utf-8")
        for target in (
            "/../secret.txt",
            "/pavel-purma/../../secret.txt",
            "/pavel-purma/dockering/../../../secret.txt",
            "/%2e%2e/secret.txt",
            "/%2E%2E/secret.txt",
            "/..%2fsecret.txt",
            "/..%2Fsecret.txt",
            "/%2e%2e%2fsecret.txt",
            "/pavel-purma/%2e%2e/%2e%2e/secret.txt",
            "/..\\secret.txt",
            "/..%5csecret.txt",
            "/inside.txt/../../secret.txt",
            "/..",
            "/../",
        ):
            status, _, body = server.get(target)
            self.assertIn(status, (400, 403), target)
            self.assertNotIn(b"outside the root", body, target)
        self.assertEqual(server.get("/inside.txt")[2], b"inside")
        self.assertTrue(all(e["status"] in (400, 403) for e in server.log_entries()[:-1]), "refusals are logged too")

    def test_rel_085_serve_binds_loopback_only(self):
        server = self.serve()
        self.assertEqual(server.first_line, f"serve.py: listening on 127.0.0.1:{server.port}")
        if sys.platform.startswith("linux"):
            # the whole of 127.0.0.0/8 is loopback on Linux: a socket bound to 0.0.0.0 would take this, one bound to 127.0.0.1 won't
            with self.assertRaises(OSError):
                socket.create_connection(("127.0.0.2", server.port), timeout=5).close()
            with socket.create_connection(("127.0.0.1", server.port), timeout=5):
                pass
        for option in (["--bind", "0.0.0.0"], ["--host", "0.0.0.0"]):
            result = subprocess.run(
                [sys.executable, "-B", str(SERVE), "--root", str(server.root), "--port", "0", *option],
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(result.returncode, 2, f"{option} is not an option: the address is not configurable")
            self.assertEqual(result.stdout, "")

    @unittest.skipIf(WINDOWS, "SIGTERM is delivered to a Python process on POSIX only")
    def test_rel_085_serve_stops_cleanly_on_sigterm(self):
        server = self.serve()
        self.assertEqual(server.get(MANIFEST_PATH)[0], 200)
        idle = socket.create_connection(("127.0.0.1", server.port), timeout=5)  # a client that connected and says nothing
        self.addCleanup(idle.close)
        server.proc.send_signal(signal.SIGTERM)
        self.assertEqual(server.proc.wait(timeout=20), 0, "SIGTERM is a clean exit, not a signal death or a traceback")
        self.assertNotIn("Traceback", server.stderr_path.read_text())
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", server.port), timeout=5).close()
        self.assertEqual(len(server.log_entries()), 1, "the log is complete and closed")

    def test_rel_085_serve_arguments_are_validated(self):
        def run(*args):
            return subprocess.run([sys.executable, "-B", str(SERVE), *args], capture_output=True, text=True, timeout=60)

        existing = str(Path(tempfile.gettempdir()))
        for args in (
            [],
            ["--root", existing],
            ["--port", "0"],
            ["--root", existing, "--port", "-1"],
            ["--root", existing, "--port", "65536"],
            ["--root", existing, "--port", "http"],
            ["--root", str(ROOT / "no-such-directory"), "--port", "0"],
            ["--root", str(SERVE), "--port", "0"],
            ["--root", existing, "--port", "0", "stray"],
            ["--root", existing, "--port", "0", "--nonsense"],
        ):
            result = run(*args)
            self.assertEqual(result.returncode, 2, f"{args}: {result.stderr}")
            self.assertEqual(result.stdout, "", args)
        helped = run("--help")
        self.assertEqual(helped.returncode, 0)
        self.assertIn("127.0.0.1", helped.stdout)

    @unittest.skipIf(WINDOWS, "SO_REUSEADDR lets a second socket share a port on Windows")
    def test_rel_085_serve_a_port_in_use_is_a_clean_failure(self):
        server = self.serve()
        second = subprocess.run(
            [sys.executable, "-B", str(SERVE), "--root", str(server.root), "--port", str(server.port)],
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(second.returncode, 1)
        self.assertIn("cannot listen on 127.0.0.1", second.stderr)
        self.assertNotIn("Traceback", second.stderr)


FAKE_CARGO = r"""#!/usr/bin/env bash
# test double for `cargo` as build-appimages.sh uses it: records what matters, writes what the real one would
set -eu
{
  printf 'cwd=%s\x1f' "$PWD"
  printf 'DOCKERING_E2E_VERSION=%s\x1f' "${DOCKERING_E2E_VERSION-<unset>}"
  printf 'CARGO_TARGET_DIR=%s\x1f' "${CARGO_TARGET_DIR-<unset>}"
  printf 'APPIMAGE_EXTRACT_AND_RUN=%s\x1f' "${APPIMAGE_EXTRACT_AND_RUN-<unset>}"
  printf 'NO_STRIP=%s\x1f' "${NO_STRIP-<unset>}"
  printf 'argv'
  for a in "$@"; do printf '\x1f%s' "$a"; done
  printf '\n'
} >>"$FAKE_LOG"
verb=$1
target="" features=""
shift
if [ "$verb" = xtask ]; then
  shift # package
fi
while [ $# -gt 0 ]; do
  case $1 in
    --target) target=$2; shift ;;
    --features) features=$2; shift ;;
  esac
  shift
done
case "$verb" in
  build)
    [ "${FAKE_CARGO_FAIL:-}" != build ] || exit 101
    version=$FAKE_WORKSPACE_VERSION
    case ",$features," in *,update-e2e,*) [ -z "${DOCKERING_E2E_VERSION:-}" ] || version=$DOCKERING_E2E_VERSION ;; esac
    bin=$CARGO_TARGET_DIR/$target/release/dockering
    mkdir -p "$(dirname "$bin")"
    {
      echo "not a binary; built with features '$features'"
      echo "version=$version"
      case ",$features," in *,updater,*) echo "https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json" ;; esac
      case ",$features," in *,update-e2e,*) echo "dockering-update-e2e-seam-v1" ;; esac
      [ -z "${FAKE_SEAM_EVERYWHERE:-}" ] || echo "dockering-update-e2e-seam-v1"
    } >"$bin"
    ;;
  xtask)
    [ "${FAKE_CARGO_FAIL:-}" != package ] || exit 102
    version=$(sed -n 's/^version=//p' "$CARGO_TARGET_DIR/$target/release/dockering")
    arch=${target%%-*}
    dist=$CARGO_TARGET_DIR/dist/$target
    mkdir -p "$dist"
    printf '#!/bin/sh\nif [ "$1" = --version ]; then echo "dockering %s"; fi\n' "${FAKE_APPIMAGE_VERSION:-$version}" >"$dist/Dockering-$arch.AppImage"
    chmod +x "$dist/Dockering-$arch.AppImage"
    ;;
  *) exit 97 ;;
esac
"""

FAKE_GIT = r"""#!/usr/bin/env bash
# test double for `git` as build-appimages.sh --source worktree uses it
set -eu
{
  for a in "$@"; do printf '%s\x1f' "$a"; done
  printf '\n'
} >>"$FAKE_GIT_LOG"
if [ "$1" = -C ]; then shift 2; fi
[ "$1" = worktree ] || exit 97
case "$2" in
  add)
    [ "$3" = --detach ] && [ "$5" = HEAD ] || exit 98
    mkdir -p "$4"
    cp "$FAKE_SOURCE_TOML" "$4/Cargo.toml"
    ;;
  remove)
    [ "$3" = --force ] || exit 99
    rm -rf "$4"
    ;;
  *) exit 96 ;;
esac
"""


@unittest.skipIf(WINDOWS or not BASH, "needs bash and a POSIX shell for the fakes")
class BuildScriptTests(unittest.TestCase):
    """REL-085, REL-086: build-appimages.sh against a fake cargo (and git): which features and version each AppImage gets."""

    TRIPLE = "x86_64-unknown-linux-gnu"

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="dockering-build-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.target = self.tmp / "target"
        self.out = self.tmp / "out"
        self.scratch = self.tmp / "scratch"  # TMPDIR of the script: what it leaves behind is visible
        self.scratch.mkdir()
        self.log = self.tmp / "cargo.log"
        self.git_log = self.tmp / "git.log"
        for name, body in (("cargo", FAKE_CARGO), ("git", FAKE_GIT)):
            path = self.bin / name
            path.write_text(body, encoding="utf-8", newline="\n")
            path.chmod(0o755)
        self.version = workspace_version()

    def run_script(self, *args, env=None, target=True):
        environment = os.environ.copy()
        for name in (
            "DOCKERING_E2E_VERSION",
            "CARGO_TARGET_DIR",
            "NO_STRIP",
            "FAKE_CARGO_FAIL",
            "FAKE_SEAM_EVERYWHERE",
            "FAKE_APPIMAGE_VERSION",
        ):
            environment.pop(name, None)
        environment.update(
            PATH=f"{self.bin}{os.pathsep}{environment['PATH']}",
            FAKE_LOG=str(self.log),
            FAKE_GIT_LOG=str(self.git_log),
            FAKE_WORKSPACE_VERSION=self.version,
            FAKE_SOURCE_TOML=str(ROOT / "Cargo.toml"),
            TMPDIR=str(self.scratch),
        )
        if target:
            environment["CARGO_TARGET_DIR"] = str(self.target)
        environment.update(env or {})
        return subprocess.run(
            [BASH, "--noprofile", "--norc", str(BUILD), *map(str, args)],
            cwd=self.tmp,
            env=environment,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )

    def cargo_calls(self):
        calls = []
        if not self.log.exists():
            return calls
        for line in read(self.log).splitlines():
            fields = line.split("\x1f")
            call = {"argv": fields[fields.index("argv") + 1 :]}
            for field in fields[: fields.index("argv")]:
                name, _, value = field.partition("=")
                call[name] = value
            calls.append(call)
        return calls

    def git_calls(self):
        return [line.split("\x1f")[:-1] for line in read(self.git_log).splitlines()] if self.git_log.exists() else []

    def build(self, *args, **kwargs):
        result = self.run_script(self.out, *args, **kwargs)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def appimage_version(self, name):
        done = subprocess.run([str(self.out / name), "--version"], capture_output=True, text=True, timeout=60)
        return done.stdout.strip()

    def test_rel_085_old_appimage_has_the_seam_and_the_build_time_version(self):
        self.build()
        old_build = self.cargo_calls()[0]
        self.assertEqual(
            old_build["argv"],
            ["build", "--release", "--locked", "-p", "dockering", "--target", self.TRIPLE, "--features", "updater,update-e2e"],
        )
        self.assertEqual(old_build["DOCKERING_E2E_VERSION"], "0.0.1")
        self.assertTrue(os.path.samefile(old_build["cwd"], ROOT), "`current` builds the tree the script is in")
        self.assertEqual(self.appimage_version("Dockering-old-x86_64.AppImage"), "dockering 0.0.1")

    def test_rel_086_new_appimage_is_built_like_a_release(self):
        # a stray variable of the caller must not turn the release-shaped build into a seam build
        self.build(env={"DOCKERING_E2E_VERSION": "7.7.7"})
        new_build = self.cargo_calls()[2]
        self.assertEqual(
            new_build["argv"],
            ["build", "--release", "--locked", "-p", "dockering", "--target", self.TRIPLE, "--features", "updater"],
        )
        self.assertNotIn("update-e2e", " ".join(new_build["argv"]))
        self.assertEqual(new_build["DOCKERING_E2E_VERSION"], "<unset>")
        self.assertEqual(self.appimage_version("Dockering-x86_64.AppImage"), f"dockering {self.version}")
        # the last binary cargo built is the release-shaped one: no seam, the updater, the real version
        binary = (self.target / self.TRIPLE / "release" / "dockering").read_text(encoding="utf-8")
        self.assertNotIn("dockering-update-e2e-seam-v1", binary)
        self.assertIn("releases/latest/download/dockering-update.json", binary)
        self.assertIn(f"version={self.version}\n", binary)

    def test_rel_085_each_appimage_is_packaged_the_way_the_release_workflow_packages(self):
        self.build()
        calls = self.cargo_calls()
        self.assertEqual([call["argv"][0] for call in calls], ["build", "xtask", "build", "xtask"], "old first, the release-shaped build last")
        for package in (calls[1], calls[3]):
            self.assertEqual(
                package["argv"],
                ["xtask", "package", "--target", self.TRIPLE, "--formats", "appimage", "--no-archive"],
                "no --features here: `cargo xtask package` packages the binary that is already built",
            )
            self.assertEqual(package["APPIMAGE_EXTRACT_AND_RUN"], "1", "linuxdeploy must work without FUSE")
            self.assertEqual(package["NO_STRIP"], "1", "linuxdeploy's old strip fails on libraries with .relr.dyn sections (Arch)")
        self.assertEqual({call["CARGO_TARGET_DIR"] for call in calls}, {str(self.target)})
        # the release workflow's own calls, which this script mirrors, are the ones the tests of REL-083/086 pin
        workflow = read(RELEASE_WORKFLOW)
        self.assertIn('cargo build --release --locked -p dockering --target "$TARGET" "${features[@]}"', workflow)
        self.assertIn("run: cargo xtask package --target ${{ matrix.target }}\n", workflow, "the release call has no extra flags")
        self.assertEqual(calls[1]["argv"][:4], ["xtask", "package", "--target", self.TRIPLE])
        self.assertNotIn("update-e2e", workflow)

    def test_rel_085_a_callers_no_strip_is_kept(self):
        self.build(env={"NO_STRIP": "0"})
        packages = [call for call in self.cargo_calls() if call["argv"][0] == "xtask"]
        self.assertEqual([call["NO_STRIP"] for call in packages], ["0", "0"])
        builds = [call for call in self.cargo_calls() if call["argv"][0] == "build"]
        self.assertEqual([call["NO_STRIP"] for call in builds], ["0", "0"], "it is the caller's variable, passed through")

    def test_rel_085_writes_exactly_the_two_appimages_and_reports_them(self):
        result = self.build()
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), ["Dockering-old-x86_64.AppImage", "Dockering-x86_64.AppImage"])
        for name in ("Dockering-old-x86_64.AppImage", "Dockering-x86_64.AppImage"):
            self.assertTrue(os.access(self.out / name, os.X_OK), f"{name} is executable")
            self.assertIn(name, result.stdout)
        self.assertEqual(sorted(p.name for p in self.scratch.iterdir()), [], "the scratch directory of the script is removed")

    def test_rel_085_the_seam_marker_must_be_in_the_old_binary_and_nowhere_else(self):
        result = self.run_script(self.out, env={"FAKE_SEAM_EVERYWHERE": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("the new binary has the wrong markers", result.stderr)
        self.assertFalse((self.out / "Dockering-x86_64.AppImage").exists(), "a release-shaped AppImage with the seam is never delivered")

    def test_rel_085_a_version_that_does_not_match_fails_the_build(self):
        result = self.run_script(self.out, env={"FAKE_APPIMAGE_VERSION": "9.9.9"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("printed 'dockering 9.9.9'", result.stderr)
        self.assertIn("expected 'dockering 0.0.1'", result.stderr)

    def test_rel_085_a_failed_build_or_package_step_fails_the_script(self):
        for step in ("build", "package"):
            with self.subTest(step=step):
                self.log.unlink(missing_ok=True)
                result = self.run_script(self.out, env={"FAKE_CARGO_FAIL": step})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.cargo_calls()), 1 if step == "build" else 2, "it stops at the first failure")
                self.assertEqual(
                    sorted(p.name for p in self.scratch.iterdir()), [], "the scratch directory of the script is removed on failure too"
                )

    def test_rel_085_aarch64_names_and_triple(self):
        self.build("--arch", "aarch64")
        triples = {call["argv"][call["argv"].index("--target") + 1] for call in self.cargo_calls()}
        self.assertEqual(triples, {"aarch64-unknown-linux-gnu"})
        self.assertEqual(
            sorted(p.name for p in self.out.iterdir()), ["Dockering-aarch64.AppImage", "Dockering-old-aarch64.AppImage"]
        )

    def test_rel_085_worktree_source_builds_head_in_a_temporary_worktree_and_removes_it(self):
        self.build("--source", "worktree", target=False)
        added, removed = self.git_calls()
        self.assertEqual(added[:2], ["-C", str(ROOT)])
        self.assertEqual(added[2:5], ["worktree", "add", "--detach"])
        worktree = Path(added[5])
        self.assertEqual(added[6:], ["HEAD"])
        self.assertTrue(worktree.is_relative_to(self.scratch), "inside the temporary directory, never in the repository")
        self.assertEqual(removed, ["-C", str(ROOT), "worktree", "remove", "--force", str(worktree)])
        self.assertFalse(worktree.exists())
        for call in self.cargo_calls():
            self.assertEqual(call["cwd"], str(worktree), "cargo runs in the worktree")
            target = Path(call["CARGO_TARGET_DIR"])
            self.assertEqual(target.name, "target")
            self.assertTrue(target.is_relative_to(self.scratch), "its own temporary target directory, not the repository's")
            self.assertFalse(target.is_relative_to(ROOT))
        self.assertEqual(sorted(p.name for p in self.scratch.iterdir()), [], "nothing is left behind")
        self.assertEqual(self.appimage_version("Dockering-x86_64.AppImage"), f"dockering {self.version}")

    def test_rel_085_worktree_source_honours_a_target_directory_and_cleans_up_after_a_failure(self):
        result = self.run_script(self.out, "--source=worktree", env={"FAKE_CARGO_FAIL": "build"})
        self.assertNotEqual(result.returncode, 0)
        added, removed = self.git_calls()
        self.assertEqual(removed[2:5], ["worktree", "remove", "--force"], "the worktree is removed when the build fails")
        self.assertFalse(Path(added[5]).exists())
        self.assertEqual({call["CARGO_TARGET_DIR"] for call in self.cargo_calls()}, {str(self.target)})
        self.assertEqual(sorted(p.name for p in self.scratch.iterdir()), [])

    def test_rel_085_current_source_never_touches_git(self):
        self.build("--source", "current")
        self.assertEqual(self.git_calls(), [])

    def test_rel_085_arguments_are_validated_before_anything_runs(self):
        (self.tmp / "afile").write_text("x", encoding="utf-8")
        for args in (
            [],
            [self.out, "second"],
            [self.out, "--source"],
            [self.out, "--source", "nowhere"],
            [self.out, "--source=HEAD"],
            [self.out, "--arch"],
            [self.out, "--arch", "riscv64"],
            [self.out, "--arch=x86_64;rm"],
            [self.out, "--features", "updater"],
            [self.out, "-x"],
            [self.tmp / "afile"],
        ):
            result = self.run_script(*args)
            self.assertEqual(result.returncode, 2, f"{args}: {result.stderr}")
            self.assertIn("usage: build-appimages.sh", result.stderr)
        self.assertEqual(self.cargo_calls(), [], "cargo is never called for a bad command line")
        self.assertEqual(self.git_calls(), [])
        self.assertFalse(self.out.exists())

    def test_rel_085_help_prints_the_usage_and_does_nothing(self):
        for flag in ("--help", "-h"):
            result = self.run_script(flag)
            self.assertEqual(result.returncode, 0)
            self.assertIn("usage: build-appimages.sh <out-dir> [--source current|worktree] [--arch x86_64|aarch64]", result.stdout)
        self.assertEqual(self.cargo_calls(), [])

    def test_rel_085_the_header_documents_the_three_ways_to_get_the_appimages(self):
        script = read(BUILD)
        header = script[: script.index("set -euo pipefail")]
        self.assertIn("THREE WAYS", header)
        for needle in ("gh run download", "Ubuntu 24.04 container", "linux-update-e2e.yml", "Arch", "GLIBC_x.y", "glibc"):
            self.assertIn(needle, header)
        self.assertIn("set -euo pipefail", script)


class ScriptFilesTests(unittest.TestCase):
    """REL-085: the files of scripts/linux-update are plain LF scripts that run where the test runs them."""

    def test_rel_085_the_scripts_are_plain_lf_files_with_a_shebang(self):
        files = [*UPDATE.glob("*.sh"), *UPDATE.glob("*.py")]
        self.assertIn(SERVE, files)
        self.assertIn(BUILD, files)
        for path in files:
            data = path.read_bytes()
            self.assertNotIn(b"\r", data, f"{path.name} has CRLF line endings")
            self.assertTrue(data.startswith(b"#!"), f"{path.name} has no shebang")

    @unittest.skipUnless(shutil.which("shellcheck"), "shellcheck is not installed (CI's lint job runs it)")
    def test_rel_085_shellcheck_is_clean(self):
        result = subprocess.run(["shellcheck", *map(str, UPDATE.glob("*.sh"))], capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_rel_085_ci_lints_the_scripts_with_shellcheck(self):
        ci = read(ROOT / ".github/workflows/ci.yml")
        self.assertRegex(ci, r"shellcheck [^\n]*scripts/linux-update/\*\.sh", "the lint job must shellcheck scripts/linux-update")


if __name__ == "__main__":
    unittest.main()
