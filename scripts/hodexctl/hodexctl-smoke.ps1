param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$controllerPath = Join-Path $scriptDir "hodexctl.ps1"
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("hodexctl-smoke-" + [System.Guid]::NewGuid().ToString("N"))

function Assert-Contains {
    param(
        [string]$Text,
        [string]$Expected
    )

    if ($Text -notlike "*$Expected*") {
        throw "未找到预期内容: $Expected"
    }
}

function Get-Runner {
    if (Get-Command pwsh -ErrorAction SilentlyContinue) {
        return "pwsh"
    }

    return "powershell"
}

function New-FakeSummaryAgent {
    param(
        [string]$BinDir,
        [string]$Name,
        [bool]$SupportsExec,
        [string]$ResultText,
        [string]$ArgsFile,
        [string]$PromptFile = ""
    )

    if ($env:OS -eq "Windows_NT") {
        $path = Join-Path $BinDir ($Name + ".cmd")
        $scriptPath = Join-Path $BinDir ($Name + ".py")
        $content = @"
@echo off
python "%~dp0$Name.py" %*
"@
        Set-Content -LiteralPath $path -Value $content -Encoding ASCII
        $scriptContent = @"
import sys
from pathlib import Path

supports_exec = $([int]$SupportsExec)
args_path = Path(r'''$ArgsFile''')
prompt_path = r'''$PromptFile'''
args = sys.argv[1:]

if len(args) >= 2 and args[0] == "exec" and args[1] == "--help":
    raise SystemExit(0 if supports_exec else 1)

args_path.write_text(" ".join(args), encoding="utf-8")
stdin_text = sys.stdin.buffer.read().decode("utf-8")
if prompt_path:
    Path(prompt_path).write_text(stdin_text, encoding="utf-8")

print(r'''{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"$ResultText"}}''')
"@
        Set-Content -LiteralPath $scriptPath -Value $scriptContent -Encoding UTF8
        return
    }

    $path = Join-Path $BinDir $Name
    $content = @"
#!/usr/bin/env bash
set -euo pipefail
if [[ "`${1:-}" == "exec" && "`${2:-}" == "--help" ]]; then
  $(if ($SupportsExec) { "exit 0" } else { "exit 1" })
fi
printf '%s\n' "`$*" >"$ArgsFile"
$(if ([string]::IsNullOrWhiteSpace($PromptFile)) { "cat >/dev/null" } else { "cat >""$PromptFile""" })
printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"$ResultText"}}'
"@
        Set-Content -LiteralPath $path -Value $content -Encoding UTF8
        & chmod +x $path
}

try {
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    $stateDir = Join-Path $tempRoot "state"
    $commandDir = Join-Path $tempRoot "commands"
    $releaseStateDir = Join-Path $tempRoot "release-state"
    $releaseCommandDir = Join-Path $tempRoot "release-commands"
    $sourceRepoDir = Join-Path $tempRoot "source-repo"
    $sourceCheckoutDir = Join-Path $tempRoot "source-checkout"
    $summaryBin = Join-Path $tempRoot "summary-bin"
    $summaryArgs = Join-Path $tempRoot "summary-args.txt"
    $summaryPrompt = Join-Path $tempRoot "summary-prompt.txt"
    $summaryFallbackBin = Join-Path $tempRoot "summary-fallback-bin"
    $summaryFallbackArgs = Join-Path $tempRoot "summary-fallback-args.txt"

    Write-Host "==> 检查 PowerShell 语法"
    [void][System.Management.Automation.Language.Parser]::ParseFile($controllerPath, [ref]$null, [ref]$null)

    Write-Host "==> 检查帮助输出"
    $runner = Get-Runner
    $helpOutput = (& $runner -NoProfile -File $controllerPath -Help 2>&1 | Out-String)
    Assert-Contains -Text $helpOutput -Expected "用法:"
    Assert-Contains -Text $helpOutput -Expected "hodexctl list"
    Assert-Contains -Text $helpOutput -Expected ".\hodexctl.ps1 install"
    Assert-Contains -Text $helpOutput -Expected "source <action>"

    Write-Host "==> 检查源码模式帮助输出"
    $sourceHelpOutput = (& $runner -NoProfile -File $controllerPath source help 2>&1 | Out-String)
    Assert-Contains -Text $sourceHelpOutput -Expected "源码模式用法:"
    Assert-Contains -Text $sourceHelpOutput -Expected "install                下载源码并准备工具链（不接管 hodex）"
    Assert-Contains -Text $sourceHelpOutput -Expected "指定源码记录名（工作区标识），默认 codex-source"

    Write-Host "==> 检查 source/list 子命令 help 语义"
    $sourceInstallHelpOutput = (& $runner -NoProfile -File $controllerPath source install -Help 2>&1 | Out-String)
    $listHelpOutput = (& $runner -NoProfile -File $controllerPath list -Help 2>&1 | Out-String)
    Assert-Contains -Text $sourceInstallHelpOutput -Expected "源码模式用法:"
    Assert-Contains -Text $listHelpOutput -Expected "版本列表用法:"
    Assert-Contains -Text $listHelpOutput -Expected "更新日志页操作:"

    Write-Host "==> 检查 release changelog 总结优先调用 hodex"
    New-Item -ItemType Directory -Path $summaryBin -Force | Out-Null
    New-FakeSummaryAgent -BinDir $summaryBin -Name "hodex" -SupportsExec $true -ResultText "这是 hodex 总结结果" -ArgsFile $summaryArgs -PromptFile $summaryPrompt
    New-FakeSummaryAgent -BinDir $summaryBin -Name "codex" -SupportsExec $true -ResultText "不应调用 codex" -ArgsFile (Join-Path $tempRoot "unexpected-codex.txt")
    $originalPath = $env:PATH
    try {
        $env:PATH = $summaryBin + [System.IO.Path]::PathSeparator + $env:PATH
        $env:HODEXCTL_SKIP_MAIN = "1"
        . $controllerPath
        Remove-Item Env:\HODEXCTL_SKIP_MAIN -ErrorAction SilentlyContinue
        $releaseInfo = [pscustomobject]@{
            version = "1.2.3"
            release_name = "1.2.3"
            release_tag = "v1.2.3"
            published_at = "2026-03-09T00:00:00Z"
            html_url = "https://example.invalid/releases/v1.2.3"
            asset = [pscustomobject]@{ name = "codex-x86_64-apple-darwin" }
            release = [pscustomobject]@{ body = "- add feature A`n- fix bug B" }
        }
        $summaryResult = Invoke-ReleaseSummary -ReleaseInfo $releaseInfo
        if (-not $summaryResult) { throw "Invoke-ReleaseSummary 未成功调用 hodex" }
        Assert-Contains -Text (Get-Content -LiteralPath $summaryArgs -Raw) -Expected "exec --skip-git-repo-check --color never --json -"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "版本: 1.2.3"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "完整 changelog:"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "新增功能"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "修复内容"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "破坏性变更 / 迁移要求"
        Assert-Contains -Text (Get-Content -LiteralPath $summaryPrompt -Raw) -Expected "- add feature A"
    } finally {
        $env:PATH = $originalPath
        Remove-Item Env:\HODEXCTL_SKIP_MAIN -ErrorAction SilentlyContinue
    }

    Write-Host "==> 检查 release changelog 总结会回退到 codex"
    New-Item -ItemType Directory -Path $summaryFallbackBin -Force | Out-Null
    New-FakeSummaryAgent -BinDir $summaryFallbackBin -Name "hodex" -SupportsExec $false -ResultText "不应调用 hodex" -ArgsFile (Join-Path $tempRoot "unsupported-hodex.txt")
    New-FakeSummaryAgent -BinDir $summaryFallbackBin -Name "codex" -SupportsExec $true -ResultText "这是 codex 回退总结结果" -ArgsFile $summaryFallbackArgs
    $originalPath = $env:PATH
    try {
        $env:PATH = $summaryFallbackBin + [System.IO.Path]::PathSeparator + $env:PATH
        $env:HODEXCTL_SKIP_MAIN = "1"
        . $controllerPath
        Remove-Item Env:\HODEXCTL_SKIP_MAIN -ErrorAction SilentlyContinue
        $releaseInfo = [pscustomobject]@{
            version = "2.0.0"
            release_name = "2.0.0"
            release_tag = "v2.0.0"
            published_at = "2026-03-09T00:00:00Z"
            html_url = "https://example.invalid/releases/v2.0.0"
            asset = [pscustomobject]@{ name = "codex-x86_64-apple-darwin" }
            release = [pscustomobject]@{ body = "fallback smoke" }
        }
        $summaryResult = Invoke-ReleaseSummary -ReleaseInfo $releaseInfo
        if (-not $summaryResult) { throw "Invoke-ReleaseSummary 未成功回退到 codex" }
        Assert-Contains -Text (Get-Content -LiteralPath $summaryFallbackArgs -Raw) -Expected "exec --skip-git-repo-check --color never --json -"
    } finally {
        $env:PATH = $originalPath
        Remove-Item Env:\HODEXCTL_SKIP_MAIN -ErrorAction SilentlyContinue
    }

    Write-Host "==> 检查源码模式拒绝接管 hodex"
    $activateOutput = (& $runner -NoProfile -File $controllerPath source install -Activate 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0) {
        throw "源码模式不应接受 -Activate"
    }
    Assert-Contains -Text $activateOutput -Expected "源码模式不允许接管 hodex"

    if ($env:OS -eq "Windows_NT") {
        Write-Host "==> 检查未安装状态输出"
        $statusOutput = (& $runner -NoProfile -File $controllerPath status -StateDir $stateDir 2>&1 | Out-String)
        Assert-Contains -Text $statusOutput -Expected "正式版安装状态: 未安装"
        Assert-Contains -Text $statusOutput -Expected ("状态目录: " + $stateDir)

        Write-Host "==> 检查 release-only 安装与卸载清理"
        $assetsDir = Join-Path $tempRoot 'release-assets'
        $releaseDir = Join-Path $assetsDir 'latest\download'
        $brokenReleaseDir = Join-Path $tempRoot 'broken-release-assets\latest\download'
        New-Item -ItemType Directory -Path $releaseDir -Force | Out-Null
        New-Item -ItemType Directory -Path $brokenReleaseDir -Force | Out-Null
        New-Item -ItemType Directory -Path $releaseStateDir -Force | Out-Null
        New-Item -ItemType Directory -Path $releaseCommandDir -Force | Out-Null

        $sourceExe = (Get-Command pwsh).Source
        Copy-Item $sourceExe (Join-Path $releaseDir "codex-x86_64-pc-windows-msvc.exe") -Force
        Copy-Item $sourceExe (Join-Path $releaseDir "codex-command-runner.exe") -Force
        Copy-Item $sourceExe (Join-Path $releaseDir "codex-windows-sandbox-setup.exe") -Force
        Copy-Item $sourceExe (Join-Path $brokenReleaseDir "codex-x86_64-pc-windows-msvc.exe") -Force

        $zipPath = Join-Path $releaseDir "codex-x86_64-pc-windows-msvc.exe.zip"
        if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
        Push-Location $releaseDir
        Compress-Archive -Path @("codex-x86_64-pc-windows-msvc.exe", "codex-command-runner.exe", "codex-windows-sandbox-setup.exe") -DestinationPath $zipPath -Force
        Pop-Location

        $brokenZipPath = Join-Path $brokenReleaseDir "codex-x86_64-pc-windows-msvc.exe.zip"
        if (Test-Path $brokenZipPath) { Remove-Item -Force $brokenZipPath }
        Push-Location $brokenReleaseDir
        Compress-Archive -Path @("codex-x86_64-pc-windows-msvc.exe") -DestinationPath $brokenZipPath -Force
        Pop-Location

        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
        $listener.Start()
        $releasePort = $listener.LocalEndpoint.Port
        $listener.Stop()
        $releaseServer = Start-Process -FilePath python -ArgumentList @("-m", "http.server", "$releasePort", "--bind", "127.0.0.1", "--directory", "$assetsDir") -PassThru -NoNewWindow
        $brokenListener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
        $brokenListener.Start()
        $brokenReleasePort = $brokenListener.LocalEndpoint.Port
        $brokenListener.Stop()
        $brokenServer = Start-Process -FilePath python -ArgumentList @("-m", "http.server", "$brokenReleasePort", "--bind", "127.0.0.1", "--directory", (Join-Path $tempRoot 'broken-release-assets')) -PassThru -NoNewWindow
        try {
            for ($i = 0; $i -lt 50; $i++) {
                try {
                    Invoke-WebRequest -Uri "http://127.0.0.1:$releasePort/" -UseBasicParsing | Out-Null
                    break
                } catch {
                    Start-Sleep -Milliseconds 100
                }
            }
            for ($i = 0; $i -lt 50; $i++) {
                try {
                    Invoke-WebRequest -Uri "http://127.0.0.1:$brokenReleasePort/" -UseBasicParsing | Out-Null
                    break
                } catch {
                    Start-Sleep -Milliseconds 100
                }
            }

            $env:HODEX_RELEASE_BASE_URL = "http://127.0.0.1:$releasePort"
            $releaseInstallOutput = (& $runner -NoProfile -File $controllerPath install -Yes -NoPathUpdate -StateDir $releaseStateDir -CommandDir $releaseCommandDir 2>&1 | Out-String)
            Assert-Contains -Text $releaseInstallOutput -Expected "安装完成"
            $releaseStatusOutput = (& $runner -NoProfile -File $controllerPath status -StateDir $releaseStateDir 2>&1 | Out-String)
            Assert-Contains -Text $releaseStatusOutput -Expected "Windows 运行组件: 完整"
            Assert-Contains -Text $releaseStatusOutput -Expected "codex-command-runner.exe: 已安装"
            Assert-Contains -Text $releaseStatusOutput -Expected "codex-windows-sandbox-setup.exe: 已安装"
            if (!(Test-Path (Join-Path $releaseCommandDir "hodex.cmd"))) { throw "release hodex.cmd 未生成" }
            if (!(Test-Path (Join-Path $releaseCommandDir "hodexctl.cmd"))) { throw "release hodexctl.cmd 未生成" }
            if (!(Test-Path (Join-Path $releaseStateDir "bin\codex-command-runner.exe"))) { throw "release codex-command-runner.exe 未生成" }
            if (!(Test-Path (Join-Path $releaseStateDir "bin\codex-windows-sandbox-setup.exe"))) { throw "release codex-windows-sandbox-setup.exe 未生成" }

            $releaseUninstallOutput = (& $runner -NoProfile -File $controllerPath uninstall -StateDir $releaseStateDir 2>&1 | Out-String)
            Assert-Contains -Text $releaseUninstallOutput -Expected "已删除正式版二进制、包装器和安装状态。"
            if (Test-Path (Join-Path $releaseCommandDir "hodex.cmd")) { throw "release hodex.cmd 未删除" }
            if (Test-Path (Join-Path $releaseCommandDir "hodexctl.cmd")) { throw "release hodexctl.cmd 未删除" }
            if (Test-Path (Join-Path $releaseStateDir "state.json")) { throw "release state.json 未删除" }
            if (Test-Path (Join-Path $releaseStateDir "bin\codex-command-runner.exe")) { throw "release codex-command-runner.exe 未删除" }
            if (Test-Path (Join-Path $releaseStateDir "bin\codex-windows-sandbox-setup.exe")) { throw "release codex-windows-sandbox-setup.exe 未删除" }

            Write-Host "==> 检查 Windows 缺失 helper 时严格失败"
            $env:HODEX_RELEASE_BASE_URL = "http://127.0.0.1:$brokenReleasePort"
            $brokenInstallOutput = (& $runner -NoProfile -File $controllerPath install -Yes -NoPathUpdate -StateDir (Join-Path $tempRoot 'broken-state') -CommandDir (Join-Path $tempRoot 'broken-command') 2>&1 | Out-String)
            if ($LASTEXITCODE -eq 0) { throw "缺失 helper 的 Windows release 安装不应成功" }
            Assert-Contains -Text $brokenInstallOutput -Expected "当前 Windows release 资产缺少必需 helper"
        } finally {
            try { if ($releaseServer -and -not $releaseServer.HasExited) { $releaseServer.Kill() } } catch {}
            try { if ($brokenServer -and -not $brokenServer.HasExited) { $brokenServer.Kill() } } catch {}
            Remove-Item Env:\HODEX_RELEASE_BASE_URL -ErrorAction SilentlyContinue
        }

        Write-Host "==> 检查源码空状态输出"
        $sourceStatusOutput = (& $runner -NoProfile -File $controllerPath source status -StateDir $stateDir 2>&1 | Out-String)
        $sourceListOutput = (& $runner -NoProfile -File $controllerPath source list -StateDir $stateDir 2>&1 | Out-String)
        Assert-Contains -Text $sourceStatusOutput -Expected "未安装任何源码条目"
        Assert-Contains -Text $sourceListOutput -Expected "当前没有已记录的源码条目"

        Write-Host "==> 检查源码模式本地闭环同步"
        if ((Get-Command git -ErrorAction SilentlyContinue) -and (Get-Command cargo -ErrorAction SilentlyContinue) -and (Get-Command rustc -ErrorAction SilentlyContinue)) {
            New-Item -ItemType Directory -Path (Join-Path $sourceRepoDir "src") -Force | Out-Null
            New-Item -ItemType Directory -Path $commandDir -Force | Out-Null

            Set-Content -Path (Join-Path $sourceRepoDir "Cargo.toml") -Value @"
[package]
name = "codex-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "codex"
path = "src/main.rs"
"@
            Set-Content -Path (Join-Path $sourceRepoDir "src/main.rs") -Value @"
fn main() {
    println!("smoke-build 0.1.0");
}
"@

            & git -C $sourceRepoDir init -b main | Out-Null
            & git -C $sourceRepoDir config user.name "hodexctl-smoke" | Out-Null
            & git -C $sourceRepoDir config user.email "hodexctl-smoke@example.com" | Out-Null
            & git -C $sourceRepoDir add Cargo.toml src/main.rs | Out-Null
            & git -C $sourceRepoDir commit -m "init smoke repo" | Out-Null
            & git -C $sourceRepoDir tag smoke-tag | Out-Null

            $sourceInstallOutput = (& $runner -NoProfile -File $controllerPath source install -Yes -NoPathUpdate -StateDir $stateDir -CommandDir $commandDir -GitUrl $sourceRepoDir -Profile smoke-source -Ref main -CheckoutDir $sourceCheckoutDir 2>&1 | Out-String)
            Assert-Contains -Text $sourceInstallOutput -Expected "结果摘要"
            Assert-Contains -Text $sourceInstallOutput -Expected "源码记录名: smoke-source"
            if (!(Test-Path (Join-Path $sourceCheckoutDir ".git"))) { throw "源码 checkout 未生成" }
            if (!(Test-Path (Join-Path $commandDir "hodexctl.cmd"))) { throw "hodexctl 包装器未生成" }
            $checkoutHead = (& git -C $sourceCheckoutDir rev-parse HEAD | Out-String).Trim()
            $repoHead = (& git -C $sourceRepoDir rev-parse HEAD | Out-String).Trim()
            if ($checkoutHead -ne $repoHead) { throw "源码安装后 checkout HEAD 不一致" }

            $sourceStatusOutput = (& $runner -NoProfile -File $controllerPath source status -Yes -NoPathUpdate -StateDir $stateDir -CommandDir $commandDir 2>&1 | Out-String)
            Assert-Contains -Text $sourceStatusOutput -Expected "名称: smoke-source"
            Assert-Contains -Text $sourceStatusOutput -Expected "模式: 仅管理源码 checkout 与工具链，不生成源码命令入口"

            Set-Content -Path (Join-Path $sourceRepoDir "src/main.rs") -Value @"
fn main() {
    println!("smoke-build 0.2.0");
}
"@
            & git -C $sourceRepoDir add src/main.rs | Out-Null
            & git -C $sourceRepoDir commit -m "update smoke repo" | Out-Null

            $sourceUpdateOutput = (& $runner -NoProfile -File $controllerPath source update -Yes -NoPathUpdate -StateDir $stateDir -CommandDir $commandDir 2>&1 | Out-String)
            Assert-Contains -Text $sourceUpdateOutput -Expected "更新源码"
            $checkoutHead = (& git -C $sourceCheckoutDir rev-parse HEAD | Out-String).Trim()
            $repoHead = (& git -C $sourceRepoDir rev-parse HEAD | Out-String).Trim()
            if ($checkoutHead -ne $repoHead) { throw "源码更新后 checkout HEAD 不一致" }

            & git -C $sourceRepoDir checkout -b feature-smoke-switch | Out-Null
            $env:HODEXCTL_SKIP_MAIN = "1"
            . $controllerPath
            Remove-Item Env:\HODEXCTL_SKIP_MAIN -ErrorAction SilentlyContinue
            $refCandidates = @(Get-SourceRefCandidates -RepoInput $sourceRepoDir -ProfileName "smoke-source" -DefaultRef "main" -CheckoutDir $sourceCheckoutDir)
            if ($refCandidates -notcontains "feature-smoke-switch") { throw "真实 Git refs 未进入源码 ref 候选列表" }
            if ($refCandidates -contains "smoke-tag") { throw "branch 候选列表不应默认混入 tag" }

            $sourceSwitchOutput = (& $runner -NoProfile -File $controllerPath source switch -Yes -NoPathUpdate -StateDir $stateDir -CommandDir $commandDir -Ref feature-smoke-switch 2>&1 | Out-String)
            Assert-Contains -Text $sourceSwitchOutput -Expected "切换 ref 并同步源码"
            $currentBranch = (& git -C $sourceCheckoutDir rev-parse --abbrev-ref HEAD | Out-String).Trim()
            if ($currentBranch -ne "feature-smoke-switch") { throw "源码切换 ref 后分支不正确" }

            $sourceRebuildOutput = (& $runner -NoProfile -File $controllerPath source rebuild -Yes -NoPathUpdate -StateDir $stateDir -CommandDir $commandDir 2>&1 | Out-String)
            Assert-Contains -Text $sourceRebuildOutput -Expected "source rebuild 已移除"

            $sourceUninstallOutput = (& $runner -NoProfile -File $controllerPath source uninstall -Yes -NoPathUpdate -KeepCheckout -StateDir $stateDir -CommandDir $commandDir 2>&1 | Out-String)
            Assert-Contains -Text $sourceUninstallOutput -Expected "卸载源码条目"
            if (!(Test-Path $sourceCheckoutDir)) { throw "源码 checkout 不应被删除" }
            if (Test-Path (Join-Path $commandDir "hodexctl.cmd")) { throw "hodexctl 包装器未删除" }
        } else {
            Write-Host "==> 环境缺少 git/cargo/rustc，跳过源码闭环集成测试"
        }
    } else {
        Write-Host "==> 非 Windows 环境，跳过运行态检查"
    }

    Write-Host "==> Smoke 测试通过"
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
