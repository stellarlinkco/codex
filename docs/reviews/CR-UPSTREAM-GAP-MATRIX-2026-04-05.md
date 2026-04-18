# CR-UPSTREAM-GAP-MATRIX-2026-04-05

## Summary

- 审计对象：
  - 增强版基线：`origin/main` (`6bb1e4fdc`)
  - 原版基线：`openai/main` (`06e06ab173`)
- 观测结果：
  - `git rev-list --left-right --count origin/main...openai/main` => `166 390`
  - 结论：当前不是“小补丁同步”阶段，而是长期双向分叉阶段。
- 本文目标不是逐文件列差异，而是建立可执行的模块级差距矩阵，供后续选择性吸纳使用。

## Control Contract

- Primary Setpoint：在不回退 `origin/main` 既有增强能力的前提下，识别出相对 `openai/main` 的模块级差异，并给出 `Adopt / Adapt / Defer / Reject` 裁决。
- Acceptance：
  - 差异按功能模块而非文件散点归类。
  - 每类差异都标出主落点（控制面 / 数据面 / 状态面）。
  - 每类差异都给出默认处置和验证门禁。
- Guardrails：
  - 不允许把 `origin/main` 已有增强能力当成“待同步功能”误删或回退。
  - 不把“测试全绿”误写成“主线吸纳已完成”。
- Boundary：
  - 本轮只产出审计，不修改主线业务实现。
  - 在研分支的吸纳执行见 `CR-INFLIGHT-BRANCH-MATRIX-2026-04-05.md`。

## State Estimate

### 增强版已存在、必须冻结的本地能力

这些能力已经在 `origin/main` 落地，后续吸纳上游时只能兼容，不能回退：

| 模块 | 主落点 | 当前状态 | 证据 | 默认裁决 |
| --- | --- | --- | --- | --- |
| `hodexctl` 安装与版本管理链路 | 控制面 | 已在增强版主线稳定存在 | `docs/install.md`、`.github/workflows/ci.yml` 中独立 smoke 流程 | Freeze local |
| GitHub webhook + Kanban + serve Web UI 扩展 | 控制面 / 数据面 | 已在增强版主线存在且有独立文档 | `codex-rs/docs/github-webhook.md`、`docs/kanban-board-prd-github.md` | Freeze local |
| 针对 fork 的发布 / 打包 / musl 兼容链路 | 控制面 | 已在增强版主线深度定制 | `git log openai/main..origin/main` 中连续的 musl / release / installer 提交 | Freeze local |
| 当前 agent teams 行为与协作定制 | 控制面 / 状态面 | 本地已经历多轮演化 | `Allow multiple agent teams per session`、`core: simplify agent teams workflow` | Freeze local, only adapt selective upstream fixes |

### 当前需要审计的原版差距类型

| 差异模块 | 主落点 | `origin/main` 现状 | `openai/main` / 候选增量 | 复杂性转移 | 默认裁决 | 门禁 |
| --- | --- | --- | --- | --- | --- | --- |
| app-server v2 account / login flow | 数据面 / 状态面 | 仅有增强版当前能力，不含完整 upstream login/account 增量 | 原版和候选同步分支包含 `LoginAccount*`、device-code / ChatGPT 登录链路 | 把认证复杂性从 CLI/局部登录转移到 app-server 协议与状态面 | Adopt in batch B | `codex-app-server-protocol`、`codex-app-server` 相关测试 + schema 输出 |
| provider auth / model provider 扩展 | 数据面 / 状态面 | 未见完整 provider command auth 包 | 候选同步分支包含 `provider_auth`、`model_provider_info`、core auth 补强 | 把 provider 认证复杂性从调用侧转移到 core auth/state 统一层 | Adopt in batch B |
| agent jobs finalization / polling 收口 | 控制面 / 状态面 | 现主线无该 upstream 修复 | 候选同步分支含 `Fix agent jobs finalization race and reduce status polling churn` | 把线程完成判定从时间假设转移到显式状态订阅与原子收口 | Adopt in batch A | `codex-state` + `suite::agent_jobs` |
| shell / git safety hardening | 控制面 | 当前已有本地安全策略，但未吸纳该 upstream git global option 修复 | 候选同步分支含 `[codex] Block unsafe git global options from safe allowlist` | 把命令安全边界从规则白名单扩展到 git pre-subcommand 全局参数 | Adopt in batch A | shell-command 相关 targeted tests |
| TUI early-exit / terminal restore | 控制面 / 数据面 | 现主线未见等价修复 | 候选同步分支含 `tui: always restore terminal on early exit` | 把 early-exit 风险从用户环境残留转移到 TUI 清理路径 | Adopt in batch A |
| plugin mention / model availability / 小型 UX 修补 | 数据面 | 本地已有 UI 扩展，但未必与 upstream 同步 | 候选同步分支带少量 TUI/chat composer 补丁 | 复杂性小，但容易和本地 UI 方向冲突 | Adapt only if dependency of batch A/B |
| multi-agent / agent-teams 主体实现 | 控制面 / 状态面 | 本地已强定制，且当前存在独立在研分支 | 原版与本地双向分叉，且 `origin/main` 还叠加 agent-org 研究线 | 高耦合；一旦吸纳会同时触碰工具面、状态面、测试面 | Defer; selective fix only |
| webhook / kanban / workspace | 控制面 / 数据面 / 状态面 | 本地增强版独有方向 | 原版没有等价能力，或不是同一产品方向 | 这是本地主导能力，不是上游缺口 | Reject as upstream sync target |
| `hodexctl` / 安装器链路 | 控制面 | 本地增强版独有方向 | 原版无等价功能 | 这是本地主导能力，不是上游缺口 | Reject as upstream sync target |

## First-Principles Notes

### 不变量

- `origin/main` 的 fork 差异不是噪音，而是产品方向：
  - `hodexctl`
  - GitHub webhook / Kanban / serve Web UI
  - 对发布链路与 CI 的 fork 定制
- 这些能力在后续同步中属于“硬约束”，不是“可以被更干净的 upstream 覆盖掉”的实现细节。

### 当前主要误差

- 误差不是“主线缺一个 commit”，而是：
  - 增强版主线缺少一批适合吸纳的 upstream 修复和能力包。
  - 这些能力包尚未被拆成低耦合、可合并的控制输入。
  - 旧同步尝试 `PR #78` 已关闭，因此当前系统没有有效执行器把候选集并入主线。

### 复杂性转移账本

| 差异包 | 复杂性原位置 | 新位置 | 收益 | 新成本 | 失效模式 |
| --- | --- | --- | --- | --- | --- |
| provider auth / login account | 分散在登录入口和调用链 | app-server 协议 + core auth + 状态面 | 统一认证入口、便于外部客户端接入 | 协议与状态复杂度上升 | wire shape 漂移、取消登录 / 多 provider 分支不一致 |
| agent jobs finalization | worker 结束时序与等待循环 | state runtime + status subscription | 去掉 grace-sleep 假设、收敛更稳定 | 状态机和测试复杂度上升 | 结果写入与完成态仍不原子 |
| git safe allowlist | shell 安全规则的局部分支 | 命令安全统一判定层 | 降低 unsafe git globals 穿透审批风险 | 规则维护更严格 | 误杀合法命令或留下未覆盖 wrapper |

## Recommended Absorption Order

1. 批次 A：稳定性与安全
   - `agent_jobs` 原子完成态 / 轮询收口
   - git safe allowlist 修补
   - TUI early-exit terminal restore
2. 批次 B：认证 / 登录能力
   - app-server `LoginAccount*`
   - provider auth
   - 必要的 protocol / config schema 更新
3. 批次 C：依赖性测试与轻量 UX 微调
   - 只吸纳前两批所需的 test stabilization / plugin mention / model availability 变更
4. `multi-agent` 大改与 agent-org 方向
   - 单独审计，不和 A/B/C 混批

## Evidence

### Commands

```bash
git rev-list --left-right --count origin/main...openai/main
git diff --dirstat=files,0 origin/main...openai/main
git log --oneline --decorate --reverse openai/main..origin/main
rg -n "hodexctl|github webhook|kanban|device-code|provider auth|LoginAccount" docs codex-rs .github -S
```

### Key Outputs

- `origin/main...openai/main` => `166 390`
- `origin/main` 侧有明确的本地增强线：
  - `hodexctl`
  - GitHub webhook / Kanban
  - installer / release / musl / Windows wrapper 定制
- `openai` / 候选同步包侧有明确的可吸纳增量：
  - `LoginAccount*`
  - provider auth
  - `agent_jobs` 完成态收口
  - git safe command hardening

## Gate Boundary

- 本文不授权直接 merge 任一在研分支。
- 后续执行必须从 `origin/main` 新建干净 worktree。
- 只有完成分批裁决并获得对应测试 / CI 证据后，才允许进入主线吸纳。
