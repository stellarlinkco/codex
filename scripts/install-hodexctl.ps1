$ErrorActionPreference = "Stop"

$repo = if ($env:HODEXCTL_REPO) { $env:HODEXCTL_REPO } elseif ($env:CODEX_REPO) { $env:CODEX_REPO } else { "stellarlinkco/codex" }
$controllerUrlBase = if ($env:HODEX_CONTROLLER_URL_BASE) { $env:HODEX_CONTROLLER_URL_BASE.TrimEnd('/') } else { "https://raw.githubusercontent.com" }
$stateDir = if ($env:HODEX_STATE_DIR) { $env:HODEX_STATE_DIR } else { $null }
$commandDir = if ($env:HODEX_COMMAND_DIR) { $env:HODEX_COMMAND_DIR } elseif ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { $null }
$controllerUrl = "$controllerUrlBase/$repo/main/scripts/hodexctl/hodexctl.ps1"

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
$controllerPath = Join-Path $tempRoot "hodexctl.ps1"

try {
  New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
  Write-Host "==> 下载 hodexctl 管理脚本"
  Invoke-WebRequest -Uri $controllerUrl -OutFile $controllerPath

  $argumentList = @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", $controllerPath,
    "manager-install",
    "-Yes",
    "-Repo", $repo
  )

  if ($stateDir) {
    $argumentList += @("-StateDir", $stateDir)
  }

  if ($commandDir) {
    $argumentList += @("-CommandDir", $commandDir)
  }

  if ($env:HODEXCTL_NO_PATH_UPDATE -eq "1") {
    $argumentList += "-NoPathUpdate"
  }

  if ($env:GITHUB_TOKEN) {
    $argumentList += @("-GitHubToken", $env:GITHUB_TOKEN)
  }

  $runner = if (Get-Command pwsh -ErrorAction SilentlyContinue) { "pwsh" } else { "powershell" }
  Write-Host "==> 启动 hodexctl 首次安装"
  & $runner @argumentList
  if ($LASTEXITCODE -ne 0) {
    throw "hodexctl manager-install 失败，退出码: $LASTEXITCODE"
  }
} finally {
  if (Test-Path $tempRoot) {
    Remove-Item -Recurse -Force $tempRoot
  }
}
