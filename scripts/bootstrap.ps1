#!/usr/bin/env pwsh
<#
.SYNOPSIS
Checks the Windows native prerequisites required by GPUI Kit.

.PARAMETER Install
Installs Visual Studio 2022 Build Tools with the C++ workload through winget,
then performs the same non-destructive checks.
#>
[CmdletBinding()]
param([switch]$Install)

$ErrorActionPreference = 'Stop'

if ($Install) {
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if (-not $winget) {
        throw 'winget was not found. Install App Installer, then rerun with -Install.'
    }

    Write-Host 'Installing Visual Studio 2022 Build Tools (C++ workload and recommended components)...'
    & winget install --id Microsoft.VisualStudio.2022.BuildTools --exact `
        --accept-package-agreements --accept-source-agreements `
        --override '--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    if ($LASTEXITCODE -ne 0) { throw "winget exited with code $LASTEXITCODE" }
}

$failures = [System.Collections.Generic.List[string]]::new()
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    $failures.Add('vswhere.exe is missing (install Visual Studio 2022 Build Tools).')
} else {
    $installations = @(& $vswhere -all -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    $validInstall = $null

    foreach ($installation in $installations) {
        $vcvars = Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
        $crt = Get-ChildItem -Path (Join-Path $installation 'VC\Tools\MSVC') -Filter msvcrt.lib -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '[\/]lib[\/]x64[\/]msvcrt\.lib$' } |
            Select-Object -First 1
        if ($crt -and (Test-Path $vcvars)) {
            $validInstall = $installation
            Write-Host "MSVC x64 tools: OK ($installation)"
            Write-Host "x64 CRT: OK ($($crt.FullName))"
            break
        }
    }

    if (-not $validInstall) {
        $failures.Add('No VS installation has vcvars64.bat plus the x64 CRT (msvcrt.lib).')
    } else {
        $bundledCMake = Join-Path $validInstall 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
        if ((Get-Command cmake -ErrorAction SilentlyContinue) -or (Test-Path $bundledCMake)) {
            Write-Host 'CMake: OK'
        } else {
            $failures.Add('CMake is missing. Add the C++ CMake tools component to Build Tools.')
        }
    }
}

$sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdkLib = Get-ChildItem -Path (Join-Path $sdkRoot 'Lib\*\um\x64\kernel32.lib') -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1
if ($sdkLib) {
    Write-Host "Windows SDK: OK ($($sdkLib.FullName))"
} else {
    $failures.Add('Windows 10/11 SDK x64 libraries are missing.')
}

if ($failures.Count -gt 0) {
    Write-Error (($failures | ForEach-Object { "- $_" }) -join "`n")
    Write-Host "`nFix automatically: pwsh -NoProfile scripts/bootstrap.ps1 -Install"
    Write-Host 'Or modify Visual Studio Build Tools and select Desktop development with C++ (recommended components included).'
    exit 1
}

Write-Host 'All Windows build prerequisites are available.'
Write-Host 'Build with: pwsh -NoProfile scripts/dev.ps1 <cargo args>'
