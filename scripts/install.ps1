$ErrorActionPreference = "Stop"

$repo = if ($env:CODEX_REPO) { $env:CODEX_REPO } else { "stellarlinkco/codex" }
$installDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $HOME ".local\\bin" }

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($arch) {
  "X64" { $target = "x86_64-pc-windows-msvc" }
  "Arm64" { $target = "aarch64-pc-windows-msvc" }
  default { throw "Unsupported Windows architecture: $arch" }
}

$asset = "codex-$target.exe.zip"
$url = "https://github.com/$repo/releases/latest/download/$asset"

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
$zipPath = Join-Path $tempRoot $asset
$extractDir = Join-Path $tempRoot "extract"

try {
  New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
  Invoke-WebRequest -Uri $url -OutFile $zipPath
  Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

  New-Item -ItemType Directory -Path $installDir -Force | Out-Null

  $mainExe = Join-Path $extractDir "codex-$target.exe"
  if (!(Test-Path $mainExe)) {
    throw "Expected $mainExe in archive."
  }

  Copy-Item $mainExe (Join-Path $installDir "codex.exe") -Force

  foreach ($helper in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
    $src = Join-Path $extractDir $helper
    if (Test-Path $src) {
      Copy-Item $src (Join-Path $installDir $helper) -Force
    }
  }

  Write-Host "Installed codex.exe to $installDir"
  Write-Host "Ensure $installDir is on your PATH."
} finally {
  if (Test-Path $tempRoot) {
    Remove-Item -Recurse -Force $tempRoot
  }
}
