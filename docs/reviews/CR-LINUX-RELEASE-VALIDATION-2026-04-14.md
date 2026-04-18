# CR Linux Release Validation - 2026-04-14

## 范围

- 目标分支：
  - `fix/ci-watchdog-rust-test-stability-20260409`
- 目标产物：
  - `/tmp/codex-linux-gnu-out.oCJWmh/codex`
  - `/tmp/codex-linux-gnu-out.oCJWmh/codex-app-server-test-client`
  - `/tmp/codex-linux-webui-out.OAPQzI/codex`
  - `/tmp/codex-linux-rebuild.ioU2TO/codex`
- Linux 运行方式：
  - 使用 Docker `rust:1.93-bookworm`
  - 使用独立 Linux `CODEX_HOME`
  - 仅使用最小 `local-newapi` 配置
  - 不使用 `auth.json`
- Ubuntu 复测方式：
  - 使用 Docker `ubuntu:24.04`
  - 复用同一组 Linux release 二进制
  - 复用同一份独立 Linux `CODEX_HOME`
  - 仅使用最小 `local-newapi` 配置
  - 不使用 `auth.json`

## 结论

- Linux release 二进制已成功构建：
  - `codex` 为 Linux x86_64 ELF，可执行
  - `codex-app-server-test-client` 为 Linux x86_64 ELF，可执行
- 已验证通过的真实运行路径：
  - `codex --version`
  - `codex login status`
  - `codex features list`
  - `codex mcp list`
  - `codex exec ...` 经 `local-newapi` 完成真实对话
  - `codex-app-server-test-client --codex-bin /out/codex send-message-v2 ...`
  - `codex-app-server-test-client --codex-bin /out/codex model-list`
  - `codex-app-server-test-client serve -> websocket model-list -> websocket send-message-v2`
- Ubuntu 24.04 容器中，以上核心路径也已完成复测并通过。
- 基于当前源码重新构建出的最新 Linux release：
  - `/tmp/codex-linux-rebuild.ioU2TO/codex`
  - 已确认是 Linux x86_64 ELF，不再是之前误拷贝出的 Mach-O
- 已在 Ubuntu 24.04 中完成两条本轮关键回归验证：
    - `codex completion zsh | head -n 5`
      - 退出码：`0`
      - 说明 `completion` 的 `BrokenPipe` 修复已在真实 Linux release 生效
    - `codex completion bash | head -n 3`
      - 退出码：`0`
    - `codex completion fish | head -n 3`
      - 退出码：`0`
      - 说明 `completion` 的 `BrokenPipe` 修复不只在 `zsh`，在常见 shell completion 路径上也已复核通过
    - `codex -s workspace-write -a never review --uncommitted`
      - 会话头明确显示 `sandbox: workspace-write`
      - 说明 `review` 已正确继承根级 sandbox flags，不再错误退回 `read-only`
    - `codex --dangerously-bypass-approvals-and-sandbox review --uncommitted`
      - 会话头明确显示 `sandbox: danger-full-access`
      - 在带 `git` 的 Linux 镜像中可完成最小 review 工作流并正常退出
      - 说明 `review` 也已正确继承根级 danger flag
  - 另补一条最新 ELF 的 `serve` 启动烟测：
    - Ubuntu 24.04 中可成功打印 `Codex Web UI running at http://127.0.0.1:40197?...`
    - 说明这次 CLI 修复没有破坏 WebUI/服务入口
- 基于当前源码最新 Linux release 的追加验证：
  - `codex login status`
    - 返回 `Not logged in`
    - 退出码：`1`
    - 与当前仅使用 `local-newapi`、不使用 `auth.json` 的测试口径一致
  - `codex features list`
    - 可正常输出 feature 列表
    - 退出码：`0`
  - `codex mcp list`
    - 返回 `No MCP servers configured yet. Try \`codex mcp add my-tool -- my-command\`.`
    - 退出码：`0`
  - `codex logout`
    - 在无凭证状态下返回 `Not logged in`
    - 退出码：`0`
  - `codex exec --dangerously-bypass-approvals-and-sandbox -C /codex-home "Reply with exactly: latest-linux-exec-ok"`
    - 最终返回 `latest-linux-exec-ok`
    - 说明最新 rebuilt Linux release 的非交互 `exec` 主链在 `local-newapi` 下可用
  - `codex app-server generate-json-schema --out /work/schema`
    - 退出码：`0`
    - 生成文件数：`170`
  - `codex app-server generate-ts --out /work/ts`
    - 退出码：`0`
    - 生成文件数：`519`
  - `sandbox linux /bin/echo latest-sandbox-linux-ok`
    - 返回 `latest-sandbox-linux-ok`
  - `mcp add/list/get/remove` 生命周期
    - 可成功新增 `self-test-latest`
    - `mcp get --json` 返回 `command=/out/codex` 与 `args=["mcp-server"]`
    - 可成功删除并回到 `No MCP servers configured yet`
  - `mcp-server` newline JSON-RPC 握手
    - `initialize -> notifications/initialized -> tools/list -> tools/call(codex) -> tools/call(codex-reply)` 全部通过
    - `codex` 返回 `latest-linux-mcp-tool-ok`
    - `codex-reply` 返回 `latest-linux-mcp-reply-ok`
  - `codex-app-server-test-client --codex-bin /out/codex`
    - `model-list` 成功
    - `send-message-v2` 最终返回 `latest-linux-app-server-ok`
  - `codex debug app-server send-message-v2`
    - 最终返回 `latest-linux-debug-app-server-ok`
  - `codex --dangerously-bypass-approvals-and-sandbox review --uncommitted`
    - 最小 git repo 中对 `README.md` 的单文件改动可完成 review
    - 最终结论为该改动未引入 correctness / maintainability / behavioral issue
    - 退出码：`0`
  - `cloud list` / `apply bogus-task-id`
    - 分别稳定返回：
      - `Not signed in. Please run 'codex login' to sign in with ChatGPT, then re-run 'codex cloud'.`
      - `Error: ChatGPT token not available`
  - `codex apply`
    - 在缺少位置参数 `<TASK_ID>` 时稳定返回 clap 参数错误：
      - `error: the following required arguments were not provided:`
      - `<TASK_ID>`
    - 退出码：`2`
  - `mcp login` / `mcp logout`（stdio MCP server）
    - 稳定返回：
      - `OAuth login is only supported for streamable HTTP servers.`
      - `OAuth logout is only supported for streamable_http transports.`
  - TUI
    - Ubuntu 24.04 PTY 中可正常启动
    - 未信任目录下会真实显示目录信任确认页
    - 在已信任目录 `/codex-home` 中已完成完整消息往返，最终返回 `latest-linux-tui-home-ok`
    - `resume` 关键续会话路径最终返回 `latest-linux-resume-ok`
    - `fork` 关键续会话路径最终返回 `latest-linux-fork-ok`
    - `resume --last` 关键选择器路径最终返回 `latest-linux-resume-last-ok`
    - `fork --last` 关键选择器路径最终返回 `latest-linux-fork-last-ok`
- 本轮不能宣称“所有 Linux 功能全测完”：
  - 虽然主要顶层 CLI 入口已基本都有真实工作流或真实边界证据，但仍未穷举所有子命令参数组合
  - 未逐一覆盖所有 TUI 交互场景
  - 未逐一覆盖 MCP server 生命周期
  - 未逐一覆盖 Linux 沙箱细分模式
- WebUI 方面新增确认：
  - `codex serve` 提供的内嵌 Web UI 已在 Ubuntu 24.04 容器里真实打开并进入会话页
  - 已能新建 session，并经 `local-newapi` 在浏览器对话页返回 `ubuntu-webui-ok`
- `Board -> Sessions` 空白问题已在源码侧定位并修复：
  - 根因不是后端 `/api/kanban` 无数据，而是前端 `KanbanPage` 只实现了 `github/workspace` 两种 scope
  - `KanbanHeader` 暴露了 `Sessions` 标签，但页面没有接入 `useSessions + /api/kanban` 数据流
  - 已补齐 `sessions` 看板 query、卡片映射与拖拽移动路径，并新增前端回归测试
  - 已通过前端验证：
    - `npm test -- --run src/routes/kanban/index.test.tsx`
    - `npm run typecheck`
  - macOS 本地 release 二进制已完成真实浏览器复测
  - 新构建的 Linux release 二进制已在 Ubuntu 24.04 容器中完成同路径真实浏览器复测
  - 最新 rebuilt Linux release 二进制已完成 `Sessions` 跨列拖拽复测，并确认刷新后状态持久化

## 构建记录

- Linux 源码镜像：
  - `/tmp/codex-linux-src.UjkevT`
- Linux 输出目录：
  - `/tmp/codex-linux-gnu-out.oCJWmh`
- Linux WebUI 修复后的新输出目录：
  - `/tmp/codex-linux-webui-out.OAPQzI`
- Linux 当前源码回归修复后的最新输出目录：
  - `/tmp/codex-linux-rebuild.ioU2TO`
- Linux 当前源码对应的独立 target 目录：
  - `/tmp/codex-linux-target.wEoEmd`
- Linux Cargo 缓存：
  - `/tmp/codex-linux-cargo-home`
- 构建命令核心流程：
  - `make release-codex OUT=/out/codex`
  - `cargo build -p codex-app-server-test-client --release`
- 构建结果：
  - `codex` 完成于 `release` profile，构建耗时约 `47m 11s`
  - `codex-app-server-test-client` 完成于 `release` profile，构建耗时约 `14m 19s`
  - 新版 `codex`（包含本轮 WebUI `Sessions` 修复）完成于 `release` profile，构建耗时约 `40m 30s`
  - 当前源码重新构建的 `codex`（包含 `review` root flags 合并与 `completion` BrokenPipe 修复）完成于 `release` profile，构建耗时约 `43m 51s`
- `file` 检查结果：
  - `/out/codex`: `ELF 64-bit LSB pie executable, x86-64, dynamically linked, stripped`
  - `/out/codex-app-server-test-client`: `ELF 64-bit LSB pie executable, x86-64, dynamically linked, stripped`
  - `/tmp/codex-linux-webui-out.OAPQzI/codex`: `ELF 64-bit LSB pie executable, x86-64, dynamically linked, stripped`
  - `/tmp/codex-linux-rebuild.ioU2TO/codex`: `ELF 64-bit LSB pie executable, x86-64, dynamically linked, stripped`

## 运行验证

### 1. CLI 基础命令

- `docker run ... /out/codex --version`
  - 退出码：0
  - 结果：`codex-cli 1.3.0`
- `docker run ... /out/codex login status`
  - 退出码：0
  - 结果：`Not logged in`
- `docker run ... /out/codex features list`
  - 退出码：0
  - 结果：成功列出 feature 状态
- `docker run ... /out/codex mcp list`
  - 退出码：0
  - 结果：`No MCP servers configured yet.`

### 2. `local-newapi` 真实对话

- 命令：
  - `docker run ... /out/codex exec --dangerously-bypass-approvals-and-sandbox --ephemeral --skip-git-repo-check -C /tmp "Reply with exactly: linux-local-newapi-ok"`
- 退出码：0
- 结果：
  - provider: `local-newapi`
  - model: `gpt-5.4`
  - 最终回复：`linux-local-newapi-ok`

### 3. app-server 子进程路径

- 命令：
  - `docker run ... /out/codex-app-server-test-client --codex-bin /out/codex send-message-v2 "Reply with exactly: linux-app-server-ok"`
- 退出码：0
- 结果：
  - thread/start 成功
  - turn/start 成功
  - 最终 agent message：`linux-app-server-ok`

- 命令：
  - `docker run ... /out/codex-app-server-test-client --codex-bin /out/codex model-list`
- 退出码：0
- 结果：
  - `model/list` 成功返回模型列表
  - 已看到 `gpt-5.4`、`gpt-5.3-codex`、`gpt-5.2-codex`、`gpt-5.1-codex-max`、`gpt-5.2`、`gpt-5.1-codex-mini`

### 4. app-server websocket 路径

- 命令：
  - `docker run ... /out/codex-app-server-test-client --codex-bin /out/codex serve --listen ws://127.0.0.1:4222`
  - `docker run same-container ... /out/codex-app-server-test-client --url ws://127.0.0.1:4222 model-list`
  - `docker run same-container ... /out/codex-app-server-test-client --url ws://127.0.0.1:4222 send-message-v2 "Reply with exactly: linux-ws-app-server-ok"`
- 退出码：0
- 结果：
  - app-server 成功启动并监听 `ws://127.0.0.1:4222`
  - websocket `model-list` 成功返回
  - websocket `send-message-v2` 成功返回 `linux-ws-app-server-ok`

## Ubuntu 24.04 复测

### 1. CLI 基础命令

- `docker run ... ubuntu:24.04 /out/codex --version`
  - 退出码：0
  - 结果：`codex-cli 1.3.0`
- `docker run ... ubuntu:24.04 /out/codex login status`
  - 退出码：0
  - 结果：`Not logged in`
- `docker run ... ubuntu:24.04 /out/codex features list`
  - 退出码：0
  - 结果：成功列出 feature 状态
- `docker run ... ubuntu:24.04 /out/codex mcp list`
  - 退出码：0
  - 结果：`No MCP servers configured yet.`

### 2. `local-newapi` 真实对话

- 命令：
  - `docker run ... ubuntu:24.04 /out/codex exec --dangerously-bypass-approvals-and-sandbox --ephemeral --skip-git-repo-check -C /tmp "Reply with exactly: ubuntu-local-newapi-ok"`
- 退出码：0
- 结果：
  - provider: `local-newapi`
  - model: `gpt-5.4`
  - 最终回复：`ubuntu-local-newapi-ok`

### 3. app-server 子进程路径

- 命令：
  - `docker run ... ubuntu:24.04 /out/codex-app-server-test-client --codex-bin /out/codex model-list`
- 退出码：0
- 结果：
  - `model/list` 成功返回模型列表
  - user agent 中已确认运行环境为 `Ubuntu 24.4.0; x86_64`

- 命令：
  - `docker run ... ubuntu:24.04 /out/codex-app-server-test-client --codex-bin /out/codex send-message-v2 "Reply with exactly: ubuntu-app-server-ok"`
- 退出码：0
- 结果：
  - thread/start 成功
  - turn/start 成功
  - 最终 agent message：`ubuntu-app-server-ok`

### 4. app-server websocket 路径

- 命令：
  - `docker run ... ubuntu:24.04 /out/codex-app-server-test-client --codex-bin /out/codex serve --listen ws://127.0.0.1:4222`
  - `sleep 2`
  - `docker run same-container ... ubuntu:24.04 /out/codex-app-server-test-client --url ws://127.0.0.1:4222 model-list`
  - `docker run same-container ... ubuntu:24.04 /out/codex-app-server-test-client --url ws://127.0.0.1:4222 send-message-v2 "Reply with exactly: ubuntu-ws-app-server-ok"`
- 退出码：0
- 结果：
  - websocket `model-list` 成功返回
  - websocket `send-message-v2` 成功返回 `ubuntu-ws-app-server-ok`

### 5. Ubuntu 特有观测

- 第一次 websocket 复测在 `serve` 刚返回后立即连接，出现过一次：
  - `failed to connect to websocket app-server at ws://127.0.0.1:4222`
- 加入 `sleep 2` 后复测通过。
- 当前结论：
  - 这是 app-server 就绪时序问题，不是 Ubuntu 运行时崩溃。

### 6. 当前源码最新 Linux release 关键回归复测

- `completion` BrokenPipe 复测：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out ubuntu:24.04 bash -lc 'set -o pipefail; /out/codex completion zsh | head -n 5 >/tmp/codex.zsh; rc=$?; printf "EXIT=%s\n" "$rc"; sed -n "1,5p" /tmp/codex.zsh'`
  - 退出码：0
  - 结果：
    - 输出前 5 行为 zsh completion 内容
    - `EXIT=0`
  - 当前结论：
    - `completion` 在管道提前关闭时不再 panic

- `review` root flags 继承复测：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/root/.codex ubuntu:24.04 bash -lc '... timeout 60s /out/codex -s workspace-write -a never review --uncommitted ...'`
  - 结果要点：
    - 会话头显示：
      - `approval: never`
      - `sandbox: workspace-write [workdir, /tmp, $TMPDIR, /root/.codex/memories]`
    - `provider: local-newapi`
    - review 成功读取 git 状态、diff 和文件内容，并输出最终审查结论
  - 当前结论：
    - `review` 已正确继承根级 `-s workspace-write`
    - 本轮修复在 Ubuntu 24.04 的真实 Linux release 上已得到运行时验证

- `serve` 启动烟测：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/root/.codex ubuntu:24.04 bash -lc 'timeout 15s /out/codex serve ...'`
  - 结果：
    - 输出 `Codex Web UI running at http://127.0.0.1:40197?token=...`
  - 当前结论：
    - 最新 Linux release 的 WebUI 服务入口可正常启动

### 7. 当前源码最新 Linux release 的追加验证

- `sandbox linux`：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 /out/codex sandbox linux /bin/echo latest-sandbox-linux-ok`
  - 结果：
    - 标准输出：`latest-sandbox-linux-ok`
  - 当前结论：
    - 最新 Linux release 的 `sandbox linux` 最小执行路径可用

- MCP 配置生命周期：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex mcp add self-test-latest -- /out/codex mcp-server ...'`
  - 结果：
    - `mcp list` 列出：
      - `self-test-latest  /out/codex  mcp-server  ... enabled  Unsupported`
    - `mcp get self-test-latest --json` 返回 `stdio` 配置，`command=/out/codex`，`args=["mcp-server"]`
    - `mcp remove` 后回到：
      - `No MCP servers configured yet.`
  - 观测：
    - 过程中出现一次 warning：
      - `failed to clean up stale arg0 temp dirs: Directory not empty (os error 39)`
    - 但不影响 MCP 配置闭环完成
  - 当前结论：
    - 最新 Linux release 的 MCP 配置新增、查询、删除路径可用

- `mcp-server` 协议握手与工具调用：
  - 真实验证方式：
    - 直接启动 `/tmp/codex-linux-rebuild.ioU2TO/codex mcp-server`
    - 按仓库测试同款 newline-delimited JSON-RPC 协议发送：
      - `initialize`
      - `notifications/initialized`
      - `tools/list`
      - `tools/call` (`codex`)
      - `tools/call` (`codex-reply`)
  - 结果：
    - `initialize.protocolVersion = 2025-03-26`
    - `serverInfo.name = codex-mcp-server`
    - `tools/list` 返回工具：
      - `codex`
      - `codex-reply`
    - `codex` 返回：
      - `latest-linux-mcp-tool-ok`
    - `codex-reply` 返回：
      - `latest-linux-mcp-reply-ok`
  - 当前结论：
    - 最新 Linux release 的 `mcp-server` 握手、工具发现、首轮回复、同线程续答均已真实通过

- `codex-app-server-test-client --codex-bin /out/codex`：
  - `model-list`
    - 成功返回模型列表
  - `send-message-v2`
    - 最终 agent message：`latest-linux-app-server-ok`
  - 当前结论：
    - 旧 Linux `codex-app-server-test-client` 挂最新 Linux `codex` 时，stdio app-server 主链仍可用

- `codex debug app-server send-message-v2`：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex debug app-server send-message-v2 "Reply with exactly: latest-linux-debug-app-server-ok"'`
  - 结果：
    - `thread/start` 返回 `modelProvider = local-newapi`
    - 最终 agent message：`latest-linux-debug-app-server-ok`
  - 当前结论：
    - 最新 Linux release 的 debug app-server 消息往返可用

- `cloud` / `apply` 边界：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex cloud list; ...; /out/codex apply bogus-task-id; ...'`
  - 结果：
    - `cloud list` 输出：
      - `Not signed in. Please run 'codex login' to sign in with ChatGPT, then re-run 'codex cloud'.`
    - `apply bogus-task-id` 输出：
      - `Error: ChatGPT token not available`
    - 退出码：
      - `CLOUD_EXIT=1`
      - `APPLY_EXIT=1`
  - 当前结论：
    - 最新 Linux release 在未登录 ChatGPT 的前提下，`cloud` / `apply` 的边界报错口径稳定

- `mcp login` / `mcp logout` 对 stdio MCP server 的 OAuth 不支持边界：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-rebuild.ioU2TO:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex mcp add self-test-oauth -- /out/codex mcp-server ...'`
  - 结果：
    - `mcp login self-test-oauth` 输出：
      - `Error: OAuth login is only supported for streamable HTTP servers.`
    - `mcp logout self-test-oauth` 输出：
      - `Error: OAuth logout is only supported for streamable_http transports.`
    - 退出码：
      - `LOGIN_EXIT=1`
      - `LOGOUT_EXIT=1`
  - 当前结论：
    - 最新 Linux release 对 `stdio` MCP server 的 OAuth 不支持边界与此前结论一致

- TUI 启动、目录信任确认与完整消息往返：
  - 真实验证方式：
    - 在 Ubuntu 24.04 PTY 中直接启动：
      - `/out/codex --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C /tmp "Reply with exactly: latest-linux-tui-ok"`
    - 为了绕过 onboarding gate 并单独验证主对话链路，在测试用 `CODEX_HOME/config.toml` 中显式加入：
      - `[projects."/codex-home"]`
      - `trust_level = "trusted"`
    - 随后在 Ubuntu 24.04 PTY 中启动：
      - `/out/codex --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C /codex-home "Reply with exactly: latest-linux-tui-home-ok"`
  - 结果：
    - 终端真实渲染：
      - 欢迎页
      - `/tmp` 目录信任确认页
      - `1. Yes, continue`
      - `2. No, quit`
    - 在 `/codex-home` trusted 场景下：
      - 终端真实显示用户 prompt：
        - `Reply with exactly: latest-linux-tui-home-ok`
      - 终端真实显示最终回答：
        - `latest-linux-tui-home-ok`
      - 会话文件：
        - `/tmp/codex-linux-home.xxYAeR/sessions/2026/04/16/rollout-2026-04-16T01-33-54-019d93ec-5f6a-7e31-8e40-5a599875bdf7.jsonl`
      - 会话文件中已记录：
        - user message = `Reply with exactly: latest-linux-tui-home-ok`
        - assistant final answer = `latest-linux-tui-home-ok`
  - 当前结论：
    - 最新 Linux release 的 TUI 入口与首屏交互可用
    - 最新 Linux release 的 TUI 在 trusted 目录中的完整消息往返也已真实通过

- TUI `resume` / `fork` 关键续会话路径：
  - 真实验证方式：
    - 以上一条 trusted `/codex-home` 成功会话 `019d93ec-5f6a-7e31-8e40-5a599875bdf7` 为锚点
    - 在 Ubuntu 24.04 PTY 中执行：
      - `/out/codex resume 019d93ec-5f6a-7e31-8e40-5a599875bdf7`
      - 在续会话中发送：`Reply with exactly: latest-linux-resume-ok`
    - 在 Ubuntu 24.04 PTY 中执行：
      - `/out/codex fork 019d93ec-5f6a-7e31-8e40-5a599875bdf7`
      - 在 fork 后的新会话中发送：`Reply with exactly: latest-linux-fork-ok`
  - 结果：
    - `resume` PTY 真实显示历史回答 `latest-linux-tui-home-ok`
    - `resume` 续会话真实显示最终回答：
      - `latest-linux-resume-ok`
    - `resume` 对应会话文件：
      - `/tmp/codex-linux-home.xxYAeR/sessions/2026/04/16/rollout-2026-04-16T01-33-54-019d93ec-5f6a-7e31-8e40-5a599875bdf7.jsonl`
    - 该文件中已记录：
      - user message = `Reply with exactly: latest-linux-resume-ok`
      - assistant final answer = `latest-linux-resume-ok`
      - task_complete last_agent_message = `latest-linux-resume-ok`
    - `fork` PTY 真实显示：
      - `Thread forked from 019d93ec-5f6a-7e31-8e40-5a599875bdf7`
    - `fork` 新会话真实显示最终回答：
      - `latest-linux-fork-ok`
    - `fork` 对应会话文件：
      - `/tmp/codex-linux-home.xxYAeR/sessions/2026/04/16/rollout-2026-04-16T01-36-49-019d93ef-0778-7173-9ebd-b6fea52a892c.jsonl`
    - 该文件中已记录：
      - user message = `Reply with exactly: latest-linux-fork-ok`
      - assistant final answer = `latest-linux-fork-ok`
      - task_complete last_agent_message = `latest-linux-fork-ok`
  - 当前结论：
    - 最新 Linux release 的 TUI `resume` 关键续会话路径已真实通过
    - 最新 Linux release 的 TUI `fork` 关键续会话路径已真实通过
    - 当前仍不能据此宣称所有 TUI 交互都已完整验证

- TUI `resume --last` / `fork --last` 关键选择器路径：
  - 真实验证方式：
    - 在 Ubuntu 24.04 PTY 中执行：
      - `/out/codex resume --last --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C /codex-home`
      - 在最近会话中发送：`Reply with exactly: latest-linux-resume-last-ok`
    - 在 Ubuntu 24.04 PTY 中执行：
      - `/out/codex fork --last --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C /codex-home`
      - 在 fork 后的新会话中发送：`Reply with exactly: latest-linux-fork-last-ok`
  - 结果：
    - `resume --last` 对应会话文件：
      - `/tmp/codex-linux-home.xxYAeR/sessions/2026/04/16/rollout-2026-04-16T01-36-49-019d93ef-0778-7173-9ebd-b6fea52a892c.jsonl`
    - 该文件中已记录：
      - user message = `Reply with exactly: latest-linux-resume-last-ok`
      - assistant final answer = `latest-linux-resume-last-ok`
      - task_complete last_agent_message = `latest-linux-resume-last-ok`
    - `fork --last` 对应会话文件：
      - `/tmp/codex-linux-home.xxYAeR/sessions/2026/04/16/rollout-2026-04-16T02-27-48-019d941d-b720-7500-a3a7-ad0a145d89f2.jsonl`
    - 该文件中已记录：
      - user message = `Reply with exactly: latest-linux-fork-last-ok`
      - assistant final answer = `latest-linux-fork-last-ok`
      - task_complete last_agent_message = `latest-linux-fork-last-ok`
  - 当前结论：
    - 最新 Linux release 的 TUI `resume --last` 关键选择器路径已真实通过
    - 最新 Linux release 的 TUI `fork --last` 关键选择器路径已真实通过
    - 当前仍不能据此宣称所有 TUI 交互都已完整验证

## 异常与复测

- 首次执行 `--codex-bin /out/codex model-list` 时，曾出现一次：
  - `codex app-server exited: signal: 7 (SIGBUS)`
  - `Error: codex app-server closed stdout`
- 随后进行了两次复测：
  - 直连 `--codex-bin /out/codex model-list`
  - websocket `serve -> model-list`
- 两次复测均通过，当前没有稳定复现路径。
- 当前结论：
  - 这次 `SIGBUS` 只能记为“一次性异常，已复测通过”，不能记为已定位根因。

## 环境注意事项

- Linux 侧配置没有直接复用 macOS 全量 `~/.codex/config.toml`。
- Linux 侧只使用独立最小配置，避免平台参数漂移带来的伪问题。
- `codex-app-server-test-client serve --kill` 在该容器镜像下会失败：
  - 原因：镜像内缺少 `lsof`
  - 这不是 `codex` 主程序故障；去掉 `--kill` 后 websocket 测试通过。

## 当前边界

- 本轮已证明：
  - Linux release 能构建
  - Linux CLI 基础命令可运行
  - Linux `local-newapi` 主对话链路可用
  - Linux app-server 的 stdio 和 websocket 两条核心链路可用
  - Ubuntu 24.04 容器中，同一组 Linux release 二进制的 CLI、`local-newapi`、app-server stdio、app-server websocket 核心链路均可用
  - 仓库当前修复在 macOS 本地新 release 二进制 `/tmp/codex-mac-webui-fix` 上已完成真实浏览器复测：
    - `serve --dev` 可正常启动
    - 打开 `/kanban?token=...` 后切到 `Sessions`，页面不再空白
    - 可真实看到 `BACKLOG / IN PROGRESS / REVIEW / DONE` 四列和已有 session 卡片
  - 仓库当前修复在 Ubuntu 24.04 容器中的新 Linux release 二进制 `/tmp/codex-linux-webui-out.OAPQzI/codex` 上也已完成真实浏览器复测：
    - 纯 release 模式 `serve` 可正常启动并提供内嵌静态资源
    - 打开 `/kanban?token=...` 后切到 `Sessions`，页面不再空白
    - 可真实看到 `BACKLOG / IN PROGRESS / REVIEW / DONE` 四列和已有 session 卡片
    - 浏览器网络面板已确认 `GET /api/sessions` -> `200`，`GET /api/kanban` -> `200`
  - 最新 rebuilt Linux release 二进制 `/tmp/codex-linux-rebuild.ioU2TO/codex` 也已在 Ubuntu 24.04 容器中完成真实浏览器复测：
    - 以 `serve --host 0.0.0.0 --port 4319 --token <redacted> --no-open` 启动
    - 从宿主浏览器打开 `/kanban?token=...` 并切到 `Sessions`
    - 页面真实显示 `BACKLOG / IN PROGRESS / REVIEW / DONE` 四列和已有 session 卡片
    - 浏览器网络请求已确认 `GET /api/sessions` -> `200`，`GET /api/kanban` -> `200`
    - 截图保存在：
      - `/tmp/latest-linux-rebuild-webui-4319.png`
  - 最新 rebuilt Linux release 二进制 `/tmp/codex-linux-rebuild.ioU2TO/codex` 已进一步完成真实浏览器跨列拖拽复测：
    - 从 `BACKLOG` 将 session `019d93ef-0778-7173-9ebd-b6fea52a892c` 拖到 `IN PROGRESS`
    - 浏览器网络面板确认：
      - `PUT /api/kanban/cards/019d93ef-0778-7173-9ebd-b6fea52a892c` -> `200`
      - 请求体：`{"columnId":"in-progress","position":0}`
    - 页面状态同步变化：
      - `BACKLOG` 计数从 `11` 变为 `10`
      - `IN PROGRESS` 计数从 `0` 变为 `1`
      - 状态文本显示 `... dropped over droppable area in-progress`
    - 刷新页面后再次切回 `Sessions`：
      - `BACKLOG = 10`
      - `IN PROGRESS = 1`
      - 说明跨列移动已落到服务端，不是单纯前端乐观更新
    - 截图保存在：
      - `/tmp/latest-linux-rebuild-webui-drag-4320.png`
- 本轮尚未证明：
  - 所有 CLI 子命令和所有 TUI 交互均已完整验证
  - 所有 Linux 特性开关、MCP、sandbox 组合路径均已完整验证
- 额外说明：
  - `/tmp/codex-linux-webui-out.OAPQzI/codex` 仍可作为此前 WebUI 路径验证证据
  - 但涉及本轮 `review` / `completion` 修复时，应以 `/tmp/codex-linux-rebuild.ioU2TO/codex` 为准

## WebUI Sessions Kanban 源码修复记录

- 修复文件：
  - `web/src/hooks/queries/useKanban.ts`
  - `web/src/routes/kanban/types.ts`
  - `web/src/routes/kanban/KanbanCard.tsx`
  - `web/src/routes/kanban/KanbanColumn.tsx`
  - `web/src/routes/kanban/CardDetailPanel.tsx`
  - `web/src/routes/kanban/index.tsx`
  - `web/src/routes/kanban/index.test.tsx`
- 变更摘要：
  - 新增 `useKanban`，把 `/api/kanban` 接入 `sessions` scope
  - 为看板卡片补充 `session/github` 两类数据模型，避免 `Sessions` 标签落到空渲染分支
  - `sessions` scope 下支持基于 session 元数据构建卡片和列
  - `sessions` 卡片拖拽会调用 `moveKanbanCard`
  - 为看板列和卡片补充拖拽类型元数据，并调整碰撞检测优先级，避免空列和列空白区误回落到源卡片
  - 补充 `KeyboardSensor`，使 `Sessions` 看板具备官方可访问性拖拽路径
  - 详情侧栏仍只对 GitHub/workspace 卡片开启，不把 session 卡片误送入 GitHub detail 逻辑
- 前端回归验证：
  - `npm test -- --run src/routes/kanban/index.test.tsx`
    - 结果：通过
  - `npm run typecheck`
    - 结果：通过

## WebUI Sessions 浏览器复测补充

### 1. macOS 本地 release 二进制复测

- 产物：
  - `/tmp/codex-mac-webui-fix`
  - `/tmp/codex-mac-webui-dragfix`
- 启动命令：
  - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR /tmp/codex-mac-webui-fix serve --host 127.0.0.1 --port 4312 --token <redacted> --no-open --dev`
- 真实浏览器观测：
  - 打开 `http://127.0.0.1:4312/kanban?token=<redacted>`
  - 默认进入 `GitHub` scope，可见 `No GitHub work items. Configure repos and sync.`
  - 切换到 `Sessions` 后，页面立即显示：
    - `BACKLOG`
    - `IN PROGRESS`
    - `REVIEW`
    - `DONE`
  - `BACKLOG` 列内可见已有 session 卡片，页面不再空白
- 配套证据：
  - 浏览器网络面板已看到：
    - `GET /api/sessions` -> `200`
    - `GET /api/kanban` -> `200`
  - 截图已保存：
    - `/tmp/kanban-sessions-release-webui-2026-04-14.png`
- 补充复测：
  - 使用新的 release 产物 `/tmp/codex-mac-webui-dragfix` 以纯 release 模式启动：
    - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR /tmp/codex-mac-webui-dragfix serve --host 127.0.0.1 --port 4317 --token <redacted> --no-open`
  - 在真实浏览器中把 session 卡片从 `IN PROGRESS` 拖到 `REVIEW`
  - 浏览器网络面板确认：
    - `PUT /api/kanban/cards/019d8b66-2828-7593-aa93-e3d7a49a2398` -> `200`
    - 请求体：`{"columnId":"review","position":0}`
  - 页面状态同步变化：
    - `IN PROGRESS` 计数变为 `0`
    - `REVIEW` 计数变为 `1`
    - 状态文本显示 `... dropped over droppable area review`
  - 截图已保存：
    - `/tmp/kanban-sessions-mac-release-dragfix-2026-04-15.png`

### 2. Ubuntu 24.04 容器中的 Linux release 二进制复测

- 产物：
  - `/tmp/codex-linux-webui-out.OAPQzI/codex`
- 启动路径：
  - 容器：`codex-ubuntu-webui-release-test`
  - 启动参数：`CODEX_HOME=/codex-home /out/codex serve --host 0.0.0.0 --port 4312 --token <redacted> --no-open`
- 真实浏览器观测：
  - 打开 `http://127.0.0.1:4314/kanban?token=<redacted>`
  - 默认仍先进入 `GitHub` scope，可见未配置仓库时的空状态
  - 切换到 `Sessions` 后，页面立即显示：
    - `BACKLOG`
    - `IN PROGRESS`
    - `REVIEW`
    - `DONE`
  - `BACKLOG` 列内可见已有 session 卡片，页面不再空白
  - 点击 session 卡片后页面保持稳定，未误打开 GitHub detail 面板，也未触发空白
- 配套证据：
  - HTTP 探针：
    - `GET /` -> `200`
    - `GET /api/sessions` -> `200`
    - `GET /api/kanban?scope=sessions` -> `200`
  - 浏览器网络面板已看到：
    - `GET /api/sessions` -> `200`
    - `GET /api/kanban` -> `200`
  - 截图已保存：
    - `/tmp/kanban-sessions-linux-release-webui-2026-04-14.png`
  - 在最新 rebuilt Linux release `/tmp/codex-linux-rebuild.ioU2TO/codex` 上继续做跨列拖拽复测：
    - 从 `BACKLOG` 把 session `019d93ef-0778-7173-9ebd-b6fea52a892c` 拖到 `IN PROGRESS`
    - 浏览器网络面板确认：
      - `PUT /api/kanban/cards/019d93ef-0778-7173-9ebd-b6fea52a892c` -> `200`
      - 请求体：`{"columnId":"in-progress","position":0}`
    - 页面计数实时变化：
      - `BACKLOG` 从 `11` 变为 `10`
      - `IN PROGRESS` 从 `0` 变为 `1`
      - 状态文本为 `Draggable item ... was dropped over droppable area in-progress`
    - 刷新页面并重新切回 `Sessions` 后，计数仍保持：
      - `BACKLOG = 10`
      - `IN PROGRESS = 1`
    - 截图已保存：
      - `/tmp/latest-linux-rebuild-webui-drag-4320.png`
- 补充观测：
  - 同一页面默认 `GitHub` scope 仍会触发若干 `404` 的 GitHub 相关请求，这是当前无仓库配置下的既有行为噪声
  - 第一次误用 `serve --dev` 时，服务明确报错：
    - `--dev enabled but web/dist/index.html not found`
  - 改为纯 release 模式后启动正常，因此本节结论基于真正的嵌入式 release 资源，而不是运行时前端目录
  - 在新的 Linux release 产物覆盖后，再次使用 Ubuntu 24.04 容器以纯 release 模式复测：
    - 容器：`codex-ubuntu-webui-release-test-2`
    - `docker exec ... /out/codex --version` -> `codex-cli 1.3.0`
    - 打开 `http://127.0.0.1:4318/kanban?token=<redacted>` 后，`Sessions` 看板正常显示当前列状态
    - 将 session 卡片 `019d8b66-2828-7593-aa93-e3d7a49a2398` 从 `REVIEW` 拖到 `DONE`
    - 浏览器网络面板确认：
      - `PUT /api/kanban/cards/019d8b66-2828-7593-aa93-e3d7a49a2398` -> `200`
      - 请求体：`{"columnId":"done","position":0}`
    - 页面状态同步变化：
      - `REVIEW` 计数变为 `0`
      - `DONE` 计数变为 `1`
      - 状态文本显示 `... dropped over droppable area done`
    - 截图已保存：
      - `/tmp/kanban-sessions-linux-release-dragfix-2026-04-15.png`

### 3. 跨列拖拽验证补充

- 浏览器高层 `drag` 自动化一度把 drop target 命中回源卡片，不能作为产品缺陷证据。
- 在前端补充列/卡片拖拽类型元数据、碰撞检测优先级和 `KeyboardSensor` 之后，已用页面内真实 `pointer` 事件序列完成跨列验证：
  - 源卡片：`019d8b66-2828-7593-aa93-e3d7a49a2398`
  - 目标列：`in-progress`
  - 真实请求：
    - `PUT /api/kanban/cards/019d8b66-2828-7593-aa93-e3d7a49a2398` -> `200`
    - 请求体：`{"columnId":"in-progress","position":0}`
- 页面状态也已同步更新：
  - `BACKLOG` 计数从 `5` 变为 `4`
  - `IN PROGRESS` 计数从 `0` 变为 `1`
  - 可见该 session 卡片已出现在 `IN PROGRESS`
- 可访问性状态文本也已明确显示：
  - `Draggable item ... was dropped over droppable area in-progress`
- 截图已保存：
  - `/tmp/kanban-sessions-drag-cross-column-2026-04-15.png`

## 当前边界更新

- 本轮已真实证明：
  - `Sessions` scope 数据接入和页面渲染已恢复
  - session 卡片跨列拖放已在真实浏览器交互下完成验证
  - 该跨列拖放验证已覆盖：
    - 开发态前端调试服务
    - macOS 本地新的 release 二进制
    - Ubuntu 24.04 容器中的新的 Linux release 二进制
- 本轮仍未证明：
  - 所有 CLI 子命令和所有 TUI 交互均已完整验证
  - 所有 Linux 特性开关、MCP、sandbox 组合路径均已完整验证

## macOS release 补充验证

### 1. CLI 子命令补测

- 使用产物：
  - `/tmp/codex-mac-webui-dragfix`
- 使用配置：
  - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR`
  - 默认 provider 为 `local-newapi`
- 已真实通过：
  - `login status`
    - 结果：`Not logged in`
  - `features list`
    - 结果：成功列出 feature flags
  - `mcp list`
    - 结果：`No MCP servers configured yet.`
  - `exec --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --ephemeral -C /tmp "Reply with exactly: mac-release-exec-ok"`
    - 结果：最终回复 `mac-release-exec-ok`
  - `sandbox macos /bin/echo sandbox-macos-ok`
    - 结果：标准输出 `sandbox-macos-ok`
  - `app-server generate-json-schema --out /tmp/codex-app-server-schema-out`
    - 结果：成功生成 `36` 个 schema 文件
  - `app-server generate-ts --out /tmp/codex-app-server-ts-out`
    - 结果：成功生成 `221` 个 TypeScript 文件
  - `completion zsh > /tmp/codex-completion-zsh-test`
    - 结果：成功生成补全脚本，共 `2537` 行
  - `cloud list`
    - 结果：明确返回 `Not signed in. Please run 'codex login'...`
  - `apply bogus-task-id`
    - 结果：明确返回 `Error: ChatGPT token not available`
  - `app /tmp`
    - 结果：检测到现有 `/Applications/Codex.app`，成功执行：
      - `Opening Codex Desktop at /Applications/Codex.app...`
      - `Opening workspace /private/tmp...`
- 已确认可解析的帮助入口：
  - `sandbox --help`
  - `debug --help`
  - `debug app-server --help`
  - `review --help`
  - `resume --help`
  - `fork --help`
  - `app-server --help`
  - `app --help`
  - `cloud --help`
  - `apply --help`

### 2. TUI 最小真实交互

- 使用独立干净家目录：
  - `/tmp/codex-tui-home.Zvmp1R`
  - 仅复制最小 `config.toml`
  - 不复用旧 `state.sqlite`、旧 session、旧日志
- 启动命令：
  - `CODEX_HOME=/tmp/codex-tui-home.Zvmp1R RUST_LOG=trace /tmp/codex-mac-webui-dragfix -c log_dir=/tmp/codex-tui-logs-clean -C /tmp`
- 真实交互结果：
  - 首次进入时出现目录信任提示，确认后进入主界面
  - 主界面模型栏显示 `gpt-5.4`
  - 提交消息 `Reply with exactly: tui-local-newapi-ok`
  - 最终 assistant 回复：`tui-local-newapi-ok`
- 日志证据：
  - `/tmp/codex-tui-logs-clean/codex-tui.log`
  - 已确认：
    - `provider_name=local-newapi`
    - `model_provider_id: "local-newapi"`
    - `last_agent_message: Some("tui-local-newapi-ok")`
- 当前结论：
  - 之前那条带 `403 Forbidden: Country, region, or territory not supported` 的脏会话不能作为当前版本 TUI 故障证据
  - 在干净 `CODEX_HOME` 条件下，macOS release TUI 已通过 `local-newapi` 最小真实对话验证

### 3. `resume --last` 与 `fork --last`

- 在同一干净 `CODEX_HOME=/tmp/codex-tui-home.Zvmp1R` 下继续验证：
  - `resume --last --no-alt-screen -C /tmp`
    - 启动后继续同一会话历史
    - 发送 `Reply with exactly: resume-last-ok`
    - 最终 assistant 回复：`resume-last-ok`
  - `fork --last --no-alt-screen -C /tmp`
    - 启动后明确提示：
      - `Thread forked from 019d8f4e-87ec-7750-8f90-cbd27f300b0d`
    - 发送 `Reply with exactly: fork-last-ok`
    - 最终 assistant 回复：`fork-last-ok`
- 额外观测：
  - `resume --last 'Reply with exactly: ...'` 这种把 prompt 直接作为位置参数的写法会把文本当成 `SESSION_ID` 解析
  - 这属于 CLI 用法限制，不是运行时崩溃

### 4. macOS app-server 调试链路

- 采用 release 二进制内置调试路径，不依赖额外测试客户端：
  - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR /tmp/codex-mac-webui-dragfix debug app-server send-message-v2 'Reply with exactly: mac-debug-app-server-ok'`
- 真实结果：
  - `initialize` 成功
  - `thread/start` 成功
  - `turn/start` 成功
  - `modelProvider` 为 `local-newapi`
  - 最终 `agentMessage` 为 `mac-debug-app-server-ok`
  - 进程正常退出：`[codex app-server exited: exit status: 0]`

### 5. `review --uncommitted` 非交互审查链路

- 命令：
  - `CODEX_HOME=/tmp/codex-tui-home.Zvmp1R /tmp/codex-mac-webui-dragfix review --uncommitted`
- 真实结果：
  - provider: `local-newapi`
  - model: `gpt-5.4`
  - approval: `never`
  - sandbox: `read-only`
  - 命令完成并返回审查结论：
    - `I did not find a discrete, actionable bug that is provably introduced by this patch.`
- 当前结论：
  - macOS release 的非交互 code review 主链可正常启动、读取工作树 diff、执行只读检查并完成输出

### 6. MCP 配置生命周期

- 使用独立 MCP 配置目录：
  - `/tmp/codex-mcp-home`
- 真实执行：
  - `mcp add self-test -- /tmp/codex-mac-webui-dragfix mcp-server`
    - 结果：`Added global MCP server 'self-test'.`
  - `mcp list`
    - 结果：可见 `self-test`
    - `Command`: `/tmp/codex-mac-webui-dragfix`
    - `Args`: `mcp-server`
    - `Status`: `enabled`
  - `mcp get self-test --json`
    - 结果：返回完整 JSON 配置
  - `mcp remove self-test`
    - 结果：`Removed global MCP server 'self-test'.`
  - 再次 `mcp list`
    - 结果：`No MCP servers configured yet.`

### 7. MCP 认证相关边界

- 在 `self-test` 为 `stdio` MCP server 时：
  - `mcp login self-test`
    - 结果：`OAuth login is only supported for streamable HTTP servers.`
  - `mcp logout self-test`
    - 结果：`OAuth logout is only supported for streamable_http transports.`
- 当前结论：
  - MCP OAuth 登录/登出路径的能力边界明确
  - 对本轮使用的 `stdio` server，这是预期限制，不是运行时崩溃

### 8. `mcp-server` 协议握手与工具调用

- 真实验证方式：
  - 直接启动 `/tmp/codex-mac-webui-dragfix mcp-server`
  - 按仓库测试中的同款 newline-delimited JSON-RPC 协议做初始化，而不是 `Content-Length` framing
- 初始化握手结果：
  - `initialize.protocolVersion = 2025-03-26`
  - `serverInfo.name = codex-mcp-server`
- `tools/list` 结果：
  - 返回 `2` 个工具
  - 工具名：
    - `codex`
    - `codex-reply`
- `tools/call` 结果：
  - 调用 `codex` 工具，prompt 为 `Reply with exactly: mcp-tool-ok`
    - 返回：`mcp-tool-ok`
    - 同时返回 `threadId`
  - 调用 `codex-reply` 工具，沿用上一步 `threadId`
    - 返回：`mcp-reply-ok`
- 当前结论：
  - `mcp-server` 的初始化、工具发现、首轮会话启动、同线程继续回复四条核心路径均已真实通过

## Linux release 补充验证

### 1. Ubuntu 24.04 容器中的 `sandbox linux`

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 /out/codex sandbox linux /bin/echo sandbox-linux-ok`
- 退出码：0
- 结果：
  - 标准输出：`sandbox-linux-ok`
- 当前结论：
  - Linux release 二进制在 Ubuntu 24.04 容器中，`sandbox linux` 子命令最小执行路径可用

### 2. Ubuntu 24.04 容器中的 MCP 配置生命周期

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex mcp add self-test -- /out/codex mcp-server && /out/codex mcp list && /out/codex mcp get self-test --json && /out/codex mcp remove self-test && /out/codex mcp list'`
- 退出码：0
- 结果：
  - `mcp add` 成功添加 `self-test`
  - `mcp list` 能列出 `self-test  /out/codex  mcp-server  ... enabled  Unsupported`
  - `mcp get self-test --json` 返回 `stdio` 传输配置，`command=/out/codex`，`args=["mcp-server"]`
  - `mcp remove` 成功删除
  - 二次 `mcp list` 返回 `No MCP servers configured yet`
- 当前结论：
  - Linux release 二进制在 Ubuntu 24.04 容器中，MCP 配置的新增、查询、删除闭环可用

### 3. Ubuntu 24.04 容器中的 `mcp-server` 协议握手与工具调用

- 真实验证方式：
  - 直接启动 `/tmp/codex-linux-webui-out.OAPQzI/codex mcp-server`
  - 按仓库测试同款 newline-delimited JSON-RPC 协议发送 `initialize`、`tools/call`
- 初始化握手结果：
  - `protocolVersion = 2025-03-26`
  - `serverInfo.name = codex-mcp-server`
- `tools/call` 结果：
  - `codex` 返回：`linux-mcp-tool-ok`
  - `codex-reply` 返回：`linux-mcp-reply-ok`
- 当前结论：
  - Linux release 二进制在 Ubuntu 24.04 容器中，`mcp-server` 的握手、首轮回复、同线程续答均已真实通过

### 4. Ubuntu 24.04 容器中的 MCP OAuth 不支持边界

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex mcp add self-test -- /out/codex mcp-server >/dev/null && /out/codex mcp login self-test; status1=$?; /out/codex mcp logout self-test; status2=$?; /out/codex mcp remove self-test >/dev/null; printf "LOGIN_EXIT=%s\nLOGOUT_EXIT=%s\n" "$status1" "$status2"'`
- 退出码：0
- 结果：
  - `mcp login self-test` 输出：`OAuth login is only supported for streamable HTTP servers.`
  - `mcp logout self-test` 输出：`OAuth logout is only supported for streamable_http transports.`
  - `LOGIN_EXIT=1`
  - `LOGOUT_EXIT=1`
- 当前结论：
  - Linux release 二进制对 `stdio` MCP server 的 OAuth 不支持边界行为与 macOS 一致

### 5. Ubuntu 24.04 容器中的 app-server 产物生成与真实对话

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc "/out/codex debug app-server send-message-v2 'Reply with exactly: linux-debug-app-server-ok'"` 
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v "$tmpdir":/work ubuntu:24.04 bash -lc '/out/codex app-server generate-json-schema --out /work/schema && /out/codex app-server generate-ts --out /work/ts && find /work/schema -type f | wc -l && find /work/ts -type f | wc -l'`
- 退出码：0
- 结果：
  - `debug app-server send-message-v2` 最终返回：`linux-debug-app-server-ok`
  - `thread/start` 返回 `modelProvider = local-newapi`
  - `generate-json-schema` 和 `generate-ts` 均成功
  - 容器内按 `find ... -type f` 统计，生成文件数分别为：
    - `SCHEMA_COUNT=170`
    - `TS_COUNT=519`
- 当前结论：
  - Linux release 二进制在 Ubuntu 24.04 容器中，app-server 的 schema/TS 产物生成和基于 `local-newapi` 的真实消息往返均可用

### 6. Ubuntu 24.04 瘦镜像下 `review --uncommitted` 的环境边界

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -v /Volumes/Work/code/stellarlinkco-codex-src:/repo -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc 'cd /repo && /out/codex review --uncommitted'`
- 结果：
  - Codex 本体能启动并识别：
    - `provider: local-newapi`
    - `workdir: /repo`
  - 但容器缺少 `git`、`python3`、`node` 等依赖，内部检查链路报错：
    - `/bin/bash: line 1: git: command not found`
    - `/bin/bash: line 1: python3: command not found`
- 当前结论：
  - 该场景当前验证到的是 Ubuntu 瘦镜像环境边界，不足以判定 Linux release 的 `review --uncommitted` 成功或失败
  - 若要继续验证，需要切换到带 `git/python3/node` 的 Ubuntu 环境后复测

### 7. Ubuntu 24.04 容器中的 `completion` / `cloud` / `apply` 边界

- 命令：
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out ubuntu:24.04 bash -lc '/out/codex completion zsh >/tmp/codex.zsh && wc -l /tmp/codex.zsh'`
  - `docker run --rm -v /tmp/codex-linux-webui-out.OAPQzI:/out -v /tmp/codex-linux-home.xxYAeR:/codex-home -e CODEX_HOME=/codex-home ubuntu:24.04 bash -lc '/out/codex cloud list; echo "---"; /out/codex apply bogus-task-id'`
- 结果：
  - `completion zsh` 成功生成补全文件，`wc -l` 为 `2509`
  - `cloud list` 输出：`Not signed in. Please run 'codex login' to sign in with ChatGPT, then re-run 'codex cloud'.`
  - `apply bogus-task-id` 输出：`Error: ChatGPT token not available`
- 当前结论：
  - Linux release 二进制可以正常生成 shell completion
  - `cloud` 与 `apply` 的成功路径仍依赖 ChatGPT 登录/token，不属于 `local-newapi` 主链能力

### 8. `review --uncommitted` 的 Linux 根因定位与源码修复

- 复现结论：
  - 在 Ubuntu 24.04 容器中，即使显式传入 `-s danger-full-access -a never` 或 `--dangerously-bypass-approvals-and-sandbox`，`codex review --uncommitted` 的会话头仍显示 `sandbox: read-only`
  - 在该会话里，子进程对 bind mount 进来的 `/repo` 呈现异常只读/不可遍历状态：
    - 能 `stat /repo`
    - 但 `ls /repo`、`find /repo`、`cat /repo/.git/HEAD` 会报 `Permission denied`
    - `git -C /repo status --short` 会退化为 `fatal: not a git repository`
- 根因判断：
  - 入口不在 Linux sandbox 本身，而在 `codex review` 的 CLI 包装层
  - `codex-rs/cli/src/main.rs:600` 之前的实现会重新构造一个新的 `ExecCli`
  - 但只透传了 `-c` / feature toggle 覆盖，没有把根级普通参数如 `--sandbox`、`--dangerously-bypass-approvals-and-sandbox`、`--cd` 等并入 `ExecCli`
  - 结果是 `review` 子命令丢失了根级沙箱覆盖，退回默认 `read-only`
- 源码修复：
  - 在 [main.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/cli/src/main.rs#L600) 的 `Subcommand::Review` 分支加入 `merge_exec_cli_flags(&mut exec_cli, &interactive);`
  - 新增 [main.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/cli/src/main.rs#L1104) `merge_exec_cli_flags`
  - 新增解析级回归测试：
    - [main.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/cli/src/main.rs#L1474) `review_merges_root_sandbox_and_cwd_flags`
    - [main.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/cli/src/main.rs#L1503) `review_merges_root_dangerously_bypass_flag`
- 本地回归验证：
  - `cargo test -p codex-cli` 全量通过
  - 用本地 debug 二进制执行：
    - `codex-rs/target/debug/codex -s workspace-write review --uncommitted`
    - `codex-rs/target/debug/codex --dangerously-bypass-approvals-and-sandbox review --uncommitted`
  - 真实会话头已显示：
    - `sandbox: workspace-write [workdir, /tmp, $TMPDIR, /Users/mison/.codex/memories]`
    - `sandbox: danger-full-access`
- 当前结论：
  - `review` 丢失根级沙箱参数的问题已经在源码层修复，并有自动化测试兜底
  - 旧的 Linux release 二进制仍保留该缺陷，需在下一次 Linux release 重编后二进制复测确认

### 9. 新 mac release 二进制上的 `review` 沙箱回归

- 新产物：
  - `/tmp/codex-mac-reviewfix`
  - 生成时间：`2026-04-15 15:45:59`
- 命令：
  - `/tmp/codex-mac-reviewfix -s workspace-write review --uncommitted`
  - `/tmp/codex-mac-reviewfix --dangerously-bypass-approvals-and-sandbox review --uncommitted`
- 结果：
  - 第一条会话头显示：
    - `sandbox: workspace-write [workdir, /tmp, $TMPDIR, /Users/mison/.codex/memories]`
  - 第二条会话头显示：
    - `sandbox: danger-full-access`
- 当前结论：
  - 修复后的 mac release 二进制已确认继承 `review` 的根级沙箱参数
  - 该缺陷在 mac release 层已闭环，不再停留在 debug 二进制或测试层

### 10. 新 Linux release 二进制上的 `review` 沙箱回归

- 新产物：
  - `/tmp/codex-linux-reviewfix/codex`
  - 文件类型：`ELF 64-bit LSB pie executable, x86-64`
  - 生成时间：`2026-04-15 18:03:00` 左右
- Linux release 构建方式：
  - 在 `rust:1.93` 容器内构建
  - 由于常规 release 配置在当前 Docker 内存条件下会在链接阶段被 `SIGKILL`，本次为验证修复是否进入 Linux release 二进制，采用低内存 release 配置：
    - `CARGO_PROFILE_RELEASE_LTO=off`
    - `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`
    - `CARGO_PROFILE_RELEASE_STRIP=none`
  - 构建耗时约 `29m 08s`
- Ubuntu 24.04 容器起检：
  - 命令：
    - `docker run --rm -v /tmp/codex-linux-reviewfix:/binout -v /tmp/codex-linux-home.xxYAeR:/root/.codex ubuntu:24.04 bash -lc '/binout/codex --version'`
  - 结果：
    - 输出 `codex-cli 1.3.0`
- Ubuntu 24.04 容器中的 `review --uncommitted` 回归：
  - `workspace-write` 命令：
    - `docker run --rm -v /tmp/codex-linux-reviewfix:/binout -v /tmp/codex-linux-home.xxYAeR:/root/.codex -v /Volumes/Work/code/stellarlinkco-codex-src:/repo -w /repo ubuntu:24.04 bash -lc 'timeout 25s /binout/codex -s workspace-write review --uncommitted 2>&1 | sed -n "1,120p"'`
  - `workspace-write` 结果：
    - 会话头显示：
      - `sandbox: workspace-write [workdir, /tmp, $TMPDIR, /root/.codex/memories]`
    - provider 显示：
      - `provider: local-newapi`
  - `danger-full-access` 命令：
    - `docker run --rm -v /tmp/codex-linux-reviewfix:/binout -v /tmp/codex-linux-home.xxYAeR:/root/.codex -v /Volumes/Work/code/stellarlinkco-codex-src:/repo -w /repo ubuntu:24.04 bash -lc 'timeout 25s /binout/codex --dangerously-bypass-approvals-and-sandbox review --uncommitted 2>&1 | sed -n "1,120p"'`
  - `danger-full-access` 结果：
    - 会话头显示：
      - `sandbox: danger-full-access`
    - provider 显示：
      - `provider: local-newapi`
- 运行时额外观察：
  - Ubuntu 24.04 的最小基础镜像默认不带 `git`
  - 因此 `review --uncommitted` 在继续执行仓库检查时会报：
    - `/bin/bash: line 1: git: command not found`
  - 这说明：
    - 新 Linux release 二进制已经正确继承根级沙箱参数
    - 但要在最小 Ubuntu 镜像里完整走通 `review` 成功路径，仍需在镜像中安装 `git`
  - 挂载的 `local-newapi` home 里还出现了历史 `rollout path` 失效日志：
    - `state db returned stale rollout path ...`
    - 这次没有阻断 `review` 启动和沙箱验证，属于已有状态库噪音，不是本次修复引入的新故障
- 补充环境复验：
  - 在 Ubuntu 24.04 容器中额外安装 `git`、并执行 `git config --global --add safe.directory /repo` 后再次复测
  - `danger-full-access` 结果：
    - `review --uncommitted` 已能成功执行真实 Git 检查命令
    - 输出中可见：
      - `git status --short`
      - `git diff --staged --stat`
      - `git diff --stat`
      - `git ls-files --others --exclude-standard`
    - 说明新 Linux release 在 `danger-full-access` 下，已经不再受旧版 `sandbox: read-only` 缺陷影响，真实仓库可读
  - `workspace-write` 结果：
    - 会话头仍正确显示 `sandbox: workspace-write`
    - 但在当前环境组合下，Git 仍报：
      - `fatal: not a git repository (or any parent up to mount point /)`
    - 同一次会话里 `find /repo -name .git` 也未能在 sandbox 子进程视角下找到 `.git`
    - 当前只能保守判断为：
      - 这是 `Ubuntu 24.04 容器 + macOS bind mount + Linux workspace-write sandbox` 组合下的残余环境兼容性问题
      - 它与本次已修复的“`review` 丢失根级沙箱参数”不是同一个缺陷
      - 本轮尚未完成对此残余问题的根因修复
- 当前结论：
  - 修复后的 Linux release 二进制已经确认继承 `review` 的根级沙箱参数
  - 旧 Linux release 的 `sandbox: read-only` 问题在新 Linux release 上已复现消失
  - Linux release 层关于“根级沙箱参数未传入 `review`”这一缺陷的验证已经闭环
  - 但 Ubuntu 容器中的 `workspace-write` 真实 Git 可用性仍有残余环境问题，不能把它表述成“Linux 所有 review 场景都已完全无问题”

### 10. 2026-04-15 进一步隔离结果

- `sandbox linux --full-auto` 在 Ubuntu 容器本地目录 `/tmp/localrepo` 下可正常看到 `.git`
  - 结果：
    - `pwd = /tmp/localrepo`
    - `find /tmp/localrepo -maxdepth 2 -name .git` 返回 `/tmp/localrepo/.git`
- 同样的 `sandbox linux --full-auto` 在 `macOS bind mount` 的 `/repo` 下：
  - `ls -ld /repo` 与 `ls -ld /repo/.git` 可以成功
  - 但 `find /repo -maxdepth 2 -name .git` 返回 `find: '/repo': Permission denied`
- 这说明：
  - 之前 `workspace-write` 下 `.git` 不可遍历的问题，主要集中在 `Ubuntu 24.04 容器 + macOS bind mount + 旧 Linux 沙箱路径` 这个环境组合
  - 不能把它泛化成“Linux 本地文件系统下一律看不到 `.git`”
- 额外对照：
  - 在同一 Ubuntu 容器内构造本地最小 git 仓库 `/tmp/smallrepo`，执行现有 `/out/codex -s workspace-write -a never review --uncommitted`
  - Git 检查命令与 diff 生成都成功，最终给出 review 结论
  - 但会话头仍显示 `sandbox: read-only`
- 当前判断：
  - 这表明容器里当前 `/out/codex` 这份 Linux release 二进制不是最新修复产物，不能继续拿它代表“当前源码对应的 Linux release 验证结果”
  - 后续 Linux `review` 验证应基于重新构建后的最新 Linux 二进制继续

## 真实异常补充

### 1. `completion zsh | head` 的 BrokenPipe 已修复

- 命令：
  - 修复前：`/tmp/codex-mac-webui-dragfix completion zsh | head -n 5`
  - 修复后：`codex-rs/target/debug/codex completion zsh | head -n 5`
- 修复：
  - `codex-rs/cli/src/main.rs` 改为使用 `Shell::try_generate`
  - 对 `ErrorKind::BrokenPipe` 显式视为正常提前结束，不再 panic
  - 新增单元测试 `completion_ignores_broken_pipe`
- 验证：
  - `cargo test -p codex-cli` 通过
  - 真实复测 `completion zsh | head -n 5` 退出码为 `0`
  - 首屏输出：
    - `#compdef codex`
    - `autoload -U is-at-least`
- 当前结论：
  - 补全内容生成链路正常
  - 对下游提早关闭管道的 `BrokenPipe` 已完成收敛，不再属于当前残余问题

### 2. TUI 日志中的 GitHub `403 Forbidden`

- 在 `/tmp/codex-tui-logs-clean/codex-tui.log` 中可见：
  - `GET https://api.github.com/repos/openai/codex/releases/latest` -> `403 Forbidden`
- 当前判断：
  - 这是 release 检查或更新探测相关的 GitHub API 限流/拒绝
  - 不是 `local-newapi` 主对话链路失败
  - 不影响本轮 `tui-local-newapi-ok` 的真实对话结果

## 二进制覆盖矩阵更新 - 2026-04-15

### 1. 本轮新增完成的二进制验证

- `apply_patch`
  - 用临时文件执行真实 patch，`alpha -> alpha-updated` 成功落盘
- `codex-file-search`
  - 在临时目录执行 `--json` 搜索，成功返回 `sub/beta.rs`
- `codex-execpolicy`
  - 用自定义 `prefix_rule(pattern=["echo","ok"])` 规则文件检查 `echo ok`
  - 输出 `decision":"allow"`
- `codex-execpolicy-legacy`
  - `check-json {"program":"/bin/echo","args":["echo","legacy-ok"]}` 返回结构化 JSON：
    - `{"result":"unverified","error":{"type":"NoSpecForProgram","program":"/bin/echo"}}`
  - 说明 legacy JSON 解析与错误返回链路可用
- `codex-write-config-schema`
  - 成功写出 schema 文件，JSON 根对象 `title = "ConfigToml"`
- `codex-file-search`
  - `--json -C . 'config schema' -l 5` 成功返回匹配文件列表
  - 命中包括：
    - `core/src/config/schema.rs`
    - `core/src/config/schema.md`
- `md-events`
  - 成功把 Markdown `# Title` / `- a` / `- b` 解析为 pulldown-cmark 事件流
- `logs_client`
  - 对 `/tmp/codex-linux-home.xxYAeR/logs_1.sqlite` 成功回放最近日志并输出 compact 日志行
  - 已看到真实日志内容：
    - `stream disconnected - retrying sampling request`
    - `Shutting down Codex instance`
- `codex-stdio-to-uds`
  - 对本地 UDS echo server 成功完成 `ping-uds -> ACK:ping-uds` 往返
- `codex-responses-api-proxy`
  - 以前台本地 HTTP server 作为 upstream
  - 代理成功转发 `POST /v1/responses`
  - upstream 侧确认收到 `Authorization: Bearer dummy-key`
  - `GET /shutdown` 返回 `200`
- `codex-exec`
  - 当前重新复测：
    - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR`
    - `--dangerously-bypass-approvals-and-sandbox --ephemeral -o /tmp/codex-exec-latest-final.txt 'Reply with exactly: exec-bin-latest-ok'`
  - stdout 与最终消息文件都返回 `exec-bin-latest-ok`
- `codex-debug-client`
  - 交互式连接当前 `local-newapi` 测试 `CODEX_HOME`
  - 成功收到最终回复：`assistant: debug-client-latest-ok`
  - 当前已观测边界：
    - 启动时会打印多条 `state db returned stale rollout path` 错误日志
    - 但不阻断最小消息往返
- `codex-app-server`
  - 直接启动 `codex-rs/target/debug/codex-app-server`
  - 当前重新复测已按 README 协议手工完成：
    - `initialize -> initialized -> thread/start -> turn/start -> turn/completed`
  - 返回：
    - `THREAD_ID = 019d9449-5878-7f81-bf4c-908ac6302553`
    - `TURN_RESPONSE = true`
    - `FINAL = app-server-direct-latest-ok`
    - `COMPLETED = true`
  - 当前额外观测：
    - app-server 协议的 initialized notification 方法名是 `initialized`
    - MCP 协议常用的 `notifications/initialized` 不适用于这里
    - `thread/start.params.sandbox` 必须使用 kebab-case，例如 `danger-full-access`
    - 传错为 `dangerFullAccess` 时会返回结构化 JSON-RPC 参数错误
- `codex-app-server-test-client`
  - 当前重新复测：
    - `CODEX_HOME=/tmp/codex-linux-home.xxYAeR`
    - `--codex-bin target/debug/codex model-list`
    - `--codex-bin target/debug/codex send-message-v2 'Reply with exactly: app-server-test-client-latest-ok'`
  - `model-list` 成功返回模型目录
  - `send-message-v2` 日志中确认：
    - `AgentMessage.text = "app-server-test-client-latest-ok"`
    - 收到 `turn/completed`
    - `codex app-server` 以 `exit status: 0` 正常退出
- `codex-execve-wrapper`
  - 手工模拟 `CODEX_ESCALATE_SOCKET` 升级协议
  - 当前重新复测时，server 侧确认收到：
    - `file = /bin/echo`
    - `argv = ["echo", "execve-wrapper-latest-ok"]`
  - 给 wrapper 返回 `{"action":"Run"}`
  - wrapper 最终真实 `execv("/bin/echo", ["echo","execve-wrapper-latest-ok"])`
  - stdout 输出 `execve-wrapper-latest-ok`，退出码 `0`
- `export`
  - 成功导出 TS/JSON schema 文件到临时目录
  - 导出文件数：`689`
  - 产物样例包括：
    - `codex_app_server_protocol.v2.schemas.json`
    - `InitializeParams.ts`
    - `v1/InitializeResponse.json`
- `write_schema_fixtures`
  - 成功写出 `typescript/` 与 `json/` fixture 文件
- `test_notify_capture`
  - 成功把 payload 原子写入目标 JSON 文件
  - 最终文件内容：`{"event":"notify-ok"}`
- `rmcp_test_server`
  - 通过 stdio MCP JSON-RPC 完成 `initialize -> notifications/initialized -> tools/list`
  - 返回 `tool_names = ["echo"]`
  - 当前重新复测时返回：
    - `serverInfo.name = "rmcp"`
    - `serverInfo.version = "0.15.0"`
- `test_stdio_server`
  - 通过 stdio MCP JSON-RPC 完成 `initialize -> notifications/initialized -> tools/list`
  - 返回 `tool_names = ["echo", "image", "image_scenario"]`
  - 当前重新复测时返回：
    - `protocolVersion = "2025-03-26"`
    - `serverInfo.name = "rmcp"`
- `test_streamable_http_server`
  - 当前重新复测时实际启动在 `127.0.0.1:3920/mcp`
  - `GET /.well-known/oauth-authorization-server/mcp` 返回 OAuth metadata JSON
- `codex-mcp-server`
  - 直接启动 `codex-rs/target/debug/codex-mcp-server`
  - 通过 newline-delimited JSON-RPC 完成 `initialize -> notifications/initialized -> tools/list`
  - 返回：
    - `protocolVersion = 2025-03-26`
    - `serverInfo.name = codex-mcp-server`
    - `serverInfo.version = 1.3.0`
    - `tool_names = ["codex", "codex-reply"]`

### 2. 与既有验证合并后的覆盖结论

- `codex`
  - 已有 mac release、Linux release、TUI、WebUI、app-server、review、mcp、cloud/apply/completion、sandbox 等多条真实验证证据
- `codex-tui`
  - 已有 macOS 交互式真实对话与 `resume --last` / `fork --last` 验证
  - 已有 Linux release trusted 目录下的真实对话、`resume` 与 `fork` 关键路径验证
- `codex-mcp-server`
  - 当前 debug 产物已完成直接握手与 `tools/list`
  - 既有文档中另有 Linux release 直接握手与工具调用验证
- `codex-linux-sandbox`
  - 已有 Ubuntu 24.04 release 二进制 `sandbox linux /bin/echo sandbox-linux-ok` 验证

### 3. 当前已完成 / 未覆盖边界

- 已完成：
  - 当前仓库中所有 macOS/Linux 可运行的非 Windows 二进制，均已获得真实功能验证证据
  - 其中用户链路型程序至少有一条真实工作流验证，不仅是 `--help`
- 当前环境未覆盖：
  - `codex-command-runner`
  - `codex-windows-sandbox-setup`
  - 原因：两者为 Windows 专用二进制，当前只有 macOS 主机与 Linux 容器验证环境，无法做真实运行验证

## 二进制覆盖矩阵补充更新 - 2026-04-16

### 1. 辅助二进制的次级真实分支补证

- `codex-file-search`
  - 补测命令：
    - `codex-rs/target/debug/codex-file-search --json --compute-indices -C codex-rs main`
  - 结果：
    - 成功返回带 `indices` 的 JSON 行结果，而不是仅文件名文本输出
    - 样例结果：
      - `{"score":109,"path":"cli/src/main.rs","match_type":"file","root":"codex-rs","indices":[8,9,10,11]}`
- `codex-execpolicy`
  - 使用临时策略文件补测 `--resolve-host-executables` 分支：
    - `host_executable(name = "git", paths = ["/usr/bin/git"])`
    - `prefix_rule(pattern = ["git", "status"], decision = "allow", match = [["git", "status"]])`
  - 执行：
    - `codex-rs/target/debug/codex-execpolicy check --rules /tmp/codex-execpolicy-extra.codexpolicy --resolve-host-executables /usr/bin/git status`
  - 结果：
    - 返回 `decision = "allow"`
    - `matchedPrefix = ["git","status"]`
    - `resolvedProgram = "/usr/bin/git"`
- `codex-execpolicy-legacy`
  - 补测 `--require-safe` 对非 safe 命令的退出语义：
    - `codex-rs/target/debug/codex-execpolicy-legacy --require-safe check cp src.txt dest.txt`
  - 结果：
    - stdout 返回 `result = "match"`
    - 进程退出码为 `12`
    - 说明 legacy 引擎对“命中可写路径但未达 safe”场景的 CLI 退出语义正常

### 2. stdio MCP 测试 server 的额外协议分支补证

- `rmcp_test_server`
  - 通过 Python 驱动 stdio JSON-RPC 完成：
    - `initialize -> notifications/initialized -> tools/list -> tools/call(echo)`
  - 补测结果：
    - `structuredContent.echo = "rmcp-call-ok"`
    - `structuredContent.env = "manual-mcp-env"`
  - 说明：
    - 不仅能列出工具，也能真实执行 `echo` 工具并读取环境变量
- `test_stdio_server`
  - 通过 Python 驱动 stdio JSON-RPC 完成：
    - `initialize -> notifications/initialized -> tools/list -> resources/list -> resources/read -> tools/call(image_scenario)`
  - 补测结果：
    - `resources/list` 返回 `memo://codex/example-note`
    - `resources/read` 返回文本：
      - `This is a sample MCP resource served by the rmcp test server.`
    - `tools/call(image_scenario)` 返回：
      - 第一块 `content.type = "text"`，`text = "cap"`
      - 第二块 `content.type = "image"`，`mimeType = "image/png"`
  - 说明：
    - 该二进制已不仅验证 `tools/list`，还补齐了资源读取与图像内容块输出分支

### 3. 当前口径更新

- 可以确认：
  - 当前 macOS/Linux 可运行的非 Windows Rust 二进制，都至少有一条真实功能路径证据
  - 本轮又补充了若干此前未单独落证的“次级功能分支”
- 仍不能确认：
  - “所有二进制程序的所有参数组合、所有错误分支、所有交互分支都已穷举测试完成”
  - 尤其是 `codex` / `codex-tui` / `codex-mcp-server` / `test_streamable_http_server` 这类交互面较大的程序，当前只能说关键路径和部分次级路径已有真实证据，不能说已穷举

## 并行验证补充 - 2026-04-16

### 1. CLI / TUI Linux 补充证据

- `resume --last` 的文本位置参数误用：
  - 命令：
    - `docker run ... ubuntu:24.04 bash -lc '/out/codex resume --last "Reply with exactly: should-be-session-id"'`
  - 结果：
    - 退出码 `1`
    - 关键输出：`Error: stdin is not a terminal`
  - 结论：
    - 在当前 Linux/Ubuntu 非 TTY 调用条件下，顶层 `codex resume` 先撞到 TUI 运行边界，而不是形成 clap 参数错误
    - 因此不能把“`resume --last` 一定返回 clap 参数错误”写成已验证结论
- `fork --last` 的文本位置参数误用：
  - 命令：
    - `docker run ... ubuntu:24.04 bash -lc '/out/codex fork --last --no-alt-screen -C /tmp "Reply with exactly: should-be-session-id"'`
  - 结果：
    - 退出码 `2`
    - 关键输出：
      - `error: the argument '--last' cannot be used with '[SESSION_ID]'`
  - 结论：
    - `fork --last` 下直接追加文本位置参数，会真实触发 clap 参数冲突
- `exec` 在 untrusted / non-git 且未加 `--skip-git-repo-check`：
  - 命令：
    - `docker run ... ubuntu:24.04 bash -lc 'cd /work && /out/codex exec "Reply with exactly: should-fail-git-check"'`
  - 结果：
    - 退出码 `1`
    - 关键输出：
      - `Not inside a trusted directory and --skip-git-repo-check was not specified.`
  - 结论：
    - 该真实拒绝分支成立
- Linux release TUI 非 TTY 边界：
  - 命令：
    - `docker run ... ubuntu:24.04 bash -lc '/out/codex --no-alt-screen --dangerously-bypass-approvals-and-sandbox -C /work "Reply with exactly: cli-nontty-unknown-ok"'`
  - 结果：
    - 退出码 `1`
    - 关键输出：
      - `Error: stdin is not a terminal`
  - 结论：
    - Linux release 的 TUI 在非 TTY 下明确拒绝
- unknown trust 目录的 trust onboarding：
  - 交互式界面已观察到：
    - `Do you trust the contents of this directory?`
    - `1. Yes, continue`
    - `2. No, quit`
  - 结论：
    - unknown trust 目录下会真实进入 trust onboarding，而不是直接跳过

### 2. WebUI / Linux release 的认证口径补充

- 先前一条子任务脚本曾得到：
  - `/api/sessions` 未在等待窗口内就绪
  - `/api/kanban` 未返回 `200`
- 主线程复核后确认：
  - 上述结论不能直接作为后端失败证据，因为该脚本是未带 token 的直接 `curl`
  - 文档中既有浏览器实测口径一直是：
    - 打开 `http://127.0.0.1:<port>/kanban?token=<redacted>`
    - 然后浏览器网络面板确认 `GET /api/sessions` -> `200`、`GET /api/kanban` -> `200`
- 主线程重新起 Linux release WebUI 并带 token 直连 API：
  - `/api/sessions?token=<token>` -> `HTTP=200`
  - `/api/kanban?token=<token>` -> `HTTP=200`
  - 响应样例：
    - `/api/sessions` 返回 `{"sessions":[...]}`
    - `/api/kanban` 返回 `{"columns":[...],"cardPositions":{...}}`
- 结论：
  - 这轮 WebUI 子任务里“API 未就绪”的失败，更准确地说是认证/调用方式不成立，而不是当前 Linux release 的 `/api/sessions` 或 `/api/kanban` 已确认损坏

### 3. app-server 次级请求路径补充

- 使用本地 debug `codex-app-server-test-client` 和 debug `codex`，在 `CODEX_HOME=/tmp/codex-linux-home.xxYAeR` 下执行：
  - `codex-rs/target/debug/codex-app-server-test-client --codex-bin .../codex thread-list`
- 结果：
  - 成功完成 `initialize -> initialized -> thread/list`
  - 返回多条历史线程
  - 已确认字段包括：
    - `modelProvider = "local-newapi"`
    - `id`
    - `path`
    - `preview`
    - `source`
- 结论：
  - app-server/test-client 的次级请求路径不再只停留在基础握手或 `send-message-v2`
  - `thread-list` 已获得真实运行证据

### 4. 当前未收敛门禁

- `cargo test -p codex-rmcp-client streamable_http_404_session_expiry_recovers_and_retries_once -- --exact --nocapture`
  - 本轮先后遇到：
    - Cargo 产物目录锁竞争
    - 首轮长时间编译
    - 二次复测时测试二进制长时间停留在执行态，未在本轮收口前形成可审计通过/失败结论
- 当前结论：
  - 不能把这条 `404 session recovery` 写成“已通过”
  - 也不能仅凭挂起现象就写成“功能失败”
  - 应继续视为未收敛的测试门禁，而非已定性的产品缺陷

## 补充推进 - 2026-04-16 续

### 1. `rmcp-client` streamable HTTP 恢复测试的最终收敛结果

- 之前挂起的根因不是功能失败，而是：
  - `cargo test` 调度层存在 Cargo 锁竞争与长编译噪声
  - 从 Cargo 入口观察时，信息密度太低，容易误判为测试本身挂住
- 主线程改为直接运行已编好的集成测试二进制：
  - `codex-rs/target/debug/deps/streamable_http_recovery-4431435dd305fbe9 streamable_http_404_session_expiry_recovers_and_retries_once --exact --nocapture`
  - 结果：
    - `running 1 test`
    - `test streamable_http_404_session_expiry_recovers_and_retries_once ... ok`
    - `test result: ok. 1 passed; 0 failed; ... finished in 0.52s`
  - stderr：
    - `starting rmcp streamable http test server on http://127.0.0.1:<port>/mcp`
- 同样直接运行：
  - `... streamable_http_401_does_not_trigger_recovery --exact --nocapture`
  - 结果：
    - `test streamable_http_401_does_not_trigger_recovery ... ok`
    - `test result: ok. 1 passed; 0 failed; ... finished in 0.06s`
- 结论更新：
  - `test_streamable_http_server` 对应的 `404 session expiry recovery` 与 `401 does not trigger recovery` 都已获得真实通过证据
  - 先前的“未收敛门禁”应收缩为：Cargo 调度层观测噪声，而非功能未通过

### 2. `test_streamable_http_server` bearer 鉴权边界的黑盒 HTTP 证据

- 直接启动：
  - `MCP_STREAMABLE_HTTP_BIND_ADDR=127.0.0.1:<port>`
  - `MCP_EXPECT_BEARER=test-bearer`
  - 二进制：`codex-rs/target/debug/test_streamable_http_server`
- 黑盒请求结果：
  - `GET /.well-known/oauth-authorization-server/mcp`（无鉴权） -> `HTTP 200`
  - `GET /mcp`（无鉴权） -> `HTTP 401`
  - `GET /mcp`（带 `Authorization: Bearer test-bearer` 但未声明 `Accept: text/event-stream`） -> `HTTP 406`
    - body: `Not Acceptable: Client must accept text/event-stream`
- 结论：
  - well-known OAuth metadata 路径确实免鉴权
  - `/mcp` 路径确实受 bearer 保护
  - 带对 bearer 后，下一层会继续校验 streamable HTTP 的 `Accept` 头，而不是静默放过

### 3. Ubuntu 容器中 `review --uncommitted` 的边界进一步细化

- 之前的阻塞口径是：
  - 容器内 `apt-get install git` 链路过慢，未进入 `codex review`
- 主线程改为：
  - 在宿主先建好带 `.git` 的临时仓库
  - 再挂进 `ubuntu:24.04` 容器，避免装包链路噪声
- 挂到 `/tmp/repo` 后的 pseudo-TTY 运行结果：
  - `review --uncommitted` 会真实启动并进入诊断链路
  - 多个内部 `exec` 明确失败为：
    - `/bin/bash: line 1: git: command not found`
  - 同时后续探测还观察到：
    - `find: ‘/tmp/repo’: Permission denied`
  - 最终 `timeout 20s` 触发，退出码 `124`
- 在 non-git 目录挂载场景下：
  - `review --uncommitted` 首先撞到目录信任门槛：
    - `Not inside a trusted directory and --skip-git-repo-check was not specified.`
  - 退出码 `1`
- 结论：
  - Ubuntu 容器下 `review --uncommitted` 的未收敛点已经更具体：
    - 在 git repo 场景，真实依赖容器内 `git` 可执行文件，且挂载目录权限/可见性也可能影响后续诊断
    - 在 non-git 场景，当前首先命中的是 trusted directory / git repo check 边界
  - 因此不能再把这类问题笼统记为“apt 装包阻塞”；更准确的说法是“Ubuntu 容器环境依赖与挂载权限边界尚未完全收敛”

### 4. Ubuntu 24.04 + `git` 固定镜像下的进一步隔离

- 为避免每次都把“apt 安装耗时”混入验证链路，主线程先构建了临时镜像：
  - `codex-ubuntu-review:24.04`
  - 基础：`ubuntu:24.04`
  - 额外安装：`git`、`bsdutils`
- 构建结果：
  - 镜像已成功构建
  - 说明此前的 Ubuntu 装包链路不是永久性外部阻塞，只是一次性环境准备成本

- 在该固定镜像中，把宿主 git 仓库挂到 `/tmp/repo` 后执行：
  - `timeout 30s script -qec "/out/codex -s workspace-write -a never review --uncommitted" /dev/null`
- 结果：
  - `codex review` 正常启动
  - 但内部 Git 诊断明确失败为：
    - `fatal: not a git repository (or any parent up to mount point /tmp)`
    - `Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).`
  - 同一轮内部探测还观察到：
    - `.git` 目录本身存在
    - 但在 Codex 子进程沙箱视角下，`ls .git` 返回 `Permission denied`
  - 最终超时退出：`EXIT=124`

- 对照验证：
  - 直接在容器普通 shell 中运行：
    - `cd /tmp/repo && git status --short`
  - 结果：
    - 正常输出 ` M README.md`
  - 说明：
    - 容器中的 git 与挂载仓库本身都是可用的
    - 问题不在宿主仓库损坏，也不在 Ubuntu 容器缺 git

- 再做 `danger-full-access` 对照：
  - `timeout 30s script -qec "/out/codex --dangerously-bypass-approvals-and-sandbox review --uncommitted" /dev/null`
  - 结果：
    - `git -C /tmp/repo status --short` 成功
    - `git -C /tmp/repo diff --stat` 成功
    - 最终 review 正常完成，退出码 `0`
  - 说明：
    - 在同一 Ubuntu 24.04 + git 镜像、同一挂载仓库下，`danger-full-access` 能通过，而 `workspace-write` 不能
    - 因此当前更准确的边界是：
      - Docker 挂载仓库在 Codex Linux 沙箱的 `workspace-write` 视角下，对 `.git` 的可见性 / Git 发现存在限制
      - 这不是“Ubuntu 缺 git”或“仓库本身损坏”

- 再做 `exec` 级别的最小对照：
  - `codex -s workspace-write -a never exec "git -C /tmp/repo status --short"`
  - 结果：
    - 返回：
      - `fatal: not a git repository (or any parent up to mount point /tmp)`
      - `Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).`
  - 加 `GIT_DISCOVERY_ACROSS_FILESYSTEM=1` 后：
    - 返回：
      - `fatal: not a git repository (or any of the parent directories): .git`
  - 说明：
    - 仅打开跨文件系统发现还不够
    - Git 发现跨越 mountpoint 后，仍在 `.git` 可见性这一层失败

- 更新结论：
  - Ubuntu 24.04 容器中的 `review --uncommitted`，在“挂载宿主 git 仓库 + `workspace-write` 沙箱”这一组合下，当前主要是 `.git` 可见性 / Git 发现边界
  - 同组合在 `danger-full-access` 下可正常工作
  - 这条边界现已具备较完整的对照证据，可以作为后续是否修复 Linux sandbox / mounted repo 行为的明确入口

### 5. `--skip-git-repo-check` 的当前 CLI 口径

- 在 Ubuntu 24.04 + git 固定镜像中，尝试：
  - `codex review --skip-git-repo-check --uncommitted`
  - `codex --skip-git-repo-check review --uncommitted`
- 结果：
  - 两者都返回 clap 参数错误：
    - `error: unexpected argument '--skip-git-repo-check' found`
    - 退出码 `2`
- 结论：
  - 当前 release 口径下，`--skip-git-repo-check` 不是 `review` 路径可用的公开 CLI 参数
  - 因此 non-git 场景不能通过该参数绕过 `review` 的前置目录 / git 检查

### 6. Docker Desktop `fakeowner` 挂载与容器本地 ext4 的进一步对照

- 在继续收敛 Linux sandbox 行为时，主线程对 `codex-rs/linux-sandbox/src/bwrap.rs` 做了三类最小修复尝试：
  - 把只读 carveout 从路径级 `--ro-bind` 改成 fd 级 `--ro-bind-fd`
  - 把 writable root 从路径级 `--bind` 改成 fd 级 `--bind-fd`
  - 对嵌套 writable roots 按“父路径先、子路径后”排序，并在 bwrap 层裁剪与工作目录嵌套的 `/tmp` writable root
- 其中还补了最小测试草案，覆盖：
  - writable root 的只读子路径使用 fd bind
  - nested writable roots 的父子顺序
  - `/tmp` 父 root 在存在更具体工作目录 child root 时被裁剪

- 在 Ubuntu 24.04 + git 固定镜像中，直接挂载宿主仓库到 `/tmp/repo`，再用最新调试版 Linux 二进制执行：
  - `script -qec "/out/codex -s workspace-write -a never review --uncommitted" /dev/null`
- 结果：
  - `sandbox: workspace-write [workdir, /tmp, $TMPDIR, /root/.codex/memories]`
  - `git status --short` 仍报：
    - `fatal: not a git repository (or any parent up to mount point /tmp)`
    - `Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).`
  - 进一步探测表明：
    - `stat /tmp/repo` 与 `stat /tmp/repo/.git` 成功
    - `stat /tmp/repo/README.md` 也成功
    - 但 `ls -la /tmp/repo` 返回 `Permission denied`
    - `find /tmp/repo -maxdepth 2 ...` 无法枚举目录内容
    - `head /tmp/repo/.git/HEAD` / `head /tmp/repo/.git/config` 返回 `Permission denied`
- 结论：
  - 这时问题已经不再只是 `.git` carveout 或 nested writable roots 顺序
  - 更准确的边界是：
    - Docker Desktop `fakeowner` 挂载目录进入 bwrap 后，目录 `stat` / 已知路径 `stat` 可以成功
    - 但目录遍历（`readdir`）与 `.git` 文件读取仍失败

- 做容器本地 ext4 对照：
  - 先执行：
    - `cp -R /mnt/repo /tmp/repo-local`
  - 再在 `/tmp/repo-local` 执行同一条：
    - `script -qec "/out/codex -s workspace-write -a never review --uncommitted" /dev/null`
- 结果：
  - `git status --short` 成功输出：
    - ` M README.md`
  - `git diff --stat` 与 `git diff` 成功输出真实差异
  - 最终 review 正常完成，退出码 `0`
  - agent 最终结论为：
    - 当前改动仅影响 `README.md`，未发现可执行代码层面的正确性问题
- 结论：
  - 同一 Ubuntu 24.04 + git 固定镜像、同一 `workspace-write` 沙箱、同一份仓库内容：
    - 宿主 fakeowner 挂载目录：失败
    - 复制到容器本地 ext4 后：成功
  - 因此当前剩余未收敛点应更准确记录为：
    - Docker Desktop `fakeowner` 挂载目录与 Codex Linux bwrap sandbox 的目录遍历 / `.git` 文件可读性边界
  - 不能把这条边界泛化成“Linux sandbox 在 git repo 上普遍不可用”
