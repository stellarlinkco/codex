# Product Requirements Document: GitHub Kanban Board (codex serve, embedded GitHub webhook)

**Version**: 3.1  
**Date**: 2026-03-13  
**Author**: Sarah (Product Owner)  
**Status**: Final

---

## Executive Summary

在现有 `codex serve` Web UI 中提供一个 GitHub 看板视图：以 **GitHub Issue / Pull Request** 作为卡片（而不是 codex session），用户可以拖拽卡片在工作流列之间移动，并触发 Codex 的异步工作。

该方案将 GitHub webhook 能力 **融合进 `codex serve`**（统一入口、单进程、单端口）：

- `codex serve` 负责：接收 GitHub webhook（`POST /github/webhook`）、拉取/缓存 GitHub 上下文、准备 worktree、运行 Codex（异步工作）、持久化状态，并提供 Web UI + Kanban 交互（拖拽、持久化位置、SSE 同步）。
- `config.toml` 中配置 `[github_webhook]` 后，仅需启动一次 `codex serve` 即可启用 GitHub webhook + Kanban。

目标是把“看板上的每张卡片 = GitHub work item”，让看板成为 GitHub 驱动的任务面板，而不是会话状态面板；`codex github` CLI 子命令在融合后不再提供。

体验参考：`BloopAI/vibe-kanban` 的信息架构（看板 + 详情抽屉 + 执行日志/模型/提示词 per-task），但不照搬其“workspace / MCP”实现。

---

## Problem Statement

**Current Situation**: 当前看板实现展示的是 codex sessions，并包含 `thinking/active/idle` 等会话运行态。该呈现不等同于 GitHub 的 Issue/PR 工作流；用户无法在看板里直接看到“有哪些 Issue/PR 需要做”，也无法用拖拽触发针对某个 Issue/PR 的异步执行。

**Proposed Outcome**: 用户在 `config.toml` 启用 `[github_webhook]` 后启动 `codex serve`，打开 Kanban 页面即可看到目标仓库的全部 Issue/PR。拖拽卡片到特定列会触发 Codex 异步工作（例如进入 In Progress 即开始处理、进入 Review 即请求审查/生成 PR 说明等），并能在 UI 中看到执行状态的回传。

---

## Goals

- G1: Kanban 卡片来源切换为 GitHub Issue/PR（统一称为 work item）。
- G2: `codex serve`（内嵌 GitHub webhook/runtime）提供“同步最新 Issue/PR”能力，供 Kanban 使用。
- G3: Kanban 支持拖拽移动与列内排序，并持久化。
- G4: 拖拽到特定列可触发 Codex 异步工作，并能看到 job 状态（queued/running/succeeded/failed）。
- G5: 多标签页通过 SSE 同步看板状态与 job 状态。
- G6: 支持多 repo 同步与过滤（repo 维度筛选），并可在一个看板中汇总展示。
- G7: 每个 work item 支持独立的“执行 prompt 前缀”和“模型选择”（不是全局）。
- G8: 看板内可预览 issue/PR 内容（至少 body），不强制跳转 GitHub。
- G9: UI 可查看每次 job 的执行日志（stdout/stderr 或等价输出），便于定位失败原因。
- G10: 支持“关闭任务”（关闭 GitHub Issue/PR，或至少提供显式关闭动作），并允许过滤 closed items。

---

## Non-Goals

- NG1: 不在 Web 前端直接调用 GitHub API；所有 GitHub 交互通过 `codex serve` 后端完成（鉴权/限流/缓存复用现有 GitHub runtime 实现）。
- NG2: 不在 v2 实现复杂过滤器（label/assignee/milestone/全文搜索）；只做最小可用筛选（repo + open/closed + 简单搜索可选）。
- NG3: 不实现多用户协作与权限模型（本地单用户工具链）。
- NG4: 不强制写回 GitHub（comment/review/reaction）的策略；写回行为以可配置为准。
- NG5: 不在 MVP 实现“停止正在运行的 job”（取消/中止）——若要支持需要明确进程管理与幂等语义。

---

## Key Concepts & Data Model

### Work Item

一个 GitHub work item 是 Issue 或 Pull Request：

- `repo`: `"owner/repo"`
- `kind`: `"issue" | "pull"`
- `number`: `u32`
- `title`: `string`
- `state`: `"open" | "closed" | "merged"(optional)`
- `url`: `string`
- `updatedAt`: `unix_ms`
- `labels`: `{ name, color }[]` (optional)
- `comments`: `u32` (optional)

`workItemKey`（稳定主键）：

```
<repo>#<number>:<kind>
e.g. openai/codex#123:issue
```

### Board State (Persisted)

看板元数据（列定义 + 卡片位置）由 `codex serve` 维护并持久化到本地（仍然保持 KISS JSON 文件，不引入 DB）。

- `columns`: 默认 `Backlog / In Progress / Review / Done`
- `cardPositions`: `workItemKey -> { columnId, position }`
- `cardSettings`: `workItemKey -> { promptPrefix?, model? }`（per-item 配置，覆盖全局默认）

### Job State

拖拽触发的异步工作以 job 表示：

- `jobId`: string
- `workItemKey`
- `status`: queued | running | succeeded | failed | canceled
- `createdAt`, `startedAt`, `finishedAt` (optional)
- `lastError` (optional)
- `resultSummary` (optional, short)

job 的执行由 `codex serve` 内嵌的 GitHub runtime 负责；`codex serve` 负责展示状态并触发/管理请求。

---

## Functional Requirements

### R1: GitHub 同步（codex serve）

- `codex serve` 提供“同步 work items”能力：拉取目标 repo(s) 的 Issue/PR 列表并落盘为快照（支持多个 repo）。
- `codex serve` 从快照读取，渲染看板。
- 同步策略：启动时同步 + 固定间隔刷新（默认 5 分钟），或由用户手动触发一次同步。

### R2: 看板渲染与拖拽（codex serve）

- Kanban 展示 Issue/PR 卡片。
- 卡片可拖拽跨列移动、列内排序；操作持久化并通过 SSE 同步。
- 卡片不展示 session 的 `idle` 等运行态；改为展示 GitHub 元信息 + job 状态。
- 支持 open/closed 过滤；默认仅展示 open，closed work item 不触发 job。
- 支持 repo 维度筛选（在一个看板中汇总展示时，用户可按 repo 收敛视图）。

### R2.1: 任务详情预览

- 点击卡片打开“详情抽屉/侧栏”，展示 work item 的 title、labels、状态、更新时间与 Markdown body 预览（至少 issue body）。
- 详情视图提供跳转 GitHub 的链接，但不要求跳转才能看正文。

### R3: 拖拽触发异步工作（codex serve）

- 当 work item 被拖拽到某些列时，会触发 job 请求（例如移动到 In Progress 即触发 `start`）。
- `codex serve` 直接在同进程内提交 job 到 GitHub runtime 执行队列（不引入第二个监听进程、也不引入外部 IPC）。
- GitHub runtime 准备 worktree、生成 `.codex_github_context.md`、运行 Codex，并写回 job 状态。
- 触发 job 时可以注入该 work item 的 `promptPrefix`（per-item），并允许 per-item 选择模型（覆盖默认）。

### R3.1: 模型选择（per-item）

- 每个 work item 在详情视图可选择模型（例如 `gpt-5.2` / `gpt-4.1` / `o3` 等），作为该 work item 的默认执行模型。
- 模型选择会影响之后该 work item 的 job 触发（至少 In Progress 触发路径）。

### R3.2: 日志可见性

- 每个 job 必须有可查询的执行日志（stdout/stderr 或等价），前端可在详情视图查看/复制。
- 最小实现允许“最近一次 job 日志”；增强实现允许按 jobId 查看历史日志。

### R3.3: 关闭任务

- 前端提供显式动作“Close on GitHub”，由后端调用 GitHub API 将对应 Issue/PR 关闭。
- 关闭成功后触发一次同步并更新看板；closed item 默认不可拖拽触发 job。

### R4: GitHub webhook 路由（codex serve）

- 当 `config.toml` 中存在 `[github_webhook]` 且 `enabled=true` 时，`codex serve` 启用 webhook 接收路由：
  - `POST /github/webhook`（GitHub 配置的 payload URL）
- 该路由不使用 `codex serve` 的 UI token 鉴权；仅通过 GitHub HMAC（`X-Hub-Signature-256`）+ allowlist + permission checks 保护。
- `github_webhook.listen` 字段在融合模式下被忽略（保持兼容；实际监听地址由 `codex serve --host/--port` 决定）。

---

## Open Questions (Must Resolve Before Build)

1. **触发规则**：哪些列触发什么动作？（例如 Backlog->In Progress 触发 `start`，In Progress->Review 触发 `review`）
2. **执行可见性**：异步工作结果是否需要在 `codex serve` 里以 session/chat 形式可视化，还是只显示 job 状态与摘要？
3. **GitHub 写回**：job 完成后是否自动 comment/review？默认关闭还是开启？

---

## Acceptance Criteria (MVP)

- [ ] 在 `config.toml` 启用 `[github_webhook]` 后启动 `codex serve`，Kanban 可以看到 Issue/PR 列表。
- [ ] Kanban 支持多 repo 同步，并可在 UI 中按 repo 过滤。
- [ ] 拖拽移动卡片会更新位置并持久化，刷新后保留。
- [ ] 拖拽到 In Progress 会产生一个 job（至少可见 queued/running/succeeded/failed 四态）。
- [ ] 每个 work item 可独立设置 prompt 前缀与模型，触发 job 时生效（不是全局）。
- [ ] Kanban 内可预览 work item 的正文（至少 body），无需跳转 GitHub。
- [ ] UI 可查看 job 的执行日志（至少最近一次 job）。
- [ ] UI 可显式关闭 GitHub Issue/PR，并默认过滤 closed items（closed 不可触发 job）。
- [ ] job 状态通过 SSE 实时同步到多标签页。
