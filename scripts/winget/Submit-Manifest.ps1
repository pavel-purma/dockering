#Requires -Version 7.3
# Prepares, and with -Submit opens, the microsoft/winget-pkgs pull request for a rendered Dockering manifest (REL-040, REL-041).
# Usage: pwsh -NoProfile -File scripts/winget/Submit-Manifest.ps1 [-Tag vX.Y.Z] [-ManifestDir <dir>] [-CloneDir <dir>] [-Repo owner/name]
#          [-Branch <name>] [-InstallTested] [-Attribution <text>] [-CommitAttribution <text>] [-Submit]
# Run New-Manifest.ps1 first: -ManifestDir defaults to the directory it wrote (target/winget/<version>/manifests/d/Dockering/Dockering/<version>).
# Without -Submit everything stays on this machine: the clone is created or refreshed, the branch is made from the tip of winget-pkgs, the
#   manifests are copied in and committed, and the pull request title and body are written to target/winget/<version>/pr-body.md. Nothing is pushed.
# With -Submit it also needs `gh` to reach microsoft/winget-pkgs (the Microsoft organisation's SAML SSO must be authorised for the token),
#   creates the maintainer's fork if there is none, pushes the branch to the fork and opens the pull request. It never force-pushes.
# The clone is <root of the drive that holds this worktree>/repos/winget-pkgs (D:\repos\winget-pkgs for a worktree on D:; ~/repos/winget-pkgs
#   off Windows), a blobless, depth-1, sparse clone of microsoft/winget-pkgs with the remote `upstream`; the fork is the remote `fork`.
# The pull request template's boxes are ticked only for what this script verified. The Contributor License Agreement box is never ticked
#   (only the contributor can sign it); "Tested manifest locally" stays open unless -InstallTested says the maintainer ran the manifest in
#   a sandbox or VM (never run `winget install --manifest` on a machine that has Dockering installed: it replaces that installation).
# One line per check, like New-Manifest.ps1. Last lines: BRANCH, COMMIT, TITLE, BODY_FILE and, with -Submit, PR_URL. Exit 0 = done, 1 = a check failed.
param(
    [string] $Tag,
    [string] $ManifestDir,
    [string] $CloneDir,
    [string] $Repo = 'pavel-purma/dockering',
    [string] $Branch,
    [switch] $InstallTested,
    [string] $Attribution,
    [string] $CommitAttribution,
    [switch] $Submit
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Lib.ps1')

$UpstreamUrl = "https://github.com/$WingetUpstream.git"
$SparsePaths = @('manifests/d/Dockering', '.github', 'Tools')

# The remote of the clone that points at microsoft/winget-pkgs, or $null.
function Get-UpstreamRemote([string] $Dir) {
    $r = Invoke-Native 'git' @('-C', $Dir, 'config', '--get-regexp', '^remote\..+\.url$')
    foreach ($line in ($r.Output -split "\r?\n")) {
        if ($line -cmatch '^remote\.(.+)\.url (\S+)$') {
            $name, $url = $Matches[1], $Matches[2]
            if ($url -match '(?i)github\.com[:/]microsoft/winget-pkgs(\.git)?/?$') { return $name }
        }
    }
    $null
}

function Initialize-Clone([string] $Dir) {
    if (-not (Test-Path -LiteralPath (Join-Path $Dir '.git'))) {
        if ((Test-Path -LiteralPath $Dir) -and @(Get-ChildItem -LiteralPath $Dir -Force).Count) { Fail 'clone' "$Dir exists, is not empty and is not a git clone" }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Dir) | Out-Null
        $r = Invoke-Native 'git' @('clone', '--quiet', '--filter=blob:none', '--depth', '1', '--sparse', '--origin', 'upstream', $UpstreamUrl, $Dir)
        if ($r.Code -ne 0) { Fail 'clone' "git clone failed ($($r.Code)): $($r.Output)" }
        Pass 'clone' "created $Dir (blobless, depth 1, sparse)"
    }
    $remote = Get-UpstreamRemote $Dir
    if (-not $remote) { Fail 'clone' "$Dir has no remote for $UpstreamUrl; use another -CloneDir" }
    $dirty = Invoke-Git $Dir @('status', '--porcelain')
    if ($dirty) { Fail 'clone' "$Dir has uncommitted changes:`n$dirty`nClean it up yourself (git -C $Dir status), then run again" }
    [void](Invoke-Git $Dir (@('sparse-checkout', 'set') + $SparsePaths))
    [void](Invoke-Git $Dir @('fetch', '--quiet', '--depth', '1', $remote, $WingetUpstreamBranch))
    $tip = Invoke-Git $Dir @('rev-parse', '--short', 'FETCH_HEAD')
    Pass 'clone.fetch' "$remote/$WingetUpstreamBranch at $tip"
    $remote
}

function Invoke-Main {
    $root = Get-RepoRoot
    $release = Get-ReleaseInfo $Repo $Tag
    $version = ConvertTo-WingetVersion $release.Tag
    Pass 'release.tag' "$($release.Tag) -> version $version"
    if ($release.IsDraft -or $release.IsPrerelease) { Fail 'release.published' "$($release.Tag) is not a published stable release" }

    $relative = Get-ManifestRelativeDir $WingetPackageId $version
    if (-not $ManifestDir) { $ManifestDir = Join-Path $root "target/winget/$version/$relative" }
    if (-not (Test-Path -LiteralPath $ManifestDir)) { Fail 'manifest.dir' "$ManifestDir does not exist; run scripts/winget/New-Manifest.ps1 first" }
    $ManifestDir = (Resolve-Path -LiteralPath $ManifestDir).Path
    $problems = Test-RenderedManifests $ManifestDir $WingetPackageId $version $Repo $release.Sums
    if ($problems.Count) {
        foreach ($p in $problems) { Write-Check 'FAIL' 'manifest.check' $p }
        throw 'check failed'
    }
    Pass 'manifest.check' "$ManifestDir agrees with $($release.Tag)"
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        $v = Invoke-Native 'winget' @('validate', '--manifest', $ManifestDir)
        if ($v.Code -ne 0) { Fail 'winget.validate' "winget validate failed ($($v.Code)): $($v.Output)" }
        Pass 'winget.validate' ($v.Output -split "\r?\n" | Select-Object -Last 1)
    } else {
        Fail 'winget.validate' 'winget is not installed; the pull request says the manifest was validated, so it has to be'
    }

    $upstream = Get-UpstreamState $WingetPackageId $version
    if ($upstream.HasVersion) { Fail 'upstream.version' "$WingetPackageId $version is already in winget-pkgs" }
    if ($upstream.OpenPrs.Count) { Fail 'upstream.open_pr' "an open pull request already names the package: #$($upstream.OpenPrs[0].Number) $($upstream.OpenPrs[0].Url)" }
    Pass 'upstream.state' "$(if ($upstream.PackageExists) { 'package exists' } else { 'new package' }); no open pull request names it"

    if (-not $CloneDir) { $CloneDir = Get-DefaultCloneDir $root }
    $remote = Initialize-Clone $CloneDir

    $title = if ($upstream.PackageExists) { "New version: $WingetPackageId version $version" } else { "New package: $WingetPackageId version $version" }
    if (-not $Branch) { $Branch = "$WingetPackageId-$version" }
    [void](Invoke-Git $CloneDir @('switch', '--quiet', '-C', $Branch, 'FETCH_HEAD'))
    $target = Join-Path $CloneDir $relative
    if (Test-Path -LiteralPath $target) { Fail 'clone.target' "$relative already exists in the clone" }
    New-Item -ItemType Directory -Force -Path $target | Out-Null
    $names = @(Get-ChildItem -LiteralPath $ManifestDir -Filter '*.yaml' -File | ForEach-Object Name)
    foreach ($name in $names) { Copy-Item -LiteralPath (Join-Path $ManifestDir $name) -Destination (Join-Path $target $name) }
    [void](Invoke-Git $CloneDir @('-c', 'core.autocrlf=false', 'add', '--', $relative))
    $staged = @((Invoke-Git $CloneDir @('diff', '--cached', '--name-only')) -split "\r?\n" | Where-Object { $_ } | Sort-Object)
    $expected = @($names | ForEach-Object { "$relative/$_" } | Sort-Object)
    if (Compare-Object $expected $staged) { Fail 'clone.staged' "staged [$($staged -join ', ')], expected exactly [$($expected -join ', ')]" }
    $message = $title
    if ($CommitAttribution) { $message += "`n`n$($CommitAttribution.Trim())" }
    $c = Invoke-Native 'git' @('-C', $CloneDir, '-c', 'core.autocrlf=false', 'commit', '--quiet', '-m', $message)
    if ($c.Code -ne 0) { Fail 'clone.commit' "git commit failed ($($c.Code)): $($c.Output)" }
    $commit = Invoke-Git $CloneDir @('rev-parse', 'HEAD')
    Pass 'clone.commit' "branch $Branch, commit $($commit.Substring(0, 9)): $title"

    $templatePath = Join-Path $CloneDir '.github/PULL_REQUEST_TEMPLATE.md'
    $template = $null
    if (Test-Path -LiteralPath $templatePath) { $template = [IO.File]::ReadAllText($templatePath) }
    if (-not $template) { Warn 'pr.template' "no $templatePath in the clone; using the built-in copy"; $template = $FallbackPullRequestTemplate }
    $channelText = switch ($release.Channel) {
        'signed' { 'The installers are Authenticode-signed.' }
        'unsigned' { 'The installers are **not** Authenticode-signed (the project publishes unsigned builds; the release notes say so).' }
        default { '' }
    }
    $description = @(
        "$WingetPackageId $version, built from [$($release.Tag)]($($release.Url)) of https://github.com/$Repo.",
        '',
        '- Inno Setup installers for x64 and arm64, per-user (default) and per-machine (`/ALLUSERS`) scope, `UpgradeBehavior: install`.',
        '- Each `InstallerSha256` equals the value in the release''s `SHA256SUMS` and GitHub''s own digest of the asset, and the public URL was downloaded and hashed again.',
        $(if ($channelText) { "- $channelText" })
    ) -join "`n"
    $ticks = @{ NoOtherPrs = $true; OneManifest = $true; Validated = $true; Schema = $true; InstallTested = [bool]$InstallTested }
    $pr = Format-PullRequestBody $template $description $ticks $Attribution
    foreach ($key in $pr.Unmatched) { Warn 'pr.template' "the template has no '$key' box to tick (winget-pkgs changed its template)" }
    $bodyFile = Join-Path $root "target/winget/$version/pr-body.md"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $bodyFile) | Out-Null
    [IO.File]::WriteAllText($bodyFile, $pr.Body, [Text.UTF8Encoding]::new($false))
    Pass 'pr.body' $bodyFile
    if (-not $InstallTested) { Info 'pr.install' 'the "Tested manifest locally" box is left open: it was not installed in a sandbox' }
    Info 'pr.cla' 'the Contributor License Agreement box is left open: the maintainer signs it at https://cla.opensource.microsoft.com'

    $prUrl = $null
    if ($Submit) {
        $access = Test-UpstreamAccess
        if (-not $access.Ok) {
            $hint = if ($access.SsoUrl) { " Authorise the gh token for the Microsoft organisation at $($access.SsoUrl) and run again." } else { '' }
            Fail 'gh.access' "gh can't reach $WingetUpstream ($($access.Detail)).$hint"
        }
        Pass 'gh.access' "gh can reach $WingetUpstream"
        $login = (Invoke-Native 'gh' @('api', 'user', '--jq', '.login')).Output
        if (-not $login) { Fail 'gh.user' 'gh could not tell who is logged in' }
        $forkName = "$login/winget-pkgs"
        $f = Invoke-Native 'gh' @('api', "repos/$forkName", '--jq', '[.fork, .parent.full_name] | @tsv')
        if ($f.Code -ne 0) {
            Info 'fork' "$forkName does not exist; creating it"
            $created = Invoke-Native 'gh' @('repo', 'fork', $WingetUpstream, '--clone=false')
            if ($created.Code -ne 0) { Fail 'fork' "gh repo fork failed ($($created.Code)): $($created.Output)" }
            for ($i = 0; $i -lt 12 -and $f.Code -ne 0; $i++) {
                Start-Sleep -Seconds 5
                $f = Invoke-Native 'gh' @('api', "repos/$forkName", '--jq', '[.fork, .parent.full_name] | @tsv')
            }
            if ($f.Code -ne 0) { Fail 'fork' "$forkName is still not available a minute after forking" }
        }
        if ($f.Output -cne "true`t$WingetUpstream") { Fail 'fork' "$forkName is not a fork of $WingetUpstream ($($f.Output))" }
        Pass 'fork' $forkName
        $forkUrl = "https://github.com/$forkName.git"
        $remotes = @((Invoke-Git $CloneDir @('remote')) -split "\r?\n")
        if ($remotes -contains 'fork') { [void](Invoke-Git $CloneDir @('remote', 'set-url', 'fork', $forkUrl)) } else { [void](Invoke-Git $CloneDir @('remote', 'add', 'fork', $forkUrl)) }
        $existing = Invoke-Native 'gh' @('pr', 'list', '--repo', $WingetUpstream, '--author', $login, '--head', $Branch, '--state', 'open', '--json', 'url', '--jq', '.[0].url')
        if ($existing.Code -eq 0 -and $existing.Output) {
            $prUrl = $existing.Output
            Pass 'pr.exists' "already open: $prUrl"
        } else {
            $sync = Invoke-Native 'gh' @('repo', 'sync', $forkName, '--branch', $WingetUpstreamBranch)
            if ($sync.Code -eq 0) { Pass 'fork.sync' "$forkName/$WingetUpstreamBranch is up to date with $WingetUpstream" }
            else { Warn 'fork.sync' "could not update $forkName/${WingetUpstreamBranch}: $($sync.Output)" }
            $p = Invoke-Native 'git' @('-C', $CloneDir, 'push', '--quiet', 'fork', "${Branch}:refs/heads/$Branch")
            if ($p.Code -ne 0) { Fail 'push' "git push to $forkUrl failed ($($p.Code)): $($p.Output)`nIf it says 'shallow update not allowed', the fork's $WingetUpstreamBranch is behind winget-pkgs: update it (gh repo sync $forkName) and run again. If the branch exists there from an earlier attempt, delete it or pass another -Branch; this script never force-pushes" }
            Pass 'push' "$Branch -> $forkName"
            $created = Invoke-Native 'gh' @('pr', 'create', '--repo', $WingetUpstream, '--base', $WingetUpstreamBranch, '--head', "${login}:$Branch", '--title', $title, '--body-file', $bodyFile)
            if ($created.Code -ne 0) { Fail 'pr.create' "gh pr create failed ($($created.Code)): $($created.Output)" }
            $prUrl = ($created.Output -split "\r?\n" | Where-Object { $_ -match '^https://' } | Select-Object -Last 1)
            Pass 'pr.create' $prUrl
        }
    } else {
        Skip 'submit' 'nothing was pushed (run again with -Submit to open the pull request)'
    }
    Write-Host "BRANCH $Branch"
    Write-Host "COMMIT $commit"
    Write-Host "TITLE $title"
    Write-Host "BODY_FILE $bodyFile"
    if ($prUrl) { Write-Host "PR_URL $prUrl" }
}

$script:ExitCode = 1
try {
    Invoke-Main
    $script:ExitCode = 0
} catch {
    if ($_.Exception.Message -ne 'check failed') { Write-Check 'FAIL' 'error' $_.Exception.Message }
}
exit $script:ExitCode
