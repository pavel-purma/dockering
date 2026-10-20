"""REL-087 (b): the update leg of scripts/installer-smoke.ps1 and the CI step that runs it.

The leg installs the last published release per user, runs the installer under test with exactly
the updater's argv (`installer_args(InstallKind::InnoUser)` in crates/dk-update/src/apply/mod.rs),
checks the result and uninstalls. None of that can run here: it needs Windows, Inno Setup and a
registry. What these tests prove, and nothing more:

* the argv in the script is the argv the app builds (read from the Rust source), from ONE variable;
* the new parameters are required together and the default legs' lines are still there, in order;
* the script and the CI step parse (needs `pwsh`);
* the SHA256SUMS helper and the process filter, run by `pwsh` against temp files and fake objects;
* the CI step's download / skip logic, run by `pwsh` against a fake `gh` and a stub script;
* the leg's CONTROL FLOW (Linux only), run by `pwsh` against fake installers with `Get-Process` and
  `Get-ItemProperty` replaced: a fake new installer exits 3 unless it got the updater's argv exactly.

Everything Windows-specific stays unverified (spike W1): Mark-of-the-Web, SmartScreen, Defender,
UAC, the share-locked start, what `Start-Process` and Inno really do, a window after /RELAUNCH.
Those are manual items in docs/plan/release-checklist.md.

Run with `cd scripts/tests && python3 -B -m unittest test_installer_smoke_update`.
"""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_ci_gate import ci_jobs  # noqa: E402


ROOT = Path(__file__).resolve().parents[2]
SMOKE = ROOT / "scripts/installer-smoke.ps1"
CI = ROOT / ".github/workflows/ci.yml"
RELEASE = ROOT / ".github/workflows/release.yml"
APPLY = ROOT / "crates/dk-update/src/apply/mod.rs"
SPEC = ROOT / "docs/spec/features/distribution.md"
CHECKLIST = ROOT / "docs/plan/release-checklist.md"
PWSH = shutil.which("pwsh")
UNIX = os.name != "nt"
LINUX = sys.platform.startswith("linux")

STEP_NAME = "Update leg, last release to this build (REL-087)"
SMOKE_STEP_NAME = "Silent install / uninstall, per user and all users (REL-014)"
W1_TESTS = (
    "upd_003_windows_unsigned_exe_is_no_signature_w1",
    "upd_003_windows_open_handle_verdict_matches_path_w1",
    "upd_007_locked_per_user_start_works_w1",
)

# The default legs of the script as they were before the update leg existed: every one of these
# lines must still be there, in this order (the update leg only adds lines around them).
ORIGINAL_LINES = (
    "    $scope = '/ALLUSERS'",
    "    $dir = Join-Path $env:ProgramFiles 'Dockering'",
    '    $key = "HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\$appId"',
    "    $programs = [Environment]::GetFolderPath('CommonPrograms')",
    "    $scope = '/CURRENTUSER'",
    "    $dir = Join-Path $env:LOCALAPPDATA 'Programs\\Dockering'",
    '    $key = "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\$appId"',
    "    $programs = [Environment]::GetFolderPath('Programs')",
    "$shortcut = Join-Path $programs 'Dockering.lnk'",
    "Install 'install'",
    "foreach ($f in 'dockering.exe', 'LICENSE', 'THIRD_PARTY_LICENSES.html', 'unins000.exe') {",
    '    if (-not (Test-Path (Join-Path $dir $f))) { Fail "missing $f in $dir" }',
    "$subsystem = Get-PeSubsystem (Join-Path $dir 'dockering.exe')",
    '''if ($subsystem -ne 2) { Fail "dockering.exe has PE subsystem $subsystem, expected 2 (Windows GUI, no console window)" }''',
    '''$versionOut = Join-Path ([IO.Path]::GetTempPath()) "dockering-version-$PID.txt"''',
    "$p = Start-Process (Join-Path $dir 'dockering.exe') -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $versionOut",
    "$reported = if (Test-Path $versionOut) { (Get-Content $versionOut -Raw).Trim() } else { '' }",
    "Remove-Item $versionOut -ErrorAction SilentlyContinue",
    '''if ($p.ExitCode -ne 0 -or $reported -ne "dockering $Version") { Fail "--version exited $($p.ExitCode) and printed '$reported', expected 'dockering $Version'" }''',
    "$info = (Get-Item (Join-Path $dir 'dockering.exe')).VersionInfo",
    """if ($info.ProductVersion -ne $Version -or $info.ProductName -ne 'Dockering') {""",
    '''    Fail "VERSIONINFO is '$($info.ProductName) $($info.ProductVersion)'"''',
    "$entry = Get-ItemProperty $key -ErrorAction SilentlyContinue",
    '''if (-not $entry -or $entry.DisplayVersion -ne $Version) { Fail "uninstall entry missing or wrong version at $key" }''',
    '''if (-not (Test-Path $shortcut)) { Fail "Start Menu shortcut missing: $shortcut" }''',
    'Write-Host "installed $Version into $dir"',
    "Install 'reinstall'",
    "$p = Start-Process (Join-Path $dir 'unins000.exe') -ArgumentList $silent -Wait -PassThru",
    '''if ($p.ExitCode -ne 0) { Fail "uninstall exited with $($p.ExitCode)" }''',
    "for ($i = 0; $i -lt 20 -and (Test-Path $dir); $i++) { Start-Sleep -Milliseconds 500 }",
    'if (Test-Path $dir) { Fail "install dir still exists: $dir" }',
    'if (Test-Path $key) { Fail "uninstall entry still exists: $key" }',
    'if (Test-Path $shortcut) { Fail "Start Menu shortcut still exists: $shortcut" }',
    'Write-Host "uninstalled cleanly"',
)


# ---- readers -----------------------------------------------------------------------------------


def smoke_text():
    return SMOKE.read_text(encoding="utf-8")


def smoke_code():
    """The script without its full-line comments (the header names the switches too)."""
    return "\n".join(line for line in smoke_text().split("\n") if not line.lstrip().startswith("#"))


def rust_installer_args():
    """`installer_args` of crates/dk-update/src/apply/mod.rs, per kind, read from the source."""
    source = APPLY.read_text(encoding="utf-8")
    found = re.search(r"pub fn installer_args\(kind: InstallKind\) -> Vec<&'static str> \{(.*?)\n\}\n", source, re.S)
    if not found:
        raise AssertionError("installer_args not found in crates/dk-update/src/apply/mod.rs")
    body = found.group(1)
    base = re.search(r"vec!\[(.*?)\]", body, re.S)
    pick = re.search(
        r'args\.push\(if kind == InstallKind::InnoMachine \{\s*"([^"]*)"\s*\} else \{\s*"([^"]*)"\s*\}\);', body, re.S
    )
    if not base or not pick:
        raise AssertionError("the shape of installer_args changed; teach this test the new shape")
    common = re.findall(r'"([^"]*)"', base.group(1))
    machine, user = pick.groups()
    return {"InnoMachine": common + [machine], "InnoUser": common + [user]}


def script_updater_args():
    """The `$UpdaterArgs = @(...)` list of the script (exactly one assignment, literals only)."""
    found = re.findall(r"^\$UpdaterArgs\s*=\s*@\((.*?)\)\s*$", smoke_code(), re.M | re.S)
    if len(found) != 1:
        raise AssertionError(f"expected exactly one $UpdaterArgs assignment, found {len(found)}")
    if not re.fullmatch(r"\s*(?:'[^']*'\s*,?\s*)+", found[0]):
        raise AssertionError("$UpdaterArgs must be a list of plain single-quoted literals")
    return re.findall(r"'([^']*)'", found[0])


def build_steps():
    """The `build` job's steps of ci.yml as raw text blocks (text parsing, like test_ci_gate)."""
    steps, current = [], None
    for line in ci_jobs()["build"]:
        if line.startswith("      - "):
            current = [line]
            steps.append(current)
        elif current is not None:
            current.append(line)
    return ["\n".join(step) for step in steps]


def step_name(step):
    found = re.search(r"^      - name: (.*)$", step, re.M)
    return found.group(1).strip() if found else None


def find_build_step(name):
    for step in build_steps():
        if step_name(step) == name:
            return step
    raise AssertionError(f"no step named {name!r} in the build job of ci.yml")


def run_block(step):
    lines = step.split("\n")
    start = next(i for i, line in enumerate(lines) if line == "        run: |")
    body = []
    for line in lines[start + 1 :]:
        if line.strip() and not line.startswith(" " * 10):
            break
        body.append(line[10:] if line.strip() else "")
    return "\n".join(body).rstrip("\n") + "\n"


def pwsh(*args, env=None, cwd=None, timeout=120):
    environment = dict(os.environ)
    environment.update({"POWERSHELL_TELEMETRY_OPTOUT": "1", "DOTNET_NOLOGO": "1", "NO_COLOR": "1"})
    environment.update(env or {})
    return subprocess.run(
        [PWSH, "-NoProfile", "-NonInteractive", *args],
        capture_output=True,
        text=True,
        env=environment,
        cwd=cwd,
        timeout=timeout,
        check=False,
    )


def sha256(data):
    return hashlib.sha256(data).hexdigest()


# ---- the argv ----------------------------------------------------------------------------------


class ArgvTests(unittest.TestCase):
    def test_rel_087_update_leg_uses_the_updaters_argv(self):
        rust = rust_installer_args()
        # Sanity of the extraction itself: the two kinds differ only in the scope switch.
        self.assertEqual(rust["InnoUser"][-1], "/CURRENTUSER")
        self.assertEqual(rust["InnoMachine"][-1], "/ALLUSERS")
        self.assertEqual(rust["InnoUser"][:-1], rust["InnoMachine"][:-1])
        self.assertEqual(rust["InnoUser"][:2], ["/SILENT", "/SUPPRESSMSGBOXES"])

        script = script_updater_args()
        self.assertEqual(script, rust["InnoUser"], "the script's $UpdaterArgs must equal installer_args(InnoUser), in order")

        # ONE variable: the switches that only the updater passes appear once in the code, and the
        # new installer is started from that variable, not from a second list.
        code = smoke_code()
        for switch in ("/UPDATE", "/RELAUNCH", "/SILENT"):
            self.assertEqual(len(re.findall(re.escape(f"'{switch}'"), code)), 1, f"{switch} must be written once")
        self.assertEqual(len(re.findall(r"-ArgumentList \$UpdaterArgs\b", code)), 1)
        self.assertRegex(code, r"Start-Process \$newPath -ArgumentList \$UpdaterArgs -PassThru")

        # The spec says the same (UPD-007).
        spec = SPEC.read_text(encoding="utf-8")
        self.assertIn("`" + " ".join(rust["InnoUser"][:-1]) + "`", spec)

    @unittest.skipUnless(PWSH, "pwsh is not installed")
    def test_rel_087_update_leg_argv_as_powershell_evaluates_it(self):
        done = pwsh(
            "-Command",
            ". $env:SMOKE -Setup x -Version 0; $UpdaterArgs | ConvertTo-Json -Compress -AsArray",
            env={"SMOKE": str(SMOKE)},
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(json.loads(done.stdout), rust_installer_args()["InnoUser"])


# ---- parameters and the default legs -----------------------------------------------------------


class ParameterTests(unittest.TestCase):
    def test_rel_087_update_leg_requires_both_parameters(self):
        text = smoke_text()
        param = re.search(r"^param\((.*?)^\)", text, re.M | re.S)
        self.assertIsNotNone(param, "param block not found")
        block = param.group(1)
        for declaration in (
            "[Parameter(Mandatory)] [string] $Setup",
            "[Parameter(Mandatory)] [string] $Version",
            "[switch] $AllUsers",
            "[string] $UpdateFrom",
            "[string] $UpdateFromSums",
            "[switch] $AllowSameVersion",
        ):
            self.assertIn(declaration, block)
        # Neither update parameter is Mandatory on its own: the pair is checked explicitly, so a lone
        # one is a clear error instead of an interactive prompt.
        self.assertNotIn("Mandatory)] [string] $UpdateFrom", block)

        code = smoke_code()
        check = code.index("-UpdateFrom and -UpdateFromSums go together")
        self.assertLess(code.index("if ($MyInvocation.InvocationName -eq '.') { return }"), check)
        self.assertLess(check, code.index("if ($AllUsers) {"), "validate before anything touches the machine")
        self.assertLess(check, code.index("Invoke-UpdateLeg $UpdateFrom"))

    @unittest.skipUnless(PWSH, "pwsh is not installed")
    def test_rel_087_update_leg_rejects_lone_and_conflicting_parameters(self):
        cases = {
            "only -UpdateFrom": (["-UpdateFrom", "old.exe"], "go together"),
            "only -UpdateFromSums": (["-UpdateFromSums", "SHA256SUMS"], "go together"),
            "with -AllUsers": (["-UpdateFrom", "old.exe", "-UpdateFromSums", "SHA256SUMS", "-AllUsers"], "per-user"),
            "-AllowSameVersion alone": (["-AllowSameVersion"], "only applies to the update leg"),
        }
        for label, (extra, message) in cases.items():
            with self.subTest(label):
                done = pwsh("-File", str(SMOKE), "-Setup", "setup.exe", "-Version", "1.2.3", *extra)
                self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                self.assertIn("::error::", done.stdout)
                self.assertIn(message, done.stdout)

    def test_rel_087_default_legs_unchanged(self):
        lines = smoke_text().split("\n")
        position = 0
        for expected in ORIGINAL_LINES:
            try:
                position = lines.index(expected, position) + 1
            except ValueError:
                self.fail(f"an original line of the default legs is gone, changed or moved: {expected!r}")
        # The original Install function and the silent switches, verbatim.
        text = smoke_text()
        self.assertIn(
            "function Install([string] $what) {\n"
            "    $p = Start-Process (Resolve-Path $Setup) -ArgumentList ($silent + $scope) -Wait -PassThru\n"
            '    if ($p.ExitCode -ne 0) { Fail "$what exited with $($p.ExitCode)" }\n'
            "}\n",
            text,
        )
        self.assertIn("$silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-')", text)
        self.assertIn("$appId = '{8C3F4E2A-6B1D-4E7A-9F2C-5D8B1A3E7C64}_is1'", text)
        self.assertIn("$ErrorActionPreference = 'Stop'\n", text)
        self.assertIn(
            "function Fail([string] $message) {\n"
            '    Write-Host "::error::$message"\n'
            "    exit 1\n"
            "}\n",
            text,
        )
        self.assertIn(
            "function Get-PeSubsystem([string] $Path) {\n"
            "    $stream = [IO.File]::OpenRead($Path)\n"
            "    try {\n"
            "        $reader = [IO.BinaryReader]::new($stream)\n"
            "        $stream.Position = 0x3C\n"
            "        $stream.Position = $reader.ReadInt32() + 4 + 20 + 68\n"
            "        return $reader.ReadUInt16()\n"
            "    } finally { $stream.Dispose() }\n"
            "}\n",
            text,
        )
        # The update leg is reached only when -UpdateFrom is given, before the default body starts.
        dispatch = text.index("if ($UpdateFrom) {")
        self.assertLess(dispatch, text.index("Install 'install'"))
        self.assertLess(text.index("$shortcut = Join-Path $programs 'Dockering.lnk'"), dispatch)
        self.assertIn("    exit 0\n}\n", text[dispatch : dispatch + 200])

        # Nobody else passes the new parameters: release.yml never does, and ci.yml's two default
        # invocations are exactly as they were.
        for line in RELEASE.read_text(encoding="utf-8").split("\n"):
            if "installer-smoke.ps1" in line:
                self.assertNotIn("-UpdateFrom", line, "release.yml must not run the update leg")
                self.assertNotIn("-AllowSameVersion", line)
        ci = CI.read_text(encoding="utf-8")
        self.assertIn("pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup $setup -Version $version\n", ci)
        self.assertIn("pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup $setup -Version $version -AllUsers\n", ci)


# ---- parsing -----------------------------------------------------------------------------------

PARSE = (
    "$e = $null; $t = $null; "
    "[void][System.Management.Automation.Language.Parser]::ParseFile($env:PS_FILE, [ref]$t, [ref]$e); "
    "if ($e) { $e | ForEach-Object { 'line ' + $_.Extent.StartLineNumber + ': ' + $_.Message }; exit 1 }"
)


@unittest.skipUnless(PWSH, "pwsh is not installed")
class ParseTests(unittest.TestCase):
    def test_rel_087_script_parses(self):
        done = pwsh("-Command", PARSE, env={"PS_FILE": str(SMOKE)})
        self.assertEqual(done.returncode, 0, "installer-smoke.ps1 does not parse:\n" + done.stdout + done.stderr)

    def test_rel_087_ci_step_parses(self):
        with tempfile.TemporaryDirectory() as tmp:
            step = Path(tmp) / "step.ps1"
            step.write_text(run_block(find_build_step(STEP_NAME)), encoding="utf-8")
            done = pwsh("-Command", PARSE, env={"PS_FILE": str(step)})
        self.assertEqual(done.returncode, 0, "the CI step's script does not parse:\n" + done.stdout + done.stderr)


# ---- the SHA256SUMS helper and the process filter ----------------------------------------------


@unittest.skipUnless(PWSH, "pwsh is not installed")
class HelperTests(unittest.TestCase):
    def test_rel_087_sha256sums_helper(self):
        name = "Dockering-Setup-x64.exe"
        data = b"pretend installer bytes\x00\x01"
        digest = sha256(data)
        zeros = "0" * 64
        cases = [
            # label, SHA256SUMS text, expected Ok, expected fragment of Reason
            ("text format (two spaces)", f"{digest}  {name}\n", True, ""),
            ("binary format (space, star)", f"{digest} *{name}\n", True, ""),
            ("upper-case hex", f"{digest.upper()}  {name}\n", True, ""),
            ("CRLF line ends", f"{digest}  {name}\r\n{zeros}  other.exe\r\n", True, ""),
            ("other lines are ignored", f"{zeros}  other.exe\n{digest}  {name}\n\nnot a checksum line\n", True, ""),
            ("a mismatch fails", f"{zeros}  {name}\n", False, "SHA-256 of"),
            ("a missing line fails", f"{digest}  other.exe\n", False, "no line for"),
            ("an empty file fails", "", False, "no line for"),
            ("the name matches whole and exactly", f"{digest}  {name}.sig\n{digest}  x{name}\n{digest}  {name.lower()}\n", False, "no line for"),
            ("a path in the name is not the name", f"{digest}  dir/{name}\n", False, "no line for"),
            ("a short hash is no line", f"{digest[:63]}  {name}\n", False, "no line for"),
            ("one line with a wrong hash wins nothing", f"{digest}  {name}\n{zeros}  {name}\n", False, "2 times"),
            ("the same line twice is ambiguous too", f"{digest}  {name}\n{digest}  {name}\n", False, "2 times"),
        ]
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            installer = tmp / name
            installer.write_bytes(data)
            specs = []
            for index, (_label, sums, _ok, _reason) in enumerate(cases):
                path = tmp / f"SHA256SUMS.{index}"
                path.write_bytes(sums.encode())
                specs.append({"file": str(installer), "sums": str(path)})
            specs.append({"file": str(tmp / "missing.exe"), "sums": str(tmp / "SHA256SUMS.0")})
            specs.append({"file": str(installer), "sums": str(tmp / "no-such-sums")})
            cases_file = tmp / "cases.json"
            cases_file.write_text(json.dumps(specs), encoding="utf-8")
            done = pwsh(
                "-Command",
                ". $env:SMOKE -Setup x -Version 0; "
                "$cases = Get-Content -Raw $env:CASES | ConvertFrom-Json; "
                "$out = foreach ($c in $cases) { Test-Sha256Sums $c.file $c.sums }; "
                "ConvertTo-Json -InputObject @($out) -Compress",
                env={"SMOKE": str(SMOKE), "CASES": str(cases_file)},
            )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        results = json.loads(done.stdout)
        self.assertEqual(len(results), len(cases) + 2)
        for (label, _sums, ok, reason), result in zip(cases, results):
            with self.subTest(label):
                self.assertEqual(result["Ok"], ok, result)
                self.assertIn(reason, result["Reason"])
                self.assertEqual(result["Name"], name)
                if ok:
                    self.assertEqual(result["Expected"], digest)
                    self.assertEqual(result["Actual"], digest)
                    self.assertEqual(result["Reason"], "")
        with self.subTest("installer file missing"):
            self.assertFalse(results[-2]["Ok"])
            self.assertIn("installer not found", results[-2]["Reason"])
        with self.subTest("SHA256SUMS file missing"):
            self.assertFalse(results[-1]["Ok"])
            self.assertIn("SHA256SUMS not found", results[-1]["Reason"])

    def test_rel_087_process_filter_helper(self):
        with tempfile.TemporaryDirectory() as tmp:
            install = str(Path(tmp) / "Programs" / "Dockering")
            candidates = [
                {"Id": 1, "Path": install + "/dockering.exe"},
                {"Id": 2, "Path": install.upper() + "/dockering.exe"},  # Windows paths compare case-insensitively
                {"Id": 3, "Path": install + "-old/dockering.exe"},  # a sibling that shares the prefix
                {"Id": 4, "Path": install + "/sub/dockering.exe"},  # not directly in the dir
                {"Id": 5, "Path": None},  # a process whose path can't be read
                {"Id": 6, "Path": str(Path(tmp) / "elsewhere" / "dockering.exe")},
                {"Id": 7, "Path": install + "/../Dockering/dockering.exe"},  # normalised before comparing
            ]
            done = pwsh(
                "-Command",
                ". $env:SMOKE -Setup x -Version 0; "
                "$c = $env:CANDIDATES | ConvertFrom-Json; "
                "$hit = @(Select-ProcessInDirectory $c $env:INSTALL); "
                "$none = @(Select-ProcessInDirectory $null $env:INSTALL); "
                "ConvertTo-Json -Compress -InputObject ([ordered]@{ ids = @($hit | ForEach-Object { $_.Id }); none = $none.Count })",
                env={"SMOKE": str(SMOKE), "CANDIDATES": json.dumps(candidates), "INSTALL": install},
            )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        found = json.loads(done.stdout)
        self.assertEqual(sorted(found["ids"]), [1, 2, 7])
        self.assertEqual(found["none"], 0)


# ---- the CI step -------------------------------------------------------------------------------


class CiStepTests(unittest.TestCase):
    def test_rel_087_ci_step_downloads_last_release_and_skips_when_none(self):
        steps = [step_name(step) for step in build_steps()]
        self.assertIn(STEP_NAME, steps)
        self.assertEqual(
            steps.index(STEP_NAME),
            steps.index(SMOKE_STEP_NAME) + 1,
            "the update leg runs right after the existing install smoke step",
        )
        step = find_build_step(STEP_NAME)
        self.assertIn("        if: runner.os == 'Windows'\n", step)
        self.assertIn("        shell: pwsh\n", step)
        self.assertIn("          GH_TOKEN: ${{ github.token }}\n", step)
        self.assertNotIn("continue-on-error", step, "a failing update leg must fail the job")
        script = run_block(step)
        # The last STABLE release, its per-arch installer and its SHA256SUMS, with gh.
        self.assertIn("gh release list --repo $env:REPO --exclude-drafts --exclude-pre-releases --limit 1 --json tagName", script)
        self.assertIn("gh release download $tag --repo $env:REPO --pattern $name --pattern SHA256SUMS --dir $dest --clobber", script)
        self.assertIn('$name = "Dockering-Setup-$env:ARCH.exe"', script)
        self.assertIn('$setup = "target\\dist\\$env:TARGET\\$name"', script)
        self.assertIn("ARCH: ${{ matrix.arch }}", step)
        self.assertIn("TARGET: ${{ matrix.target }}", step)
        # ... the same installer path as the existing smoke step builds.
        self.assertIn('target\\dist\\${{ matrix.target }}\\Dockering-Setup-${{ matrix.arch }}.exe', find_build_step(SMOKE_STEP_NAME))
        # No release, or none reachable: a notice or a warning and exit 0, never a failure.
        self.assertIn("Skip 'notice' 'no stable release is published yet", script)
        self.assertIn("Skip 'warning'", script)
        self.assertEqual(len(re.findall(r"^\s*exit 0$", script, re.M)), 1, "only the Skip helper exits 0")
        # The leg gets both parameters, and the same-version mode only when the tag IS the version.
        self.assertIn("'-UpdateFrom', (Join-Path $dest $name), '-UpdateFromSums', (Join-Path $dest 'SHA256SUMS')", script)
        self.assertIn('if ($tag -eq "v$version") { $legArgs += \'-AllowSameVersion\' }', script)
        self.assertIn("exit $LASTEXITCODE", script)
        # Least privilege stays as it was: contents: read, nothing wider.
        ci = CI.read_text(encoding="utf-8")
        self.assertIn("\npermissions:\n  contents: read\n", ci)
        self.assertNotIn("contents: write", ci)
        self.assertNotIn("permissions:", "\n".join(ci_jobs()["build"]), "the build job must not widen permissions")

    def test_rel_087_ci_yaml_is_valid_and_wired(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML is not installed")
        document = yaml.safe_load(CI.read_text(encoding="utf-8"))
        steps = document["jobs"]["build"]["steps"]
        names = [s.get("name") for s in steps]
        step = steps[names.index(STEP_NAME)]
        self.assertEqual(step["if"], "runner.os == 'Windows'")
        self.assertEqual(step["shell"], "pwsh")
        self.assertEqual(step["env"], {
            "GH_TOKEN": "${{ github.token }}",
            "REPO": "${{ github.repository }}",
            "TARGET": "${{ matrix.target }}",
            "ARCH": "${{ matrix.arch }}",
        })
        self.assertEqual(document["permissions"], {"contents": "read"})
        self.assertNotIn("permissions", document["jobs"]["build"])
        self.assertIn("build", document["jobs"]["gate"]["needs"])


FAKE_GH = """#!/bin/sh
echo "$*" >> "$FAKE_GH_LOG"
case "$1 $2" in
  "release list")
    case "$FAKE_GH_MODE" in
      none) echo '[]' ;;
      listerror) echo "gh: HTTP 502 (fake)" >&2; exit 1 ;;
      *) printf '[{"tagName":"%s"}]\\n' "$FAKE_GH_TAG" ;;
    esac ;;
  "release download")
    [ "$FAKE_GH_MODE" = "nodownload" ] && { echo "gh: no assets match (fake)" >&2; exit 1; }
    dir=""; prev=""
    for a in "$@"; do [ "$prev" = "--dir" ] && dir="$a"; prev="$a"; done
    mkdir -p "$dir"
    echo installer > "$dir/Dockering-Setup-x64.exe"
    echo sums > "$dir/SHA256SUMS" ;;
  *) echo "fake gh: unexpected call: $*" >&2; exit 9 ;;
esac
"""

# Stands in for scripts/installer-smoke.ps1: records its arguments and exits with $STUB_EXIT.
STUB_SMOKE = """($args -join ' ') | Set-Content -LiteralPath $env:STUB_LOG
exit ([int] $env:STUB_EXIT)
"""


@unittest.skipUnless(PWSH and UNIX, "needs pwsh and a Unix shell for the fake gh")
class CiStepRunTests(unittest.TestCase):
    """Runs the step's real script with pwsh in a scratch checkout: a fake `gh`, a stub smoke script."""

    def run_step(self, mode, tag="v0.3.0", workspace="0.3.0", stub_exit="0", gh_on_path=True):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "bin").mkdir()
            gh = tmp / "bin" / "gh"
            gh.write_text(FAKE_GH, encoding="utf-8")
            gh.chmod(gh.stat().st_mode | stat.S_IXUSR)
            (tmp / "repo" / "scripts").mkdir(parents=True)
            (tmp / "repo" / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{workspace}"\n', encoding="utf-8")
            (tmp / "repo" / "scripts" / "installer-smoke.ps1").write_text(STUB_SMOKE, encoding="utf-8")
            (tmp / "runner").mkdir()
            # No waiting between retries: a function named like the cmdlet wins over the cmdlet.
            script = tmp / "step.ps1"
            script.write_text("function Start-Sleep { }\n" + run_block(find_build_step(STEP_NAME)), encoding="utf-8")
            empty = tmp / "empty"
            empty.mkdir()
            path = str(tmp / "bin") + os.pathsep + os.environ["PATH"] if gh_on_path else str(empty)
            env = {
                "PATH": path,
                "GH_TOKEN": "fake-token-not-a-secret",
                "REPO": "owner/repo",
                "TARGET": "x86_64-pc-windows-msvc",
                "ARCH": "x64",
                "RUNNER_TEMP": str(tmp / "runner"),
                "GITHUB_STEP_SUMMARY": str(tmp / "summary.md"),
                "FAKE_GH_LOG": str(tmp / "gh.log"),
                "FAKE_GH_MODE": mode,
                "FAKE_GH_TAG": tag,
                "STUB_LOG": str(tmp / "stub.log"),
                "STUB_EXIT": stub_exit,
            }
            done = pwsh("-File", str(script), env=env, cwd=tmp / "repo")
            def read(name):
                path = tmp / name
                return path.read_text(encoding="utf-8") if path.exists() else None

            return {
                "rc": done.returncode,
                "out": done.stdout + done.stderr,
                "gh": (read("gh.log") or "").splitlines(),
                "stub": read("stub.log"),
                "summary": read("summary.md"),
                "tmp": str(tmp),
            }

    def test_rel_087_ci_step_skips_with_a_notice_when_there_is_no_release(self):
        result = self.run_step("none")
        self.assertEqual(result["rc"], 0, result["out"])
        self.assertIn("::notice title=Update leg skipped::no stable release is published yet", result["out"])
        self.assertEqual(len(result["gh"]), 1, result["gh"])
        self.assertIsNone(result["stub"], "the smoke script must not run without an old release")
        self.assertIn("Update leg (REL-087) not run", result["summary"])

    def test_rel_087_ci_step_skips_with_a_warning_when_github_does_not_answer(self):
        result = self.run_step("listerror")
        self.assertEqual(result["rc"], 0, result["out"])
        self.assertIn("::warning title=Update leg skipped::", result["out"])
        self.assertEqual(len([line for line in result["gh"] if line.startswith("release list")]), 3, "three tries")
        self.assertFalse([line for line in result["gh"] if line.startswith("release download")])
        self.assertIsNone(result["stub"])

    def test_rel_087_ci_step_skips_with_a_warning_when_the_download_fails(self):
        result = self.run_step("nodownload")
        self.assertEqual(result["rc"], 0, result["out"])
        self.assertIn("::warning title=Update leg skipped::could not download Dockering-Setup-x64.exe and SHA256SUMS from v0.3.0", result["out"])
        self.assertEqual(len([line for line in result["gh"] if line.startswith("release download")]), 3)
        self.assertIsNone(result["stub"])

    def test_rel_087_ci_step_runs_the_leg_with_the_last_release(self):
        # The last release is older than the workspace version: strict mode (a version bump).
        result = self.run_step("release", tag="v0.3.0", workspace="0.4.0")
        self.assertEqual(result["rc"], 0, result["out"])
        runner = Path(result["tmp"]) / "runner" / "update-from"
        self.assertEqual(
            result["gh"][1],
            f"release download v0.3.0 --repo owner/repo --pattern Dockering-Setup-x64.exe --pattern SHA256SUMS --dir {runner} --clobber",
        )
        self.assertEqual(
            result["stub"].strip(),
            "-Setup target\\dist\\x86_64-pc-windows-msvc\\Dockering-Setup-x64.exe -Version 0.4.0 "
            f"-UpdateFrom {runner / 'Dockering-Setup-x64.exe'} -UpdateFromSums {runner / 'SHA256SUMS'}",
        )
        self.assertNotIn("-AllowSameVersion", result["stub"])

    def test_rel_087_ci_step_allows_the_same_version_only_when_the_tag_is_the_version(self):
        same = self.run_step("release", tag="v0.3.0", workspace="0.3.0")
        self.assertEqual(same["rc"], 0, same["out"])
        self.assertTrue(same["stub"].strip().endswith("-AllowSameVersion"), same["stub"])
        other = self.run_step("release", tag="v0.3.1", workspace="0.3.0")
        self.assertEqual(other["rc"], 0, other["out"])
        self.assertNotIn("-AllowSameVersion", other["stub"])

    def test_rel_087_ci_step_fails_when_the_leg_fails(self):
        result = self.run_step("release", stub_exit="1")
        self.assertEqual(result["rc"], 1, "a failing update leg must fail the step\n" + result["out"])

    def test_rel_087_ci_step_fails_loudly_without_gh(self):
        result = self.run_step("release", gh_on_path=False)
        self.assertEqual(result["rc"], 1, result["out"])
        self.assertIn("::error::gh is not on PATH", result["out"])
        self.assertIsNone(result["stub"])


# ---- the leg's control flow against fake installers --------------------------------------------

FAKE_OLD_INSTALLER = """#!/bin/sh
# the OLD release's installer: a per-user install of "dockering 0.3.0"
[ "$*" = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER" ] || { echo "old: unexpected args: $*" >&2; exit 3; }
mkdir -p "$SIM_DIR"
printf '#!/bin/sh\\necho "dockering 0.3.0"\\n' > "$SIM_DIR/dockering.exe"; chmod +x "$SIM_DIR/dockering.exe"
echo licence > "$SIM_DIR/LICENSE"; echo licences > "$SIM_DIR/THIRD_PARTY_LICENSES.html"
printf '#!/bin/sh\\nrm -rf "$SIM_DIR" "$SIM_KEY" "$SIM_LNK"\\n' > "$SIM_DIR/unins000.exe"; chmod +x "$SIM_DIR/unins000.exe"
echo 0.3.0 > "$SIM_KEY"; echo shortcut > "$SIM_LNK"
"""

FAKE_NEW_INSTALLER = """#!/bin/sh
# the installer UNDER TEST: exits 3 unless it was started with exactly the updater's argv
[ "$*" = "/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /UPDATE /RELAUNCH /CURRENTUSER" ] || { echo "new: unexpected args: $*" >&2; exit 3; }
[ -n "$SIM_NEW_EXIT" ] && exit "$SIM_NEW_EXIT"
printf '#!/bin/sh\\necho "dockering %s"\\n' "$SIM_NEW_VERSION" > "$SIM_DIR/dockering.exe"; chmod +x "$SIM_DIR/dockering.exe"
echo "$SIM_NEW_VERSION" > "$SIM_KEY"
if [ -z "$SIM_NO_RELAUNCH" ]; then
  nohup sleep 60 > /dev/null 2>&1 &
  echo $! > "$SIM_PIDFILE"
fi
"""

# Everything that only exists on Windows is replaced: the uninstall entry is a file, "processes
# running from the install dir" is whatever the fake installer started.
SIM_HARNESS = r"""
$ErrorActionPreference = 'Stop'
$work = $env:SIM_WORK
. $env:SMOKE -Setup "$work/new.sh" -Version $env:SIM_VERSION
$RelaunchTimeoutSec = 2
$InstallerTimeoutSec = 30
$dir = $env:SIM_DIR; $key = $env:SIM_KEY; $shortcut = $env:SIM_LNK
if ($env:SIM_ARGV_MUTATED) { $UpdaterArgs = @('/SILENT') }
function Get-ItemProperty([string] $Path, $ErrorAction) {
    if (Test-Path $Path) { [pscustomobject]@{ DisplayVersion = (Get-Content $Path -Raw).Trim() } }
}
function Get-Process([string] $Name, $ErrorAction) {
    if (-not (Test-Path $env:SIM_PIDFILE)) { return }
    $id = [int] (Get-Content $env:SIM_PIDFILE -Raw)
    $stat = "/proc/$id/stat"
    if ((Test-Path $stat) -and ((Get-Content $stat -Raw) -notmatch '^\d+ \(.*\) Z ')) {
        [pscustomobject]@{ Id = $id; Path = "$env:SIM_DIR/dockering.exe" }
    }
}
Invoke-UpdateLeg "$work/old/Dockering-Setup-x64.exe" "$work/SHA256SUMS" ($env:SIM_SAME_OK -eq '1')
"LEG-RETURNED"
"""


@unittest.skipUnless(PWSH and LINUX, "needs pwsh on Linux (/proc, shell-script installers)")
class UpdateLegFlowTests(unittest.TestCase):
    """The leg's own logic, run for real, against fakes. Proves nothing about Windows or Inno."""

    def run_leg(self, *, new_version="0.4.0", workspace="0.4.0", same_ok=False, bad_sum=False, **flags):
        with tempfile.TemporaryDirectory() as work:
            work = Path(work)
            (work / "old").mkdir()
            old = work / "old" / "Dockering-Setup-x64.exe"
            old.write_text(FAKE_OLD_INSTALLER, encoding="utf-8")
            new = work / "new.sh"
            new.write_text(FAKE_NEW_INSTALLER, encoding="utf-8")
            for script in (old, new):
                script.chmod(script.stat().st_mode | stat.S_IXUSR)
            digest = "0" * 64 if bad_sum else sha256(old.read_bytes())
            (work / "SHA256SUMS").write_text(f"{digest}  {old.name}\n", encoding="utf-8")
            harness = work / "harness.ps1"
            harness.write_text(SIM_HARNESS, encoding="utf-8")
            env = {
                "SMOKE": str(SMOKE),
                "SIM_WORK": str(work),
                "SIM_DIR": str(work / "app"),
                "SIM_KEY": str(work / "key"),
                "SIM_LNK": str(work / "lnk"),
                "SIM_PIDFILE": str(work / "pid"),
                "SIM_NEW_VERSION": new_version,
                "SIM_VERSION": workspace,
                "SIM_SAME_OK": "1" if same_ok else "0",
            }
            env.update({f"SIM_{name.upper()}": value for name, value in flags.items()})
            done = pwsh("-File", str(harness), env=env)
            leftover = None
            pidfile = work / "pid"
            if pidfile.exists():
                pid = int(pidfile.read_text().strip())
                stat_file = Path(f"/proc/{pid}/stat")
                leftover = stat_file.exists() and not re.match(r"^\d+ \(.*\) Z ", stat_file.read_text())
                if leftover:
                    os.kill(pid, 9)
            return {
                "rc": done.returncode,
                "out": done.stdout + done.stderr,
                "installed": (work / "app").exists(),
                "leftover_process": leftover,
            }

    def test_rel_087_update_leg_flow_passes_a_real_update(self):
        result = self.run_leg()
        self.assertEqual(result["rc"], 0, result["out"])
        out = result["out"]
        self.assertIn("matches SHA256SUMS", out)
        self.assertIn("old release installed into", out)
        self.assertIn("update installer ran with '/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /UPDATE /RELAUNCH /CURRENTUSER' and exited 0", out)
        self.assertIn("/RELAUNCH started dockering.exe", out)
        self.assertIn("version changed: 'dockering 0.3.0' -> 'dockering 0.4.0'", out)
        self.assertIn("updated to 0.4.0", out)
        self.assertIn("uninstalled cleanly", out)
        self.assertIn("LEG-RETURNED", out)
        self.assertFalse(result["leftover_process"], "the relaunched app must be stopped")

    def test_rel_087_update_leg_flow_fails_when_the_argv_is_not_the_updaters(self):
        result = self.run_leg(argv_mutated="1")
        self.assertEqual(result["rc"], 1, result["out"])
        self.assertIn("::error::update installer exited with 3", result["out"])

    def test_rel_087_update_leg_flow_same_version_is_strict_unless_allowed(self):
        strict = self.run_leg(new_version="0.3.0", workspace="0.3.0")
        self.assertEqual(strict["rc"], 1, strict["out"])
        self.assertIn("::error::old and new both report 'dockering 0.3.0'", strict["out"])
        allowed = self.run_leg(new_version="0.3.0", workspace="0.3.0", same_ok=True)
        self.assertEqual(allowed["rc"], 0, allowed["out"])
        self.assertIn("SKIP: old and new both report 'dockering 0.3.0'", allowed["out"])
        self.assertIn("uninstalled cleanly", allowed["out"])

    def test_rel_087_update_leg_flow_fails_on_each_broken_assertion(self):
        cases = {
            "a bad checksum stops before anything is installed": (
                {"bad_sum": True},
                "::error::old installer rejected: SHA-256 of Dockering-Setup-x64.exe is",
            ),
            "the installer's exit code": ({"new_exit": "5"}, "::error::update installer exited with 5"),
            "no /RELAUNCH process": ({"no_relaunch": "1"}, "::error::/RELAUNCH did not start"),
            "the version after the update": (
                {"new_version": "0.9.9"},
                "::error::after the update --version exited 0 and printed 'dockering 0.9.9', expected 'dockering 0.4.0'",
            ),
        }
        for label, (options, message) in cases.items():
            with self.subTest(label):
                result = self.run_leg(**options)
                self.assertEqual(result["rc"], 1, result["out"])
                self.assertIn(message, result["out"])
                self.assertNotIn("LEG-RETURNED", result["out"])
                if "bad checksum" in label:
                    self.assertFalse(result["installed"], "nothing may be installed from an unverified installer")


# ---- documents ---------------------------------------------------------------------------------


class DocumentTests(unittest.TestCase):
    def w1_block(self):
        lines = CHECKLIST.read_text(encoding="utf-8").split("\n")
        start = next(i for i, line in enumerate(lines) if line.startswith("- [ ] Windows, **spike W1, manual**"))
        block = []
        for line in lines[start + 1 :]:
            if line.startswith("- "):
                break
            block.append(line)
        return "\n".join(block)

    def test_rel_087_checklist_w1_rows_are_manual_unchecked_and_name_real_tests(self):
        block = self.w1_block()
        boxes = re.findall(r"^  - \[(.)\] ", block, re.M)
        self.assertGreaterEqual(len(boxes), 9, "one checkbox per W1 item")
        self.assertEqual(set(boxes), {" "}, "no W1 row may be checked from here")
        lowered = block.lower()
        for topic in (
            "mark-of-the-web",
            "smartscreen",
            "defender",
            "smart app control",
            "unknown publisher",
            "share-locked",
            "cancel",
            "explorer",
            "not elevated",
            "winverifytrust",
        ):
            self.assertIn(topic, lowered, f"the W1 checklist must cover {topic}")
        sources = "\n".join(path.read_text(encoding="utf-8") for path in (ROOT / "crates/dk-update/src").rglob("*.rs"))
        for name in W1_TESTS:
            self.assertIn(name, block)
            self.assertRegex(sources, rf"fn {name}\(\)", f"{name} no longer exists in crates/dk-update")
        self.assertIn("cargo test -p dk-update --lib -- _w1", block)

    def test_rel_087_spec_row_claims_only_the_ci_leg_verified(self):
        spec = SPEC.read_text(encoding="utf-8")
        row = next(line for line in spec.split("\n") if line.startswith("| REL-087 | *Windows CI leg green"))
        # The CI leg ran (same-version mode); nothing beyond it, and none of W1, is claimed.
        self.assertIn("W1, the manual rows, is not started", row)
        self.assertIn("nothing beyond that CI leg is verified on Windows", row)
        self.assertIn("same-version mode", row)
        self.assertNotIn("W1 passed", row)
        self.assertIn("test_installer_smoke_update.py", row)
        self.assertIn("-AllowSameVersion", row)
        self.assertIn("release-checklist.md", row)
        for test in W1_TESTS:
            self.assertIn(test, row)


if __name__ == "__main__":
    unittest.main()
