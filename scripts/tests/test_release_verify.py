"""REL-016/031/032/034/083/084/086: run the `verify` step of release.yml against fake repositories,
and check how `package` and `assemble` read its outputs.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
The step's script is extracted from the workflow and run the way GitHub runs `shell: bash`
(`bash --noprofile --norc -eo pipefail`), so the test follows the real gates. The workflow is read
as text (no YAML parser, so the tests run on every runner); a parser check runs only when PyYAML exists.
"""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release.yml"
EVIDENCE_SCRIPT = ROOT / "scripts/release-evidence.sh"
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
# The six release targets (the `package` matrix) and the two marker strings of REL-084/REL-086.
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
UPDATER_MARKER = b"https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json"
SEAM_MARKER = b"dockering-update-e2e-seam-v1"
VERIFY_OUTPUTS = {"version", "prerelease", "unsigned", "macos_signed", "updater", "sign_manifest"}


def shell_path(path):
    text = Path(path).as_posix()
    if os.name == "nt":
        return "/" + text[0].lower() + text[2:]
    return text


def workflow_lines():
    return WORKFLOW.read_text(encoding="utf-8").split("\n")


def job_lines(job):
    """The lines of one job under `jobs:` (header included)."""
    lines = workflow_lines()
    start = lines.index(f"  {job}:", lines.index("jobs:"))
    end = next(
        (i for i in range(start + 1, len(lines)) if re.match(r"^  [A-Za-z0-9_-]+:\s*$", lines[i])),
        len(lines),
    )
    return lines[start:end]


def job_steps(job):
    """The steps of a job, each a list of lines (comment lines dropped)."""
    lines = job_lines(job)
    steps = []
    for line in lines[lines.index("    steps:") + 1 :]:
        if line.startswith("      - "):
            steps.append([line])
        elif steps and not line.lstrip().startswith("#"):
            steps[-1].append(line)
    return steps


def step_name(step):
    for line in step:
        found = re.match(r"^(?:      - |        )name:\s*(.*)$", line)
        if found:
            return found.group(1)
    return None


def find_step(job, prefix):
    found = [step for step in job_steps(job) if (step_name(step) or "").startswith(prefix)]
    if len(found) != 1:
        raise AssertionError(f"expected exactly one step named {prefix!r} in job {job}, found {len(found)}")
    return found[0]


def step_condition(step):
    for line in step:
        found = re.match(r"^(?:      - |        )if:\s*(.*)$", line)
        if found:
            return found.group(1)
    return None


def step_script_of(step):
    for i, line in enumerate(step):
        inline = re.match(r"^        run:\s*([^|\s].*)$", line)
        if inline:
            return inline.group(1)
        if line.strip() == "run: |":
            body = []
            for rest in step[i + 1 :]:
                if rest.strip() and not rest.startswith(" " * 10):
                    break
                body.append(rest[10:])
            return "\n".join(body)
    raise AssertionError(f"step {step_name(step)!r} has no run script")


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
                REQUESTED_UPDATER="inherit",
                PUBLIC_RELEASES="",
                UPDATER="",
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
            "REQUESTED_UPDATER": "",  # a push has no inputs
            "PUBLIC_RELEASES": "true",
            **SIGNED,
            **extra,
        }

    def unsigned_tag(self, **extra):
        """A tag push while PUBLIC_RELEASES is unset: OS signing is off."""
        return {
            "EVENT_NAME": "push",
            "GITHUB_REF": "refs/tags/v0.3.0",
            "GITHUB_REF_NAME": "v0.3.0",
            "REQUESTED_UPDATER": "",
            **extra,
        }

    def assertChannel(self, env, updater, sign_manifest, unsigned, *, keys=KEYS_WITH_KEY, label=""):
        result, outputs = self.run_verify(env, keys=keys)
        self.assertEqual(result.returncode, 0, f"{label}\n{result.stdout}{result.stderr}")
        self.assertEqual(set(outputs), VERIFY_OUTPUTS, f"{label}: every existing output is kept")
        self.assertEqual(
            (outputs["updater"], outputs["sign_manifest"], outputs["unsigned"]),
            (updater, sign_manifest, unsigned),
            label,
        )
        return outputs

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

    # --- REL-083: the updater gate is independent of OS signing -------------------------------

    def test_rel_016_unsigned_default_unchanged(self):
        # The default dispatch and an unsigned tag stay unsigned and need no configuration at all.
        self.assertChannel({}, "none", "false", "true", label="default dispatch")
        self.assertChannel(
            self.unsigned_tag(), "none", "false", "true", keys=KEYS_EMPTY, label="unsigned tag"
        )
        # An explicit unsigned run bypasses OS signing even with every credential present, and
        # without UPDATER it does not sign the manifest either.
        outputs = self.assertChannel(
            {
                **self.tag(EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main"),
                "REQUESTED_UNSIGNED": "true",
                "REQUESTED_UPDATER": "inherit",
                "MACOS_SIGNING": "apple",
                "HAS_APPLE": "true",
            },
            "none",
            "false",
            "true",
            label="explicit unsigned with credentials",
        )
        self.assertEqual(outputs["macos_signed"], "false")
        # The signed channel is unchanged: PUBLIC_RELEASES selects OS signing, not the updater.
        outputs = self.assertChannel(self.tag(), "none", "true", "false", label="signed tag")
        self.assertEqual(outputs["macos_signed"], "false")

    def test_rel_083_truth_table(self):
        key = {"HAS_UPDATE_KEY": "true"}
        main = {"EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main", "GITHUB_REF_NAME": "main"}
        on_tag = {"EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/tags/v0.3.0", "GITHUB_REF_NAME": "v0.3.0"}

        def dispatch(unsigned, requested, variable, base=None, **extra):
            env = {**(base or main), "REQUESTED_UNSIGNED": unsigned, "REQUESTED_UPDATER": requested}
            if variable is not None:
                env["UPDATER"] = variable
            return {**env, **extra}

        signed_config = {**SIGNED}
        # (label, env, updater, sign_manifest, unsigned): the rows of plan 6.4, then the variations.
        rows = [
            ("tag, PUBLIC_RELEASES unset, UPDATER unset", self.unsigned_tag(), "none", "false", "true"),
            ("tag, PUBLIC_RELEASES unset, UPDATER=none", self.unsigned_tag(UPDATER="none"), "none", "false", "true"),
            ("tag, PUBLIC_RELEASES unset, UPDATER=all", self.unsigned_tag(UPDATER="all", **key), "all", "true", "true"),
            ("tag, PUBLIC_RELEASES=true, UPDATER unset", self.tag(), "none", "true", "false"),
            ("tag, PUBLIC_RELEASES=true, UPDATER=none", self.tag(UPDATER="none"), "none", "true", "false"),
            ("tag, PUBLIC_RELEASES=true, UPDATER=all", self.tag(UPDATER="all"), "all", "true", "false"),
            ("main, unsigned=true, input inherit, variable all", dispatch("true", "inherit", "all", **key), "all", "true", "true"),
            ("main, unsigned=true, input inherit, variable none", dispatch("true", "inherit", "none"), "none", "false", "true"),
            ("main, unsigned=true, input inherit, variable unset", dispatch("true", "inherit", None), "none", "false", "true"),
            ("main, unsigned=true, input none, variable all", dispatch("true", "none", "all", **key), "none", "false", "true"),
            ("main, unsigned=true, input none, variable unset", dispatch("true", "none", None), "none", "false", "true"),
            ("main, unsigned=true, input all, variable none", dispatch("true", "all", "none", **key), "all", "true", "true"),
            ("main, unsigned=false, input all", dispatch("false", "all", None, **signed_config), "all", "true", "false"),
            ("main, unsigned=false, input inherit, variable unset", dispatch("false", "inherit", None, **signed_config), "none", "true", "false"),
            ("main, unsigned=false, input none, variable all", dispatch("false", "none", "all", **signed_config), "none", "true", "false"),
            ("v* tag dispatch, input inherit, variable all", dispatch("true", "inherit", "all", on_tag, **key), "all", "true", "true"),
            ("v* tag dispatch, input inherit, variable unset", dispatch("true", "inherit", None, on_tag), "none", "false", "true"),
            (
                "signed v* tag dispatch, input inherit, variable none",
                dispatch("false", "inherit", "none", on_tag, PUBLIC_RELEASES="true", **signed_config),
                "none",
                "true",
                "false",
            ),
        ]
        for label, env, updater, sign_manifest, unsigned in rows:
            with self.subTest(label):
                self.assertChannel(env, updater, sign_manifest, unsigned, label=label)

    def test_rel_083_needs_key_and_public_key(self):
        no_key = "UPDATER=all requires UPDATE_SIGNING_KEY"
        no_public = "UPDATER=all requires a public key in crates/dk-update/src/keys.rs"
        # Unsigned builds with the updater: the variable, or the input on a branch.
        for env in (
            {"UPDATER": "all"},
            {"REQUESTED_UPDATER": "all"},
            self.unsigned_tag(UPDATER="all"),
        ):
            with self.subTest(env=env):
                result, _ = self.run_verify(env)
                self.assertRejected(result, no_key)
                for keys in (KEYS_EMPTY, KEYS_COMMENTED):
                    result, _ = self.run_verify({**env, "HAS_UPDATE_KEY": "true"}, keys=keys)
                    self.assertRejected(result, no_public)
                result, outputs = self.run_verify({**env, "HAS_UPDATE_KEY": "true"})
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual((outputs["updater"], outputs["sign_manifest"]), ("all", "true"))
        # Without the updater an unsigned build needs neither.
        result, outputs = self.run_verify({"UPDATER": "none"}, keys=KEYS_EMPTY)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((outputs["updater"], outputs["sign_manifest"]), ("none", "false"))
        # Signed mode keeps its own wording when both apply.
        result, _ = self.run_verify({"REQUESTED_UNSIGNED": "false", **SIGNED, "HAS_UPDATE_KEY": "false", "UPDATER": "all"})
        self.assertRejected(result, "signed mode requires UPDATE_SIGNING_KEY")

    def test_rel_083_rejects_unknown_value(self):
        for value in ("yes", "ALL", "true", "None", " all", "1", "all;none"):
            with self.subTest(variable=value):
                result, _ = self.run_verify({"UPDATER": value, "HAS_UPDATE_KEY": "true"})
                self.assertRejected(result, "the repository variable UPDATER must be none or all")
        # An invalid variable fails even when the input would override it, and on a tag.
        for env in ({"REQUESTED_UPDATER": "none"}, self.unsigned_tag(), self.tag()):
            with self.subTest(env=env):
                result, _ = self.run_verify({**env, "UPDATER": "bogus", "HAS_UPDATE_KEY": "true"})
                self.assertRejected(result, "the repository variable UPDATER must be none or all")
        for value in ("yes", "All", "unsigned", "true"):
            with self.subTest(input=value):
                result, _ = self.run_verify({"REQUESTED_UPDATER": value, "HAS_UPDATE_KEY": "true"})
                self.assertRejected(result, "the updater input must be inherit, none or all")
        # An invalid input on a tag fails too (there it must be `inherit`).
        result, _ = self.run_verify(self.unsigned_tag(EVENT_NAME="workflow_dispatch", REQUESTED_UPDATER="bogus"))
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        # An empty variable is none, not an error.
        self.assertChannel({"UPDATER": ""}, "none", "false", "true", label="empty variable")

    def test_rel_083_tag_requires_inherit(self):
        message = "the updater input must be 'inherit' on a version tag"
        for requested in ("all", "none"):
            for label, base in (("unsigned tag", self.unsigned_tag()), ("signed tag", self.tag())):
                with self.subTest(requested=requested, tag=label):
                    env = {**base, "EVENT_NAME": "workflow_dispatch", "REQUESTED_UPDATER": requested, "HAS_UPDATE_KEY": "true"}
                    result, _ = self.run_verify(env)
                    self.assertRejected(result, message)
                    # Even when it equals the variable: on a tag the variable is the only source.
                    result, _ = self.run_verify({**env, "UPDATER": requested})
                    self.assertRejected(result, message)
        # `inherit` and a push (which has no inputs, so an empty one) use the variable.
        for requested in ("inherit", ""):
            env = {**self.unsigned_tag(UPDATER="all", HAS_UPDATE_KEY="true"), "REQUESTED_UPDATER": requested}
            self.assertChannel(env, "all", "true", "true", label=f"tag, input {requested!r}")
        self.assertChannel(
            {**self.unsigned_tag(HAS_UPDATE_KEY="true"), "EVENT_NAME": "workflow_dispatch", "REQUESTED_UPDATER": "inherit", "UPDATER": "all"},
            "all",
            "true",
            "true",
            label="tag dispatch, inherit",
        )
        # On a branch the input is honoured.
        for requested in ("all", "none"):
            self.assertChannel(
                {"REQUESTED_UPDATER": requested, "HAS_UPDATE_KEY": "true"},
                requested,
                "true" if requested == "all" else "false",
                "true",
                label=f"branch, input {requested}",
            )

    def test_rel_031_unsigned_updater_all_needs_no_signpath(self):
        env = {"UPDATER": "all", "HAS_UPDATE_KEY": "true", "WINDOWS_SIGNING": "none", "HAS_SIGNPATH": "false"}
        for base in ({}, self.unsigned_tag()):  # a branch rehearsal and an unsigned tag
            outputs = self.assertChannel({**base, **env}, "all", "true", "true", label=str(base))
            self.assertEqual(outputs["macos_signed"], "false")
        # Provider credentials and a signer pin change nothing while OS signing is off.
        outputs = self.assertChannel(
            {**self.unsigned_tag(), **SIGNED, "UPDATER": "all", "MACOS_SIGNING": "apple", "HAS_APPLE": "true"},
            "all",
            "true",
            "true",
            label="credentials present, OS signing off",
        )
        self.assertEqual(outputs["macos_signed"], "false")

    def test_rel_031_signed_updater_none_still_needs_update_key(self):
        env = {"REQUESTED_UNSIGNED": "false", **SIGNED, "UPDATER": "none"}
        result, _ = self.run_verify({**env, "HAS_UPDATE_KEY": "false"})
        self.assertRejected(result, "signed mode requires UPDATE_SIGNING_KEY")
        for keys in (KEYS_EMPTY, KEYS_COMMENTED):
            result, _ = self.run_verify(env, keys=keys)
            self.assertRejected(result, "signed mode requires a public key in crates/dk-update/src/keys.rs")
        result, _ = self.run_verify({**self.tag(UPDATER="none"), "HAS_UPDATE_KEY": "false"})
        self.assertRejected(result, "signed mode requires UPDATE_SIGNING_KEY")
        self.assertChannel(env, "none", "true", "false", label="signed, updater none, key present")

    def test_rel_032_manifest_checked_whenever_sign_manifest(self):
        # The run fails without the key and the public key whenever sign_manifest is true ...
        modes = {
            "signed, updater none": {"REQUESTED_UNSIGNED": "false", **SIGNED, "UPDATER": "none"},
            "unsigned, updater all": {"UPDATER": "all", "HAS_UPDATE_KEY": "true"},
            "signed, updater all": {"REQUESTED_UNSIGNED": "false", **SIGNED, "UPDATER": "all"},
        }
        for label, env in modes.items():
            with self.subTest(label):
                result, outputs = self.run_verify(env)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(outputs["sign_manifest"], "true")
                result, _ = self.run_verify({**env, "HAS_UPDATE_KEY": "false"})
                self.assertRejected(result, "requires UPDATE_SIGNING_KEY")
                result, _ = self.run_verify(env, keys=KEYS_EMPTY)
                self.assertRejected(result, "requires a public key in crates/dk-update/src/keys.rs")
        # ... and `assemble` signs and verifies the manifest on that output alone.
        for prefix in ("Sign the update manifest", "Manifest signature verifies against the app's public keys"):
            with self.subTest(prefix):
                condition = step_condition(find_step("assemble", prefix))
                self.assertEqual(condition, "needs.verify.outputs.sign_manifest == 'true'")


@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required")
class ReleaseEvidenceTests(unittest.TestCase):
    """REL-084/086: scripts/release-evidence.sh with fake release binaries."""

    def run_script(self, *args, cwd=None):
        converted = [shell_path(arg) if isinstance(arg, Path) else arg for arg in args]
        return subprocess.run(
            [BASH, "--noprofile", "--norc", shell_path(EVIDENCE_SCRIPT), *converted],
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=60,
        )

    @staticmethod
    def fake_binary(updater=False, seam=False, filler=3000):
        body = b"\x7fELF\x02\x01\x01\x00\xff\xfe\n\x00" * (filler // 12)
        if updater:
            body += b"\x00unrelated\n" + UPDATER_MARKER + b"\x00\x80tail"
        if seam:
            body += b"\xc3\x28\n" + SEAM_MARKER + b"\x00"
        return body + bytes(range(256))

    def scan(self, root, target, **markers):
        binary = root / "bin" / target
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(self.fake_binary(**markers))
        evidence = root / "artifacts" / f"release-package-{target}" / f"updater-evidence-{target}.txt"
        result = self.run_script("scan", binary, evidence)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return evidence

    def bundle(self, root, per_target=None):
        """Scan one fake binary per target; per_target maps target -> markers dict."""
        per_target = per_target or {}
        for target in TARGETS:
            self.scan(root, target, **per_target.get(target, {}))
        return root / "artifacts"

    def write_bundle(self, root, per_target=None):
        """Write the evidence files directly (fast); per_target maps target -> (updater, seam)."""
        per_target = per_target or {}
        for target in TARGETS:
            updater, seam = per_target.get(target, (False, False))
            evidence = root / "artifacts" / f"release-package-{target}" / f"updater-evidence-{target}.txt"
            evidence.parent.mkdir(parents=True, exist_ok=True)
            evidence.write_text(
                f"updater={str(updater).lower()}\nseam={str(seam).lower()}\n", encoding="utf-8", newline="\n"
            )
        return root / "artifacts"

    def test_rel_084_scan_reports_both_markers_from_the_binary(self):
        with tempfile.TemporaryDirectory(prefix="dockering-evidence-") as temp:
            root = Path(temp)
            cases = {
                (False, False): "updater=false\nseam=false\n",
                (True, False): "updater=true\nseam=false\n",
                (False, True): "updater=false\nseam=true\n",
                (True, True): "updater=true\nseam=true\n",
            }
            for (updater, seam), expected in cases.items():
                with self.subTest(updater=updater, seam=seam):
                    evidence = self.scan(root, "x86_64-unknown-linux-gnu", updater=updater, seam=seam)
                    self.assertEqual(evidence.read_text(encoding="utf-8"), expected)
            # A near miss is not a match; the scan needs the exact strings.
            binary = root / "near-miss"
            binary.write_bytes(UPDATER_MARKER[:-1] + b"\n" + SEAM_MARKER[:-1] + b"\n")
            evidence = root / "near-miss.txt"
            self.assertEqual(self.run_script("scan", binary, evidence).returncode, 0)
            self.assertEqual(evidence.read_text(encoding="utf-8"), "updater=false\nseam=false\n")

    def test_rel_084_scan_handles_a_large_binary_and_rejects_a_missing_one(self):
        with tempfile.TemporaryDirectory(prefix="dockering-evidence-") as temp:
            root = Path(temp)
            binary = root / "dockering.exe"
            binary.write_bytes(bytes(range(256)) * 16384 + UPDATER_MARKER)  # 4 MiB, marker last
            evidence = root / "out" / "deep" / "updater-evidence.txt"
            result = self.run_script("scan", binary, evidence)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(evidence.read_text(encoding="utf-8"), "updater=true\nseam=false\n")
            for bad in (root / "missing", root):
                result = self.run_script("scan", bad, root / "never.txt")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("is missing or empty", result.stdout + result.stderr)
            (root / "empty").write_bytes(b"")
            self.assertNotEqual(self.run_script("scan", root / "empty", root / "never.txt").returncode, 0)
            self.assertFalse((root / "never.txt").exists())

    def test_rel_084_evidence_agrees_with_updater_output(self):
        with tempfile.TemporaryDirectory(prefix="dockering-evidence-") as temp:
            root = Path(temp)
            with_updater = {target: {"updater": True} for target in TARGETS}
            without = {target: {"updater": False} for target in TARGETS}
            artifacts = self.bundle(root / "all", with_updater)
            result = self.run_script("check", artifacts, "all")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotEqual(self.run_script("check", artifacts, "none").returncode, 0)
            artifacts_none = self.bundle(root / "none", without)
            result = self.run_script("check", artifacts_none, "none")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotEqual(self.run_script("check", artifacts_none, "all").returncode, 0)
            # One target that disagrees fails the run and is named, for either channel.
            for channel, updater_value in (("all", True), ("none", False)):
                for target in TARGETS:
                    with self.subTest(channel=channel, target=target):
                        mixed = {t: (updater_value, False) for t in TARGETS}
                        mixed[target] = (not updater_value, False)
                        bundle = self.write_bundle(root / f"{channel}-{target}", mixed)
                        result = self.run_script("check", bundle, channel)
                        self.assertNotEqual(result.returncode, 0)
                        text = result.stdout + result.stderr
                        self.assertIn(f"{target}: the binary has updater={str(not updater_value).lower()}", text)
                        self.assertIn(f"the release channel is updater={channel}", text)
                        # Exactly that target is reported.
                        self.assertEqual(text.count("::error::"), 1, text)
            # Only none|all are channels.
            result = self.run_script("check", artifacts, "inherit")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must be none or all", result.stdout + result.stderr)

    def test_rel_084_check_needs_all_six_well_formed_files(self):
        with tempfile.TemporaryDirectory(prefix="dockering-evidence-") as temp:
            root = Path(temp)
            artifacts = self.bundle(root, {target: {"updater": True} for target in TARGETS})
            first = TARGETS[0]
            path = artifacts / f"release-package-{first}" / f"updater-evidence-{first}.txt"
            good = path.read_text(encoding="utf-8")
            # A Windows runner's CRLF is tolerated.
            path.write_text(good.replace("\n", "\r\n"), encoding="utf-8", newline="")
            result = self.run_script("check", artifacts, "all")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            for label, content in (
                ("empty", ""),
                ("one line", "updater=true\n"),
                ("swapped", "seam=false\nupdater=true\n"),
                ("extra line", "updater=true\nseam=false\nmore=1\n"),
                ("not a bool", "updater=yes\nseam=false\n"),
                ("unknown key", "updater=true\nseams=false\n"),
            ):
                with self.subTest(label):
                    path.write_text(content, encoding="utf-8", newline="")
                    result = self.run_script("check", artifacts, "all")
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(f"malformed updater evidence for {first}", result.stdout + result.stderr)
            # Every one of the six files is required, and a missing artifact directory is reported.
            path.write_text(good, encoding="utf-8", newline="")
            for target in TARGETS:
                with self.subTest(missing=target):
                    evidence = artifacts / f"release-package-{target}" / f"updater-evidence-{target}.txt"
                    saved = evidence.read_text(encoding="utf-8")
                    evidence.unlink()
                    result = self.run_script("check", artifacts, "all")
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(f"missing updater evidence for {target}", result.stdout + result.stderr)
                    evidence.write_text(saved, encoding="utf-8", newline="")
            shutil.rmtree(artifacts)
            result = self.run_script("check", artifacts, "all")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((result.stdout + result.stderr).count("missing updater evidence"), 6)

    def test_rel_086_release_binary_has_no_seam(self):
        with tempfile.TemporaryDirectory(prefix="dockering-evidence-") as temp:
            root = Path(temp)
            for channel, updater in (("all", True), ("none", False)):
                for target in TARGETS:
                    with self.subTest(channel=channel, target=target):
                        per_target = {t: (updater, False) for t in TARGETS}
                        per_target[target] = (updater, True)
                        bundle = self.write_bundle(root / f"{channel}-{target}", per_target)
                        # The updater line agrees, so only the seam can make this fail.
                        result = self.run_script("check", bundle, channel)
                        self.assertNotEqual(result.returncode, 0)
                        text = result.stdout + result.stderr
                        self.assertIn(
                            f"{target}: the release binary contains the update-e2e test seam (REL-086)", text
                        )
                        self.assertEqual(text.count("::error::"), 1, text)
            # End to end: a scanned binary that carries the seam marker fails the check; a clean one passes.
            seamed = self.bundle(root / "scanned-seam", {t: {"updater": True, "seam": t == TARGETS[3]} for t in TARGETS})
            result = self.run_script("check", seamed, "all")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(f"{TARGETS[3]}: the release binary contains the update-e2e test seam", result.stdout)
            clean = self.bundle(root / "scanned-clean", {t: {"updater": True} for t in TARGETS})
            result = self.run_script("check", clean, "all")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("none contains the test seam", result.stdout)

    def test_rel_084_package_step_writes_evidence_from_the_unpackaged_binary(self):
        script = step_script_of(find_step("package", "Record updater evidence"))
        self.assertIn("scripts/release-evidence.sh", script)
        for runner_os, exe in (("Linux", "dockering"), ("macOS", "dockering"), ("Windows", "dockering.exe")):
            for updater in (True, False):
                with self.subTest(runner_os=runner_os, updater=updater), tempfile.TemporaryDirectory(
                    prefix="dockering-evidence-"
                ) as temp:
                    root = Path(temp)
                    target = "x86_64-pc-windows-msvc"
                    (root / "scripts").mkdir()
                    shutil.copy(EVIDENCE_SCRIPT, root / "scripts/release-evidence.sh")
                    binary = root / "target" / target / "release" / exe
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(self.fake_binary(updater=updater))
                    # Packaged output next to it must never be scanned instead.
                    (root / "target" / target / "release" / "other").write_bytes(self.fake_binary(updater=not updater))
                    (root / "step.sh").write_text(script, encoding="utf-8", newline="\n")
                    environment = os.environ.copy()
                    environment.update(RUNNER_OS=runner_os, TARGET=target)
                    result = subprocess.run(
                        [BASH, "--noprofile", "--norc", "-eo", "pipefail", shell_path(root / "step.sh")],
                        cwd=root,
                        env=environment,
                        capture_output=True,
                        text=True,
                        timeout=60,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    evidence = root / "target/dist" / target / f"updater-evidence-{target}.txt"
                    self.assertEqual(
                        evidence.read_text(encoding="utf-8"),
                        f"updater={str(updater).lower()}\nseam=false\n",
                    )


@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required")
@unittest.skipIf(sys.platform == "darwin", "the workflow runs these steps on Ubuntu's Bash")
class ReleaseStepScriptTests(unittest.TestCase):
    """Run the `package` build step and the `assemble` notes step as GitHub would."""

    def run_step(self, step, env, cwd, *, mock_bin=None):
        script = Path(cwd) / "step.sh"
        script.write_text(step_script_of(step), encoding="utf-8", newline="\n")
        environment = os.environ.copy()
        environment.update(env)
        environment["STEP_SCRIPT"] = shell_path(script)
        prefix = ""
        if mock_bin is not None:
            environment["MOCK_BIN"] = shell_path(mock_bin)
            prefix = 'export PATH="$MOCK_BIN:$PATH"; '
        return subprocess.run(
            [BASH, "--noprofile", "--norc", "-c", prefix + 'bash --noprofile --norc -eo pipefail "$STEP_SCRIPT"'],
            cwd=cwd,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
        )

    def test_upd_005_feature_follows_updater_output(self):
        step = find_step("package", "cargo build --release")
        for updater, expected in (("all", ["--features", "updater"]), ("none", []), ("", [])):
            for target in TARGETS:
                with self.subTest(updater=updater, target=target), tempfile.TemporaryDirectory(
                    prefix="dockering-build-test-"
                ) as temp:
                    root = Path(temp)
                    bin_dir = root / "bin"
                    bin_dir.mkdir()
                    shim = bin_dir / "cargo"
                    shim.write_text(
                        '#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$ARGS_FILE"\n', encoding="utf-8", newline="\n"
                    )
                    shim.chmod(0o755)
                    args_file = root / "args.txt"
                    result = self.run_step(
                        step,
                        {
                            "UPDATER": updater,
                            "TARGET": target,
                            "ARGS_FILE": shell_path(args_file),
                            # What the old selection read; it must have no effect any more.
                            "PUBLIC_RELEASES": "true",
                        },
                        root,
                        mock_bin=bin_dir,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    args = args_file.read_text(encoding="utf-8").split("\n")[:-1]
                    self.assertEqual(
                        args,
                        ["build", "--release", "--locked", "-p", "dockering", "--target", target, *expected],
                    )
                    self.assertNotIn("update-e2e", " ".join(args))

    CHANGELOG = "# Changelog\n\n## [0.3.0] - 2026-01-01\n\n- Something.\n\n## [0.2.0] - 2025-12-01\n\n- Older.\n"

    def notes(self, **env):
        step = find_step("assemble", "Release notes from CHANGELOG.md")
        with tempfile.TemporaryDirectory(prefix="dockering-notes-test-") as temp:
            root = Path(temp)
            (root / "dist").mkdir()
            (root / "CHANGELOG.md").write_text(self.CHANGELOG, encoding="utf-8")
            base = {
                "VERSION": "0.3.0",
                "UNSIGNED": "true",
                "MACOS_SIGNED": "false",
                "UPDATER": "none",
                "SIGN_MANIFEST": "false",
                "WINDOWS_SIGNING": "none",
                "PUBLIC_REPOSITORY": "true",
                "GITHUB_REPOSITORY": "pavel-purma/dockering",
            }
            result = self.run_step(step, {**base, **env}, root)
            notes = root / "dist/RELEASE_NOTES.md"
            return result, notes.read_text(encoding="utf-8") if notes.exists() else ""

    ALL_WORDING = (
        "Dockering checks for updates in these builds",
        "the update manifest is signed with the project's update key",
        "Windows installs made with the installer (Inno) and Linux AppImages in a writable location can install an update from within the app after a click",
        "macOS and every other install (portable zip, .deb, .tar.gz, read-only locations) show a Download button",
    )
    NONE_WORDING = "Automatic updates are not included in these builds"
    UNSIGNED_OS = (
        "**Unsigned release:** Windows files are not Authenticode-signed; macOS files are not "
        "Developer ID-signed or notarised. Windows SmartScreen and macOS Gatekeeper may warn or block launch."
    )

    def test_rel_083_release_notes_state_the_channel(self):
        cases = {
            "unsigned, updater all": ({"UNSIGNED": "true", "UPDATER": "all", "SIGN_MANIFEST": "true"}, "all", True),
            "unsigned, updater none": ({"UNSIGNED": "true", "UPDATER": "none", "SIGN_MANIFEST": "false"}, "none", False),
            "signed, updater none": (
                {"UNSIGNED": "false", "UPDATER": "none", "SIGN_MANIFEST": "true", "WINDOWS_SIGNING": "signpath"},
                "none",
                True,
            ),
            "signed, updater all": (
                {"UNSIGNED": "false", "UPDATER": "all", "SIGN_MANIFEST": "true", "WINDOWS_SIGNING": "signpath"},
                "all",
                True,
            ),
        }
        for label, (env, updater, signed_manifest) in cases.items():
            with self.subTest(label):
                result, notes = self.notes(**env)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("- Something.", notes)
                self.assertNotIn("- Older.", notes)
                self.assertNotIn("Automatic updates are disabled", notes)
                if env["UNSIGNED"] == "true":
                    # The OS-signing statement is the one the notes always had.
                    self.assertIn(self.UNSIGNED_OS, notes)
                    self.assertNotIn("**Signed release:**", notes)
                else:
                    self.assertIn("**Signed release:** `dockering.exe` and the Windows installers are Authenticode-signed.", notes)
                    self.assertNotIn("**Unsigned release:**", notes)
                    self.assertIn("macOS files are not Developer ID-signed or notarised; Gatekeeper may warn or block launch.", notes)
                if updater == "all":
                    for wording in self.ALL_WORDING:
                        self.assertIn(wording, notes)
                    self.assertNotIn(self.NONE_WORDING, notes)
                else:
                    self.assertIn(self.NONE_WORDING, notes)
                    self.assertNotIn("Dockering checks for updates", notes)
                    self.assertNotIn("Download button", notes)
                signed_sentence = "the update manifest is signed with the project's update key"
                self.assertEqual(signed_sentence in notes, signed_manifest)
                self.assertEqual("the update manifest has no signature" in notes, not signed_manifest)

    def test_rel_083_release_notes_reject_an_inconsistent_channel(self):
        for updater, sign_manifest in (("all", "false"), ("none", ""), ("", "true"), ("inherit", "true"), ("all", "")):
            with self.subTest(updater=updater, sign_manifest=sign_manifest):
                result, _ = self.notes(UPDATER=updater, SIGN_MANIFEST=sign_manifest)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("inconsistent update channel", result.stdout + result.stderr)


class ReleaseWorkflowStaticTests(unittest.TestCase):
    """The wiring of release.yml, read as text (no YAML parser needed)."""

    @staticmethod
    def code(lines):
        return "\n".join(line for line in lines if not line.lstrip().startswith("#"))

    def test_rel_083_package_reads_verify_outputs(self):
        # `verify` publishes the gate; `package` and `assemble` only read it.
        verify = "\n".join(job_lines("verify"))
        for output in ("updater", "sign_manifest", "unsigned", "macos_signed", "version", "prerelease"):
            self.assertIn(f"{output}: ${{{{ steps.v.outputs.{output} }}}}", verify)
        build = self.code(find_step("package", "cargo build --release"))
        self.assertIn("needs.verify.outputs.updater", build)
        self.assertIn("--features updater", build)
        for old in ("PUBLIC_RELEASES", "vars.", "unsigned"):
            self.assertNotIn(old, build, "the feature selection must not read OS signing or the variable")
        # The updater is built for every target: no matrix entry or OS condition narrows the build step.
        self.assertIsNone(step_condition(find_step("package", "cargo build --release")))
        # PUBLIC_RELEASES decides OS signing only: it never appears in `package`, and in `assemble`
        # only in the REL-031 signed-channel check.
        self.assertNotIn("PUBLIC_RELEASES", self.code(job_lines("package")))
        users = [
            step_name(step)
            for step in job_steps("assemble")
            if "PUBLIC_RELEASES" in self.code(step)
        ]
        self.assertEqual(users, ["Stable public releases must be signed (REL-031)"])
        rel_031 = find_step("assemble", "Stable public releases must be signed (REL-031)")
        self.assertIn("needs.verify.outputs.unsigned != 'true'", step_condition(rel_031))
        self.assertNotIn("sign_manifest", step_condition(rel_031))
        # assemble reads both outputs.
        assemble = self.code(job_lines("assemble"))
        self.assertIn("needs.verify.outputs.updater", assemble)
        self.assertIn("needs.verify.outputs.sign_manifest", assemble)
        notes = self.code(find_step("assemble", "Release notes from CHANGELOG.md"))
        self.assertIn("UPDATER: ${{ needs.verify.outputs.updater }}", notes)
        self.assertIn("SIGN_MANIFEST: ${{ needs.verify.outputs.sign_manifest }}", notes)
        # No job re-derives the gate from the repository variable.
        for job in ("package", "assemble", "draft"):
            self.assertNotIn("vars.UPDATER", self.code(job_lines(job)))
            self.assertNotIn("inputs.updater", self.code(job_lines(job)))

    def test_rel_017_minisig_follows_sign_manifest(self):
        # The signature file exists exactly when sign_manifest is true; no other step creates it.
        sign = find_step("assemble", "Sign the update manifest")
        self.assertIn("sign-manifest", step_script_of(sign))
        self.assertEqual(step_condition(sign), "needs.verify.outputs.sign_manifest == 'true'")
        for step in job_steps("assemble"):
            if step_name(step) != step_name(sign):
                self.assertNotIn("sign-manifest", self.code(step), step_name(step))
        # Only the REL-031 channel check still reads the OS signing mode in `assemble`.
        for step in job_steps("assemble"):
            if "needs.verify.outputs.unsigned" in self.code(step) and "UNSIGNED" not in self.code(step):
                self.assertEqual(step_name(step), "Stable public releases must be signed (REL-031)")

    def test_rel_083_updater_input_and_description(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        start = text.index("  workflow_dispatch:")
        block = text[start : text.index("\npermissions:", start)]
        unsigned = block[block.index("      unsigned:") : block.index("      updater:")]
        self.assertNotIn("without the updater", unsigned)
        self.assertIn("OS code signing", unsigned)
        updater = block[block.index("      updater:") :]
        for fragment in ("type: choice", "default: inherit", "- inherit", "- none", "- all"):
            self.assertIn(fragment, updater)
        # Header comment documents both new controls.
        header = text[: text.index("name: Release")]
        self.assertIn("UPDATER", header)
        self.assertIn("updater    inherit (default) | none | all", header)

    def test_rel_041_winget_requires_authenticode_signed_installers(self):
        # A .minisig is also shipped by OS-unsigned builds with UPDATER=all (REL-083), so it must not be
        # winget's only signed-release test: the installers' own Authenticode signature decides.
        text = (ROOT / ".github/workflows/winget.yml").read_text(encoding="utf-8")
        self.assertIn("Get-AuthenticodeSignature", text)
        self.assertIn("-ne 'Valid'", text)
        for name in ("Dockering-Setup-x64.exe", "Dockering-Setup-arm64.exe"):
            self.assertIn(name, text.split("Get-AuthenticodeSignature")[0])
        self.assertIn("vars.PUBLIC_RELEASES == 'true'", text)

    def test_rel_086_release_workflow_lists_no_seam_feature(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        code = self.code(text.split("\n"))
        self.assertNotIn("update-e2e", code)
        features = re.findall(r"--features[= ]+([A-Za-z0-9_,/-]+)", code)
        self.assertEqual(features, ["updater"], "the only feature a release build may select is `updater`")
        for flag in ("--all-features", "--no-default-features"):
            self.assertNotIn(flag, code)
        # The seam marker is scanned for, never enabled: its string lives in the script, not the workflow.
        self.assertNotIn(SEAM_MARKER.decode(), code)

    def test_rel_084_workflow_wires_the_evidence(self):
        record = find_step("package", "Record updater evidence")
        build = find_step("package", "cargo build --release")
        names = [step_name(step) for step in job_steps("package")]
        self.assertLess(names.index(step_name(build)), names.index(step_name(record)))
        # It runs before any signing or packaging step touches the binary.
        for early in ("Sign dockering.exe", "cargo xtask package", "Package macOS"):
            self.assertLess(
                names.index(step_name(record)),
                next(i for i, name in enumerate(names) if (name or "").startswith(early)),
                early,
            )
        self.assertIsNone(step_condition(record))
        # The per-target artifact carries the evidence file; the review bundle and `dist` do not.
        uploads = [step for step in job_steps("package") if "actions/upload-artifact" in "".join(step) and "release-package-" in "".join(step)]
        self.assertEqual(len(uploads), 1)
        self.assertIn("target/dist/${{ matrix.target }}/updater-evidence-${{ matrix.target }}.txt", "\n".join(uploads[0]))
        collect = step_script_of(find_step("assemble", "Collect assets"))
        self.assertIn("-name 'Dockering-*'", collect)
        check = find_step("assemble", "Updater evidence agrees with the update channel")
        self.assertIn('release-evidence.sh check artifacts "$UPDATER"', step_script_of(check))
        self.assertIn("needs.verify.outputs.updater", "\n".join(check))
        assemble_names = [step_name(step) for step in job_steps("assemble")]
        self.assertLess(assemble_names.index(step_name(check)), assemble_names.index("Update manifest (UPD-002)"))
        review = find_step("assemble", "Upload complete release review artifact")
        self.assertIn("path: dist/*", "\n".join(review))
        evidence = find_step("assemble", "Upload updater evidence")
        text = "\n".join(evidence)
        self.assertIn("path: artifacts/release-package-*/updater-evidence-*.txt", text)
        self.assertIn("name: updater-evidence-${{ needs.verify.outputs.version }}", text)
        # The draft job publishes only the review bundle.
        draft = self.code(job_lines("draft"))
        self.assertIn("name: release-review-${{ needs.verify.outputs.version }}", draft)
        self.assertNotIn("evidence", draft)

    def test_workflow_parses_as_yaml(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML is not installed")
        data = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        self.assertEqual(set(data["jobs"]), {"verify", "package", "assemble", "draft"})
        inputs = (data.get("on") or data[True])["workflow_dispatch"]["inputs"]
        self.assertEqual(inputs["updater"]["options"], ["inherit", "none", "all"])
        self.assertEqual(inputs["updater"]["default"], "inherit")
        self.assertEqual(inputs["unsigned"]["type"], "boolean")
        for job in data["jobs"].values():
            for step in job["steps"]:
                self.assertTrue("run" in step or "uses" in step, step)
        self.assertEqual(
            data["jobs"]["verify"]["outputs"]["updater"], "${{ steps.v.outputs.updater }}"
        )


if __name__ == "__main__":
    unittest.main()
