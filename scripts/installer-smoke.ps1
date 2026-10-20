# Installer smoke test (REL-014, REL-021, REL-025, REL-027, REL-028) and the updater's update leg (REL-087 b).
# Usage: pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup <Dockering-Setup-x64.exe> -Version <semver> [-AllUsers]
# Silent install → files, GUI subsystem, version, uninstall entry, Start Menu shortcut → reinstall
# (repair) → silent uninstall → nothing left. -AllUsers needs an elevated shell (CI runners are).
#
# Update leg (REL-087 b, UPD-007), a separate run that replaces the default legs (CI runs those first, in the same job):
#   pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup <new installer> -Version <new semver> `
#        -UpdateFrom <old per-arch installer> -UpdateFromSums <SHA256SUMS of that release> [-AllowSameVersion]
# -UpdateFrom and -UpdateFromSums go together. The leg is per user (no -AllUsers):
#   1. the old installer's SHA-256 must equal the line for its file name in SHA256SUMS (a missing line or a mismatch fails);
#   2. the old build is installed per user and silently, exactly like the default per-user leg; `dockering --version` is recorded;
#   3. the NEW installer (-Setup) runs with exactly the updater's argv: $UpdaterArgs, the list that
#      installer_args(InstallKind::InnoUser) builds in crates/dk-update/src/apply/mod.rs
#      (scripts/tests/test_installer_smoke_update.py keeps the two equal);
#   4. asserted: installer exit code 0; `dockering --version` is `dockering <Version>` and differs from the old one; the per-user
#      uninstall entry shows the new DisplayVersion; the install dir has its files and the Start Menu shortcut; /RELAUNCH started
#      a dockering.exe from the install dir (a process, never a window), which is then stopped;
#   5. a silent uninstall leaves no install dir, uninstall entry or shortcut.
# -AllowSameVersion: the old and the new build report the same version when the last release IS the workspace version (the usual
# state between releases). "Differs" can't hold then, so that one assertion is skipped with a visible SKIP line and all others
# still run. Without the switch (a version bump) equal versions fail.
#
# NOT verified by this leg (spike W1, docs/plan/features/linux-appimage-updates.md §6.6; the manual items are in
# docs/plan/release-checklist.md; nothing here is a Windows result until a run on Windows says so):
#   - Mark-of-the-Web and SmartScreen: both installers are files from the build and `gh release download`, not written by the app;
#   - Defender and Smart App Control: whether they block an unsigned app starting an unsigned installer;
#   - the UAC prompt and its 'Unknown publisher' text, and the all-users relaunch through Explorer (this leg is per user);
#   - the installer starting while the app holds it FILE_SHARE_READ-locked (UPD-007): nothing locks it here;
#   - cancelling the installer, and a window appearing after /RELAUNCH (only that the process started is asserted).
# The leg itself is checked on Windows only by a green run on windows-2025 and windows-11-arm.
param(
    [Parameter(Mandatory)] [string] $Setup,
    [Parameter(Mandatory)] [string] $Version,
    [switch] $AllUsers,
    [string] $UpdateFrom,
    [string] $UpdateFromSums,
    [switch] $AllowSameVersion
)
$ErrorActionPreference = 'Stop'

$appId = '{8C3F4E2A-6B1D-4E7A-9F2C-5D8B1A3E7C64}_is1'
$silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-')
# The updater's installer argv for a per-user Inno install, the ONE list the update leg reads. Keep it equal, in the same order,
# to installer_args(InstallKind::InnoUser) in crates/dk-update/src/apply/mod.rs (a test compares them).
$UpdaterArgs = @('/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/UPDATE', '/RELAUNCH', '/CURRENTUSER')
$InstallerTimeoutSec = 300
$RelaunchTimeoutSec = 30

function Fail([string] $message) {
    Write-Host "::error::$message"
    exit 1
}

function Get-PeSubsystem([string] $Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadInt32() + 4 + 20 + 68
        return $reader.ReadUInt16()
    } finally { $stream.Dispose() }
}

function Install([string] $what) {
    $p = Start-Process (Resolve-Path $Setup) -ArgumentList ($silent + $scope) -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$what exited with $($p.ExitCode)" }
}

# ── update leg helpers (REL-087 b) ─────────────────────────────────────────────────────────────────

# Pure (no output, no exit): is $File the file that $SumsFile lists under its name? Returns an object with Ok, Name, Expected,
# Actual and Reason. $SumsFile is GNU sha256sum output: `<64 hex>  <name>` (text) or `<64 hex> *<name>` (binary). The whole file
# name must match (case-sensitive) and exactly one line must name it. A missing line is a failure, never a pass.
function Test-Sha256Sums([string] $File, [string] $SumsFile) {
    $name = [IO.Path]::GetFileName($File)
    $verdict = [pscustomobject]@{ Ok = $false; Name = $name; Expected = ''; Actual = ''; Reason = '' }
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) {
        $verdict.Reason = "installer not found: $File"
        return $verdict
    }
    if (-not (Test-Path -LiteralPath $SumsFile -PathType Leaf)) {
        $verdict.Reason = "SHA256SUMS not found: $SumsFile"
        return $verdict
    }
    $listed = @()
    foreach ($line in (Get-Content -LiteralPath $SumsFile)) {
        if ($line -match '^([0-9A-Fa-f]{64}) [ *](.+?)\s*$' -and $Matches[2] -ceq $name) { $listed += $Matches[1].ToLowerInvariant() }
    }
    if ($listed.Count -ne 1) {
        $verdict.Reason = if ($listed.Count -eq 0) { "SHA256SUMS has no line for $name" } else { "SHA256SUMS lists $name $($listed.Count) times" }
        return $verdict
    }
    $verdict.Expected = $listed[0]
    $verdict.Actual = (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($verdict.Actual -ne $verdict.Expected) {
        $verdict.Reason = "SHA-256 of $name is $($verdict.Actual), SHA256SUMS says $($verdict.Expected)"
        return $verdict
    }
    $verdict.Ok = $true
    return $verdict
}

# Pure: the candidates (Get-Process objects, or anything with a Path) whose executable sits directly in $Directory.
function Select-ProcessInDirectory([object[]] $Candidate, [string] $Directory) {
    $want = [IO.Path]::GetFullPath($Directory).TrimEnd('\', '/')
    foreach ($process in $Candidate) {
        if (-not $process.Path) { continue }
        $parent = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($process.Path))
        if ([string]::Equals($parent, $want, [StringComparison]::OrdinalIgnoreCase)) { $process }
    }
}

# The running dockering processes whose executable is in the install dir.
function Get-InstalledProcess([string] $InstallDir) {
    Select-ProcessInDirectory (Get-Process -Name dockering -ErrorAction SilentlyContinue) $InstallDir
}

# `dockering.exe --version` the way the default legs run it (a GUI-subsystem exe prints nothing to a pipe in PowerShell, so the
# output goes to a file through Start-Process -Wait -RedirectStandardOutput, REL-028). Returns ExitCode and Output.
function Get-ReportedVersion([string] $Exe) {
    $out = Join-Path ([IO.Path]::GetTempPath()) "dockering-version-$PID-$([guid]::NewGuid().ToString('N')).txt"
    $p = Start-Process $Exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $out
    $text = if (Test-Path $out) { ([string](Get-Content $out -Raw)).Trim() } else { '' }
    Remove-Item $out -ErrorAction SilentlyContinue
    return [pscustomobject]@{ ExitCode = $p.ExitCode; Output = $text }
}

# The update leg proper. Reads the script-level $Setup (the NEW installer), $Version, $dir, $key, $shortcut, $silent and
# $UpdaterArgs, like the default legs do. Every failure goes through Fail (::error:: and exit 1).
function Invoke-UpdateLeg([string] $Old, [string] $OldSums, [bool] $SameVersionOk) {
    # A relaunched app would look for updates after 30 s; nothing here wants that (plan decision 14). Best effort: that the
    # installer hands its environment on to the relaunched process is not verified.
    $env:DOCKERING_DISABLE_UPDATES = '1'
    $exe = Join-Path $dir 'dockering.exe'

    # (1) the old installer is the one that was published
    $sum = Test-Sha256Sums $Old $OldSums
    if (-not $sum.Ok) { Fail "old installer rejected: $($sum.Reason)" }
    Write-Host "old installer $($sum.Name) matches SHA256SUMS ($($sum.Actual))"

    # (2) install the old build per user, like the default per-user leg
    $oldPath = (Resolve-Path -LiteralPath $Old).ProviderPath
    $p = Start-Process $oldPath -ArgumentList ($silent + '/CURRENTUSER') -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "old installer exited with $($p.ExitCode)" }
    if (-not (Test-Path $exe)) { Fail "old install has no dockering.exe in $dir" }
    $before = Get-ReportedVersion $exe
    if ($before.ExitCode -ne 0 -or $before.Output -notmatch '^dockering \S+$') {
        Fail "old --version exited $($before.ExitCode) and printed '$($before.Output)', expected 'dockering <version>'"
    }
    $oldHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    Write-Host "old release installed into ${dir}: '$($before.Output)'"

    # (3) run the new installer with exactly the updater's argv. Not `Start-Process -Wait`: /RELAUNCH starts dockering.exe from
    # the installer, and this must not depend on whether -Wait also waits for such a child. Watching for the relaunched process
    # while the installer runs also catches an app that starts and exits at once. `.Handle` keeps ExitCode readable.
    $newPath = (Resolve-Path -LiteralPath $Setup).ProviderPath
    $installer = Start-Process $newPath -ArgumentList $UpdaterArgs -PassThru
    $null = $installer.Handle
    $seen = [System.Collections.Generic.HashSet[int]]::new()
    $deadline = [DateTime]::UtcNow.AddSeconds($InstallerTimeoutSec)
    while (-not $installer.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline) {
            Stop-Process -Id $installer.Id -Force -ErrorAction SilentlyContinue
            Fail "update installer still running after $InstallerTimeoutSec s"
        }
        foreach ($process in (Get-InstalledProcess $dir)) { [void] $seen.Add($process.Id) }
        Start-Sleep -Milliseconds 100
    }
    if ($installer.ExitCode -ne 0) { Fail "update installer exited with $($installer.ExitCode)" }
    Write-Host "update installer ran with '$($UpdaterArgs -join ' ')' and exited 0"

    # (4) /RELAUNCH: a dockering.exe from the install dir is running. Only the process is asserted, never a window (a runner
    # without a usable display may show none). Stop it afterwards so the files can be replaced or removed.
    $until = [DateTime]::UtcNow.AddSeconds($RelaunchTimeoutSec)
    while ($seen.Count -eq 0 -and [DateTime]::UtcNow -lt $until) {
        foreach ($process in (Get-InstalledProcess $dir)) { [void] $seen.Add($process.Id) }
        if ($seen.Count -eq 0) { Start-Sleep -Milliseconds 250 }
    }
    if ($seen.Count -eq 0) { Fail "/RELAUNCH did not start $exe within $RelaunchTimeoutSec s of the installer exiting" }
    Write-Host "/RELAUNCH started dockering.exe (pid $((@($seen) | Sort-Object) -join ', ')); no window is asserted"
    foreach ($id in @($seen)) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    for ($i = 0; $i -lt 40 -and (Get-InstalledProcess $dir); $i++) {
        foreach ($process in (Get-InstalledProcess $dir)) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Milliseconds 250
    }
    if (Get-InstalledProcess $dir) { Fail "dockering.exe from $dir is still running after the relaunch check" }

    # the version, the uninstall entry, the files
    $after = Get-ReportedVersion $exe
    if ($after.ExitCode -ne 0 -or $after.Output -ne "dockering $Version") {
        Fail "after the update --version exited $($after.ExitCode) and printed '$($after.Output)', expected 'dockering $Version'"
    }
    if ($after.Output -eq $before.Output) {
        if (-not $SameVersionOk) {
            Fail "old and new both report '$($after.Output)': nothing shows the update happened (pass -AllowSameVersion only when the last release is this version)"
        }
        Write-Host "SKIP: old and new both report '$($after.Output)', so the version change is not asserted (-AllowSameVersion)"
    } else {
        Write-Host "version changed: '$($before.Output)' -> '$($after.Output)'"
    }
    $newHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    Write-Host "dockering.exe SHA-256 $oldHash -> $newHash$(if ($newHash -eq $oldHash) { ' (unchanged; informational)' })"
    $entry = Get-ItemProperty $key -ErrorAction SilentlyContinue
    if (-not $entry -or $entry.DisplayVersion -ne $Version) { Fail "uninstall entry missing or wrong version at $key after the update" }
    foreach ($f in 'dockering.exe', 'LICENSE', 'THIRD_PARTY_LICENSES.html', 'unins000.exe') {
        if (-not (Test-Path (Join-Path $dir $f))) { Fail "missing $f in $dir after the update" }
    }
    if (-not (Test-Path $shortcut)) { Fail "Start Menu shortcut missing after the update: $shortcut" }
    Write-Host "updated to $Version in $dir"

    # (5) uninstall
    $p = Start-Process (Join-Path $dir 'unins000.exe') -ArgumentList $silent -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "uninstall exited with $($p.ExitCode)" }
    # The uninstaller deletes itself through a helper process; give it a moment.
    for ($i = 0; $i -lt 20 -and (Test-Path $dir); $i++) { Start-Sleep -Milliseconds 500 }
    if (Test-Path $dir) { Fail "install dir still exists: $dir" }
    if (Test-Path $key) { Fail "uninstall entry still exists: $key" }
    if (Test-Path $shortcut) { Fail "Start Menu shortcut still exists: $shortcut" }
    Write-Host "uninstalled cleanly"
}

# Dot-sourced (`. scripts/installer-smoke.ps1 -Setup x -Version 0`, as the tests do) the file only defines its functions and
# variables; nothing below touches the machine.
if ($MyInvocation.InvocationName -eq '.') { return }

if (($UpdateFrom -and -not $UpdateFromSums) -or ($UpdateFromSums -and -not $UpdateFrom)) {
    Fail '-UpdateFrom and -UpdateFromSums go together: give both or neither'
}
if ($UpdateFrom -and $AllUsers) { Fail '-UpdateFrom runs the per-user update leg; do not combine it with -AllUsers' }
if ($AllowSameVersion -and -not $UpdateFrom) { Fail '-AllowSameVersion only applies to the update leg (-UpdateFrom)' }

if ($AllUsers) {
    $scope = '/ALLUSERS'
    $dir = Join-Path $env:ProgramFiles 'Dockering'
    $key = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
    $programs = [Environment]::GetFolderPath('CommonPrograms')
} else {
    $scope = '/CURRENTUSER'
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\Dockering'
    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
    $programs = [Environment]::GetFolderPath('Programs')
}
$shortcut = Join-Path $programs 'Dockering.lnk'

if ($UpdateFrom) {
    Invoke-UpdateLeg $UpdateFrom $UpdateFromSums $AllowSameVersion.IsPresent
    exit 0
}

Install 'install'
foreach ($f in 'dockering.exe', 'LICENSE', 'THIRD_PARTY_LICENSES.html', 'unins000.exe') {
    if (-not (Test-Path (Join-Path $dir $f))) { Fail "missing $f in $dir" }
}
$subsystem = Get-PeSubsystem (Join-Path $dir 'dockering.exe')
if ($subsystem -ne 2) { Fail "dockering.exe has PE subsystem $subsystem, expected 2 (Windows GUI, no console window)" }
$versionOut = Join-Path ([IO.Path]::GetTempPath()) "dockering-version-$PID.txt"
$p = Start-Process (Join-Path $dir 'dockering.exe') -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $versionOut
$reported = if (Test-Path $versionOut) { (Get-Content $versionOut -Raw).Trim() } else { '' }
Remove-Item $versionOut -ErrorAction SilentlyContinue
if ($p.ExitCode -ne 0 -or $reported -ne "dockering $Version") { Fail "--version exited $($p.ExitCode) and printed '$reported', expected 'dockering $Version'" }
$info = (Get-Item (Join-Path $dir 'dockering.exe')).VersionInfo
if ($info.ProductVersion -ne $Version -or $info.ProductName -ne 'Dockering') {
    Fail "VERSIONINFO is '$($info.ProductName) $($info.ProductVersion)'"
}
$entry = Get-ItemProperty $key -ErrorAction SilentlyContinue
if (-not $entry -or $entry.DisplayVersion -ne $Version) { Fail "uninstall entry missing or wrong version at $key" }
if (-not (Test-Path $shortcut)) { Fail "Start Menu shortcut missing: $shortcut" }
Write-Host "installed $Version into $dir"

Install 'reinstall'

$p = Start-Process (Join-Path $dir 'unins000.exe') -ArgumentList $silent -Wait -PassThru
if ($p.ExitCode -ne 0) { Fail "uninstall exited with $($p.ExitCode)" }
# The uninstaller deletes itself through a helper process; give it a moment.
for ($i = 0; $i -lt 20 -and (Test-Path $dir); $i++) { Start-Sleep -Milliseconds 500 }
if (Test-Path $dir) { Fail "install dir still exists: $dir" }
if (Test-Path $key) { Fail "uninstall entry still exists: $key" }
if (Test-Path $shortcut) { Fail "Start Menu shortcut still exists: $shortcut" }
Write-Host "uninstalled cleanly"
