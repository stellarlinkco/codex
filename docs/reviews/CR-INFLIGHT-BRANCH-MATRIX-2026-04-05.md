# CR-INFLIGHT-BRANCH-MATRIX-2026-04-05

## Summary

- 审计对象：
  - 基线：`origin/main` (`6bb1e4fdc`)
  - 在研分支 1：`sync/openai-rust-v0.117.0-stability-plugins-auth` (`bfd8fe43e`)
  - 在研分支 2：`fork/feat/agent-teams-org-mesh` (`227ee9a77`)
- 审计目标：
  - 判断哪些已经开发中的功能包适合继续吸纳。
  - 判断哪些分支必须拆包、延后或重做。

## Control Contract

- Primary Setpoint：把“已开发但未并主线”的候选能力拆成可执行批次，并给出明确裁决。
- Acceptance：
  - 每条在研分支都要给出状态估计、主落点、风险级别、默认处置。
  - 至少明确下一轮可执行批次及其测试门禁。
- Guardrails：
  - 不复活已关闭的 `PR #78`。
  - 不把高耦合 agent-org 分支混进稳定性 / 认证批次。

## State Estimate

### Branch 1: `sync/openai-rust-v0.117.0-stability-plugins-auth`

- 与主线关系：
  - `git rev-list --left-right --count origin/main...sync/openai-rust-v0.117.0-stability-plugins-auth` => `0 11`
  - 说明：这是相对 `origin/main` 纯前进的候选集，不含额外主线落后债。
- 外部状态：
  - `PR #78` 指向该分支头 `bfd8fe43e`
  - 当前状态：`CLOSED`
  - 结论：候选集成物存在，但旧执行闭环已失效。

#### Candidate Packages

| 候选包 | 代表提交 | 主落点 | 作用面 | 风险 | 默认裁决 | 说明 |
| --- | --- | --- | --- | --- | --- | --- |
| A1 `agent_jobs` 收口 | `475bb86ec` | `core/src/tools/handlers/agent_jobs.rs`、`state/src/runtime/agent_jobs.rs` | 控制面 / 状态面 | 中 | Adopt | 价值高、边界清晰、验证便宜 |
| A2 git safety hardening | `d1ccf911f` | `shell-command/src/command_safety/*` | 控制面 | 低到中 | Adopt | 明确安全收益，不依赖大面积协议变更 |
| A3 TUI early-exit restore | `fd8bd6eb7` | TUI terminal 清理路径 | 控制面 / 数据面 | 低 | Adopt | 局部修复，适合与 A1/A2 同批 |
| B1 app-server device-code login | `f2ddff912` | app-server protocol / processor / tests | 数据面 / 状态面 | 中到高 | Adopt | 需要 schema 与协议门禁 |
| B2 provider command auth | `84c368d94` | core auth / provider auth / config types | 数据面 / 状态面 | 中到高 | Adopt | 必须和 B1 一起看，不宜拆得更碎 |
| B3 review 修补 | `e12a5be07`、`e7394b8bc` | B1/B2 附属文件 | 数据面 / 状态面 | 中 | Adopt with B | 不是独立功能，应并入 B 批次 |
| C1 core integration stabilization | `1ec00ef63` | core tests / shell snapshot / cli stream | 控制面 | 中 | Adapt | 只吸纳 A/B 真正依赖的测试修补 |
| C2 auth/network/UI stabilization | `5b6db1f3e`、`bf4679fa4` | app_list / network-proxy / TUI tests | 控制面 / 数据面 | 中 | Adapt | 不做独立批次，按 A/B 依赖精简吸纳 |
| C3 clippy tail fix | `bfd8fe43e` | `core/src/config/mod.rs`、`tools/sandboxing.rs` | 控制面 | 低 | Adopt only if rebasing old patch | 本质是旧 PR 尾部修正，不单独成批 |

#### Execution Decision

- 该分支不直接 merge。
- 必须拆成 3 个新批次：
  1. `batch/upstream-stability-safety`
  2. `batch/upstream-auth-login`
  3. `batch/upstream-dependent-test-polish`
- 每个批次都从最新 `origin/main` 新建 worktree，而不是从旧 sync 分支直接继续推。

### Branch 2: `fork/feat/agent-teams-org-mesh`

- 与主线关系：
  - `git rev-list --left-right --count origin/main...fork/feat/agent-teams-org-mesh` => `26 49`
  - 说明：该分支不仅前进很多，而且已经明显脱离当前主线，存在 rebase 债。
- 文件范围：
  - 不只是 `multi_agents`
  - 还覆盖 `serve`、`web`、`cli`、`docs`、`Cargo.lock`、`CI`、`kanban workspace`
- 结论：
  - 这不是“可直接吸纳的功能包”
  - 这是一个高耦合、跨控制面 / 数据面 / 状态面的研究分支

#### High-Coupling Areas

| 区域 | 主落点 | 风险 | 观察结论 | 默认裁决 |
| --- | --- | --- | --- | --- |
| `multi_agents` agent-org 工具面扩张 | 控制面 / 状态面 | 高 | 工具面大幅扩容，触碰团队治理、artifact、review、status、release、incident | Defer |
| `serve` / `web` workspace-kanban 扩张 | 数据面 / 状态面 | 高 | 已超出单纯 agent-teams 范围，混入看板与 workspace 面 | Defer |
| `cli/github_cmd` / docs / governance | 控制面 | 中到高 | 影响 GitHub 工作流和协作入口 | Defer |
| 文档设计与规划稿 | 控制面 | 低 | 对实现有参考价值，但不是主线代码 | Keep as design input |

#### Execution Decision

- 本轮不合并该分支。
- 必须先完成 3 件事后才允许进入实现阶段：
  1. 从分支中抽取一份“agent-org 与现有 agent-teams 的契约差异说明”
  2. 重新按功能域拆成多个候选包，不能再以整分支为单位评审
  3. 在最新 `origin/main` 上验证哪些能力仍然缺失，哪些已被主线后来提交覆盖

## Recommended Batch Order

### Batch A: 稳定性与安全

- 来源：
  - `475bb86ec`
  - `d1ccf911f`
  - `fd8bd6eb7`
- 验证门禁：
  - `cargo test -p codex-state`
  - `cargo test -p codex-core --test all suite::agent_jobs`
  - shell-command / TUI 对应 targeted tests

### Batch B: 认证与登录

- 来源：
  - `f2ddff912`
  - `84c368d94`
  - `e12a5be07`
  - `e7394b8bc`
- 验证门禁：
  - `cargo test -p codex-app-server-protocol`
  - `cargo test -p codex-app-server --test all suite::v2::account`
  - schema 生成与 config schema 校验

### Batch C: 依赖性测试补强

- 来源：
  - `1ec00ef63`
  - `5b6db1f3e`
  - `bf4679fa4`
  - `bfd8fe43e`
- 验证门禁：
  - 仅跟随 A/B 的真实依赖项吸纳
  - 不允许为了合并旧分支而把整批测试漂移打进主线

## Evidence

### Commands

```bash
git rev-list --left-right --count origin/main...sync/openai-rust-v0.117.0-stability-plugins-auth
git log --oneline --decorate origin/main..sync/openai-rust-v0.117.0-stability-plugins-auth
gh pr view 78 --repo stellarlinkco/codex --json state,closedAt,headRefOid,title
git rev-list --left-right --count origin/main...fork/feat/agent-teams-org-mesh
git diff --name-only origin/main..fork/feat/agent-teams-org-mesh
```

### Key Outputs

- sync 分支：`0 11`
- org-mesh 分支：`26 49`
- `PR #78`：`CLOSED`，head `bfd8fe43e`
- org-mesh 分支触碰：
  - `multi_agents`
  - `serve`
  - `web`
  - `cli`
  - `docs`
  - `Cargo.lock`
  - CI 配置

## Gate Boundary

- 下一轮执行只能从新 worktree 开始，禁止在当前脏工作树或旧 sync 分支上继续叠加。
- `feat/agent-teams-org-mesh` 不进入 A/B/C 任一批次。
- 只有当 A/B 批次分别拿到本地 targeted tests + CI 绿灯后，才允许继续推进后续吸纳。
