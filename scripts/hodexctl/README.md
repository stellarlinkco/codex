## Hodexctl 使用说明

`hodexctl` 用来独立管理 `hodex` 的 release 安装，以及源码下载 / 同步 / 工具链准备；不会覆盖现有 `codex`。

### 固定规则

- `hodex` 只用于 release 版本管理。
- `hodexctl source ...` 只负责源码下载、同步和工具链准备。
- 源码模式不会编译、部署，也不会接管 `hodex`。
- `codex` 原有安装体系不受 `hodexctl` 卸载影响。

### 适用平台

- macOS
- Linux
- WSL
- Windows PowerShell

Linux / WSL 的 release 资产选择顺序为 `legacy-musl` -> `musl` -> `gnu`。

### 快速开始

#### macOS / Linux / WSL

```bash
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.sh | bash
```

#### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/install-hodexctl.ps1 | iex
```

安装脚本会自动完成 `hodexctl` 自身安装，并提示下一步命令。后续统一使用：

```bash
hodexctl
```

如果你希望手动下载脚本再运行，也可以使用：

```bash
curl -fsSL https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/hodexctl/hodexctl.sh -o ./hodexctl.sh
chmod +x ./hodexctl.sh
./hodexctl.sh
```

```powershell
$script = Join-Path $env:TEMP "hodexctl.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/stellarlinkco/codex/main/scripts/hodexctl/hodexctl.ps1 -OutFile $script
& $script
```

### 常用命令

```bash
hodexctl install
hodexctl list
hodexctl upgrade
hodexctl upgrade 1.2.2
hodexctl downgrade 1.2.1
hodexctl download 1.2.2
hodexctl status
hodexctl relink
hodexctl uninstall
```

```bash
hodexctl source install --repo stellarlinkco/codex --ref main
hodexctl source update --profile codex-source
hodexctl source switch --profile codex-source --ref feature/my-branch
hodexctl source status
hodexctl source list
hodexctl source uninstall --profile codex-source
```

### 默认位置

- 状态目录：
  - macOS / Linux / WSL: `~/.hodex`
  - Windows: `%LOCALAPPDATA%\hodex`
- 默认源码 checkout 建议放在：`~/hodex-src/<host>/<owner>/<repo>`

### 行为说明

- 直接运行 `hodexctl` 会显示帮助。
- `list` 会列出当前平台可下载版本，并支持查看 changelog。
- changelog 页的 `AI总结` 会优先调用本机 `hodex`，不可用时回退到 `codex`。
- GitHub API 匿名请求遇到 `403` 时，会优先尝试 `gh api` 兜底；如果 `gh` 不可用、未登录或无权限，会给出明确提示。
- `relink` 只重建包装器，不重新下载二进制。

### 查看状态

```bash
hodexctl status
```

状态页会显示当前 release 安装、命令目录、PATH 处理结果，以及已登记的源码条目摘要。

### 卸载说明

```bash
hodexctl uninstall
```

- 该命令会卸载受管 release；如果只装了管理器，也会一并清理。
- 源码条目需要通过 `hodexctl source uninstall` 单独清理。
- 当最后一个 release / 源码条目都被移除后，`hodexctl` 包装器和受管 PATH 也会一并清理。

### 常用选项

```bash
hodexctl install --yes --no-path-update
hodexctl install --github-token <token>
hodexctl status --state-dir /custom/state
hodexctl source install --git-url git@github.com:someone/codex.git --profile codex-fork
```

Windows PowerShell 对应参数名为 `-Yes`、`-NoPathUpdate`、`-GitHubToken`、`-StateDir`、`-GitUrl`、`-Profile`。
