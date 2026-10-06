#Requires -Version 7.3
# Release verifier (REL-018, REL-032): checks a downloaded release against what release.yml promises.
# Usage: pwsh -NoProfile -File scripts/verify-release.ps1 -Dir <dir> -Version <semver> -Expect Signed|Unsigned
#          [-Changelog <CHANGELOG.md>] [-Provider signpath|azure] [-SignerSubject <subject DN>] [-Repo owner/name]
#          [-Attest -Commit <sha> [-SourceRef <ref>]] [-Published] [-LaunchSmoke]
# One line per check: [PASS] / [FAIL] / [SKIP] <id>  <detail>, then RESULT. Exit 0 = no FAIL, 1 = a FAIL, 2 = usage error.
#
# Check ids: files.set · checksums.format|coverage|match · manifest.schema|version|notes_url|platforms|platform.<key>
#   notes.changelog|footer · windows.<x64|arm64>.zip|exe.pe|setup.pe|exe.version|setup.version|exe.signature|setup.signature
#   |exe.run · windows.signer · linux.<x86_64|aarch64>.appimage|tar|deb.control|deb.elf · macos.<arch>.dmg
#   attest.<file> · published.<file> · latest.<file>|api · launch.<upgrade|demo>.<stays-up|window|exit|crash|log> · launch.restore
# -LaunchSmoke starts the downloaded app on THIS machine's real profile (Windows only; backed up and restored).
param(
    [string] $Dir,
    [string] $Version,
    [string] $Expect,
    [string] $Changelog,
    [string] $Provider = 'signpath',
    [string] $SignerSubject,
    [string] $Repo = 'pavel-purma/dockering',
    [switch] $Attest,
    [string] $Commit,
    [string] $SourceRef,
    [switch] $Published,
    [switch] $LaunchSmoke
)
$ErrorActionPreference = 'Stop'

$Distributions = @(
    'Dockering-Setup-x64.exe', 'Dockering-Setup-arm64.exe', 'Dockering-x64.zip', 'Dockering-arm64.zip',
    'Dockering-aarch64.dmg', 'Dockering-x86_64.dmg',
    'Dockering-x86_64.AppImage', 'Dockering-aarch64.AppImage', 'Dockering-x86_64.deb', 'Dockering-aarch64.deb',
    'Dockering-x86_64.tar.gz', 'Dockering-aarch64.tar.gz'
)
$Metadata = @('dockering-update.json', 'SHA256SUMS', 'RELEASE_NOTES.md')
$Platforms = [ordered]@{
    'windows-x86_64'  = @('Dockering-Setup-x64.exe', 'inno')
    'windows-aarch64' = @('Dockering-Setup-arm64.exe', 'inno')
    'macos-aarch64'   = @('Dockering-aarch64.dmg', 'dmg')
    'macos-x86_64'    = @('Dockering-x86_64.dmg', 'dmg')
    'linux-x86_64'    = @('Dockering-x86_64.AppImage', 'appimage')
    'linux-aarch64'   = @('Dockering-aarch64.AppImage', 'appimage')
}
$WindowsArchs = @(
    @{ Name = 'x64'; Triple = 'x86_64-pc-windows-msvc'; Machine = 0x8664; Native = 'X64' },
    @{ Name = 'arm64'; Triple = 'aarch64-pc-windows-msvc'; Machine = 0xAA64; Native = 'Arm64' }
)
$LinuxArchs = @(
    @{ Name = 'x86_64'; Triple = 'x86_64-unknown-linux-gnu'; Elf = 0x3E; Deb = 'amd64' },
    @{ Name = 'aarch64'; Triple = 'aarch64-unknown-linux-gnu'; Elf = 0xB7; Deb = 'arm64' }
)
$DigestSha256 = '2.16.840.1.101.3.4.2.1'
$AttributionLine = 'Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)'

$script:Results = [System.Collections.Generic.List[object]]::new()
$script:Hashes = @{}
$script:Extracted = @{}
$script:Temp = $null
$script:Ctx = $null

# ── results ─────────────────────────────────────────────────────────────────────────────────

function Add-Result([string] $Status, [string] $Id, [string] $Detail) {
    $script:Results.Add([pscustomobject]@{ Status = $Status; Id = $Id; Detail = $Detail })
    Write-Host ('[{0}] {1}  {2}' -f $Status, $Id, $Detail)
}
function Pass([string] $Id, [string] $Detail) { Add-Result 'PASS' $Id $Detail }
function Fail([string] $Id, [string] $Detail) { Add-Result 'FAIL' $Id $Detail }
function Skip([string] $Id, [string] $Reason) { Add-Result 'SKIP' $Id $Reason }
function Assert-That([string] $Id, [bool] $Ok, [string] $PassDetail, [string] $FailDetail) {
    if ($Ok) { Pass $Id $PassDetail } else { Fail $Id $FailDetail }
}

# A check that throws is a FAIL, never a silent skip.
function Invoke-Check([string] $Id, [scriptblock] $Body) {
    try { & $Body } catch { Fail $Id ('check could not run: ' + $_.Exception.Message) }
}

# ── helpers ─────────────────────────────────────────────────────────────────────────────────

function Get-Sha256([string] $Path) {
    if (-not $script:Hashes.ContainsKey($Path)) {
        $script:Hashes[$Path] = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    }
    $script:Hashes[$Path]
}

function Get-TempRoot {
    if (-not $script:Temp) {
        $script:Temp = Join-Path ([IO.Path]::GetTempPath()) "dockering-verify-$PID"
        New-Item -ItemType Directory -Path $script:Temp -Force | Out-Null
    }
    $script:Temp
}

function Get-AssetPath([string] $Name, [string] $Id) {
    $path = Join-Path $script:Ctx.Dir $Name
    if (Test-Path -LiteralPath $path -PathType Leaf) { return $path }
    Skip $Id "$Name is missing (see files.set)"
    $null
}

# Machine, subsystem and certificate-table entry of a PE file; $null if it isn't one.
function Get-PeInfo([string] $Path) {
    $fs = [IO.File]::OpenRead($Path)
    try {
        $r = [IO.BinaryReader]::new($fs)
        if ($fs.Length -lt 0x40 -or $r.ReadUInt16() -ne 0x5A4D) { return $null }
        $fs.Position = 0x3C
        $pe = $r.ReadInt32()
        $fs.Position = $pe
        if ($r.ReadUInt32() -ne 0x4550) { return $null }
        $machine = $r.ReadUInt16()
        $optional = $pe + 24
        $fs.Position = $optional
        $magic = $r.ReadUInt16()
        $fs.Position = $optional + 68
        $subsystem = $r.ReadUInt16()
        $fs.Position = $optional + $(if ($magic -eq 0x20B) { 112 } else { 96 }) + 32
        $certOffset = $r.ReadUInt32()
        $certSize = $r.ReadUInt32()
        [pscustomobject]@{ Machine = $machine; Subsystem = $subsystem; CertOffset = $certOffset; CertSize = $certSize }
    } finally { $fs.Dispose() }
}

# Digest algorithm OID of the first signer in the PE certificate table.
function Get-DigestOid([string] $Path) {
    $b = [IO.File]::ReadAllBytes($Path)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    $dir = $pe + 24 + $(if ([BitConverter]::ToUInt16($b, $pe + 24) -eq 0x20B) { 112 } else { 96 })
    $off = [BitConverter]::ToUInt32($b, $dir + 32)
    $len = [BitConverter]::ToUInt32($b, $off)
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
    $cms.Decode([byte[]]$b[($off + 8)..($off + $len - 1)])
    $cms.SignerInfos[0].DigestAlgorithm.Value
}

# e_machine of an ELF header (little endian), or $null.
function Get-ElfMachine([byte[]] $Bytes) {
    if ($Bytes.Length -lt 20 -or $Bytes[0] -ne 0x7F -or $Bytes[1] -ne 0x45 -or $Bytes[2] -ne 0x4C -or $Bytes[3] -ne 0x46) { return $null }
    [BitConverter]::ToUInt16($Bytes, 18)
}

function Read-Head([string] $Path, [int] $Count) {
    $fs = [IO.File]::OpenRead($Path)
    try {
        $buffer = New-Object byte[] $Count
        $n = $fs.Read($buffer, 0, $Count)
        [byte[]]$buffer[0..([Math]::Max($n, 1) - 1)]
    } finally { $fs.Dispose() }
}

# Members of an `ar` archive (a .deb): Name, Offset, Size.
function Read-ArMembers([string] $Path) {
    $fs = [IO.File]::OpenRead($Path)
    try {
        $magic = New-Object byte[] 8
        if ($fs.Read($magic, 0, 8) -ne 8 -or [Text.Encoding]::ASCII.GetString($magic) -ne "!<arch>`n") { throw 'not an ar archive' }
        $members = [System.Collections.Generic.List[object]]::new()
        $header = New-Object byte[] 60
        while ($fs.Read($header, 0, 60) -eq 60) {
            $text = [Text.Encoding]::ASCII.GetString($header)
            $size = [long]$text.Substring(48, 10).Trim()
            $members.Add([pscustomobject]@{ Name = $text.Substring(0, 16).Trim().TrimEnd('/'); Offset = $fs.Position; Size = $size })
            $fs.Position += $size + ($size % 2)
        }
        , $members.ToArray()
    } finally { $fs.Dispose() }
}

# Reads the wanted entries of a gzip-compressed tar stream: names of all files, and up to $Max bytes of each wanted one.
function Read-TarGz([IO.Stream] $Stream, [scriptblock] $Want, [int] $Max) {
    $gzip = [IO.Compression.GZipStream]::new($Stream, [IO.Compression.CompressionMode]::Decompress)
    $tar = [Formats.Tar.TarReader]::new($gzip)
    $names = [System.Collections.Generic.List[string]]::new()
    $data = @{}
    while ($null -ne ($entry = $tar.GetNextEntry())) {
        if ($null -eq $entry.DataStream) { continue }
        $name = $entry.Name -replace '^\./', ''
        $names.Add($name)
        if (& $Want $name) {
            $buffer = New-Object byte[] $Max
            $n = $entry.DataStream.Read($buffer, 0, $Max)
            $data[$name] = [byte[]]$buffer[0..([Math]::Max($n, 1) - 1)]
        }
    }
    [pscustomobject]@{ Names = $names.ToArray(); Data = $data }
}

# A gzip-compressed tar stored as an `ar` member of a .deb.
function Read-ArTarGz([string] $Path, $Member, [scriptblock] $Want, [int] $Max) {
    $fs = [IO.File]::OpenRead($Path)
    try {
        $fs.Position = $Member.Offset
        $window = [IO.MemoryStream]::new()
        $buffer = New-Object byte[] 81920
        $remaining = $Member.Size
        while ($remaining -gt 0) {
            $n = $fs.Read($buffer, 0, [int][Math]::Min($buffer.Length, $remaining))
            if ($n -le 0) { break }
            $window.Write($buffer, 0, $n)
            $remaining -= $n
        }
        $window.Position = 0
        Read-TarGz $window $Want $Max
    } finally { $fs.Dispose() }
}

function Get-ChangelogSection([string] $Text, [string] $Version) {
    $lines = ($Text -replace "`r`n", "`n") -split "`n"
    $heading = "## [$Version]"
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -eq $heading -or $lines[$i].StartsWith("$heading - ")) { $start = $i; break }
    }
    if ($start -lt 0) { return $null }
    $body = [System.Collections.Generic.List[string]]::new()
    for ($i = $start + 1; $i -lt $lines.Count -and -not $lines[$i].StartsWith('## ['); $i++) { $body.Add($lines[$i]) }
    ($body -join "`n").Trim()
}

function Get-ExpectedFiles {
    $names = @($Distributions) + @($Metadata)
    if ($script:Ctx.Expect -eq 'Signed') { $names += 'dockering-update.json.minisig' }
    $names
}

# ── checks: files, checksums, manifest, notes ───────────────────────────────────────────────

function Test-FileSet {
    $expected = Get-ExpectedFiles
    $actual = @(Get-ChildItem -LiteralPath $script:Ctx.Dir -Force)
    $problems = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $expected) {
        $item = $actual | Where-Object { $_.Name -ceq $name } | Select-Object -First 1
        if (-not $item) { $problems.Add("missing $name") }
        elseif ($item.PSIsContainer) { $problems.Add("$name is a directory") }
        elseif ($item.Length -eq 0) { $problems.Add("$name is empty") }
    }
    foreach ($item in $actual) {
        if ($expected -cnotcontains $item.Name) { $problems.Add("unexpected $($item.Name)") }
    }
    if ($script:Ctx.Expect -eq 'Unsigned' -and ($actual.Name -contains 'dockering-update.json.minisig')) { $problems.Add('an unsigned release must not ship a .minisig') }
    Assert-That 'files.set' ($problems.Count -eq 0) "$($expected.Count) expected files, none missing, empty or extra" ($problems -join '; ')
}

function Test-Checksums {
    $path = Get-AssetPath 'SHA256SUMS' 'checksums.format'
    if (-not $path) { return }
    $entries = [ordered]@{}
    $badLines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in (Get-Content -LiteralPath $path)) {
        if ($line -match '^([0-9a-f]{64})  (\S.*)$') { $entries[$Matches[2]] = $Matches[1] } elseif ($line.Trim()) { $badLines.Add($line) }
    }
    Assert-That 'checksums.format' ($badLines.Count -eq 0 -and $entries.Count -gt 0) "$($entries.Count) entries" "malformed line(s): $($badLines -join ' | ')"

    $covered = @($entries.Keys)
    $want = @(Get-ExpectedFiles | Where-Object { $_ -ne 'SHA256SUMS' })
    $missing = @($want | Where-Object { $covered -cnotcontains $_ })
    $extra = @($covered | Where-Object { $want -cnotcontains $_ })
    Assert-That 'checksums.coverage' ($missing.Count -eq 0 -and $extra.Count -eq 0) "covers all $($want.Count) files" ("missing entries: [$($missing -join ', ')]; unexpected entries: [$($extra -join ', ')]")

    $mismatch = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $covered) {
        $file = Join-Path $script:Ctx.Dir $name
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
        if ((Get-Sha256 $file) -ne $entries[$name]) { $mismatch.Add($name) }
    }
    Assert-That 'checksums.match' ($mismatch.Count -eq 0) 'every hash matches' ("hash mismatch: $($mismatch -join ', ')")
}

function Test-Manifest {
    $path = Get-AssetPath 'dockering-update.json' 'manifest.schema'
    if (-not $path) { return }
    $m = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -AsHashtable
    $ctx = $script:Ctx
    Assert-That 'manifest.schema' ($m['schema'] -eq 1) 'schema 1' "schema is '$($m['schema'])'"
    Assert-That 'manifest.version' ($m['version'] -ceq $ctx.Version) "version $($ctx.Version)" "version is '$($m['version'])', expected $($ctx.Version)"
    $notes = "https://github.com/$($ctx.Repo)/releases/tag/v$($ctx.Version)"
    Assert-That 'manifest.notes_url' ($m['notes_url'] -ceq $notes) $notes "notes_url is '$($m['notes_url'])'"
    $keys = @($m['platforms'].Keys | Sort-Object)
    $wanted = @($Platforms.Keys | Sort-Object)
    Assert-That 'manifest.platforms' (($keys -join ',') -ceq ($wanted -join ',')) 'six platforms' "platforms are [$($keys -join ', ')]"
    foreach ($key in $Platforms.Keys) {
        $id = "manifest.platform.$key"
        $name, $kind = $Platforms[$key]
        $entry = $m['platforms'][$key]
        if (-not $entry) { Fail $id 'missing from the manifest'; continue }
        $file = Join-Path $ctx.Dir $name
        $problems = [System.Collections.Generic.List[string]]::new()
        if ($entry['kind'] -cne $kind) { $problems.Add("kind '$($entry['kind'])' != $kind") }
        $url = "https://github.com/$($ctx.Repo)/releases/download/v$($ctx.Version)/$name"
        if ($entry['url'] -cne $url) { $problems.Add("url '$($entry['url'])'") }
        if (Test-Path -LiteralPath $file -PathType Leaf) {
            if ([long]$entry['size'] -ne (Get-Item -LiteralPath $file).Length) { $problems.Add("size $($entry['size']) != $((Get-Item -LiteralPath $file).Length)") }
            if (([string]$entry['sha256']).ToLowerInvariant() -ne (Get-Sha256 $file)) { $problems.Add('sha256 does not match the file') }
        } else { $problems.Add("$name is missing") }
        Assert-That $id ($problems.Count -eq 0) "$name ($kind)" ($problems -join '; ')
    }
}

function Test-Notes {
    $path = Get-AssetPath 'RELEASE_NOTES.md' 'notes.footer'
    if (-not $path) { return }
    $ctx = $script:Ctx
    $notes = (Get-Content -LiteralPath $path -Raw) -replace "`r`n", "`n"
    if ($ctx.Changelog) {
        $section = Get-ChangelogSection (Get-Content -LiteralPath $ctx.Changelog -Raw) $ctx.Version
        if (-not $section) { Fail 'notes.changelog' "$($ctx.Changelog) has no section for $($ctx.Version)" }
        else { Assert-That 'notes.changelog' ($notes.TrimStart().StartsWith($section)) 'starts with the changelog section verbatim' 'the notes do not start with the changelog section' }
    } else { Skip 'notes.changelog' 'no -Changelog given' }

    $problems = [System.Collections.Generic.List[string]]::new()
    if ($ctx.Expect -eq 'Unsigned') {
        if (-not $notes.Contains('**Unsigned release:**')) { $problems.Add('no **Unsigned release:** note') }
        if (-not $notes.Contains('Automatic updates are disabled')) { $problems.Add('does not say that automatic updates are disabled') }
    } else {
        if (-not $notes.Contains('**Signed release:**')) { $problems.Add('no **Signed release:** note') }
        if ($notes.Contains('**Unsigned release:**')) { $problems.Add('has the unsigned note') }
        if (-not $notes.Contains('#code-signing-policy')) { $problems.Add('no link to the code signing policy') }
        if ($ctx.Provider -eq 'signpath' -and -not $notes.Contains($AttributionLine)) { $problems.Add('no SignPath attribution') }
    }
    if (-not $notes.Contains('**Verify a download:**')) { $problems.Add('no **Verify a download:** note') }
    Assert-That 'notes.footer' ($problems.Count -eq 0) "$($ctx.Expect) footer" ($problems -join '; ')
}

# ── checks: Windows ─────────────────────────────────────────────────────────────────────────

function Get-PaddedVersionInfo([string] $Path) {
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    [pscustomobject]@{ ProductName = ([string]$info.ProductName).Trim(); ProductVersion = ([string]$info.ProductVersion).Trim() }
}

function Test-Authenticode([string] $Id, [string] $Path, [string] $Label) {
    $expect = $script:Ctx.Expect
    $info = Get-PeInfo $Path
    $hasCert = $info.CertSize -gt 0
    if (-not $IsWindows) {
        if ($expect -eq 'Signed') { Fail $Id 'Authenticode can only be verified on Windows' } else { Skip $Id 'Authenticode is checked on Windows only' }
        return $null
    }
    $sig = Get-AuthenticodeSignature -LiteralPath $Path
    if ($expect -eq 'Unsigned') {
        Assert-That $Id ($sig.Status -eq 'NotSigned' -and -not $hasCert) "$Label is NotSigned" "$Label status is $($sig.Status), certificate table $($info.CertSize) bytes"
        return $null
    }
    $problems = [System.Collections.Generic.List[string]]::new()
    if ($sig.Status -ne 'Valid') { $problems.Add("status $($sig.Status)") }
    if (-not $sig.TimeStamperCertificate) { $problems.Add('no RFC 3161 timestamp') }
    if ($hasCert) {
        $oid = Get-DigestOid $Path
        if ($oid -ne $DigestSha256) { $problems.Add("digest $oid is not SHA-256") }
    } else { $problems.Add('no embedded certificate table') }
    Assert-That $Id ($problems.Count -eq 0) "$Label is Valid, timestamped, SHA-256" ($problems -join '; ')
    if ($sig.SignerCertificate) { [pscustomobject]@{ Subject = $sig.SignerCertificate.Subject; Issuer = $sig.SignerCertificate.Issuer; File = $Label } } else { $null }
}

function Test-Windows {
    $ctx = $script:Ctx
    $signers = [System.Collections.Generic.List[object]]::new()
    foreach ($arch in $WindowsArchs) {
        $p = "windows.$($arch.Name)"
        $zip = Get-AssetPath "Dockering-$($arch.Name).zip" "$p.zip"
        $setup = Get-AssetPath "Dockering-Setup-$($arch.Name).exe" "$p.setup.pe"
        $exe = $null
        if ($zip) {
            Invoke-Check "$p.zip" {
                $dest = Join-Path (Get-TempRoot) $arch.Name
                [IO.Compression.ZipFile]::ExtractToDirectory($zip, $dest)
                $files = @(Get-ChildItem -LiteralPath $dest -Recurse -File | ForEach-Object { $_.FullName.Substring($dest.Length + 1).Replace('\', '/') } | Sort-Object)
                $want = @('LICENSE', 'THIRD_PARTY_LICENSES.html', 'dockering.exe') | ForEach-Object { "dockering-$($arch.Triple)/$_" } | Sort-Object
                Assert-That "$p.zip" (($files -join '|') -ceq ($want -join '|')) "$($want.Count) files under dockering-$($arch.Triple)/" "layout is [$($files -join ', ')]"
                $script:Extracted[$arch.Name] = Join-Path $dest "dockering-$($arch.Triple)"
            }
            $candidate = Join-Path (Join-Path (Join-Path (Get-TempRoot) $arch.Name) "dockering-$($arch.Triple)") 'dockering.exe'
            if (Test-Path -LiteralPath $candidate) { $exe = $candidate }
        }
        if ($exe) {
            Invoke-Check "$p.exe.pe" {
                $pe = Get-PeInfo $exe
                $problems = [System.Collections.Generic.List[string]]::new()
                if (-not $pe) { $problems.Add('not a PE file') } else {
                    if ($pe.Machine -ne $arch.Machine) { $problems.Add(('machine 0x{0:X}, expected 0x{1:X}' -f $pe.Machine, $arch.Machine)) }
                    if ($pe.Subsystem -ne 2) { $problems.Add("subsystem $($pe.Subsystem), expected 2 (GUI)") }
                    if (($pe.CertSize -gt 0) -ne ($ctx.Expect -eq 'Signed')) { $problems.Add("certificate table is $($pe.CertSize) bytes") }
                }
                Assert-That "$p.exe.pe" ($problems.Count -eq 0) ('0x{0:X}, GUI subsystem, certificate table matches {1}' -f $arch.Machine, $ctx.Expect) ($problems -join '; ')
            }
            Invoke-Check "$p.exe.version" {
                if (-not $IsWindows) { Skip "$p.exe.version" 'version resources are read on Windows only'; return }
                $v = Get-PaddedVersionInfo $exe
                Assert-That "$p.exe.version" ($v.ProductName -ceq 'Dockering' -and $v.ProductVersion -ceq $ctx.Version) "Dockering $($v.ProductVersion)" "'$($v.ProductName)' '$($v.ProductVersion)', expected 'Dockering' '$($ctx.Version)'"
            }
            Invoke-Check "$p.exe.signature" {
                $signer = Test-Authenticode "$p.exe.signature" $exe "$p dockering.exe"
                if ($signer) { $signers.Add($signer) }
            }
            Invoke-Check "$p.exe.run" {
                if (-not $IsWindows) { Skip "$p.exe.run" 'Windows only'; return }
                $native = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
                if ($native -ne $arch.Native) { Skip "$p.exe.run" "this host is $native"; return }
                $out = Join-Path (Get-TempRoot) "version-$($arch.Name).txt"
                $run = Start-Process -FilePath $exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $out
                $text = if (Test-Path -LiteralPath $out) { (Get-Content -LiteralPath $out -Raw).Trim() } else { '' }
                Assert-That "$p.exe.run" ($run.ExitCode -eq 0 -and $text -ceq "dockering $($ctx.Version)") "prints '$text'" "exit $($run.ExitCode), printed '$text', expected 'dockering $($ctx.Version)'"
            }
        } else { foreach ($suffix in 'exe.pe', 'exe.version', 'exe.signature', 'exe.run') { Skip "$p.$suffix" 'dockering.exe could not be extracted (see the zip check)' } }
        if ($setup) {
            Invoke-Check "$p.setup.pe" {
                $pe = Get-PeInfo $setup
                $signed = $ctx.Expect -eq 'Signed'
                Assert-That "$p.setup.pe" ($pe -and $pe.Subsystem -eq 2 -and (($pe.CertSize -gt 0) -eq $signed)) "GUI installer, certificate table matches $($ctx.Expect)" "not a GUI PE, or certificate table is $($pe.CertSize) bytes"
            }
            Invoke-Check "$p.setup.version" {
                if (-not $IsWindows) { Skip "$p.setup.version" 'version resources are read on Windows only'; return }
                $v = Get-PaddedVersionInfo $setup
                Assert-That "$p.setup.version" ($v.ProductName -ceq 'Dockering' -and $v.ProductVersion -ceq $ctx.Version) "Dockering $($v.ProductVersion)" "'$($v.ProductName)' '$($v.ProductVersion)', expected 'Dockering' '$($ctx.Version)'"
            }
            Invoke-Check "$p.setup.signature" {
                $signer = Test-Authenticode "$p.setup.signature" $setup "$p Dockering-Setup-$($arch.Name).exe"
                if ($signer) { $signers.Add($signer) }
            }
        } else { foreach ($suffix in 'setup.version', 'setup.signature') { Skip "$p.$suffix" "Dockering-Setup-$($arch.Name).exe is missing" } }
    }
    if ($ctx.Expect -eq 'Signed') {
        Invoke-Check 'windows.signer' {
            if ($signers.Count -ne 4) { Fail 'windows.signer' "found $($signers.Count) signed files, expected 4"; return }
            $subjects = @($signers | ForEach-Object Subject | Sort-Object -Unique)
            $issuers = @($signers | ForEach-Object Issuer | Sort-Object -Unique)
            $detail = "signer: $($subjects[0]) | issuer: $($issuers[0])"
            $problems = [System.Collections.Generic.List[string]]::new()
            if ($subjects.Count -ne 1) { $problems.Add("signers differ: $($subjects -join ' || ')") }
            if ($issuers.Count -ne 1) { $problems.Add("issuers differ: $($issuers -join ' || ')") }
            if ($ctx.SignerSubject -and $subjects[0] -cne $ctx.SignerSubject) { $problems.Add("signer is not the pinned '$($ctx.SignerSubject)'") }
            Assert-That 'windows.signer' ($problems.Count -eq 0) $detail "$($problems -join '; '); $detail"
        }
    }
}

# ── checks: Linux and macOS ─────────────────────────────────────────────────────────────────

function Test-Linux {
    $ctx = $script:Ctx
    foreach ($arch in $LinuxArchs) {
        $p = "linux.$($arch.Name)"
        $appImage = Get-AssetPath "Dockering-$($arch.Name).AppImage" "$p.appimage"
        if ($appImage) {
            Invoke-Check "$p.appimage" {
                $machine = Get-ElfMachine (Read-Head $appImage 64)
                Assert-That "$p.appimage" ($machine -eq $arch.Elf) ('ELF machine 0x{0:X}' -f $arch.Elf) ("ELF machine is $(if ($null -eq $machine) { 'not ELF' } else { '0x{0:X}' -f $machine }), expected 0x$('{0:X}' -f $arch.Elf)")
            }
        }
        $tar = Get-AssetPath "Dockering-$($arch.Name).tar.gz" "$p.tar"
        if ($tar) {
            Invoke-Check "$p.tar" {
                $fs = [IO.File]::OpenRead($tar)
                try { $result = Read-TarGz $fs { param($n) $n -ceq "dockering-$($arch.Triple)/dockering" } 64 } finally { $fs.Dispose() }
                $root = "dockering-$($arch.Triple)"
                $problems = [System.Collections.Generic.List[string]]::new()
                foreach ($f in "$root/dockering", "$root/LICENSE", "$root/THIRD_PARTY_LICENSES.html") { if ($result.Names -cnotcontains $f) { $problems.Add("missing $f") } }
                $machine = if ($result.Data.ContainsKey("$root/dockering")) { Get-ElfMachine $result.Data["$root/dockering"] } else { $null }
                if ($machine -ne $arch.Elf) { $problems.Add("binary ELF machine is $(if ($null -eq $machine) { 'not ELF' } else { '0x{0:X}' -f $machine })") }
                Assert-That "$p.tar" ($problems.Count -eq 0) ('binary is ELF 0x{0:X}, licence files present' -f $arch.Elf) ($problems -join '; ')
            }
        }
        $deb = Get-AssetPath "Dockering-$($arch.Name).deb" "$p.deb.control"
        if ($deb) {
            Invoke-Check "$p.deb.control" {
                $members = Read-ArMembers $deb
                $control = $members | Where-Object { $_.Name -like 'control.tar*' } | Select-Object -First 1
                if (-not $control) { Fail "$p.deb.control" 'no control.tar member'; return }
                if ($control.Name -notlike '*.gz') { Skip "$p.deb.control" "$($control.Name) is not gzip"; return }
                $c = Read-ArTarGz $deb $control { param($n) $n -ceq 'control' } 65536
                $text = if ($c.Data.ContainsKey('control')) { [Text.Encoding]::UTF8.GetString($c.Data['control']) } else { '' }
                $version = if ($text -match '(?m)^Version:\s*(\S+)') { $Matches[1] } else { '' }
                $architecture = if ($text -match '(?m)^Architecture:\s*(\S+)') { $Matches[1] } else { '' }
                Assert-That "$p.deb.control" ($version -ceq $ctx.Version -and $architecture -ceq $arch.Deb) "Version $version, Architecture $architecture" "Version '$version', Architecture '$architecture'; expected $($ctx.Version), $($arch.Deb)"
            }
            Invoke-Check "$p.deb.elf" {
                $members = Read-ArMembers $deb
                $dataMember = $members | Where-Object { $_.Name -like 'data.tar*' } | Select-Object -First 1
                if (-not $dataMember) { Fail "$p.deb.elf" 'no data.tar member'; return }
                if ($dataMember.Name -notlike '*.gz') { Skip "$p.deb.elf" "$($dataMember.Name) is not gzip"; return }
                $d = Read-ArTarGz $deb $dataMember { param($n) $n -ceq 'usr/bin/dockering' } 64
                $machine = if ($d.Data.ContainsKey('usr/bin/dockering')) { Get-ElfMachine $d.Data['usr/bin/dockering'] } else { $null }
                Assert-That "$p.deb.elf" ($machine -eq $arch.Elf) ('usr/bin/dockering is ELF 0x{0:X}' -f $arch.Elf) ("usr/bin/dockering is $(if ($null -eq $machine) { 'missing or not ELF' } else { '0x{0:X}' -f $machine }), expected 0x$('{0:X}' -f $arch.Elf)")
            }
        }
    }
}

function Test-MacOS {
    foreach ($arch in 'aarch64', 'x86_64') {
        $id = "macos.$arch.dmg"
        $dmg = Get-AssetPath "Dockering-$arch.dmg" $id
        if (-not $dmg) { continue }
        Invoke-Check $id {
            $length = (Get-Item -LiteralPath $dmg).Length
            $fs = [IO.File]::OpenRead($dmg)
            try {
                $fs.Position = [Math]::Max($length - 512, 0)
                $tail = New-Object byte[] 4
                [void]$fs.Read($tail, 0, 4)
            } finally { $fs.Dispose() }
            Assert-That $id ($length -ge 512 -and [Text.Encoding]::ASCII.GetString($tail) -ceq 'koly') "$length bytes with the UDIF trailer" "no UDIF 'koly' trailer in the last 512 bytes"
        }
    }
}

# ── checks: attestations, public URLs ───────────────────────────────────────────────────────

function Test-Attestations {
    $ctx = $script:Ctx
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Fail 'attest' 'the gh CLI is not installed'; return }
    $ref = if ($ctx.SourceRef) { $ctx.SourceRef } else { "refs/tags/v$($ctx.Version)" }
    foreach ($name in Get-ExpectedFiles) {
        $file = Join-Path $ctx.Dir $name
        $id = "attest.$name"
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { Skip $id 'file is missing'; continue }
        $out = & gh attestation verify $file -R $ctx.Repo --source-digest $ctx.Commit --source-ref $ref --signer-workflow "$($ctx.Repo)/.github/workflows/release.yml" --deny-self-hosted-runners 2>&1
        $code = $LASTEXITCODE
        Assert-That $id ($code -eq 0) "verified against $($ctx.Commit.Substring(0, 7)) at $ref" ("gh exited $code`: $(($out | Select-Object -Last 1))")
    }
}

function Get-HttpStatus([Net.Http.HttpClient] $Client, [string] $Url) {
    $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Get, $Url)
    $request.Headers.Range = [Net.Http.Headers.RangeHeaderValue]::new(0, 0)
    $response = $Client.SendAsync($request, [Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
    try { [int]$response.StatusCode } finally { $response.Dispose() }
}

function Test-Published {
    $ctx = $script:Ctx
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $true
    $handler.MaxAutomaticRedirections = 5
    $client = [Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(60)
    $client.DefaultRequestHeaders.UserAgent.ParseAdd('dockering-verify-release')
    try {
        $base = "https://github.com/$($ctx.Repo)/releases"
        foreach ($name in Get-ExpectedFiles) {
            Invoke-Check "published.$name" {
                $code = Get-HttpStatus $client "$base/download/v$($ctx.Version)/$name"
                Assert-That "published.$name" ($code -in 200, 206) "HTTP $code" "HTTP $code at /download/v$($ctx.Version)/$name"
            }
        }
        if ($ctx.Version.Contains('-')) { Skip 'latest' 'a pre-release is never "latest"'; return }
        foreach ($name in $Distributions) {
            Invoke-Check "latest.$name" {
                $code = Get-HttpStatus $client "$base/latest/download/$name"
                Assert-That "latest.$name" ($code -in 200, 206) "HTTP $code" "HTTP $code at /latest/download/$name"
            }
        }
        Invoke-Check 'latest.api' {
            $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Get, "https://api.github.com/repos/$($ctx.Repo)/releases/latest")
            $request.Headers.Accept.ParseAdd('application/vnd.github+json')
            $json = $client.SendAsync($request).GetAwaiter().GetResult().Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json
            $names = @($json.assets | ForEach-Object name | Sort-Object)
            $want = @(Get-ExpectedFiles | Sort-Object)
            $problems = [System.Collections.Generic.List[string]]::new()
            if ($json.tag_name -cne "v$($ctx.Version)") { $problems.Add("latest is $($json.tag_name)") }
            if ($json.draft) { $problems.Add('draft') }
            if ($json.prerelease) { $problems.Add('pre-release') }
            if (($names -join '|') -cne ($want -join '|')) { $problems.Add("assets are [$($names -join ', ')]") }
            Assert-That 'latest.api' ($problems.Count -eq 0) "v$($ctx.Version) is latest with all $($want.Count) assets" ($problems -join '; ')
        }
    } finally { $client.Dispose() }
}

# ── launch smoke (Windows, real profile) ────────────────────────────────────────────────────

function Get-ProfilePaths {
    [pscustomobject]@{
        Data   = Join-Path $env:APPDATA 'dockering\Dockering\data'
        Config = Join-Path $env:APPDATA 'dockering\Dockering\config'
        Logs   = Join-Path $env:LOCALAPPDATA 'dockering\Dockering\data\logs'
    }
}

# Byte-exact copies of the profile files that the app rewrites, in memory and in a temp directory.
function Backup-Profile([string[]] $Files, [string] $BackupDir) {
    New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null
    $entries = foreach ($file in $Files) {
        $exists = Test-Path -LiteralPath $file -PathType Leaf
        $bytes = if ($exists) { [IO.File]::ReadAllBytes($file) } else { $null }
        if ($exists) { [IO.File]::WriteAllBytes((Join-Path $BackupDir ([IO.Path]::GetFileName($file))), $bytes) }
        [pscustomobject]@{ Path = $file; Existed = $exists; Bytes = $bytes; Hash = $(if ($exists) { (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash } else { $null }) }
    }
    , @($entries)
}

# Puts every file back and says whether it is byte-identical (or absent) afterwards.
function Restore-Profile($Backup) {
    $ok = $true
    foreach ($entry in $Backup) {
        try {
            if ($entry.Existed) {
                [IO.File]::WriteAllBytes($entry.Path, $entry.Bytes)
                if ((Get-FileHash -Algorithm SHA256 -LiteralPath $entry.Path).Hash -ne $entry.Hash) { $ok = $false }
            } elseif (Test-Path -LiteralPath $entry.Path) {
                Remove-Item -LiteralPath $entry.Path -Force
            }
        } catch { $ok = $false }
    }
    $ok
}

# state.json as an older release would have left it: updates.last_run_version = $Older.
function Set-OlderRunVersion([string] $StatePath, [string] $Older = '0.0.1') {
    $dir = Split-Path -Parent $StatePath
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $state = if (Test-Path -LiteralPath $StatePath) { Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json -AsHashtable } else { @{} }
    if (-not $state.ContainsKey('updates') -or $null -eq $state['updates']) { $state['updates'] = @{} }
    $state['updates']['last_run_version'] = $Older
    [IO.File]::WriteAllText($StatePath, ($state | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($false))
}

function Get-LogSnapshot([string] $LogDir) {
    $snapshot = @{}
    if (Test-Path -LiteralPath $LogDir) { foreach ($f in Get-ChildItem -LiteralPath $LogDir -File) { $snapshot[$f.FullName] = $f.Length } }
    $snapshot
}

function Read-NewLogText([string] $LogDir, $Before) {
    $text = [System.Collections.Generic.List[string]]::new()
    if (-not (Test-Path -LiteralPath $LogDir)) { return '' }
    foreach ($f in Get-ChildItem -LiteralPath $LogDir -File) {
        $from = if ($Before.ContainsKey($f.FullName)) { [long]$Before[$f.FullName] } else { 0 }
        if ($f.Length -le $from) { continue }
        $fs = [IO.File]::Open($f.FullName, 'Open', 'Read', 'ReadWrite')
        try {
            $fs.Position = $from
            $text.Add([IO.StreamReader]::new($fs).ReadToEnd())
        } finally { $fs.Dispose() }
    }
    $text -join "`n"
}

# One run of the app: stays up, shows a window, closes with 0, writes no crash report or panic.
function Invoke-AppRun([string] $Id, [string] $Exe, [switch] $Demo) {
    $profilePaths = Get-ProfilePaths
    $dataDir = $profilePaths.Data
    $logDir = $profilePaths.Logs
    $crashesBefore = if (Test-Path -LiteralPath $dataDir) { @(Get-ChildItem -LiteralPath $dataDir -Filter 'crash-*.txt' | ForEach-Object Name) } else { @() }
    $logsBefore = Get-LogSnapshot $logDir
    $arguments = if ($Demo) { @{ ArgumentList = '--demo' } } else { @{} }
    $started = [Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $Exe @arguments -PassThru
    $null = $proc.Handle
    $demoRoot = $null
    try {
        if ($Demo) {
            $demoRoot = Join-Path ([IO.Path]::GetTempPath()) "dockering-demo-$($proc.Id)"
            $dataDir = Join-Path $demoRoot 'data'
            $logDir = Join-Path $demoRoot 'logs'
            $crashesBefore = @()
            $logsBefore = @{}
        }
        while ($started.ElapsedMilliseconds -lt 10000 -and -not $proc.HasExited) { Start-Sleep -Milliseconds 250 }
        if ($proc.HasExited) {
            Fail "$Id.stays-up" "exited with code $($proc.ExitCode) after $($started.ElapsedMilliseconds) ms"
        } else {
            Pass "$Id.stays-up" 'still running after 10 s'
            $title = ''
            $until = $started.ElapsedMilliseconds + 15000
            while ($started.ElapsedMilliseconds -lt $until -and -not $proc.HasExited) {
                $proc.Refresh()
                $title = $proc.MainWindowTitle
                if ($title -like '*Dockering*') { break }
                Start-Sleep -Milliseconds 250
            }
            Assert-That "$Id.window" ($title -like '*Dockering*') "window titled '$title'" "no main window titled Dockering (title '$title'; is this an interactive session?)"
            [void]$proc.CloseMainWindow()
            if ($proc.WaitForExit(15000)) { Assert-That "$Id.exit" ($proc.ExitCode -eq 0) 'closed with exit code 0' "exit code $($proc.ExitCode) after the close request" }
            else { Fail "$Id.exit" 'still running 15 s after the close request' }
        }
    } finally {
        if (-not $proc.HasExited) { try { $proc.Kill($true) } catch { } }
    }
    $crashes = if (Test-Path -LiteralPath $dataDir) { @(Get-ChildItem -LiteralPath $dataDir -Filter 'crash-*.txt' | ForEach-Object Name | Where-Object { $crashesBefore -notcontains $_ }) } else { @() }
    Assert-That "$Id.crash" ($crashes.Count -eq 0) 'no crash report written' "new crash report(s) in ${dataDir}: $($crashes -join ', ')"
    $log = Read-NewLogText $logDir $logsBefore
    if (-not $log.Trim()) {
        Skip "$Id.log" "the run wrote no log text under $logDir (is the log level off?)"
    } else {
        $panics = @($log -split "`n" | Where-Object { $_ -match 'panick' })
        $noisy = @($log -split "`n" | Where-Object { $_ -match '\b(ERROR|WARN)\b' })
        $detail = "$($log.Length) chars of log, $($noisy.Count) ERROR/WARN line(s)" + $(if ($noisy.Count) { ", first: $((($noisy | Select-Object -First 3) -join ' / ').Trim())" } else { '' })
        Assert-That "$Id.log" ($panics.Count -eq 0) "no panic; $detail" "$($panics.Count) panic line(s): $(($panics | Select-Object -First 2) -join ' / ')"
    }
    if ($demoRoot -and (Test-Path -LiteralPath $demoRoot)) { Remove-Item -LiteralPath $demoRoot -Recurse -Force -ErrorAction SilentlyContinue }
}

function Test-Launch {
    if (-not $IsWindows) { Skip 'launch' 'the launch smoke runs on Windows only'; return }
    $arch = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
    $dir = $script:Extracted[$arch]
    $exe = if ($dir) { Join-Path $dir 'dockering.exe' } else { $null }
    if (-not $exe -or -not (Test-Path -LiteralPath $exe)) { Fail 'launch' "no extracted $arch dockering.exe to start"; return }
    if (Get-Process -Name dockering -ErrorAction SilentlyContinue) { Fail 'launch' 'Dockering is already running: close it first (a second instance would hide a crash)'; return }
    $paths = Get-ProfilePaths
    $statePath = Join-Path $paths.Data 'state.json'
    $backupDir = Join-Path ([IO.Path]::GetTempPath()) "dockering-verify-backup-$PID"
    $backup = Backup-Profile @($statePath, (Join-Path $paths.Config 'config.toml')) $backupDir
    Write-Host "backup of the profile files: $backupDir (restored automatically)"
    try {
        Set-OlderRunVersion $statePath
        Invoke-AppRun 'launch.upgrade' $exe
        Invoke-AppRun 'launch.demo' $exe -Demo
    } finally {
        $restored = Restore-Profile $backup
        Assert-That 'launch.restore' $restored 'state.json and config.toml are restored byte for byte' "could not restore the profile files; copies are in $backupDir"
        if ($restored) { Remove-Item -LiteralPath $backupDir -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

# ── main ────────────────────────────────────────────────────────────────────────────────────

function Test-Usage {
    $problems = [System.Collections.Generic.List[string]]::new()
    if (-not $Dir -or -not (Test-Path -LiteralPath $Dir -PathType Container)) { $problems.Add('-Dir must be an existing directory') }
    if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?$') { $problems.Add('-Version must be a SemVer version without build metadata') }
    if ($Expect -cnotin 'Signed', 'Unsigned') { $problems.Add('-Expect must be Signed or Unsigned') }
    if ($Provider -cnotin 'signpath', 'azure') { $problems.Add('-Provider must be signpath or azure') }
    if ($Repo -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') { $problems.Add('-Repo must be owner/name') }
    if ($Changelog -and -not (Test-Path -LiteralPath $Changelog -PathType Leaf)) { $problems.Add('-Changelog must be an existing file') }
    if ($SignerSubject -and $Expect -ne 'Signed') { $problems.Add('-SignerSubject needs -Expect Signed') }
    if ($Attest -and $Commit -notmatch '^[0-9a-f]{40}$') { $problems.Add('-Attest needs -Commit <40-hex release commit>') }
    if ($SourceRef -and -not $Attest) { $problems.Add('-SourceRef needs -Attest') }
    $problems
}

function Invoke-Main {
    $usage = @(Test-Usage)
    if ($usage.Count) {
        $usage | ForEach-Object { [Console]::Error.WriteLine("usage: $_") }
        $script:ExitCode = 2
        return
    }
    $script:Ctx = [pscustomobject]@{
        Dir = (Resolve-Path -LiteralPath $Dir).Path; Version = $Version; Expect = $Expect; Changelog = $Changelog
        Provider = $Provider; SignerSubject = $SignerSubject; Repo = $Repo; Commit = $Commit; SourceRef = $SourceRef
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    try {
        Invoke-Check 'files.set' { Test-FileSet }
        Invoke-Check 'checksums' { Test-Checksums }
        Invoke-Check 'manifest' { Test-Manifest }
        Invoke-Check 'notes' { Test-Notes }
        Invoke-Check 'windows' { Test-Windows }
        Invoke-Check 'linux' { Test-Linux }
        Invoke-Check 'macos' { Test-MacOS }
        if ($Attest) { Invoke-Check 'attest' { Test-Attestations } }
        if ($Published) { Invoke-Check 'published' { Test-Published } }
        if ($LaunchSmoke) { Invoke-Check 'launch' { Test-Launch } }
    } finally {
        if ($script:Temp -and (Test-Path -LiteralPath $script:Temp)) { Remove-Item -LiteralPath $script:Temp -Recurse -Force -ErrorAction SilentlyContinue }
    }
    $failed = @($script:Results | Where-Object Status -eq 'FAIL').Count
    $passed = @($script:Results | Where-Object Status -eq 'PASS').Count
    $skipped = @($script:Results | Where-Object Status -eq 'SKIP').Count
    Write-Host ('RESULT {0} checks: {1} passed, {2} failed, {3} skipped' -f $script:Results.Count, $passed, $failed, $skipped)
    $script:ExitCode = if ($failed) { 1 } else { 0 }
}

if ($MyInvocation.InvocationName -eq '.') { return }
$script:ExitCode = 1
Invoke-Main | Out-Null
exit $script:ExitCode
