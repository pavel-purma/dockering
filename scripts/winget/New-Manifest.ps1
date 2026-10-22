#Requires -Version 7.3
# Renders the winget manifests of a published Dockering release (REL-040, REL-041) from the templates in packaging/winget/manifests/.
# Usage: pwsh -NoProfile -File scripts/winget/New-Manifest.ps1 [-Tag vX.Y.Z] [-OutDir <dir>] [-Repo owner/name]
# -Tag defaults to the latest published release. Nothing is pushed or submitted. The checks, in order: the release is published and
#   stable, both installers and SHA256SUMS are assets, SHA256SUMS agrees with GitHub's own digest of each installer, the rendered files
#   read back correctly, the installers download from the public URL with exactly that SHA-256 (they are hashed, never run), and
#   `winget validate` passes. The manifests go to <OutDir>/manifests/<d>/<Publisher>/<Name>/<version>/ (default target/winget/<version>).
# One line per check: [PASS] / [WARN] / [SKIP] / [FAIL] <id>  <detail>. Last lines on success: VERSION, CHANNEL (signed|unsigned|unknown)
#   and MANIFEST_DIR. Exit 0 = rendered, 1 = a check failed.
param(
    [string] $Tag,
    [string] $OutDir,
    [string] $Repo = 'pavel-purma/dockering'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Lib.ps1')

function Invoke-Main {
    $root = Get-RepoRoot
    $release = Get-ReleaseInfo $Repo $Tag
    $version = ConvertTo-WingetVersion $release.Tag
    Pass 'release.tag' "$($release.Tag) -> version $version ($($release.Url))"

    if ($release.IsDraft -or $release.IsPrerelease) {
        Fail 'release.published' "$($release.Tag) is a $(if ($release.IsDraft) { 'draft' } else { 'pre-release' }); winget gets published stable releases only"
    }
    Pass 'release.published' "published $($release.PublishedAt)"

    $missing = @(@($WingetInstallers.Keys) + 'SHA256SUMS' | Where-Object { $release.Assets -notcontains $_ })
    if ($missing) { Fail 'release.assets' "missing: $($missing -join ', ')" }
    Pass 'release.assets' "$($WingetInstallers.Keys -join ', '), SHA256SUMS"

    $values = @{ VERSION = $version }
    foreach ($asset in $WingetInstallers.Keys) {
        if (-not $release.Sums.ContainsKey($asset)) { Fail "checksums.$asset" 'not listed in SHA256SUMS' }
        $sum = $release.Sums[$asset]
        if (-not $release.Digests.ContainsKey($asset)) { Fail "checksums.$asset" 'GitHub has no digest for the asset' }
        if ($release.Digests[$asset] -cne $sum) { Fail "checksums.$asset" "SHA256SUMS says $sum, GitHub's digest says $($release.Digests[$asset])" }
        $values[$WingetInstallers[$asset]] = $sum.ToUpperInvariant()
        Pass "checksums.$asset" "$sum (SHA256SUMS = GitHub digest)"
    }

    switch ($release.Channel) {
        'signed' { Pass 'release.channel' 'signed release (the notes say the Windows files are Authenticode-signed)' }
        'unsigned' { Warn 'release.channel' 'unsigned release: the installers are not Authenticode-signed (REL-041 asks for a signed release; the maintainer must accept this)' }
        default { Warn 'release.channel' 'the release notes name neither a signed nor an unsigned release' }
    }

    if (-not $OutDir) { $OutDir = Join-Path $root "target/winget/$version" }
    $dir = New-ManifestSet (Join-Path $root 'packaging/winget/manifests') $OutDir $WingetPackageId $version $values
    Pass 'manifest.render' $dir

    $problems = Test-RenderedManifests $dir $WingetPackageId $version $Repo $release.Sums
    if ($problems.Count) {
        foreach ($p in $problems) { Write-Check 'FAIL' 'manifest.check' $p }
        throw 'check failed'
    }
    Pass 'manifest.check' 'identifier, version, URLs and SHA-256 values agree with the release'

    $download = Join-Path ([IO.Path]::GetTempPath()) ('dockering-winget-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $download | Out-Null
    try {
        foreach ($asset in $WingetInstallers.Keys) {
            $url = "https://github.com/$Repo/releases/download/$($release.Tag)/$asset"
            $path = Join-Path $download $asset
            Invoke-WebRequest -Uri $url -OutFile $path -UserAgent 'dockering-winget-scripts'
            $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
            $expected = $values[$WingetInstallers[$asset]]
            if ($actual -cne $expected) { Fail "download.$asset" "the public URL serves SHA-256 $actual, the manifest says $expected" }
            Pass "download.$asset" "public URL serves SHA-256 $actual"
            if ($IsWindows) {
                $status = [string](Get-AuthenticodeSignature -LiteralPath $path).Status
                $implied = if ($release.Channel -eq 'signed') { 'Valid' } else { 'NotSigned' }
                if ($status -ne $implied) { Warn "signature.$asset" "Authenticode status is $status, the release notes imply $implied" }
                else { Pass "signature.$asset" "Authenticode status $status, as the release notes say" }
            }
        }
    } finally {
        Remove-Item -LiteralPath $download -Recurse -Force -ErrorAction SilentlyContinue
    }

    if (Get-Command winget -ErrorAction SilentlyContinue) {
        $v = Invoke-Native 'winget' @('validate', '--manifest', $dir)
        if ($v.Code -ne 0) { Fail 'winget.validate' "winget validate failed ($($v.Code)): $($v.Output)" }
        Pass 'winget.validate' ($v.Output -split "\r?\n" | Select-Object -Last 1)
    } else {
        Skip 'winget.validate' 'winget is not installed'
    }
    Write-Host "VERSION $version"
    Write-Host "CHANNEL $($release.Channel)"
    Write-Host "MANIFEST_DIR $dir"
}

$script:ExitCode = 1
try {
    Invoke-Main
    $script:ExitCode = 0
} catch {
    if ($_.Exception.Message -ne 'check failed') { Write-Check 'FAIL' 'error' $_.Exception.Message }
}
exit $script:ExitCode
