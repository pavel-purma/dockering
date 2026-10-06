"""REL-031/032/034: run the `verify` step of release.yml against fake repositories.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
The step's script is extracted from the workflow and run the way GitHub runs `shell: bash`
(`bash --noprofile --norc -eo pipefail`), so the test follows the real gates.
"""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release.yml"
BASH = (
    str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
    if os.name == "nt"
    else shutil.which("bash")
)
KEYS_WITH_KEY = 'pub const PUBLIC_KEYS: &[&str] = &["RWQf6LRCGA9i53mlYRhIjBdPYZUY8rB8cmpAwStz9Ik9flV0Lf5DpFSg"];\n'
KEYS_EMPTY = "pub const PUBLIC_KEYS: &[&str] = &[];\n"
KEYS_COMMENTED = 'pub const PUBLIC_KEYS: &[&str] = &[\n    // "RWQcommentedOut",\n];\n'
SIGNED = {
    "WINDOWS_SIGNING": "signpath",
    "HAS_SIGNPATH": "true",
    "HAS_UPDATE_KEY": "true",
    "WINDOWS_SIGNER_SUBJECT": "CN=SignPath Foundation, O=SignPath Foundation, L=Lewes, S=Delaware, C=US",
}


def shell_path(path):
    text = path.as_posix()
    if os.name == "nt":
        return "/" + text[0].lower() + text[2:]
    return text


def verify_script():
    lines = WORKFLOW.read_text(encoding="utf-8").split("\n")
    start = lines.index("      - id: v")
    run = next(i for i in range(start, len(lines)) if lines[i].strip() == "run: |")
    body = []
    for line in lines[run + 1 :]:
        if line.strip() and not line.startswith(" " * 10):
            break
        body.append(line[10:])
    return "\n".join(body)


@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required")
@unittest.skipIf(sys.platform == "darwin", "the workflow runs on Ubuntu's Bash")
@unittest.skipUnless(sys.version_info >= (3, 11), "the verify step reads Cargo.toml with tomllib")
class ReleaseVerifyTests(unittest.TestCase):
    def run_verify(self, env, *, keys=KEYS_WITH_KEY, version="0.3.0", changelog_version="0.3.0"):
        with tempfile.TemporaryDirectory(prefix="dockering-verify-test-") as temp:
            root = Path(temp)
            (root / "crates/dk-update/src").mkdir(parents=True)
            (root / "crates/dk-update/src/keys.rs").write_text(keys, encoding="utf-8")
            (root / "Cargo.toml").write_text(
                f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
            )
            (root / "CHANGELOG.md").write_text(
                f"# Changelog\n\n## [{changelog_version}] - 2026-01-01\n\n- Something.\n",
                encoding="utf-8",
            )
            git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"]
            subprocess.run(git + ["init", "-q", "-b", "main"], cwd=root, check=True)
            subprocess.run(git + ["add", "-A"], cwd=root, check=True)
            subprocess.run(git + ["commit", "-q", "-m", "x"], cwd=root, check=True)
            sha = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True
            ).stdout.strip()
            subprocess.run(["git", "update-ref", "refs/remotes/origin/main", sha], cwd=root, check=True)

            bin_dir = root / "bin"
            bin_dir.mkdir()
            shim = bin_dir / "python3"
            shim.write_text(
                '#!/usr/bin/env bash\nexec "' + Path(sys.executable).as_posix() + '" "$@"\n',
                encoding="utf-8",
                newline="\n",
            )
            shim.chmod(0o755)
            script = root / "verify.sh"
            script.write_text(verify_script(), encoding="utf-8", newline="\n")
            output = root / "github_output"
            output.write_text("", encoding="utf-8")

            environment = os.environ.copy()
            environment.update(
                EVENT_NAME="workflow_dispatch",
                REQUESTED_UNSIGNED="true",
                PUBLIC_RELEASES="",
                WINDOWS_SIGNING="none",
                MACOS_SIGNING="none",
                WINDOWS_SIGNER_SUBJECT="",
                HAS_AZURE="false",
                HAS_SIGNPATH="false",
                HAS_APPLE="false",
                HAS_UPDATE_KEY="false",
                GITHUB_REF="refs/heads/main",
                GITHUB_REF_NAME="main",
                GITHUB_SHA=sha,
                GITHUB_OUTPUT=shell_path(output),
                MOCK_BIN=shell_path(bin_dir),
                SCRIPT=shell_path(script),
            )
            environment.update(env)
            result = subprocess.run(
                [
                    BASH,
                    "--noprofile",
                    "--norc",
                    "-c",
                    'export PATH="$MOCK_BIN:$PATH"; bash --noprofile --norc -eo pipefail "$SCRIPT"',
                ],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=60,
            )
            outputs = dict(
                line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines() if "=" in line
            )
            return result, outputs

    def assertRejected(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stdout + result.stderr)

    def tag(self, **extra):
        return {
            "EVENT_NAME": "push",
            "GITHUB_REF": "refs/tags/v0.3.0",
            "GITHUB_REF_NAME": "v0.3.0",
            "PUBLIC_RELEASES": "true",
            **SIGNED,
            **extra,
        }

    def test_rel_016_branch_dispatch_defaults_to_unsigned(self):
        result, outputs = self.run_verify({})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((outputs["unsigned"], outputs["macos_signed"]), ("true", "false"))

    def test_rel_016_unsigned_tag_needs_no_configuration(self):
        result, outputs = self.run_verify(
            {"EVENT_NAME": "push", "GITHUB_REF": "refs/tags/v0.3.0", "GITHUB_REF_NAME": "v0.3.0"},
            keys=KEYS_EMPTY,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(outputs["unsigned"], "true")

    def test_rel_016_explicit_unsigned_overrides_the_signed_channel(self):
        result, outputs = self.run_verify(self.tag(EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main", REQUESTED_UNSIGNED="true"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(outputs["unsigned"], "true")

    def test_rel_034_signed_rehearsal_does_not_need_macos_or_a_signer_pin(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "WINDOWS_SIGNER_SUBJECT": ""}
        result, outputs = self.run_verify(env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((outputs["unsigned"], outputs["macos_signed"]), ("false", "false"))

    def test_rel_034_apple_signing_requires_its_secrets(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "MACOS_SIGNING": "apple"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "macOS signing/notarisation configuration is incomplete")
        result, outputs = self.run_verify({**env, "HAS_APPLE": "true"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(outputs["macos_signed"], "true")

    def test_rel_034_unknown_macos_mode_rejected(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "MACOS_SIGNING": "yes"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "MACOS_SIGNING must be apple or none")

    def test_rel_031_signed_mode_needs_a_windows_provider(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "WINDOWS_SIGNING": "none"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "signed mode requires WINDOWS_SIGNING=azure or signpath")

    def test_rel_031_incomplete_provider_configuration_rejected(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "HAS_SIGNPATH": "false"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "SignPath signing configuration is incomplete")
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "WINDOWS_SIGNING": "azure"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "Azure signing configuration is incomplete")

    def test_rel_031_signed_mode_needs_the_update_signing_key(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "HAS_UPDATE_KEY": "false"}
        result, _ = self.run_verify(env)
        self.assertRejected(result, "signed mode requires UPDATE_SIGNING_KEY")

    def test_rel_031_signed_mode_needs_a_compiled_in_public_key(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED}
        for keys in (KEYS_EMPTY, KEYS_COMMENTED):
            result, _ = self.run_verify(env, keys=keys)
            self.assertRejected(result, "requires a public key in crates/dk-update/src/keys.rs")

    def test_rel_032_signed_tag_needs_the_signer_subject(self):
        result, _ = self.run_verify(self.tag(WINDOWS_SIGNER_SUBJECT=""))
        self.assertRejected(result, "signed tags require the variable WINDOWS_SIGNER_SUBJECT")
        result, outputs = self.run_verify(self.tag())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(outputs["unsigned"], "false")

    def test_rel_011_tag_must_match_the_workspace_version(self):
        result, _ = self.run_verify(self.tag(GITHUB_REF="refs/tags/v0.3.1", GITHUB_REF_NAME="v0.3.1"))
        self.assertRejected(result, "tag v0.3.1 != workspace version 0.3.0")

    def test_rel_011_changelog_needs_a_section_for_the_version(self):
        result, _ = self.run_verify({}, changelog_version="0.2.0")
        self.assertRejected(result, "CHANGELOG.md has no '## [0.3.0]' section")


if __name__ == "__main__":
    unittest.main()
