# Codex 多 Agent / 多 Model 基座架构方案书

日期：2026-03-06

假设：第一版 swarm 不推翻现有 `thread` 运行模型，而是在其上叠一层轻量控制平面。

## 1. 先说结论
这套仓库已经不是“从零开始做 swarm”。真正存在的底座有三块：

- 运行单元已经存在：`thread` / `turn` / `item` 是稳定主语，`app-server` 也已经把它们作为对外 API 原语暴露出来。
- 协作事件已经存在：`protocol` 里已经有 `CollabAgentSpawn*`、`CollabAgentInteraction*`、`CollabWaiting*`、`CollabClose*`、`CollabResume*`。
- 子 agent 身份已经存在：`SessionSource::SubAgent(ThreadSpawn { parent_thread_id, depth, agent_nickname, agent_role })` 已经能表达父子关系和角色。

所以，正确方向不是再造一套“智能 swarm 框架”。正确方向是：

1. 保留 `thread-per-agent`。
2. 在 `protocol + app-server + state` 之上补一个轻量控制平面。
3. 把多 agent 调度、预算、审计、回放、策略继承做成一等对象。
4. 把 `spawn_agent` / `spawn_team` 这类 collab tool，从“工具层私有机制”抬升成“协议层稳定能力”。

一句话版本：**不是重写执行器，而是把现有多 agent 能力产品化、协议化、可恢复化。**

## 2. 现状判断

### 2.1 仓库已经有 swarm 雏形

- 根 `README.md` 已经把目标写得很直白：`agent teams`、`multi-model support`、`long-running orchestration`。见 `README.md:2`、`README.md:3`。
- `app-server` 已经把 `Thread`、`Turn`、`Item` 定义成顶层 API 原语。见 `codex-rs/app-server/README.md:51`。
- `turn/plan/updated`、`collabToolCall` 已经作为流式事件暴露给客户端。见 `codex-rs/app-server/README.md:659`、`codex-rs/app-server/README.md:675`。
- `protocol` 已经显式声明多种 collab 生命周期事件。见 `codex-rs/protocol/src/protocol.rs:1176`。
- `protocol` 已经把 sub-agent 来源建模进 `SessionSource`。见 `codex-rs/protocol/src/protocol.rs:1998`。

这说明仓库缺的不是“多 agent 能不能跑”，而是“多 agent 能不能被稳定管理”。

### 2.2 当前架构的真实边界

当前可以概括成 5 层：

1. **交互层**：`cli`、`tui`、`serve`、`web`
2. **协议层**：`protocol`、`app-server-protocol`
3. **运行层**：`core`、`agent/control`、tool handlers
4. **状态层**：`state`、rollout、thread metadata、runtime state
5. **执行/安全层**：`exec`、`execpolicy`、sandbox、process hardening、MCP

这是对的。问题不在分层，而在“跨层对象”还没有被系统化：现在多 agent 主要以 tool call 和临时事件存在，还没成为一等资源。

## 3. 已验证的硬限制

### 3.1 Team 机制还只是进程内协作，不是通用控制面

- 当前 `agent teams` 文档明确写着：`no nested teams`。见 `docs/agent-teams.md:42`。
- team 持久化依赖 `$CODEX_HOME/teams/<team_id>` 下的 `config.json`、`inbox/*.jsonl`、`tasks.lock` 等文件。见 `docs/agent-teams.md:48` 至 `docs/agent-teams.md:53`。

这套机制对 3 到 5 个 agent 很实用。对 50 个 agent，就开始露怯了：

- 文件锁竞争会变多。
- inbox JSONL 会越滚越大。
- 跨 session / 跨连接调度会很别扭。
- 嵌套编排没有表达空间。

### 3.2 事件面能表达协作，但还不够表达 swarm

现在的协议已经有：

- `CollabAgentSpawnBegin/End`
- `CollabAgentInteractionBegin/End`
- `CollabWaitingBegin/End`
- `CollabCloseBegin/End`
- `CollabResumeBegin/End`

见 `codex-rs/protocol/src/protocol.rs:1176` 至 `codex-rs/protocol/src/protocol.rs:1195`。

问题是它们主要还是“工具调用事件”，不是“swarm 资源事件”。它们缺少几个关键维度：

- 稳定的 `swarm_run_id`
- 稳定的 `agent_id` 与 `parent_agent_id`
- 任务级 `task_id`
- 因果链 `causal_parent`
- 统一 `sequence`
- 预算、优先级、租约状态

没有这些字段，50 个 agent 的 fan-in 聚合会越来越像“日志猜谜”。

### 3.3 线程状态管理还是单线程监听心智

`ThreadState` 当前维护一个 listener，并通过 `listener_command_tx` 串行化监听侧动作。见 `codex-rs/app-server/src/thread_state.rs:52` 至 `codex-rs/app-server/src/thread_state.rs:60`，`codex-rs/app-server/src/thread_state.rs:72` 至 `codex-rs/app-server/src/thread_state.rs:84`。

这说明现有设计偏向“一个线程被一个前端连接稳定消费”。它不是错，只是不适合直接承接 swarm 控制台场景里的：

- 多观察者订阅
- 重连回放
- 按 agent 分组筛选
- 控制面与 UI 面分离

### 3.4 运行时背压不够硬

`core` 里事件通道存在 `async_channel::unbounded()`。见 `codex-rs/core/src/codex.rs:374`。

这对单 agent 或少量子 agent 没问题。对 50 个高产出 agent，有两个现实问题：

- 慢消费者会把内存顶起来。
- UI 或 app-server 如果扇出订阅，事件风暴会先出现，而不是 CPU 先满。

### 3.5 并发护栏存在，但它们是保护网，不是调度器

- agent 侧 spawn slot 受 `agent_max_threads` 控制。见 `codex-rs/core/src/agent/control.rs:86`、`codex-rs/core/src/agent/control.rs:215`、`codex-rs/core/src/agent/control.rs:355`。
- CSV 批量 agent job 的最大并发常量是 `64`。见 `codex-rs/core/src/tools/handlers/agent_jobs.rs:37`。
- team 并发默认受 `[agents].max_threads` 控制。见 `docs/agent-teams.md:41`。

这些都是必要护栏，但它们不等于：

- 调度策略
- 优先级
- 预算分配
- 取消传播
- 故障恢复
- 资源隔离

## 4. 设计目标

目标不是“最聪明”，而是“最稳”。

### 4.1 必须满足

- 保持现有单 agent CLI / TUI / app-server 语义不破。
- 保持 `thread` 仍然是最小执行单元。
- 支持 50 个 agent，但不要求 50 个 agent 完全对等自治。
- 支持多 model，但不把 model router 做成另一套框架。
- 支持可回放、可恢复、可审计。
- 人始终能中断、接管、降级。

### 4.2 明确不做

- 不做 peer-to-peer agent mesh。
- 不做“任何 agent 都能自由谈判”的黑板总线。
- 不做通用 DAG 编排语言。
- 不做第三套事件存储。
- 不做新的 provider 插件大框架。

这些东西不是永远不做，而是现在做了大概率先把复杂度做爆。

## 5. 推荐目标架构

## 5.1 总体图

```text
CLI / TUI / Web / SDK
        |
        v
App Server v2 / Protocol
        |
        v
Swarm Control Plane
  - SwarmRun
  - AgentSpec
  - TaskSpec
  - Budget
  - Lease
  - Artifact
        |
        v
Thread Runtime (existing)
  - CodexThread
  - AgentControl
  - Tool handlers
        |
        +--> Model Adapters
        +--> Exec / MCP / Hooks / File Search
        +--> State / Rollout / Metadata / Replay
```

核心原则：**控制平面新增，对数据平面少改，对执行平面尽量不动。**

## 5.2 控制平面对象

建议新增 6 个一等对象。

### A. `SwarmRun`

表示一次多 agent 编排运行。

最小字段：

- `id`
- `kind`：`team` / `swarm`
- `root_thread_id`
- `status`：`pending | running | blocked | completed | failed | cancelled`
- `created_at` / `updated_at`
- `budget_id`
- `policy_scope_id`
- `summary_artifact_id`

### B. `AgentSpec`

表示某个 agent 的期望配置，不等于线程运行时快照。

最小字段：

- `agent_id`
- `swarm_run_id`
- `parent_agent_id`
- `thread_id`
- `role`
- `nickname`
- `model_provider`
- `model`
- `cwd`
- `sandbox_policy`
- `approval_policy`
- `priority`
- `status_reason`

### C. `TaskSpec`

表示要交付的工作项。

最小字段：

- `task_id`
- `swarm_run_id`
- `assignee_agent_id`
- `depends_on`
- `input_artifact_ids`
- `output_artifact_ids`
- `deadline`
- `lease_until`

### D. `Budget`

控制 token、并发和工具成本。

最小字段：

- `max_agents`
- `max_parallel_tools`
- `max_prompt_tokens`
- `max_completion_tokens`
- `max_cost_usd`
- `degrade_policy`

### E. `Lease`

控制长任务接管与 orphan 回收。

最小字段：

- `owner`
- `ownership_token`
- `lease_until`
- `heartbeat_at`

### F. `Artifact`

表示 agent 之间显式发布的产物，而不是默认共享上下文。

最小字段：

- `artifact_id`
- `kind`：`plan | patch | summary | review | trace | table | ranking`
- `producer_agent_id`
- `consumers`
- `content_ref`
- `digest`

## 5.3 数据平面

数据平面继续复用现有 `item/started` → delta → `item/completed` 结构。见 `codex-rs/app-server/README.md:654` 至 `codex-rs/app-server/README.md:685`。

但所有 item / collab / turn 事件都应补齐统一 envelope：

- `swarmRunId`
- `agentId`
- `parentAgentId`
- `taskId`
- `sequence`
- `causalParent`
- `modelProvider`
- `model`
- `budgetClass`

注意，这不是重写 `ThreadItem`。这是给事件加统一抬头，方便：

- 聚合
- 回放
- fan-in 排序
- 因果追踪
- 审计

## 5.4 调度器

调度器只做 6 件事：

1. 创建 / 关闭 agent
2. 分配 task
3. 管理 budget
4. 管理 lease / heartbeat
5. 传播取消与超时
6. 汇总 artifact

**不要**让调度器直接理解每个工具的业务细节。

调度器不应该：

- 直接操作 git diff 细节
- 直接决定 patch merge 策略
- 直接改 prompt 模板

那会把调度器做成“上帝对象”。

## 5.5 记忆模型

推荐四层记忆，而不是共享一个大上下文池。

### 层 1：只读共享上下文

仓库概况、系统指令、项目约束。所有 agent 可读，不可写。

### 层 2：线程私有工作记忆

每个 `thread` 自己的 turn / item / intermediate state。默认不共享。

### 层 3：显式发布 artifact

只有被 publish 的总结、评审、排行榜、patch 才能被别的 agent 消费。

### 层 4：压缩后的长期记忆

用于恢复、resume、review、历史对账。优先继续复用现有 rollout / extended history，而不是重做数据库。

原则就一句：**默认隔离，按 artifact 共享。**

## 5.6 多 Model 架构

建议把多 model 做成“适配层 + 路由策略”，不要做成“自治选择器”。

### 适配层职责

- 统一 provider 能力描述
- 暴露 `default_model`
- 暴露 `ensure_ready` / warmup
- 暴露 token / timeout / capability 信息

### 路由策略维度

- 角色：planner、reviewer、coder、searcher
- 风险：高风险任务优先高质量模型
- 成本：低价值 fan-out 用便宜模型
- 延迟：交互链路优先低延迟
- 工具需求：某些 provider 对工具链更稳

但策略必须简单、可解释、可覆盖。别搞一个黑箱 router。

## 5.7 策略治理

推荐四层继承：

1. 组织级
2. 项目级
3. swarm run 级
4. agent 级

冲突规则只用一条：**子级只能收紧，不能放宽。**

这样能保持现有 execpolicy / sandbox / approval 体系不被穿透。

## 5.8 观测与审计

Swarm 真正怕的不是“失败”，而是“说不清楚”。

必须补的最小观测字段：

- `trace_id`
- `span_id`
- `swarm_run_id`
- `agent_id`
- `task_id`
- `request_id`
- `sequence`

必须补的最小指标：

- 活跃 agent 数
- 被 budget 拒绝的调度次数
- 平均等待时延
- 慢消费者积压
- tool 并发拒绝数
- orphan agent 回收数

必须补的最小审计证据：

- 谁创建了谁
- 谁给谁发了什么任务
- 谁调用了什么工具
- 谁产出了哪个 artifact
- 谁批准了危险操作

## 6. 为什么我不建议“agent swarm from scratch”

因为仓库已经有 70% 了。

现有事实非常明确：

- `app-server` 已经有 thread API 和 turn streaming。
- `protocol` 已经有 collab 事件。
- `SessionSource` 已经能表达 sub-agent 来源。
- `AgentControl` 已经有线程配额护栏。
- hooks 甚至已经有 `SubagentStart`、`TeammateIdle`、`TaskCompleted` 这些多 agent 生命周期触发点。见 `docs/hooks.md:188` 至 `docs/hooks.md:190`。

所以现在再做一套 actor runtime、peer mesh、共享记忆总线，只会出现两个系统：

- 旧系统继续承载真实交互
- 新系统负责“未来蓝图”

最后两个都难维护。这个坑太经典了。

## 7. 推荐 API 演进

新 API 不要塞到旧表面里拼拼补补。建议全部进 app-server v2。

建议新增最小资源：

- `swarm/start`
- `swarm/read`
- `swarm/list`
- `swarm/cancel`
- `swarm/agent/list`
- `swarm/task/list`
- `swarm/task/retry`
- `swarm/artifact/read`
- `swarm/events/subscribe`

而现有：

- `spawn_agent`
- `spawn_team`
- `team_task_*`
- `team_inbox_*`

继续保留，但把它们降级为：

- CLI/TUI 的高级 sugar
- 调用新控制面的兼容入口

这才叫兼容迁移。

## 8. UI 建议

### 8.1 TUI

TUI 不该变成 swarm 大屏。

TUI 第一阶段只做：

- agent 摘要列表
- 阻塞状态排序
- 最近事件摘要
- 单 agent 深钻 transcript
- 手动接管 / 取消 / 重试

### 8.2 Web

50 agent 的控制台更适合 Web 首发。

原因很简单：

- 需要分组视图
- 需要过滤与排序
- 需要时间线
- 需要 artifact 面板
- 需要长时驻留和重连恢复

TUI 适合操作。Web 适合编排和观察。两者别互相冒充。

## 9. 三阶段落地路线

## 阶段一：把现有能力固化成稳定控制面

目标：不改执行器，只补对象和元数据。

- 给事件补 swarm envelope
- 给 thread / sub-agent 补稳定 agent 元数据
- 抽象 state backend，隔离 team JSONL 持久化细节
- app-server v2 增加 `swarm/read` / `swarm/list` / `swarm/events`
- TUI 增加 agent summary 视图
- Web 增加 swarm run 列表与详情页

交付标准：

- 10 到 20 agent 稳定可观测
- 可断线重连
- 可重放
- 不破现有 CLI / TUI

## 阶段二：补调度、预算、租约

目标：让多 agent 真正可管理。

- 引入 `TaskSpec` / `Budget` / `Lease`
- 支持取消传播、超时回收、orphan 接管
- 支持按角色路由 model
- 支持 artifact publish / subscribe

交付标准：

- 20 到 50 agent 可持续运行
- 慢消费者不会无限堆积
- 失败任务可局部重试

## 阶段三：跨 session / 跨机器 swarm

目标：让编排脱离单一前端连接。

- 控制面脱离进程内 team 限制
- 统一 Web / SDK / CLI 对 swarm 的入口
- 支持长任务和多机器观察面

交付标准：

- swarm run 可跨连接继续观察和控制
- 本地 / 服务端 / 混合部署模型清晰

## 10. 关键风险

### 风险 1：把控制面做成第二个运行时

这是最大风险。控制面应编排现有 `thread runtime`，而不是复制它。

### 风险 2：事件字段越补越散

如果不定义统一 envelope，后面会在 item、turn、collab、artifact 上各补一遍，最后全乱。

### 风险 3：过早远程化

现在先抽 state backend 接口就够了。别急着上中心化服务。

### 风险 4：共享上下文失控

不做 artifact publish 机制的话，最后只会退化成所有 agent 抄同一大 prompt，既贵又乱。

## 11. 最小决策集

如果今天只能拍 6 个板，我建议拍这 6 个：

1. 保留 `thread-per-agent`。
2. 新能力统一走 app-server v2。
3. 补 swarm envelope，不重写 item 模型。
4. 引入 `SwarmRun` / `AgentSpec` / `TaskSpec` / `Budget` / `Lease` / `Artifact`。
5. 默认隔离记忆，只按 artifact 共享。
6. 第一阶段不做消息总线、不做 peer mesh、不做 DAG 引擎。

## 12. 仓库证据索引

- 目标已经面向 multi-agent / multi-model：`README.md:2`、`README.md:3`
- `Thread / Turn / Item` 是现有系统主语：`codex-rs/app-server/README.md:51`
- plan 与 collab 已进入流式事件面：`codex-rs/app-server/README.md:659`、`codex-rs/app-server/README.md:675`
- collab 生命周期事件已存在：`codex-rs/protocol/src/protocol.rs:1176`
- sub-agent 父子关系已存在：`codex-rs/protocol/src/protocol.rs:2015`
- team 当前受单 session / 无嵌套限制：`docs/agent-teams.md:42`
- team 持久化依赖本地 JSON / JSONL / lock：`docs/agent-teams.md:48`
- app-server 的 thread listener 仍是单 listener 心智：`codex-rs/app-server/src/thread_state.rs:52`
- 运行时存在无界事件通道：`codex-rs/core/src/codex.rs:374`
- 当前并发护栏已经存在但还不是调度器：`codex-rs/core/src/agent/control.rs:86`、`codex-rs/core/src/tools/handlers/agent_jobs.rs:37`
- hooks 已能感知多 agent 生命周期：`docs/hooks.md:45`、`docs/hooks.md:188`

## 13. 最后一句

Codex 现在最该做的，不是“更像 swarm”。而是让已经存在的 swarm 雏形，先变得可控、可观测、可恢复。

这事做对了，50 个 agent 不是问题。

做反了，5 个 agent 都会开始吵。
