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
$script:ExplicitSourceProfile = $PSBoundParameters.ContainsKey("Profile") -or $PSBoundParameters.ContainsKey("Name")
$script:SourceProfile = $Profile
$script:SourceRef = if ([string]::IsNullOrWhiteSpace($Ref)) { "main" } else { $Ref }
$script:SourceGitUrl = $GitUrl
$script:SourceCheckoutDir = $CheckoutDir
$script:SourceCheckoutPolicy = if ($RemoveCheckout) { "remove" } elseif ($KeepCheckout) { "keep" } else { "ask" }
$script:ExplicitSourceRef = $PSBoundParameters.ContainsKey("Ref")
$script:RawSourceHelpRequest = ($Command -eq "source" -and $Version -eq "help")

if ($PSBoundParameters.ContainsKey("Name")) {
    Write-Warning "-Name 已废弃，请改用 -Profile。"
}

if ($Activate -or $NoActivate) {
    throw "源码模式不允许接管 hodex；源码 checkout 仅用于同步与工具链管理。"
}

function Show-Usage {
    $standaloneCommand = ".\hodexctl.ps1"
    @"
用法:
  $($script:DisplayCommand)
  $($script:DisplayCommand) <command> [version] [options]

命令:
  install [version]      初始安装或重装 hodex，默认安装 latest
  upgrade [version]      升级到 latest 或指定版本
  download [version]     下载当前平台资产到下载目录，默认 latest
  downgrade <version>    降级到指定版本
  source <action>        源码下载/同步/工具链管理
  uninstall              卸载 hodex 相关文件
  status                 查看当前安装状态
  list                   交互式列出当前平台可下载版本，并支持查看更新日志
  relink                 重新生成 hodex / hodexctl 包装器
  help                   显示帮助

选项:
  -Repo <owner/repo>             指定 GitHub 仓库，默认 stellarlinkco/codex
  -CommandDir <path>             指定生成 hodex / hodexctl 的目录
  -StateDir <path>               指定状态目录，默认 %LOCALAPPDATA%\hodex
  -DownloadDir <path>            指定下载目录，默认 ~/Downloads
  -NodeMode <mode>               Node 处理策略：ask|skip|native|nvm|manual
  -GitUrl <url>                  源码模式指定 Git clone 地址
  -Ref <branch|tag|commit>       源码模式指定分支、标签或提交，默认 main
  -CheckoutDir <path>            源码模式指定 checkout 目录
  -Profile <profile-name>        源码模式指定源码记录名，默认 codex-source
  -KeepCheckout                  源码卸载时保留源码目录
  -RemoveCheckout                源码卸载时删除源码目录
  -List                          等价于 list
  -Yes                           非交互模式，使用默认选项
  -NoPathUpdate                  不自动修改 PATH
  -GitHubToken <token>           GitHub API Token，缓解速率限制
  -Help                          显示帮助

示例（已安装后，推荐通过 hodexctl 使用）:
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
  hodexctl uninstall

示例（独立下载脚本后直接运行）:
  $standaloneCommand install
  $standaloneCommand install 1.2.2
  $standaloneCommand upgrade
  $standaloneCommand download 1.2.3 -DownloadDir ~/Downloads
  $standaloneCommand list
  $standaloneCommand downgrade 1.2.2
  $standaloneCommand source install -GitUrl https://github.com/stellarlinkco/codex.git -Ref main
  $standaloneCommand relink -CommandDir %LOCALAPPDATA%\hodex\commands
  $standaloneCommand uninstall
"@ | Write-Host
}

function Show-SourceUsage {
    @"
源码模式用法:
  $($script:DisplayCommand) source <action> [options]

动作:
  install                下载源码并准备工具链（不接管 hodex）
  update                 同步当前 ref 最新代码并复用现有 checkout
  switch                 切换到指定 -Ref 并同步源码
  status                 查看源码记录状态
  uninstall              移除源码记录，可选删除 checkout
  list                   列出所有源码记录
  help                   显示本帮助

常用选项:
  -Repo <owner/repo>             使用 GitHub 仓库名
  -GitUrl <url>                  使用 HTTPS / SSH Git URL
  -Ref <branch|tag|commit>       指定源码分支、标签或提交
  -CheckoutDir <path>            指定源码 checkout 目录
  -Profile <profile-name>        指定源码记录名（工作区标识），默认 codex-source
                                  备注: 这不是命令名，也不会接管 hodex
  -KeepCheckout / -RemoveCheckout 控制卸载时是否保留源码目录
"@ | Write-Host
}

function Show-ListUsage {
    @"
版本列表用法:
  $($script:DisplayCommand) list

列表页操作:
  输入编号查看更新日志
  输入 0 进入源码下载 / 管理
  直接回车退出

更新日志页操作:
  a        AI总结（调用 hodex/codex）
  i        安装当前版本
  d        下载当前平台资产
  b        返回版本列表
  q        退出
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
            $script:LastGhFallbackDetail = "已自动改用 gh api 获取 GitHub 数据。"
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

    switch ($script:LastGhFallbackReason) {
        "gh-success" { return "$BaseMessage`n$($script:LastGhFallbackDetail)" }
        "gh-missing" { return "$BaseMessage。当前未检测到 gh，可设置 GITHUB_TOKEN 或安装并登录 gh 后重试。" }
        "gh-not-authenticated" { return "$BaseMessage。已尝试 gh 兜底，但 gh 未登录；请执行 gh auth login，或设置 GITHUB_TOKEN 后重试。" }
        "gh-access-denied" { return "$BaseMessage。已尝试 gh 兜底，但当前 gh 登录态或 token 对仓库 $($script:RepoName) 没有足够权限：$($script:LastGhFallbackDetail)" }
        "gh-failed" { return "$BaseMessage。已尝试 gh 兜底，但 gh api 仍失败：$($script:LastGhFallbackDetail)" }
        default {
            if (-not [string]::IsNullOrWhiteSpace($script:ApiToken)) {
                return "$BaseMessage。已提供 GITHUB_TOKEN，但 GitHub API 仍不可用；也可尝试 gh auth login 后重试。"
            }
            return "$BaseMessage。可设置 GITHUB_TOKEN，或安装并登录 gh 后重试。"
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
            Write-WarnLine "$Label 失败，将在 ${delaySeconds}s 后重试（$attempt/$maxAttempts）：$(Get-RetryErrorSummary -ErrorText $message)"
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
        $client = [System.Net.Http.HttpClient]::new()
        $progressEnabled = [Environment]::UserInteractive -and -not [Console]::IsOutputRedirected
        $previousProgressPreference = $global:ProgressPreference

        try {
            if ($progressEnabled) {
                $global:ProgressPreference = "Continue"
            }

            $response = $client.GetAsync($Uri, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
            $response.EnsureSuccessStatusCode()

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
                            $status = "{0}% | {1} / {2} | 速度 {3}" -f $percent, (Format-ByteSize $bytesReadTotal), (Format-ByteSize $totalBytes), $speedText
                            Write-Progress -Activity $Label -Status $status -PercentComplete $percent
                        } else {
                            $status = "{0} | 速度 {1}" -f (Format-ByteSize $bytesReadTotal), $speedText
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
        Fail "目录不可写: $Path"
    } finally {
        Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
    }
}

function Normalize-Parameters {
    $validCommands = @("install", "upgrade", "download", "downgrade", "source", "uninstall", "status", "list", "relink", "help")

    if ($script:InvokedWithNoArgs) {
        Show-Usage
        exit 0
    }

    if ($List) {
        $script:RequestedCommand = "list"
    }

    if ($script:RequestedCommand -notin $validCommands) {
        if ($PSBoundParameters.ContainsKey("Version")) {
            Fail "多余参数: $script:RequestedVersion"
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
            if ($PSBoundParameters.ContainsKey("Version")) {
                $script:SourceAction = $script:RequestedVersion
            } else {
                $script:SourceAction = "list"
            }

            if ($script:SourceAction -notin @("install", "update", "rebuild", "switch", "status", "uninstall", "list", "help")) {
                Fail "source 仅支持 install|update|switch|status|uninstall|list|help；兼容别名 rebuild 会直接提示已移除。"
            }

            if ($script:SourceAction -eq "help") {
                Show-SourceUsage
                exit 0
            }
        }
        "downgrade" {
            if (-not $PSBoundParameters.ContainsKey("Version") -or (Normalize-Version $script:RequestedVersion) -eq "latest") {
                Fail "downgrade 需要显式指定版本"
            }
        }
        "uninstall" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "uninstall 不接受额外版本参数"
            }
        }
        "status" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "status 不接受额外版本参数"
            }
        }
        "list" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "list 不接受额外版本参数"
            }
        }
        "relink" {
            if ($PSBoundParameters.ContainsKey("Version")) {
                Fail "relink 不接受额外版本参数"
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
        Fail "当前脚本仅支持 Windows；macOS、Linux、WSL 请改用 hodexctl.sh"
    }

    if (-not [Environment]::Is64BitOperatingSystem) {
        Fail "仅支持 64 位 Windows。"
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
            Fail "不支持的 Windows 架构: $arch"
        }
    }
}

function Get-StateFilePath {
    return Join-Path $script:StateRoot "state.json"
}

function Ensure-StateShape {
    param([object]$State)

    if ($null -eq $State) {
        $State = [pscustomobject]@{}
    }

    if (-not ($State.PSObject.Properties.Name -contains "schema_version")) {
        $State | Add-Member -NotePropertyName schema_version -NotePropertyValue 2 -Force
    }

    if (-not ($State.PSObject.Properties.Name -contains "source_profiles") -or $null -eq $State.source_profiles) {
        $State | Add-Member -NotePropertyName source_profiles -NotePropertyValue ([ordered]@{}) -Force
    }

    if (-not ($State.PSObject.Properties.Name -contains "active_runtime_aliases") -or $null -eq $State.active_runtime_aliases) {
        $State | Add-Member -NotePropertyName active_runtime_aliases -NotePropertyValue ([ordered]@{}) -Force
    }

    if (
        -not [string]::IsNullOrWhiteSpace([string]$State.binary_path) -and
        (
            ($State.active_runtime_aliases -is [System.Collections.IDictionary] -and -not $State.active_runtime_aliases.Contains("hodex")) -or
            ($State.active_runtime_aliases.PSObject.Properties.Name -notcontains "hodex")
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
            ($State.active_runtime_aliases.PSObject.Properties.Name -contains "hodex" -and [string]$State.active_runtime_aliases.hodex -ne "release")
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
    } elseif ($State.active_runtime_aliases.PSObject.Properties.Name -contains "hodex_stable") {
        $State.active_runtime_aliases.PSObject.Properties.Remove("hodex_stable")
    }

    if (-not ($State.PSObject.Properties.Name -contains "controller_path") -or [string]::IsNullOrWhiteSpace([string]$State.controller_path)) {
        $State | Add-Member -NotePropertyName controller_path -NotePropertyValue (Join-Path $script:StateRoot "libexec\hodexctl.ps1") -Force
    }

    $profiles = $State.source_profiles
    if ($profiles -is [System.Collections.IDictionary]) {
        foreach ($profileName in @($profiles.Keys)) {
            $profile = $profiles[$profileName]
            if ($null -eq $profile) {
                continue
            }
            if (-not ($profile.PSObject.Properties.Name -contains "last_synced_at") -or [string]::IsNullOrWhiteSpace([string]$profile.last_synced_at)) {
                $legacyLastBuiltAt = if ($profile.PSObject.Properties.Name -contains "last_built_at") { [string]$profile.last_built_at } else { "" }
                $profile | Add-Member -NotePropertyName last_synced_at -NotePropertyValue $legacyLastBuiltAt -Force
            }
        }
    } else {
        foreach ($property in $profiles.PSObject.Properties) {
            $profile = $property.Value
            if ($null -eq $profile) {
                continue
            }
            if (-not ($profile.PSObject.Properties.Name -contains "last_synced_at") -or [string]::IsNullOrWhiteSpace([string]$profile.last_synced_at)) {
                $legacyLastBuiltAt = if ($profile.PSObject.Properties.Name -contains "last_built_at") { [string]$profile.last_built_at } else { "" }
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
        node_setup_choice = $CurrentNodeSetupChoice
        installed_at      = $InstalledAt
        source_profiles   = [ordered]@{}
        active_runtime_aliases = [ordered]@{}
    }

    if ($state.source_profiles) {
        $payload.source_profiles = $state.source_profiles
    }
    if ($state.active_runtime_aliases) {
        $payload.active_runtime_aliases = $state.active_runtime_aliases
    }
    if (-not $payload.active_runtime_aliases.Contains("hodex")) {
        $payload.active_runtime_aliases.hodex = "release"
    }
    if ($payload.active_runtime_aliases.hodex -eq "release" -and $payload.active_runtime_aliases.Contains("hodex_stable")) {
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
    if ($state.active_runtime_aliases.PSObject.Properties.Name -contains "hodex") {
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
        Write-Host "请选择 hodex / hodexctl 的命令目录:"
        Write-Host "  1. $script:DefaultCommandDir"
        Write-Host "  2. $(Join-Path $script:StateRoot 'bin')"
        Write-Host "  3. 自定义目录"

        $choice = Read-Host "输入选项 [1/2/3]"
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
                $customDir = Read-Host "请输入安装目录"
                if ([string]::IsNullOrWhiteSpace($customDir)) {
                    Write-WarnLine "目录不能为空。"
                    continue
                }
                $script:CurrentCommandDir = Normalize-UserPath $customDir
                break
            }
            default {
                Write-WarnLine "请输入 1、2 或 3。"
            }
        }
    }

    Ensure-DirWritable $script:CurrentCommandDir
}

function Update-PathIfNeeded {
    $script:PathUpdateMode = "skipped"
    $script:PathProfile = ""

    if ($NoPathUpdate) {
        $script:PathUpdateMode = "disabled"
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
        return
    }

    if (Test-PathContains -PathValue $currentPath -Entry $script:CurrentCommandDir) {
        $script:PathUpdateMode = "already"
        return
    }

    $shouldUpdate = $true
    if (-not $Yes) {
        $answer = Read-Host "当前目录 $script:CurrentCommandDir 不在 PATH 中，是否写入用户 PATH？[Y/n]"
        if ($answer -match "^(n|N|no|NO)$") {
            $shouldUpdate = $false
        }
    }

    if (-not $shouldUpdate) {
        $script:PathUpdateMode = "user-skipped"
        return
    }

    $newUserPath = Add-PathEntry -PathValue $userPath -Entry $script:CurrentCommandDir
    [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    $env:Path = Add-PathEntry -PathValue $currentPath -Entry $script:CurrentCommandDir
    $script:PathUpdateMode = "added"
    $script:PathProfile = "User"
}

function Persist-StateRuntimeMetadata {
    if (-not $script:State) {
        return
    }

    $script:State.command_dir = $script:CurrentCommandDir
    $script:State.path_update_mode = $script:PathUpdateMode
    $script:State.path_profile = $script:PathProfile
    Save-State -State $script:State
}

function Remove-PathIfNeeded {
    param(
        [string]$CurrentCommandDir,
        [string]$CurrentPathUpdateMode
    )

    if ($CurrentPathUpdateMode -notin @("added", "configured")) {
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
                Invoke-HeadRequestWithRetry -Label "release-download" -Uri $assetUrl
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
                    Invoke-HeadRequestWithRetry -Label "release-download" -Uri $assetUrl
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

    Fail "未找到版本 $RequestedVersion 对应的当前平台资产：$($script:AssetCandidates -join ', ')"
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
            Fail (Get-GitHubApiFailureMessage -BaseMessage "获取 latest release 失败，请检查仓库名、GitHub API 限流或网络状态")
        }
    }

    try {
        $releases = @(Invoke-GitHubApi -Uri "https://api.github.com/repos/$script:RepoName/releases?per_page=100")
    } catch {
        Fail (Get-GitHubApiFailureMessage -BaseMessage "获取 release 列表失败，请检查仓库名、GitHub API 限流或网络状态")
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

    Fail "未找到版本 $RequestedVersion 对应的 release。"
}

function Get-AllReleases {
    $releases = New-Object System.Collections.Generic.List[object]
    $page = 1

    while ($true) {
        try {
            $pageReleases = @(Invoke-GitHubApi -Uri "https://api.github.com/repos/$script:RepoName/releases?per_page=100&page=$page")
        } catch {
            Fail (Get-GitHubApiFailureMessage -BaseMessage "获取 release 列表失败，请检查仓库名、GitHub API 限流或网络状态")
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

    $releaseObject = $null
    foreach ($candidateRelease in @($Release)) {
        if ($null -eq $candidateRelease) {
            continue
        }
        if ($candidateRelease.PSObject.Properties.Name -contains "assets") {
            $releaseObject = $candidateRelease
            break
        }
    }
    if ($null -eq $releaseObject) {
        Fail "release 响应缺少 assets 字段。"
    }

    foreach ($candidate in $script:AssetCandidates) {
        foreach ($asset in @($releaseObject.assets)) {
            if ([string]$asset.name -eq $candidate) {
                if ($script:ArchitectureName -eq "aarch64" -and [string]$asset.name -like "*x86_64*") {
                    Write-WarnLine "当前 release 未提供 Windows ARM64 原生资产，将回退使用 x64 资产，依赖 Windows ARM 的 x64 仿真。"
                }
                return $asset
            }
        }
    }

    Fail "release 未找到匹配当前平台的资产：$($script:AssetCandidates -join ', ')"
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
        Write-WarnLine "release 未提供 digest，跳过 SHA-256 校验。"
        return
    }

    if (-not $Digest.StartsWith("sha256:")) {
        Write-WarnLine "暂不支持的 digest 格式: $Digest"
        return
    }

    $expected = $Digest.Substring(7)
    $actual = (Get-FileHash -LiteralPath $DownloadedFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) {
        Fail "SHA-256 校验失败。期望 $expected，实际 $actual"
    }

    Write-Step "SHA-256 校验通过: $actual"
}

function Get-ReleaseDetailsText {
    param(
        [object]$ReleaseInfo
    )

    $release = $ReleaseInfo.release
    $asset = $ReleaseInfo.asset
    $body = [string]$release.body
    if ([string]::IsNullOrWhiteSpace($body)) {
        $body = "<该版本未提供更新日志>"
    }

    return @"
版本: $([string]$ReleaseInfo.version)
Release: $([string]$ReleaseInfo.release_name) ($([string]$ReleaseInfo.release_tag))
发布时间: $([string]$ReleaseInfo.published_at)
当前平台资产: $([string]$asset.name)
页面: $([string]$ReleaseInfo.html_url)

更新日志:
$body
"@
}

function Get-ReleaseSummaryPrompt {
    param([object]$ReleaseInfo)

    $release = $ReleaseInfo.release
    $body = [string]$release.body
    if ([string]::IsNullOrWhiteSpace($body)) {
        $body = "<该版本未提供更新日志>"
    }

    return @"
请基于下面这个 Hodex release 的完整 changelog，输出一份简体中文总结。

输出要求：
1. 只输出最终总结，不要输出思考过程、分析过程、草稿、自我说明或额外前言。
2. 必须优先按类别做结构化总结，能归类就归类；推荐分类顺序为：
   - 新增功能
   - 改进优化
   - 修复内容
   - 破坏性变更 / 迁移要求
   - 其他说明
3. 没有内容的类别可以省略；不要为了凑分类编造内容。
4. 每个类别下用简短要点列出关键信息，优先覆盖用户最关心的变化。
5. 如果存在破坏性变更、兼容性影响、配置变更、需要手动处理的步骤，必须单独明确指出。
6. 不要遗漏重要信息，不要编造日志中不存在的内容。
7. 直接开始输出中文总结正文，不要再写“以下是总结”等前言。

版本: $([string]$ReleaseInfo.version)
Release: $([string]$ReleaseInfo.release_name) ($([string]$ReleaseInfo.release_tag))
发布时间: $([string]$ReleaseInfo.published_at)
页面: $([string]$ReleaseInfo.html_url)

完整 changelog:
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
        [void](Read-Host "按回车返回版本详情")
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
        Write-WarnLine "未找到可用的 hodex/codex 命令，无法自动总结 changelog。"
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
                Write-WarnLine "首选命令不可用，已自动改用 $([string]$candidate.Name)。"
                Write-Host ""
            }

            Write-Host "AI 总结生成中，请稍候..."
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
                    Write-WarnLine "$([string]$candidate.Name) 执行失败: $([string](Get-RetryErrorSummary -ErrorText $stderrText))"
                }
            } catch {
            }

            $usedFallback = $true
            Write-Host ""
            Write-WarnLine "$([string]$candidate.Name) 总结 changelog 失败，准备尝试下一个可用命令。"
            Write-Host ""
        }

        Clear-ReleaseSummaryScreen
        Write-WarnLine "当前找到的 hodex/codex 都无法执行 changelog 总结。"
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
        $answer = Read-Host "目标文件已存在，是否覆盖？[Y/n]"
        if ($answer -match "^(n|N|no|NO)$") {
            Write-Info "已取消下载。"
            return
        }
    }

    Write-Step "下载 Hodex 资产"
    Write-Step "命中 release: $([string]$release.name) ($([string]$release.tag_name))"
    Write-Step "下载资产: $([string]$asset.name)"
    Write-Step "保存路径: $outputPath"
    $downloadResult = Invoke-DownloadWithProgress -Label ("下载 " + [string]$asset.name) -Uri ([string]$asset.browser_download_url) -OutFile $outputPath
    Verify-DigestIfPresent -DownloadedFile $outputPath -Digest ([string]$asset.digest)
    Write-Info ("下载完成: {0}，平均速度 {1}/s" -f (Format-ByteSize $downloadResult.bytes), (Format-ByteSize $downloadResult.speed))
    Write-Info "已下载到: $outputPath"
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
    Write-Step "下载 hodexctl 管理脚本"
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
  echo hodex 二进制不存在，请先运行 hodexctl install。 1>&2
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
    Write-Error "hodex 二进制不存在，请先运行 hodexctl install。"
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
  echo $CommandName 对应的二进制不存在，请重新运行 hodexctl 安装或重编译。 1>&2
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
    Write-Error "$CommandName 对应的二进制不存在，请重新运行 hodexctl 安装或重编译。"
    exit 1
}
& "$BinaryPath" @args
"@

    Set-Content -LiteralPath $WrapperPath -Value $content -Encoding UTF8
}

function Generate-HodexctlCmdWrapper {
    param(
        [string]$WrapperPath,
        [string]$ControllerPath
    )

    $runner = Get-ControllerCommand
    $content = @"
@echo off
set "HODEX_DISPLAY_NAME=hodexctl"
if not exist "$ControllerPath" (
  echo hodexctl 管理脚本不存在，请重新安装。 1>&2
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
        [string]$ControllerPath
    )

    $runner = Get-ControllerCommand
    $content = @"
`$ErrorActionPreference = "Stop"
`$env:HODEX_DISPLAY_NAME = "hodexctl"
if (-not (Test-Path -LiteralPath "$ControllerPath")) {
    Write-Error "hodexctl 管理脚本不存在，请重新安装。"
    exit 1
}
if (`$args.Count -eq 0) {
    & "$runner" -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" help
    exit `$LASTEXITCODE
}
& "$runner" -NoProfile -ExecutionPolicy Bypass -File "$ControllerPath" @args
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
    $keepControllerWrapper = $releaseInstalled -or @((Get-SourceProfiles).Keys).Count -gt 0

    if ($keepControllerWrapper -and -not [string]::IsNullOrWhiteSpace($ControllerPath)) {
        Generate-HodexctlCmdWrapper -WrapperPath (Join-Path $CommandDir "hodexctl.cmd") -ControllerPath $ControllerPath
        Generate-HodexctlPs1Wrapper -WrapperPath (Join-Path $CommandDir "hodexctl.ps1") -ControllerPath $ControllerPath
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
        Write-WarnLine "未检测到 winget，无法自动使用系统方式安装。请改用手动安装：$script:NodeDownloadUrl"
        return
    }

    Write-Step "使用 winget 安装 Node.js LTS"
    & winget install --exact --id OpenJS.NodeJS.LTS --accept-package-agreements --accept-source-agreements
}

function Install-NodeWithNvm {
    if (Test-Command "nvm") {
        Write-Step "使用 nvm 安装 Node.js LTS"
        & nvm install lts
        & nvm use lts
        return
    }

    if (-not (Test-Command "winget")) {
        Write-WarnLine "未检测到 winget，无法自动安装 nvm-windows。请手动安装：$script:NvmWindowsReleaseUrl"
        return
    }

    Write-Step "通过 winget 安装 nvm-windows"
    & winget install --exact --id CoreyButler.NVMforWindows --accept-package-agreements --accept-source-agreements
    Write-WarnLine "nvm-windows 已安装。首次安装后通常需要重新打开 PowerShell，再执行: nvm install lts"
}

function Prompt-NodeChoice {
    param([string]$PreviousChoice)

    if (Test-Command "node") {
        $script:NodeSetupChoice = "already-installed"
        return
    }

    if ($script:SelectedNodeMode -eq "ask" -and -not [string]::IsNullOrWhiteSpace($PreviousChoice)) {
        $script:NodeSetupChoice = $PreviousChoice
        Write-Info "当前未安装 Node.js，沿用既有记录: $PreviousChoice"
        return
    }

    if ($script:SelectedNodeMode -eq "ask" -and $Yes) {
        $script:NodeSetupChoice = "skip"
        Write-WarnLine "检测到未安装 Node.js；非交互模式默认跳过。"
        return
    }

    $effectiveMode = $script:SelectedNodeMode
    if ($effectiveMode -eq "ask") {
        Write-Host "检测到当前系统未安装 Node.js，可选处理方式:"
        Write-Host "  1. 系统方式安装"
        Write-Host "     - Windows: winget"
        Write-Host "  2. 使用 nvm（Windows 下为 nvm-windows）"
        Write-Host "  3. 手动下载安装（官网链接）"
        Write-Host "  4. 跳过"

        while ($true) {
            $answer = Read-Host "请选择 [1/2/3/4]"
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
                    Write-WarnLine "请输入 1、2、3 或 4。"
                }
            }
        }
    }

    $script:NodeSetupChoice = $effectiveMode
    switch ($effectiveMode) {
        "skip" {
            Write-Info "已跳过 Node.js 环境处理。"
        }
        "manual" {
            Write-Info "请手动安装 Node.js：$script:NodeDownloadUrl"
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
        Write-Step "临时下载路径: $downloadPath"
        $downloadResult = Invoke-DownloadWithProgress -Label ("下载 " + $assetName) -Uri ([string]$Asset.browser_download_url) -OutFile $downloadPath
        Verify-DigestIfPresent -DownloadedFile $downloadPath -Digest ([string]$Asset.digest)
        Write-Info ("下载完成: {0}，平均速度 {1}/s" -f (Format-ByteSize $downloadResult.bytes), (Format-ByteSize $downloadResult.speed))

        $sourceBinary = $downloadPath
        if ($assetName.ToLowerInvariant().EndsWith(".zip")) {
            $extractDir = Join-Path $tempDir "extract"
            New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
            Expand-Archive -LiteralPath $downloadPath -DestinationPath $extractDir -Force
            $sourceBinary = Join-Path $extractDir ([System.IO.Path]::GetFileNameWithoutExtension($assetName))

            if (-not (Test-Path -LiteralPath $sourceBinary)) {
                Fail "未在 release 资产中找到预期的 Windows 可执行文件。"
            }

            foreach ($helperName in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
                $helperSource = Join-Path $extractDir $helperName
                if (-not (Test-Path -LiteralPath $helperSource)) {
                    Fail "当前 Windows release 资产缺少必需 helper：$helperName"
                }
                Copy-Item -LiteralPath $helperSource -Destination (Join-Path $helperDir $helperName) -Force
            }
        } else {
            $assetUri = [System.Uri][string]$Asset.browser_download_url
            foreach ($helperName in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
                $helperDownloadPath = Join-Path $tempDir $helperName
                $helperUri = [System.Uri]::new($assetUri, $helperName).AbsoluteUri
                try {
                    $helperResult = Invoke-DownloadWithProgress -Label ("下载 " + $helperName) -Uri $helperUri -OutFile $helperDownloadPath
                    Write-Info ("下载完成: {0}，平均速度 {1}/s" -f (Format-ByteSize $helperResult.bytes), (Format-ByteSize $helperResult.speed))
                } catch {
                    Fail "当前 Windows release 资产缺少必需 helper：$helperName"
                }
                Copy-Item -LiteralPath $helperDownloadPath -Destination (Join-Path $helperDir $helperName) -Force
            }
        }

        if (-not (Test-Path -LiteralPath $sourceBinary)) {
            Fail "未在 release 资产中找到预期的 Windows 可执行文件。"
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
    Write-Host ("当前等待确认输入，直接回车将采用默认值 {0}。" -f $(if ($DefaultYes) { "Y" } else { "N" }))
    $answer = Read-Host "$Prompt $suffix"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        return $DefaultYes
    }
    return $answer -match "^(y|Y|yes|YES)$"
}

function Validate-SourceProfileName {
    param([string]$ProfileName)

    if ([string]::IsNullOrWhiteSpace($ProfileName)) {
        Fail "源码记录名不能为空。"
    }
    if ($ProfileName -notmatch '^[A-Za-z0-9._-]+$') {
        Fail "源码记录名仅支持字母、数字、点、下划线和连字符。"
    }
    if ($ProfileName -in @("hodex", "hodexctl", "hodex-stable")) {
        Fail "源码记录名不能使用保留名称: $ProfileName"
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

    $answer = Read-Host "请输入源码仓库（owner/repo 或 Git URL，默认 stellarlinkco/codex）"
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

    $host = "github.com"
    $pathPart = $RemoteInput
    if ($RemoteInput -match '://') {
        $stripped = ($RemoteInput -replace '^[^:]+://', '') -replace '^[^@]+@', ''
        $host, $pathPart = $stripped.Split('/', 2)
    } elseif ($RemoteInput -match '^git@') {
        $stripped = ($RemoteInput -replace '^git@', '')
        $host, $pathPart = $stripped.Split(':', 2)
    }
    $pathPart = $pathPart.TrimStart('/') -replace '\.git$', ''
    return [pscustomobject]@{ host = $host; path = $pathPart }
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

    Write-Host "  可选项:"
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
            Write-Host ("  默认值: " + $DefaultValue)
            if (-not [string]::IsNullOrWhiteSpace($Note)) {
                Write-Host ("  备注: " + $Note)
            }
            Write-Host "  输入当前页编号可直接选择，也可直接输入自定义值"
            Write-Host "  n/p 翻页，/关键词 过滤，c 清空过滤"
            if (-not [string]::IsNullOrWhiteSpace($query)) {
                Write-Host ("  当前过滤: " + $query)
            }

            if ($filteredItems.Count -eq 0) {
                Write-Host "  当前过滤无匹配候选"
            } else {
                $pageEnd = [Math]::Min($pageStart + $pageSize, $filteredItems.Count)
                $pageCount = [Math]::Ceiling($filteredItems.Count / [double]$pageSize)
                $pageNumber = [Math]::Floor($pageStart / $pageSize) + 1
                Write-Host ("  候选项: 第 {0}/{1} 页，共 {2} 项" -f $pageNumber, $pageCount, $filteredItems.Count)
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
                Write-WarnLine "请输入当前页范围内的编号。"
                continue
            }

            return $answer
        }
    }

    while ($true) {
        Write-Host $Label
        Write-Host ("  默认值: " + $DefaultValue)
        if (-not [string]::IsNullOrWhiteSpace($Note)) {
            Write-Host ("  备注: " + $Note)
        }
        Show-ChoiceItems -Items $ChoiceItems
        Write-Host "  输入编号可直接选择，也可直接输入自定义值"

        $answer = Read-Host ">"
        if ([string]::IsNullOrWhiteSpace($answer)) {
            return $DefaultValue
        }

        $index = 0
        if ([int]::TryParse($answer, [ref]$index) -and $ChoiceItems.Count -gt 0) {
            if ($index -ge 1 -and $index -le $ChoiceItems.Count) {
                return [string]$ChoiceItems[$index - 1]
            }
            Write-WarnLine "编号超出范围，请重新输入。"
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

    return Read-ValueWithChoices -Label "目标 ref（branch / tag / commit）" -DefaultValue $DefaultRef -ChoiceItems (Get-SourceRefCandidates -RepoInput $RepoInput -ProfileName $ProfileName -DefaultRef $DefaultRef -CheckoutDir $CheckoutDir) -Note "候选项默认只展示 branch；标签或 commit 可直接输入"
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
        Write-Host "源码下载向导"
        Write-Host ""
        Write-Host "将按顺序确认仓库、源码记录名、ref 和 checkout 目录。"
        Write-Host "直接回车表示接受默认值；源码模式仅下载/同步源码，不再编译。"
        Write-Host ""

        $repoInput = Read-ValueWithChoices -Label "源码仓库（owner/repo 或 Git URL）" -DefaultValue "stellarlinkco/codex" -ChoiceItems (Get-SourceRepoCandidates)

        while ($true) {
            $profileName = Read-ValueWithChoices -Label "源码记录名" -DefaultValue "codex-source" -ChoiceItems (Get-SourceProfileCandidates -RepoInput $repoInput) -Note "这是源码记录名/工作区标识，不是命令名"
            try {
                Validate-SourceProfileName -ProfileName $profileName
                break
            } catch {
                Write-WarnLine "源码记录名不能使用保留名称。"
            }
        }

        $remoteUrl = Get-SourceRemoteUrlFromInput -RepoInput $repoInput
        $defaultCheckoutDir = Get-DefaultSourceCheckoutDir -RemoteInput $remoteUrl
        $refName = Read-ValueWithChoices -Label "源码 ref（branch / tag / commit）" -DefaultValue "main" -ChoiceItems (Get-SourceRefCandidates -RepoInput $repoInput -ProfileName $profileName -DefaultRef "main" -CheckoutDir $defaultCheckoutDir) -Note "候选项默认只展示 branch；标签或 commit 可直接输入"
        Write-Host ""
        Write-Host "步骤 4/4 checkout"
        Write-Host "默认会把源码 checkout 放到受管源码目录，便于后续 update / switch 复用。"
        $checkoutDir = Normalize-UserPath (Read-ValueWithChoices -Label "源码 checkout 目录" -DefaultValue $defaultCheckoutDir -ChoiceItems (Get-SourceCheckoutCandidates -RemoteUrl $remoteUrl -DefaultCheckoutDir $defaultCheckoutDir))

        Write-Host ""
        Write-Host "向导摘要"
        Write-Host ("  仓库: " + $repoInput)
        Write-Host ("  源码记录名: " + $profileName)
        Write-Host ("  ref: " + $refName)
        Write-Host ("  checkout: " + $checkoutDir)

        Write-Host ""
        Write-Host "即将进入确认步骤。"
        if (Confirm-YesNo -Prompt "确认使用以上配置继续下载？" -DefaultYes $true) {
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

        if (-not (Confirm-YesNo -Prompt "是否重新填写源码下载向导？" -DefaultYes $true)) {
            Write-Info "已取消。"
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

    $answer = Read-Host "源码 checkout 目录 [$DefaultDir]"
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
                Write-Host "选择源码条目"
                Write-Host ""
                Write-Host ("共 {0} 个源码条目；默认优先定位 {1}，否则落到第一项。" -f $profiles.Count, $script:DefaultSourceProfileName)
                Write-Host "上下键移动  Enter 确认  q 取消"
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
                Write-Host ("选中摘要: {0} | ref={1} | checkout={2}" -f [string]$profiles[$cursor], [string]$selectedProfile.current_ref, [string]$selectedProfile.checkout_dir)
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
                            Fail "已取消选择源码条目。"
                        }
                    }
                }
            }
        }
        while ($true) {
            Write-Host "请选择源码条目:"
            for ($i = 0; $i -lt $profiles.Count; $i++) {
                $profile = Get-SourceProfile -ProfileName ([string]$profiles[$i])
                Write-Host ("  {0}. {1} | {2} | {3}" -f ($i + 1), [string]$profiles[$i], [string]$profile.current_ref, [string]$profile.repo_input)
            }
            $choice = Read-Host ("输入编号选择源码条目 [1-" + $profiles.Count + "]")
            $index = 0
            if ([int]::TryParse($choice, [ref]$index) -and $index -ge 1 -and $index -le $profiles.Count) {
                return [string]$profiles[$index - 1]
            }
            Write-WarnLine "请输入有效编号。"
        }
    }

    if ($script:ExplicitSourceProfile -and -not [string]::IsNullOrWhiteSpace($script:SourceProfile)) {
        return $script:SourceProfile
    }
    $defaultSourceProfile = if ([string]::IsNullOrWhiteSpace($script:SourceProfile)) { $script:DefaultSourceProfileName } else { $script:SourceProfile }
    if (-not $Yes) {
        $answer = Read-Host "源码记录名 [$defaultSourceProfile]"
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
    Write-Host ("即将执行: " + $ActionLabel)
    Write-Host ("  源码记录名: " + $ProfileName)
    Write-Host ("  仓库: " + $(if ([string]::IsNullOrWhiteSpace($RepoInput)) { "<unknown>" } else { $RepoInput }))
    Write-Host ("  checkout: " + $(if ([string]::IsNullOrWhiteSpace($CheckoutDir)) { "<unknown>" } else { $CheckoutDir }))
    Write-Host ("  ref: " + $(if ([string]::IsNullOrWhiteSpace($RefName)) { "<unknown>" } else { $RefName }))
    if (-not [string]::IsNullOrWhiteSpace($CurrentRef) -and $CurrentRef -ne $RefName) {
        Write-Host ("  当前 -> 目标 ref: " + $CurrentRef + " -> " + $RefName)
    }
    if (-not [string]::IsNullOrWhiteSpace($CurrentCheckout) -and $CurrentCheckout -ne $CheckoutDir) {
        Write-Host ("  当前 -> 目标 checkout: " + $CurrentCheckout + " -> " + $CheckoutDir)
    }
    if (-not [string]::IsNullOrWhiteSpace($CheckoutMode)) {
        Write-Host ("  checkout 策略: " + $CheckoutMode)
    }
    if (-not [string]::IsNullOrWhiteSpace($Hint)) {
        Write-Host ("  说明: " + $Hint)
    }
    return (Confirm-YesNo -Prompt "确认继续？" -DefaultYes $true)
}

function Get-SourceErrorCategory {
    param([string]$Message)

    switch -Regex ($Message) {
        '工具链|缺失|rustup|cargo|rustc|xcode-clt|msvc-build-tools' { return "工具链问题" }
        'Git 仓库|远端|clone|git|checkout|未提交修改' { return "Git / 源码目录问题" }
        'ref|branch|tag|commit' { return "目标 ref 问题" }
        '构建|编译|产物|cargo build' { return "构建问题" }
        '名称|保留名称|-dev|参数' { return "输入参数问题" }
        default { return "未分类问题" }
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
    Write-Host "结果摘要"
        Write-Host ("  动作: " + $ActionLabel)
        Write-Host ("  源码记录名: " + $ProfileName)
    if (-not [string]::IsNullOrWhiteSpace($RefName)) {
        Write-Host ("  当前 ref: " + $RefName)
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
            Write-Host ("  默认仓库: " + $previewRepo)
            Write-Host ("  默认源码记录名: " + $previewName)
            Write-Host ("  默认 ref: " + $script:SourceRef)
            Write-Host ("  默认 checkout: " + $previewCheckout)
            Write-Host "  执行内容: clone/fetch、工具链检查、登记源码记录"
        }
        "2" {
            Write-Host "  默认对象: 单个源码条目自动选中；多个源码条目进入选择器"
            Write-Host "  执行内容: fetch 最新代码、切回当前 ref、同步 checkout"
            Write-Host "  保留规则: 只管理源码目录和工具链，不影响 hodex release"
        }
        "3" {
            Write-Host "  默认对象: 单个源码条目自动选中；多个源码条目进入选择器"
            Write-Host "  执行内容: 先确认新的 branch/tag/commit，再切换并同步源码"
            Write-Host "  安全限制: checkout 存在未提交修改时会拒绝切换"
        }
        "4" {
            Write-Host "  当前版本已移除源码编译能力。"
            Write-Host "  如需最新源码，请使用“更新源码”或“切换 ref”。"
        }
        "5" {
            Write-Host "  默认对象: 单个源码条目自动展示详情；多个源码条目展示摘要列表"
            Write-Host "  展示内容: 仓库、ref、checkout、工作区、最近同步时间"
        }
        "6" {
            Write-Host "  默认对象: 单个源码条目自动选中；多个源码条目进入选择器"
            Write-Host "  删除内容: 源码条目记录，可选删除 checkout"
            Write-Host "  最后清理: 如果这是最后一个 runtime，会连同 hodexctl 和受管 PATH 一起清理"
        }
        "7" {
            Write-Host ("  展示内容: 所有源码条目的仓库、ref、checkout 摘要")
            Write-Host ("  当前已记录: " + $profileCount + " 个")
        }
    }
}

function Ensure-GitWorktreeClean {
    param([string]$CheckoutDir)

    $status = (& git -C $CheckoutDir status --porcelain --untracked-files=no 2>$null | Out-String).Trim()
    if (-not [string]::IsNullOrWhiteSpace($status)) {
        Fail "源码目录存在未提交修改，请先提交或清理后再切换/更新: $CheckoutDir"
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
    Fail "未找到可用的 ref: $RefName"
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
    Fail "未识别到可支持的源码构建入口（缺少 codex-rs/Cargo.toml 或 Cargo.toml）。"
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
    Fail "当前源码仓库未检测到可构建的 codex CLI 入口。"
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
        Fail "未找到源码构建目标对应的 Cargo package。"
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
                Fail "未知的源码构建模式: $BuildMode"
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
    Write-Host ("编译进度预估: {0} 个编译单元" -f $totalUnits)

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

    Write-Progress -Activity "编译源码版 Hodex" -Status "准备构建图" -PercentComplete 0

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
                                Write-Progress -Activity "编译源码版 Hodex" -Status $status -PercentComplete $percent -SecondsRemaining ([int][Math]::Max($remaining, 0))
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
                            Write-Progress -Activity "编译源码版 Hodex" -Status "构建完成" -PercentComplete 100 -Completed
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
        Write-Progress -Activity "编译源码版 Hodex" -Completed
        throw "EXITCODE=$LASTEXITCODE"
    }

    Write-Progress -Activity "编译源码版 Hodex" -Status "构建完成" -PercentComplete 100 -Completed
}

function Build-SourceBinary {
    param(
        [string]$WorkspaceRoot,
        [string]$BinaryOutputPath
    )

    $strategy = Get-SourceBuildStrategy -WorkspaceRoot $WorkspaceRoot
    Write-Step "编译源码版 Hodex"
    Invoke-WithRetry -Label "cargo-build" -ScriptBlock {
        Invoke-CargoBuildWithProgress -WorkspaceRoot $WorkspaceRoot -BuildMode ([string]$strategy.mode) -BuildTarget ([string]$strategy.target)
    }

    $sourceBinary = Join-Path $WorkspaceRoot "target\release\codex.exe"
    if (-not (Test-Path -LiteralPath $sourceBinary)) {
        Fail "源码构建完成，但未找到预期产物: $sourceBinary"
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
        $requiredMissing.Add("msvc-build-tools")
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

    Write-Host "源码模式工具链检查:"
    foreach ($item in @("git", "rustup", "cargo", "rustc", "msvc-build-tools")) {
        $status = if (@($Report.required_missing) -contains $item) { "缺失" } else { "已安装" }
        Write-Host "  - ${item}: $status"
    }
    foreach ($item in @("just", "node", "npm")) {
        $status = if (@($Report.optional_missing) -contains $item) { "缺失" } else { "已安装" }
        Write-Host "  - ${item}: $status"
    }
}

function Install-RustupWithWinget {
    if (-not (Test-Command "winget")) {
        Fail "未检测到 winget，无法自动安装 rustup。"
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
                Write-Step "安装 just"
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
    if (@($report.required_missing).Count -eq 0 -and @($report.optional_missing).Count -eq 0) {
        return $report
    }

    if (Confirm-YesNo -Prompt "是否自动安装上述缺失工具？" -DefaultYes $true) {
        Auto-InstallSourceToolchain -Report $report
        $report = Detect-SourceToolchain
        Show-SourceToolchainReport -Report $report
    }

    if (@($report.required_missing).Count -gt 0) {
        Fail "源码构建所需工具链仍不完整，请先补齐缺失项后重试。"
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
        Write-Step "克隆源码仓库"
        Invoke-NativeCommandWithRetry -Label "git-clone" -FilePath "git" -ArgumentList @("clone", $RemoteUrl, $CheckoutDir)
        return
    }

    if (Test-Path -LiteralPath (Join-Path $CheckoutDir ".git")) {
        $currentRemote = (& git -C $CheckoutDir remote get-url origin 2>$null | Out-String).Trim()
        if (-not [string]::IsNullOrWhiteSpace($currentRemote) -and $currentRemote -ne $RemoteUrl) {
            if (Confirm-YesNo -Prompt "源码目录远端与当前请求不同，是否将 origin 改为 $RemoteUrl ?" -DefaultYes $false) {
                & git -C $CheckoutDir remote set-url origin $RemoteUrl
            } else {
                Fail "源码目录远端与当前请求不一致: $CheckoutDir"
            }
        }
        return
    }

    Fail "源码 checkout 目录已存在且不是 Git 仓库: $CheckoutDir"
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
        [string]$ActionLabel = "同步源码条目",
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
        "首次 clone 到新目录"
    } elseif (-not [string]::IsNullOrWhiteSpace($script:SourceCheckoutDir) -and $checkoutDir -ne $defaultCheckoutDir) {
        "使用显式指定的独立 checkout"
    } else {
        "复用现有 checkout"
    }

    if (-not $SkipPlanConfirm) {
        if (-not (Confirm-SourcePlan -ActionLabel $ActionLabel -ProfileName $ProfileName -RepoInput ([string]$repoInfo.repo_input) -CheckoutDir $checkoutDir -RefName $refName -CheckoutMode $checkoutMode -CurrentRef $(if ($existing) { [string]$existing.current_ref } else { "" }) -CurrentCheckout $(if ($existing) { [string]$existing.checkout_dir } else { "" }))) {
            Write-Info "已取消。"
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

    Write-Step "源码同步完成: $checkoutDir"
    Show-SourceResultSummary -ActionLabel $ActionLabel -ProfileName $ProfileName -RefName $refName -CheckoutDir $checkoutDir
}

function Invoke-SourceInstall {
    if (-not (Run-SourceInstallWizard)) {
        return
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $false
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "下载源码并准备工具链" -SkipPlanConfirm
}

function Invoke-SourceUpdate {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "未检测到源码记录，请先执行 hodexctl source install。"
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $existing = Get-SourceProfile -ProfileName $profileName
    if (-not $PSBoundParameters.ContainsKey("Ref")) {
        $script:SourceRef = [string]$existing.current_ref
    }
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "更新源码"
}

function Invoke-SourceRebuild {
    Fail "source rebuild 已移除；源码模式现在只保留源码下载/同步和开发工具链准备功能。"
}

function Invoke-SourceSwitch {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "未检测到源码记录，请先执行 hodexctl source install。"
    }
    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $existing = Get-SourceProfile -ProfileName $profileName
    if (-not $script:ExplicitSourceRef) {
        if ([Environment]::UserInteractive -and -not $Yes) {
            $script:SourceRef = Read-SourceRefWithChoices -RepoInput ([string]$existing.repo_input) -ProfileName $profileName -DefaultRef ([string]$existing.current_ref) -CheckoutDir ([string]$existing.checkout_dir)
            $script:ExplicitSourceRef = $true
        } else {
            Fail "source switch 需要通过 -Ref 指定目标分支、标签或提交。"
        }
    }
    Invoke-SourceBuild -ProfileName $profileName -ActivationMode "no" -ActionLabel "切换 ref 并同步源码"
}

function Invoke-SourceStatus {
    Write-Host "源码模式状态:"
    $profiles = Get-SourceProfiles
    if ($profiles.Count -eq 0) {
        Write-Host "  未安装任何源码条目"
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
            Fail "未找到源码条目: $selectedProfileName"
        }
        Write-Host "  名称: $selectedProfileName"
        Write-Host "  仓库: $([string]$profile.repo_input)"
        Write-Host "  远端: $([string]$profile.remote_url)"
        Write-Host "  目录: $([string]$profile.checkout_dir)"
        Write-Host "  Ref: $([string]$profile.current_ref) ($([string]$profile.ref_kind))"
        Write-Host "  工作区: $([string]$profile.build_workspace_root)"
        Write-Host "  安装时间: $([string]$profile.installed_at)"
        Write-Host "  最近同步: $([string]$profile.last_synced_at)"
        Write-Host "  模式: 仅管理源码 checkout 与工具链，不生成源码命令入口"
        return
    }

    foreach ($profileName in $profiles.Keys) {
        $profile = $profiles[$profileName]
        Write-Host ("  - {0} | {1} | {2} | {3} | 仅源码管理" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref, [string]$profile.checkout_dir)
    }
}

function Invoke-SourceList {
    Write-Host "源码条目列表:"
    $profiles = Get-SourceProfiles
    if ($profiles.Count -eq 0) {
        Write-Host "  当前没有已记录的源码条目"
        return
    }
    foreach ($profileName in $profiles.Keys) {
        $profile = $profiles[$profileName]
        Write-Host ("  - {0} | {1} | {2} | {3} | 仅源码管理" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref, [string]$profile.checkout_dir)
    }
}

function Invoke-SourceUninstall {
    if ($null -eq $script:State -or @((Get-SourceProfiles).Keys).Count -eq 0) {
        Fail "未检测到源码条目。"
    }

    $profileName = Resolve-SourceProfileName -RequireExisting $true
    $profile = Get-SourceProfile -ProfileName $profileName
    if ($null -eq $profile) {
        Fail "未找到源码条目: $profileName"
    }
    if (-not (Confirm-SourcePlan -ActionLabel "卸载源码条目" -ProfileName $profileName -RepoInput ([string]$profile.repo_input) -CheckoutDir ([string]$profile.checkout_dir) -RefName ([string]$profile.current_ref) -Hint "将删除源码条目记录；可选删除 checkout。" -CheckoutMode "删除现有条目资源" -CurrentRef ([string]$profile.current_ref) -CurrentCheckout ([string]$profile.checkout_dir))) {
        Write-Info "已取消。"
        return
    }

    $removeCheckout = switch ($script:SourceCheckoutPolicy) {
        "remove" { $true }
        "keep" { $false }
        default {
            if ($Yes) { $false } else { Confirm-YesNo -Prompt "是否同时删除源码目录 $([string]$profile.checkout_dir) ？" -DefaultYes $false }
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

    Write-Host "已卸载源码条目: $profileName"
    Show-SourceResultSummary -ActionLabel "卸载源码条目" -ProfileName $profileName -RefName $oldRef -CheckoutDir $oldCheckout
}

function Pause-SourceMenu {
    if ([Environment]::UserInteractive) {
        [void](Read-Host "按回车继续")
    }
}

function Show-SourceMenu {
    while ($true) {
        Clear-Host
        $profileCount = @((Get-SourceProfiles).Keys).Count
        Write-Host "源码下载 / 管理"
        Write-Host ""
        Write-Host "规则: hodex 固定指向 release；源码模式只管理 checkout 和工具链。"
        Write-Host "当前状态: 已记录源码条目 $profileCount 个"
        Write-Host ""
        Write-Host "  [源码同步]"
        Write-Host "  1. 下载源码并准备工具链             下载或复用 checkout，并检查开发工具链"
        Write-Host "  2. 更新源码                         拉取当前源码条目对应 ref 的最新代码"
        Write-Host "  3. 切换分支 / 标签 / 提交并同步      切到新的 ref 后同步源码"
        Write-Host ""
        Write-Host "  [查看与清理]"
        Write-Host "  5. 查看源码状态                     查看单个或全部源码条目"
        Write-Host "  6. 卸载源码条目                     删除条目记录，可选删 checkout"
        Write-Host "  7. 列出源码条目                     快速查看所有源码条目摘要"
        Write-Host "  q. 返回版本列表"
        Write-Host ""

        $choice = Read-Host "请选择操作（输入编号后回车）"
        $actionLabel = ""
        $actionHint = ""
        $action = $null
        switch ($choice) {
            "1" {
                $actionLabel = "下载源码并准备工具链"
                $actionHint = "接下来会确认仓库、checkout 目录、工具链和源码记录名。"
                $action = { Invoke-SourceInstall }
            }
            "2" {
                $actionLabel = "更新源码"
                $actionHint = "将拉取当前源码条目的最新代码并同步 checkout。"
                $action = { Invoke-SourceUpdate }
            }
            "3" {
                $actionLabel = "切换 ref 并同步源码"
                $actionHint = "接下来需要指定新的 branch / tag / commit。"
                $action = { Invoke-SourceSwitch }
            }
            "5" {
                $actionLabel = "查看源码状态"
                $actionHint = "将展示源码条目的详细状态信息。"
                $action = { Invoke-SourceStatus }
            }
            "6" {
                $actionLabel = "卸载源码条目"
                $actionHint = "将删除选中条目的记录；可选删除源码目录。"
                $action = { Invoke-SourceUninstall }
            }
            "7" {
                $actionLabel = "列出源码条目"
                $actionHint = "将展示当前所有源码条目摘要。"
                $action = { Invoke-SourceList }
            }
            "q" { return }
            "Q" { return }
            default {
                Write-WarnLine "请输入 1、2、3、5、6、7 或 q。"
                Pause-SourceMenu
                continue
            }
        }

        Clear-Host
        Write-Host "源码下载 / 管理"
        Write-Host ""
        Write-Host "正在进入: $actionLabel"
        Write-Host "提示: $actionHint"
        Write-Host ""
        Show-SourceMenuActionPreview -Choice $choice
        Write-Host ""

        try {
            & $action
            Write-Host ""
            Write-Host "操作完成: $actionLabel"
        } catch {
            Write-Host ""
            Write-WarnLine "操作失败: $actionLabel"
            Write-WarnLine ("失败分类: " + (Get-SourceErrorCategory -Message $_.Exception.Message))
            Write-WarnLine $_.Exception.Message
        }
        Pause-SourceMenu
    }
}

function Invoke-List {
    $items = @(Get-MatchingReleases)
    if ($items.Count -eq 0) {
        Fail "当前平台没有可用的 release 资产。"
    }

    $currentVersion = ""
    if ($script:State) {
        $currentVersion = [string]$script:State.installed_version
    }

    if (-not [Environment]::UserInteractive) {
        Write-Host "当前平台可下载版本: $script:PlatformLabel"
        Write-Host ("{0,3}. {1,-12} {2}" -f 0, "源码模式", "源码下载 / 管理")
        for ($i = 0; $i -lt $items.Count; $i++) {
            $item = $items[$i]
            Write-Host ("{0,3}. {1,-12} {2} {3}" -f ($i + 1), [string]$item.version, [string]$item.published_at, [string]$item.asset.name)
        }
        return
    }

    while ($true) {
        Write-Host ""
        Write-Host "可下载版本 ($script:PlatformLabel):"
        Write-Host ("{0,3}. {1,-12} {2}" -f 0, "源码模式", "源码下载 / 管理")
        for ($i = 0; $i -lt $items.Count; $i++) {
            $item = $items[$i]
            $marker = ""
            if (-not [string]::IsNullOrWhiteSpace($currentVersion) -and [string]$item.version -eq $currentVersion) {
                $marker = " [已安装]"
            }
            Write-Host ("{0,3}. {1,-12} {2} {3}{4}" -f ($i + 1), [string]$item.version, [string]$item.published_at, [string]$item.asset.name, $marker)
        }

        $choice = Read-Host "输入编号查看更新日志，输入 0 进入源码模式管理，直接回车退出"
        if ([string]::IsNullOrWhiteSpace($choice)) {
            return
        }

        $index = 0
        if (-not [int]::TryParse($choice, [ref]$index) -or $index -lt 0 -or $index -gt $items.Count) {
            Write-WarnLine "请输入有效编号。"
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
            Write-Host " AI总结 " -ForegroundColor Black -BackgroundColor Yellow -NoNewline
            Write-Host " 输入 a 调用 hodex/codex 对当前 changelog 做 AI 总结"
            $action = Read-Host "操作: [a]AI总结（hodex/codex） [i]安装 [d]下载到 $script:DownloadRoot [b]返回列表 [q]退出"
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
                    Invoke-InstallLike -RequestedVersion ([string]$selected.release_tag) -ActionLabel "安装"
                    return
                }
                "d" {
                    Invoke-Download -RequestedVersion ([string]$selected.release_tag)
                    return
                }
                "b" { break }
                "" { break }
                "q" { return }
                default { Write-WarnLine "请输入 a、i、d、b 或 q。" }
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
    Write-Step "检测到平台: $script:PlatformLabel"
    Write-Step "命中 release: $(if ([string]::IsNullOrWhiteSpace($releaseName)) { "<unknown>" } else { $releaseName }) ($(if ([string]::IsNullOrWhiteSpace($releaseTag)) { "<unknown>" } else { $releaseTag }))"
    Write-Step "下载资产: $([string]$asset.name)"

    Select-CommandDir

    $binaryDir = Join-Path $script:StateRoot "bin"
    $binaryPath = Join-Path $binaryDir "codex.exe"
    $controllerPath = Join-Path $script:StateRoot "libexec\hodexctl.ps1"
    Ensure-DirWritable $binaryDir
    Ensure-DirWritable (Split-Path -Parent $controllerPath)

    Write-Step "安装目标二进制: $binaryPath"
    Write-Step "命令目录: $script:CurrentCommandDir"

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

    Write-Step "安装完成: $binaryPath"
    & $binaryPath --version

    switch ($script:PathUpdateMode) {
        "added" {
            Write-Info "已写入用户 PATH。"
        }
        "configured" {
            Write-Info "已刷新用户 PATH。"
        }
        "already" {
            Write-Info "命令目录已在 PATH 中: $script:CurrentCommandDir"
        }
        "disabled" {
            Write-WarnLine "命令目录未自动写入 PATH，请手动加入: $script:CurrentCommandDir"
        }
        "user-skipped" {
            Write-WarnLine "命令目录未自动写入 PATH，请手动加入: $script:CurrentCommandDir"
        }
    }

    Write-Info "下一步: 运行 'hodex --version' 验证安装"
    Write-Info "管理命令: 'hodexctl status' / 'hodexctl list'"
}

function Invoke-Uninstall {
    if (-not $script:State) {
        Fail "未检测到 hodex 安装状态，无需卸载。"
    }
    if ([string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
        Fail "未检测到正式版 release 安装；如需卸载源码版，请使用 hodexctl source uninstall。"
    }

    Write-Step "卸载正式版 Hodex"

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
        Write-Info "已删除正式版二进制、包装器和安装状态。"
    } else {
        Write-Info "已删除正式版二进制；源码条目与管理脚本已保留。"
    }
}

function Invoke-Status {
    Write-Host "平台: $script:PlatformLabel"
    Write-Host "状态目录: $script:StateRoot"

    if ($script:State) {
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.binary_path)) {
            Write-Host "正式版安装状态: 已安装"
            Write-Host "版本: $([string]$script:State.installed_version)"
            Write-Host "Release: $([string]$script:State.release_name) ($([string]$script:State.release_tag))"
            Write-Host "资产: $([string]$script:State.asset_name)"
            Write-Host "二进制: $([string]$script:State.binary_path)"
            if ($env:OS -eq "Windows_NT") {
                $helpersComplete = Test-ReleaseHelpersComplete -BinaryPath ([string]$script:State.binary_path)
                Write-Host ("Windows 运行组件: " + $(if ($helpersComplete) { "完整" } else { "缺失" }))
                foreach ($helperPath in @(Get-ReleaseHelperPaths -BinaryPath ([string]$script:State.binary_path))) {
                    $helperName = Split-Path -Leaf $helperPath
                    Write-Host ("  - {0}: {1}" -f $helperName, $(if (Test-Path -LiteralPath $helperPath) { "已安装" } else { "缺失" }))
                }
                if (-not $helpersComplete) {
                    Write-WarnLine "当前 Windows 安装不完整，请重新执行 hodexctl install 或 hodexctl upgrade。"
                }
            }
        } else {
            Write-Host "正式版安装状态: 未安装"
        }
        Write-Host "命令目录: $([string]$script:State.command_dir)"
        Write-Host "管理脚本副本: $([string]$script:State.controller_path)"
        Write-Host "PATH 处理: $([string]$script:State.path_update_mode)"
        if (-not [string]::IsNullOrWhiteSpace([string]$script:State.path_profile)) {
            Write-Host "PATH 作用域: $([string]$script:State.path_profile)"
        }
        Write-Host "Node 处理选择: $([string]$script:State.node_setup_choice)"
        Write-Host "安装时间: $([string]$script:State.installed_at)"
        Write-Host "hodex 包装器: $(Join-Path ([string]$script:State.command_dir) 'hodex.cmd')"
        Write-Host "hodexctl 包装器: $(Join-Path ([string]$script:State.command_dir) 'hodexctl.cmd')"
        Write-Host "受管 hodex 指向: $(Get-ActiveHodexAlias)"
        Write-Host "源码条目数量: $(@((Get-SourceProfiles).Keys).Count)"
        foreach ($profileName in ((Get-SourceProfiles).Keys)) {
            $profile = Get-SourceProfile -ProfileName $profileName
            Write-Host ("源码条目: {0} | {1} | {2} | 仅源码管理" -f $profileName, [string]$profile.repo_input, [string]$profile.current_ref)
        }
    } else {
        Write-Host "正式版安装状态: 未安装"
        Write-Host "源码条目数量: 0"
    }

    $hodexCmd = Get-Command hodex -ErrorAction SilentlyContinue
    if ($hodexCmd) {
        Write-Host "PATH 中的 hodex: $($hodexCmd.Source)"
    } else {
        Write-Host "PATH 中的 hodex: 未找到"
    }

    $codexCmd = Get-Command codex -ErrorAction SilentlyContinue
    if ($codexCmd) {
        Write-Host "PATH 中的 codex: $($codexCmd.Source)"
    } else {
        Write-Host "PATH 中的 codex: 未找到"
    }

    $nodeCmd = Get-Command node -ErrorAction SilentlyContinue
    if ($nodeCmd) {
        Write-Host "Node.js: $(& $nodeCmd.Source -v)"
    } else {
        Write-Host "Node.js: 未安装"
    }
}

function Invoke-Relink {
    if (-not $script:State) {
        Fail "未检测到 hodex 安装状态，无法重建链接。"
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
    Write-Info "已重建正式版与管理脚本包装器到: $script:CurrentCommandDir"
}

if (-not $env:HODEXCTL_SKIP_MAIN) {
    Ensure-LocalToolPaths
    Normalize-Parameters
    if ($script:RawSourceHelpRequest -or ($script:RequestedCommand -eq "source" -and $script:SourceAction -eq "help")) {
        Show-SourceUsage
        exit 0
    }
    Detect-Platform
    $script:State = Load-State

    switch ($script:RequestedCommand) {
        "install" {
            Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "安装"
        }
        "upgrade" {
            Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "升级"
        }
        "download" {
            Invoke-Download -RequestedVersion $script:RequestedVersion
        }
        "downgrade" {
            Invoke-InstallLike -RequestedVersion $script:RequestedVersion -ActionLabel "降级"
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
        default {
            Fail "未知命令: $script:RequestedCommand"
        }
    }
}
