"""REL-040/REL-041: the winget manifests, the scripts that render and submit them, and the skill that drives them.

The scripts under scripts/winget/ are PowerShell. The tests that run them need `pwsh` and are skipped without it; they never touch the network
(no release lookup, no winget-pkgs API, no push): they call the pure functions of scripts/winget/Lib.ps1 on fixtures. The text tests run
everywhere and keep the package identifier, the template file names, the workflow and the skill in step.

Run with python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'.
"""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / "scripts/winget/Lib.ps1"
TEMPLATES = ROOT / "packaging/winget/manifests"
SKILL = ROOT / ".agents/skills/winget/SKILL.md"
PWSH = shutil.which("pwsh")
PACKAGE_ID = "Dockering.Dockering"
X64 = "a" * 64
ARM64 = "b" * 64
PR_TEMPLATE = """<!-- PR Title Format: "New package: Publisher.Name version X.Y.Z" -->

## Description
<!-- Describe what this PR changes. -->

## Checklist

- [ ] Signed the [Contributor License Agreement](https://cla.opensource.microsoft.com)
- [ ] Linked to an issue (if applicable)

## Manifest Checklist

- [ ] Checked that there aren't other open [pull requests](https://github.com/microsoft/winget-pkgs/pulls) for the same manifest update/change
- [ ] This PR only modifies one (1) manifest
- [ ] Validated manifest locally with `winget validate --manifest <path>`
- [ ] Tested manifest locally with `winget install --manifest <path>`
- [ ] Manifest conforms to the [1.12 schema](https://example.invalid)
"""


def run_powershell(body, **env):
    """Dot-sources Lib.ps1 and runs `body`; returns the parsed JSON the body wrote with ConvertTo-Json."""
    script = "$ErrorActionPreference = 'Stop'\n. $env:WINGET_LIB\n" + body
    with tempfile.TemporaryDirectory() as scratch:
        path = Path(scratch) / "case.ps1"
        path.write_text(script, encoding="utf-8")
        result = subprocess.run(
            [PWSH, "-NoProfile", "-File", str(path)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env={**os.environ, "WINGET_LIB": str(LIB), **env},
            timeout=120,
        )
    if result.returncode != 0:
        raise AssertionError(f"pwsh exited {result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}")
    lines = [line for line in result.stdout.splitlines() if not re.match(r"^\[(PASS|WARN|INFO|SKIP|FAIL)\] ", line)]
    return json.loads("\n".join(lines)) if lines else None


class WingetTextTests(unittest.TestCase):
    def test_rel_040_templates_are_named_after_the_identifier(self):
        names = sorted(path.name for path in TEMPLATES.glob("*.yaml"))
        self.assertEqual(
            names,
            [f"{PACKAGE_ID}.installer.yaml", f"{PACKAGE_ID}.locale.en-US.yaml", f"{PACKAGE_ID}.yaml"],
        )

    def test_rel_040_every_template_declares_the_identifier(self):
        for path in TEMPLATES.glob("*.yaml"):
            with self.subTest(path.name):
                self.assertRegex(path.read_text(encoding="utf-8"), rf"(?m)^PackageIdentifier: {re.escape(PACKAGE_ID)}$")

    def test_rel_040_the_old_identifier_is_gone(self):
        # The word is spelled in two pieces so that this file doesn't match its own search.
        old = "Pavel" + "Purma"
        tracked = subprocess.run(["git", "-C", str(ROOT), "ls-files", "-z"], capture_output=True, check=True).stdout.decode("utf-8").split("\0")
        offenders = []
        for name in filter(None, tracked):
            path = ROOT / name
            if name.startswith("vendor/") or path.suffix in {".png", ".ico", ".icns", ".bin", ".bmp"} or not path.is_file():
                continue
            try:
                if old in path.read_text(encoding="utf-8"):
                    offenders.append(name)
            except UnicodeDecodeError:
                continue
        self.assertEqual(offenders, [])

    def test_rel_041_workflow_and_scripts_use_the_same_identifier(self):
        workflow = (ROOT / ".github/workflows/winget.yml").read_text(encoding="utf-8")
        self.assertIn(f"identifier: {PACKAGE_ID}", workflow)
        self.assertIn(f"$WingetPackageId = '{PACKAGE_ID}'", LIB.read_text(encoding="utf-8"))

    def test_rel_040_installer_template_matches_the_script_placeholders(self):
        installer = (TEMPLATES / f"{PACKAGE_ID}.installer.yaml").read_text(encoding="utf-8")
        lib = LIB.read_text(encoding="utf-8")
        for asset, placeholder in (("Dockering-Setup-x64.exe", "X64_SHA256"), ("Dockering-Setup-arm64.exe", "ARM64_SHA256")):
            with self.subTest(asset):
                self.assertIn(f"'{asset}'", lib)
                self.assertIn(f"= '{placeholder}'", lib)
                self.assertIn(f"/{asset}", installer)
                self.assertIn("{{" + placeholder + "}}", installer)

    def test_rel_041_scripts_never_pass_a_user_string_to_a_shell(self):
        for path in (ROOT / "scripts/winget").glob("*.ps1"):
            with self.subTest(path.name):
                text = path.read_text(encoding="utf-8")
                self.assertNotRegex(text, r"(?i)\bInvoke-Expression\b|\biex\b")
                self.assertNotRegex(text, r"(?i)Start-Process\b[^\n]*-Verb\s+RunAs")
                self.assertNotIn("--force", text.replace("never force-pushes", "").replace("no --force", ""))

    def test_rel_041_scripts_are_ascii(self):
        for path in (ROOT / "scripts/winget").glob("*.ps1"):
            with self.subTest(path.name):
                self.assertTrue(path.read_bytes().isascii(), "PowerShell 7 reads BOM-less files as UTF-8, but the other scripts are ASCII too")

    def test_rel_042_installer_template_carries_the_inno_uninstall_key(self):
        iss = (ROOT / "packaging/windows/dockering.iss").read_text(encoding="utf-8")
        app_id = re.search(r"(?m)^AppId=\{(\{[0-9A-Fa-f-]{36}\})\s*$", iss)
        self.assertIsNotNone(app_id, "AppId not found in dockering.iss")
        # Inno writes HKCU|HKLM\...\Uninstall\<AppId>_is1; the doubled brace of the script is an escape for one.
        product_code = f"{app_id.group(1)}_is1"
        installer = (TEMPLATES / f"{PACKAGE_ID}.installer.yaml").read_text(encoding="utf-8")
        self.assertRegex(installer, rf'(?m)^ProductCode: "{re.escape(product_code)}"$')
        # root-level values apply to every installer entry of an Inno package, so none of the entries may override them
        self.assertEqual(installer.count("ProductCode:"), 1)
        self.assertNotIn("AppsAndFeaturesEntries", installer)
        self.assertRegex(iss, r'(?m)^#define AppName "Dockering"$')
        self.assertRegex(iss, r"(?m)^UninstallDisplayName=\{#AppName\}$")
        self.assertRegex(iss, r"(?m)^AppPublisher=Dockering contributors$")

    def test_rel_041_the_skill_runs_the_three_scripts(self):
        text = SKILL.read_text(encoding="utf-8")
        for name in ("Get-Status.ps1", "New-Manifest.ps1", "Submit-Manifest.ps1"):
            with self.subTest(name):
                self.assertIn(f"scripts/winget/{name}", text)
                self.assertTrue((ROOT / "scripts/winget" / name).is_file())

    def test_rel_041_the_skill_names_the_clone_location_and_the_gates(self):
        text = SKILL.read_text(encoding="utf-8")
        self.assertIn("/repos", text)
        self.assertIn("winget-pkgs", text)
        self.assertIn("Gate", text)

    def test_rel_041_the_skill_is_registered_for_every_host(self):
        frontmatter = SKILL.read_text(encoding="utf-8").split("---")[1]
        self.assertRegex(frontmatter, r"(?m)^name: winget$")
        self.assertRegex(frontmatter, r"(?m)^description: .{40,}")
        self.assertTrue((ROOT / ".claude/commands/winget.md").is_file())
        self.assertIn(".agents/skills/winget/SKILL.md", (ROOT / ".claude/commands/winget.md").read_text(encoding="utf-8"))
        opencode = json.loads((ROOT / "opencode.json").read_text(encoding="utf-8"))
        self.assertIn("winget", opencode["command"])
        self.assertIn(".agents/skills/winget/SKILL.md", (ROOT / "AGENTS.md").read_text(encoding="utf-8"))


@unittest.skipUnless(PWSH, "pwsh is required to run the winget scripts' functions")
class WingetLibraryTests(unittest.TestCase):
    def test_every_script_parses(self):
        for path in sorted((ROOT / "scripts/winget").glob("*.ps1")):
            with self.subTest(path.name):
                result = subprocess.run(
                    [
                        PWSH,
                        "-NoProfile",
                        "-Command",
                        "$errors = $null; [void][System.Management.Automation.Language.Parser]::ParseFile($env:WINGET_SCRIPT, [ref]$null, [ref]$errors); "
                        "$errors | ForEach-Object { \"line $($_.Extent.StartLineNumber): $($_.Message)\" }; exit [int](@($errors).Count -gt 0)",
                    ],
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    env={**os.environ, "WINGET_SCRIPT": str(path)},
                    timeout=60,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
    def test_version_comes_from_a_stable_tag_only(self):
        result = run_powershell(
            """
            $out = [ordered]@{ stable = ConvertTo-WingetVersion 'v0.3.0' }
            foreach ($bad in 'v0.3.0-rc.1', '0.3.0', 'v1.2', 'v1.2.3.4', 'latest') {
                try { [void](ConvertTo-WingetVersion $bad); $out[$bad] = 'accepted' } catch { $out[$bad] = 'rejected' }
            }
            $out | ConvertTo-Json
            """
        )
        self.assertEqual(result.pop("stable"), "0.3.0")
        self.assertEqual(set(result.values()), {"rejected"})

    def test_manifest_directory_follows_the_winget_pkgs_layout(self):
        result = run_powershell(
            """
            [ordered]@{
                version = Get-ManifestRelativeDir 'Dockering.Dockering' '0.3.0'
                package = Get-ManifestRelativeDir 'Dockering.Dockering' $null
                nested = Get-ManifestRelativeDir 'Microsoft.VisualStudio.Code' '1.0.0'
            } | ConvertTo-Json
            """
        )
        self.assertEqual(result["version"], "manifests/d/Dockering/Dockering/0.3.0")
        self.assertEqual(result["package"], "manifests/d/Dockering/Dockering")
        self.assertEqual(result["nested"], "manifests/m/Microsoft/VisualStudio/Code/1.0.0")

    def test_checksum_file_is_parsed_strictly(self):
        result = run_powershell(
            f"""
            $sums = ConvertFrom-ChecksumFile "{X64}  Dockering-Setup-x64.exe`n{ARM64} *Dockering-Setup-arm64.exe`n"
            $bad = try {{ [void](ConvertFrom-ChecksumFile "nonsense"); 'accepted' }} catch {{ 'rejected' }}
            [ordered]@{{ x64 = $sums['Dockering-Setup-x64.exe']; arm64 = $sums['Dockering-Setup-arm64.exe']; bad = $bad }} | ConvertTo-Json
            """
        )
        self.assertEqual(result, {"x64": X64, "arm64": ARM64, "bad": "rejected"})

    def test_release_channel_is_read_from_the_notes(self):
        result = run_powershell(
            """
            [ordered]@{
                unsigned = Get-ReleaseChannel '**Unsigned release:** Windows files are not signed.'
                signed = Get-ReleaseChannel '**Signed release:** `dockering.exe` is signed.'
                unknown = Get-ReleaseChannel 'nothing'
            } | ConvertTo-Json
            """
        )
        self.assertEqual(result, {"unsigned": "unsigned", "signed": "signed", "unknown": "unknown"})

    def test_clone_directory_is_on_the_drive_of_the_worktree(self):
        result = run_powershell(
            """
            Get-DefaultCloneDir 'D:\\worktrees\\dockering\\t3-68a850d0' | ConvertTo-Json
            """
        )
        if os.name == "nt":
            self.assertEqual(result, "D:\\repos\\winget-pkgs")
        else:
            self.assertEqual(Path(result), Path.home() / "repos" / "winget-pkgs")

    def test_templates_render_into_manifests_that_pass_the_read_back(self):
        with tempfile.TemporaryDirectory() as out:
            result = run_powershell(
                f"""
                $values = @{{ VERSION = '0.3.0'; X64_SHA256 = '{X64.upper()}'; ARM64_SHA256 = '{ARM64.upper()}' }}
                $dir = New-ManifestSet $env:TEMPLATES $env:OUT 'Dockering.Dockering' '0.3.0' $values
                $sums = @{{ 'Dockering-Setup-x64.exe' = '{X64}'; 'Dockering-Setup-arm64.exe' = '{ARM64}' }}
                $problems = Test-RenderedManifests $dir 'Dockering.Dockering' '0.3.0' 'pavel-purma/dockering' $sums
                [ordered]@{{ dir = $dir; problems = @($problems) }} | ConvertTo-Json
                """,
                TEMPLATES=str(TEMPLATES),
                OUT=out,
            )
            self.assertEqual(result["problems"], [])
            rendered = Path(result["dir"])
            self.assertEqual(rendered.relative_to(out).as_posix(), "manifests/d/Dockering/Dockering/0.3.0")
            installer = (rendered / f"{PACKAGE_ID}.installer.yaml").read_text(encoding="utf-8")
            self.assertNotIn("{{", installer)
            self.assertIn(f'InstallerSha256: "{X64.upper()}"', installer)
            self.assertIn("releases/download/v0.3.0/Dockering-Setup-arm64.exe", installer)
            self.assertIn("# yaml-language-server: $schema=", installer)
            self.assertNotIn("\r", installer)
            self.assertNotIn("Template (REL-040)", installer)
            self.assertNotIn("REL-040", installer)
            self.assertNotIn("Created with", installer)
            self.assertTrue(installer.startswith("# yaml-language-server: $schema="))

    def test_read_back_finds_a_wrong_checksum_url_version_and_identifier(self):
        with tempfile.TemporaryDirectory() as out:
            result = run_powershell(
                f"""
                $values = @{{ VERSION = '0.3.0'; X64_SHA256 = '{X64.upper()}'; ARM64_SHA256 = '{ARM64.upper()}' }}
                $dir = New-ManifestSet $env:TEMPLATES $env:OUT 'Dockering.Dockering' '0.3.0' $values
                $sums = @{{ 'Dockering-Setup-x64.exe' = '{"c" * 64}'; 'Dockering-Setup-arm64.exe' = '{ARM64}' }}
                $wrongSum = Test-RenderedManifests $dir 'Dockering.Dockering' '0.3.0' 'pavel-purma/dockering' $sums
                $wrongRepo = Test-RenderedManifests $dir 'Dockering.Dockering' '0.3.0' 'someone/else' $null
                $wrongVersion = Test-RenderedManifests $dir 'Dockering.Dockering' '0.4.0' 'pavel-purma/dockering' $null
                $wrongId = Test-RenderedManifests $dir 'Other.Package' '0.3.0' 'pavel-purma/dockering' $null
                [ordered]@{{ sum = @($wrongSum); repo = @($wrongRepo); version = @($wrongVersion); id = @($wrongId) }} | ConvertTo-Json
                """,
                TEMPLATES=str(TEMPLATES),
                OUT=out,
            )
        self.assertTrue(any("differs from SHA256SUMS" in problem for problem in result["sum"]), result["sum"])
        self.assertTrue(any("is not under https://github.com/someone/else/" in problem for problem in result["repo"]), result["repo"])
        self.assertTrue(any("PackageVersion is not 0.4.0" in problem for problem in result["version"]), result["version"])
        self.assertTrue(any("missing Other.Package" in problem for problem in result["id"]), result["id"])

    def test_an_unfilled_placeholder_is_an_error(self):
        result = run_powershell(
            """
            $out = [ordered]@{}
            try { [void](Expand-ManifestTemplate "PackageVersion: `"{{VERSION}}`"`nSha: {{X64_SHA256}}" @{ VERSION = '1.0.0' }); $out.missing = 'accepted' }
            catch { $out.missing = $_.Exception.Message }
            $out | ConvertTo-Json
            """
        )
        self.assertIn("X64_SHA256", result["missing"])

    def test_the_template_set_must_hold_exactly_the_three_manifests(self):
        with tempfile.TemporaryDirectory() as templates, tempfile.TemporaryDirectory() as out:
            (Path(templates) / f"{PACKAGE_ID}.yaml").write_text("PackageIdentifier: x\n", encoding="utf-8")
            result = run_powershell(
                """
                try { [void](New-ManifestSet $env:TEMPLATES $env:OUT 'Dockering.Dockering' '0.3.0' @{}); 'accepted' | ConvertTo-Json }
                catch { $_.Exception.Message | ConvertTo-Json }
                """,
                TEMPLATES=templates,
                OUT=out,
            )
        self.assertIn("expected", result)

    def test_pull_request_body_ticks_only_what_was_verified(self):
        with tempfile.TemporaryDirectory() as scratch:
            template = Path(scratch) / "template.md"
            template.write_text(PR_TEMPLATE, encoding="utf-8")
            result = run_powershell(
                """
                $template = [IO.File]::ReadAllText($env:TEMPLATE)
                $ticks = @{ NoOtherPrs = $true; OneManifest = $true; Validated = $true; Schema = $true; InstallTested = $false }
                $pr = Format-PullRequestBody $template "Dockering.Dockering 0.3.0." $ticks "Generated with a tool."
                [ordered]@{ body = $pr.Body; unmatched = @($pr.Unmatched) } | ConvertTo-Json
                """,
                TEMPLATE=str(template),
            )
        body = result["body"]
        self.assertEqual(result["unmatched"], [])
        self.assertIn("- [ ] Signed the [Contributor License Agreement]", body)
        self.assertIn("- [ ] Tested manifest locally", body)
        self.assertIn("- [ ] Linked to an issue", body)
        for text in ("Checked that there aren't other open", "only modifies one (1) manifest", "Validated manifest locally", "Manifest conforms to"):
            with self.subTest(text):
                self.assertRegex(body, rf"- \[x\] .*{re.escape(text)}")
        # the description sits under its heading, after the template's own comment, before the next heading
        self.assertRegex(body, r"## Description\n<!-- Describe what this PR changes\. -->\n\nDockering\.Dockering 0\.3\.0\.\n\n## Checklist")
        self.assertTrue(body.rstrip().endswith("Generated with a tool."))

    def test_installing_the_manifest_in_a_sandbox_ticks_the_install_box(self):
        with tempfile.TemporaryDirectory() as scratch:
            template = Path(scratch) / "template.md"
            template.write_text(PR_TEMPLATE, encoding="utf-8")
            result = run_powershell(
                """
                $template = [IO.File]::ReadAllText($env:TEMPLATE)
                $pr = Format-PullRequestBody $template 'x' @{ InstallTested = $true } $null
                $pr.Body | ConvertTo-Json
                """,
                TEMPLATE=str(template),
            )
        self.assertRegex(result, r"- \[x\] Tested manifest locally")
        self.assertIn("- [ ] Signed the", result)

    def test_a_template_that_changed_is_reported_not_guessed(self):
        result = run_powershell(
            """
            $pr = Format-PullRequestBody "## Description`n`n- [ ] Something new`n" 'x' @{ Validated = $true; Schema = $true } $null
            [ordered]@{ body = $pr.Body; unmatched = @($pr.Unmatched) } | ConvertTo-Json
            """
        )
        self.assertEqual(sorted(result["unmatched"]), ["Schema", "Validated"])
        self.assertNotIn("[x]", result["body"])

    def test_the_built_in_template_has_every_box_the_script_ticks(self):
        result = run_powershell(
            """
            $ticks = @{ NoOtherPrs = $true; OneManifest = $true; Validated = $true; Schema = $true; InstallTested = $true }
            $pr = Format-PullRequestBody $FallbackPullRequestTemplate 'x' $ticks $null
            ConvertTo-Json -InputObject @($pr.Unmatched)
            """
        )
        self.assertEqual(result, [])


if __name__ == "__main__":
    unittest.main()
