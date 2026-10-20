"""REL-085, REL-086: `linux-update-e2e.yml` is read as text (no YAML parser, so the test runs on every runner) and checked
against the rules of the plan (docs/plan/features/linux-appimage-updates.md §6.5): read-only token, no secret, no environment,
not a required check, a paths filter, a weekly run and a manual one, a build identical to the release's, run.sh as the one
entry point, and no AppImage uploaded as an artifact.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
"""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/linux-update-e2e.yml"
RELEASE = ROOT / ".github/workflows/release.yml"
CI = ROOT / ".github/workflows/ci.yml"


def read(path):
    return Path(path).read_text(encoding="utf-8")


def without_comments(text):
    return "\n".join(re.sub(r"(^|\s)#.*$", "", line) for line in text.split("\n"))


def env_value(text, name):
    match = re.search(rf'^\s+{name}:\s*"?([^"\n#]+?)"?\s*(?:#.*)?$', text, flags=re.M)
    return match.group(1) if match else None


class WorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = read(WORKFLOW)
        cls.code = without_comments(cls.text)

    def test_rel_085_the_token_is_read_only_and_there_is_no_secret_or_environment(self):
        self.assertRegex(self.code, r"(?m)^permissions:\n  contents: read\n(?!  )")
        self.assertEqual(len(re.findall(r"(?m)^\s*permissions:", self.code)), 1, "no job widens the permissions")
        self.assertNotRegex(self.code, r"(?m)^\s+environment:", "no deployment environment, so no environment secret")
        self.assertNotIn("secrets.", self.code)
        self.assertNotIn("id-token", self.code)
        self.assertNotIn("pull_request_target", self.code, "a pull request's code runs with a read-only token and no secret")
        self.assertNotRegex(self.code, r"(?m)^\s+(contents|actions|packages|attestations): write")

    def test_rel_085_it_runs_on_pull_requests_that_touch_the_updater_weekly_and_on_demand(self):
        on = self.code[self.code.index("\non:") : self.code.index("\npermissions:")]
        self.assertIn("pull_request:", on)
        self.assertIn("schedule:", on)
        self.assertIn("workflow_dispatch:", on)
        paths = re.findall(r'^\s+- "([^"]+)"', on, flags=re.M)
        for needed in (
            "crates/dk-update/**",
            "crates/dk-hub/src/single_instance.rs",
            "crates/dockering/src/shell/**",
            "crates/dockering/src/state/updates.rs",
            "scripts/linux-update/**",
            "xtask/src/**",
            ".github/workflows/linux-update-e2e.yml",
        ):
            self.assertIn(needed, paths)
        self.assertTrue(any(p.startswith("crates/dk-hub/src/updates") for p in paths))
        self.assertRegex(on, r'cron: "\d+ \d+ \* \* \d"', "weekly")

    def test_rel_085_it_is_not_a_required_check(self):
        ci = without_comments(read(CI))
        self.assertNotIn("linux-update-e2e", ci, "ci.yml's gate must not depend on it (REL-080)")
        self.assertNotIn("Linux update e2e", ci)

    def test_rel_085_the_build_matches_the_release_build(self):
        release = read(RELEASE)
        for name in ("RUST_TOOLCHAIN", "PACKAGER_VERSION"):
            self.assertEqual(env_value(self.code, name), env_value(release, name), f"{name} follows release.yml")
        self.assertEqual(env_value(self.code, "RUST_TOOLCHAIN"), re.search(r'channel = "([^"]+)"', read(ROOT / "rust-toolchain.toml")).group(1))
        self.assertIn('cargo install cargo-packager --locked --version "$PACKAGER_VERSION"', self.code)
        self.assertIn("bash scripts/bootstrap.sh", self.code, "the same GPUI Kit prerequisites as the release")
        # the two AppImages come from the one script that mirrors the release's calls (and checks the seam marker)
        self.assertIn('scripts/linux-update/build-appimages.sh "$RUNNER_TEMP/appimages" --source current', self.code)
        self.assertIn("runs-on: ubuntu-24.04", self.code)

    def test_rel_085_run_sh_is_the_entry_point_in_a_clean_ubuntu_container(self):
        self.assertIn("E2E_IMAGE: mirror.gcr.io/library/ubuntu:24.04", self.code)
        self.assertIn('bash scripts/linux-update/run.sh "$E2E_IMAGE"', self.code)
        self.assertIn('--old "$RUNNER_TEMP/appimages/Dockering-old-x86_64.AppImage"', self.code)
        self.assertIn('--new "$RUNNER_TEMP/appimages/Dockering-x86_64.AppImage"', self.code)
        self.assertIn('--scenario "$SCENARIO"', self.code)

    def test_rel_085_the_manual_input_reaches_the_shell_only_through_the_environment(self):
        for block in re.findall(r"(?ms)^\s+run: \|\n(.*?)(?=^\s+- |^\s+\w+:|\Z)", self.code):
            self.assertNotIn("${{", block, "a `run:` script interpolates nothing; values go through `env:`")
        self.assertIn("SCENARIO: ${{ github.event_name == 'workflow_dispatch' && inputs.scenario || 'all' }}", self.code)
        options = re.search(r"options: \[([^\]]+)\]", self.code).group(1)
        self.assertEqual([o.strip() for o in options.split(",")], ["all", "s1", "s2", "s3", "s4"])

    def test_rel_085_the_evidence_is_uploaded_and_the_appimages_are_not(self):
        uploads = re.findall(r"(?ms)uses: actions/upload-artifact@\S+\n(.*?)(?=^\s+- |\Z)", self.code)
        self.assertEqual(len(uploads), 1)
        self.assertIn("path: ${{ runner.temp }}/e2e-out", uploads[0])
        self.assertIn("if-no-files-found: warn", uploads[0])
        self.assertRegex(uploads[0], r"retention-days: \d+")
        self.assertNotIn("AppImage", uploads[0], "the seam-built AppImage is an artifact of the run at most, never uploaded or released")
        self.assertNotIn("gh release", self.code)
        self.assertNotIn("softprops", self.code)

    def test_rel_085_the_checkout_does_not_keep_the_token(self):
        self.assertIn("persist-credentials: false", self.code)

    def test_rel_085_a_run_is_cancelled_by_a_newer_push_to_the_same_pull_request_only(self):
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", self.code)


if __name__ == "__main__":
    unittest.main()
