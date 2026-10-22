#Requires -Version 7.3
# Shared helpers of the winget scripts (REL-040, REL-041). New-Manifest.ps1 and Submit-Manifest.ps1 dot-source this file, and so
# do the tests (scripts/tests/test_winget.py). It defines constants and functions only: no top-level side effects.

$WingetPackageId = 'Dockering.Dockering'
$WingetUpstream = 'microsoft/winget-pkgs'
$WingetUpstreamBranch = 'master'
# Release asset -> placeholder of its SHA-256 in packaging/winget/manifests/*.installer.yaml.
$WingetInstallers = [ordered]@{
    'Dockering-Setup-x64.exe'   = 'X64_SHA256'
    'Dockering-Setup-arm64.exe' = 'ARM64_SHA256'
}

# Fallback for the pull request body when the winget-pkgs template can't be read.
$FallbackPullRequestTemplate = @'
## Description

## Checklist

- [ ] Signed the [Contributor License Agreement](https://cla.opensource.microsoft.com)
- [ ] Linked to an issue (if applicable)

## Manifest Checklist

- [ ] Checked that there aren't other open [pull requests](https://github.com/microsoft/winget-pkgs/pulls) for the same manifest update/change
- [ ] This PR only modifies one (1) manifest
- [ ] Validated manifest locally with `winget validate --manifest <path>`
- [ ] Tested manifest locally with `winget install --manifest <path>`
- [ ] Manifest conforms to the latest schema
'@

# One line per check, like scripts/verify-release.ps1. Fail also stops the script: every step depends on the one before it.
function Write-Check([string] $Status, [string] $Id, [string] $Detail) {
    Write-Host ('[{0}] {1}  {2}' -f $Status, $Id, $Detail)
}
function Pass([string] $Id, [string] $Detail) { Write-Check 'PASS' $Id $Detail }
function Info([string] $Id, [string] $Detail) { Write-Check 'INFO' $Id $Detail }
function Warn([string] $Id, [string] $Detail) { Write-Check 'WARN' $Id $Detail }
function Skip([string] $Id, [string] $Reason) { Write-Check 'SKIP' $Id $Reason }
function Fail([string] $Id, [string] $Detail) { Write-Check 'FAIL' $Id $Detail; throw 'check failed' }

function Get-RepoRoot { (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path }

# The winget-pkgs clone lives in <root of the drive that holds this worktree>/repos/winget-pkgs, so a worktree on D: uses D:\repos.
# Other systems have no drive roots: ~/repos/winget-pkgs.
function Get-DefaultCloneDir([string] $RepoRoot) {
    $base = if ($IsWindows) { [IO.Path]::GetPathRoot($RepoRoot) } else { $HOME }
    Join-Path (Join-Path $base 'repos') 'winget-pkgs'
}

# vX.Y.Z -> X.Y.Z. A pre-release tag, or anything else, is not submitted (REL-041: stable releases only).
function ConvertTo-WingetVersion([string] $Tag) {
    if ($Tag -cmatch '^v(\d+\.\d+\.\d+)$') { return $Matches[1] }
    throw "not a stable release tag: '$Tag' (expected vX.Y.Z)"
}

# winget-pkgs layout: manifests/<first letter of the publisher>/<id parts>/<version>
function Get-ManifestRelativeDir([string] $Id, [string] $Version) {
    $segments = @('manifests', $Id.Substring(0, 1).ToLowerInvariant()) + ($Id -split '\.')
    if ($Version) { $segments += $Version }
    $segments -join '/'
}

# SHA256SUMS: "<64 hex>  <file name>" per line, as `sha256sum` writes it.
function ConvertFrom-ChecksumFile([string] $Text) {
    $sums = @{}
    foreach ($line in ($Text -split "\r?\n")) {
        if (-not $line.Trim()) { continue }
        if ($line -cmatch '^([0-9a-f]{64}) [ *]([^\s/\\]+)$') { $sums[$Matches[2]] = $Matches[1] }
        else { throw "unrecognised line in SHA256SUMS: '$line'" }
    }
    $sums
}

# The release notes' footer says whether the Windows files are Authenticode-signed (release.yml's notes step).
function Get-ReleaseChannel([string] $Body) {
    if ($Body -match '\*\*Unsigned release:\*\*') { return 'unsigned' }
    if ($Body -match '\*\*Signed release:\*\*') { return 'signed' }
    'unknown'
}

# Fills {{PLACEHOLDERS}} and drops the template's own comments; the yaml-language-server schema line stays.
# A placeholder without a value, or any `{{` left over, is an error.
function Expand-ManifestTemplate([string] $Text, [hashtable] $Values) {
    $lines = foreach ($line in ($Text -split "\r?\n")) {
        if ($line.StartsWith('#') -and -not $line.StartsWith('# yaml-language-server:')) { continue }
        foreach ($m in [regex]::Matches($line, '\{\{([A-Z0-9_]+)\}\}')) {
            $name = $m.Groups[1].Value
            if (-not $Values.ContainsKey($name)) { throw "no value for the placeholder {{$name}}" }
            $line = $line.Replace($m.Value, [string]$Values[$name])
        }
        if ($line.Contains('{{')) { throw "unresolved placeholder in: $line" }
        $line
    }
    ($lines -join "`n").TrimEnd() + "`n"
}

# Renders the three manifests of the package into <OutRoot>/manifests/<d>/<Publisher>/<Name>/<version>/ and returns that directory.
# The template directory must hold exactly <id>.yaml, <id>.locale.en-US.yaml and <id>.installer.yaml.
function New-ManifestSet([string] $TemplateDir, [string] $OutRoot, [string] $Id, [string] $Version, [hashtable] $Values) {
    $names = @("$Id.yaml", "$Id.locale.en-US.yaml", "$Id.installer.yaml")
    $found = @(Get-ChildItem -LiteralPath $TemplateDir -Filter '*.yaml' -File | ForEach-Object Name | Sort-Object)
    if (Compare-Object @($names | Sort-Object) $found) {
        throw "templates in $TemplateDir are [$($found -join ', ')], expected [$($names -join ', ')]"
    }
    $dir = Join-Path $OutRoot (Get-ManifestRelativeDir $Id $Version)
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    foreach ($name in $names) {
        $text = Expand-ManifestTemplate ([IO.File]::ReadAllText((Join-Path $TemplateDir $name))) $Values
        [IO.File]::WriteAllText((Join-Path $dir $name), $text, [Text.UTF8Encoding]::new($false))
    }
    $dir
}

# Fills the winget-pkgs pull request template: puts the description under its Description heading and ticks only the boxes the
# script verified. $Ticks keys: NoOtherPrs, OneManifest, Validated, InstallTested, Schema. The Contributor License Agreement box
# is never ticked: only the contributor can sign it. Returns the body and the requested ticks that found no matching line.
function Format-PullRequestBody([string] $Template, [string] $Description, [hashtable] $Ticks, [string] $Suffix) {
    $boxes = [ordered]@{
        NoOtherPrs    = 'other open'
        OneManifest   = 'only modifies one (1) manifest'
        Validated     = 'Validated manifest locally'
        InstallTested = 'Tested manifest locally'
        Schema        = 'Manifest conforms to'
    }
    $unmatched = [Collections.Generic.List[string]]::new()
    $lines = [Collections.Generic.List[string]]::new()
    $described = $false
    foreach ($line in ($Template -split "\r?\n")) { $lines.Add($line) }
    foreach ($key in $boxes.Keys) {
        if (-not $Ticks[$key]) { continue }
        $index = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -cmatch '^\s*- \[ \] ' -and $lines[$i].Contains($boxes[$key])) { $index = $i; break }
        }
        if ($index -lt 0) { $unmatched.Add($key) } else { $lines[$index] = $lines[$index].Replace('- [ ] ', '- [x] ') }
    }
    $out = [Collections.Generic.List[string]]::new()
    $pending = $false
    foreach ($line in $lines) {
        # The description goes under its heading, after the template's own HTML comment lines.
        if ($pending -and -not $line.TrimStart().StartsWith('<!--')) {
            $out.Add('')
            $out.Add($Description.TrimEnd())
            $pending = $false
            $described = $true
        }
        $out.Add($line)
        if (-not $described -and $line -cmatch '^##\s.*Description') { $pending = $true }
    }
    if ($pending) { $out.Add(''); $out.Add($Description.TrimEnd()); $described = $true }
    if (-not $described) { $out.Insert(0, ''); $out.Insert(0, $Description.TrimEnd()) }
    $body = ($out -join "`n").TrimEnd() + "`n"
    if ($Suffix) { $body += "`n" + $Suffix.Trim() + "`n" }
    [pscustomobject]@{ Body = $body; Unmatched = @($unmatched) }
}

# Runs a program with an argument vector (no shell, NFR-022) and returns its exit code and combined output.
function Invoke-Native([string] $File, [string[]] $Arguments) {
    $out = & $File @Arguments 2>&1
    $code = $LASTEXITCODE
    [pscustomobject]@{ Code = $code; Output = (($out | ForEach-Object { "$_" }) -join "`n").Trim() }
}

# Like Invoke-Native for git, and an error with the output when git fails.
function Invoke-Git([string] $Dir, [string[]] $GitArgs) {
    $r = Invoke-Native 'git' (@('-C', $Dir) + $GitArgs)
    if ($r.Code -ne 0) { throw "git $($GitArgs -join ' ') failed ($($r.Code)): $($r.Output)" }
    $r.Output
}

# Unauthenticated GitHub API call. `gh` is not used for the upstream repository: the Microsoft organisation enforces SAML SSO, which
# blocks `gh`'s token for every call until the maintainer authorises it, but public reads work without a token (60 requests/hour).
function Invoke-UpstreamApi([string] $Path) {
    $headers = @{ 'User-Agent' = 'dockering-winget-scripts'; Accept = 'application/vnd.github+json' }
    for ($attempt = 1; ; $attempt++) {
        $status = 0
        $body = Invoke-RestMethod -Uri "https://api.github.com/$Path" -Headers $headers -SkipHttpErrorCheck -StatusCodeVariable status
        if ($attempt -lt 2 -and ([int]$status -in 403, 429 -or [int]$status -ge 500)) { Start-Sleep -Seconds 5; continue }
        return [pscustomobject]@{ Status = [int]$status; Body = $body }
    }
}

# GitHub's explanation of an error answer (its JSON "message", or the raw text).
function Get-ApiMessage($Response) {
    $body = $Response.Body
    $text = if ($body -is [string]) { $body } elseif ($body.PSObject.Properties['message']) { [string]$body.message } else { '' }
    ($text -split "\r?\n")[0]
}

# What winget-pkgs holds for the package: does it exist, which versions, and the open pull requests that name the package.
function Get-UpstreamState([string] $Id, [string] $Version) {
    $packageDir = Get-ManifestRelativeDir $Id $null
    $r = Invoke-UpstreamApi "repos/$WingetUpstream/contents/$packageDir"
    $versions = @()
    $exists = $false
    if ($r.Status -eq 200) {
        $exists = $true
        $versions = @($r.Body | Where-Object type -eq 'dir' | ForEach-Object name)
    } elseif ($r.Status -ne 404) {
        throw "GitHub answered $($r.Status) for $packageDir in ${WingetUpstream}: $(Get-ApiMessage $r)"
    }
    $query = [uri]::EscapeDataString("repo:$WingetUpstream is:pr is:open `"$Id`" in:title")
    $s = Invoke-UpstreamApi "search/issues?q=$query"
    if ($s.Status -ne 200) { throw "GitHub answered $($s.Status) for the pull request search: $(Get-ApiMessage $s)" }
    $prs = @($s.Body.items | Where-Object { $_.title.Contains($Id) } | ForEach-Object {
        $pull = Invoke-UpstreamApi "repos/$WingetUpstream/pulls/$($_.number)"
        $label = if ($pull.Status -eq 200) { [string]$pull.Body.head.label } else { '' }
        [pscustomobject]@{ Number = $_.number; Title = $_.title; Url = $_.html_url; HeadLabel = $label }
    })
    [pscustomobject]@{
        PackageExists = $exists
        Versions      = $versions
        HasVersion    = [bool]$Version -and ($versions -contains $Version)
        OpenPrs       = $prs
    }
}

# Can `gh` reach the upstream repository? False with the SSO authorisation link when the organisation's SAML enforcement blocks it.
function Test-UpstreamAccess {
    $r = Invoke-Native 'gh' @('api', "repos/$WingetUpstream", '--jq', '.full_name')
    if ($r.Code -eq 0) { return [pscustomobject]@{ Ok = $true; SsoUrl = $null; Detail = $r.Output } }
    $url = $null
    if ($r.Output -match 'https://github\.com/enterprises/\S+') { $url = $Matches[0] }
    [pscustomobject]@{ Ok = $false; SsoUrl = $url; Detail = ($r.Output -split "\r?\n")[0] }
}

# Reads a rendered manifest directory back and returns the problems found (an empty array = fine): the three files, the identifier,
# the version, the manifest version, the installer URLs (they must point at this release of this repository) and the SHA-256 values.
function Test-RenderedManifests([string] $Dir, [string] $Id, [string] $Version, [string] $Repo, [hashtable] $Sums) {
    $problems = [Collections.Generic.List[string]]::new()
    $names = @("$Id.yaml", "$Id.locale.en-US.yaml", "$Id.installer.yaml")
    $manifestVersions = [Collections.Generic.HashSet[string]]::new()
    foreach ($name in $names) {
        $path = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $path)) { $problems.Add("missing $name"); continue }
        $text = [IO.File]::ReadAllText($path)
        if ($text.Contains("`r")) { $problems.Add("$name has CR characters (the manifests are written with LF)") }
        if ($text -notmatch "(?m)^PackageIdentifier: $([regex]::Escape($Id))\s*$") { $problems.Add("${name}: PackageIdentifier is not $Id") }
        if ($text -notmatch "(?m)^PackageVersion: `"$([regex]::Escape($Version))`"\s*$") { $problems.Add("${name}: PackageVersion is not $Version") }
        if ($text -match '(?m)^ManifestVersion: (\S+)\s*$') { [void]$manifestVersions.Add($Matches[1]) } else { $problems.Add("${name}: no ManifestVersion") }
        if ($text -match '\{\{|\}\}') { $problems.Add("${name}: unresolved placeholder") }
    }
    if ($manifestVersions.Count -gt 1) { $problems.Add("the files disagree on ManifestVersion: $($manifestVersions -join ', ')") }
    $installerPath = Join-Path $Dir "$Id.installer.yaml"
    if (Test-Path -LiteralPath $installerPath) {
        $text = [IO.File]::ReadAllText($installerPath)
        $prefix = "https://github.com/$Repo/releases/download/v$Version/"
        $urls = @([regex]::Matches($text, '(?m)^\s+InstallerUrl:\s*(\S+)\s*$') | ForEach-Object { $_.Groups[1].Value })
        $shas = @([regex]::Matches($text, '(?m)^\s+InstallerSha256:\s*"?([^"\s]+)"?\s*$') | ForEach-Object { $_.Groups[1].Value })
        if ($urls.Count -eq 0) { $problems.Add('the installer manifest has no InstallerUrl') }
        if ($urls.Count -ne $shas.Count) { $problems.Add("$($urls.Count) InstallerUrl but $($shas.Count) InstallerSha256") }
        for ($i = 0; $i -lt $urls.Count; $i++) {
            if (-not $urls[$i].StartsWith($prefix)) { $problems.Add("InstallerUrl $($urls[$i]) is not under $prefix"); continue }
            $asset = $urls[$i].Substring($prefix.Length)
            if ($i -lt $shas.Count) {
                if ($shas[$i] -cnotmatch '^[0-9A-F]{64}$') { $problems.Add("InstallerSha256 of $asset is not 64 uppercase hex digits") }
                elseif ($Sums -and $Sums.ContainsKey($asset) -and $Sums[$asset].ToUpperInvariant() -cne $shas[$i]) { $problems.Add("InstallerSha256 of $asset differs from SHA256SUMS") }
                elseif ($Sums -and -not $Sums.ContainsKey($asset)) { $problems.Add("$asset is not in SHA256SUMS") }
            }
        }
    }
    , @($problems)
}

# The published release to submit: the latest one, or the named tag. Sums comes from SHA256SUMS, Digests from the GitHub asset records
# (an independent source), both keyed by asset name.
function Get-ReleaseInfo([string] $Repo, [string] $Tag) {
    $ghArgs = @('release', 'view')
    if ($Tag) { $ghArgs += $Tag }
    $ghArgs += @('--repo', $Repo, '--json', 'tagName,isDraft,isPrerelease,body,url,publishedAt,assets')
    $r = Invoke-Native 'gh' $ghArgs
    if ($r.Code -ne 0) { throw "gh release view failed ($($r.Code)): $($r.Output)" }
    $release = $r.Output | ConvertFrom-Json
    $sums = @{}
    if (@($release.assets | Where-Object name -eq 'SHA256SUMS').Count -eq 1) {
        $d = Invoke-Native 'gh' @('release', 'download', $release.tagName, '--repo', $Repo, '--pattern', 'SHA256SUMS', '--output', '-')
        if ($d.Code -ne 0) { throw "gh release download SHA256SUMS failed ($($d.Code)): $($d.Output)" }
        $sums = ConvertFrom-ChecksumFile $d.Output
    }
    $digests = @{}
    foreach ($a in $release.assets) {
        if ($a.digest -and ([string]$a.digest) -cmatch '^sha256:([0-9a-f]{64})$') { $digests[$a.name] = $Matches[1] }
    }
    [pscustomobject]@{
        Tag          = [string]$release.tagName
        IsDraft      = [bool]$release.isDraft
        IsPrerelease = [bool]$release.isPrerelease
        Url          = [string]$release.url
        PublishedAt  = [string]$release.publishedAt
        Channel      = Get-ReleaseChannel ([string]$release.body)
        Assets       = @($release.assets | ForEach-Object name)
        Sums         = $sums
        Digests      = $digests
    }
}

# One winget-pkgs pull request: state, labels (the validation bot reports through them) and comment count. Read without a token.
function Get-PullRequestState([int] $Number) {
    $r = Invoke-UpstreamApi "repos/$WingetUpstream/pulls/$Number"
    if ($r.Status -ne 200) { throw "GitHub answered $($r.Status) for pull request #$Number of ${WingetUpstream}: $(Get-ApiMessage $r)" }
    $p = $r.Body
    [pscustomobject]@{
        Title    = [string]$p.title
        Url      = [string]$p.html_url
        State    = [string]$p.state
        Merged   = [bool]$p.merged
        Draft    = [bool]$p.draft
        Labels   = @($p.labels | ForEach-Object name)
        Comments = [int]$p.comments
    }
}
