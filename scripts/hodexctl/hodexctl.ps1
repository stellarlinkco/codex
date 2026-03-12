param(
    [Parameter(Position = 0)]
    [string]$Command = "install",

    [Parameter(Position = 1)]
    [string]$Version = "latest",

    [string]$Repo = $(if ($env:HODEX_REPO) { $env:HODEX_REPO } else { "stellarlinkco/codex" }),
    [Alias("InstallDir")]
    [string]$CommandDir,
    [string]$StateDir = $(if ($env:HODEX_STATE_DIR) { $env:HODEX_STATE_DIR } elseif ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "hodex" } else { Join-Path $HOME "AppData\Local\hodex" }),
    [string]$DownloadDir = $(if ($env:HODEX_DOWNLOAD_DIR) { $env:HODEX_DOWNLOAD_DIR } else { Join-Path $HOME "Downloads" }),
    [ValidateSet("ask", "skip", "native", "nvm", "manual")]
    [string]$NodeMode = "ask",
    [switch]$Yes,
    [switch]$NoPathUpdate,
    [string]$GitHubToken = $env:GITHUB_TOKEN,
    [string]$GitUrl,
    [string]$Ref,
    [string]$CheckoutDir,
    [Alias("Name")]
    [string]$Profile,
    [switch]$Activate,
    [switch]$NoActivate,
    [switch]$KeepCheckout,
    [switch]$RemoveCheckout,
    [switch]$List,
    [switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$script:NodeDownloadUrl = "https://nodejs.org/en/download"
$script:NvmWindowsReleaseUrl = "https://github.com/coreybutler/nvm-windows/releases"
$script:ControllerUrlBase = if ($env:HODEX_CONTROLLER_URL_BASE) { $env:HODEX_CONTROLLER_URL_BASE.TrimEnd("/") } else { "https://raw.githubusercontent.com" }
$script:ReleaseBaseUrl = if ($env:HODEX_RELEASE_BASE_URL) { $env:HODEX_RELEASE_BASE_URL.TrimEnd("/") } else { "" }
$script:ExplicitCommandDir = $PSBoundParameters.ContainsKey("CommandDir")
$script:PlatformLabel = ""
$script:ArchitectureName = ""
$script:AssetCandidates = @()
$script:PathUpdateMode = "skipped"
$script:PathProfile = ""
$script:PathManagedByHodexctl = $false
$script:PathDetectedSource = ""
$script:NodeSetupChoice = "skip"
$script:State = $null
$script:RequestedCommand = $Command
$script:RequestedVersion = $Version
$script:RepoName = $Repo
$script:ExplicitSourceRepo = $PSBoundParameters.ContainsKey("Repo")
$script:StateRoot = $StateDir
$script:DefaultCommandDir = Join-Path $script:StateRoot "commands"
$script:CurrentCommandDir = $CommandDir
$script:DownloadRoot = $DownloadDir
$script:SelectedNodeMode = $NodeMode
$script:ApiToken = $GitHubToken
$script:ApiUserAgent = "hodexctl"
$script:LastGhFallbackReason = ""
$script:LastGhFallbackDetail = ""
$script:InvokedWithNoArgs = ($PSBoundParameters.Count -eq 0)
$script:DisplayCommand = if ($env:HODEX_DISPLAY_NAME) { $env:HODEX_DISPLAY_NAME } else { ".\hodexctl.ps1" }
$script:SourceAction = ""
$script:DefaultSourceProfileName = "codex-source"
$script:DefaultSourceRef = "main"
$script:ExplicitSourceProfile = $PSBoundParameters.ContainsKey("Profile") -or $PSBoundParameters.ContainsKey("Name")
$script:SourceProfile = $Profile
$script:SourceRef = if ([string]::IsNullOrWhiteSpace($Ref)) { $script:DefaultSourceRef } else { $Ref }
$script:SourceGitUrl = $GitUrl
$script:SourceCheckoutDir = $CheckoutDir
$script:SourceCheckoutPolicy = if ($RemoveCheckout) { "remove" } elseif ($KeepCheckout) { "keep" } else { "ask" }
$script:ExplicitSourceRef = $PSBoundParameters.ContainsKey("Ref")
$script:RawSourceHelpRequest = ($Command -eq "source" -and $Version -eq "help")

if ($PSBoundParameters.ContainsKey("Name")) {
    Write-Warning "-Name is deprecated; use -Profile."
}

function Show-Usage {
    $standaloneCommand = ".\hodexctl.ps1"
    @"
Usage:
  $($script:DisplayCommand)
  $($script:DisplayCommand) <command> [version] [options]

Commands:
  install [version]      Install or reinstall hodex (default: latest)
  upgrade [version]      Upgrade to latest or a specific version
  download [version]     Download release asset to download dir (default: latest)
  downgrade <version>    Downgrade to a specific version
  source <action>        Source download/sync/toolchain management
  uninstall              Remove hodex files
  status                 Show current install status
  list                   Interactive list of available versions with changelog
  relink                 Regenerate hodex / hodexctl wrappers
  repair                 Repair wrapper / PATH / state drift
  help                   Show help

Options:
  -Repo <owner/repo>             GitHub repo (default: stellarlinkco/codex)
  -CommandDir <path>             Command dir for hodex / hodexctl
  -StateDir <path>               State dir (default: %LOCALAPPDATA%\hodex)
  -DownloadDir <path>            Download dir (default: ~/Downloads)
  -NodeMode <mode>               Node handling: ask|skip|native|nvm|manual
  -GitUrl <url>                  Source mode Git clone URL
  -Ref <branch|tag|commit>       Source mode ref (default: main)
  -CheckoutDir <path>            Source mode checkout dir
  -Profile <profile-name>        Source profile name (default: codex-source)
  -KeepCheckout                  Keep checkout on source uninstall
  -RemoveCheckout                Remove checkout on source uninstall
  -List                          Same as list
  -Yes                           Non-interactive (accept defaults)
  -NoPathUpdate                  Do not modify PATH
  -GitHubToken <token>           GitHub API token (mitigate rate limit)
  -Help                          Show help

Examples (after install, recommended via hodexctl):
  hodexctl
  hodexctl status
  hodexctl list
  hodexctl upgrade
  hodexctl download 1.2.3 -DownloadDir ~/Downloads
  hodexctl downgrade 1.2.2
  hodexctl source install -Repo stellarlinkco/codex -Ref main
  hodexctl source switch -Profile codex-source -Ref feature/my-branch
  hodexctl source status
  hodexctl source list
  hodexctl relink -CommandDir %LOCALAPPDATA%\hodex\commands
  hodexctl repair
  hodexctl uninstall

Examples (run script directly):
  $standaloneCommand install
  $standaloneCommand install 1.2.2
  $standaloneCommand upgrade
  $standaloneCommand download 1.2.3 -DownloadDir ~/Downloads
  $standaloneCommand list
  $standaloneCommand downgrade 1.2.2
  $standaloneCommand source install -GitUrl https://github.com/stellarlinkco/codex.git -Ref main
  $standaloneCommand relink -CommandDir %LOCALAPPDATA%\hodex\commands
  $standaloneCommand repair
  $standaloneCommand uninstall
"@ | Write-Host
}

function Show-SourceUsage {
    @"
Source mode usage:
  $($script:DisplayCommand) source <action> [options]

Actions:
  install                Download source and prepare toolchain (does not take over hodex)
  update                 Sync latest code for current ref and reuse checkout
  switch                 Switch to specified -Ref and sync source
  status                 Show source profile status
  uninstall              Remove source profile; optional checkout deletion
  list                   List all source profiles
  help                   Show this help

Common options:
  -Repo <owner/repo>             GitHub repo name
  -GitUrl <url>                  HTTPS / SSH Git URL
  -Ref <branch|tag|commit>       Source ref
  -CheckoutDir <path>            Source checkout dir
  -Profile <profile-name>        Source profile name (workspace id), default codex-source
                                  Note: this is not a command name and does not take over hodex
  -KeepCheckout / -RemoveCheckout Control whether to keep checkout on uninstall
"@ | Write-Host
}

function Show-ListUsage {
    @"
Release list usage:
  $($script:DisplayCommand) list

List view actions:
  Enter number to view changelog
  Enter 0 to open source download/management
  Press Enter to exit

Changelog view actions:
  a        AI summary (hodex/codex)
  i        Install selected version
  d        Download asset for current platform
  b        Back to version list
  q        Quit
"@ | Write-Host
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Write-Info {
    param([string]$Message)
    Write-Host $Message
}

function Write-WarnLine {
    param([string]$Message)
    Write-Warning $Message
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Normalize-Version {
    param([string]$RawVersion)

    if ([string]::IsNullOrWhiteSpace($RawVersion) -or $RawVersion -eq "latest") {
        return "latest"
    }

    if ($RawVersion.StartsWith("rust-v")) {
        return $RawVersion.Substring(6)
    }

    if ($RawVersion.StartsWith("v")) {
        return $RawVersion.Substring(1)
    }

    return $RawVersion
}

function Normalize-UserPath {
    param([string]$RawPath)

    if ([string]::IsNullOrWhiteSpace($RawPath)) {
        return $RawPath
    }

    if ($RawPath -eq "~") {
        return $HOME
    }

    if ($RawPath.StartsWith("~/") -or $RawPath.StartsWith("~\")) {
        $relative = $RawPath.Substring(2).TrimStart("\", "/")
        return [System.IO.Path]::GetFullPath((Join-Path $HOME $relative))
    }

    if ([System.IO.Path]::IsPathRooted($RawPath)) {
        return [System.IO.Path]::GetFullPath($RawPath)
    }

    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $RawPath))
}

function Test-Command {
    param([string]$Name)
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Get-ApiHeaders {
    $headers = @{
        Accept = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2022-11-28"
        "User-Agent" = $script:ApiUserAgent
    }

    if (-not [string]::IsNullOrWhiteSpace($script:ApiToken)) {
        $headers.Authorization = "Bearer $script:ApiToken"
    }

    return $headers
}

function Invoke-GitHubApi {
    param([string]$Uri)
    $script:LastGhFallbackReason = ""
    $script:LastGhFallbackDetail = ""

    try {
        return Invoke-WithRetry -Label "github-api" -ScriptBlock {
            Invoke-RestMethod -Uri $Uri -Headers (Get-ApiHeaders)
        }
    } catch {
        $fallback = Invoke-GhApiFallback -Uri $Uri
        if ($null -ne $fallback) {
            if ($script:LastGhFallbackReason -eq "gh-success" -and -not [string]::IsNullOrWhiteSpace($script:LastGhFallbackDetail)) {
                Write-Info $script:LastGhFallbackDetail
            }
            return $fallback
        }
        throw
    }
}

function Get-GhApiPathFromUri {
    param([string]$Uri)

    if ($Uri.StartsWith("https://api.github.com/")) {
        return $Uri.Substring("https://api.github.com/".Length)
    }
    return $null
}

function Invoke-GhApiFallback {
    param([string]$Uri)

    $script:LastGhFallbackReason = ""
    $script:LastGhFallbackDetail = ""

    if (-not (Test-Command "gh")) {
        $script:LastGhFallbackReason = "gh-missing"
        return $null
    }

    $apiPath = Get-GhApiPathFromUri -Uri $Uri
    if ([string]::IsNullOrWhiteSpace($apiPath)) {
        $script:LastGhFallbackReason = "gh-unsupported"
        return $null
    }

    $stderrFile = [System.IO.Path]::GetTempFileName()
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    try {
        $argumentList = @("api", "-H", "Accept: application/vnd.github+json", "-H", "X-GitHub-Api-Version: 2022-11-28", $apiPath)
        if (-not [string]::IsNullOrWhiteSpace($script:ApiToken)) {
            $env:GH_TOKEN = $script:ApiToken
        }

        & gh @argumentList 1> $stdoutFile 2> $stderrFile
        if ($LASTEXITCODE -eq 0) {
            $script:LastGhFallbackReason = "gh-success"
            $script:LastGhFallbackDetail = "Automatically switched to gh api to fetch GitHub data."
            $json = Get-Content -LiteralPath $stdoutFile -Raw
            return $json | ConvertFrom-Json -Depth 100
        }

        $detail = if (Test-Path -LiteralPath $stderrFile) { (Get-Content -LiteralPath $stderrFile -Raw).Trim() } else { "" }
        $script:LastGhFallbackDetail = $detail
        if ($detail -match 'not logged in|authenticate|gh auth login|authentication required') {
            $script:LastGhFallbackReason = "gh-not-authenticated"
        } elseif ($detail -match 'HTTP 401|HTTP 403|HTTP 404|Resource not accessible|Not Found|Forbidden|insufficient_scope|requires authentication') {
            $script:LastGhFallbackReason = "gh-access-denied"
        } else {
            $script:LastGhFallbackReason = "gh-failed"
        }
        return $null
    } finally {
        if (-not [string]::IsNullOrWhiteSpace($script:ApiToken)) {
            Remove-Item Env:\GH_TOKEN -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
    }
}

function Get-GitHubApiFailureMessage {
    param([string]$BaseMessage)

    $base = $BaseMessage.Trim()
    $baseSentence = if ($base -match '[.!?]$') { $base } else { "$base." }

    switch ($script:LastGhFallbackReason) {
        "gh-success" { return "$baseSentence`n$($script:LastGhFallbackDetail)" }
        "gh-missing" { return "$baseSentence gh not found; set GITHUB_TOKEN or install/login to gh and retry." }
        "gh-not-authenticated" { return "$baseSentence gh fallback was attempted but gh is not logged in; run 'gh auth login' or set GITHUB_TOKEN and retry." }
        "gh-access-denied" { return "$baseSentence gh fallback was attempted but the current gh login/token lacks permission for $($script:RepoName): $($script:LastGhFallbackDetail)" }
        "gh-failed" { return "$baseSentence gh fallback was attempted but gh api still failed: $($script:LastGhFallbackDetail)" }
        default {
            if (-not [string]::IsNullOrWhiteSpace($script:ApiToken)) {
                return "$baseSentence GITHUB_TOKEN was provided, but GitHub API is still unavailable; you can also try 'gh auth login' and retry."
            }
            return "$baseSentence Set GITHUB_TOKEN or install/login to gh and retry."
        }
    }
}

function Get-RetryAttempts {
    if ($env:HODEX_RETRY_ATTEMPTS -match '^\d+$' -and [int]$env:HODEX_RETRY_ATTEMPTS -gt 0) {
        return [int]$env:HODEX_RETRY_ATTEMPTS
    }
    return 3
}

function Get-RetryDelaySeconds {
    if ($env:HODEX_RETRY_DELAY_SECONDS -match '^\d+$' -and [int]$env:HODEX_RETRY_DELAY_SECONDS -ge 0) {
        return [int]$env:HODEX_RETRY_DELAY_SECONDS
    }
    return 2
}

function Get-RetryDelayStepSeconds {
    if ($env:HODEX_RETRY_DELAY_STEP_SECONDS -match '^\d+$' -and [int]$env:HODEX_RETRY_DELAY_STEP_SECONDS -ge 0) {
        return [int]$env:HODEX_RETRY_DELAY_STEP_SECONDS
    }
    return 2
}

function Get-RetryErrorSummary {
    param([string]$ErrorText)

    if ([string]::IsNullOrWhiteSpace($ErrorText)) {
        return ""
    }

    $normalized = ($ErrorText -replace '\s+', ' ').Trim()
    if ($normalized.Length -le 240) {
        return $normalized
    }
    return $normalized.Substring(0, 240)
}

function Test-RetryableError {
    param(
        [string]$Label,
        [string]$ErrorText,
        [int]$ExitCode = 1
    )

    if ([string]::IsNullOrWhiteSpace($ErrorText)) {
        return $false
    }

    switch ($Label) {
        "github-api" { return $ErrorText -match 'timed? out|SSL|TLS|Could not resolve host|Name or service not known|Failed to connect|Connection reset|5\d\d|temporarily unavailable' }
        "release-download" { return $ErrorText -match 'timed? out|SSL|TLS|Could not resolve host|Failed to connect|Connection reset|5\d\d|temporarily unavailable' }
        "controller-download" { return $ErrorText -match 'timed? out|SSL|TLS|Could not resolve host|Failed to connect|Connection reset|5\d\d|temporarily unavailable' }
        "git-clone" { return $ErrorText -match 'Could not resolve host|Failed to connect|Connection timed out|Connection reset|TLS|SSL|RPC failed|remote end hung up unexpectedly|unable to access' }
        "git-fetch" { return $ErrorText -match 'Could not resolve host|Failed to connect|Connection timed out|Connection reset|TLS|SSL|RPC failed|remote end hung up unexpectedly|unable to access' }
        "cargo-metadata" { return $ErrorText -match 'spurious network error|failed to download|index\.crates\.io|SSL connect error|network error|timed? out|Connection reset|failed to get' }
        "cargo-build" { return $ErrorText -match 'spurious network error|failed to download|index\.crates\.io|SSL connect error|network error|timed? out|Connection reset|failed to get' }
        "cargo-install" { return $ErrorText -match 'spurious network error|failed to download|index\.crates\.io|SSL connect error|network error|timed? out|Connection reset|failed to get' }
        default { return $false }
    }
}

function Invoke-WithRetry {
    param(
        [string]$Label,
        [scriptblock]$ScriptBlock
    )

    $maxAttempts = Get-RetryAttempts
    $delaySeconds = Get-RetryDelaySeconds
    $delayStep = Get-RetryDelayStepSeconds

    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        try {
            return & $ScriptBlock
        } catch {
            $message = ($_ | Out-String)
            if ($attempt -ge $maxAttempts -or -not (Test-RetryableError -Label $Label -ErrorText $message)) {
                throw
            }
            Write-WarnLine "$Label failed; retrying in ${delaySeconds}s ($attempt/$maxAttempts): $(Get-RetryErrorSummary -ErrorText $message)"
            Start-Sleep -Seconds $delaySeconds
            $delaySeconds += $delayStep
        }
    }
}

function Invoke-NativeCommandWithRetry {
    param(
        [string]$Label,
        [string]$FilePath,
        [string[]]$ArgumentList,
        [switch]$CaptureOutput
    )

    return Invoke-WithRetry -Label $Label -ScriptBlock {
        if ($CaptureOutput) {
            $stdoutFile = [System.IO.Path]::GetTempFileName()
            $stderrFile = [System.IO.Path]::GetTempFileName()
            try {
                & $FilePath @ArgumentList 1> $stdoutFile 2> $stderrFile
                $exitCode = $LASTEXITCODE
                $stdout = if (Test-Path -LiteralPath $stdoutFile) { Get-Content -LiteralPath $stdoutFile -Raw -ErrorAction SilentlyContinue } else { "" }
                $stderr = if (Test-Path -LiteralPath $stderrFile) { Get-Content -LiteralPath $stderrFile -Raw -ErrorAction SilentlyContinue } else { "" }

                if ($exitCode -ne 0) {
                    throw "EXITCODE=$exitCode`n$stderr`n$stdout".Trim()
                }

                return $stdout
            } finally {
                Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
                Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
            }
        }

        $outputLines = [System.Collections.Generic.List[string]]::new()
        & $FilePath @ArgumentList 2>&1 | ForEach-Object {
            $line = [string]$_
            $outputLines.Add($line)
            Write-Host $line
        }
        $exitCode = $LASTEXITCODE

        if ($exitCode -ne 0) {
            throw ("EXITCODE={0}`n{1}" -f $exitCode, (($outputLines -join [Environment]::NewLine).Trim()))
        }

        return $null
    }
}

function Invoke-WebRequestWithRetry {
    param(
        [string]$Label,
        [string]$Uri,
        [string]$OutFile
    )

    Invoke-WithRetry -Label $Label -ScriptBlock {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile
        return $null
    }
}

function Format-ByteSize {
    param([double]$Bytes)

    if ($Bytes -lt 1024) { return ("{0:N0} B" -f $Bytes) }
    if ($Bytes -lt 1MB) { return ("{0:N1} KB" -f ($Bytes / 1KB)) }
    if ($Bytes -lt 1GB) { return ("{0:N1} MB" -f ($Bytes / 1MB)) }
    return ("{0:N1} GB" -f ($Bytes / 1GB))
}

function Invoke-DownloadWithProgress {
    param(
        [string]$Label,
        [string]$Uri,
        [string]$OutFile
    )

    Invoke-WithRetry -Label "release-download" -ScriptBlock {
        if (-not ("System.Net.Http.HttpClient" -as [type])) {
            try {
                Add-Type -AssemblyName System.Net.Http
            } catch {
                Fail "Failed to load System.Net.Http; cannot download release assets."
            }
        }
        $client = [System.Net.Http.HttpClient]::new()
        $progressEnabled = [Environment]::UserInteractive -and -not [Console]::IsOutputRedirected
        $previousProgressPreference = $global:ProgressPreference

        try {
            if ($progressEnabled) {
                $global:ProgressPreference = "Continue"
            }

            $response = $client.GetAsync($Uri, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
            [void]$response.EnsureSuccessStatusCode()

            $totalBytes = $response.Content.Headers.ContentLength
            $contentStream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $fileStream = [System.IO.File]::Open($OutFile, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
            $buffer = New-Object byte[] 65536
            $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
            $bytesReadTotal = [int64]0
            $lastUpdateMs = 0

            try {
                while (($bytesRead = $contentStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $fileStream.Write($buffer, 0, $bytesRead)
                    $bytesReadTotal += $bytesRead

                    if ($progressEnabled -and (($stopwatch.ElapsedMilliseconds - $lastUpdateMs) -ge 200)) {
                        $lastUpdateMs = $stopwatch.ElapsedMilliseconds
                        $seconds = [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.1)
                        $speedText = (Format-ByteSize ($bytesReadTotal / $seconds)) + "/s"
                        if ($totalBytes) {
                            $percent = [Math]::Min([int](($bytesReadTotal * 100) / $totalBytes), 100)
                            $status = "{0}% | {1} / {2} | Speed {3}" -f $percent, (Format-ByteSize $bytesReadTotal), (Format-ByteSize $totalBytes), $speedText
                            Write-Progress -Activity $Label -Status $status -PercentComplete $percent
                        } else {
                            $status = "{0} | Speed {1}" -f (Format-ByteSize $bytesReadTotal), $speedText
                            Write-Progress -Activity $Label -Status $status -PercentComplete -1
                        }
                    }
                }
            } finally {
                $fileStream.Dispose()
                $contentStream.Dispose()
            }

            if ($progressEnabled) {
                Write-Progress -Activity $Label -Completed
            }

            $seconds = [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.1)
            return [pscustomobject]@{
                bytes = $bytesReadTotal
                total = $totalBytes
                speed = ($bytesReadTotal / $seconds)
            }
        } finally {
            $client.Dispose()
            if ($progressEnabled) {
                $global:ProgressPreference = $previousProgressPreference
            }
        }
    }
}

function Invoke-HeadRequestWithRetry {
    param(
        [string]$Label,
        [string]$Uri
    )

    Invoke-WithRetry -Label $Label -ScriptBlock {
        Invoke-WebRequest -Method Head -Uri $Uri | Out-Null
        return $null
    }
}

function Normalize-PathEntry {
    param([string]$Entry)

    if ([string]::IsNullOrWhiteSpace($Entry)) {
        return ""
    }

    return $Entry.Trim().TrimEnd("\", "/").ToLowerInvariant()
}

function Split-PathList {
    param([string]$PathValue)

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return @()
    }

    return @(
        $PathValue.Split(";", [System.StringSplitOptions]::RemoveEmptyEntries) |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ }
    )
}

function Test-PathContains {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    $needle = Normalize-PathEntry $Entry
    foreach ($segment in (Split-PathList $PathValue)) {
        if ((Normalize-PathEntry $segment) -eq $needle) {
            return $true
        }
    }

    return $false
}

function Remove-PathEntry {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    $needle = Normalize-PathEntry $Entry
    $segments = foreach ($segment in (Split-PathList $PathValue)) {
        if ((Normalize-PathEntry $segment) -ne $needle) {
            $segment
        }
    }

    return ($segments -join ";")
}

function Replace-PathEntry {
    param(
        [string]$PathValue,
        [string]$OldEntry,
        [string]$NewEntry
    )

    $oldNeedle = Normalize-PathEntry $OldEntry
    $result = New-Object System.Collections.Generic.List[string]
    $replaced = $false

    foreach ($segment in (Split-PathList $PathValue)) {
        if ((Normalize-PathEntry $segment) -eq $oldNeedle) {
            if (-not $replaced) {
                $result.Add($NewEntry)
                $replaced = $true
            }
            continue
        }
        $result.Add($segment)
    }

    if (-not $replaced -and -not (Test-PathContains -PathValue ($result -join ";") -Entry $NewEntry)) {
        $result.Insert(0, $NewEntry)
    }

    return ($result -join ";")
}

function Add-PathEntry {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    if (Test-PathContains -PathValue $PathValue -Entry $Entry) {
        return $PathValue
    }

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return $Entry
    }

    return "$Entry;$PathValue"
}

function Ensure-LocalToolPaths {
    $candidates = @(
        (Join-Path $HOME ".cargo\bin"),
        (Join-Path $HOME ".local\bin")
    )

    foreach ($candidate in $candidates) {
        if (-not (Test-Path -LiteralPath $candidate)) {
            continue
        }
        if (-not (Test-PathContains -PathValue $env:Path -Entry $candidate)) {
            $env:Path = Add-PathEntry -PathValue $env:Path -Entry $candidate
        }
    }
}

function Ensure-DirWritable {
    param([string]$Path)

    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    $probe = Join-Path $Path ".hodex-write-test-$PID.tmp"
    try {
        Set-Content -Path $probe -Value "ok" -Encoding ASCII
    } catch {
        Fail "Directory not writable: $Path"
    } finally {
        Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
    }
}

function Normalize-Parameters {
    $validCommands = @("install", "upgrade", "download", "downgrade", "source", "uninstall", "status", "list", "relink", "repair", "help", "manager-install")

    if ($script:InvokedWithNoArgs) {
        Show-Usage
        exit 0
    }

    if ($List) {
        $script:RequestedCommand = "list"
    }

    if ($script:RequestedCommand -notin $validCommands) {
        if ($PSBoundParameters.ContainsKey("Version")) {
            Fail "Unexpected extra arg: $script:RequestedVersion"
        }
        $script:RequestedVersion = $script:RequestedCommand
        $script:RequestedCommand = "install"
    }

    switch ($script:RequestedCommand) {
        "help" {
            Show-Usage
            exit 0
        }
        "source" {
            if ($script:RequestedVersion -in @("install", "update", "rebuild", "switch", "status", "uninstall", "list", "help")) {
                $script:SourceAction = $script:RequestedVersion
            } else {
                $script:SourceAction = "list"
            }

            if ($script:SourceAction -notin @("install", "update", "rebuild", "switch", "status", "uninstall", "list", "help")) {
                Fail "source supports only install|update|switch|status|uninstall|list|help; rebuild alias was removed and now only shows a hint."
            }

            if ($script:SourceAction -eq "help") {
                Show-SourceUsage
                exit 0
            }
        }
        "downgrade" {
            if (-not $PSBoundParameters.ContainsKey("Version") -or (Normalize-Version $script:RequestedVersion) -eq "latest") {
                Fail "downgrade requires an explicit version"
            }
        }
        "uninstall" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "uninstall does not accept a version argument"
            }
        }
        "status" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "status does not accept a version argument"
            }
        }
        "list" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "list does not accept a version argument"
            }
        }
        "relink" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "relink does not accept a version argument"
            }
        }
        "repair" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "repair does not accept a version argument"
            }
        }
        "manager-install" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "manager-install does not accept a version argument"
            }
        }
    }

    if ($Help) {
        switch ($script:RequestedCommand) {
            "source" {
                Show-SourceUsage
                exit 0
            }
            "list" {
                Show-ListUsage
                exit 0
            }
            default {
                Show-Usage
                exit 0
            }
        }
    }

    $script:StateRoot = Normalize-UserPath $script:StateRoot
    $script:DefaultCommandDir = Join-Path $script:StateRoot "commands"
    $script:DownloadRoot = Normalize-UserPath $script:DownloadRoot
    if ($script:ExplicitCommandDir) {
        $script:CurrentCommandDir = Normalize-UserPath $script:CurrentCommandDir
    }
}

function Detect-Platform {
    if ($env:OS -ne "Windows_NT") {
        if ($script:RawSourceHelpRequest -or ($script:RequestedCommand -eq "source" -and $script:SourceAction -eq "help")) {
            Show-SourceUsage
            exit 0
        }
        Fail "This script supports Windows only; use hodexctl.sh on macOS/Linux/WSL."
    }

    if (-not [Environment]::Is64BitOperatingSystem) {
        Fail "Only 64-bit Windows is supported."
    }

    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    switch ($arch) {
        "Arm64" {
            $script:ArchitectureName = "aarch64"
            $script:PlatformLabel = "Windows (ARM64)"
            $script:AssetCandidates = @(
                "codex-aarch64-pc-windows-msvc.exe.zip",
                "codex-aarch64-pc-windows-msvc.exe",
                "codex-x86_64-pc-windows-msvc.exe.zip",
                "codex-x86_64-pc-windows-msvc.exe"
            )
        }
        "X64" {
            $script:ArchitectureName = "x86_64"
            $script:PlatformLabel = "Windows (x64)"
            $script:AssetCandidates = @(
                "codex-x86_64-pc-windows-msvc.exe.zip",
                "codex-x86_64-pc-windows-msvc.exe"
            )
        }
        default {
            Fail "Unsupported Windows architecture: $arch"
        }
    }
}

function Get-ObjectPropertyNames {
    param([object]$InputObject)

    if ($null -eq $InputObject) {
        return @()
    }

    if ($InputObject -is [System.Collections.IDictionary]) {
        return @($InputObject.Keys | ForEach-Object { [string]$_ })
    }

    if ($InputObject -is [System.Array]) {
        $nonNullItems = @($InputObject | Where-Object { $null -ne $_ })
        if ($nonNullItems.Count -eq 1) {
            return @(Get-ObjectPropertyNames -InputObject $nonNullItems[0])
        }
        return @()
    }

    if ($null -eq $InputObject.PSObject -or $null -eq $InputObject.PSObject.Properties) {
        return @()
    }

    return @(
        $InputObject.PSObject.Properties |
            Where-Object { $null -ne $_ -and $null -ne $_.Name } |
            ForEach-Object { [string]$_.Name }
    )
}

function Get-StateFilePath {
    return Join-Path $script:StateRoot "state.json"
}

function Ensure-StateShape {
    param([object]$State)

    if ($null -eq $State) {
        $State = [pscustomobject]@{}
    }

    if ($State -is [System.Array]) {
        $nonNullItems = @($State | Where-Object { $null -ne $_ })
        if ($nonNullItems.Count -eq 0) {
            $State = [pscustomobject]@{}
        } elseif ($nonNullItems.Count -eq 1) {
            $State = $nonNullItems[0]
        } else {
            $State = $nonNullItems[-1]
        }
    }

    $statePropertyNames = @(Get-ObjectPropertyNames -InputObject $State)

    if ($statePropertyNames -notcontains "schema_version") {
        $State | Add-Member -NotePropertyName schema_version -NotePropertyValue 2 -Force
        $statePropertyNames = @(Get-ObjectPropertyNames -InputObject $State)
    }

    if ($statePropertyNames -notcontains "source_profiles" -or $null -eq $State.source_profiles) {
        $State | Add-Member -NotePropertyName source_profiles -NotePropertyValue ([ordered]@{}) -Force
        $statePropertyNames = @(Get-ObjectPropertyNames -InputObject $State)
    }

    if ($statePropertyNames -notcontains "active_runtime_aliases" -or $null -eq $State.active_runtime_aliases) {
        $State | Add-Member -NotePropertyName active_runtime_aliases -NotePropertyValue ([ordered]@{}) -Force
    }

    foreach ($field in @("repo", "installed_version", "release_tag", "release_name", "asset_name", "binary_path", "command_dir", "path_update_mode", "path_profile", "path_detected_source", "node_setup_choice", "installed_at")) {
        if (@(Get-ObjectPropertyNames -InputObject $State) -notcontains $field) {
            $State | Add-Member -NotePropertyName $field -NotePropertyValue "" -Force
        }
    }
    if (@(Get-ObjectPropertyNames -InputObject $State) -notcontains "path_managed_by_hodexctl") {
        $State | Add-Member -NotePropertyName path_managed_by_hodexctl -NotePropertyValue $false -Force
    }
    if (@(Get-ObjectPropertyNames -InputObject $State) -notcontains "wrappers_created" -or $null -eq $State.wrappers_created) {
        $State | Add-Member -NotePropertyName wrappers_created -NotePropertyValue @() -Force
    }

    if (
        -not [string]::IsNullOrWhiteSpace([string]$State.binary_path) -and
        (
            ($State.active_runtime_aliases -is [System.Collections.IDictionary] -and -not $State.active_runtime_aliases.Contains("hodex")) -or
            (@(Get-ObjectPropertyNames -InputObject $State.active_runtime_aliases) -notcontains "hodex")
        )
    ) {
        if ($State.active_runtime_aliases -is [System.Collections.IDictionary]) {
            $State.active_runtime_aliases["hodex"] = "release"
        } else {
            $State.active_runtime_aliases | Add-Member -NotePropertyName hodex -NotePropertyValue "release" -Force
        }
    }

    if (
        (
            ($State.active_runtime_aliases -is [System.Collections.IDictionary] -and $State.active_runtime_aliases.Contains("hodex") -and [string]$State.active_runtime_aliases["hodex"] -ne "release") -or
            (@(Get-ObjectPropertyNames -InputObject $State.active_runtime_aliases) -contains "hodex" -and [string]$State.active_runtime_aliases.hodex -ne "release")
        )
    ) {
        if (-not [string]::IsNullOrWhiteSpace([string]$State.binary_path)) {
            if ($State.active_runtime_aliases -is [System.Collections.IDictionary]) {
                $State.active_runtime_aliases["hodex"] = "release"
            } else {
                $State.active_runtime_aliases | Add-Member -NotePropertyName hodex -NotePropertyValue "release" -Force
            }
        } else {
            if ($State.active_runtime_aliases -is [System.Collections.IDictionary]) {
                $State.active_runtime_aliases.Remove("hodex")
            } else {
                $State.active_runtime_aliases.PSObject.Properties.Remove("hodex")
            }
        }
    }

    if ($State.active_runtime_aliases -is [System.Collections.IDictionary]) {
        if ($State.active_runtime_aliases.Contains("hodex_stable")) {
            $State.active_runtime_aliases.Remove("hodex_stable")
        }
    } elseif (@(Get-ObjectPropertyNames -InputObject $State.active_runtime_aliases) -contains "hodex_stable") {
        $State.active_runtime_aliases.PSObject.Properties.Remove("hodex_stable")
    }

    if (@(Get-ObjectPropertyNames -InputObject $State) -notcontains "controller_path" -or [string]::IsNullOrWhiteSpace([string]$State.controller_path)) {
        $State | Add-Member -NotePropertyName controller_path -NotePropertyValue (Join-Path $script:StateRoot "libexec\hodexctl.ps1") -Force
    }

    $profiles = $State.source_profiles
    if ($profiles -is [System.Collections.IDictionary]) {
        foreach ($profileName in @($profiles.Keys)) {
            $profile = $profiles[$profileName]
            if ($null -eq $profile) {
                continue
            }
            if (@(Get-ObjectPropertyNames -InputObject $profile) -notcontains "last_synced_at" -or [string]::IsNullOrWhiteSpace([string]$profile.last_synced_at)) {
                $legacyLastBuiltAt = if (@(Get-ObjectPropertyNames -InputObject $profile) -contains "last_built_at") { [string]$profile.last_built_at } else { "" }
                $profile | Add-Member -NotePropertyName last_synced_at -NotePropertyValue $legacyLastBuiltAt -Force
            }
        }
    } else {
        foreach ($property in $profiles.PSObject.Properties) {
            $profile = $property.Value
            if ($null -eq $profile) {
                continue
            }
            if (@(Get-ObjectPropertyNames -InputObject $profile) -notcontains "last_synced_at" -or [string]::IsNullOrWhiteSpace([string]$profile.last_synced_at)) {
                $legacyLastBuiltAt = if (@(Get-ObjectPropertyNames -InputObject $profile) -contains "last_built_at") { [string]$profile.last_built_at } else { "" }
                $profile | Add-Member -NotePropertyName last_synced_at -NotePropertyValue $legacyLastBuiltAt -Force
            }
        }
    }

    return $State
}

function Save-State {
    param([object]$State)

    $stateFile = Get-StateFilePath
    New-Item -ItemType Directory -Path (Split-Path -Parent $stateFile) -Force | Out-Null
    (Ensure-StateShape $State) | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $stateFile -Encoding UTF8
}

function Load-State {
    $stateFile = Get-StateFilePath
    if (-not (Test-Path -LiteralPath $stateFile)) {
        return $null
    }

    $state = Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
    if ($null -eq $state) {
        return $null
    }

    return Ensure-StateShape $state
}

function Write-State {
    param(
        [string]$InstalledVersion,
        [string]$ReleaseTag,
        [string]$ReleaseName,
        [string]$AssetName,
        [string]$BinaryPath,
        [string]$ControllerPath,
        [string]$CurrentCommandDir,
        [string[]]$WrappersCreated,
        [string]$CurrentPathUpdateMode,
        [string]$CurrentPathProfile,
        [string]$CurrentNodeSetupChoice,
        [string]$InstalledAt
    )

    $state = Ensure-StateShape $script:State
    $payload = [ordered]@{
        schema_version    = 2
        repo              = $script:RepoName
        installed_version = $InstalledVersion
        release_tag       = $ReleaseTag
        release_name      = $ReleaseName
        asset_name        = $AssetName
        binary_path       = $BinaryPath
        controller_path   = $ControllerPath
        command_dir       = $CurrentCommandDir
        wrappers_created  = $WrappersCreated
        path_update_mode  = $CurrentPathUpdateMode
        path_profile      = $CurrentPathProfile
        path_managed_by_hodexctl = $script:PathManagedByHodexctl
        path_detected_source = $script:PathDetectedSource
        node_setup_choice = $CurrentNodeSetupChoice
        installed_at      = $InstalledAt
        source_profiles   = [ordered]@{}
        active_runtime_aliases = [ordered]@{}
    }

    if ($state.source_profiles) {
        $payload.source_profiles = $state.source_profiles
    }
    if ($state.active_runtime_aliases) {
        if ($state.active_runtime_aliases -is [System.Collections.IDictionary]) {
            $payload.active_runtime_aliases = $state.active_runtime_aliases
        } else {
            $aliases = [ordered]@{}
            foreach ($property in $state.active_runtime_aliases.PSObject.Properties) {
                $aliases[$property.Name] = $property.Value
            }
            $payload.active_runtime_aliases = $aliases
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($BinaryPath)) {
        $payload.active_runtime_aliases["hodex"] = "release"
    } elseif ($payload.active_runtime_aliases.Contains("hodex")) {
        $payload.active_runtime_aliases.Remove("hodex")
    }
    if ($payload.active_runtime_aliases.Contains("hodex_stable")) {
        $payload.active_runtime_aliases.Remove("hodex_stable")
    }

    $script:State = [pscustomobject]$payload
    Save-State -State $script:State
}

function Get-SourceProfiles {
    $state = Ensure-StateShape $script:State
    $profiles = $state.source_profiles
    if ($profiles -is [System.Collections.IDictionary]) {
        return $profiles
    }

    $result = [ordered]@{}
    foreach ($property in $profiles.PSObject.Properties) {
        $result[$property.Name] = $property.Value
    }
    return $result
}

function Get-SourceProfile {
    param([string]$ProfileName)

    $profiles = Get-SourceProfiles
    if ($profiles.Contains($ProfileName)) {
        return $profiles[$ProfileName]
    }

    return $null
}

function Get-ActiveHodexAlias {
    $state = Ensure-StateShape $script:State
    if ($state.active_runtime_aliases -is [System.Collections.IDictionary] -and $state.active_runtime_aliases.Contains("hodex")) {
        return [string]$state.active_runtime_aliases["hodex"]
    }
    if (@(Get-ObjectPropertyNames -InputObject $state.active_runtime_aliases) -contains "hodex") {
        return [string]$state.active_runtime_aliases.hodex
    }
    return ""
}

function Set-SourceProfile {
    param(
        [string]$ProfileName,
        [hashtable]$ProfileData,
        [string]$ActivationMode = "preserve"
    )

    $state = Ensure-StateShape $script:State
    $profiles = Get-SourceProfiles
    $existing = $profiles[$ProfileName]

    $ProfileData["activated_as_hodex"] = $false
    $profiles[$ProfileName] = [pscustomobject]$ProfileData
    $state.source_profiles = $profiles

    $aliases = [ordered]@{}
    if ($state.active_runtime_aliases -is [System.Collections.IDictionary]) {
        foreach ($key in $state.active_runtime_aliases.Keys) {
            $aliases[$key] = $state.active_runtime_aliases[$key]
        }
    } else {
        foreach ($property in $state.active_runtime_aliases.PSObject.Properties) {
            $aliases[$property.Name] = $property.Value
        }
    }

    $releaseInstalled = -not [string]::IsNullOrWhiteSpace([string]$state.binary_path)
    if ($releaseInstalled) {
        $aliases["hodex"] = "release"
    } else {
        $aliases.Remove("hodex")
    }
    $aliases.Remove("hodex_stable")

    $state.active_runtime_aliases = $aliases
    $script:State = $state
    Save-State -State $script:State
}

function Remove-SourceProfile {
    param([string]$ProfileName)

    $state = Ensure-StateShape $script:State
    $profiles = Get-SourceProfiles
    if ($profiles.Contains($ProfileName)) {
        $profiles.Remove($ProfileName)
    }
    $state.source_profiles = $profiles

    $aliases = [ordered]@{}
    if ($state.active_runtime_aliases -is [System.Collections.IDictionary]) {
        foreach ($key in $state.active_runtime_aliases.Keys) {
            $aliases[$key] = $state.active_runtime_aliases[$key]
        }
    } else {
        foreach ($property in $state.active_runtime_aliases.PSObject.Properties) {
            $aliases[$property.Name] = $property.Value
        }
    }

    if (-not [string]::IsNullOrWhiteSpace([string]$state.binary_path)) {
        $aliases["hodex"] = "release"
    } else {
        $aliases.Remove("hodex")
    }
    $aliases.Remove("hodex_stable")

    $state.active_runtime_aliases = $aliases
    $script:State = $state
    Save-State -State $script:State
}

function Clear-ReleaseState {
    $state = Ensure-StateShape $script:State

    foreach ($name in @("repo", "installed_version", "release_tag", "release_name", "asset_name", "binary_path", "node_setup_choice", "installed_at")) {
        $state | Add-Member -NotePropertyName $name -NotePropertyValue "" -Force
    }
    $state | Add-Member -NotePropertyName wrappers_created -NotePropertyValue @() -Force

    $aliases = [ordered]@{}
    if ($state.active_runtime_aliases -is [System.Collections.IDictionary]) {
        foreach ($key in $state.active_runtime_aliases.Keys) {
            $aliases[$key] = $state.active_runtime_aliases[$key]
        }
    } else {
        foreach ($property in $state.active_runtime_aliases.PSObject.Properties) {
            $aliases[$property.Name] = $property.Value
        }
    }

    if ([string]$aliases["hodex"] -eq "release") {
        $aliases.Remove("hodex")
    }
    $aliases.Remove("hodex_stable")
    $state.active_runtime_aliases = $aliases
    $script:State = $state
    Save-State -State $script:State
}

function Select-CommandDir {
    if (-not [string]::IsNullOrWhiteSpace($script:CurrentCommandDir)) {
        Ensure-DirWritable $script:CurrentCommandDir
        return
    }

    if ($script:State -and -not [string]::IsNullOrWhiteSpace($script:State.command_dir)) {
        $script:CurrentCommandDir = [string]$script:State.command_dir
        Ensure-DirWritable $script:CurrentCommandDir
        return
    }

    if ($Yes) {
        $script:CurrentCommandDir = $script:DefaultCommandDir
        Ensure-DirWritable $script:CurrentCommandDir
        return
    }

    while ($true) {
        Write-Host "Select command directory for hodex / hodexctl:"
        Write-Host "  1. $script:DefaultCommandDir"
        Write-Host "  2. $(Join-Path $script:StateRoot 'bin')"
        Write-Host "  3. Custom directory"

        $choice = Read-Host "Enter choice [1/2/3]"
        switch ($choice) {
            "1" {
                $script:CurrentCommandDir = $script:DefaultCommandDir
                break
            }
            "2" {
                $script:CurrentCommandDir = Join-Path $script:StateRoot "bin"
                break
            }
            "3" {
                $customDir = Read-Host "Enter install directory"
                if ([string]::IsNullOrWhiteSpace($customDir)) {
                    Write-WarnLine "Directory cannot be empty."
                    continue
                }
                $script:CurrentCommandDir = Normalize-UserPath $customDir
                break
            }
            default {
                Write-WarnLine "Please enter 1, 2, or 3."
            }
        }
    }

    Ensure-DirWritable $script:CurrentCommandDir
}

function Update-PathIfNeeded {
    $script:PathUpdateMode = "skipped"
    $script:PathProfile = ""
    $script:PathManagedByHodexctl = $false
    $script:PathDetectedSource = ""

    if ($NoPathUpdate) {
        $script:PathUpdateMode = "disabled"
        $script:PathDetectedSource = "disabled"
        return
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $currentPath = $env:Path

    if (
        $script:State -and
        [string]$script:State.path_update_mode -eq "added" -and
        -not [string]::IsNullOrWhiteSpace([string]$script:State.command_dir) -and
        [string]$script:State.command_dir -ne $script:CurrentCommandDir
    ) {
        $newUserPath = Replace-PathEntry -PathValue $userPath -OldEntry ([string]$script:State.command_dir) -NewEntry $script:CurrentCommandDir
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
        $env:Path = Replace-PathEntry -PathValue $currentPath -OldEntry ([string]$script:State.command_dir) -NewEntry $script:CurrentCommandDir
        $script:PathUpdateMode = "configured"
        $script:PathProfile = "User"
        $script:PathManagedByHodexctl = $true
        $script:PathDetectedSource = "managed-user-path"
        return
    }

    if (Test-PathContains -PathValue $userPath -Entry $script:CurrentCommandDir) {
        if (-not (Test-PathContains -PathValue $currentPath -Entry $script:CurrentCommandDir)) {
            $env:Path = Add-PathEntry -PathValue $currentPath -Entry $script:CurrentCommandDir
            $script:PathUpdateMode = "configured"
        } else {
            $script:PathUpdateMode = "already"
        }
        $script:PathProfile = "User"
        $script:PathManagedByHodexctl = $false
        $script:PathDetectedSource = "preexisting-user-path"
        return
    }

    if (Test-PathContains -PathValue $currentPath -Entry $script:CurrentCommandDir) {
        $script:PathDetectedSource = "current-process-only"
    }

    $shouldUpdate = $true
    if (-not $Yes) {
        if ($script:PathDetectedSource -eq "current-process-only") {
            $answer = Read-Host "Command dir $script:CurrentCommandDir is only in the current session PATH. Write to user PATH? [Y/n]"
        } else {
            $answer = Read-Host "Command dir $script:CurrentCommandDir is not in PATH. Write to user PATH? [Y/n]"
        }
        if ($answer -match "^(n|N|no|NO)$") {
            $shouldUpdate = $false
        }
    }

    if (-not $shouldUpdate) {
        $script:PathUpdateMode = "user-skipped"
        if ([string]::IsNullOrWhiteSpace($script:PathDetectedSource)) {
            $script:PathDetectedSource = "user-skipped"
        }
        return
    }

    $newUserPath = Add-PathEntry -PathValue $userPath -Entry $script:CurrentCommandDir
    [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    $env:Path = Add-PathEntry -PathValue $currentPath -Entry $script:CurrentCommandDir
    $script:PathUpdateMode = "added"
    $script:PathProfile = "User"
    $script:PathManagedByHodexctl = $true
    $script:PathDetectedSource = "managed-user-path"
}

function Persist-StateRuntimeMetadata {
    if (-not $script:State) {
        return
    }

    $script:State.command_dir = $script:CurrentCommandDir
    $script:State.path_update_mode = $script:PathUpdateMode
    $script:State.path_profile = $script:PathProfile
    $script:State.path_managed_by_hodexctl = $script:PathManagedByHodexctl
    $script:State.path_detected_source = $script:PathDetectedSource
    Save-State -State $script:State
}

function Remove-PathIfNeeded {
    param(
        [string]$CurrentCommandDir,
        [string]$CurrentPathUpdateMode
    )

    $managedByHodexctl = $false
    if ($script:State -and $script:State.path_managed_by_hodexctl) {
        $managedByHodexctl = $true
    } elseif (
        $script:State -and
        [string]::IsNullOrWhiteSpace([string]$script:State.path_detected_source) -and
        $CurrentPathUpdateMode -in @("added", "configured")
    ) {
        # Compatibility for legacy state.json: infer managed PATH from old fields when path_managed_by_hodexctl is missing.
        $managedByHodexctl = $true
    }

    if (-not $managedByHodexctl) {
        return
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    [Environment]::SetEnvironmentVariable("Path", (Remove-PathEntry -PathValue $userPath -Entry $CurrentCommandDir), "User")
    $env:Path = Remove-PathEntry -PathValue $env:Path -Entry $CurrentCommandDir
}

function Get-ReleaseDownloadRoot {
    if (-not [string]::IsNullOrWhiteSpace($script:ReleaseBaseUrl)) {
        return $script:ReleaseBaseUrl
    }

    return "https://github.com/$script:RepoName/releases"
}

function New-DirectReleaseDescriptor {
    param(
        [string]$ReleaseTag,
        [string]$ReleaseName,
        [string]$HtmlUrl,
        [string]$AssetName,
        [string]$AssetUrl
    )

    return [pscustomobject]@{
        tag_name     = $ReleaseTag
        name         = $ReleaseName
        published_at = ""
        html_url     = $HtmlUrl
        body         = ""
        assets       = @(
            [pscustomobject]@{
                name                 = $AssetName
                browser_download_url = $AssetUrl
                digest               = ""
            }
        )
    }
}

function Resolve-ReleaseDirect {
    param([string]$RequestedVersion)

    $root = Get-ReleaseDownloadRoot
    $normalized = Normalize-Version $RequestedVersion

    if ($normalized -eq "latest") {
        foreach ($candidate in $script:AssetCandidates) {
            $assetUrl = "$root/latest/download/$candidate"
            try {
                [void](Invoke-HeadRequestWithRetry -Label "release-download" -Uri $assetUrl)
                return New-DirectReleaseDescriptor -ReleaseTag "latest" -ReleaseName "latest" -HtmlUrl "$root/latest" -AssetName $candidate -AssetUrl $assetUrl
            } catch {
            }
        }
    } else {
        $tags = @($RequestedVersion, $normalized, "v$normalized", "rust-v$normalized") |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            Select-Object -Unique

        foreach ($tag in $tags) {
            foreach ($candidate in $script:AssetCandidates) {
                $assetUrl = "$root/download/$tag/$candidate"
                try {
                    [void](Invoke-HeadRequestWithRetry -Label "release-download" -Uri $assetUrl)
                    $htmlUrl = if ([string]::IsNullOrWhiteSpace($script:ReleaseBaseUrl)) {
                        "https://github.com/$script:RepoName/releases/tag/$tag"
                    } else {
                        "$root/download/$tag"
                    }
                    return New-DirectReleaseDescriptor -ReleaseTag $tag -ReleaseName (Normalize-Version $tag) -HtmlUrl $htmlUrl -AssetName $candidate -AssetUrl $assetUrl
                } catch {
                }
            }
        }
    }

    Fail "No asset for version $RequestedVersion on this platform: $($script:AssetCandidates -join ', ')"
}

function Resolve-Release {
    param([string]$RequestedVersion)

    if (-not [string]::IsNullOrWhiteSpace($script:ReleaseBaseUrl)) {
        return Resolve-ReleaseDirect -RequestedVersion $RequestedVersion
    }

    if ((Normalize-Version $RequestedVersion) -eq "latest") {
        try {
            return Invoke-GitHubApi -Uri "https://api.github.com/repos/$script:RepoName/releases/latest"
        } catch {
            Fail (Get-GitHubApiFailureMessage -BaseMessage "Failed to fetch latest release; check repo name, GitHub API rate limits, or network.")
        }
    }

    try {
        $releases = @(Invoke-GitHubApi -Uri "https://api.github.com/repos/$script:RepoName/releases?per_page=100")
    } catch {
        Fail (Get-GitHubApiFailureMessage -BaseMessage "Failed to fetch release list; check repo name, GitHub API rate limits, or network.")
    }
    $normalized = Normalize-Version $RequestedVersion
    $candidates = @($RequestedVersion, $normalized, "v$normalized", "rust-v$normalized") |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Unique

    foreach ($release in $releases) {
        if ($candidates -contains [string]$release.tag_name -or $candidates -contains [string]$release.name) {
            return $release
        }
    }

    foreach ($release in $releases) {
        if ([string]$release.name -eq $normalized) {
            return $release
        }
    }

    Fail "Release not found for version $RequestedVersion."
}

function Get-AllReleases {
    $releases = New-Object System.Collections.Generic.List[object]
    $page = 1

    while ($true) {
        try {
            $pageReleases = @(Invoke-GitHubApi -Uri "https://api.github.com/repos/$script:RepoName/releases?per_page=100&page=$page")
        } catch {
            Fail (Get-GitHubApiFailureMessage -BaseMessage "Failed to fetch release list; check repo name, GitHub API rate limits, or network.")
        }
        if ($pageReleases.Count -eq 0) {
            break
        }

        foreach ($release in $pageReleases) {
            [void]$releases.Add($release)
        }

        if ($pageReleases.Count -lt 100) {
            break
        }

        $page += 1
    }

    return $releases.ToArray()
}

function Get-AssetInfo {
    param([object]$Release)

    foreach ($candidate in $script:AssetCandidates) {
        foreach ($asset in @($Release.assets)) {
                if ([string]$asset.name -eq $candidate) {
                    if ($script:ArchitectureName -eq "aarch64" -and [string]$asset.name -like "*x86_64*") {
                    Write-WarnLine "Current release does not provide a native Windows ARM64 asset; falling back to x64 (requires Windows ARM x64 emulation)."
                }
                return $asset
            }
        }
    }

    Fail "Release has no matching asset for this platform: $($script:AssetCandidates -join ', ')"
}

function Get-MatchingReleases {
    $items = New-Object System.Collections.Generic.List[object]

    foreach ($release in @(Get-AllReleases)) {
        $matchedAsset = $null
        foreach ($candidate in $script:AssetCandidates) {
            foreach ($asset in @($release.assets)) {
                if ([string]$asset.name -eq $candidate) {
                    $matchedAsset = $asset
                    break
                }
            }
            if ($null -ne $matchedAsset) {
                break
            }
        }

        if ($null -eq $matchedAsset) {
            continue
        }

        $version = Normalize-Version $(if ([string]::IsNullOrWhiteSpace([string]$release.tag_name)) { [string]$release.name } else { [string]$release.tag_name })
        [void]$items.Add([pscustomobject]@{
            version      = $version
            release      = $release
            asset        = $matchedAsset
            release_tag  = [string]$release.tag_name
            release_name = [string]$release.name
            published_at = [string]$release.published_at
            html_url     = [string]$release.html_url
        })
    }

    return $items.ToArray()
}

function Verify-DigestIfPresent {
    param(
        [string]$DownloadedFile,
        [string]$Digest
    )

    if ([string]::IsNullOrWhiteSpace($Digest)) {
        Write-WarnLine "Release did not provide a digest; skipping SHA-256 verification."
        return
    }

    if (-not $Digest.StartsWith("sha256:")) {
        Write-WarnLine "Unsupported digest format: $Digest"
        return
    }

    $expected = $Digest.Substring(7)
    $actual = (Get-FileHash -LiteralPath $DownloadedFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) {
        Fail "SHA-256 verification failed. Expected $expected, got $actual"
    }

    Write-Step "SHA-256 verified: $actual"
}

function Get-ReleaseDetailsText {
    param(
        [object]$ReleaseInfo
    )

    $release = $ReleaseInfo.release
    $asset = $ReleaseInfo.asset
    $body = [string]$release.body
    if ([string]::IsNullOrWhiteSpace($body)) {
        $body = "<No changelog provided for this release>"
    }

    return @"
Version: $([string]$ReleaseInfo.version)
Release: $([string]$ReleaseInfo.release_name) ($([string]$ReleaseInfo.release_tag))
Published at: $([string]$ReleaseInfo.published_at)
Asset (current platform): $([string]$asset.name)
Page: $([string]$ReleaseInfo.html_url)

Changelog:
$body
"@
}

function Get-ReleaseSummaryPrompt {
    param([object]$ReleaseInfo)

    $release = $ReleaseInfo.release
    $body = [string]$release.body
    if ([string]::IsNullOrWhiteSpace($body)) {
        $body = "<No changelog provided for this release>"
    }

    return @"
Please summarize the full changelog below for this Hodex release.

Requirements:
1. Output only the final summary; do not include analysis, drafts, self-references, or extra preface.
2. Prefer structured categories when possible, recommended order:
   - New Features
   - Improvements
   - Fixes
   - Breaking Changes / Migration
   - Other Notes
3. Omit empty categories; do not invent content just to fill categories.
4. Use short bullet points under each category, prioritizing user-relevant changes.
5. If there are breaking changes, compatibility impact, config changes, or manual steps, call them out explicitly.
6. Do not omit important information; do not fabricate anything not in the changelog.
7. Start directly with the summary (no "Summary:" preface).

Version: $([string]$ReleaseInfo.version)
Release: $([string]$ReleaseInfo.release_name) ($([string]$ReleaseInfo.release_tag))
Published at: $([string]$ReleaseInfo.published_at)
Page: $([string]$ReleaseInfo.html_url)

Full changelog:
$body
"@
}

function Get-ReleaseSummaryAgentCandidates {
    $candidates = [System.Collections.Generic.List[object]]::new()

    foreach ($name in @("hodex", "codex")) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command) {
            $alreadyAdded = $false
            foreach ($existing in $candidates) {
                if ([string]$existing.Source -eq [string]$command.Source) {
                    $alreadyAdded = $true
                    break
                }
            }
            if (-not $alreadyAdded) {
                $candidates.Add($command)
            }
        }
    }

    return @($candidates)
}

function Test-ReleaseSummaryExecSupport {
    param([object]$CommandInfo)

    try {
        & $CommandInfo.Source exec --help *> $null
        return $LASTEXITCODE -eq 0
    } catch {
        return $false
    }
}

function Pause-AfterReleaseSummary {
    if ([Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected) {
        [void](Read-Host "Press Enter to return to release details")
    }
}

function Clear-ReleaseSummaryScreen {
    if ([Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected) {
        Clear-Host
    }
}

function Write-ReleaseSummaryJsonEvent {
    param(
        [string]$JsonLine,
        [hashtable]$StreamedItems
    )

    if ([string]::IsNullOrWhiteSpace($JsonLine)) {
        return
    }

    try {
        $event = $JsonLine | ConvertFrom-Json -Depth 20
    } catch {
        return
    }

    $eventType = [string]$event.type
    if ($eventType -eq "item.delta") {
        $itemId = [string]$(if ($null -ne $event.item_id) { $event.item_id } elseif ($null -ne $event.item -and $null -ne $event.item.id) { $event.item.id } else { "" })
        $deltaText = ""
        if ($null -ne $event.delta) {
            if ($event.delta -is [string]) {
                $deltaText = [string]$event.delta
            } elseif ($null -ne $event.delta.text) {
                $deltaText = [string]$event.delta.text
            } elseif ($null -ne $event.delta.output_text) {
                $deltaText = [string]$event.delta.output_text
            }
        }
        if (-not [string]::IsNullOrWhiteSpace($deltaText)) {
            Write-Host $deltaText -NoNewline
            if (-not [string]::IsNullOrWhiteSpace($itemId)) {
                $StreamedItems[$itemId] = $true
            }
        }
        return
    }

    if ($eventType -eq "item.completed" -and $null -ne $event.item -and [string]$event.item.type -eq "agent_message") {
        $itemId = [string]$event.item.id
        $text = [string]$event.item.text
        if ([string]::IsNullOrWhiteSpace($text)) {
            return
        }
        if ($StreamedItems.ContainsKey($itemId)) {
            if (-not $text.EndsWith("`n")) {
                Write-Host ""
            }
        } else {
            Write-Host $text
        }
    }
}

function Invoke-ReleaseSummary {
    param([object]$ReleaseInfo)

    $candidates = @(Get-ReleaseSummaryAgentCandidates)
    if ($candidates.Count -eq 0) {
        Clear-ReleaseSummaryScreen
        Write-WarnLine "No available hodex/codex command found; cannot summarize the changelog."
        Pause-AfterReleaseSummary
        return $false
    }

    $promptPath = Join-Path ([System.IO.Path]::GetTempPath()) ("hodex-release-summary-" + [System.Guid]::NewGuid().ToString("N") + ".txt")
    $usedFallback = $false

    try {
        Set-Content -LiteralPath $promptPath -Value (Get-ReleaseSummaryPrompt -ReleaseInfo $ReleaseInfo) -Encoding UTF8

        foreach ($candidate in $candidates) {
            if (-not (Test-ReleaseSummaryExecSupport -CommandInfo $candidate)) {
                $usedFallback = $true
                continue
            }

            Clear-ReleaseSummaryScreen
            if ($usedFallback) {
                Write-WarnLine "Preferred command unavailable; switched to $([string]$candidate.Name)."
                Write-Host ""
            }

            Write-Host "Generating AI summary, please wait..."
            Write-Host ""

            try {
                $stderrPath = Join-Path ([System.IO.Path]::GetTempPath()) ("hodex-release-summary-stderr-" + [System.Guid]::NewGuid().ToString("N") + ".log")
                $streamedItems = @{}
                try {
                    ([System.IO.File]::ReadAllText($promptPath)) | & $candidate.Source exec --skip-git-repo-check --color never --json - 2> $stderrPath | ForEach-Object {
                        Write-ReleaseSummaryJsonEvent -JsonLine ([string]$_) -StreamedItems $streamedItems
                    }
                    $exitCode = $LASTEXITCODE
                } finally {
                    $stderrText = if (Test-Path -LiteralPath $stderrPath) { Get-Content -LiteralPath $stderrPath -Raw -ErrorAction SilentlyContinue } else { "" }
                    Remove-Item -LiteralPath $stderrPath -Force -ErrorAction SilentlyContinue
                }

                if ($exitCode -eq 0) {
                    Pause-AfterReleaseSummary
                    return $true
                }
                if (-not [string]::IsNullOrWhiteSpace($stderrText)) {
                    Write-WarnLine "$([string]$candidate.Name) failed: $([string](Get-RetryErrorSummary -ErrorText $stderrText))"
                }
            } catch {
            }

            $usedFallback = $true
            Write-Host ""
            Write-WarnLine "$([string]$candidate.Name) failed to summarize the changelog; trying the next command."
            Write-Host ""
        }

        Clear-ReleaseSummaryScreen
        Write-WarnLine "None of the available hodex/codex commands could run the changelog summary."
        Pause-AfterReleaseSummary
        return $false
    } finally {
        Remove-Item -LiteralPath $promptPath -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-Download {
    param([string]$RequestedVersion)

    $release = Resolve-Release -RequestedVersion $RequestedVersion
    $asset = Get-AssetInfo -Release $release

    Ensure-DirWritable $script:DownloadRoot
    $outputPath = Join-Path $script:DownloadRoot ([string]$asset.name)

    if ([Environment]::UserInteractive -and (Test-Path -LiteralPath $outputPath) -and -not $Yes) {
        $answer = Read-Host "Target file already exists. Overwrite? [Y/n]"
        if ($answer -match "^(n|N|no|NO)$") {
            Write-Info "Download canceled."
            return
        }
    }

    Write-Step "Download Hodex asset"
    Write-Step "Selected release: $([string]$release.name) ($([string]$release.tag_name))"
    Write-Step "Download asset: $([string]$asset.name)"
    Write-Step "Save path: $outputPath"
    $downloadResult = Invoke-DownloadWithProgress -Label ("Downloading " + [string]$asset.name) -Uri ([string]$asset.browser_download_url) -OutFile $outputPath
    Verify-DigestIfPresent -DownloadedFile $outputPath -Digest ([string]$asset.digest)
    Write-Info ("Download complete: {0}, average speed {1}/s" -f (Format-ByteSize $downloadResult.bytes), (Format-ByteSize $downloadResult.speed))
    Write-Info "Downloaded to: $outputPath"
}

function Sync-ControllerCopy {
    param([string]$TargetPath)

    New-Item -ItemType Directory -Path (Split-Path -Parent $TargetPath) -Force | Out-Null

    if ($PSCommandPath -and (Test-Path -LiteralPath $PSCommandPath)) {
        if (Test-Path -LiteralPath $TargetPath) {
            $sourcePath = [System.IO.Path]::GetFullPath((Get-Item -LiteralPath $PSCommandPath).FullName)
            $targetFullPath = [System.IO.Path]::GetFullPath((Get-Item -LiteralPath $TargetPath).FullName)
            if ([StringComparer]::OrdinalIgnoreCase.Equals($sourcePath, $targetFullPath)) {
                return
            }
        }
        Copy-Item -LiteralPath $PSCommandPath -Destination $TargetPath -Force
        return
    }

    $rawUrl = "$script:ControllerUrlBase/$script:RepoName/main/scripts/hodexctl/hodexctl.ps1"
    Write-Step "Download hodexctl manager script"
    Invoke-WebRequestWithRetry -Label "controller-download" -Uri $rawUrl -OutFile $TargetPath
}

function Get-ControllerCommand {
    if (Test-Command "pwsh") {
        return "pwsh"
    }

    return "powershell"
}

function Generate-HodexCmdWrapper {
    param(
        [string]$WrapperPath,
        [string]$BinaryPath
    )

    $content = @"
@echo off
if not exist "$BinaryPath" (
  echo hodex binary is missing; run hodexctl install first. 1>&2
  exit /b 1
)
"$BinaryPath" %*
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding ASCII
}

function Generate-HodexPs1Wrapper {
    param(
        [string]$WrapperPath,
        [string]$BinaryPath
    )

    $content = @"
`$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath "$BinaryPath")) {
    Write-Error "hodex binary is missing; run hodexctl install first."
    exit 1
}
& "$BinaryPath" @args
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding UTF8
}

function Generate-RuntimeCmdWrapper {
    param(
        [string]$WrapperPath,
        [string]$BinaryPath,
        [string]$CommandName
    )

    $content = @"
@echo off
if not exist "$BinaryPath" (
  echo $CommandName binary is missing; rerun hodexctl install or rebuild. 1>&2
  exit /b 1
)
"$BinaryPath" %*
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding ASCII
}

function Generate-RuntimePs1Wrapper {
    param(
        [string]$WrapperPath,
        [string]$BinaryPath,
        [string]$CommandName
    )

    $content = @"
`$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath "$BinaryPath")) {
    Write-Error "$CommandName binary is missing; rerun hodexctl install or rebuild."
    exit 1
}
& "$BinaryPath" @args
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding UTF8
}

function Generate-HodexctlCmdWrapper {
    param(
        [string]$WrapperPath,
        [string]$ControllerPath,
        [string]$StateDir
    )

    $runner = Get-ControllerCommand
    $content = @"
@echo off
set "HODEX_DISPLAY_NAME=hodexctl"
set "HODEX_STATE_DIR=$StateDir"
if not exist "$ControllerPath" (
  echo hodexctl controller is missing; reinstall hodexctl. 1>&2
  exit /b 1
)
if "%~1"=="" (
  $runner -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" help
  exit /b %ERRORLEVEL%
)
$runner -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" %*
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding ASCII
}

function Generate-HodexctlPs1Wrapper {
    param(
        [string]$WrapperPath,
        [string]$ControllerPath,
        [string]$StateDir
    )

    $runner = Get-ControllerCommand
    $content = @"
`$ErrorActionPreference = "Stop"
`$env:HODEX_DISPLAY_NAME = "hodexctl"
`$env:HODEX_STATE_DIR = "$StateDir"
if (-not (Test-Path -LiteralPath "$ControllerPath")) {
    Write-Error "hodexctl controller is missing; reinstall hodexctl."
    exit 1
}
`$forwardedArgs = @(`$args)
`$hasStateDirOverride = `$false
foreach (`$arg in `$forwardedArgs) {
    if (`$arg -is [string]) {
        `$normalizedArg = ([string]`$arg).Trim()
        if (`$normalizedArg -ieq "-StateDir" -or `$normalizedArg -ieq "--state-dir" -or `$normalizedArg -like "--state-dir=*") {
            `$hasStateDirOverride = `$true
            break
        }
    }
}
if (`$forwardedArgs.Count -eq 0) {
    & "$runner" -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" help
    exit `$LASTEXITCODE
}
if (-not `$hasStateDirOverride) {
    `$forwardedArgs = @("-StateDir", "$StateDir") + `$forwardedArgs
}
& "$runner" -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" @forwardedArgs
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding UTF8
}

function Create-Wrappers {
    param(
        [string]$CurrentCommandDir,
        [string]$BinaryPath,
        [string]$ControllerPath
    )

    Ensure-DirWritable $CurrentCommandDir
    Generate-HodexCmdWrapper -WrapperPath (Join-Path $CurrentCommandDir "hodex.cmd") -BinaryPath $BinaryPath
    Generate-HodexPs1Wrapper -WrapperPath (Join-Path $CurrentCommandDir "hodex.ps1") -BinaryPath $BinaryPath
    Generate-HodexctlCmdWrapper -WrapperPath (Join-Path $CurrentCommandDir "hodexctl.cmd") -ControllerPath $ControllerPath
    Generate-HodexctlPs1Wrapper -WrapperPath (Join-Path $CurrentCommandDir "hodexctl.ps1") -ControllerPath $ControllerPath
}

function Remove-ManagedRuntimeWrappersFromDir {
    param([string]$CommandDir)

    if ([string]::IsNullOrWhiteSpace($CommandDir)) {
        return
    }

    foreach ($name in @("hodex", "hodex-stable", "hodexctl")) {
        Remove-Item -LiteralPath (Join-Path $CommandDir ($name + ".cmd")) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $CommandDir ($name + ".ps1")) -Force -ErrorAction SilentlyContinue
    }

    foreach ($profileName in (Get-SourceProfiles).Keys) {
        Remove-Item -LiteralPath (Join-Path $CommandDir ($profileName + ".cmd")) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $CommandDir ($profileName + ".ps1")) -Force -ErrorAction SilentlyContinue
    }
}

function Sync-RuntimeWrappersFromState {
    param(
        [string]$CommandDir,
        [string]$ControllerPath
    )

    Ensure-DirWritable $CommandDir
    Remove-ManagedRuntimeWrappersFromDir -CommandDir $CommandDir

    $releaseInstalled = -not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path) -and (Test-Path -LiteralPath ([string]$script:State.binary_path))
    $keepControllerWrapper = -not [string]::IsNullOrWhiteSpace($ControllerPath) -and $null -ne $script:State

    if ($keepControllerWrapper -and -not [string]::IsNullOrWhiteSpace($ControllerPath)) {
        Generate-HodexctlCmdWrapper -WrapperPath (Join-Path $CommandDir "hodexctl.cmd") -ControllerPath $ControllerPath -StateDir $script:StateRoot
        Generate-HodexctlPs1Wrapper -WrapperPath (Join-Path $CommandDir "hodexctl.ps1") -ControllerPath $ControllerPath -StateDir $script:StateRoot
    }

    foreach ($profileName in (Get-SourceProfiles).Keys) {
        $profile = Get-SourceProfile -ProfileName $profileName
        if ($null -eq $profile) {
            continue
        }
        $binaryPath = [string]$profile.binary_path
        if ([string]::IsNullOrWhiteSpace($binaryPath) -or -not (Test-Path -LiteralPath $binaryPath)) {
            continue
        }
        Generate-RuntimeCmdWrapper -WrapperPath (Join-Path $CommandDir ($profileName + ".cmd")) -BinaryPath $binaryPath -CommandName $profileName
        Generate-RuntimePs1Wrapper -WrapperPath (Join-Path $CommandDir ($profileName + ".ps1")) -BinaryPath $binaryPath -CommandName $profileName
    }

    if ($releaseInstalled) {
        Generate-HodexCmdWrapper -WrapperPath (Join-Path $CommandDir "hodex.cmd") -BinaryPath ([string]$script:State.binary_path)
        Generate-HodexPs1Wrapper -WrapperPath (Join-Path $CommandDir "hodex.ps1") -BinaryPath ([string]$script:State.binary_path)
    }
}

function Remove-OldWrappersIfNeeded {
    param([string]$NewCommandDir)

    if (-not $script:State) {
        return
    }

    $oldCommandDir = [string]$script:State.command_dir
    if ([string]::IsNullOrWhiteSpace($oldCommandDir) -or $oldCommandDir -eq $NewCommandDir) {
        return
    }

    Remove-ManagedRuntimeWrappersFromDir -CommandDir $oldCommandDir
}

function Install-NodeNative {
    if (-not (Test-Command "winget")) {
        Write-WarnLine "winget not detected; cannot install via system. Use manual install: $script:NodeDownloadUrl"
        return
    }

    Write-Step "Install Node.js LTS with winget"
    & winget install --exact --id OpenJS.NodeJS.LTS --accept-package-agreements --accept-source-agreements
}

function Install-NodeWithNvm {
    if (Test-Command "nvm") {
        Write-Step "Install Node.js LTS with nvm"
        & nvm install lts
        & nvm use lts
        return
    }

    if (-not (Test-Command "winget")) {
        Write-WarnLine "winget not detected; cannot auto-install nvm-windows. Install manually: $script:NvmWindowsReleaseUrl"
        return
    }

    Write-Step "Install nvm-windows with winget"
    & winget install --exact --id CoreyButler.NVMforWindows --accept-package-agreements --accept-source-agreements
    Write-WarnLine "nvm-windows installed. After first install, reopen PowerShell and run: nvm install lts"
}

function Prompt-NodeChoice {
    param([string]$PreviousChoice)

    if (Test-Command "node") {
        $script:NodeSetupChoice = "already-installed"
        return
    }

    if ($script:SelectedNodeMode -eq "ask" -and -not [string]::IsNullOrWhiteSpace($PreviousChoice)) {
        $script:NodeSetupChoice = $PreviousChoice
        Write-Info "Node.js not installed; reuse previous choice: $PreviousChoice"
        return
    }

    if ($script:SelectedNodeMode -eq "ask" -and $Yes) {
        $script:NodeSetupChoice = "skip"
        Write-WarnLine "Node.js not installed; non-interactive mode defaults to skip."
        return
    }

    $effectiveMode = $script:SelectedNodeMode
    if ($effectiveMode -eq "ask") {
        Write-Host "Node.js not detected; choose an option:"
        Write-Host "  1. Install via system package manager"
        Write-Host "     - Windows: winget"
        Write-Host "  2. Use nvm (nvm-windows on Windows)"
        Write-Host "  3. Manual download/install (official site)"
        Write-Host "  4. Skip"

        while ($true) {
            $answer = Read-Host "Choose [1/2/3/4]"
            switch ($answer) {
                "1" {
                    $effectiveMode = "native"
                    break
                }
                "2" {
                    $effectiveMode = "nvm"
                    break
                }
                "3" {
                    $effectiveMode = "manual"
                    break
                }
                "4" {
                    $effectiveMode = "skip"
                    break
                }
                default {
                    Write-WarnLine "Please enter 1, 2, 3, or 4."
                }
            }
        }
    }

    $script:NodeSetupChoice = $effectiveMode
    switch ($effectiveMode) {
        "skip" {
            Write-Info "Skipped Node.js setup."
        }
        "manual" {
            Write-Info "Please install Node.js manually: $script:NodeDownloadUrl"
        }
        "native" {
            Install-NodeNative
        }
        "nvm" {
            Install-NodeWithNvm
        }
    }
}

function Install-BinaryFromAsset {
    param(
        [object]$Asset,
        [string]$BinaryPath
    )

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("hodex-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    try {
        $assetName = [string]$Asset.name
        $helperDir = Split-Path -Parent $BinaryPath
        $downloadPath = Join-Path $tempDir $assetName
        Write-Step "Temp download path: $downloadPath"
        $downloadResult = Invoke-DownloadWithProgress -Label ("Downloading " + $assetName) -Uri ([string]$Asset.browser_download_url) -OutFile $downloadPath
        Verify-DigestIfPresent -DownloadedFile $downloadPath -Digest ([string]$Asset.digest)
        Write-Info ("Download complete: {0}, average speed {1}/s" -f (Format-ByteSize $downloadResult.bytes), (Format-ByteSize $downloadResult.speed))

        $sourceBinary = $downloadPath
        if ($assetName.ToLowerInvariant().EndsWith(".zip")) {
            $extractDir = Join-Path $tempDir "extract"
            New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
            Expand-Archive -LiteralPath $downloadPath -DestinationPath $extractDir -Force
            $sourceBinary = Join-Path $extractDir ([System.IO.Path]::GetFileNameWithoutExtension($assetName))

            if (-not (Test-Path -LiteralPath $sourceBinary)) {
                Fail "Expected Windows executable not found in release asset."
            }

            foreach ($helperName in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
                $helperSource = Join-Path $extractDir $helperName
                if (-not (Test-Path -LiteralPath $helperSource)) {
                    Fail "Windows release asset missing required helper: $helperName"
                }
                Copy-Item -LiteralPath $helperSource -Destination (Join-Path $helperDir $helperName) -Force
            }
        } else {
            $assetUri = [System.Uri][string]$Asset.browser_download_url
            foreach ($helperName in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
                $helperDownloadPath = Join-Path $tempDir $helperName
                $helperUri = [System.Uri]::new($assetUri, $helperName).AbsoluteUri
                try {
                    $helperResult = Invoke-DownloadWithProgress -Label ("Downloading " + $helperName) -Uri $helperUri -OutFile $helperDownloadPath
                    Write-Info ("Download complete: {0}, average speed {1}/s" -f (Format-ByteSize $helperResult.bytes), (Format-ByteSize $helperResult.speed))
                } catch {
                    Fail "Windows release asset missing required helper: $helperName"
                }
                Copy-Item -LiteralPath $helperDownloadPath -Destination (Join-Path $helperDir $helperName) -Force
            }
        }

        if (-not (Test-Path -LiteralPath $sourceBinary)) {
            Fail "Expected Windows executable not found in release asset."
        }

        Copy-Item -LiteralPath $sourceBinary -Destination $BinaryPath -Force
    } finally {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Get-ReleaseHelperPaths {
    param([string]$BinaryPath)

    $binaryDir = Split-Path -Parent $BinaryPath
    return @(
        (Join-Path $binaryDir "codex-command-runner.exe"),
        (Join-Path $binaryDir "codex-windows-sandbox-setup.exe")
    )
}

function Test-ReleaseHelpersComplete {
    param([string]$BinaryPath)

    foreach ($helperPath in @(Get-ReleaseHelperPaths -BinaryPath $BinaryPath)) {
        if (-not (Test-Path -LiteralPath $helperPath)) {
            return $false
        }
    }
    return $true
}

function Get-InstalledBinaryVersion {
    param([string]$BinaryPath)

    if (-not (Test-Path -LiteralPath $BinaryPath)) {
        return ""
    }

    try {
        $firstLine = (& $BinaryPath --version 2>$null | Select-Object -First 1)
        if ($null -eq $firstLine) {
            return ""
        }

        $match = [regex]::Match([string]$firstLine, '[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?')
        if ($match.Success) {
            return $match.Value
        }
    } catch {
    }

    return ""
}

function Confirm-YesNo {
    param(
        [string]$Prompt,
        [bool]$DefaultYes = $true
    )

    if ($Yes) {
        return $DefaultYes
    }

    $suffix = if ($DefaultYes) { "[Y/n]" } else { "[y/N]" }
    Write-Host ("Awaiting confirmation; press Enter to accept the default {0}." -f $(if ($DefaultYes) { "Y" } else { "N" }))
    $answer = Read-Host "$Prompt $suffix"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        return $DefaultYes
    }
    return $answer -match "^(y|Y|yes|YES)$"
}

function Validate-SourceProfileName {
    param([string]$ProfileName)

    if ([string]::IsNullOrWhiteSpace($ProfileName)) {
        Fail "Source profile name cannot be empty."
    }
    if ($ProfileName -notmatch '^[A-Za-z0-9._-]+$') {
        Fail "Source profile name allows only letters, numbers, dots, underscores, and hyphens."
    }
    if ($ProfileName -in @("hodex", "hodexctl", "hodex-stable")) {
        Fail "Source profile name cannot use a reserved name: $ProfileName"
    }
}

function Resolve-SourceRepoInput {
    param([string]$ProfileName)

    if (-not [string]::IsNullOrWhiteSpace($script:SourceGitUrl)) {
        return [pscustomobject]@{ repo_input = $script:SourceGitUrl; remote_url = $script:SourceGitUrl }
    }

    if ($script:ExplicitSourceRepo -and -not [string]::IsNullOrWhiteSpace($script:RepoName)) {
        return [pscustomobject]@{ repo_input = $script:RepoName; remote_url = "https://github.com/$($script:RepoName).git" }
    }

    $existing = Get-SourceProfile -ProfileName $ProfileName
    if ($existing) {
        return [pscustomobject]@{
            repo_input = [string]$existing.repo_input
            remote_url = [string]$existing.remote_url
        }
    }

    if ($Yes) {
        return [pscustomobject]@{ repo_input = "stellarlinkco/codex"; remote_url = "https://github.com/stellarlinkco/codex.git" }
    }

    $answer = Read-Host "Enter source repo (owner/repo or Git URL, default stellarlinkco/codex)"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        $answer = "stellarlinkco/codex"
    }
    if ($answer -match '://|^git@') {
        return [pscustomobject]@{ repo_input = $answer; remote_url = $answer }
    }
    return [pscustomobject]@{ repo_input = $answer; remote_url = "https://github.com/$answer.git" }
}

function Parse-SourceRemoteIdentity {
    param([string]$RemoteInput)

    $remoteHost = "github.com"
    $pathPart = $RemoteInput
    if ($RemoteInput -match '://') {
        $stripped = ($RemoteInput -replace '^[^:]+://', '') -replace '^[^@]+@', ''
        $remoteHost, $pathPart = $stripped.Split('/', 2)
    } elseif ($RemoteInput -match '^git@') {
        $stripped = ($RemoteInput -replace '^git@', '')
        $remoteHost, $pathPart = $stripped.Split(':', 2)
    }
    $pathPart = $pathPart.TrimStart('/') -replace '\.git$', ''
    return [pscustomobject]@{ host = $remoteHost; path = $pathPart }
}

function Get-DefaultSourceCheckoutDir {
    param([string]$RemoteInput)

    $identity = Parse-SourceRemoteIdentity -RemoteInput $RemoteInput
    return Join-Path (Join-Path $HOME "hodex-src") (Join-Path $identity.host $identity.path)
}

function Get-SourceRemoteUrlFromInput {
    param([string]$RepoInput)

    if ($RepoInput -match '://|^git@') {
        return $RepoInput
    }

    return "https://github.com/$RepoInput.git"
}

function Get-UniqueChoiceItems {
    param([string[]]$Values)

    $items = [System.Collections.Generic.List[string]]::new()
    foreach ($value in $Values) {
        if ([string]::IsNullOrWhiteSpace($value)) {
            continue
        }
        $exists = $false
        foreach ($existing in $items) {
            if ($existing -eq $value) {
                $exists = $true
                break
            }
        }
        if (-not $exists) {
            $items.Add($value)
        }
    }
    return @($items)
}

function Show-ChoiceItems {
    param([string[]]$Items)

    if ($Items.Count -eq 0) {
        return
    }

    Write-Host "  Choices:"
    for ($i = 0; $i -lt $Items.Count; $i++) {
        Write-Host ("    {0}. {1}" -f ($i + 1), $Items[$i])
    }
}

function Get-FilteredChoiceItems {
    param(
        [string[]]$Items,
        [string]$Query = ""
    )

    if ([string]::IsNullOrWhiteSpace($Query)) {
        return @($Items)
    }

    $filtered = [System.Collections.Generic.List[string]]::new()
    foreach ($item in $Items) {
        if ($item -like ("*" + $Query + "*")) {
            $filtered.Add($item)
        }
    }
    return @($filtered)
}

function Read-ValueWithChoices {
    param(
        [string]$Label,
        [string]$DefaultValue,
        [string[]]$ChoiceItems = @(),
        [string]$Note = ""
    )

    $ChoiceItems = @($ChoiceItems)
    if ($ChoiceItems.Count -gt 12 -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected) {
        $pageSize = 10
        $pageStart = 0
        $query = ""

        while ($true) {
            $filteredItems = @(Get-FilteredChoiceItems -Items $ChoiceItems -Query $query)
            if ($pageStart -lt 0) {
                $pageStart = 0
            }
            if ($filteredItems.Count -gt 0 -and $pageStart -ge $filteredItems.Count) {
                $pageStart = [Math]::Floor(($filteredItems.Count - 1) / $pageSize) * $pageSize
            }

            Clear-Host
            Write-Host $Label
            Write-Host ("  Default: " + $DefaultValue)
            if (-not [string]::IsNullOrWhiteSpace($Note)) {
                Write-Host ("  Note: " + $Note)
            }
            Write-Host "  Enter a number on this page to select, or type a custom value"
            Write-Host "  n/p to page, / to filter, c to clear filter"
            if (-not [string]::IsNullOrWhiteSpace($query)) {
                Write-Host ("  Current filter: " + $query)
            }

            if ($filteredItems.Count -eq 0) {
                Write-Host "  No matches for current filter"
            } else {
                $pageEnd = [Math]::Min($pageStart + $pageSize, $filteredItems.Count)
                $pageCount = [Math]::Ceiling($filteredItems.Count / [double]$pageSize)
                $pageNumber = [Math]::Floor($pageStart / $pageSize) + 1
                Write-Host ("  Choices: page {0}/{1}, {2} items" -f $pageNumber, $pageCount, $filteredItems.Count)
                for ($i = $pageStart; $i -lt $pageEnd; $i++) {
                    Write-Host ("    {0}. {1}" -f ($i - $pageStart + 1), $filteredItems[$i])
                }
            }

            $answer = Read-Host ">"
            if ([string]::IsNullOrWhiteSpace($answer)) {
                return $DefaultValue
            }

            switch ($answer.ToLowerInvariant()) {
                "n" {
                    if ($filteredItems.Count -gt 0) {
                        $pageStart += $pageSize
                        if ($pageStart -ge $filteredItems.Count) {
                            $pageStart = [Math]::Floor(($filteredItems.Count - 1) / $pageSize) * $pageSize
                        }
                    }
                    continue
                }
                "p" {
                    $pageStart -= $pageSize
                    if ($pageStart -lt 0) {
                        $pageStart = 0
                    }
                    continue
                }
                "c" {
                    $query = ""
                    $pageStart = 0
                    continue
                }
            }

            if ($answer.StartsWith("/")) {
                $query = $answer.Substring(1)
                $pageStart = 0
                continue
            }

            $index = 0
            if ([int]::TryParse($answer, [ref]$index) -and $filteredItems.Count -gt 0) {
                $visibleCount = [Math]::Min($pageSize, $filteredItems.Count - $pageStart)
                if ($index -ge 1 -and $index -le $visibleCount) {
                    return [string]$filteredItems[$pageStart + $index - 1]
                }
                Write-WarnLine "Please enter a number within the current page."
                continue
            }

            return $answer
        }
    }

    while ($true) {
        Write-Host $Label
        Write-Host ("  Default: " + $DefaultValue)
        if (-not [string]::IsNullOrWhiteSpace($Note)) {
            Write-Host ("  Note: " + $Note)
        }
        Show-ChoiceItems -Items $ChoiceItems
        Write-Host "  Enter a number to select, or type a custom value"

        $answer = Read-Host ">"
        if ([string]::IsNullOrWhiteSpace($answer)) {
            return $DefaultValue
        }

        $index = 0
        if ([int]::TryParse($answer, [ref]$index) -and $ChoiceItems.Count -gt 0) {
            if ($index -ge 1 -and $index -le $ChoiceItems.Count) {
                return [string]$ChoiceItems[$index - 1]
            }
            Write-WarnLine "Number out of range, please retry."
            continue
        }

        return $answer
    }
}

function Get-SourceRepoCandidates {
    $items = [System.Collections.Generic.List[string]]::new()
    $items.Add("stellarlinkco/codex")
    $items.Add("https://github.com/stellarlinkco/codex.git")

    foreach ($profileName in (Get-SourceProfiles).Keys) {
        $profile = Get-SourceProfile -ProfileName $profileName
        if ($null -eq $profile) {
            continue
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$profile.repo_input)) {
            $items.Add([string]$profile.repo_input)
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$profile.remote_url) -and [string]$profile.remote_url -ne ("https://github.com/" + [string]$profile.repo_input + ".git")) {
            $items.Add([string]$profile.remote_url)
        }
    }

    return Get-UniqueChoiceItems -Values @($items)
}

function Get-SourceProfileSuggestion {
    param([string]$RepoInput)

    $remoteUrl = Get-SourceRemoteUrlFromInput -RepoInput $RepoInput
    $identity = Parse-SourceRemoteIdentity -RemoteInput $remoteUrl
    $repoName = [System.IO.Path]::GetFileName([string]$identity.path)
    $candidate = (($repoName + "-source").ToLowerInvariant() -replace '[^a-z0-9._-]', '-') -replace '-{2,}', '-'
    return $candidate.Trim('-')
}

function Get-SourceProfileCandidates {
    param([string]$RepoInput)

    $items = [System.Collections.Generic.List[string]]::new()
    $items.Add($script:DefaultSourceProfileName)
    $suggested = Get-SourceProfileSuggestion -RepoInput $RepoInput
    if (-not [string]::IsNullOrWhiteSpace($suggested)) {
        $items.Add($suggested)
    }
    foreach ($profileName in (Get-SourceProfiles).Keys) {
        $items.Add([string]$profileName)
    }
    return Get-UniqueChoiceItems -Values @($items)
}

function Get-GitCheckoutRefCandidates {
    param([string]$CheckoutDir)

    if ([string]::IsNullOrWhiteSpace($CheckoutDir) -or -not (Test-Path -LiteralPath (Join-Path $CheckoutDir ".git"))) {
        return @()
    }

    $items = [System.Collections.Generic.List[string]]::new()
    try {
        & git -C $CheckoutDir fetch --all --tags --prune --force *> $null
        $refs = (& git -C $CheckoutDir for-each-ref --format="%(refname:short)" refs/heads refs/remotes/origin 2>$null | ForEach-Object { [string]$_ })
        foreach ($ref in $refs) {
            if ([string]::IsNullOrWhiteSpace($ref) -or $ref -eq "origin/HEAD" -or $ref -eq "origin") {
                continue
            }
            if ($ref.StartsWith("origin/")) {
                $items.Add($ref.Substring(7))
            } else {
                $items.Add($ref)
            }
        }
    } catch {
        return @()
    }

    return Get-UniqueChoiceItems -Values @($items)
}

function Get-SourceRefCandidates {
    param(
        [string]$RepoInput = "",
        [string]$ProfileName = "",
        [string]$DefaultRef = "",
        [string]$CheckoutDir = ""
    )

    $items = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($DefaultRef)) {
        $items.Add($DefaultRef)
    }
    $items.Add($script:DefaultSourceRef)
    foreach ($name in @("master", "develop", "dev")) {
        $items.Add($name)
    }
    foreach ($gitRef in (Get-GitCheckoutRefCandidates -CheckoutDir $CheckoutDir)) {
        $items.Add([string]$gitRef)
    }

    foreach ($profileNameItem in (Get-SourceProfiles).Keys) {
        $profile = Get-SourceProfile -ProfileName $profileNameItem
        if ($null -eq $profile -or [string]::IsNullOrWhiteSpace([string]$profile.current_ref)) {
            continue
        }
        if (-not [string]::IsNullOrWhiteSpace($ProfileName) -and [string]$profileNameItem -eq $ProfileName) {
            $items.Add([string]$profile.current_ref)
        } elseif (-not [string]::IsNullOrWhiteSpace($RepoInput) -and [string]$profile.repo_input -eq $RepoInput) {
            $items.Add([string]$profile.current_ref)
        }
    }

    return Get-UniqueChoiceItems -Values @($items)
}

function Get-SourceCheckoutCandidates {
    param(
        [string]$RemoteUrl,
        [string]$DefaultCheckoutDir
    )

    $items = [System.Collections.Generic.List[string]]::new()
    $items.Add($DefaultCheckoutDir)
    $items.Add((Join-Path $HOME "hodex-src"))

    foreach ($profileName in (Get-SourceProfiles).Keys) {
        $profile = Get-SourceProfile -ProfileName $profileName
        if ($null -eq $profile -or [string]::IsNullOrWhiteSpace([string]$profile.checkout_dir)) {
            continue
        }
        if ([string]$profile.remote_url -eq $RemoteUrl) {
            $items.Add([string]$profile.checkout_dir)
        }
    }

    return Get-UniqueChoiceItems -Values @($items)
}

function Read-SourceRefWithChoices {
    param(
        [string]$RepoInput = "",
        [string]$ProfileName = "",
        [string]$DefaultRef = "",
        [string]$CheckoutDir = ""
    )

    if ([string]::IsNullOrWhiteSpace($DefaultRef)) {
        $DefaultRef = $script:DefaultSourceRef
    }

    return Read-ValueWithChoices -Label "Target ref (branch / tag / commit)" -DefaultValue $DefaultRef -ChoiceItems (Get-SourceRefCandidates -RepoInput $RepoInput -ProfileName $ProfileName -DefaultRef $DefaultRef -CheckoutDir $CheckoutDir) -Note "Candidates default to branches; tags or commits can be typed directly"
}

function Run-SourceInstallWizard {
    if (-not [Environment]::UserInteractive -or $Yes) {
        return $true
    }

    if (
        -not [string]::IsNullOrWhiteSpace($script:SourceGitUrl) -or
        -not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir) -or
        -not [string]::IsNullOrWhiteSpace($Profile) -or
        $script:SourceRef -ne "main" -or
        $script:RepoName -ne "stellarlinkco/codex"
    ) {
        return $true
    }

    while ($true) {
        Clear-Host
        Write-Host "Source download wizard"
        Write-Host ""
        Write-Host "We will confirm repo, source profile, ref, and checkout dir in order."
        Write-Host "Press Enter to accept defaults; source mode only downloads/syncs and does not build."
        Write-Host ""

        $repoInput = Read-ValueWithChoices -Label "Source repo (owner/repo or Git URL)" -DefaultValue "stellarlinkco/codex" -ChoiceItems (Get-SourceRepoCandidates)

        while ($true) {
            $profileName = Read-ValueWithChoices -Label "Source profile name" -DefaultValue "codex-source" -ChoiceItems (Get-SourceProfileCandidates -RepoInput $repoInput) -Note "This is a source profile/workspace id, not a command name"
            try {
                Validate-SourceProfileName -ProfileName $profileName
                break
            } catch {
                Write-WarnLine "Source profile name cannot use a reserved name."
            }
        }

        $remoteUrl = Get-SourceRemoteUrlFromInput -RepoInput $repoInput
        $defaultCheckoutDir = Get-DefaultSourceCheckoutDir -RemoteInput $remoteUrl
        $refName = Read-ValueWithChoices -Label "Source ref (branch / tag / commit)" -DefaultValue "main" -ChoiceItems (Get-SourceRefCandidates -RepoInput $repoInput -ProfileName $profileName -DefaultRef "main" -CheckoutDir $defaultCheckoutDir) -Note "Candidates default to branches; tags or commits can be typed directly"
        Write-Host ""
        Write-Host "Step 4/4: checkout"
        Write-Host "By default, checkout is placed under the managed source directory for update/switch reuse."
        $checkoutDir = Normalize-UserPath (Read-ValueWithChoices -Label "Source checkout dir" -DefaultValue $defaultCheckoutDir -ChoiceItems (Get-SourceCheckoutCandidates -RemoteUrl $remoteUrl -DefaultCheckoutDir $defaultCheckoutDir))

        Write-Host ""
        Write-Host "Wizard summary"
        Write-Host ("  Repo: " + $repoInput)
        Write-Host ("  Source profile: " + $profileName)
        Write-Host ("  ref: " + $refName)
        Write-Host ("  checkout: " + $checkoutDir)

        Write-Host ""
        Write-Host "Entering confirmation step."
        if (Confirm-YesNo -Prompt "Continue with the above configuration?" -DefaultYes $true) {
            if ($repoInput -match '://|^git@') {
                $script:SourceGitUrl = $repoInput
            } else {
                $script:RepoName = $repoInput
                $script:ExplicitSourceRepo = $true
            }
            $script:SourceProfile = $profileName
            $script:SourceRef = $refName
            $script:SourceCheckoutDir = $checkoutDir
            return $true
        }

        if (-not (Confirm-YesNo -Prompt "Restart the source download wizard?" -DefaultYes $true)) {
            Write-Info "Canceled."
            return $false
        }
    }
}

function Resolve-SourceCheckoutDir {
    param(
        [string]$DefaultDir,
        [string]$ProfileName
    )

    if (-not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir)) {
        return Normalize-UserPath $script:SourceCheckoutDir
    }

    $existing = Get-SourceProfile -ProfileName $ProfileName
    if ($existing -and -not [string]::IsNullOrWhiteSpace([string]$existing.checkout_dir)) {
        return [string]$existing.checkout_dir
    }

    if ($Yes) {
        return $DefaultDir
    }

    $answer = Read-Host "Source checkout dir [$DefaultDir]"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        return $DefaultDir
    }
    return Normalize-UserPath $answer
}

function Resolve-SourceProfileName {
    param([bool]$RequireExisting)

    if ($RequireExisting) {
        if ($script:ExplicitSourceProfile -and -not [string]::IsNullOrWhiteSpace($script:SourceProfile)) {
            return $script:SourceProfile
        }
        if (Get-SourceProfile -ProfileName $script:DefaultSourceProfileName) {
            return $script:DefaultSourceProfileName
        }
        $profiles = @((Get-SourceProfiles).Keys)
        if ($profiles.Count -eq 1) {
            return [string]$profiles[0]
        }
        if ($Yes) {
            return [string]$profiles[0]
        }
        if ($Host.Name -and $Host.UI.RawUI) {
            $cursor = 0
            $preferredIndex = [Array]::IndexOf($profiles, $script:DefaultSourceProfileName)
            if ($preferredIndex -ge 0) {
                $cursor = $preferredIndex
            }
            while ($true) {
                Clear-Host
                Write-Host "Select source profile"
                Write-Host ""
                Write-Host ("Total {0} profiles; default focus is {1}, otherwise the first item." -f $profiles.Count, $script:DefaultSourceProfileName)
                Write-Host "Arrow keys move  Enter confirm  q cancel"
                Write-Host ""
                for ($i = 0; $i -lt $profiles.Count; $i++) {
                    $profile = Get-SourceProfile -ProfileName ([string]$profiles[$i])
                    if ($i -eq $cursor) {
                        Write-Host ("> {0} | {1} | {2}" -f [string]$profiles[$i], [string]$profile.current_ref, [string]$profile.repo_input)
                    } else {
                        Write-Host ("  {0} | {1} | {2}" -f [string]$profiles[$i], [string]$profile.current_ref, [string]$profile.repo_input)
                    }
                }
                $selectedProfile = Get-SourceProfile -ProfileName ([string]$profiles[$cursor])
                Write-Host ""
                Write-Host ("Selection: {0} | ref={1} | checkout={2}" -f [string]$profiles[$cursor], [string]$selectedProfile.current_ref, [string]$selectedProfile.checkout_dir)
                $key = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
                switch ($key.VirtualKeyCode) {
                    13 { return [string]$profiles[$cursor] }
                    38 {
                        $cursor -= 1
                        if ($cursor -lt 0) { $cursor = $profiles.Count - 1 }
                    }
                    40 {
                        $cursor += 1
                        if ($cursor -ge $profiles.Count) { $cursor = 0 }
                    }
                    default {
                        if ($key.Character -eq 'q' -or $key.Character -eq 'Q') {
                            Fail "Source profile selection canceled."
                        }
                    }
                }
            }
        }
        while ($true) {
            Write-Host "Select a source profile:"
            for ($i = 0; $i -lt $profiles.Count; $i++) {
                $profile = Get-SourceProfile -ProfileName ([string]$profiles[$i])
                Write-Host ("  {0}. {1} | {2} | {3}" -f ($i + 1), [string]$profiles[$i], [string]$profile.current_ref, [string]$profile.repo_input)
            }
            $choice = Read-Host ("Enter number to select profile [1-" + $profiles.Count + "]")
            $index = 0
            if ([int]::TryParse($choice, [ref]$index) -and $index -ge 1 -and $index -le $profiles.Count) {
                return [string]$profiles[$index - 1]
            }
            Write-WarnLine "Please enter a valid number."
        }
    }

    if ($script:ExplicitSourceProfile -and -not [string]::IsNullOrWhiteSpace($script:SourceProfile)) {
        return $script:SourceProfile
    }
    $defaultSourceProfile = if ([string]::IsNullOrWhiteSpace($script:SourceProfile)) { $script:DefaultSourceProfileName } else { $script:SourceProfile }
    if (-not $Yes) {
        $answer = Read-Host "Source profile name [$defaultSourceProfile]"
        if (-not [string]::IsNullOrWhiteSpace($answer)) {
            return $answer
        }
    }
    return $defaultSourceProfile
}

function Confirm-SourcePlan {
    param(
        [string]$ActionLabel,
        [string]$ProfileName,
        [string]$RepoInput,
        [string]$CheckoutDir,
        [string]$RefName,
        [string]$Hint = "",
        [string]$CheckoutMode = "",
        [string]$CurrentRef = "",
        [string]$CurrentCheckout = ""
    )

    if (-not [Environment]::UserInteractive) {
        return $true
    }

    Write-Host ""
    Write-Host ("About to run: " + $ActionLabel)
    Write-Host ("  Source profile: " + $ProfileName)
    Write-Host ("  Repo: " + $(if ([string]::IsNullOrWhiteSpace($RepoInput)) { "<unknown>" } else { $RepoInput }))
    Write-Host ("  checkout: " + $(if ([string]::IsNullOrWhiteSpace($CheckoutDir)) { "<unknown>" } else { $CheckoutDir }))
    Write-Host ("  ref: " + $(if ([string]::IsNullOrWhiteSpace($RefName)) { "<unknown>" } else { $RefName }))
    if (-not [string]::IsNullOrWhiteSpace($CurrentRef) -and $CurrentRef -ne $RefName) {
        Write-Host ("  Current -> target ref: " + $CurrentRef + " -> " + $RefName)
    }
    if (-not [string]::IsNullOrWhiteSpace($CurrentCheckout) -and $CurrentCheckout -ne $CheckoutDir) {
        Write-Host ("  Current -> target checkout: " + $CurrentCheckout + " -> " + $CheckoutDir)
    }
    if (-not [string]::IsNullOrWhiteSpace($CheckoutMode)) {
        Write-Host ("  Checkout strategy: " + $CheckoutMode)
    }
    if (-not [string]::IsNullOrWhiteSpace($Hint)) {
        Write-Host ("  Note: " + $Hint)
    }
    return (Confirm-YesNo -Prompt "Continue?" -DefaultYes $true)
}

function Get-SourceErrorCategory {
    param([string]$Message)

    switch -Regex ($Message) {
        'toolchain|missing|rustup|cargo|rustc|xcode-clt|msvc-build-tools' { return "Toolchain issue" }
        'git repo|remote|clone|git|checkout|uncommitted' { return "Git / source checkout issue" }
        'ref|branch|tag|commit' { return "Target ref issue" }
        'build|compile|artifact|cargo build' { return "Build issue" }
        'name|reserved|-dev|argument|parameter' { return "Input issue" }
        default { return "Unclassified issue" }
    }
}

function Show-SourceResultSummary {
    param(
        [string]$ActionLabel,
        [string]$ProfileName,
        [string]$RefName = "",
        [string]$CheckoutDir = "",
        [string]$BinaryPath = "",
        [string]$WrapperPath = ""
    )

    Write-Host ""
    Write-Host "Result summary"
        Write-Host ("  Action: " + $ActionLabel)
        Write-Host ("  Source profile: " + $ProfileName)
    if (-not [string]::IsNullOrWhiteSpace($RefName)) {
        Write-Host ("  Current ref: " + $RefName)
    }
    if (-not [string]::IsNullOrWhiteSpace($CheckoutDir)) {
        Write-Host ("  checkout: " + $CheckoutDir)
    }
}

function Show-SourceMenuActionPreview {
    param([string]$Choice)

    $profiles = Get-SourceProfiles
    $profileCount = @($profiles.Keys).Count

    switch ($Choice) {
        "1" {
            $previewName = if ([string]::IsNullOrWhiteSpace($script:SourceProfile)) { $script:DefaultSourceProfileName } else { $script:SourceProfile }
            $previewRepo = if ([string]::IsNullOrWhiteSpace($script:SourceGitUrl)) { $script:RepoName } else { $script:SourceGitUrl }
            $previewRemote = Get-SourceRemoteUrlFromInput -RepoInput $previewRepo
            $previewCheckout = if ([string]::IsNullOrWhiteSpace($script:SourceCheckoutDir)) { Get-DefaultSourceCheckoutDir -RemoteInput $previewRemote } else { Normalize-UserPath $script:SourceCheckoutDir }
            Write-Host ("  Default repo: " + $previewRepo)
            Write-Host ("  Default source profile: " + $previewName)
            Write-Host ("  Default ref: " + $script:SourceRef)
            Write-Host ("  Default checkout: " + $previewCheckout)
            Write-Host "  Action: clone/fetch, toolchain check, register source profile"
        }
        "2" {
            Write-Host "  Default target: auto-select single profile; multiple profiles open selector"
            Write-Host "  Action: fetch latest code, switch back to current ref, sync checkout"
            Write-Host "  Scope: manage source dir and toolchain only; does not affect hodex release"
        }
        "3" {
            Write-Host "  Default target: auto-select single profile; multiple profiles open selector"
            Write-Host "  Action: confirm new branch/tag/commit, then switch and sync source"
            Write-Host "  Safety: refuse switch if checkout has uncommitted changes"
        }
        "4" {
            Write-Host "  Source build has been removed in the current version."
            Write-Host "  Use 'Update source' or 'Switch ref' to get the latest source."
        }
        "5" {
            Write-Host "  Default target: show details for a single profile; multiple profiles show a summary list"
            Write-Host "  Includes: repo, ref, checkout, workspace, last sync time"
        }
        "6" {
            Write-Host "  Default target: auto-select single profile; multiple profiles open selector"
            Write-Host "  Removes: source profile record; optional checkout deletion"
            Write-Host "  Final cleanup: if this is the last runtime, remove hodexctl and managed PATH entries"
        }
        "7" {
            Write-Host ("  Includes: repo/ref/checkout summary for all source profiles")
            Write-Host ("  Currently recorded: " + $profileCount)
        }
    }
}

function Ensure-GitWorktreeClean {
    param([string]$CheckoutDir)

    $status = (& git -C $CheckoutDir status --porcelain --untracked-files=no 2>$null | Out-String).Trim()
    if (-not [string]::IsNullOrWhiteSpace($status)) {
        Fail "Source checkout has uncommitted changes. Commit or clean before switching/updating: $CheckoutDir"
    }
}

function Get-SourceRefKind {
    param(
        [string]$CheckoutDir,
        [string]$RefName
    )

    Invoke-GitFetchWithSummary -CheckoutDir $CheckoutDir
    & git -C $CheckoutDir show-ref --verify --quiet ("refs/remotes/origin/" + $RefName) *> $null
    if ($LASTEXITCODE -eq 0) {
        return "branch"
    }
    & git -C $CheckoutDir show-ref --verify --quiet ("refs/heads/" + $RefName) *> $null
    if ($LASTEXITCODE -eq 0) {
        return "branch"
    }
    & git -C $CheckoutDir show-ref --verify --quiet ("refs/tags/" + $RefName) *> $null
    if ($LASTEXITCODE -eq 0) {
        return "tag"
    }
    & git -C $CheckoutDir rev-parse --verify ($RefName + "^{commit}") *> $null
    if ($LASTEXITCODE -eq 0) {
        return "commit"
    }
    Fail "No matching ref found: $RefName"
}

function Write-GitFetchSummary {
    param([string]$OutputText)

    foreach ($line in ($OutputText -split "`r?`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        if ($line -match '^From ' -or $line -match '\[(new tag|tag update|new branch)\]') {
            Write-Host $line
        }
    }
}

function Invoke-GitFetchWithSummary {
    param([string]$CheckoutDir)

    $outputText = Invoke-NativeCommandWithRetry -Label "git-fetch" -FilePath "git" -ArgumentList @("-C", $CheckoutDir, "fetch", "--all", "--tags", "--prune", "--force") -CaptureOutput
    Write-GitFetchSummary -OutputText $outputText
}

function Write-GitCheckoutSummary {
    param([string]$OutputText)

    foreach ($line in ($OutputText -split "`r?`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        if ($line -match '^(Already on |Switched to branch |Switched to a new branch |Your branch is )') {
            Write-Host $line
        }
    }
}

function Invoke-GitCheckoutWithSummary {
    param(
        [string]$CheckoutDir,
        [string[]]$GitArgs
    )

    $outputText = Invoke-NativeCommandWithRetry -Label "git-checkout" -FilePath "git" -ArgumentList (@("-C", $CheckoutDir) + $GitArgs) -CaptureOutput
    Write-GitCheckoutSummary -OutputText $outputText
}

function Write-GitMergeSummary {
    param([string]$OutputText)

    $rangeLine = ""
    $statusLine = ""
    foreach ($line in ($OutputText -split "`r?`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        if ($line -eq "Already up to date.") {
            Write-Host $line
            return
        }
        if ($line -match '^Updating ') {
            $rangeLine = $line
            continue
        }
        if ($line -match '^[\s]*[0-9]+ files? changed') {
            $statusLine = $line.Trim()
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($rangeLine)) {
        Write-Host $rangeLine
    }
    if (-not [string]::IsNullOrWhiteSpace($statusLine)) {
        Write-Host $statusLine
    }
}

function Invoke-GitMergeWithSummary {
    param(
        [string]$CheckoutDir,
        [string]$TargetRef
    )

    $outputText = Invoke-NativeCommandWithRetry -Label "git-merge" -FilePath "git" -ArgumentList @("-C", $CheckoutDir, "merge", "--ff-only", $TargetRef) -CaptureOutput
    Write-GitMergeSummary -OutputText $outputText
}

function Switch-SourceCheckoutToRef {
    param(
        [string]$CheckoutDir,
        [string]$RefName,
        [string]$RefKind
    )

    switch ($RefKind) {
        "branch" {
            Ensure-GitWorktreeClean -CheckoutDir $CheckoutDir
            & git -C $CheckoutDir show-ref --verify --quiet ("refs/heads/" + $RefName) *> $null
            if ($LASTEXITCODE -eq 0) {
                Invoke-GitCheckoutWithSummary -CheckoutDir $CheckoutDir -GitArgs @("checkout", $RefName)
            } else {
                Invoke-GitCheckoutWithSummary -CheckoutDir $CheckoutDir -GitArgs @("checkout", "-b", $RefName, "--track", ("origin/" + $RefName))
            }
            Invoke-GitMergeWithSummary -CheckoutDir $CheckoutDir -TargetRef ("origin/" + $RefName)
        }
        "tag" {
            Ensure-GitWorktreeClean -CheckoutDir $CheckoutDir
            Invoke-GitCheckoutWithSummary -CheckoutDir $CheckoutDir -GitArgs @("checkout", $RefName)
        }
        "commit" {
            Ensure-GitWorktreeClean -CheckoutDir $CheckoutDir
            Invoke-GitCheckoutWithSummary -CheckoutDir $CheckoutDir -GitArgs @("checkout", $RefName)
        }
    }
}

function Get-SourceWorkspaceRoot {
    param([string]$CheckoutDir)

    $nested = Join-Path $CheckoutDir "codex-rs\Cargo.toml"
    if (Test-Path -LiteralPath $nested) {
        return (Split-Path -Parent $nested)
    }
    $rootCargo = Join-Path $CheckoutDir "Cargo.toml"
    if (Test-Path -LiteralPath $rootCargo) {
        return $CheckoutDir
    }
    Fail "No supported source build entry found (missing codex-rs/Cargo.toml or Cargo.toml)."
}

function Get-SourceBuildStrategy {
    param([string]$WorkspaceRoot)

    $metadataJson = Invoke-NativeCommandWithRetry -Label "cargo-metadata" -FilePath "cargo" -ArgumentList @("metadata", "--format-version", "1", "--no-deps", "--manifest-path", (Join-Path $WorkspaceRoot "Cargo.toml")) -CaptureOutput
    $metadata = $metadataJson | ConvertFrom-Json
    foreach ($package in @($metadata.packages)) {
        if ([string]$package.name -ne "codex-cli") {
            continue
        }
        foreach ($target in @($package.targets)) {
            if ([string]$target.name -eq "codex" -and @($target.kind) -contains "bin") {
                return [pscustomobject]@{ mode = "package"; target = "codex-cli" }
            }
        }
    }
    foreach ($package in @($metadata.packages)) {
        foreach ($target in @($package.targets)) {
            if ([string]$target.name -eq "codex" -and @($target.kind) -contains "bin") {
                return [pscustomobject]@{ mode = "bin"; target = "codex" }
            }
        }
    }
    Fail "No buildable codex CLI entry found in the source repo."
}

function Get-CargoBuildTotalUnits {
    param(
        [object]$Metadata,
        [string]$BuildMode,
        [string]$BuildTarget
    )

    $rootPackage = $null
    foreach ($package in @($Metadata.packages)) {
        if ($BuildMode -eq "package") {
            if ([string]$package.name -eq $BuildTarget) {
                $rootPackage = $package
                break
            }
            continue
        }
        foreach ($target in @($package.targets)) {
            if ([string]$target.name -eq $BuildTarget -and @($target.kind) -contains "bin") {
                $rootPackage = $package
                break
            }
        }
        if ($null -ne $rootPackage) {
            break
        }
    }
    if ($null -eq $rootPackage) {
        Fail "No Cargo package found for the source build target."
    }

    $nodeMap = @{}
    foreach ($node in @($Metadata.resolve.nodes)) {
        $nodeMap[[string]$node.id] = $node
    }

    $reachable = New-Object 'System.Collections.Generic.HashSet[string]'
    $queue = [System.Collections.Generic.Queue[string]]::new()
    $queue.Enqueue([string]$rootPackage.id)
    while ($queue.Count -gt 0) {
        $packageId = $queue.Dequeue()
        if (-not $reachable.Add($packageId)) {
            continue
        }
        $node = $nodeMap[$packageId]
        if ($null -eq $node) {
            continue
        }
        foreach ($dep in @($node.deps)) {
            if ($null -ne $dep.pkg) {
                $queue.Enqueue([string]$dep.pkg)
            }
        }
    }

    $compileKinds = @("bin", "lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "custom-build")
    $compileKindSet = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($kind in $compileKinds) {
        [void]$compileKindSet.Add($kind)
    }

    $total = 0
    foreach ($package in @($Metadata.packages)) {
        if (-not $reachable.Contains([string]$package.id)) {
            continue
        }
        foreach ($target in @($package.targets)) {
            $kinds = @($target.kind)
            $isCompilable = $false
            foreach ($kind in $kinds) {
                if ($compileKindSet.Contains([string]$kind)) {
                    $isCompilable = $true
                    break
                }
            }
            if (-not $isCompilable) {
                continue
            }
            if ($kinds -contains "example" -or $kinds -contains "bench" -or $kinds -contains "test") {
                continue
            }
            $total += 1
        }
    }

    return [Math]::Max($total, 1)
}

function Format-DurationText {
    param([double]$Seconds)

    if ($Seconds -lt 0) {
        return "--:--"
    }
    $totalSeconds = [int][Math]::Floor($Seconds)
    $hours = [int]($totalSeconds / 3600)
    $minutes = [int](($totalSeconds % 3600) / 60)
    $secs = [int]($totalSeconds % 60)
    if ($hours -gt 0) {
        return ("{0}:{1:D2}:{2:D2}" -f $hours, $minutes, $secs)
    }
    return ("{0:D2}:{1:D2}" -f $minutes, $secs)
}

function Invoke-CargoBuildWithProgress {
    param(
        [string]$WorkspaceRoot,
        [string]$BuildMode,
        [string]$BuildTarget
    )

    $manifestPath = Join-Path $WorkspaceRoot "Cargo.toml"
    $progressEnabled = [Environment]::UserInteractive -and -not [Console]::IsOutputRedirected
    if (-not $progressEnabled) {
        switch ($BuildMode) {
            "package" {
                & cargo build --manifest-path $manifestPath -p $BuildTarget --bin codex --release
            }
            "bin" {
                & cargo build --manifest-path $manifestPath --bin $BuildTarget --release
            }
            default {
                Fail "Unknown source build mode: $BuildMode"
            }
        }
        if ($LASTEXITCODE -ne 0) {
            throw "EXITCODE=$LASTEXITCODE"
        }
        return
    }

    $metadataJson = Invoke-NativeCommandWithRetry -Label "cargo-metadata" -FilePath "cargo" -ArgumentList @("metadata", "--format-version", "1", "--manifest-path", $manifestPath) -CaptureOutput
    $metadata = $metadataJson | ConvertFrom-Json
    $totalUnits = Get-CargoBuildTotalUnits -Metadata $metadata -BuildMode $BuildMode -BuildTarget $BuildTarget
    Write-Host ("Estimated compile progress: {0} compilation units" -f $totalUnits)

    $cargoArgs = New-Object System.Collections.Generic.List[string]
    [void]$cargoArgs.Add("build")
    [void]$cargoArgs.Add("--message-format")
    [void]$cargoArgs.Add("json-render-diagnostics")
    [void]$cargoArgs.Add("--manifest-path")
    [void]$cargoArgs.Add($manifestPath)
    [void]$cargoArgs.Add("--release")
    if ($BuildMode -eq "package") {
        [void]$cargoArgs.Add("-p")
        [void]$cargoArgs.Add($BuildTarget)
        [void]$cargoArgs.Add("--bin")
        [void]$cargoArgs.Add("codex")
    } else {
        [void]$cargoArgs.Add("--bin")
        [void]$cargoArgs.Add($BuildTarget)
    }

    $completed = 0
    $freshCount = 0
    $seen = New-Object 'System.Collections.Generic.HashSet[string]'
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    Write-Progress -Activity "Build Hodex from source" -Status "Preparing build graph" -PercentComplete 0

    & cargo @cargoArgs 2>&1 | ForEach-Object {
        $line = [string]$_
        $trimmed = $line.TrimStart()
        if ($trimmed.StartsWith("{")) {
            try {
                $message = $trimmed | ConvertFrom-Json -ErrorAction Stop
                switch ([string]$message.reason) {
                    "compiler-artifact" {
                        $targetKinds = @($message.target.kind)
                        $isCompilable = $false
                        foreach ($kind in $targetKinds) {
                            if (@("bin", "lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "custom-build") -contains [string]$kind) {
                                $isCompilable = $true
                                break
                            }
                        }
                        if ($isCompilable -and -not ($targetKinds -contains "example" -or $targetKinds -contains "bench" -or $targetKinds -contains "test")) {
                            $key = ("{0}|{1}|{2}" -f [string]$message.package_id, [string]$message.target.name, (($targetKinds | ForEach-Object { [string]$_ }) -join ","))
                            if ($seen.Add($key)) {
                                $completed += 1
                                if ($message.fresh) {
                                    $freshCount += 1
                                }
                                $percent = [Math]::Min([int](($completed * 100) / $totalUnits), 99)
                                $elapsed = [Math]::Max($stopwatch.Elapsed.TotalSeconds, 0.1)
                                $remaining = if ($completed -gt 0) { ($elapsed / $completed) * [Math]::Max($totalUnits - $completed, 0) } else { -1 }
                                $status = "{0}/{1} | ETA {2} | fresh={3} | {4}" -f $completed, $totalUnits, (Format-DurationText -Seconds $remaining), $freshCount, ([string]$message.target.name)
                                Write-Progress -Activity "Build Hodex from source" -Status $status -PercentComplete $percent -SecondsRemaining ([int][Math]::Max($remaining, 0))
                            }
                        }
                        return
                    }
                    "compiler-message" {
                        if ($null -ne $message.message -and -not [string]::IsNullOrWhiteSpace([string]$message.message.rendered)) {
                            Write-Host ([string]$message.message.rendered)
                        }
                        return
                    }
                    "build-script-executed" {
                        return
                    }
                    "build-finished" {
                        if ($message.success) {
                            Write-Progress -Activity "Build Hodex from source" -Status "Build finished" -PercentComplete 100 -Completed
                        }
                        return
                    }
                }
            } catch {
            }
        }
        Write-Host $line
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Progress -Activity "Build Hodex from source" -Completed
        throw "EXITCODE=$LASTEXITCODE"
    }

    Write-Progress -Activity "Build Hodex from source" -Status "Build finished" -PercentComplete 100 -Completed
}

function Build-SourceBinary {
    param(
        [string]$WorkspaceRoot,
        [string]$BinaryOutputPath
    )

    $strategy = Get-SourceBuildStrategy -WorkspaceRoot $WorkspaceRoot
    Write-Step "Build Hodex from source"
    Invoke-WithRetry -Label "cargo-build" -ScriptBlock {
        Invoke-CargoBuildWithProgress -WorkspaceRoot $WorkspaceRoot -BuildMode ([string]$strategy.mode) -BuildTarget ([string]$strategy.target)
    }

    $sourceBinary = Join-Path $WorkspaceRoot "target\release\codex.exe"
    if (-not (Test-Path -LiteralPath $sourceBinary)) {
        Fail "Source build finished but expected artifact not found: $sourceBinary"
    }

    Ensure-DirWritable (Split-Path -Parent $BinaryOutputPath)
    Copy-Item -LiteralPath $sourceBinary -Destination $BinaryOutputPath -Force
}

function Detect-SourceToolchain {
    $requiredMissing = New-Object System.Collections.Generic.List[string]
    $optionalMissing = New-Object System.Collections.Generic.List[string]

    foreach ($name in @("git", "rustup", "cargo", "rustc")) {
        if (-not (Test-Command $name)) {
            $requiredMissing.Add($name)
        }
    }
    if (-not (Test-Command "cl")) {
        $optionalMissing.Add("msvc-build-tools")
    }
    if (-not (Test-Command "just")) {
        $optionalMissing.Add("just")
    }
    if (-not (Test-Command "node")) {
        $optionalMissing.Add("node")
    }
    if (-not (Test-Command "npm") -and -not (Test-Command "pnpm")) {
        $optionalMissing.Add("npm")
    }

    return [pscustomobject]@{
        required_missing = @($requiredMissing)
        optional_missing = @($optionalMissing)
    }
}

function Show-SourceToolchainReport {
    param([object]$Report)

    Write-Host "Source toolchain check:"
    foreach ($item in @("git", "rustup", "cargo", "rustc")) {
        $status = if (@($Report.required_missing) -contains $item) { "missing" } else { "installed" }
        Write-Host "  - ${item}: $status"
    }
    foreach ($item in @("msvc-build-tools", "just", "node", "npm")) {
        $status = if (@($Report.optional_missing) -contains $item) { "missing" } else { "installed" }
        Write-Host "  - ${item}: $status"
    }
}

function Install-RustupWithWinget {
    if (-not (Test-Command "winget")) {
        Fail "winget not detected; cannot auto-install rustup."
    }
    & winget install --exact --id Rustlang.Rustup --accept-package-agreements --accept-source-agreements
    Ensure-LocalToolPaths
}

function Auto-InstallSourceToolchain {
    param([object]$Report)

    foreach ($item in @($Report.required_missing) + @($Report.optional_missing)) {
        switch ($item) {
            "git" {
                & winget install --exact --id Git.Git --accept-package-agreements --accept-source-agreements
            }
            "rustup" { Install-RustupWithWinget }
            "cargo" { Install-RustupWithWinget }
            "rustc" { Install-RustupWithWinget }
            "msvc-build-tools" {
                & winget install --exact --id Microsoft.VisualStudio.2022.BuildTools --accept-package-agreements --accept-source-agreements --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools"
            }
            "just" {
                Write-Step "Install just"
                Invoke-NativeCommandWithRetry -Label "cargo-install" -FilePath "cargo" -ArgumentList @("install", "just")
                $cargoBin = Join-Path $HOME ".cargo\bin"
                if (Test-Path -LiteralPath $cargoBin -and -not (Test-PathContains -PathValue $env:Path -Entry $cargoBin)) {
                    $env:Path = Add-PathEntry -PathValue $env:Path -Entry $cargoBin
                }
            }
            "node" {
                Install-NodeNative
            }
            "npm" {
                Install-NodeNative
            }
        }
    }
}

function Ensure-SourceToolchainReady {
    $report = Detect-SourceToolchain
    Show-SourceToolchainReport -Report $report
    if (@($report.required_missing).Count -eq 0) {
        return $report
    }

    if (Confirm-YesNo -Prompt "Auto-install missing tools above?" -DefaultYes $true) {
        Auto-InstallSourceToolchain -Report ([pscustomobject]@{
            required_missing = @($report.required_missing)
            optional_missing = @()
        })
        $report = Detect-SourceToolchain
        Show-SourceToolchainReport -Report $report
    }

    if (@($report.required_missing).Count -gt 0) {
        Fail "Source build toolchain is still incomplete; install missing items and retry."
    }
    return $report
}

function Get-SourceToolchainSnapshotJson {
    param([object]$Report)

    return (@{
        os = "windows"
        arch = $script:ArchitectureName
        required_missing = @($Report.required_missing)
        optional_missing = @($Report.optional_missing)
    } | ConvertTo-Json -Compress -Depth 5)
}

function Prepare-SourceCheckout {
    param(
        [string]$RemoteUrl,
        [string]$CheckoutDir
    )

    if (-not (Test-Path -LiteralPath $CheckoutDir)) {
        Ensure-DirWritable (Split-Path -Parent $CheckoutDir)
        Write-Step "Clone source repo"
        Invoke-NativeCommandWithRetry -Label "git-clone" -FilePath "git" -ArgumentList @("clone", $RemoteUrl, $CheckoutDir)
        return
    }

    if (Test-Path -LiteralPath (Join-Path $CheckoutDir ".git")) {
        $currentRemote = (& git -C $CheckoutDir remote get-url origin 2>$null | Out-String).Trim()
        if (-not [string]::IsNullOrWhiteSpace($currentRemote) -and $currentRemote -ne $RemoteUrl) {
            if (Confirm-YesNo -Prompt "Source checkout remote differs from requested; update origin to $RemoteUrl ?" -DefaultYes $false) {
                & git -C $CheckoutDir remote set-url origin $RemoteUrl
            } else {
                Fail "Source checkout remote does not match requested: $CheckoutDir"
            }
        }
        return
    }

    Fail "Source checkout path exists but is not a Git repo: $CheckoutDir"
}

function Get-SourceActivationMode {
    param(
        [string]$ProfileName,
        [bool]$CurrentlyActivated
    )

    return "no"
}

function Invoke-SourceBuild {
    param(
        [string]$ProfileName,
        [string]$ActivationMode = "preserve",
        [string]$ActionLabel = "Sync source profile",
        [switch]$SkipPlanConfirm
    )

    Validate-SourceProfileName -ProfileName $ProfileName

    $repoInfo = Resolve-SourceRepoInput -ProfileName $ProfileName
    $defaultCheckoutDir = Get-DefaultSourceCheckoutDir -RemoteInput ([string]$repoInfo.remote_url)
    $existing = Get-SourceProfile -ProfileName $ProfileName
    $existingCheckout = if ($existing) { [string]$existing.checkout_dir } else { "" }
    $checkoutDir = Resolve-SourceCheckoutDir -DefaultDir $defaultCheckoutDir -ProfileName $ProfileName
    $refName = $script:SourceRef
    $checkoutMode = if (-not (Test-Path -LiteralPath $checkoutDir)) {
        "Clone into new directory"
    } elseif (-not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir) -and $checkoutDir -ne $defaultCheckoutDir) {
        "Use explicitly specified checkout"
    } else {
        "Reuse existing checkout"
    }

    if (-not $SkipPlanConfirm) {
        if (-not (Confirm-SourcePlan -ActionLabel $ActionLabel -ProfileName $ProfileName -RepoInput ([string]$repoInfo.repo_input) -CheckoutDir $checkoutDir -RefName $refName -CheckoutMode $checkoutMode -CurrentRef $(if ($existing) { [string]$existing.current_ref } else { "" }) -CurrentCheckout $(if ($existing) { [string]$existing.checkout_dir } else { "" }))) {
            Write-Info "Canceled."
            return
        }
    }

    $report = Ensure-SourceToolchainReady
    Prepare-SourceCheckout -RemoteUrl ([string]$repoInfo.remote_url) -CheckoutDir $checkoutDir
    $refKind = Get-SourceRefKind -CheckoutDir $checkoutDir -RefName $refName
    Switch-SourceCheckoutToRef -CheckoutDir $checkoutDir -RefName $refName -RefKind $refKind
    $workspaceRoot = if (Test-Path -LiteralPath (Join-Path $checkoutDir "codex-rs\Cargo.toml")) {
        Join-Path $checkoutDir "codex-rs"
    } elseif (Test-Path -LiteralPath (Join-Path $checkoutDir "Cargo.toml")) {
        $checkoutDir
    } else {
        $checkoutDir
    }

    if ($null -eq $script:State) {
        $script:State = Ensure-StateShape $null
    }
    Select-CommandDir
    $controllerPath = Join-Path $script:StateRoot "libexec\hodexctl.ps1"
    Sync-ControllerCopy -TargetPath $controllerPath

    $installedAt = if ($existing) { [string]$existing.installed_at } else { "" }
    if ([string]::IsNullOrWhiteSpace($installedAt)) {
        $installedAt = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
    }
    $lastSyncedAt = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
    $workspaceMode = if (-not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir) -and -not [string]::IsNullOrWhiteSpace($existingCheckout) -and $existingCheckout -ne $checkoutDir) { "isolated" } elseif (-not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir) -and $checkoutDir -ne $defaultCheckoutDir) { "isolated" } else { "shared" }

    Set-SourceProfile -ProfileName $ProfileName -ActivationMode $ActivationMode -ProfileData ([ordered]@{
        name = $ProfileName
        repo_input = [string]$repoInfo.repo_input
        remote_url = [string]$repoInfo.remote_url
        checkout_dir = $checkoutDir
        workspace_mode = $workspaceMode
        current_ref = $refName
        ref_kind = $refKind
        build_workspace_root = $workspaceRoot
        binary_path = ""
        wrapper_path = ""
        installed_at = $installedAt
        last_synced_at = $lastSyncedAt
        toolchain_snapshot = (Get-SourceToolchainSnapshotJson -Report $report | ConvertFrom-Json)
    })

    $script:State.command_dir = $script:CurrentCommandDir
    $script:State.controller_path = $controllerPath
    Save-State -State $script:State
    Sync-RuntimeWrappersFromState -CommandDir $script:CurrentCommandDir -ControllerPath $controllerPath
    Update-PathIfNeeded
    Persist-StateRuntimeMetadata

    Write-Step "Source sync completed: $checkoutDir"
    Show-SourceResultSummary -ActionLabel $ActionLabel -ProfileName $ProfileName -RefName $refName -CheckoutDir $checkoutDir
}

function Invoke-SourceInstall {
    if (-not (Run-SourceInstallWizard)) {
        return
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $false
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "Download source and prepare toolchain" -SkipPlanConfirm
}

function Invoke-SourceUpdate {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "No source profiles found. Run 'hodexctl source install' first."
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $existing = Get-SourceProfile -ProfileName $profileName
    if (-not $PSBoundParameters.ContainsKey("Ref")) {
        $script:SourceRef = [string]$existing.current_ref
    }
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "Update source"
}

function Invoke-SourceRebuild {
    Fail "source rebuild has been removed; source mode now only keeps download/sync and toolchain prep."
}

function Invoke-SourceSwitch {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "No source profiles found. Run 'hodexctl source install' first."
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $existing = Get-SourceProfile -ProfileName $profileName
    if (-not $script:ExplicitSourceRef) {
        if ([Environment]::UserInteractive -and -not $Yes) {
            $script:SourceRef = Read-SourceRefWithChoices -RepoInput ([string]$existing.repo_input) -ProfileName $profileName -DefaultRef ([string]$existing.current_ref) -CheckoutDir ([string]$existing.checkout_dir)
            $script:ExplicitSourceRef = $true
        } else {
            Fail "source switch requires -Ref to specify branch/tag/commit."
        }
    }
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "Switch ref and sync source"
}

function Invoke-SourceStatus {
    Write-Host "Source mode status:"
    $profiles = Get-SourceProfiles
    if ($profiles.Count -eq 0) {
        Write-Host "  No source profiles installed"
        return
    }

    $selectedProfileName = ""
    if ($script:ExplicitSourceProfile -and -not [string]::IsNullOrWhiteSpace($script:SourceProfile)) {
        $selectedProfileName = $script:SourceProfile
    } elseif ($profiles.Count -eq 1) {
        $selectedProfileName = Resolve-SourceProfileName -RequireExisting $true
    }

    if (-not [string]::IsNullOrWhiteSpace($selectedProfileName)) {
        $profile = Get-SourceProfile -ProfileName $selectedProfileName
        if (-not $profile) {
            Fail "Source profile not found: $selectedProfileName"
        }
        Write-Host "  Name: $selectedProfileName"
        Write-Host "  Repo: $([string]$profile.repo_input)"
        Write-Host "  Remote: $([string]$profile.remote_url)"
        Write-Host "  Checkout: $([string]$profile.checkout_dir)"
        Write-Host "  Ref: $([string]$profile.current_ref) ($([string]$profile.ref_kind))"
        Write-Host "  Workspace: $([string]$profile.build_workspace_root)"
        Write-Host "  Installed at: $([string]$profile.installed_at)"
        Write-Host "  Last synced: $([string]$profile.last_synced_at)"
        Write-Host "  Mode: manage checkout and toolchain only; no source command wrappers generated"
        return
    }

    foreach ($profileName in $profiles.Keys) {
        $profile = $profiles[$profileName]
        Write-Host ("  - {0} | {1} | {2} | {3} | source-only management" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref, [string]$profile.checkout_dir)
    }
}

function Invoke-SourceList {
    Write-Host "Source profiles:"
    $profiles = Get-SourceProfiles
    if ($profiles.Count -eq 0) {
        Write-Host "  No source profiles recorded"
        return
    }
    foreach ($profileName in $profiles.Keys) {
        $profile = $profiles[$profileName]
        Write-Host ("  - {0} | {1} | {2} | {3} | source-only management" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref, [string]$profile.checkout_dir)
    }
}

function Invoke-SourceUninstall {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "No source profiles found."
    }

    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $profile = Get-SourceProfile -ProfileName $profileName
    if ($null -eq $profile) {
        Fail "Source profile not found: $profileName"
    }
    if (-not (Confirm-SourcePlan -ActionLabel "Uninstall source profile" -ProfileName $profileName -RepoInput ([string]$profile.repo_input) -CheckoutDir ([string]$profile.checkout_dir) -RefName ([string]$profile.current_ref) -Hint "This will remove the source profile record; optionally delete the checkout." -CheckoutMode "Remove existing profile assets" -CurrentRef ([string]$profile.current_ref) -CurrentCheckout ([string]$profile.checkout_dir))) {
        Write-Info "Canceled."
        return
    }

    $removeCheckout = switch ($script:SourceCheckoutPolicy) {
        "remove" { $true }
        "keep" { $false }
        default {
            if ($Yes) { $false } else { Confirm-YesNo -Prompt "Also delete checkout directory $([string]$profile.checkout_dir) ?" -DefaultYes $false }
        }
    }
    if ($removeCheckout -and -not [string]::IsNullOrWhiteSpace([string]$profile.checkout_dir)) {
        Remove-Item -LiteralPath ([string]$profile.checkout_dir) -Recurse -Force -ErrorAction SilentlyContinue
    }

    $oldRef = [string]$profile.current_ref
    $oldCheckout = [string]$profile.checkout_dir
    Remove-SourceProfile -ProfileName $profileName
    Sync-RuntimeWrappersFromState -CommandDir ([string]$script:State.command_dir) -ControllerPath ([string]$script:State.controller_path)

    if (@((Get-SourceProfiles).Keys).Count -eq 0 -and [string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.command_dir)) {
            Remove-PathIfNeeded -CurrentCommandDir ([string]$script:State.command_dir) -CurrentPathUpdateMode ([string]$script:State.path_update_mode)
        }
        foreach ($wrapperPath in @(
            (Join-Path ([string]$script:State.command_dir) "hodexctl.cmd"),
            (Join-Path ([string]$script:State.command_dir) "hodexctl.ps1")
        )) {
            if (-not [string]::IsNullOrWhiteSpace($wrapperPath)) {
                Remove-Item -LiteralPath $wrapperPath -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath (Get-StateFilePath) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath ([string]$script:State.controller_path) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "list-ui-state.json") -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "libexec") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "bin") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "src") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $script:StateRoot -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Source profile uninstalled: $profileName"
    Show-SourceResultSummary -ActionLabel "Uninstall source profile" -ProfileName $profileName -RefName $oldRef -CheckoutDir $oldCheckout
}

function Pause-SourceMenu {
    if ([Environment]::UserInteractive) {
        [void](Read-Host "Press Enter to continue")
    }
}

function Show-SourceMenu {
    while ($true) {
        Clear-Host
        $profileCount = @((Get-SourceProfiles).Keys).Count
        Write-Host "Source download / management"
        Write-Host ""
        Write-Host "Rule: hodex always points to release; source mode only manages checkout and toolchain."
        Write-Host "Current: $profileCount source profiles recorded"
        Write-Host ""
        Write-Host "  [Sync]"
        Write-Host "  1. Download source and prepare toolchain         Download or reuse checkout and check dev toolchain"
        Write-Host "  2. Update source                                 Fetch latest code for current profile ref"
        Write-Host "  3. Switch branch / tag / commit and sync         Switch to new ref then sync source"
        Write-Host ""
        Write-Host "  [View / Clean up]"
        Write-Host "  5. View source status                     Show one or all source profiles"
        Write-Host "  6. Uninstall source profile               Remove profile record; optionally delete checkout"
        Write-Host "  7. List source profiles                   Quick summary of all source profiles"
        Write-Host "  q. Back to version list"
        Write-Host ""

        $choice = Read-Host "Choose an action (enter number)"
        $actionLabel = ""
        $actionHint = ""
        $action = $null
        switch ($choice) {
            "1" {
                $actionLabel = "Download source and prepare toolchain"
                $actionHint = "Next: confirm repo, checkout dir, toolchain, and source profile name."
                $action = { Invoke-SourceInstall }
            }
            "2" {
                $actionLabel = "Update source"
                $actionHint = "Will fetch latest code for the current profile and sync checkout."
                $action = { Invoke-SourceUpdate }
            }
            "3" {
                $actionLabel = "Switch ref and sync source"
                $actionHint = "Next: specify a new branch / tag / commit."
                $action = { Invoke-SourceSwitch }
            }
            "5" {
                $actionLabel = "View source status"
                $actionHint = "Will show detailed status for source profiles."
                $action = { Invoke-SourceStatus }
            }
            "6" {
                $actionLabel = "Uninstall source profile"
                $actionHint = "Will remove the selected profile record; optionally delete the checkout."
                $action = { Invoke-SourceUninstall }
            }
            "7" {
                $actionLabel = "List source profiles"
                $actionHint = "Will show a summary for all source profiles."
                $action = { Invoke-SourceList }
            }
            "q" { return }
            "Q" { return }
            default {
                Write-WarnLine "Please enter 1, 2, 3, 5, 6, 7, or q."
                Pause-SourceMenu
                continue
            }
        }

        Clear-Host
        Write-Host "Source download / management"
        Write-Host ""
        Write-Host "Entering: $actionLabel"
        Write-Host "Hint: $actionHint"
        Write-Host ""
        Show-SourceMenuActionPreview -Choice $choice
        Write-Host ""

        try {
            & $action
            Write-Host ""
            Write-Host "Completed: $actionLabel"
        } catch {
            Write-Host ""
            Write-WarnLine "Failed: $actionLabel"
            Write-WarnLine ("Failure category: " + (Get-SourceErrorCategory -Message $_.Exception.Message))
            Write-WarnLine $_.Exception.Message
        }
        Pause-SourceMenu
    }
}

function Invoke-List {
    $items = @(Get-MatchingReleases)
    if ($items.Count -eq 0) {
        Fail "No release assets available for this platform."
    }

    $currentVersion = ""
    if ($script:State) {
        $currentVersion = [string]$script:State.installed_version
    }

    if (-not [Environment]::UserInteractive) {
        Write-Host "Available versions for this platform: $script:PlatformLabel"
        Write-Host ("{0,3}. {1,-12} {2}" -f 0, "Source mode", "Source download / management")
        for ($i = 0; $i -lt $items.Count; $i++) {
            $item = $items[$i]
            Write-Host ("{0,3}. {1,-12} {2} {3}" -f ($i + 1), [string]$item.version, [string]$item.published_at, [string]$item.asset.name)
        }
        return
    }

    while ($true) {
        Write-Host ""
        Write-Host "Available versions ($script:PlatformLabel):"
        Write-Host ("{0,3}. {1,-12} {2}" -f 0, "Source mode", "Source download / management")
        for ($i = 0; $i -lt $items.Count; $i++) {
            $item = $items[$i]
        $marker = ""
        if (-not [string]::IsNullOrWhiteSpace($currentVersion) -and [string]$item.version -eq $currentVersion) {
                $marker = " [installed]"
        }
        Write-Host ("{0,3}. {1,-12} {2} {3}{4}" -f ($i + 1), [string]$item.version, [string]$item.published_at, [string]$item.asset.name, $marker)
    }

        $choice = Read-Host "Enter number to view changelog, 0 for source mode, or press Enter to exit"
    if ([string]::IsNullOrWhiteSpace($choice)) {
        return
    }

    $index = 0
    if (-not [int]::TryParse($choice, [ref]$index) -or $index -lt 0 -or $index -gt $items.Count) {
            Write-WarnLine "Please enter a valid number."
        continue
    }
        if ($index -eq 0) {
            Show-SourceMenu
            continue
        }

        $selected = $items[$index - 1]
        Write-Host ""
        Write-Host (Get-ReleaseDetailsText -ReleaseInfo $selected)

        while ($true) {
            Write-Host ""
            Write-Host " AI summary " -ForegroundColor Black -BackgroundColor Yellow -NoNewline
            Write-Host " Press a to run hodex/codex for an AI changelog summary"
            $action = Read-Host "Action: [a]AI summary (hodex/codex) [i]Install [d]Download to $script:DownloadRoot [b]Back [q]Quit"
            $normalizedAction = if ($null -eq $action) { "" } else { $action.ToLowerInvariant() }
            switch ($normalizedAction) {
                "a" {
                    [void](Invoke-ReleaseSummary -ReleaseInfo $selected)
                    Write-Host ""
                    Write-Host (Get-ReleaseDetailsText -ReleaseInfo $selected)
                }
                "s" {
                    [void](Invoke-ReleaseSummary -ReleaseInfo $selected)
                    Write-Host ""
                    Write-Host (Get-ReleaseDetailsText -ReleaseInfo $selected)
                }
                "i" {
                    Invoke-InstallLike -RequestedVersion ([string]$selected.release_tag) -ActionLabel "Install"
                    return
                }
                "d" {
                    Invoke-Download -RequestedVersion ([string]$selected.release_tag)
                    return
                }
                "b" { break }
                "" { break }
                "q" { return }
                default { Write-WarnLine "Please enter a, i, d, b, or q." }
            }
        }
    }
}

function Invoke-InstallLike {
    param(
        [string]$RequestedVersion,
        [string]$ActionLabel
    )

    $previousNodeChoice = ""
    if ($script:State) {
        $previousNodeChoice = [string]$script:State.node_setup_choice
    }

    $release = Resolve-Release -RequestedVersion $RequestedVersion
    $asset = Get-AssetInfo -Release $release
    $releaseTag = [string]$release.tag_name
    $releaseName = [string]$release.name
    $resolvedVersion = Normalize-Version $(if ([string]::IsNullOrWhiteSpace($releaseTag)) { $releaseName } else { $releaseTag })

    Write-Step "$ActionLabel Hodex"
    Write-Step "Detected platform: $script:PlatformLabel"
    Write-Step "Selected release: $(if ([string]::IsNullOrWhiteSpace($releaseName)) { "<unknown>" } else { $releaseName }) ($(if ([string]::IsNullOrWhiteSpace($releaseTag)) { "<unknown>" } else { $releaseTag }))"
    Write-Step "Download asset: $([string]$asset.name)"

    Select-CommandDir

    $binaryDir = Join-Path $script:StateRoot "bin"
    $binaryPath = Join-Path $binaryDir "codex.exe"
    $controllerPath = Join-Path $script:StateRoot "libexec\hodexctl.ps1"
    Ensure-DirWritable $binaryDir
    Ensure-DirWritable (Split-Path -Parent $controllerPath)

    Write-Step "Install target binary: $binaryPath"
    Write-Step "Command dir: $script:CurrentCommandDir"

    Install-BinaryFromAsset -Asset $asset -BinaryPath $binaryPath
    Sync-ControllerCopy -TargetPath $controllerPath
    Remove-OldWrappersIfNeeded -NewCommandDir $script:CurrentCommandDir
    Prompt-NodeChoice -PreviousChoice $previousNodeChoice

    $detectedVersion = Get-InstalledBinaryVersion -BinaryPath $binaryPath
    if (-not [string]::IsNullOrWhiteSpace($detectedVersion)) {
        $resolvedVersion = $detectedVersion
        if ($releaseTag -eq "latest") {
            $releaseTag = "v$detectedVersion"
        }
        if ([string]::IsNullOrWhiteSpace($releaseName) -or $releaseName -eq "latest") {
            $releaseName = $detectedVersion
        }
    }

    $installedAt = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
    Write-State `
        -InstalledVersion $resolvedVersion `
        -ReleaseTag $releaseTag `
        -ReleaseName $releaseName `
        -AssetName ([string]$asset.name) `
        -BinaryPath $binaryPath `
        -ControllerPath $controllerPath `
        -CurrentCommandDir $script:CurrentCommandDir `
        -WrappersCreated @(
            (Join-Path $script:CurrentCommandDir "hodex.cmd"),
            (Join-Path $script:CurrentCommandDir "hodex.ps1"),
            (Join-Path $script:CurrentCommandDir "hodexctl.cmd"),
            (Join-Path $script:CurrentCommandDir "hodexctl.ps1")
        ) `
        -CurrentPathUpdateMode $script:PathUpdateMode `
        -CurrentPathProfile $script:PathProfile `
        -CurrentNodeSetupChoice $script:NodeSetupChoice `
        -InstalledAt $installedAt

    Sync-RuntimeWrappersFromState -CommandDir $script:CurrentCommandDir -ControllerPath $controllerPath
    Update-PathIfNeeded
    $script:State.node_setup_choice = $script:NodeSetupChoice
    Persist-StateRuntimeMetadata

    Write-Step "Install complete: $binaryPath"
    & $binaryPath --version

    switch ($script:PathUpdateMode) {
        "added" {
            Write-Info "User PATH updated."
        }
        "configured" {
            Write-Info "User PATH refreshed."
        }
        "already" {
            Write-Info "Command dir already in PATH: $script:CurrentCommandDir"
        }
        "disabled" {
            Write-WarnLine "Command dir not added to PATH; add manually: $script:CurrentCommandDir"
        }
        "user-skipped" {
            Write-WarnLine "Command dir not added to PATH; add manually: $script:CurrentCommandDir"
        }
    }

    Write-Info "Next: run 'hodex --version' to verify the install"
    Write-Info "Management: 'hodexctl status' / 'hodexctl list'"
}

function Invoke-ManagerInstall {
    $existingState = $script:State
    $stateLoaded = $null -ne $existingState

    Write-Step "Install hodexctl manager"
    Select-CommandDir

    $controllerPath = Join-Path $script:StateRoot "libexec\hodexctl.ps1"
    Ensure-DirWritable (Split-Path -Parent $controllerPath)
    Sync-ControllerCopy -TargetPath $controllerPath
    if ($stateLoaded) {
        Remove-OldWrappersIfNeeded -NewCommandDir $script:CurrentCommandDir
    }

    $installedVersion = ""
    $releaseTag = ""
    $releaseName = ""
    $assetName = ""
    $binaryPath = ""
    $nodeChoice = ""
    $installedAt = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")

    if ($stateLoaded) {
        $installedVersion = [string]$existingState.installed_version
        $releaseTag = [string]$existingState.release_tag
        $releaseName = [string]$existingState.release_name
        $assetName = [string]$existingState.asset_name
        $binaryPath = [string]$existingState.binary_path
        $nodeChoice = [string]$existingState.node_setup_choice
        if (-not [string]::IsNullOrWhiteSpace([string]$existingState.installed_at)) {
            $installedAt = [string]$existingState.installed_at
        }
    }

    Write-State `
        -InstalledVersion $installedVersion `
        -ReleaseTag $releaseTag `
        -ReleaseName $releaseName `
        -AssetName $assetName `
        -BinaryPath $binaryPath `
        -ControllerPath $controllerPath `
        -CurrentCommandDir $script:CurrentCommandDir `
        -WrappersCreated @(
            (Join-Path $script:CurrentCommandDir "hodex.cmd"),
            (Join-Path $script:CurrentCommandDir "hodex.ps1"),
            (Join-Path $script:CurrentCommandDir "hodexctl.cmd"),
            (Join-Path $script:CurrentCommandDir "hodexctl.ps1")
        ) `
        -CurrentPathUpdateMode $script:PathUpdateMode `
        -CurrentPathProfile $script:PathProfile `
        -CurrentNodeSetupChoice $nodeChoice `
        -InstalledAt $installedAt

    Sync-RuntimeWrappersFromState -CommandDir $script:CurrentCommandDir -ControllerPath $controllerPath
    Update-PathIfNeeded
    Persist-StateRuntimeMetadata

    Write-Step "hodexctl installed: $(Join-Path $script:CurrentCommandDir 'hodexctl.cmd')"
    Write-Info "State dir: $script:StateRoot"
    Write-Info "Command dir: $script:CurrentCommandDir"
    Write-Info "Only the manager is installed; run: hodexctl install"
    switch ($script:PathUpdateMode) {
        "added" {
            Write-Info "User PATH updated."
            Write-Info "If PowerShell still does not see hodexctl, reopen it."
        }
        "configured" {
            Write-Info "User PATH refreshed."
            Write-Info "If PowerShell still does not see hodexctl, reopen it."
        }
        "already" {
            Write-Info "Command dir already in PATH: $script:CurrentCommandDir"
        }
        "disabled" {
            Write-WarnLine "Command dir not added to PATH; add manually: $script:CurrentCommandDir"
        }
        "user-skipped" {
            Write-WarnLine "Command dir not added to PATH; add manually: $script:CurrentCommandDir"
        }
    }
    Write-Info "Next: run 'hodexctl' for help"
    Write-Info "Install release: 'hodexctl install'"
    Write-Info "List versions: 'hodexctl list'"
    Write-Info "Download source and prepare toolchain: 'hodexctl source install -Repo stellarlinkco/codex -Ref main'"
}

function Invoke-Uninstall {
    if (-not $script:State) {
        Fail "No hodex install state found; nothing to uninstall."
    }
    if ([string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
        if (@((Get-SourceProfiles).Keys).Count -gt 0) {
            Fail "No release install found; to remove source profiles use: hodexctl source uninstall."
        }

        Write-Step "Uninstall hodexctl manager"
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.command_dir)) {
            Remove-PathIfNeeded -CurrentCommandDir ([string]$script:State.command_dir) -CurrentPathUpdateMode ([string]$script:State.path_update_mode)
        }
        foreach ($wrapperPath in @(
            (Join-Path ([string]$script:State.command_dir) "hodexctl.cmd"),
            (Join-Path ([string]$script:State.command_dir) "hodexctl.ps1")
        )) {
            if (-not [string]::IsNullOrWhiteSpace($wrapperPath)) {
                Remove-Item -LiteralPath $wrapperPath -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath (Get-StateFilePath) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath ([string]$script:State.controller_path) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "list-ui-state.json") -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "libexec") -Force -Recurse -ErrorAction SilentlyContinue
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.command_dir)) {
            Remove-Item -LiteralPath ([string]$script:State.command_dir) -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "bin") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $script:StateRoot -Force -ErrorAction SilentlyContinue
        Write-Info "hodexctl manager uninstalled."
        return
    }

    Write-Step "Uninstall Hodex release"

    if (-not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
        Remove-Item -LiteralPath ([string]$script:State.binary_path) -Force -ErrorAction SilentlyContinue
        foreach ($helperPath in @(Get-ReleaseHelperPaths -BinaryPath ([string]$script:State.binary_path))) {
            Remove-Item -LiteralPath $helperPath -Force -ErrorAction SilentlyContinue
        }
    }
    Remove-ManagedRuntimeWrappersFromDir -CommandDir ([string]$script:State.command_dir)
    Clear-ReleaseState
    Sync-RuntimeWrappersFromState -CommandDir ([string]$script:State.command_dir) -ControllerPath ([string]$script:State.controller_path)

    if (@((Get-SourceProfiles).Keys).Count -eq 0) {
        Remove-PathIfNeeded -CurrentCommandDir ([string]$script:State.command_dir) -CurrentPathUpdateMode ([string]$script:State.path_update_mode)
        foreach ($wrapperPath in @(
            (Join-Path ([string]$script:State.command_dir) "hodexctl.cmd"),
            (Join-Path ([string]$script:State.command_dir) "hodexctl.ps1")
        )) {
            if (-not [string]::IsNullOrWhiteSpace($wrapperPath)) {
                Remove-Item -LiteralPath $wrapperPath -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath (Get-StateFilePath) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath ([string]$script:State.controller_path) -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "list-ui-state.json") -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "libexec") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath (Join-Path $script:StateRoot "bin") -Force -Recurse -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $script:StateRoot -Force -ErrorAction SilentlyContinue
        Write-Info "Removed release binary, wrappers, and install state."
    } else {
        Write-Info "Removed release binary; source profiles and manager script kept."
    }
}

function Invoke-Status {
    $repairNeeded = $false
    Write-Output "Platform: $script:PlatformLabel"
    Write-Output "State dir: $script:StateRoot"

    if ($script:State) {
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
            Write-Output "Release install status: installed"
            Write-Output "Version: $([string]$script:State.installed_version)"
            Write-Output "Release: $([string]$script:State.release_name) ($([string]$script:State.release_tag))"
            Write-Output "Asset: $([string]$script:State.asset_name)"
            Write-Output "Binary: $([string]$script:State.binary_path)"
            if ($env:OS -eq "Windows_NT") {
                $helpersComplete = Test-ReleaseHelpersComplete -BinaryPath ([string]$script:State.binary_path)
                Write-Output ("Windows runtime components: " + $(if ($helpersComplete) { "complete" } else { "missing" }))
                foreach ($helperPath in @(Get-ReleaseHelperPaths -BinaryPath ([string]$script:State.binary_path))) {
                    $helperName = Split-Path -Leaf $helperPath
                    Write-Output ("  - {0}: {1}" -f $helperName, $(if (Test-Path -LiteralPath $helperPath) { "installed" } else { "missing" }))
                }
                if (-not $helpersComplete) {
                    Write-WarnLine "Current Windows install is incomplete; run hodexctl install or hodexctl upgrade."
                }
            }
        } else {
            Write-Output "Release install status: not installed"
            if (-not [string]::IsNullOrWhiteSpace([string]$script:State.controller_path) -and (Test-Path -LiteralPath ([string]$script:State.controller_path))) {
                Write-Output "Manager status: installed"
                Write-Output "Hint: run hodexctl install to install release"
            }
        }
        Write-Output "Command dir: $([string]$script:State.command_dir)"
        Write-Output "Manager script copy: $([string]$script:State.controller_path)"
        Write-Output "PATH update mode: $([string]$script:State.path_update_mode)"
        Write-Output ("PATH managed by hodexctl: " + $(if ($script:State.path_managed_by_hodexctl) { "true" } else { "false" }))
        Write-Output "PATH source: $([string]$script:State.path_detected_source)"
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.path_profile)) {
            Write-Output "PATH scope: $([string]$script:State.path_profile)"
        }
        Write-Output "Node setup choice: $([string]$script:State.node_setup_choice)"
        Write-Output "Installed at: $([string]$script:State.installed_at)"
        $hodexWrapper = Join-Path ([string]$script:State.command_dir) 'hodex.cmd'
        $hodexctlWrapper = Join-Path ([string]$script:State.command_dir) 'hodexctl.cmd'
        if (Test-Path -LiteralPath $hodexWrapper) {
            Write-Output "hodex wrapper: $hodexWrapper"
        }
        if (Test-Path -LiteralPath $hodexctlWrapper) {
            Write-Output "hodexctl wrapper: $hodexctlWrapper"
        }
        Write-Output "Managed hodex target: $(Get-ActiveHodexAlias)"
        Write-Output "Source profiles: $(@((Get-SourceProfiles).Keys).Count)"
        foreach ($profileName in ((Get-SourceProfiles).Keys)) {
            $profile = Get-SourceProfile -ProfileName $profileName
            Write-Output ("Source profile: {0} | {1} | {2} | source-only management" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref)
        }

        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.controller_path) -and -not (Test-Path -LiteralPath ([string]$script:State.controller_path))) {
            Write-Output "Diagnostics: manager script copy missing"
            $repairNeeded = $true
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.command_dir) -and -not (Test-Path -LiteralPath (Join-Path ([string]$script:State.command_dir) "hodexctl.cmd"))) {
            Write-Output "Diagnostics: hodexctl wrapper missing"
            $repairNeeded = $true
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path) -and -not (Test-Path -LiteralPath ([string]$script:State.binary_path))) {
            Write-Output "Diagnostics: hodex release binary missing"
            $repairNeeded = $true
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path) -and (Test-Path -LiteralPath ([string]$script:State.binary_path)) -and -not (Test-Path -LiteralPath (Join-Path ([string]$script:State.command_dir) "hodex.cmd"))) {
            Write-Output "Diagnostics: hodex wrapper missing"
            $repairNeeded = $true
        }
        if ([string]$script:State.path_detected_source -eq "current-process-only") {
            Write-Output "Diagnostics: PATH is only visible in this session; new terminals may not work"
            $repairNeeded = $true
        }
    } else {
        Write-Output "Release install status: not installed"
        Write-Output "Source profiles: 0"
    }

    $hodexCmd = Get-Command hodex -ErrorAction SilentlyContinue
    if ($hodexCmd) {
        Write-Output "hodex in PATH: $($hodexCmd.Source)"
    } else {
        Write-Output "hodex in PATH: not found"
        if ($script:State -and -not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
            $repairNeeded = $true
        }
    }

    $codexCmd = Get-Command codex -ErrorAction SilentlyContinue
    if ($codexCmd) {
        Write-Output "codex in PATH: $($codexCmd.Source)"
    } else {
        Write-Output "codex in PATH: not found"
    }

    $nodeCmd = Get-Command node -ErrorAction SilentlyContinue
    if ($nodeCmd) {
        Write-Output "Node.js: $(& $nodeCmd.Source -v)"
    } else {
        Write-Output "Node.js: not installed"
    }

    if ($repairNeeded) {
        Write-Output "Recommended: run hodexctl repair"
    }
}

function Invoke-Relink {
    if (-not $script:State) {
        Fail "No hodex install state found; cannot relink."
    }

    if (-not $script:ExplicitCommandDir) {
        $script:CurrentCommandDir = [string]$script:State.command_dir
    } else {
        $script:CurrentCommandDir = Normalize-UserPath $script:CurrentCommandDir
        Ensure-DirWritable $script:CurrentCommandDir
    }

    Remove-OldWrappersIfNeeded -NewCommandDir $script:CurrentCommandDir
    Sync-ControllerCopy -TargetPath ([string]$script:State.controller_path)
    Write-State `
        -InstalledVersion ([string]$script:State.installed_version) `
        -ReleaseTag ([string]$script:State.release_tag) `
        -ReleaseName ([string]$script:State.release_name) `
        -AssetName ([string]$script:State.asset_name) `
        -BinaryPath ([string]$script:State.binary_path) `
        -ControllerPath ([string]$script:State.controller_path) `
        -CurrentCommandDir $script:CurrentCommandDir `
        -WrappersCreated @(
            (Join-Path $script:CurrentCommandDir "hodex.cmd"),
            (Join-Path $script:CurrentCommandDir "hodex.ps1"),
            (Join-Path $script:CurrentCommandDir "hodexctl.cmd"),
            (Join-Path $script:CurrentCommandDir "hodexctl.ps1")
        ) `
        -CurrentPathUpdateMode $script:PathUpdateMode `
        -CurrentPathProfile $script:PathProfile `
        -CurrentNodeSetupChoice ([string]$script:State.node_setup_choice) `
        -InstalledAt ([string]$script:State.installed_at)

    Sync-RuntimeWrappersFromState -CommandDir $script:CurrentCommandDir -ControllerPath ([string]$script:State.controller_path)
    Update-PathIfNeeded
    Persist-StateRuntimeMetadata
    Write-Info "Rebuilt release and manager wrappers in: $script:CurrentCommandDir"
}

function Invoke-Repair {
    if (-not $script:State) {
        Fail "No hodex install state found; cannot repair."
    }

    Write-Step "Repair hodexctl local state"
    Invoke-Relink

    $script:State = Load-State
    if ($script:State -and -not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path) -and -not (Test-Path -LiteralPath ([string]$script:State.binary_path))) {
        Write-WarnLine "Release binary missing; manager script, wrappers, and PATH were repaired, but the binary cannot be restored offline."
        Write-Info "Next: run 'hodexctl install' or 'hodexctl upgrade <version>' to restore the release."
        return
    }

    Write-Info "Repair completed."
}

if (-not $env:HODEXCTL_SKIP_MAIN) {
    try {
        Ensure-LocalToolPaths
        Normalize-Parameters
        if ($Activate -or $NoActivate) {
            Fail "Source mode does not take over hodex; source checkout is for sync and toolchain management only."
        }
        if ($script:RawSourceHelpRequest -or ($script:RequestedCommand -eq "source" -and $script:SourceAction -eq "help")) {
            Show-SourceUsage
            exit 0
        }
        Detect-Platform
        $script:State = Load-State

        switch ($script:RequestedCommand) {
            "install" {
                Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "Install"
            }
            "upgrade" {
                Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "Upgrade"
            }
            "download" {
                Invoke-Download -RequestedVersion $script:RequestedVersion
            }
            "downgrade" {
                Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "Downgrade"
            }
            "source" {
                switch ($script:SourceAction) {
                    "install" { Invoke-SourceInstall }
                    "update" { Invoke-SourceUpdate }
                    "rebuild" { Invoke-SourceRebuild }
                    "switch" { Invoke-SourceSwitch }
                    "status" { Invoke-SourceStatus }
                    "uninstall" { Invoke-SourceUninstall }
                    "list" { Invoke-SourceList }
                    default { Show-SourceUsage }
                }
            }
            "uninstall" {
                Invoke-Uninstall
            }
            "status" {
                Invoke-Status
            }
            "list" {
                Invoke-List
            }
            "relink" {
                Invoke-Relink
            }
            "repair" {
                Invoke-Repair
            }
            "manager-install" {
                Invoke-ManagerInstall
            }
            default {
                Fail "Unknown command: $script:RequestedCommand"
            }
        }
    } catch {
        $message = $_.Exception.Message
        if ([string]::IsNullOrWhiteSpace($message)) {
            $message = ($_ | Out-String).Trim()
        }
        if ([string]::IsNullOrWhiteSpace($message)) {
            $message = "Unknown error."
        }
        [Console]::Out.WriteLine($message)
        exit 1
    }
}
