# Installer smoke test (REL-014, REL-021, REL-025, REL-027).
# Usage: pwsh -NoProfile -File scripts/installer-smoke.ps1 -Setup <Dockering-Setup-x64.exe> -Version <semver> [-AllUsers]
# Silent install → files, version, uninstall entry, Start Menu shortcut → reinstall (repair) → silent
# uninstall → nothing left. -AllUsers needs an elevated shell (CI runners are).
param(
    [Parameter(Mandatory)] [string] $Setup,
    [Parameter(Mandatory)] [string] $Version,
    [switch] $AllUsers
)
$ErrorActionPreference = 'Stop'

$appId = '{8C3F4E2A-6B1D-4E7A-9F2C-5D8B1A3E7C64}_is1'
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
$silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-')

function Fail([string] $message) {
    Write-Host "::error::$message"
    exit 1
}

function Install([string] $what) {
    $p = Start-Process (Resolve-Path $Setup) -ArgumentList ($silent + $scope) -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$what exited with $($p.ExitCode)" }
}

Install 'install'
foreach ($f in 'dockering.exe', 'LICENSE', 'THIRD_PARTY_LICENSES.html', 'unins000.exe') {
    if (-not (Test-Path (Join-Path $dir $f))) { Fail "missing $f in $dir" }
}
$reported = & (Join-Path $dir 'dockering.exe') --version
if ($reported -ne "dockering $Version") { Fail "--version printed '$reported', expected 'dockering $Version'" }
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
