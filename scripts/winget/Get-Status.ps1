#Requires -Version 7.3
# Where does winget publishing of Dockering stand? (REL-040, REL-041) Read-only: it changes nothing anywhere.
# Usage: pwsh -NoProfile -File scripts/winget/Get-Status.ps1 [-Tag vX.Y.Z] [-Repo owner/name] [-CloneDir <dir>] [-Pr <number>]
# Without -Pr it compares the latest published release (or -Tag) with microsoft/winget-pkgs and ends with one NEXT line:
#   NEXT submit-new-package   the package is not in winget-pkgs yet: the first submission
#   NEXT submit-new-version   the package exists and this version doesn't
#   NEXT wait-for-pr          an open pull request already names the package: wait for it (listed above), submit nothing
#   NEXT none                 winget-pkgs already has this version
# With -Pr <number> it reports one winget-pkgs pull request instead (state, labels) and ends with
#   PR_STATE merged | closed | needs-author-feedback | open
# winget-pkgs is read without a token (public API, 60 requests an hour): `gh` can't reach it until the maintainer authorises the token
# for the Microsoft organisation's SAML SSO, which the `upstream.access` line reports. Exit 0 = reported, 1 = could not find out.
param(
    [string] $Tag,
    [string] $Repo = 'pavel-purma/dockering',
    [string] $CloneDir,
    [int] $Pr
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Lib.ps1')

function Write-PullRequestStatus([int] $Number) {
    $state = Get-PullRequestState $Number
    Info 'pr.title' "#$Number $($state.Title) ($($state.Url))"
    Info 'pr.state' "$($state.State)$(if ($state.Draft) { ', draft' })$(if ($state.Merged) { ', merged' })"
    Info 'pr.labels' $(if ($state.Labels.Count) { $state.Labels -join ', ' } else { 'none' })
    Info 'pr.comments' "$($state.Comments) comment(s): gh pr view $Number --repo $WingetUpstream --comments"
    $code = if ($state.Merged) { 'merged' } elseif ($state.State -eq 'closed') { 'closed' } elseif ($state.Labels -contains 'Needs-Author-Feedback') { 'needs-author-feedback' } else { 'open' }
    Write-Host "PR_STATE $code"
}

function Invoke-Main {
    if ($Pr) { Write-PullRequestStatus $Pr; return }

    $root = Get-RepoRoot
    $release = Get-ReleaseInfo $Repo $Tag
    $version = ConvertTo-WingetVersion $release.Tag
    Pass 'release.latest' "$($release.Tag) published $($release.PublishedAt) ($($release.Url))"
    switch ($release.Channel) {
        'signed' { Pass 'release.channel' 'signed release' }
        'unsigned' { Warn 'release.channel' 'unsigned release: REL-041 asks for the first submission after a signed release, so the maintainer has to approve it' }
        default { Warn 'release.channel' 'the release notes name neither a signed nor an unsigned release' }
    }
    $missing = @(@($WingetInstallers.Keys) + 'SHA256SUMS' | Where-Object { $release.Assets -notcontains $_ })
    if ($missing) { Warn 'release.assets' "missing: $($missing -join ', ')" } else { Pass 'release.assets' 'both installers and SHA256SUMS are assets' }

    $upstream = Get-UpstreamState $WingetPackageId $version
    if ($upstream.PackageExists) { Info 'upstream.package' "$WingetPackageId is in winget-pkgs, versions: $($upstream.Versions -join ', ')" }
    else { Info 'upstream.package' "$WingetPackageId is not in winget-pkgs yet" }
    foreach ($p in $upstream.OpenPrs) { Warn 'upstream.open_pr' "#$($p.Number) $($p.Title) ($($p.Url))" }
    if ($upstream.HasVersion) { Pass 'upstream.version' "$version is already in winget-pkgs" } else { Info 'upstream.version' "$version is not in winget-pkgs" }

    $access = Test-UpstreamAccess
    if ($access.Ok) { Pass 'upstream.access' "gh can reach $WingetUpstream" }
    else {
        Warn 'upstream.access' "gh can't reach $WingetUpstream ($($access.Detail))"
        if ($access.SsoUrl) { Warn 'upstream.sso' "the maintainer authorises the gh token for the Microsoft organisation here: $($access.SsoUrl)" }
    }

    if (-not $CloneDir) { $CloneDir = Get-DefaultCloneDir $root }
    if (Test-Path -LiteralPath (Join-Path $CloneDir '.git')) { Info 'clone' "$CloneDir exists" } else { Info 'clone' "$CloneDir does not exist yet; Submit-Manifest.ps1 creates it" }

    $next = if ($upstream.HasVersion) { 'none' } elseif ($upstream.OpenPrs.Count) { 'wait-for-pr' } elseif ($upstream.PackageExists) { 'submit-new-version' } else { 'submit-new-package' }
    Write-Host "VERSION $version"
    Write-Host "NEXT $next"
}

$script:ExitCode = 1
try {
    Invoke-Main
    $script:ExitCode = 0
} catch {
    Write-Check 'FAIL' 'error' $_.Exception.Message
}
exit $script:ExitCode
