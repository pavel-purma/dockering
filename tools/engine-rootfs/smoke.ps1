#!/usr/bin/env pwsh
# Smoke test of one engine root filesystem on Windows (REL-088, ENG-152, ENG-153, ENG-164).
#
#   pwsh -NoProfile -File smoke.ps1 -Tar <dockering-engine-<image>-x86_64.tar.xz> -Image alpine|ubuntu `
#        -DistroName <name> -WorkDir <empty dir> [-Port 18480]
#
# Imports the asset into a throwaway distro, lets the image start dockerd the way it ships (the Alpine boot
# command, or systemd for Ubuntu), then checks: GET /_ping answers within 60 s, `docker run hello-world`, a
# published port reachable from Windows, a bind mount from the Windows drive, and that the import added no
# Start Menu shortcut or Windows Terminal profile (ENG-164). The distro is ALWAYS unregistered afterwards.
#
# Safety: only the distro named -DistroName is ever touched, and the name must start with `dockering-smoke-`
# or `dockering-spike-rootfs-`. Docker Desktop and every other distro are left alone.
#
#   smoke.ps1 -CleanupOnly -DistroName <name> -WorkDir <dir>
#
# unregisters the distro (a missing one is fine) and deletes the work directory, whose leaf name must start with
# `engine-rootfs-smoke`. The workflow runs it in an `always()` step, because a cancelled job never reaches the
# `finally` below.
[CmdletBinding(DefaultParameterSetName = 'Run')]
param(
    [Parameter(ParameterSetName = 'Run', Mandatory)][string] $Tar,
    [Parameter(ParameterSetName = 'Run', Mandatory)][ValidateSet('alpine', 'ubuntu')][string] $Image,
    [Parameter(ParameterSetName = 'Cleanup', Mandatory)][switch] $CleanupOnly,
    [Parameter(Mandatory)][string] $DistroName,
    [Parameter(Mandatory)][string] $WorkDir,
    [Parameter(ParameterSetName = 'Run')][ValidateRange(1024, 65535)][int] $Port = 18480
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($DistroName -notmatch '^(dockering-smoke|dockering-spike-rootfs)-[A-Za-z0-9._-]{1,60}$') {
    throw "refusing to use the distro name '$DistroName'"
}
if ($WorkDir -match "[']") { throw 'the work directory must not contain a single quote' }
if (-not $CleanupOnly) { $Tar = (Resolve-Path -LiteralPath $Tar).Path }
$Label = if ($CleanupOnly) { 'cleanup' } else { $Image }
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
$WorkDir = (Resolve-Path -LiteralPath $WorkDir).Path
$distroDir = Join-Path $WorkDir 'distro'
$bindDir = Join-Path $WorkDir 'bind'
$script:Failures = New-Object System.Collections.Generic.List[string]
$script:Holder = $null
$script:Imported = $false

function Write-Step([string] $Text) { Write-Host "smoke[$Label]: $Text" }
function Pass([string] $Check) { Write-Host "PASS $Label $Check" }
function Fail([string] $Check, [string] $Why) {
    Write-Host "FAIL $Label $Check : $Why"
    $script:Failures.Add("$Check : $Why")
}

# wsl.exe management commands print UTF-16LE.
function Invoke-WslManagement([string[]] $Arguments) {
    $previous = [Console]::OutputEncoding
    [Console]::OutputEncoding = [Text.Encoding]::Unicode
    try {
        $out = & wsl.exe @Arguments 2>&1 | Out-String
        return [pscustomobject]@{ Code = $LASTEXITCODE; Text = $out.Trim() }
    } finally {
        [Console]::OutputEncoding = $previous
    }
}

$script:GuestCount = 0
$script:WorkUnix = $null

# Runs a /bin/sh script as root in the distro. The script goes in a file under WorkDir (LF, no BOM), so no
# quoting is needed and no stdin newline can leak into it.
function Invoke-Guest([string] $Script, [int] $TimeoutSeconds = 180) {
    if (-not $script:WorkUnix) {
        $script:WorkUnix = (& wsl.exe -d $DistroName -u root --exec wslpath -a -u $WorkDir | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or -not $script:WorkUnix) { throw "wslpath failed for $WorkDir" }
    }
    $script:GuestCount++
    $name = "guest-$($script:GuestCount).sh"
    [IO.File]::WriteAllText((Join-Path $WorkDir $name), ($Script -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
    $process = Start-Process -FilePath wsl.exe -PassThru -NoNewWindow -Wait:$false `
        -ArgumentList @('-d', $DistroName, '-u', 'root', '--exec', '/bin/sh', "$($script:WorkUnix)/$name") `
        -RedirectStandardOutput (Join-Path $WorkDir "$name.out") -RedirectStandardError (Join-Path $WorkDir "$name.err")
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try { $process.Kill($true) } catch { }
        return [pscustomobject]@{ Code = 124; Text = "timed out after $TimeoutSeconds s" }
    }
    $text = ((Get-Content -Raw -LiteralPath (Join-Path $WorkDir "$name.out")) + (Get-Content -Raw -LiteralPath (Join-Path $WorkDir "$name.err"))).Trim()
    return [pscustomobject]@{ Code = $process.ExitCode; Text = $text }
}

function Remove-Distro {
    if ($script:Holder -and -not $script:Holder.HasExited) {
        try { $script:Holder.Kill($true) } catch { }
    }
    # Unregister even when the import may not have finished: "no such distribution" is a clean state.
    $result = Invoke-WslManagement @('--unregister', $DistroName)
    if ($result.Code -eq 0) { Write-Step "unregistered $DistroName" }
    else { Write-Step "unregister $DistroName : exit $($result.Code) (already gone?) $($result.Text)" }
}

if ($CleanupOnly) {
    Remove-Distro
    if ((Split-Path -Leaf $WorkDir) -like 'engine-rootfs-smoke*') {
        Remove-Item -LiteralPath $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
    } else {
        Write-Step "not deleting $WorkDir (its name does not start with engine-rootfs-smoke)"
    }
    exit 0
}

try {
    if ((Invoke-WslManagement @('-l', '-q')).Text -split "\r?\n" | Where-Object { $_.Trim() -eq $DistroName }) {
        throw "a distro named $DistroName already exists; refusing to touch it"
    }

    # 1. import (the parent of the distro directory must exist; the leaf must not hold an ext4.vhdx).
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $started = Get-Date
    $script:Imported = $true
    $import = Invoke-WslManagement @('--import', $DistroName, $distroDir, $Tar, '--version', '2')
    if ($import.Code -ne 0) { throw "wsl --import failed ($($import.Code)): $($import.Text)" }
    Pass ("import ({0:n1} s)" -f ((Get-Date) - $started).TotalSeconds)

    # 2. ENG-164: no Start Menu shortcut and no Windows Terminal profile were registered for the import.
    $lxss = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' | Where-Object {
        $p = (Get-ItemProperty -LiteralPath $_.PSPath).PSObject.Properties['DistributionName']
        $p -and $p.Value -eq $DistroName
    }
    if (-not $lxss) { Fail 'registry' 'the imported distro has no Lxss key' }
    else {
        $props = Get-ItemProperty -LiteralPath $lxss.PSPath
        $names = @($props.PSObject.Properties.Name)
        $bad = @('ShortcutPath', 'TerminalProfilePath') | Where-Object { $names -contains $_ }
        if ($bad) { Fail 'eng-164' "the import registered: $($bad -join ', ')" } else { Pass 'eng-164 no shortcut, no terminal profile' }
    }

    # 3. start the image the way it ships and keep a session open (the distro idles out ~15 s after the last one).
    $script:Holder = Start-Process -FilePath wsl.exe -PassThru -WindowStyle Hidden `
        -ArgumentList @('-d', $DistroName, '-u', 'root', '--exec', '/bin/sleep', '1800')

    # 4. GET /_ping through the same bridge Dockering uses (`docker system dial-stdio`), up to 60 s.
    $ping = @'
printf 'GET /_ping HTTP/1.0\r\nHost: docker\r\n\r\n' | docker system dial-stdio 2>&1 | head -n 1
'@
    $deadline = (Get-Date).AddSeconds(60)
    $ready = $false
    $answer = [pscustomobject]@{ Code = -1; Text = '' }
    $attempts = 0
    $startedPing = Get-Date
    while ((Get-Date) -lt $deadline) {
        $attempts++
        $answer = Invoke-Guest $ping 20
        if ($answer.Text -match '^HTTP/1\.[01] 200') { $ready = $true; break }
        Start-Sleep -Seconds 1
    }
    if ($ready) { Pass ("GET /_ping answered 200 after {0:n1} s ({1} polls)" -f ((Get-Date) - $startedPing).TotalSeconds, $attempts) }
    else {
        Fail 'ping' "no 200 from GET /_ping within 60 s; last answer: $($answer.Text)"
        $log = Invoke-Guest 'tail -n 40 /var/log/dockerd.log 2>/dev/null; journalctl -u docker --no-pager -n 40 2>/dev/null; true' 30
        Write-Host $log.Text
        throw 'dockerd never became ready'
    }

    # 5. the manifest and configuration the image promises
    $info = Invoke-Guest @'
set -eu
test -f /etc/dockering-engine-manifest.txt
grep -q '^docker:' /etc/group
test -f /etc/wsl.conf
test -f /etc/wsl-distribution.conf
grep -q 'enabled = false' /etc/wsl-distribution.conf
test ! -e /var/cache/apk || test -z "$(ls -A /var/cache/apk)"
test ! -d /var/lib/apt/lists || test -z "$(ls /var/lib/apt/lists | grep -v '^partial$\|^lock$')"
sed -n 's/^\(docker-ce\|docker\) \(.*\)$/\1 \2/p' /etc/dockering-engine-manifest.txt | head -n 1
docker version --format 'server {{.Server.Version}} api {{.Server.APIVersion}}'
'@ 60
    if ($info.Code -eq 0) { Pass "image contents ($($info.Text -replace "`n", '; '))" } else { Fail 'image contents' $info.Text }

    # 6. hello-world (pulls from Docker Hub: retried, anonymous pulls are rate limited)
    $hello = $null
    foreach ($try in 1..3) {
        $hello = Invoke-Guest 'docker run --rm hello-world' 180
        if ($hello.Code -eq 0 -and $hello.Text -match 'Hello from Docker!') { break }
        Start-Sleep -Seconds (5 * $try)
    }
    if ($hello.Code -eq 0 -and $hello.Text -match 'Hello from Docker!') { Pass 'docker run hello-world' } else { Fail 'hello-world' $hello.Text }

    # 7. a published port reachable from Windows
    $run = $null
    foreach ($try in 1..3) {
        $run = Invoke-Guest "docker rm -f dockering-smoke-web >/dev/null 2>&1; docker run -d --name dockering-smoke-web -p 127.0.0.1:${Port}:80 nginx:alpine" 180
        if ($run.Code -eq 0) { break }
        Start-Sleep -Seconds (5 * $try)
    }
    if ($run.Code -ne 0) { Fail 'published port' "docker run failed: $($run.Text)" }
    else {
        $status = $null
        foreach ($try in 1..30) {
            try { $status = (Invoke-WebRequest -Uri "http://127.0.0.1:$Port/" -UseBasicParsing -TimeoutSec 3).StatusCode; break } catch { Start-Sleep -Seconds 1 }
        }
        if ($status -eq 200) { Pass "published port 127.0.0.1:$Port answers 200" } else { Fail 'published port' "no HTTP 200 on 127.0.0.1:$Port" }
        [void](Invoke-Guest 'docker rm -f dockering-smoke-web' 60)
    }

    # 8. a bind mount from the Windows drive: read a Windows file, write one that Windows sees
    New-Item -ItemType Directory -Force -Path $bindDir | Out-Null
    [IO.File]::WriteAllText((Join-Path $bindDir 'in.txt'), 'from-windows', [Text.UTF8Encoding]::new($false))
    $bindUnix = (& wsl.exe -d $DistroName -u root --exec wslpath -a -u $bindDir | Out-String).Trim()
    if ($bindUnix -match "[']" -or -not $bindUnix) { Fail 'bind mount' "unusable path '$bindUnix'" }
    else {
        $bind = Invoke-Guest "docker run --rm -v '${bindUnix}:/data' alpine sh -c 'cat /data/in.txt > /data/out.txt; echo from-linux >> /data/out.txt'" 180
        $outFile = Join-Path $bindDir 'out.txt'
        if ($bind.Code -eq 0 -and (Test-Path -LiteralPath $outFile) -and ((Get-Content -Raw -LiteralPath $outFile) -match 'from-windows' ) -and ((Get-Content -Raw -LiteralPath $outFile) -match 'from-linux')) {
            Pass 'bind mount from the Windows drive'
        } else { Fail 'bind mount' "exit $($bind.Code): $($bind.Text)" }
    }
}
catch {
    Fail 'run' $_.Exception.Message
}
finally {
    Remove-Distro
}

if ($script:Failures.Count -gt 0) {
    Write-Host "smoke[$Label]: $($script:Failures.Count) check(s) failed"
    exit 1
}
Write-Host "smoke[$Label]: all checks passed"
