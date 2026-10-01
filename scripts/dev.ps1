#!/usr/bin/env pwsh
# Runs cargo inside the Visual Studio Build Tools x64 environment (spike F-1).
# Usage: pwsh -NoProfile scripts/dev.ps1 build -p dockering
#        pwsh -NoProfile scripts/dev.ps1 test --workspace
$ErrorActionPreference = 'Stop'

function Find-VcVars {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { throw 'vswhere.exe not found. Install Visual Studio Build Tools (see scripts/bootstrap.ps1).' }
    # Prefer installs that actually ship the x64 CRT (msvcrt.lib); a VS install without it fails at link time.
    $paths = & $vswhere -all -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    foreach ($p in $paths) {
        $crt = Get-ChildItem -Path (Join-Path $p 'VC\Tools\MSVC') -Filter msvcrt.lib -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -eq 'x64' -and $_.Directory.Parent.Name -eq 'lib' } | Select-Object -First 1
        $vcvars = Join-Path $p 'VC\Auxiliary\Build\vcvars64.bat'
        if ($crt -and (Test-Path $vcvars)) { return $vcvars }
    }
    throw 'No Visual Studio install with the x64 CRT (msvcrt.lib) was found. Run scripts/bootstrap.ps1.'
}

if (-not $env:DOCKERING_VCVARS_LOADED) {
    $vcvars = Find-VcVars
    $envDump = cmd /c "`"$vcvars`" >nul 2>&1 && set"
    foreach ($line in $envDump) {
        if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2]) }
    }
    $env:DOCKERING_VCVARS_LOADED = '1'
}

& cargo @args
exit $LASTEXITCODE
