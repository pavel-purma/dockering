"""REL-080/081/082: the `CI gate` job in ci.yml and the rules that keep it a reliable merge gate.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
The workflows are read as text (no YAML parser, so the test runs on every runner). The gate's step
script is extracted and run the way GitHub runs `shell: bash` (`bash --noprofile --norc -eo pipefail`).
"""

import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
CI = ROOT / ".github/workflows/ci.yml"
PR_TITLE = ROOT / ".github/workflows/pr-title.yml"
SPEC = ROOT / "docs/spec/50-build-and-release.md"
BASH = (
    str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
    if os.name == "nt"
    else shutil.which("bash")
)
REQUIRED_CHECKS = ("CI gate", "conventional commit title")


def lines(path):
    return path.read_text(encoding="utf-8").split("\n")


def ci_jobs():
    """Map job id -> its lines, from the `jobs:` key to the end of ci.yml."""
    text = lines(CI)
    jobs = {}
    current = None
    for line in text[text.index("jobs:") + 1 :]:
        header = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if header:
            current = header.group(1)
            jobs[current] = []
        elif current is not None:
            jobs[current].append(line)
    return jobs


def job_level(job_lines, key):
    """The value of a job-level key (four spaces of indent), or None."""
    for line in job_lines:
        found = re.match(rf"^    {re.escape(key)}:\s*(.*?)\s*$", line)
        if found:
            return found.group(1)
    return None


def gate_script():
    gate = ci_jobs()["gate"]
    start = next(i for i, line in enumerate(gate) if "Every CI job succeeded" in line)
    run = next(i for i in range(start, len(gate)) if gate[i].strip() == "run: |")
    body = []
    for line in gate[run + 1 :]:
        if line.strip() and not line.startswith(" " * 10):
            break
        body.append(line[10:])
    return "\n".join(body)


class GateJobTests(unittest.TestCase):
    def test_rel_081_gate_is_named_ci_gate(self):
        self.assertEqual(job_level(ci_jobs()["gate"], "name"), "CI gate")

    def test_rel_081_gate_runs_always(self):
        self.assertEqual(job_level(ci_jobs()["gate"], "if"), "always()")

    def test_rel_081_gate_needs_every_job(self):
        jobs = ci_jobs()
        needs = job_level(jobs["gate"], "needs")
        self.assertIsNotNone(needs, "gate has no single-line `needs: [...]`")
        listed = {name.strip() for name in needs.strip("[]").split(",") if name.strip()}
        others = set(jobs) - {"gate"}
        self.assertEqual(listed, others, "gate must need every other job in ci.yml")


@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required")
class GateStepTests(unittest.TestCase):
    def run_step(self, results):
        with tempfile.TemporaryDirectory(prefix="dockering-gate-test-") as temp:
            script = Path(temp) / "gate.sh"
            script.write_text(gate_script(), encoding="utf-8", newline="\n")
            environment = os.environ.copy()
            environment["RESULTS"] = results
            return subprocess.run(
                [BASH, "--noprofile", "--norc", "-eo", "pipefail", str(script)],
                env=environment,
                capture_output=True,
                text=True,
            )

    def test_rel_081_gate_step_requires_success(self):
        passing = self.run_step("success success success success")
        self.assertEqual(passing.returncode, 0, passing.stdout + passing.stderr)
        for results in (
            "success failure success success",
            "success cancelled success success",
            "success skipped success success",
            "",
        ):
            with self.subTest(results=results):
                self.assertNotEqual(self.run_step(results).returncode, 0)


class AlwaysReportedTests(unittest.TestCase):
    def test_rel_082_no_job_level_if(self):
        for name, job in ci_jobs().items():
            if name == "gate":
                continue
            with self.subTest(job=name):
                self.assertIsNone(job_level(job, "if"), "a skipped job never reports a check")

    def test_rel_082_no_pull_request_path_filters(self):
        for path in (CI, PR_TITLE):
            text = lines(path)
            end = next(i for i, line in enumerate(text) if re.match(r"^[A-Za-z_-]+:", line) and i > text.index("on:"))
            with self.subTest(workflow=path.name):
                for line in text[text.index("on:") : end]:
                    self.assertNotRegex(line, r"^\s+(paths|paths-ignore):")

    def test_rel_080_required_names_are_job_names(self):
        self.assertEqual(job_level(ci_jobs()["gate"], "name"), REQUIRED_CHECKS[0])
        self.assertIn(f"name: {REQUIRED_CHECKS[1]}", "\n".join(lines(PR_TITLE)))
        spec = SPEC.read_text(encoding="utf-8")
        for check in REQUIRED_CHECKS:
            with self.subTest(check=check):
                self.assertIn(f"`{check}`", spec)


if __name__ == "__main__":
    unittest.main()
