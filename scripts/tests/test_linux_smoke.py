"""REL-070..077: the host side of the Linux smoke test, checked without Docker or a network.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.

Lowest layer that proves each requirement:
- fetch.sh, leg.sh and selftest.sh run under bash against a fake `gh` and a fake `docker` that come first on
  PATH, so the checksum and attestation gates, the argument validation and the container command line are
  real code paths with scripted answers;
- contract tests read the workflow, the container-side scripts, the README and the app sources and check that
  they agree with each other and with spec section 8 (docs/spec/features/distribution.md). A UI rename or a
  changed chord then fails the pull request that makes it, not the weekly run;
- that the harness fails when it should needs a real container: scripts/linux-smoke/selftest.sh, run by the
  `selftest` job of the workflow.
"""

import ast
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SMOKE = Path(os.environ.get("SMOKE_SCRIPT_DIR") or ROOT / "scripts/linux-smoke")
FETCH = SMOKE / "fetch.sh"
LEG = SMOKE / "leg.sh"
WORKFLOW = ROOT / ".github/workflows/linux-smoke.yml"
SPEC = ROOT / "docs/spec/features/distribution.md"
BASH = (
    str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
    if os.name == "nt"
    else shutil.which("bash")
)
SPEC_IDS = [f"REL-07{n}" for n in range(8)]


def shell_path(path):
    text = Path(path).as_posix()
    if os.name == "nt" and len(text) > 1 and text[1] == ":":
        return "/" + text[0].lower() + text[2:]
    return text


def read(path):
    return Path(path).read_text(encoding="utf-8")


def bash_has(*tools):
    if not BASH or not Path(BASH).is_file():
        return False
    probe = subprocess.run(
        [BASH, "--noprofile", "--norc", "-c", "command -v " + " ".join(tools) + " >/dev/null"],
        capture_output=True,
        timeout=60,
    )
    return probe.returncode == 0


TOOLS_OK = bash_has("sha256sum", "awk", "grep", "sed", "wc", "tr", "head", "tee", "cp")


def package_names(arch="x86_64"):
    return [f"Dockering-{arch}.deb", f"Dockering-{arch}.AppImage", f"Dockering-{arch}.tar.gz"]


def sums_line(name, data):
    return f"{hashlib.sha256(data).hexdigest()}  {name}\n"


FAKE_GH = r"""#!/usr/bin/env bash
# test double for `gh` as fetch.sh uses it: serves the files in $FAKE_ASSETS and logs every call
set -u
( IFS=$'\x1f'; printf '%s\n' "$*" ) >>"${FAKE_LOG:?}"
noise() { printf '%s\n' "${FAKE_NOISE:-gh: progress that must stay off stdout}"; }
case "${1:-} ${2:-}" in
"release view")
  printf '%s\n' "${FAKE_LATEST:-v0.2.0}"
  ;;
"release download")
  tag=${3:-}
  shift 3
  dir=""
  pats=()
  while [ $# -gt 0 ]; do
    case "$1" in
    --dir) dir=$2; shift 2 ;;
    --pattern) pats+=("$2"); shift 2 ;;
    *) shift ;;
    esac
  done
  noise
  if [ "${FAKE_DOWNLOAD_FAIL:-}" = 1 ] || { [ -n "${FAKE_TAG:-}" ] && [ "$tag" != "$FAKE_TAG" ]; }; then
    echo "release not found: $tag" >&2
    exit 1
  fi
  for p in "${pats[@]}"; do
    if [ -f "$FAKE_ASSETS/$p" ]; then cp "$FAKE_ASSETS/$p" "$dir/$p"; fi
  done
  ;;
"attestation verify")
  noise
  case ",${FAKE_ATTEST_FAIL:-}," in
  *",${3##*/},"*) echo "no attestations found" >&2; exit 1 ;;
  esac
  ;;
*)
  echo "fake gh: unexpected: $*" >&2
  exit 99
  ;;
esac
"""

FAKE_DOCKER = r"""#!/usr/bin/env bash
# test double for `docker` as leg.sh and selftest.sh use it: logs every call; pull, inspect and run are scripted by FAKE_* variables
set -u
( IFS=$'\x1f'; printf '%s\n' "$*" ) >>"${FAKE_LOG:?}"
case "${1:-}" in
pull)
  ref=${!#}
  case " ${FAKE_PULL_FAIL:-} " in
  *" $ref "*) echo "Error response from daemon: manifest for $ref not found" >&2; exit 1 ;;
  esac
  ;;
image)
  printf '%b' "${FAKE_DIGESTS:-}"
  ;;
run)
  echo "output of the container"
  exit "${FAKE_RUN_RC:-0}"
  ;;
*)
  echo "fake docker: unexpected: $*" >&2
  exit 99
  ;;
esac
"""


class Sandbox:
    """A temp directory with fake executables first on PATH and a log of every call they receive."""

    def __init__(self, test):
        self._temp = tempfile.TemporaryDirectory(prefix="dockering-smoke-test-")
        test.addCleanup(self._temp.cleanup)
        self.root = Path(self._temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "calls.log"

    def fake(self, name, body):
        path = self.bin / name
        path.write_text(body, encoding="utf-8", newline="\n")
        path.chmod(0o755)

    def run(self, script, *args, env=None, timeout=120):
        """Run `bash script args...` in the sandbox directory, as the workflow's shell would.

        The arguments travel in the environment and are unpacked by a one-line wrapper: a Windows command line
        splits an argument at a newline, and a newline in a tag is exactly what fetch.sh must refuse.
        """
        environment = os.environ.copy()
        for name in (
            "GITHUB_REPOSITORY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "ARCH",
            "SMOKE_LIBC_FLOOR",
            "SMOKE_PULL_DELAY",
            "SMOKE_NOKEYS",
            "SMOKE_ENTRY",
            "SMOKE_SELFTEST_ONLY",
        ):
            environment.pop(name, None)
        environment.update(
            MOCK_BIN=shell_path(self.bin),
            FAKE_LOG=shell_path(self.log),
            SMOKE_TEST_SCRIPT=shell_path(script),
            SMOKE_TEST_ARGC=str(len(args)),
        )
        environment.update({f"SMOKE_TEST_ARG_{i}": value for i, value in enumerate(args)})
        environment.update(env or {})
        wrapper = (
            'export PATH="$MOCK_BIN:$PATH"; a=(); i=0; '
            'while [ "$i" -lt "$SMOKE_TEST_ARGC" ]; do v=SMOKE_TEST_ARG_$i; a+=("${!v}"); i=$((i + 1)); done; '
            'exec bash --noprofile --norc "$SMOKE_TEST_SCRIPT" "${a[@]}"'
        )
        return subprocess.run(
            [BASH, "--noprofile", "--norc", "-c", wrapper],
            cwd=self.root,
            env=environment,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )

    def calls(self):
        if not self.log.exists():
            return []
        return [line.split("\x1f") for line in read(self.log).splitlines() if line]


class FakeRelease:
    """The files of a release as the fake gh serves them: packages for two architectures and a SHA256SUMS."""

    def __init__(self, directory):
        self.dir = directory
        names = package_names() + package_names("aarch64")
        names += ["Dockering-Setup-x64.exe", "Dockering-x86_64.dmg", "RELEASE_NOTES.md", "dockering-update.json"]
        self.files = {name: f"bytes of {name}\n".encode() for name in names}
        for name, data in self.files.items():
            (self.dir / name).write_bytes(data)
        self.write_sums(self.sums())

    def sums(self):
        return "".join(sums_line(name, data) for name, data in sorted(self.files.items()))

    def write_sums(self, text):
        (self.dir / "SHA256SUMS").write_text(text, encoding="utf-8", newline="\n")

    def edit_sums(self, edit):
        """Rewrite SHA256SUMS from the pristine text; an edit that changes nothing would let a test pass vacuously."""
        before = self.sums()
        after = edit(before)
        if after == before:
            raise AssertionError("the edit of SHA256SUMS changed nothing")
        self.write_sums(after)
        return after

    def line(self, name):
        return sums_line(name, self.files[name])


@unittest.skipUnless(TOOLS_OK, "bash with sha256sum is required")
class FetchScriptTests(unittest.TestCase):
    """REL-071: fetch.sh downloads the published packages and verifies them before anything installs them."""

    def setUp(self):
        self.sb = Sandbox(self)
        self.sb.fake("gh", FAKE_GH)
        self.assets = self.sb.root / "assets"
        self.assets.mkdir()
        self.release = FakeRelease(self.assets)
        self.dest = self.sb.root / "pkg"

    def fetch(self, tag="v0.2.0", *, repo="owner/name", **env):
        environment = {"FAKE_ASSETS": shell_path(self.assets)}
        if repo is not None:
            environment["GITHUB_REPOSITORY"] = repo
        environment.update(env)
        return self.sb.run(FETCH, tag, "pkg", env=environment)

    def assertRejected(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("::error::fetch.sh: " + message, result.stderr)
        self.assertEqual(result.stdout, "", "nothing may reach $GITHUB_OUTPUT when verification fails")

    def gh_calls(self, *words):
        return [call for call in self.sb.calls() if call[: len(words)] == list(words)]

    def test_rel_071_verifies_checksums_and_reports_only_the_tag_and_version(self):
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "tag=v0.2.0\nversion=0.2.0\n")
        for name in package_names():
            self.assertEqual((self.dest / name).read_bytes(), self.release.files[name])
        listed = read(self.dest / "packages.sha256").splitlines()
        self.assertEqual(sorted(line[66:] for line in listed), sorted(package_names()))
        self.assertEqual(len(listed), 3)

    def test_rel_071_downloads_the_three_packages_and_the_sums_of_the_tag(self):
        self.assertEqual(self.fetch().returncode, 0)
        (call,) = self.gh_calls("release", "download")
        self.assertEqual(call[2], "v0.2.0")
        self.assertEqual(call[call.index("--repo") + 1], "owner/name")
        patterns = {call[i + 1] for i, word in enumerate(call) if word == "--pattern"}
        self.assertEqual(patterns, set(package_names()) | {"SHA256SUMS"})

    def test_rel_071_attestations_are_pinned_to_the_workflow_the_tag_and_hosted_runners(self):
        self.assertEqual(self.fetch().returncode, 0)
        calls = self.gh_calls("attestation", "verify")
        self.assertEqual(sorted(Path(call[2]).name for call in calls), sorted(package_names()))
        for call in calls:
            self.assertEqual(
                call[3:],
                [
                    "--repo",
                    "owner/name",
                    "--signer-workflow",
                    "owner/name/.github/workflows/release.yml",
                    "--source-ref",
                    "refs/tags/v0.2.0",
                    "--deny-self-hosted-runners",
                ],
            )

    def test_rel_071_latest_resolves_the_newest_published_tag(self):
        result = self.fetch("latest", FAKE_LATEST="v0.3.1", FAKE_TAG="v0.3.1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "tag=v0.3.1\nversion=0.3.1\n")
        (view,) = self.gh_calls("release", "view")
        self.assertEqual(view[view.index("--repo") + 1], "owner/name")

    def test_rel_071_prerelease_tags_are_accepted(self):
        result = self.fetch("v0.3.0-rc.1", FAKE_TAG="v0.3.0-rc.1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "tag=v0.3.0-rc.1\nversion=0.3.0-rc.1\n")

    def test_rel_071_other_architectures_use_their_own_file_names(self):
        result = self.fetch(ARCH="aarch64")
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in package_names("aarch64"):
            self.assertTrue((self.dest / name).is_file(), name)
        self.assertFalse((self.dest / "Dockering-x86_64.deb").exists())

    def test_rel_071_noise_on_stdout_cannot_reach_github_output(self):
        result = self.fetch(FAKE_NOISE="version=9.9.9")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "tag=v0.2.0\nversion=0.2.0\n")
        self.assertIn("version=9.9.9", result.stderr)

    def test_rel_071_rejects_malformed_tags_before_calling_gh(self):
        for tag in [
            "",
            "0.2.0",
            "v0.2",
            "V0.2.0",
            "v0.2.0 ",
            "v0.2.0-",
            "v0.2.0;touch x",
            "v0.2.0$(id)",
            "../v0.2.0",
            "v0.2.0\nversion=9.9.9",
        ]:
            with self.subTest(tag=tag):
                self.sb.log.unlink(missing_ok=True)
                self.assertRejected(self.fetch(tag), "unexpected release tag")
                self.assertEqual(self.sb.calls(), [])

    def test_rel_071_rejects_a_malformed_latest_tag(self):
        for latest in ["not-a-tag", "v0.3.1\nv0.3.0", "v0.3.1;id"]:
            with self.subTest(latest=latest):
                self.assertRejected(self.fetch("latest", FAKE_LATEST=latest), "unexpected release tag")
                self.assertEqual(self.gh_calls("release", "download"), [])

    def test_rel_071_requires_a_valid_repository(self):
        for repo in [None, "", "owner", "a/b/c", "a b/c", "owner/name;id", "$(id)/x"]:
            with self.subTest(repo=repo):
                self.sb.log.unlink(missing_ok=True)
                result = self.fetch(repo=repo)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertIn("::error::fetch.sh: GITHUB_REPOSITORY", result.stderr)
                self.assertEqual(self.sb.calls(), [])

    def test_rel_071_rejects_an_architecture_with_unexpected_characters(self):
        for arch in ["x86_64;touch x", "x86 64", "../x", "x86-64", "$(id)"]:
            with self.subTest(arch=arch):
                self.sb.log.unlink(missing_ok=True)
                self.assertRejected(self.fetch(ARCH=arch), "ARCH has unexpected characters")
                self.assertEqual(self.sb.calls(), [])

    def test_rel_071_takes_exactly_two_arguments(self):
        result = self.sb.run(FETCH, "v0.2.0", env={"GITHUB_REPOSITORY": "owner/name"})
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: fetch.sh", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_rel_071_fails_when_a_package_cannot_be_downloaded(self):
        self.assertRejected(self.fetch(FAKE_DOWNLOAD_FAIL="1"), "cannot download the assets of v0.2.0")
        self.assertEqual(self.gh_calls("attestation", "verify"), [])

    def test_rel_071_fails_when_a_release_asset_is_missing(self):
        for name in ["Dockering-x86_64.AppImage", "SHA256SUMS"]:
            with self.subTest(name=name):
                kept = self.sb.root / (name + ".kept")
                shutil.move(self.assets / name, kept)
                self.addCleanup(lambda src=kept, dst=self.assets / name: src.exists() and shutil.move(src, dst))
                self.assertRejected(self.fetch(), f"v0.2.0 has no asset {name}")
                self.assertEqual(self.gh_calls("attestation", "verify"), [])
                shutil.move(kept, self.assets / name)

    def test_rel_071_requires_exactly_three_checksum_entries(self):
        deb = self.release.line("Dockering-x86_64.deb")
        for label, edit in {
            "a package line is missing": lambda text: text.replace(deb, ""),
            "a package line is listed twice": lambda text: text + deb,
            "a package line has a second, different digest": lambda text: text
            + sums_line("Dockering-x86_64.deb", b"other bytes"),
            "digests in upper case do not count": lambda text: re.sub(
                r"^([0-9a-f]{64})", lambda m: m.group(1).upper(), text, flags=re.M
            ),
        }.items():
            with self.subTest(label):
                self.release.edit_sums(edit)
                self.assertRejected(self.fetch(), "SHA256SUMS does not list exactly three x86_64 packages")

    def test_rel_071_requires_each_package_exactly_once(self):
        deb = self.release.line("Dockering-x86_64.deb")
        app = self.release.line("Dockering-x86_64.AppImage")
        tar = self.release.line("Dockering-x86_64.tar.gz")
        for label, edit, offender in [
            ("the .deb twice, the tarball not at all", lambda text: text.replace(tar, deb), "Dockering-x86_64.deb"),
            ("the tarball twice, the AppImage not at all", lambda text: text.replace(app, tar), "Dockering-x86_64.AppImage"),
        ]:
            with self.subTest(label):
                text = self.release.edit_sums(edit)
                listed = [line for line in text.splitlines() if line[66:] in package_names()]
                self.assertEqual(len(listed), 3, "three entries, so the count check alone cannot catch this")
                self.assertRejected(self.fetch(), f"SHA256SUMS does not list {offender} exactly once")

    def test_rel_071_look_alike_names_are_not_taken_for_packages(self):
        extra = "".join(
            sums_line(name, b"x")
            for name in ["Dockering-x86_64.debx", "Dockering-x86_64.tar.gz.sig", "xDockering-x86_64.AppImage"]
        )
        self.release.edit_sums(lambda text: text + extra)
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(read(self.dest / "packages.sha256").splitlines()), 3)

    def test_rel_071_binary_mode_entries_are_accepted(self):
        self.release.edit_sums(lambda text: re.sub(r"^([0-9a-f]{64})  ", r"\1 *", text, flags=re.M))
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(read(self.dest / "packages.sha256").splitlines()), 3)

    def test_rel_071_checksum_mismatch_fails_before_any_attestation_is_asked_for(self):
        original = (self.assets / "Dockering-x86_64.deb").read_bytes()
        (self.assets / "Dockering-x86_64.deb").write_bytes(original + b"tampered")
        self.assertNotEqual((self.assets / "Dockering-x86_64.deb").read_bytes(), original)
        self.assertRejected(self.fetch(), "checksum mismatch")
        self.assertEqual(self.gh_calls("attestation", "verify"), [])

    def test_rel_071_one_missing_attestation_fails_the_run(self):
        for name in package_names():
            with self.subTest(name=name):
                self.assertRejected(self.fetch(FAKE_ATTEST_FAIL=name), f"no valid build provenance for {name}")

    def test_rel_071_files_left_by_an_earlier_run_are_never_verified(self):
        self.dest.mkdir()
        stale = b"stale deb from an earlier run\n"
        (self.dest / "Dockering-x86_64.deb").write_bytes(stale)
        (self.dest / "packages.sha256").write_text(sums_line("Dockering-x86_64.deb", stale), encoding="utf-8")
        (self.assets / "Dockering-x86_64.deb").unlink()
        self.assertRejected(self.fetch(), "v0.2.0 has no asset Dockering-x86_64.deb")
        self.assertFalse((self.dest / "Dockering-x86_64.deb").exists())
        self.assertFalse((self.dest / "packages.sha256").exists())


@unittest.skipUnless(TOOLS_OK, "bash with sha256sum is required")
class LegScriptTests(unittest.TestCase):
    """REL-070, 071, 076, 077: leg.sh guards its inputs, pulls with fallbacks and runs one locked-down container."""

    IMAGE = "quay.io/fedora/fedora:latest"

    def setUp(self):
        self.sb = Sandbox(self)
        self.sb.fake("docker", FAKE_DOCKER)
        self.pkg = self.sb.root / "pkg"
        self.pkg.mkdir()
        sums = ""
        for name in package_names():
            data = f"bytes of {name}\n".encode()
            (self.pkg / name).write_bytes(data)
            sums += sums_line(name, data)
        (self.pkg / "packages.sha256").write_text(sums, encoding="utf-8", newline="\n")
        self.out = self.sb.root / "out"

    def leg(self, *, slug="fedora", kind="tar", version="0.2.0", images=(IMAGE,), pkg="pkg", env=None):
        return self.sb.run(LEG, slug, kind, pkg, "out", version, *images, env={"SMOKE_PULL_DELAY": "0", **(env or {})})

    def docker_calls(self, verb):
        return [call for call in self.sb.calls() if call[0] == verb]

    def assertRefused(self, result, status):
        self.assertEqual(result.returncode, status, result.stdout + result.stderr)
        self.assertEqual(self.sb.calls(), [], "docker must not be called")

    def test_rel_070_container_gets_read_only_inputs_and_no_secrets(self):
        env = {"GH_TOKEN": "ghp_secret", "GITHUB_TOKEN": "ghs_secret", "SMOKE_LIBC_FLOOR": "warn"}
        result = self.leg(env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        (argv,) = self.docker_calls("run")
        self.assertEqual(argv[:3], ["run", "--rm", "--name"])
        self.assertRegex(argv[3], r"^smoke-fedora-\d+$")
        flags = {word for word in argv[: argv.index(self.IMAGE)] if word.startswith("-")}
        self.assertEqual(flags, {"--rm", "--name", "--shm-size=512m", "-v", "-e"}, "no extra privilege or network flag")
        mounts = {}
        for index, word in enumerate(argv):
            if word == "-v":
                found = re.match(r"^(?P<host>.+?):(?P<target>/(?:pkg|scripts|out))(?P<ro>:ro)?$", argv[index + 1])
                self.assertIsNotNone(found, argv[index + 1])
                mounts[found["target"]] = (found["host"], bool(found["ro"]))
        self.assertEqual(sorted(mounts), ["/out", "/pkg", "/scripts"])
        self.assertTrue(mounts["/pkg"][1] and mounts["/scripts"][1], "packages and scripts are mounted read-only")
        self.assertFalse(mounts["/out"][1])
        self.assertTrue(os.path.samefile(mounts["/pkg"][0], self.pkg))
        self.assertTrue(os.path.samefile(mounts["/scripts"][0], SMOKE))
        self.assertTrue(os.path.samefile(mounts["/out"][0], self.out))
        environment = [argv[index + 1] for index, word in enumerate(argv) if word == "-e"]
        self.assertEqual(
            environment,
            ["EXPECTED_VERSION=0.2.0", "LEG=fedora", "SMOKE_LIBC_FLOOR", "SMOKE_SELFTEST_ONLY"],
            "the two settings without a value are forwarded from the environment of the caller, never put on the command line",
        )
        self.assertEqual(argv[-4:], [self.IMAGE, "bash", "/scripts/entry.sh", "tar"])
        self.assertNotIn("secret", " ".join(argv))

    def test_rel_071_rechecks_the_checksums_before_starting_docker(self):
        deb = self.pkg / "Dockering-x86_64.deb"
        deb.write_bytes(deb.read_bytes() + b"tampered")
        result = self.leg()
        self.assertRefused(result, 1)
        self.assertIn("do not match packages.sha256", result.stderr)

    def test_rel_071_a_leg_without_the_checksum_list_does_not_run(self):
        (self.pkg / "packages.sha256").unlink()
        result = self.leg()
        self.assertRefused(result, 1)
        self.assertIn("packages.sha256 is missing", result.stderr)

    def test_rel_071_a_leg_needs_the_package_directory(self):
        result = self.leg(pkg="no-such-dir")
        self.assertRefused(result, 2)
        self.assertIn("no such directory", result.stderr)

    def test_rel_077_pull_retries_three_times_then_falls_back_to_the_next_image(self):
        first, second = "mirror.gcr.io/library/ubuntu:24.04", "docker.io/library/ubuntu:24.04"
        result = self.leg(slug="ubuntu-24.04", kind="deb", images=(first, second), env={"FAKE_PULL_FAIL": first})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[-1] for call in self.docker_calls("pull")], [first, first, first, second])
        (argv,) = self.docker_calls("run")
        self.assertEqual(argv[-4:], [second, "bash", "/scripts/entry.sh", "deb"])
        self.assertIn(f"image={second}\n", read(self.out / "image.txt"))

    def test_rel_077_no_pullable_image_fails_without_running_anything(self):
        first, second = "mirror.gcr.io/library/ubuntu:24.04", "docker.io/library/ubuntu:24.04"
        result = self.leg(images=(first, second), env={"FAKE_PULL_FAIL": f"{first} {second}"})
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("none of the images could be pulled", result.stderr)
        self.assertEqual(len(self.docker_calls("pull")), 6)
        self.assertEqual(self.docker_calls("run"), [])

    def test_rel_076_the_image_and_the_digest_of_its_registry_are_recorded(self):
        digests = "docker.io/library/fedora@sha256:aaa\\nquay.io/fedora/fedora@sha256:bbb\\n"
        self.assertEqual(self.leg(env={"FAKE_DIGESTS": digests, "FAKE_RUN_RC": "1"}).returncode, 1)
        self.assertEqual(
            read(self.out / "image.txt"),
            f"image={self.IMAGE}\ndigest=quay.io/fedora/fedora@sha256:bbb\n",
            "recorded even when the container fails",
        )

    def test_rel_076_a_digest_of_another_name_is_better_than_none(self):
        image = "registry.example:5000/team/tool:1.2"
        self.leg(images=(image,), env={"FAKE_DIGESTS": "other/z@sha256:ccc\\n"})
        self.assertEqual(read(self.out / "image.txt"), f"image={image}\ndigest=other/z@sha256:ccc\n")

    def test_rel_076_a_digest_that_does_not_exist_is_reported_as_unknown(self):
        self.leg()
        self.assertEqual(read(self.out / "image.txt"), f"image={self.IMAGE}\ndigest=unknown\n")

    def test_rel_076_a_digest_matches_an_image_without_a_tag_or_with_a_digest_reference(self):
        for image, line in {
            "registry.example:5000/team/tool": "registry.example:5000/team/tool@sha256:aaa",
            "ubuntu@sha256:abc": "ubuntu@sha256:abc",
        }.items():
            with self.subTest(image=image):
                self.leg(images=(image,), env={"FAKE_DIGESTS": f"other/z@sha256:zzz\\n{line}\\n"})
                self.assertEqual(read(self.out / "image.txt"), f"image={image}\ndigest={line}\n")

    def test_rel_070_the_leg_exits_with_the_status_of_the_container_and_keeps_its_output(self):
        for status in ["0", "3", "7"]:
            with self.subTest(status=status):
                result = self.leg(env={"FAKE_RUN_RC": status})
                self.assertEqual(result.returncode, int(status), result.stderr)
                self.assertIn("output of the container", read(self.out / "leg.log"))

    def test_nfr_022_unsafe_arguments_are_refused_before_docker_is_called(self):
        defaults = dict(slug="fedora", kind="tar", version="0.2.0", images=(self.IMAGE,))
        for label, change in {
            "slug with a path": dict(slug="../x"),
            "slug starting with a dash": dict(slug="-x"),
            "slug with a space": dict(slug="a b"),
            "slug with a command": dict(slug="x$(id)"),
            "empty slug": dict(slug=""),
            "unknown package kind": dict(kind="rpm"),
            "kind with a command": dict(kind="deb;id"),
            "empty kind": dict(kind=""),
            "version with a v": dict(version="v0.2.0"),
            "two-part version": dict(version="0.2"),
            "version with a command": dict(version="0.2.0;id"),
            "version with a newline": dict(version="0.2.0\nX=1"),
            "image that is a docker flag": dict(images=("--privileged",)),
            "image with extra arguments": dict(images=("-v /:/host alpine",)),
            "image with a space": dict(images=("a b",)),
            "image with a command": dict(images=("$(id)",)),
            "second image with a command": dict(images=(self.IMAGE, "ubuntu; id")),
        }.items():
            with self.subTest(label):
                self.sb.log.unlink(missing_ok=True)
                self.assertRefused(self.leg(**{**defaults, **change}), 2)

    def test_rel_070_too_few_arguments_print_the_usage(self):
        result = self.sb.run(LEG, "fedora", "tar", "pkg", "out", "0.2.0")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: leg.sh", result.stderr)
        self.assertEqual(self.sb.calls(), [])

    def test_rel_074_the_container_script_can_be_chosen_among_the_scripts_of_the_directory(self):
        result = self.leg(slug="selftest", kind="deb", env={"SMOKE_ENTRY": "selftest-entry.sh"})
        self.assertEqual(result.returncode, 0, result.stderr)
        (argv,) = self.docker_calls("run")
        self.assertEqual(argv[-4:], [self.IMAGE, "bash", "/scripts/selftest-entry.sh", "deb"])

    def test_rel_074_selftest_names_travel_in_the_environment_not_on_the_command_line(self):
        self.leg(env={"SMOKE_SELFTEST_ONLY": "no_keys first_press_dropped"})
        (argv,) = self.docker_calls("run")
        self.assertNotIn("no_keys", " ".join(argv))
        self.assertIn("SMOKE_SELFTEST_ONLY", argv)

    def test_nfr_022_unsafe_or_unknown_container_scripts_are_refused_before_docker_is_called(self):
        for entry in [
            "../run.sh",
            "sub/entry.sh",
            "/etc/passwd",
            "run.sh;id",
            "Entry.sh",
            "entry",
            "entry.py",
            "-x.sh",
            "a b.sh",
            "$(id).sh",
            "",
        ]:
            with self.subTest(entry=entry):
                self.sb.log.unlink(missing_ok=True)
                result = self.leg(env={"SMOKE_ENTRY": entry or " "})
                self.assertRefused(result, 2)
                self.assertIn("unexpected entry script", result.stderr)
        self.sb.log.unlink(missing_ok=True)
        result = self.leg(env={"SMOKE_ENTRY": "no-such-script.sh"})
        self.assertRefused(result, 2)
        self.assertIn("no such entry script: no-such-script.sh", result.stderr)

    def test_rel_074_selftest_sh_runs_the_self_test_script_in_a_clean_ubuntu_container(self):
        result = self.sb.run(SMOKE / "selftest.sh", "pkg", "out", "0.2.0", env={"SMOKE_PULL_DELAY": "0"})
        self.assertEqual(result.returncode, 0, result.stderr)
        mirror, hub = "mirror.gcr.io/library/ubuntu:24.04", "docker.io/library/ubuntu:24.04"
        self.assertEqual([call[-1] for call in self.docker_calls("pull")], [mirror], "the registry mirror comes first")
        (argv,) = self.docker_calls("run")
        self.assertRegex(argv[3], r"^smoke-selftest-\d+$")
        self.assertEqual(argv[-4:], [mirror, "bash", "/scripts/selftest-entry.sh", "deb"])
        self.assertIn("EXPECTED_VERSION=0.2.0", argv)
        result = self.sb.run(
            SMOKE / "selftest.sh", "pkg", "out", "0.2.0", env={"SMOKE_PULL_DELAY": "0", "FAKE_PULL_FAIL": mirror}
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.docker_calls("run")[-1][-4:][0], hub, "Docker Hub is the fallback")

    def test_rel_074_selftest_sh_passes_its_images_and_checks_its_arguments(self):
        result = self.sb.run(SMOKE / "selftest.sh", "pkg", "out", "0.2.0", self.IMAGE, env={"SMOKE_PULL_DELAY": "0"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.docker_calls("run")[-1][-4], self.IMAGE)
        self.sb.log.unlink(missing_ok=True)
        for args in [("pkg", "out"), ("pkg",), ()]:
            with self.subTest(args=args):
                result = self.sb.run(SMOKE / "selftest.sh", *args)
                self.assertRefused(result, 2)
                self.assertIn("usage: selftest.sh", result.stderr)
        result = self.sb.run(SMOKE / "selftest.sh", "pkg", "out", "v0.2.0")
        self.assertRefused(result, 2)
        self.assertIn("unexpected version", result.stderr)


def block(text, header, indent=0):
    """The lines under `header:` (a key at `indent` spaces), up to the next line at that indent or less."""
    lines = text.split("\n")
    start = next(
        (i for i, line in enumerate(lines) if re.match(r"^" + " " * indent + re.escape(header) + r":(\s|$)", line)),
        None,
    )
    if start is None:
        raise AssertionError(f"no '{header}:' at indent {indent}")
    body = []
    for line in lines[start + 1 :]:
        if line.strip() and len(line) - len(line.lstrip(" ")) <= indent:
            break
        body.append(line)
    return "\n".join(body)


def workflow_jobs():
    jobs = block(read(WORKFLOW), "jobs")
    return {name: block(jobs, name, 2) for name in re.findall(r"^  ([a-z][a-z0-9_-]*):\s*$", jobs, flags=re.M)}


def steps_of(job):
    """The steps of a job as text chunks, one per `      - ` entry."""
    chunks = []
    for line in block(job, "steps", 4).split("\n"):
        if line.startswith("      - "):
            chunks.append([line])
        elif chunks:
            chunks[-1].append(line)
    return ["\n".join(chunk).rstrip() for chunk in chunks]


def run_scripts_of(job):
    """The bodies of the `run:` keys of a job's steps."""
    bodies = []
    for step in steps_of(job):
        found = re.search(r"^( +)(?:- )?run: (.*)$", step, flags=re.M)
        if not found:
            continue
        if found.group(2).strip() in ("|", ">", "|-", ">-"):
            tail = step[found.end() :].split("\n")[1:]
            bodies.append("\n".join(line for line in tail if line.strip()))
        else:
            bodies.append(found.group(2))
    return bodies


def matrix_legs():
    smoke = workflow_jobs()["smoke"]
    return re.findall(r'- name: (.+)\n\s+slug: (\S+)\n\s+package: (\S+)\n\s+images: "([^"]+)"', smoke)


def rust_strings():
    text = read(ROOT / "crates/dockering/src/strings.rs")
    found = re.findall(r'^pub const ([A-Z0-9_]+): &str = "((?:[^"\\]|\\.)*)";', text, flags=re.M)

    def unescape(value):
        value = re.sub(r"\\u\{([0-9a-fA-F]+)\}", lambda m: chr(int(m.group(1), 16)), value)
        return value.replace('\\"', '"').replace("\\\\", "\\")

    return {name: unescape(value) for name, value in found}


def atspi_tables():
    """PAGES (page -> [(role, accessible name)]) and ORDER of atspi.py, read without importing gi."""
    tree = ast.parse(read(SMOKE / "atspi.py"), filename="atspi.py")
    pages = order = None
    for node in tree.body:
        if not (isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)):
            continue
        if node.targets[0].id == "PAGES":
            pages = {
                key.value: [(entry.elts[0].attr, entry.elts[1].value) for entry in value.elts]
                for key, value in zip(node.value.keys, node.value.values)
            }
        elif node.targets[0].id == "ORDER":
            order = [element.value for element in node.value.elts]
    return pages, order


def chords_of_run_sh():
    run = read(SMOKE / "run.sh")
    loop = re.search(r'for entry in ((?:"\S+ \w+" ?)+); do', run)
    return re.findall(r'"(\S+) (\w+)"', loop.group(1))


class ContractCase(unittest.TestCase):
    """Assertions on large texts that report the pattern and a label, never the whole text."""

    def assertHas(self, needle, text, what="the text"):
        if needle not in text:
            self.fail(f"{what} does not contain {needle!r}")

    def assertLacks(self, needle, text, what="the text"):
        if needle in text:
            self.fail(f"{what} contains {needle!r}")

    def assertFound(self, pattern, text, what="the text", flags=0):
        found = re.search(pattern, text, flags)
        if found is None:
            self.fail(f"{what} has no match for {pattern!r}")
        return found


class WorkflowContractTests(ContractCase):
    """REL-070, 071, 076, 077: what spec section 8 says about the workflow, read from the workflow file."""

    def setUp(self):
        self.text = read(WORKFLOW)
        self.jobs = workflow_jobs()

    def test_rel_070_triggers_are_release_weekly_manual_and_pull_requests_touching_it(self):
        on = block(self.text, "on")
        self.assertFound(r"release:\n\s+types: \[published\]", on, "on:")
        self.assertFound(r'schedule:\n\s+- cron: "\d+ \d+ \* \* [0-6]"', on, "on: (weekly: one weekday, any day of the month)")
        self.assertFound(
            r"workflow_dispatch:\n\s+inputs:\n\s+tag:\n(?:\s+.*\n)*?\s+required: false\n\s+type: string", on, "on: dispatch input"
        )
        paths = block(on, "pull_request", 2)
        self.assertHas('".github/workflows/linux-smoke.yml"', paths, "pull_request paths")
        self.assertHas('"scripts/linux-smoke/**"', paths, "pull_request paths")
        self.assertLacks("pull_request_target", self.text, "the workflow")

    def test_rel_070_the_legs_are_independent_and_match_the_spec(self):
        self.assertEqual(
            [leg[:3] for leg in matrix_legs()],
            [
                ("Ubuntu 24.04", "ubuntu-24.04", "deb"),
                ("Ubuntu 26.04", "ubuntu-26.04", "deb"),
                ("Fedora", "fedora", "tar"),
                ("Arch Linux", "arch", "tar"),
            ],
        )
        smoke = self.jobs["smoke"]
        self.assertFound(r"strategy:\n\s+fail-fast: false", smoke, "job smoke")
        self.assertFound(r"needs: fetch\b", smoke, "job smoke")

    def test_rel_070_permissions_are_read_only_except_the_alert_that_opens_an_issue(self):
        def granted(text, indent):
            return sorted(line.strip() for line in block(text, "permissions", indent).split("\n") if line.strip())

        self.assertEqual(granted(self.text, 0), ["contents: read"])
        self.assertEqual(granted(self.jobs["fetch"], 4), ["attestations: read", "contents: read"])
        self.assertEqual(granted(self.jobs["alert"], 4), ["contents: read", "issues: write"])
        self.assertEqual(self.text.count(": write"), 1, "the alert's issues: write is the only write permission")
        for name in self.jobs.keys() - {"fetch", "alert"}:
            self.assertLacks("permissions:", self.jobs[name], f"job {name}, which inherits contents: read,")

    def test_rel_070_no_secret_or_token_reaches_the_legs(self):
        self.assertLacks("secrets.", self.text, "the workflow")
        for name in self.jobs.keys() - {"fetch", "alert"}:
            for token in ("GH_TOKEN", "github.token", "GITHUB_TOKEN"):
                self.assertLacks(token, self.jobs[name], f"job {name}")
        for name, job in self.jobs.items():
            for step in steps_of(job):
                if "uses: actions/checkout@" in step:
                    self.assertHas("persist-credentials: false", step, f"the checkout of job {name}")

    def test_rel_070_no_expression_is_interpolated_into_a_shell_script(self):
        for name, job in self.jobs.items():
            scripts = run_scripts_of(job)
            self.assertTrue(scripts, f"job {name} has run steps")
            for script in scripts:
                self.assertLacks("${{", script, f"a run script of job {name} (use env: and a shell variable)")

    def test_rel_071_the_fetch_job_verifies_before_the_legs_get_the_packages(self):
        fetch = self.jobs["fetch"]
        self.assertHas('fetch.sh "$REQUESTED_TAG" pkg >> "$GITHUB_OUTPUT"', fetch, "job fetch")
        self.assertHas("REQUESTED_TAG: ${{ github.event.release.tag_name || inputs.tag || 'latest' }}", fetch, "job fetch")
        for path in [*(f"pkg/{name}" for name in package_names()), "pkg/packages.sha256"]:
            self.assertHas(path, fetch, "the packages artifact of job fetch")
        self.assertFound(r"name: packages\b", fetch, "job fetch")
        smoke = self.jobs["smoke"]
        self.assertFound(r"actions/download-artifact@\S+\n\s+with:\n\s+name: packages\n\s+path: pkg", smoke, "job smoke")
        self.assertHas("scripts/linux-smoke/leg.sh", smoke, "job smoke")

    def test_rel_072_the_legs_pass_the_package_kind_of_the_matrix_to_the_leg_script(self):
        smoke = self.jobs["smoke"]
        self.assertHas("PACKAGE: ${{ matrix.package }}", smoke, "job smoke")
        self.assertFound(r'leg\.sh "\$SLUG" "\$PACKAGE" pkg out "\$VERSION"', smoke, "job smoke")

    def test_rel_073_the_floor_is_a_warning_only_through_the_workflow_setting(self):
        settings = re.findall(r"SMOKE_LIBC_FLOOR: (\S+)", self.text)
        self.assertIn(settings, ([], ["warn"]), "warn, or removed once packaging declares the floor (plan task 5)")
        self.assertHas("SMOKE_LIBC_FLOOR", read(LEG), "leg.sh")

    def test_rel_076_evidence_is_uploaded_even_when_a_leg_fails_and_kept_14_days(self):
        smoke = self.jobs["smoke"]
        uploads = [step for step in steps_of(smoke) if "uses: actions/upload-artifact@" in step]
        self.assertEqual(len(uploads), 2, "contact sheet and evidence")
        for step in uploads:
            self.assertFound(r"if: always\(\)", step, "an upload step of job smoke")
            self.assertHas("retention-days: 14", step, "an upload step of job smoke")
        self.assertHas("name: contact-${{ matrix.slug }}.png", smoke, "job smoke")
        self.assertHas("name: smoke-${{ matrix.slug }}", smoke, "job smoke")
        self.assertHas("path: out/", smoke, "job smoke")
        self.assertHas("contact-$LEG.png", read(SMOKE / "entry.sh"), "entry.sh")

    def test_rel_076_the_summary_shows_the_image_digest_and_the_scenario_counts(self):
        (summary,) = [step for step in steps_of(self.jobs["smoke"]) if "GITHUB_STEP_SUMMARY" in step]
        self.assertFound(r"if: always\(\)", summary, "the summary step")
        self.assertHas("digest=", summary, "the summary step")
        self.assertHas("results.txt", summary, "the summary step")

    def test_rel_077_images_come_from_registries_other_than_docker_hub_first(self):
        for name, slug, _, images in matrix_legs():
            with self.subTest(slug):
                refs = images.split()
                self.assertGreaterEqual(len(refs), 2, "a fallback registry exists")
                host = refs[0].split("/")[0]
                self.assertTrue("." in host or ":" in host, f"{refs[0]} names its registry")
                self.assertNotIn(host, {"docker.io", "index.docker.io", "registry-1.docker.io"})
                if slug in ("fedora", "arch"):
                    self.assertTrue(refs[0].endswith(":latest"), "rolling distributions run on latest on purpose")
                else:
                    self.assertRegex(refs[0], r":\d+\.\d+$", "Ubuntu is pinned to a release")

    def test_rel_077_a_failed_scheduled_run_opens_or_updates_one_issue(self):
        alert = self.jobs["alert"]
        self.assertFound(r"needs: \[fetch, smoke\]", alert, "job alert")
        self.assertFound(r"if: failure\(\) && github\.event_name == 'schedule'", alert, "job alert")
        listed = alert.index("gh issue list")
        self.assertLess(listed, alert.index("gh issue comment"))
        self.assertLess(listed, alert.index("gh issue create"))
        self.assertEqual(alert.count("gh issue create"), 1)

    def test_rel_070_the_release_docs_list_the_legs_of_the_workflow(self):
        docs = read(ROOT / "docs/release.md")
        rows = re.findall(r"^\| `([a-z0-9.-]+)` \| `(deb|tar)` \| `([^`]+)` \|$", docs, flags=re.M)
        self.assertEqual(rows, [(slug, package, images.split()[0]) for _, slug, package, images in matrix_legs()])
        for artifact in ("contact-<slug>.png", "smoke-<slug>"):
            self.assertIn(artifact, docs)


class ScriptContractTests(ContractCase):
    """REL-072..075: the container-side scripts agree with the spec, the README and the app they test."""

    def setUp(self):
        self.run_sh = read(SMOKE / "run.sh")
        self.entry = read(SMOKE / "entry.sh")
        self.strings = rust_strings()
        self.spec = read(SPEC)

    def spec_line(self, requirement):
        return next(line for line in self.spec.split("\n") if line.startswith(f"| {requirement} |"))

    def test_rel_070_077_the_spec_defines_exactly_the_requirements_these_tests_cover(self):
        found = re.findall(r"^\| (REL-07\d) \|", self.spec, flags=re.M)
        self.assertEqual(found, SPEC_IDS)
        names = [
            name
            for case in (FetchScriptTests, LegScriptTests, WorkflowContractTests, ScriptContractTests)
            for name in dir(case)
            if name.startswith("test_")
        ]
        for requirement in SPEC_IDS:
            prefix = "test_" + requirement.lower().replace("-", "_") + "_"
            self.assertTrue(any(name.startswith(prefix) for name in names), f"no test is named after {requirement}")

    def test_rel_072_the_deb_is_installed_with_declared_dependencies_only_before_the_harness(self):
        deb = '"$PKG_DIR/Dockering-$(uname -m).deb"'
        install = next(line for line in self.entry.split("\n") if deb in line)
        self.assertHas("apt-get install", install, "the line that installs the .deb")
        self.assertHas("--no-install-recommends", install, "the line that installs the .deb")
        self.assertLess(self.entry.index(deb), self.entry.index("xvfb"), "the harness is installed after the package")

    def test_rel_072_every_leg_runs_the_package_scenarios_and_then_the_appimage(self):
        runs = re.findall(r"^run (\S+) (\S+) (\S+)$", self.entry, flags=re.M)
        self.assertEqual(
            runs,
            [('"$KIND"', "demo", "x11"), ('"$KIND"', "demo", "wayland"), ('"$KIND"', "upgrade", "x11"), ("appimage", "demo", "x11")],
        )
        self.assertHas("APPIMAGE_EXTRACT_AND_RUN=1", self.run_sh, "run.sh")

    def test_rel_072_the_package_checks_of_spec_section_8_are_recorded(self):
        for check in ("version", "libraries", "install_files", "desktop_entry", "libc_floor"):
            self.assertFound(rf"check {check} ", self.run_sh, "run.sh")
        self.assertHas("desktop-file-validate", self.run_sh, "run.sh")
        self.assertHas("ldd ", self.run_sh, "run.sh")
        self.assertHas('"dockering $EXPECTED_VERSION"', self.run_sh, "run.sh")

    def test_rel_072_the_readme_runtime_packages_are_the_ones_the_legs_install(self):
        readme = read(ROOT / "README.md")
        fedora = set(self.assertFound(r"sudo dnf install ([^\n]+)", readme, "README.md").group(1).split())
        arch = set(self.assertFound(r"sudo pacman -S ([^\n]+)", readme, "README.md").group(1).split())
        self.assertGreater(len(fedora), 5)
        self.assertGreater(len(arch), 5)

        def words(text):
            return set(re.findall(r"[A-Za-z0-9][A-Za-z0-9+._-]*", text))

        entry_fedora = self.entry[self.entry.index("fedora)") : self.entry.index("arch)")]
        entry_arch = self.entry[self.entry.index("arch)") : self.entry.index("*) die")]
        self.assertEqual(sorted(fedora - words(entry_fedora)), [], "README names Fedora packages no leg installs")
        self.assertEqual(sorted(arch - words(entry_arch)), [], "README names Arch packages no leg installs")

    def test_rel_073_a_missing_floor_fails_unless_the_workflow_says_warn(self):
        self.assertFound(r'\[ "\$\{SMOKE_LIBC_FLOOR:-fail\}" = warn \] && st=INFO \|\| st=FAIL', self.run_sh, "run.sh")
        self.assertHas('dpkg --compare-versions "$floor" ge "$need"', self.run_sh, "run.sh")
        self.assertHas("objdump -T", self.run_sh, "run.sh")
        self.assertHas("-e SMOKE_LIBC_FLOOR", read(LEG), "leg.sh (the setting must reach the container)")

    def test_rel_074_the_chords_run_in_the_order_of_the_spec_and_are_bound_in_the_app(self):
        expected = [
            ("ctrl+2", "Images"),
            ("ctrl+3", "Volumes"),
            ("ctrl+4", "Networks"),
            ("ctrl+comma", "Settings"),
            ("ctrl+1", "Containers"),
        ]
        self.assertEqual(chords_of_run_sh(), expected)
        spec = " ".join(self.spec_line("REL-074").split())
        self.assertHas(
            "`Mod+2`, `Mod+3`, `Mod+4`, `Mod+,` and `Mod+1` MUST select Images, Volumes, Networks, Settings and Containers",
            spec,
            "REL-074",
        )
        keymap = dict(re.findall(r'"secondary-([^"]+)",\s*(\w+),', read(ROOT / "crates/dockering/src/keymap.rs")))
        action = {
            "Images": "GoImages",
            "Volumes": "GoVolumes",
            "Networks": "GoNetworks",
            "Containers": "GoContainers",
            "Settings": "OpenSettings",
        }
        for chord, page in expected:
            key = chord.removeprefix("ctrl+").replace("comma", ",")
            self.assertEqual(keymap.get(key), action[page], f"keymap.rs binds secondary-{key} to {keymap.get(key)}")
        self.assertEqual(keymap.get("shift-p"), "CommandPalette")
        self.assertHas("key ctrl+shift+p", self.run_sh, "run.sh")

    def test_rel_074_the_accessible_names_are_the_ones_the_app_defines(self):
        s = self.strings
        pages, order = atspi_tables()
        self.assertIsNotNone(pages, "atspi.py has no PAGES table")
        self.assertEqual(order, ["Images", "Volumes", "Networks", "Containers"])
        self.assertEqual(sorted(pages), sorted(order))
        self.assertEqual({s["PAGE_CONTAINERS"], s["PAGE_IMAGES"], s["PAGE_VOLUMES"], s["PAGE_NETWORKS"]}, set(pages))
        group_by = f"{s['GROUP_BY']}: {s['GROUP_BY_COMPOSE']}"
        known = {"All": s["FILTER_ALL"], "Pull": s["PULL"], "Create": s["CREATE"], "Search\u2026": s["SEARCH"], group_by: group_by}
        for page, controls in pages.items():
            for role, name in controls:
                self.assertIn(role, ("PAGE_TAB", "PUSH_BUTTON", "ENTRY"), f"{page}: unknown role {role}")
                self.assertIn(name, known, f"atspi.py expects {name!r} on {page}; trace it to strings.rs here")
                self.assertEqual(known[name], name)
        self.assertHas(
            'format!("{}: {group_label}", s::GROUP_BY)',
            read(ROOT / "crates/dockering/src/pages/containers/view.rs"),
            "the Containers view (the group-by button label)",
        )
        atspi = read(SMOKE / "atspi.py")
        self.assertEqual(set(re.findall(r'sidebar_item\("([^"]+)"\)', atspi)), {s["PAGE_CONTAINERS"], s["SEC_GENERAL"]})
        self.assertHas('startswith("Back to ")', atspi, "atspi.py")
        back_to = self.assertFound(
            r'pub fn back_to\(page: &str\) -> String \{\s*format!\("([^"{]*)\{page\}"\)',
            read(ROOT / "crates/dockering/src/strings.rs"),
            "strings.rs",
        )
        self.assertEqual(back_to.group(1), "Back to ")

    def test_rel_074_the_window_identity_is_the_application_id_of_the_app(self):
        app_id = self.assertFound(r'pub const APP_ID: &str = "([^"]+)";', read(ROOT / "crates/dockering/src/app.rs"), "app.rs").group(1)
        self.assertEqual(app_id, "dev.dockering.Dockering")
        self.assertGreaterEqual(self.run_sh.count(app_id), 3, "WM_CLASS, app_id and the Wayland window lookup")
        self.assertHas("xprop", self.run_sh, "run.sh")

    def test_rel_074_quit_goes_through_the_palette_entry_the_app_has(self):
        self.assertIn("quit", self.strings["CMD_QUIT"].lower())
        self.assertFound(
            r"key ctrl\+shift\+p\n\s+sleep [\d.]+\n\s+typ quit[^\n]*\n\s+sleep [\d.]+\n(?:[^\n]*\n)?\s+key Return",
            self.run_sh,
            "run.sh (the X11 quit sequence)",
        )

    def test_rel_074_the_waits_are_the_40_and_10_seconds_of_the_spec(self):
        self.assertEqual(re.findall(r"within (\d+) s", self.spec_line("REL-074")), ["40", "10"])
        window = self.assertFound(r"for _ in \$\(seq 1 (\d+)\); do[^\n]*\n\s+wid=\$\(find_window\)", self.run_sh, "run.sh (window wait)")
        quit_ = self.assertFound(r"for _ in \$\(seq 1 (\d+)\); do kill -0 \$APP[^\n]*sleep 0\.25; done", self.run_sh, "run.sh (quit wait)")
        self.assertEqual(int(window.group(1)) * 0.25, 40)
        self.assertEqual(int(quit_.group(1)) * 0.25, 10)
        self.assertHas("no window after 40 s", self.run_sh, "run.sh")
        self.assertHas("still running 10 s after", self.run_sh, "run.sh")

    def test_rel_074_a_demo_launch_checks_paint_stability_state_and_exit_status(self):
        self.assertHas("ARGS=(--demo)", self.run_sh, "run.sh")
        for check in ("window", "window_identity", "first_paint", "first_frame_stable", "alive", "clean_quit", "state_saved"):
            self.assertFound(rf"check {check} ", self.run_sh, "run.sh")
        self.assertFound(r'saved" = "\$EXPECTED_VERSION"', self.run_sh, "run.sh")
        self.assertHas('"$SCENARIO" = demo', self.run_sh, "run.sh")

    def test_rel_074_the_self_test_switches_discard_only_the_navigation_keys(self):
        nav = self.assertFound(r"\nnav_key\(\) \{[^\n]*\n(?:  [^\n]*\n)+\}\n", self.run_sh, "run.sh (nav_key)").group(0)
        self.assertHas('case "${SMOKE_NOKEYS:-}" in', nav, "nav_key")
        self.assertFound(r"\n  1\) return 0 ;;\n", nav, "nav_key (SMOKE_NOKEYS=1 discards every chord)")
        self.assertFound(r'\n  first\) \[ "\$\{attempt:-1\}" -gt 1 \] \|\| return 0 ;;\n', nav, "nav_key (=first discards the first press)")
        self.assertTrue(nav.rstrip().endswith('key "$@"\n}'), "every other press goes to key")
        self.assertEqual(len(re.findall(r'^\s+nav_key "', self.run_sh, flags=re.M)), 1, "only the chord loop calls nav_key")
        self.assertFound(r'for attempt in 1 2; do\n\s+nav_key "\$chord"', self.run_sh, "run.sh (the retry loop sets attempt)")

    def test_rel_075_the_upgrade_scenario_launches_the_real_mode_on_an_older_profile(self):
        start = self.run_sh.index("upgrade)")
        upgrade = self.run_sh[start : self.run_sh.index(";;", start)]
        self.assertHas('{"updates":{"last_run_version":"0.0.1"}}', upgrade, "the upgrade scenario")
        self.assertLacks("--demo", upgrade, "the upgrade scenario")
        self.assertHas("ARGS=()", self.run_sh, "run.sh")

    def test_rel_075_every_launch_checks_for_crash_files_panics_and_unexpected_log_lines(self):
        self.assertFound(r"check no_crash PASS\b", self.run_sh, "run.sh")
        self.assertHas("panicked", self.run_sh, "run.sh")
        self.assertHas("crash-*.txt", self.run_sh, "run.sh")
        self.assertFound(r"# known headless noise[^\n]*\n\s*bad=", self.run_sh, "run.sh")
        self.assertFound(r"check log_clean FAIL", self.run_sh, "run.sh")

    def test_rel_076_every_scenario_keeps_its_results_screenshots_and_logs(self):
        for kept in ("results.txt", "01-first-frame.png", "dockering.log", "run.log", "app.stderr", "crash"):
            self.assertHas(kept, self.run_sh, "run.sh")
        self.assertHas("/out", self.entry, "entry.sh")
        self.assertHas("image.txt", read(LEG), "leg.sh")
        self.assertHas("leg.log", read(LEG), "leg.sh")

    def test_rel_070_077_the_scripts_are_plain_lf_files_with_a_shebang(self):
        for path in [*SMOKE.glob("*.sh"), SMOKE / "atspi.py"]:
            data = path.read_bytes()
            self.assertNotIn(b"\r", data, f"{path.name} has CRLF line endings, which bash in the container rejects")
            self.assertTrue(data.startswith(b"#!"), f"{path.name} has no shebang")


if __name__ == "__main__":
    unittest.main()
