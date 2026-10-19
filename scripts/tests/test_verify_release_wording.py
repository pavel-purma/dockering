"""REL-083/REL-084: scripts/verify-release.ps1 and release.yml must agree on what a draft carries.

The verifier checks a downloaded release, the workflow builds it. Two things have to stay in step:

* the one sentence about updates that the workflow's "Release notes from CHANGELOG.md" step writes
  (`all:true`, `none:true`, `none:false` = updater all, updater none with a signed manifest, updater
  none with an unsigned manifest) and the sentence `$UpdateNotes` of the verifier requires;
* the file-set rule: `dockering-update.json.minisig` ships exactly when `sign_manifest` is true, so the
  verifier derives it from `-Updater` and `-Expect`, never from `-Expect` alone.

The first group reads both files as text and runs everywhere. The second group runs the verifier on a
fake bundle for the four (-Expect, -Updater) combinations and needs `pwsh` and `bash`; without them it is
skipped. The platform checks (PE headers, ELF, Authenticode) fail on a fake bundle on purpose, so only
the file set, checksum, manifest and notes checks are asserted.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
"""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_release_verify import BASH, find_step, shell_path, step_script_of  # noqa: E402


ROOT = Path(__file__).resolve().parents[2]
VERIFIER = ROOT / "scripts/verify-release.ps1"
PWSH = shutil.which("pwsh")

# workflow `case "$UPDATER:$SIGN_MANIFEST"` label -> `$UpdateNotes` key in the verifier
CHANNELS = {"all:true": "all", "none:true": "none-signed", "none:false": "none-unsigned"}

DISTRIBUTIONS = (
    "Dockering-Setup-x64.exe",
    "Dockering-Setup-arm64.exe",
    "Dockering-x64.zip",
    "Dockering-arm64.zip",
    "Dockering-aarch64.dmg",
    "Dockering-x86_64.dmg",
    "Dockering-x86_64.AppImage",
    "Dockering-aarch64.AppImage",
    "Dockering-x86_64.deb",
    "Dockering-aarch64.deb",
    "Dockering-x86_64.tar.gz",
    "Dockering-aarch64.tar.gz",
)
PLATFORMS = {
    "windows-x86_64": ("Dockering-Setup-x64.exe", "inno"),
    "windows-aarch64": ("Dockering-Setup-arm64.exe", "inno"),
    "macos-aarch64": ("Dockering-aarch64.dmg", "dmg"),
    "macos-x86_64": ("Dockering-x86_64.dmg", "dmg"),
    "linux-x86_64": ("Dockering-x86_64.AppImage", "appimage"),
    "linux-aarch64": ("Dockering-aarch64.AppImage", "appimage"),
}
VERSION = "0.3.0"
CHANGELOG = f"# Changelog\n\n## [{VERSION}] - 2026-01-01\n\n- Something.\n\n## [0.2.0] - 2025-12-01\n\n- Older.\n"
MINISIG = "dockering-update.json.minisig"


def verifier_text():
    return VERIFIER.read_text(encoding="utf-8")


def workflow_sentences():
    """channel key -> the update sentence of release.yml's notes step."""
    script = step_script_of(find_step("assemble", "Release notes from CHANGELOG.md"))
    found = re.findall(r'^\s*(all:true|none:true|none:false)\)\s*\n\s*updates="([^"]*)" ;;', script, re.M)
    return {CHANNELS[label]: sentence for label, sentence in found}


def verifier_sentences():
    """channel key -> the `$UpdateNotes` sentence of verify-release.ps1."""
    block = re.search(r"\$UpdateNotes = \[ordered\]@\{(.*?)\n\}", verifier_text(), re.S)
    if not block:
        raise AssertionError("verify-release.ps1 has no $UpdateNotes table")
    return dict(re.findall(r"^\s*'([a-z-]+)'\s*=\s*\"([^\"]*)\"", block.group(1), re.M))


def function_body(name):
    """The text of one PowerShell function (up to the next top-level `}`)."""
    found = re.search(rf"^function {re.escape(name)}\b.*?^}}", verifier_text(), re.S | re.M)
    if not found:
        raise AssertionError(f"verify-release.ps1 has no function {name}")
    return found.group(0)


class VerifierWordingStaticTests(unittest.TestCase):
    def test_rel_084_notes_sentences_agree_between_workflow_and_verifier(self):
        workflow = workflow_sentences()
        verifier = verifier_sentences()
        self.assertEqual(sorted(workflow), sorted(CHANNELS.values()), "the workflow case lists the three channels")
        self.assertEqual(sorted(verifier), sorted(CHANNELS.values()), "the verifier lists the three channels")
        for channel in CHANNELS.values():
            with self.subTest(channel):
                self.assertEqual(verifier[channel], workflow[channel])
                # Bash would expand these inside the double quotes of the workflow, and PowerShell too.
                for forbidden in ("$", "`", "\\"):
                    self.assertNotIn(forbidden, workflow[channel])

    def test_rel_084_the_wording_follows_the_channel(self):
        sentences = verifier_sentences()
        self.assertTrue(sentences["all"].startswith("Dockering checks for updates in these builds"))
        self.assertTrue(
            sentences["none-signed"].startswith("Automatic updates are not included in these builds; the update manifest is signed")
        )
        self.assertTrue(
            sentences["none-unsigned"].startswith(
                "Automatic updates are not included in these builds; the update manifest has no signature"
            )
        )
        # The verifier rejects another channel's sentence by substring, so none may contain another.
        for one, text in sentences.items():
            for other, other_text in sentences.items():
                if one != other:
                    self.assertNotIn(other_text, text, f"{other} is inside {one}")
        # The pre-REL-083 sentence ('unsigned means no updater') is gone from both files.
        self.assertNotIn("Automatic updates are disabled", verifier_text())
        self.assertNotIn("Automatic updates are disabled", (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8"))

    def test_rel_084_file_set_rules_follow_updater(self):
        text = verifier_text()
        self.assertRegex(text, r"(?m)^\s*\[string\] \$Updater = 'none',")
        self.assertIn("-Updater none|all", text)
        # sign_manifest = (updater is all) or (OS-signed): the same formula as the workflow's verify step.
        channel = function_body("Get-UpdaterChannel")
        self.assertIn("Ctx.Updater", channel)
        self.assertIn("Ctx.Expect", channel)
        self.assertIn("Get-UpdaterChannel", function_body("Test-ManifestSigned"))
        self.assertIn("Test-ManifestSigned", function_body("Get-ExpectedFiles"))
        self.assertIn(MINISIG, function_body("Get-ExpectedFiles"))
        file_set = function_body("Test-FileSet")
        self.assertIn("Test-ManifestSigned", file_set)
        self.assertIn(MINISIG, file_set)
        # Nothing may decide the .minisig from the OS signing mode alone any more.
        for number, line in enumerate(text.splitlines(), 1):
            if "minisig" in line and "Ctx.Expect" in line and not line.lstrip().startswith("#"):
                self.fail(f"verify-release.ps1:{number} decides the .minisig from -Expect: {line.strip()}")
        self.assertIn("-Updater must be none or all", text)

    def test_rel_084_header_points_at_the_xtask_check(self):
        header = "\n".join(verifier_text().splitlines()[:20])
        self.assertIn("cargo xtask verify-update-artifacts", header)
        self.assertIn("no minisign", header)
        self.assertIn("-Updater", header)

    def test_rel_084_skill_and_docs_run_both_checks(self):
        for path in (
            ".agents/skills/release/SKILL.md",
            ".agents/skills/release/checklist-template.md",
            "docs/release.md",
            "docs/signing.md",
        ):
            with self.subTest(path):
                text = (ROOT / path).read_text(encoding="utf-8")
                self.assertIn("verify-update-artifacts", text)
                self.assertIn("-Updater", text)


@unittest.skipUnless(PWSH, "pwsh is required to run the verifier on a fake bundle")
@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required to run the workflow's notes step")
class VerifierFakeBundleTests(unittest.TestCase):
    """Run verify-release.ps1 on a fake bundle whose notes come from the workflow's own notes step."""

    COMBINATIONS = (
        # (-Expect, -Updater, sign_manifest, WINDOWS_SIGNING, UNSIGNED)
        ("Unsigned", "none", False, "none", "true"),
        ("Unsigned", "all", True, "none", "true"),
        ("Signed", "none", True, "signpath", "false"),
        ("Signed", "all", True, "signpath", "false"),
    )
    CHECKED = (
        "files.set",
        "checksums.format",
        "checksums.coverage",
        "checksums.match",
        "manifest.schema",
        "manifest.version",
        "manifest.notes_url",
        "manifest.platforms",
        *(f"manifest.platform.{key}" for key in PLATFORMS),
        "notes.changelog",
        "notes.footer",
    )

    @staticmethod
    def channel(updater, sign_manifest):
        if updater == "all":
            return "all"
        return "none-signed" if sign_manifest else "none-unsigned"

    def notes_by_workflow(self, root, *, updater, sign_manifest, windows_signing, unsigned):
        """RELEASE_NOTES.md as the workflow's step writes it, into <root>/dist."""
        script = root / "notes-step.sh"
        script.write_text(
            step_script_of(find_step("assemble", "Release notes from CHANGELOG.md")), encoding="utf-8", newline="\n"
        )
        environment = os.environ.copy()
        environment.update(
            {
                "VERSION": VERSION,
                "UNSIGNED": unsigned,
                "MACOS_SIGNED": "false",
                "UPDATER": updater,
                "SIGN_MANIFEST": "true" if sign_manifest else "false",
                "WINDOWS_SIGNING": windows_signing,
                "PUBLIC_REPOSITORY": "true",
                "GITHUB_REPOSITORY": "pavel-purma/dockering",
            }
        )
        result = subprocess.run(
            [BASH, "--noprofile", "--norc", "-eo", "pipefail", shell_path(script)],
            cwd=root,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def make_bundle(self, root, combo, *, minisig, notes_channel_of=None):
        """A bundle with every file the verifier looks at. `notes_channel_of` takes the notes of another combination."""
        _, updater, sign_manifest, windows_signing, unsigned = notes_channel_of or combo
        dist = root / "dist"
        dist.mkdir()
        (root / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
        self.notes_by_workflow(
            root, updater=updater, sign_manifest=sign_manifest, windows_signing=windows_signing, unsigned=unsigned
        )
        for name in DISTRIBUTIONS:
            (dist / name).write_bytes(f"fake {name}".encode())
        platforms = {}
        for key, (name, kind) in PLATFORMS.items():
            data = (dist / name).read_bytes()
            platforms[key] = {
                "kind": kind,
                "url": f"https://github.com/pavel-purma/dockering/releases/download/v{VERSION}/{name}",
                "sha256": hashlib.sha256(data).hexdigest(),
                "size": len(data),
            }
        manifest = {
            "schema": 1,
            "version": VERSION,
            "pub_date": "2026-10-19T00:00:00Z",
            "notes_url": f"https://github.com/pavel-purma/dockering/releases/tag/v{VERSION}",
            "platforms": platforms,
        }
        (dist / "dockering-update.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if minisig:
            # Not a real signature: the verifier only checks the file set. xtask verifies minisign.
            (dist / MINISIG).write_text("untrusted comment: fake\nRWQfake\n", encoding="utf-8")
        sums = "".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
            for path in sorted(dist.iterdir())
            if path.name != "SHA256SUMS"
        )
        (dist / "SHA256SUMS").write_text(sums, encoding="utf-8")
        return dist

    def run_verifier(self, root, dist, expect, updater):
        result = subprocess.run(
            [
                PWSH,
                "-NoProfile",
                "-File",
                str(VERIFIER),
                "-Dir",
                str(dist),
                "-Version",
                VERSION,
                "-Expect",
                expect,
                *(["-Updater", updater] if updater is not None else []),
                "-Changelog",
                str(root / "CHANGELOG.md"),
            ],
            capture_output=True,
            text=True,
            timeout=180,
        )
        statuses = {}
        details = {}
        for line in result.stdout.splitlines():
            found = re.match(r"^\[(PASS|FAIL|SKIP)\] (\S+)\s+(.*)$", line)
            if found:
                statuses[found.group(2)] = found.group(1)
                details[found.group(2)] = found.group(3)
        return result, statuses, details

    def test_rel_084_four_combinations_pass_the_checks_that_are_not_platform_specific(self):
        for combo in self.COMBINATIONS:
            expect, updater, sign_manifest, *_ = combo
            with self.subTest(expect=expect, updater=updater), tempfile.TemporaryDirectory(prefix="dockering-vr-") as temp:
                root = Path(temp)
                dist = self.make_bundle(root, combo, minisig=sign_manifest)
                result, statuses, details = self.run_verifier(root, dist, expect, updater)
                self.assertIn(result.returncode, (0, 1), result.stdout + result.stderr)
                for check in self.CHECKED:
                    self.assertEqual(statuses.get(check), "PASS", f"{check}: {details.get(check)}\n{result.stdout}")

    def test_rel_084_updater_defaults_to_none(self):
        combo = self.COMBINATIONS[0]
        with tempfile.TemporaryDirectory(prefix="dockering-vr-") as temp:
            root = Path(temp)
            dist = self.make_bundle(root, combo, minisig=False)
            _, statuses, details = self.run_verifier(root, dist, "Unsigned", None)
            for check in ("files.set", "notes.footer"):
                self.assertEqual(statuses.get(check), "PASS", f"{check}: {details.get(check)}")

    def test_rel_084_minisig_follows_updater_or_signed(self):
        for combo in self.COMBINATIONS:
            expect, updater, sign_manifest, *_ = combo
            with self.subTest(expect=expect, updater=updater), tempfile.TemporaryDirectory(prefix="dockering-vr-") as temp:
                root = Path(temp)
                # The opposite of what sign_manifest says: a missing .minisig, or one that must not be there.
                dist = self.make_bundle(root, combo, minisig=not sign_manifest)
                _, statuses, details = self.run_verifier(root, dist, expect, updater)
                self.assertEqual(statuses.get("files.set"), "FAIL", details.get("files.set"))
                self.assertIn(MINISIG, details.get("files.set", ""))

    def test_rel_084_notes_of_another_channel_fail(self):
        for index, combo in enumerate(self.COMBINATIONS):
            expect, updater, sign_manifest, *_ = combo
            with self.subTest(expect=expect, updater=updater), tempfile.TemporaryDirectory(prefix="dockering-vr-") as temp:
                root = Path(temp)
                # The notes of a release with the same OS signing mode, built with another channel.
                other = next(
                    c
                    for c in self.COMBINATIONS
                    if c[0] == expect and self.channel(c[1], c[2]) != self.channel(updater, sign_manifest)
                )
                dist = self.make_bundle(root, combo, minisig=sign_manifest, notes_channel_of=other)
                _, statuses, details = self.run_verifier(root, dist, expect, updater)
                self.assertEqual(statuses.get("files.set"), "PASS", details.get("files.set"))
                self.assertEqual(statuses.get("notes.footer"), "FAIL", details.get("notes.footer"))
                self.assertIn("update sentence", details.get("notes.footer", ""))

    def test_rel_084_a_wrong_channel_value_is_a_usage_error(self):
        combo = self.COMBINATIONS[0]
        with tempfile.TemporaryDirectory(prefix="dockering-vr-") as temp:
            root = Path(temp)
            dist = self.make_bundle(root, combo, minisig=False)
            for bad in ("both", "ALL", "None"):
                result, statuses, _ = self.run_verifier(root, dist, "Unsigned", bad)
                self.assertEqual(result.returncode, 2, f"-Updater {bad!r}: {result.stdout}{result.stderr}")
                self.assertIn("-Updater must be none or all", result.stderr)
                self.assertEqual(statuses, {})


if __name__ == "__main__":
    unittest.main()
