"""REL-017: exercise the macOS packaging recovery with mock cargo/hdiutil commands.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
No Apple tooling or real disk mounts are needed.
Set PACKAGE_MACOS_TEST_BASH=/bin/bash to exercise macOS's native Bash 3.2.
"""

import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "package-macos.sh"
BASH = os.environ.get("PACKAGE_MACOS_TEST_BASH") or (
    str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
    if os.name == "nt"
    else shutil.which("bash")
)


def shell_path(path):
    text = path.as_posix()
    if os.name == "nt":
        return "/" + text[0].lower() + text[2:]
    return text


@unittest.skipUnless(BASH and Path(BASH).is_file(), "bash is required")
class MacPackagingTests(unittest.TestCase):
    def exercise(self, mode, *, image_inside=True, device="/dev/disk4"):
        with tempfile.TemporaryDirectory(prefix="dockering-package-test-") as temp:
            root = Path(temp)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            output = root / "target/dist/x86_64-apple-darwin"
            output.mkdir(parents=True)
            image = output / "rw.Dockering_0.1.0_x64.dmg"
            if not image_inside:
                image = output.parent / "x86_64-apple-darwin-other/rw.unrelated.dmg"
            with (root / "image-info.plist").open("wb") as stream:
                plistlib.dump(
                    {"images": [{"image-path": str(image), "system-entities": [{"dev-entry": device}]}]},
                    stream,
                )
            commands = {
                "cargo": """#!/usr/bin/env bash
count=0
if [[ -f package-calls ]]; then count=$(cat package-calls); fi
count=$((count + 1))
echo "$count" > package-calls
if [[ "$CASE_KIND" == success || ( "$CASE_KIND" == recover && "$count" -gt 1 ) ]]; then
  echo packaged
  exit 0
fi
if [[ "$CASE_KIND" == generic ]]; then echo 'compiler error'; exit 7; fi
echo 'hdiutil: couldn'"'"'t eject "disk4" - Resource busy'
exit 7
""",
                "hdiutil": """#!/usr/bin/env bash
echo "$*" >> hdiutil-calls
if [[ "$1" == info ]]; then cat image-info.plist; exit 0; fi
if [[ "$CASE_KIND" == detach_failure ]]; then exit 8; fi
exit 0
""",
                "python3": '#!/usr/bin/env bash\nexec "' + Path(sys.executable).as_posix() + '" "$@"\n',
            }
            for name, content in commands.items():
                path = bin_dir / name
                path.write_text(content, encoding="utf-8", newline="\n")
                path.chmod(0o755)
            env = os.environ.copy()
            env.update(
                CASE_KIND=mode,
                MOCK_BIN=shell_path(bin_dir),
                PACKAGE_SCRIPT=shell_path(SCRIPT),
                PACKAGE_BASH=shell_path(Path(BASH)),
            )
            result = subprocess.run(
                [BASH, "--noprofile", "--norc", "-c", 'export PATH="$MOCK_BIN:$PATH"; "$PACKAGE_BASH" "$PACKAGE_SCRIPT" x86_64-apple-darwin'],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
            )
            calls_file = root / "package-calls"
            self.assertTrue(calls_file.is_file(), f"packaging did not invoke cargo: exit {result.returncode}; {result.stderr}")
            calls = int(calls_file.read_text())
            hdiutil = root / "hdiutil-calls"
            detaches = [line for line in hdiutil.read_text().splitlines() if line.startswith("detach ")] if hdiutil.exists() else []
            return result, calls, detaches

    def test_rel_017_script_parses_with_selected_bash(self):
        result = subprocess.run([BASH, "-n", str(SCRIPT)], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rel_017_success_needs_no_recovery(self):
        result, calls, detaches = self.exercise("success")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((calls, detaches), (1, []))

    def test_rel_017_busy_own_image_detached_and_retried(self):
        result, calls, detaches = self.exercise("recover")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, 2)
        self.assertEqual(detaches, ["detach /dev/disk4 -force"])

    def test_rel_017_generic_failure_preserved_without_retry(self):
        result, calls, detaches = self.exercise("generic")
        self.assertEqual(result.returncode, 7)
        self.assertEqual((calls, detaches), (1, []))

    def test_rel_017_outside_target_image_never_detached(self):
        result, calls, detaches = self.exercise("recover", image_inside=False)
        self.assertEqual(result.returncode, 7)
        self.assertEqual((calls, detaches), (1, []))

    def test_rel_017_different_disk_never_detached(self):
        result, calls, detaches = self.exercise("recover", device="/dev/disk5")
        self.assertEqual(result.returncode, 7)
        self.assertEqual((calls, detaches), (1, []))

    def test_rel_017_retry_exhaustion_preserves_original_failure(self):
        result, calls, detaches = self.exercise("busy_forever")
        self.assertEqual(result.returncode, 7)
        self.assertEqual(calls, 3)
        self.assertEqual(len(detaches), 2)

    def test_rel_017_cleanup_failure_preserves_original_failure(self):
        result, calls, detaches = self.exercise("detach_failure")
        self.assertEqual(result.returncode, 7)
        self.assertEqual(calls, 1)
        self.assertEqual(detaches, ["detach /dev/disk4 -force"])


if __name__ == "__main__":
    unittest.main()
