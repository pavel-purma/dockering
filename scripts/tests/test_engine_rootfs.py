"""REL-088/REL-089: the engine root-filesystem pipeline (tools/engine-rootfs, engine-rootfs.yml, the pin check wiring).

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'. Workflows are read as text (no YAML
parser), like the other workflow tests. pins.py is run as the workflow runs it: `python -B -I pins.py ...`.
"""

import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/engine-rootfs"
PINS_PY = TOOLS / "pins.py"
WORKFLOW = ROOT / ".github/workflows/engine-rootfs.yml"
CI = ROOT / ".github/workflows/ci.yml"
RELEASE = ROOT / ".github/workflows/release.yml"
IMAGES = ("alpine", "ubuntu")
ARCHES = ("x86_64", "aarch64")


def text(path):
    return path.read_text(encoding="utf-8")


def pins(*args, check=True):
    return subprocess.run(
        [sys.executable, "-B", "-I", str(PINS_PY), *map(str, args)], capture_output=True, text=True, check=check
    )


def job_block(workflow, job):
    """The text of one job (two-space indent) of a workflow."""
    jobs = workflow[workflow.index("\njobs:\n") :]
    match = re.search(rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [a-z][a-z0-9_-]*:\n|\Z)", jobs)
    assert match, f"no job {job}"
    return match.group(1)


class PinsFileTests(unittest.TestCase):
    def test_rel_088_pins_get_reads_the_single_source_of_truth(self):
        self.assertEqual(pins("get", "release.tag").stdout.strip(), "rootfs-1")
        self.assertEqual(pins("get", "build.source_date_epoch").stdout.strip(), "1792540800")
        self.assertEqual(
            pins("arch-get", "alpine", "x86_64", "base_sha256").stdout.strip(),
            "c5ca053cfe1d85c5b96dff8b9bc57045f7f184a30ffb6b65776409ca90388677",
        )
        specs = pins("apt-specs", "ubuntu").stdout.split()
        self.assertEqual(
            sorted(s.split("=")[0] for s in specs), ["containerd.io", "docker-ce", "docker-ce-cli"], "ubuntu pins exact versions"
        )
        self.assertTrue(all("=" in s and s.split("=")[1] for s in specs))
        self.assertNotEqual(pins("get", "no.such.key", check=False).returncode, 0)

    def test_rel_088_every_cell_pins_its_base_by_sha256(self):
        for image in IMAGES:
            for arch in ARCHES:
                with self.subTest(image=image, arch=arch):
                    sha = pins("arch-get", image, arch, "base_sha256").stdout.strip()
                    self.assertRegex(sha, r"^[0-9a-f]{64}$")
                    self.assertTrue(pins("arch-get", image, arch, "base_url").stdout.startswith("https://"))
                    self.assertGreater(int(pins("arch-get", image, arch, "base_size").stdout), 0)


class FakeBuild:
    """A directory shaped like the output of build.sh for all four cells."""

    def __init__(self, directory, tag="rootfs-1"):
        self.dir = Path(directory)
        self.tag = tag
        for image in IMAGES:
            for arch in ARCHES:
                name = f"dockering-engine-{image}-{arch}"
                data = f"{image}-{arch}-payload".encode() * 50
                (self.dir / f"{name}.tar.xz").write_bytes(data)
                (self.dir / f"{name}.manifest.txt").write_text(
                    f"image: {image}\nkey_packages:\ndocker 1.0-r0\ncontainerd 2.0-r0\nall_packages:\nfoo 1\n", encoding="utf-8"
                )
                (self.dir / f"LICENSES-{image}-{arch}.txt").write_text("licences\n", encoding="utf-8")
                entry = json.loads(
                    pins("entry", image, arch, self.dir / f"{name}.tar.xz", self.dir / f"{name}.manifest.txt", tag, "ab" * 32).stdout
                )
                (self.dir / f"{name}.json").write_text(json.dumps(entry), encoding="utf-8")
                digest = hashlib.sha256(data).hexdigest()
                (self.dir / f"{name}.tar.xz.sha256").write_text(f"{digest}  {name}.tar.xz\n", encoding="utf-8")

    def release_json(self):
        assets = []
        for f in sorted(self.dir.iterdir()):
            assets.append({"name": f.name, "size": f.stat().st_size, "digest": "sha256:" + hashlib.sha256(f.read_bytes()).hexdigest()})
        return {"assets": assets}


class PinsToolTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="engine-rootfs-test-")
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        self.build = FakeBuild(self.dir)

    def test_rel_088_entry_has_the_documented_fields(self):
        entry = json.loads((self.dir / "dockering-engine-alpine-x86_64.json").read_text(encoding="utf-8"))
        for key in ("name", "size", "sha256", "url", "base_url", "base_size", "base_sha256", "packages", "tar_sha256"):
            self.assertIn(key, entry)
        self.assertEqual(entry["name"], "dockering-engine-alpine-x86_64.tar.xz")
        self.assertEqual(
            entry["url"], "https://github.com/pavel-purma/dockering/releases/download/rootfs-1/dockering-engine-alpine-x86_64.tar.xz"
        )
        self.assertEqual(entry["packages"], ["docker=1.0-r0", "containerd=2.0-r0"])

    def test_rel_088_verify_accepts_a_complete_consistent_build(self):
        self.assertIn("4 asset(s)", pins("verify", self.dir, "rootfs-1").stdout)

    def test_rel_088_verify_rejects_a_tampered_asset_a_wrong_tag_and_a_missing_cell(self):
        (self.dir / "dockering-engine-ubuntu-x86_64.tar.xz").write_bytes(b"tampered")
        bad = pins("verify", self.dir, "rootfs-1", check=False)
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("sha256", bad.stderr)
        self.assertNotEqual(pins("verify", self.dir, "rootfs-2", check=False).returncode, 0)
        (self.dir / "dockering-engine-alpine-aarch64.json").unlink()
        self.assertIn("missing dockering-engine-alpine-aarch64.json", pins("verify", self.dir, "rootfs-1", check=False).stderr)
        self.assertNotEqual(pins("verify", self.dir, "rootfs-1", "--partial", check=False).returncode, 0)  # still tampered

    def test_rel_088_check_uploaded_compares_size_and_digest(self):
        # The release JSON lives next to the build directory, not inside it: every file in the directory is compared.
        with tempfile.TemporaryDirectory(prefix="engine-rootfs-release-") as other:
            good = Path(other) / "release.json"
            good.write_text(json.dumps(self.build.release_json()), encoding="utf-8")
            self.assertIn("match the release", pins("check-uploaded", self.dir, good).stdout)
            broken = self.build.release_json()
            broken["assets"][0]["digest"] = "sha256:" + "0" * 64
            broken["assets"][1]["size"] += 1
            del broken["assets"][2]
            bad = Path(other) / "broken.json"
            bad.write_text(json.dumps(broken), encoding="utf-8")
            result = pins("check-uploaded", self.dir, bad, check=False)
            self.assertNotEqual(result.returncode, 0)
            for needle in ("digest", "size", "not in the release"):
                self.assertIn(needle, result.stderr)

    def test_rel_088_render_and_notes_cover_all_four_cells(self):
        rendered = pins("render", self.dir, "rootfs-1").stdout
        self.assertEqual(len(re.findall(r'status = "published"', rendered)), 4)
        for image in IMAGES:
            for arch in ARCHES:
                self.assertIn(f"[images.{image}.arch.{arch}.asset]", rendered)
        notes = pins("notes", self.dir, "rootfs-1").stdout
        self.assertIn("SHA256SUMS", notes)
        self.assertIn("gh attestation verify", notes)
        self.assertEqual(notes.count("dockering-engine-"), notes.count("tar.xz`") + notes.count("<image>-<arch>"))


class WorkflowContractTests(unittest.TestCase):
    def setUp(self):
        self.text = text(WORKFLOW)
        self.code = "\n".join(line for line in self.text.split("\n") if not line.lstrip().startswith("#"))

    def test_rel_088_triggers_are_dispatch_and_schedule_only(self):
        on = self.text[self.text.index("\non:\n") : self.text.index("\npermissions:")]
        self.assertRegex(on, r"workflow_dispatch:\n\s+inputs:\n\s+tag:")
        self.assertRegex(on, r"publish:\n(?:\s+.*\n)*?\s+default: false\n\s+type: boolean")
        self.assertRegex(on, r"schedule:\n\s+- cron:")
        for trigger in ("push:", "pull_request", "release:", "workflow_run"):
            self.assertNotIn(trigger, on)
        self.assertNotIn("pull_request_target", self.code)

    def test_rel_088_matrix_is_two_images_by_two_architectures_on_native_runners(self):
        build = job_block(self.text, "build")
        cells = re.findall(r"\{ image: (\w+), arch: (\w+), os: ([\w.-]+) \}", build)
        self.assertEqual(
            cells,
            [
                ("alpine", "x86_64", "ubuntu-24.04"),
                ("alpine", "aarch64", "ubuntu-24.04-arm"),
                ("ubuntu", "x86_64", "ubuntu-24.04"),
                ("ubuntu", "aarch64", "ubuntu-24.04-arm"),
            ],
        )

    def test_rel_088_smoke_runs_the_two_x86_64_images_on_the_wsl_runner_and_always_cleans_up(self):
        smoke = job_block(self.text, "smoke")
        self.assertIn("runs-on: [self-hosted, windows, wsl]", smoke)
        self.assertEqual(re.findall(r"\{ image: (\w+), port: \d+ \}", smoke), ["alpine", "ubuntu"])
        self.assertIn("name: engine-rootfs-${{ matrix.image }}-x86_64", smoke)
        self.assertIn("DISTRO: dockering-smoke-${{ github.run_id }}-${{ matrix.image }}", smoke)
        cleanup = smoke[smoke.rindex("      - name: Unregister the distro") :]
        self.assertIn("if: always()", cleanup)
        self.assertIn("-CleanupOnly", cleanup)
        self.assertIn("max-parallel: 1", smoke)

    def test_rel_088_publish_is_gated_and_least_privilege(self):
        publish = job_block(self.text, "publish")
        self.assertIn("needs: [prepare, build, smoke]", publish)
        self.assertIn("if: needs.prepare.outputs.publish == 'true'", publish)
        self.assertIn("environment: release", publish)
        granted = sorted(re.findall(r"^      (\w+-?\w*: write)$", publish, flags=re.M))
        self.assertEqual(granted, ["attestations: write", "contents: write", "id-token: write"])
        # No other job writes anything.
        self.assertEqual(self.code.count(": write"), 3)
        self.assertIn("contents: read", self.text[self.text.index("\npermissions:") :][:40])
        # It starts as a draft, is checked, and is never the repository's Latest release.
        self.assertLess(publish.index("--draft"), publish.index("check-uploaded"))
        self.assertLess(publish.index("check-uploaded"), publish.index("--draft=false"))
        code = chr(10).join(line for line in publish.split(chr(10)) if not line.lstrip().startswith("#"))
        self.assertEqual(code.count("--latest=false"), 2, "create and edit both keep it out of Latest")
        self.assertEqual(code.count("--prerelease"), 2)
        self.assertIn("attest-build-provenance@", publish)
        self.assertIn("SHA256SUMS", publish)
        self.assertIn("LICENSES-", publish)

    def test_rel_088_prepare_refuses_a_publish_that_is_not_from_main_or_reuses_a_tag(self):
        prepare = job_block(self.text, "prepare")
        self.assertIn("refs/heads/main", prepare)
        self.assertIn("already exists", prepare)
        self.assertIn("^rootfs-(0|[1-9][0-9]*)$", prepare)
        self.assertNotIn("publish=true", job_block(self.text, "build") + job_block(self.text, "smoke"))

    def test_rel_088_inputs_reach_scripts_only_through_env(self):
        # No `${{ ... }}` of an input, an output or a matrix value inside a run: body.
        for match in re.finditer(r"(?ms)^\s+run: \|?\n?(.*?)(?=^\s+- |^\s+\w[\w-]*:\s|\Z)", self.text):
            body = match.group(1)
            for expression in re.findall(r"\$\{\{.*?\}\}", body):
                self.assertTrue(
                    re.fullmatch(r"\$\{\{ (runner\.temp|github\.token) \}\}", expression),
                    f"expression in a run script: {expression}",
                )

    def test_rel_088_every_action_is_pinned_to_a_commit(self):
        uses = re.findall(r"uses: (\S+)(.*)", self.code)
        self.assertTrue(uses)
        for ref, rest in uses:
            with self.subTest(ref=ref):
                self.assertRegex(ref, r"@[0-9a-f]{40}$")
                self.assertRegex(rest, r"# v\d")

    def test_rel_088_one_run_at_a_time(self):
        self.assertRegex(self.text, r"concurrency:\n  group: engine-rootfs\n  cancel-in-progress: false")

    def test_rel_088_the_scheduled_run_never_publishes(self):
        prepare = job_block(self.text, "prepare")
        self.assertIn("publish=false", prepare)
        self.assertIn('if [[ "$REQUESTED_PUBLISH" == true ]]; then publish=true; fi', prepare)
        # `publish` is set to true in exactly one place, inside the workflow_dispatch branch.
        self.assertEqual(prepare.count("publish=true"), 1)


class Rel089WiringTests(unittest.TestCase):
    def test_rel_089_ci_runs_the_offline_check_in_lint(self):
        ci = text(CI)
        lint = ci[ci.index("  lint:") : ci.index("  test:")]
        self.assertIn("cargo xtask engine-rootfs-pin-check --allow-unpublished", lint)
        self.assertNotIn("--online", lint, "CI must not depend on the network for this check")

    def test_rel_089_ci_lints_the_build_script_and_the_smoke_script(self):
        ci = text(CI)
        self.assertRegex(ci, r"shellcheck [^\n]*tools/engine-rootfs/build\.sh[^\n]*tools/engine-rootfs/guest/\*\.sh")
        self.assertIn("tools/engine-rootfs/smoke.ps1", ci)

    def test_rel_089_release_checks_the_published_assets_before_it_assembles(self):
        release = text(RELEASE)
        assemble = release[release.index("  assemble:") : release.index("  draft:")]
        step = "Pinned engine root filesystems are published and match (REL-089)"
        self.assertIn(step, assemble)
        self.assertIn("target/debug/xtask engine-rootfs-pin-check --online", assemble)
        self.assertLess(assemble.index("Build xtask"), assemble.index(step))
        self.assertLess(assemble.index(step), assemble.index("Update manifest (UPD-002)"))
        self.assertIn("GH_TOKEN: ${{ github.token }}", assemble[assemble.index(step) :][:300])

    def test_rel_089_the_flag_is_dropped_together_everywhere_it_is_used(self):
        # One flip: pins.toml's header says to remove --allow-unpublished from both workflows.
        header = text(TOOLS / "pins.toml")[:1500]
        self.assertIn("--allow-unpublished", header)
        self.assertEqual(text(CI).count("--allow-unpublished"), text(CI).count("engine-rootfs-pin-check --allow-unpublished") + 1)
        self.assertEqual(text(RELEASE).count("--allow-unpublished"), 2)


if __name__ == "__main__":
    unittest.main()
