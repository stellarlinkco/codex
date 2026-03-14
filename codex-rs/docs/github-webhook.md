# `codex serve` GitHub webhook 运行说明

当 `config.toml` 中启用 `[github_webhook] enabled = true` 时，`codex serve` 会在同一个进程/同一个端口上提供：

- Web UI
- `/api/*`
- `POST /github/webhook`（GitHub webhook 入口）

`POST /github/webhook` 不走 Web UI token 鉴权；只依赖 GitHub HMAC（`X-Hub-Signature-256`）+ allowlist + permission checks（与原 `codex github` 行为一致）。

## 支持的来源

当前支持三类 webhook 来源：

- repo webhook
- organization webhook
- GitHub App webhook

默认全部允许；如果配置了 `[github_webhook].sources`，只会接受被显式允许的来源。

## 支持的事件

当前支持的 GitHub 事件：

- `issue_comment`
- `issues`
- `pull_request`
- `pull_request_review_comment`
- `pull_request_review`
- `push`

触发规则：

- `issue_comment` / `pull_request_review_comment`：评论正文以命令前缀开头时触发
- `pull_request_review`：review body 中任意一行以命令前缀开头时触发
- `issues`：issue body 以命令前缀开头时触发
- `pull_request`：PR body 以命令前缀开头时触发
- `push`：`head_commit.message` 以命令前缀开头时触发

默认命令前缀是 `/codex`。如果没有 `[github_webhook]` 配置，事件面保持旧行为，只启用评论 / review 三类事件。

可见反馈：

- `issue_comment` / `pull_request_review_comment` 在请求入队后会优先给触发评论加 `eyes` reaction
- 其他可回复场景会尽快发一条短评论 / review，表示已收到并开始处理
- 如果后台执行失败，或在入队前遇到 `busy` / delivery claim / permission check 内部错误，Codex 会尽量回贴失败通知
- `push` 没有回复目标，所以不会发 ack 或失败通知

## 配置方式

非敏感默认值可以写进 `config.toml`：

```toml
[github_webhook]
enabled = true
webhook_secret_env = "GITHUB_WEBHOOK_SECRET"
github_token_env = "GITHUB_TOKEN"
github_app_id_env = "GITHUB_APP_ID"
github_app_private_key_env = "GITHUB_APP_PRIVATE_KEY"
auth_mode = "auto"
min_permission = "read"
allow_repos = ["owner/repo"]
command_prefix = "/codex"
delivery_ttl_days = 7
repo_ttl_days = 0
sources = ["repo", "organization", "github-app"]

[github_webhook.events]
issue_comment = true
issues = true
pull_request = true
pull_request_review = true
pull_request_review_comment = true
push = true
```

优先级是：CLI overrides（例如 `-c key=value`） > `config.toml` > 内置默认值。

提示：GitHub Kanban 同步优先使用 `allow_repos` 作为同步目标；如果未配置，会尝试从启动 `codex serve` 的当前目录读取 `git remote origin` 推断一个 repo。

## 环境变量

默认读取以下环境变量：

- `GITHUB_WEBHOOK_SECRET`：用于校验 `X-Hub-Signature-256`
- `GITHUB_TOKEN`：静态 token 模式下用于 REST API 和 clone/fetch
- `GITHUB_APP_ID`：GitHub App ID
- `GITHUB_APP_PRIVATE_KEY`：GitHub App PEM 私钥（支持原始 PEM 或 base64）

`auth_mode`：

- `token`：只使用 `GITHUB_TOKEN`
- `github-app`：只使用 GitHub App，自动生成 JWT 并换 installation token
- `auto`：优先使用 GitHub App installation token；不可用时回退到 `GITHUB_TOKEN`

GitHub App 模式下：

- GitHub App webhook 直接读取 payload 里的 `installation.id`
- repo / organization webhook 会自动查询仓库或组织对应的 installation

## GitHub 侧怎么配

### Repo / Organization webhook

- Payload URL：你的公网入口 + `/github/webhook`，例如 `https://example.ngrok.app/github/webhook`
- Content type：`application/json`
- Secret：与本地 `GITHUB_WEBHOOK_SECRET` 一致

如果你只想匹配当前实现，至少勾选：

- `Issue comments`
- `Issues`
- `Pull requests`
- `Pull request reviews`
- `Pull request review comments`
- `Pushes`

### GitHub App

GitHub App 的 webhook URL 也指向同一个公网入口，例如：

- `https://example.ngrok.app/github/webhook`

注意：

- 只有安装覆盖到的仓库才会投递 GitHub App webhook
- GitHub App 需要至少具备 `Metadata: read`、`Contents: read`，以及按回帖路径所需的 `Issues: write` / `Pull requests: write`

## 本地目录布局

仓库缓存和工作目录都放在 `CODEX_HOME` 下：

- repo cache：`~/.codex/github-repos/<owner>/<repo>/repo`
- issue worktree：`~/.codex/github-repos/<owner>/<repo>/issues/<number>`
- pull worktree：`~/.codex/github-repos/<owner>/<repo>/pulls/<number>`
- push worktree：`~/.codex/github-repos/<owner>/<repo>/pushes/<hash>`
- thread state：`~/.codex/github/threads/<owner>/<repo>/...`
- delivery markers：`~/.codex/github/deliveries/*.marker`

`push` 事件按分支哈希复用 worktree；同一分支后续 push 会复用同一个工作目录。

## 运行时行为

收到有效 webhook 后，`codex serve`（内嵌 GitHub runtime）会：

1. 校验 HMAC 签名
2. 识别 webhook 来源并检查是否允许
3. 检查事件是否启用
4. 校验 repo allowlist（如果配置了 `allow_repos` / `--allow-repo`）
5. 校验 sender 是否满足最小仓库权限要求
6. 认领 delivery；对可回复目标先发送 ack（reaction 或短评论 / review）
7. clone / fetch 仓库并准备 issue / pull / push worktree
8. 拉取 GitHub 上下文并写入 `.codex_github_context.md`
9. 在对应 worktree 里运行 Codex
10. 将结果回贴到 issue / PR 评论；`push` 事件只执行，不主动回帖

## 清理与存储

- delivery markers 默认 `7` 天 TTL
- repo cache 默认不自动删除（`repo_ttl_days = 0`）
- repo cache 只有在 `issues/`、`pulls/`、`pushes/` 都为空时才会被 GC

## 常见问题

### `401 bad signature`

通常是：

- GitHub 侧 secret 和本地 `GITHUB_WEBHOOK_SECRET` 不一致
- 反向代理改写了 body

### `permission check failed`

通常是：

- 当前认证模式无法获取目标仓库权限信息
- `GITHUB_TOKEN` 或 GitHub App installation 没有覆盖目标仓库
- sender 的实际仓库权限低于 `min_permission`

### `git clone failed`

通常是：

- 当前 token 无法 clone 该仓库
- 本机 `gh` 未登录或登录到错误账号
- 内网环境没有把 `github.com` 网络打通

## 推荐启动方式

静态 token：

```bash
export GITHUB_WEBHOOK_SECRET=your-secret
export GITHUB_TOKEN=your-token
codex serve --host 127.0.0.1 --port 8787
```

GitHub App：

```bash
export GITHUB_WEBHOOK_SECRET=your-secret
export GITHUB_APP_ID=123456
export GITHUB_APP_PRIVATE_KEY='-----BEGIN RSA PRIVATE KEY-----
...'
codex serve --host 127.0.0.1 --port 8787 -c github_webhook.auth_mode="github-app"
```

如果你只允许某个仓库触发：

```bash
codex serve --host 127.0.0.1 --port 8787 -c github_webhook.allow_repos='["owner/repo"]'
```

注意：融合模式下 `github_webhook.listen` 被忽略；实际监听地址由 `codex serve --host/--port` 决定。
