# Agent Teams & SubAgent 迁移实施文档

> **目标**：将 Codex 现有 18 个多 Agent 工具精简为 7 个核心工具，引入 Claude Code 的 Agent 角色系统、Coordinator 模式和 Agent Memory。
>
> **参考源码**：`reference/claude-code-sourcemap/restored-src/src/`
>
> **执行方式**：按 Task 1-7 顺序执行，每个 Task 完成后必须通过验证规则才能进入下一个。

---

## 全局约束

1. **语言**：所有修改在 Rust (`codex-rs/`) 中完成，不涉及 TypeScript 层
2. **编译**：每个 Task 完成后 `cargo build -p codex-core` 必须通过
3. **测试**：每个 Task 完成后 `cargo test -p codex-core` 必须通过（允许删除对应的废弃测试）
4. **事件兼容**：`CollabAgent*` 系列 protocol event 不得破坏（app-server / TUI 依赖这些事件）
5. **不修改 protocol crate**：`codex-rs/protocol/` 和 `codex-rs/app-server-protocol/` 本次不改动
6. **风格**：遵循现有代码风格，`cargo clippy -p codex-core` 无新增 warning

---

## Task 1：删除 Team Task 子系统

**目的**：移除文件级任务管理（claim/complete/depends_on），减少 4 个工具 + 相关基础设施。

### 1.1 要删除的文件

| 文件路径 | 行数 | 说明 |
|---------|------|------|
| `core/src/tools/handlers/multi_agents/team_task_list.rs` | 48 | team_task_list handler |
| `core/src/tools/handlers/multi_agents/team_task_claim.rs` | 93 | team_task_claim handler |
| `core/src/tools/handlers/multi_agents/team_task_claim_next.rs` | 124 | team_task_claim_next handler |
| `core/src/tools/handlers/multi_agents/team_task_complete.rs` | 125 | team_task_complete handler |
| `core/src/tools/handlers/multi_agents/locks.rs` | 100 | 仅被 task 系统使用的文件锁 |

### 1.2 要修改的文件

#### `core/src/tools/handlers/multi_agents.rs`（1864 行）

**删除类型定义**：
- Line 145: `enum PersistedTaskState` — 删除整个 enum
- Line 153: `struct PersistedTeamTask` — 删除整个 struct
- Line 164: `struct PersistedTeamTaskAssignee` — 删除整个 struct
- Line 512: `struct TeamTaskOutput` — 删除整个 struct

**删除函数**：
- Line 223: `fn team_tasks_dir()` — 删除
- Line 227: `async fn lock_team_tasks()` — 删除
- Line 304: `fn build_initial_team_tasks()` — 删除
- Line 420: `async fn read_team_tasks()` — 删除
- Line 491: `async fn write_team_task()` — 删除
- Line 1168: `async fn dispatch_task_completed_hook()` — 删除

**删除 mod 声明**：
- Line 1588: `mod team_task_list;` — 删除
- Line 1590: `mod team_task_claim;` — 删除
- Line 1592: `mod team_task_claim_next;` — 删除
- Line 1594: `mod team_task_complete;` — 删除
- Line 659: `mod locks;` — 删除

**修改 handle() match（Line 629-656）**：
- 删除 `"team_task_list"` arm
- 删除 `"team_task_claim"` arm
- 删除 `"team_task_claim_next"` arm
- 删除 `"team_task_complete"` arm

#### `core/src/tools/spec.rs`（4593 行）

**删除函数**：
- Line 1406: `create_team_task_list_tool()` — 删除整个函数
- Line 1426: `create_team_task_claim_tool()` — 删除整个函数
- Line 1454: `create_team_task_claim_next_tool()` — 删除整个函数
- Line 1486: `create_team_task_complete_tool()` — 删除整个函数

**修改 build_specs()（Line 2690-2728）**：
- 删除这 4 个工具的 `push_spec` 和 `register_handler` 调用

#### `core/src/tools/handlers/multi_agents/spawn_team.rs`（379 行）

- 找到调用 `build_initial_team_tasks()` 的地方，删除该调用及相关的 task 持久化逻辑
- 保留 agent 生成逻辑不动

#### `core/src/tools/handlers/multi_agents/tests.rs`（5678 行）

- 删除所有引用 `team_task_list`, `team_task_claim`, `team_task_claim_next`, `team_task_complete` 的测试用例
- 删除引用 `PersistedTeamTask`, `PersistedTaskState` 的测试 helper

### 1.3 验证规则

```bash
# 编译通过
cargo build -p codex-core

# 测试通过（允许删除废弃测试后）
cargo test -p codex-core

# 确认工具不再注册
cargo test -p codex-core -- --grep "team_task" 2>&1 | grep -c "FAILED" # 应为 0

# Clippy 无新增 warning
cargo clippy -p codex-core -- -D warnings
```

### 1.4 参考信息

- 当前 task 系统数据结构：`multi_agents.rs` Line 145-170
- Claude Code 不使用文件级 task 系统，完全靠消息协调：参见 `reference/claude-code-sourcemap/restored-src/src/coordinator/coordinatorMode.ts`

---

## Task 2：合并消息工具为 `send_message`

**目的**：将 `send_input` + `team_message` + `team_broadcast` + `team_ask_lead` + `team_inbox_pop` + `team_inbox_ack` 合并为 1 个 `send_message` 工具。

### 2.1 新建文件

#### `core/src/tools/handlers/multi_agents/send_message.rs`

**参考现有实现**：
- `send_input.rs`（47 行）— 基础 direct messaging
- `team_message.rs`（81 行）— team 内消息
- `team_broadcast.rs`（114 行）— 广播
- `team_ask_lead.rs`（99 行）— 向上通信

**输入 Schema**：
```rust
#[derive(Deserialize)]
struct SendMessageArgs {
    to: String,              // Agent ID, name, 或 "lead"
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    items: Option<Vec<UserInput>>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    broadcast: bool,
}
```

**路由逻辑**：
```rust
pub async fn handle(...) -> Result<ToolOutput, FunctionCallError> {
    let args: SendMessageArgs = parse_arguments(&arguments)?;
    
    if args.broadcast && args.team_id.is_some() {
        // 路径 A: 广播（复用 team_broadcast 逻辑）
        broadcast_to_team(session, turn, &args).await
    } else if args.to == "lead" && args.team_id.is_some() {
        // 路径 B: 问 lead（复用 team_ask_lead 逻辑）
        ask_lead(session, turn, &args).await
    } else if args.team_id.is_some() {
        // 路径 C: team 内指定成员（复用 team_message 逻辑）
        message_team_member(session, turn, &args).await
    } else {
        // 路径 D: 直接发送（复用 send_input 逻辑）
        direct_send(session, turn, &args).await
    }
}
```

**复用的 helper 函数**（在 `multi_agents.rs` 中）：
- `send_input_to_member()`（Line 536）— 所有路径共用
- `parse_collab_input()`（Line 1733）— 解析 message/items union
- `find_team_member()`  — team 成员查找
- `get_team_record()` — team 注册表查询

### 2.2 要删除的文件

| 文件路径 | 行数 |
|---------|------|
| `core/src/tools/handlers/multi_agents/send_input.rs` | 47 |
| `core/src/tools/handlers/multi_agents/team_message.rs` | 81 |
| `core/src/tools/handlers/multi_agents/team_broadcast.rs` | 114 |
| `core/src/tools/handlers/multi_agents/team_ask_lead.rs` | 99 |
| `core/src/tools/handlers/multi_agents/team_inbox_pop.rs` | 91 |
| `core/src/tools/handlers/multi_agents/team_inbox_ack.rs` | 74 |
| `core/src/tools/handlers/multi_agents/inbox.rs` | 292 |

### 2.3 要修改的文件

#### `core/src/tools/handlers/multi_agents.rs`

**删除 mod 声明**：
- Line 661: `mod inbox;` 
- Line 663: `mod team_ask_lead;`
- Line 665: `mod team_inbox_pop;`
- Line 667: `mod team_inbox_ack;`
- Line 671: `mod send_input;`
- Line 1596: `mod team_message;`
- Line 1598: `mod team_broadcast;`

**新增 mod 声明**：
```rust
mod send_message;
```

**修改 handle() match**：
- 删除 `"send_input"`, `"team_message"`, `"team_broadcast"`, `"team_ask_lead"`, `"team_inbox_pop"`, `"team_inbox_ack"` 共 6 个 arm
- 新增 `"send_message" => send_message::handle(session, turn, call_id, arguments).await`

#### `core/src/tools/spec.rs`

**删除函数**：
- Line 1015: `create_send_input_tool()`
- Line 1514: `create_team_message_tool()`
- Line 1561: `create_team_broadcast_tool()`
- Line 1601: `create_team_ask_lead_tool()`
- Line 1641: `create_team_inbox_pop_tool()`
- Line 1669: `create_team_inbox_ack_tool()`

**新增函数** `create_send_message_tool()`：
```rust
fn create_send_message_tool() -> ToolSpec {
    // JSON Schema:
    // {
    //   "name": "send_message",
    //   "description": "Send a message to an agent by ID/name, or broadcast to all team members.",
    //   "parameters": {
    //     "to": { "type": "string", "description": "Agent ID, agent name, or 'lead'" },
    //     "message": { "type": "string", "description": "Text message content" },
    //     "items": { "type": "array", "description": "Rich input items (alternative to message)" },
    //     "team_id": { "type": "string", "description": "Team scope for team messaging" },
    //     "broadcast": { "type": "boolean", "default": false, "description": "Send to all team members" }
    //   },
    //   "required": ["to"]
    // }
}
```

**修改 build_specs()**：
- 替换 6 个旧注册为 1 个 `send_message` 注册

### 2.4 验证规则

```bash
cargo build -p codex-core
cargo test -p codex-core

# 功能验证：确认 send_message 的 4 条路由路径都有对应逻辑
grep -n "broadcast_to_team\|ask_lead\|message_team_member\|direct_send" \
  codex-rs/core/src/tools/handlers/multi_agents/send_message.rs
# 应输出 4 个函数调用

cargo clippy -p codex-core -- -D warnings
```

### 2.5 参考信息

- Claude Code SendMessageTool 路由逻辑：`reference/claude-code-sourcemap/restored-src/src/tools/SendMessageTool/SendMessageTool.ts`
- 现有 `send_input_to_member()` helper：`multi_agents.rs` Line 536

---

## Task 3：合并 Team 生命周期为 `create_team` + `delete_team`

**目的**：将 `spawn_team` + `close_team` + `team_cleanup` + `wait_team` 合并为 `create_team` + `delete_team`。

### 3.1 新建文件

#### `core/src/tools/handlers/multi_agents/create_team.rs`

**基于** `spawn_team.rs`（379 行）简化：
- 保留：成员生成逻辑（循环 spawn + role + worktree）
- 保留：team registry 注册 + 持久化
- 删除：`build_initial_team_tasks()` 调用（Task 1 已删）
- 新增：创建完成后注入 coordinator prompt 到 lead thread（Task 5 实现，此处先留 TODO）

**输入 Schema**：
```rust
#[derive(Deserialize)]
struct CreateTeamArgs {
    team_id: Option<String>,
    members: Vec<CreateTeamMemberArgs>,
}

#[derive(Deserialize)]
struct CreateTeamMemberArgs {
    name: String,
    task: String,
    #[serde(default)]
    agent_type: Option<String>,  // role name
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    worktree: bool,
}
```

**输出**：
```rust
struct CreateTeamResult {
    team_id: String,
    members: Vec<CreateTeamMemberResult>,
}
struct CreateTeamMemberResult {
    name: String,
    agent_id: String,
    status: AgentStatus,
}
```

#### `core/src/tools/handlers/multi_agents/delete_team.rs`

**合并** `close_team.rs`（235 行）+ `team_cleanup.rs`（90 行）：
- 关闭所有成员（`shutdown_agent`）
- 清理 worktrees（`cleanup_agent_worktree`）
- 删除持久化（`remove_team_persistence`）
- 从 registry 移除（`remove_team_record`）

**输入 Schema**：
```rust
#[derive(Deserialize)]
struct DeleteTeamArgs {
    team_id: String,
    #[serde(default = "default_true")]
    cleanup: bool,  // 是否删除持久化文件，默认 true
}
```

### 3.2 要删除的文件

| 文件路径 | 行数 |
|---------|------|
| `core/src/tools/handlers/multi_agents/spawn_team.rs` | 379 |
| `core/src/tools/handlers/multi_agents/wait_team.rs` | 212 |
| `core/src/tools/handlers/multi_agents/close_team.rs` | 235 |
| `core/src/tools/handlers/multi_agents/team_cleanup.rs` | 90 |

### 3.3 要修改的文件

#### `core/src/tools/handlers/multi_agents.rs`

**删除 mod**：`spawn_team`, `wait_team`, `close_team`, `team_cleanup`

**新增 mod**：`create_team`, `delete_team`

**修改 handle() match**：
- 删除 `"spawn_team"`, `"wait_team"`, `"close_team"`, `"team_cleanup"` arm
- 新增 `"create_team"` 和 `"delete_team"` arm

**注意**：`wait_team` 功能由 `wait` 工具承接——调用者从 `create_team` 返回值获取 member agent_ids，然后传给 `wait` 工具。

#### `core/src/tools/spec.rs`

**删除函数**：
- Line 1245: `create_spawn_team_tool()`
- Line 1333: `create_wait_team_tool()`
- Line 1374: `create_close_team_tool()`
- Line 1700: `create_team_cleanup_tool()`

**新增函数**：`create_team_tool()`, `create_delete_team_tool()`

**修改 build_specs()**：替换 4 个旧注册为 2 个新注册

#### `core/src/tools/handlers/multi_agents/wait.rs`（156 行）

- 确认已支持接收多个 agent_id 列表 + mode（any/all）
- 如果 `wait_team.rs` 有额外逻辑（如自动查找 team members），将该逻辑移入 `wait.rs` 作为可选参数 `team_id`

### 3.4 验证规则

```bash
cargo build -p codex-core
cargo test -p codex-core

# 确认新工具已注册
grep -n "create_team\|delete_team" codex-rs/core/src/tools/spec.rs | head -20
# 应看到新的 create_team_tool 和 delete_team_tool 函数

# 确认旧工具已移除
grep -n "spawn_team\|wait_team\|close_team\|team_cleanup" codex-rs/core/src/tools/spec.rs
# 应无匹配（仅注释中可能残留）

cargo clippy -p codex-core -- -D warnings
```

### 3.5 参考信息

- Claude Code TeamCreateTool：`reference/claude-code-sourcemap/restored-src/src/tools/TeamCreateTool/TeamCreateTool.ts`
- Claude Code TeamDeleteTool：`reference/claude-code-sourcemap/restored-src/src/tools/TeamDeleteTool/`
- 现有 `shutdown_team_members()` helper：`multi_agents.rs`（可复用）
- 现有 `cleanup_spawned_team_members()` helper：`multi_agents.rs`（可复用）

---

## Task 4：扩展 Agent 角色系统

**目的**：新增 `plan`、`verify`、`coordinator` 三个 built-in 角色。

### 4.1 新建文件

#### `core/src/agent/builtins/plan.toml`

```toml
# Plan Agent: 只读规划，不修改文件
[model]
# 继承 parent model

[instructions]
text = """
You are a planning agent. Your task is to design implementation plans.

## Rules
- You are READ-ONLY. Do NOT modify any files.
- You may use shell commands for reading only (grep, find, cat, ls).
- You may NOT use shell commands that modify state (rm, mv, git commit, etc.).

## Output Format
1. **Critical Files**: List files that need modification with line numbers
2. **Implementation Steps**: Numbered steps with specific changes
3. **Trade-offs**: Architectural considerations and alternatives
4. **Risks**: What could go wrong and mitigation strategies
"""
```

#### `core/src/agent/builtins/verify.toml`

```toml
# Verify Agent: 运行测试和检查，输出 VERDICT
[instructions]
text = """
You are a verification agent. Your job is to verify that implementation work is correct.

## Process
1. Run build: `cargo build` or equivalent
2. Run tests: `cargo test` or equivalent
3. Run linter: `cargo clippy` or equivalent
4. Check for regressions: compare before/after behavior

## Output
You MUST end your response with exactly one of:
- VERDICT: PASS — all checks passed
- VERDICT: FAIL — critical issues found (list them)
- VERDICT: PARTIAL — some checks passed, others need attention

Include actual command outputs as evidence.
"""
```

#### `core/src/agent/builtins/coordinator.toml`

```toml
# Coordinator Agent: 编排多 agent 协作
[instructions]
text = """
You orchestrate work across multiple agents. You do NOT implement directly.

## Available Tools
- spawn_agent: Create worker agents with specific roles
- send_message: Communicate with agents
- wait: Wait for agent completion
- close_agent: Shutdown agents
- create_team / delete_team: Team lifecycle

## Workflow
1. Research: Spawn explorer agents to gather context
2. Synthesize: Read findings, understand the problem yourself
3. Plan: Design implementation approach
4. Dispatch: Spawn worker agents with PRECISE specifications
5. Verify: Spawn verify agent to check results

## Critical Rules
- NEVER delegate understanding. Read agent results and synthesize before dispatching.
- Prefer parallel spawning for independent tasks.
- Serialize tasks that modify the same files.
"""
```

### 4.2 要修改的文件

#### `core/src/agent/role.rs`（851 行）

**修改 `built_in::configs()`（Line 195）**：

新增 3 个角色条目：

```rust
// 在 configs() HashMap 中添加：
map.insert("plan", BuiltInRole {
    description: "Read-only planning agent that designs implementation strategies",
    config_file: Some("plan.toml"),
    nickname_candidates: &["Planner", "Architect", "Designer", "Strategist"],
});

map.insert("verify", BuiltInRole {
    description: "Verification agent that runs tests and checks correctness",
    config_file: Some("verify.toml"),
    nickname_candidates: &["Checker", "Validator", "Inspector", "Auditor"],
});

map.insert("coordinator", BuiltInRole {
    description: "Orchestration agent that coordinates multi-agent workflows",
    config_file: Some("coordinator.toml"),
    nickname_candidates: &["Conductor", "Director", "Orchestrator", "Manager"],
});
```

**修改 `config_file_contents()`**：
```rust
fn config_file_contents(filename: &str) -> Option<&'static str> {
    match filename {
        "explorer.toml" => Some(include_str!("builtins/explorer.toml")),
        "plan.toml" => Some(include_str!("builtins/plan.toml")),
        "verify.toml" => Some(include_str!("builtins/verify.toml")),
        "coordinator.toml" => Some(include_str!("builtins/coordinator.toml")),
        _ => None,
    }
}
```

#### `core/src/tools/spec.rs`

**修改 `create_spawn_agent_tool()`（Line 817）**：
- 在 `agent_type` 参数的 description 中加入新角色："plan", "verify", "coordinator"
- 或者通过 `spawn_tool_spec::build()` 自动从 role configs 生成

### 4.3 验证规则

```bash
cargo build -p codex-core
cargo test -p codex-core

# 确认角色配置被正确加载
grep -rn "plan\|verify\|coordinator" codex-rs/core/src/agent/role.rs
# 应看到 3 个新角色定义

# 确认 TOML 文件存在且可被 include_str! 加载
ls codex-rs/core/src/agent/builtins/
# 应包含: explorer.toml, plan.toml, verify.toml, coordinator.toml

cargo clippy -p codex-core -- -D warnings
```

### 4.4 参考信息

- Claude Code explore agent prompt：`reference/claude-code-sourcemap/restored-src/src/tools/AgentTool/built-in/exploreAgent.ts`
- Claude Code plan agent prompt：`reference/claude-code-sourcemap/restored-src/src/tools/AgentTool/built-in/planAgent.ts`
- Claude Code verify agent prompt：`reference/claude-code-sourcemap/restored-src/src/tools/AgentTool/built-in/verificationAgent.ts`
- 现有 `explorer.toml` 格式：`core/src/agent/builtins/explorer.toml`（参考格式和字段）
- 现有 `awaiter.toml` 格式：`core/src/agent/builtins/awaiter.toml`（参考格式和字段）

---

## Task 5：Coordinator System Prompt 注入

**目的**：创建 team 时自动注入编排指令到 lead agent thread。

### 5.1 新建文件

#### `core/src/agent/builtins/coordinator_prompt.md`

Markdown 模板，通过 `include_str!()` 编译时加载，运行时填充变量。

**模板内容**（参考 Claude Code `coordinatorMode.ts` 的 370 行 system prompt 精简版）：

```markdown
# Team Coordinator Instructions

You are the lead of team `{team_id}`. You coordinate work across these members:

{members_list}

## Communication
- Use `send_message` to talk to members: `send_message(to="member_name", team_id="{team_id}", message="...")`
- Use `send_message(broadcast=true, team_id="{team_id}", message="...")` to broadcast
- Use `wait` to check member status

## Workflow
1. **Understand**: Read the user's request carefully. Do NOT delegate understanding.
2. **Plan**: Break the task into independent sub-tasks.
3. **Dispatch**: Send precise, specific instructions to each member via `send_message`.
4. **Monitor**: Use `wait` to check progress. Use `send_message` for follow-ups.
5. **Verify**: When members complete, review results before reporting to user.
6. **Cleanup**: Use `delete_team` when all work is done.

## Decision Matrix
- Independent read-only tasks → spawn/message in parallel
- Tasks modifying same files → serialize (one after another)
- Need more info → spawn explorer agent first, then decide
```

### 5.2 要修改的文件

#### `core/src/tools/handlers/multi_agents/create_team.rs`（Task 3 新建的文件）

在 team 创建完成后、返回结果前，注入 coordinator prompt：

```rust
// 在所有成员 spawn 完成后：
let coordinator_template = include_str!("../../agent/builtins/coordinator_prompt.md");
let members_list = team_members.iter()
    .map(|m| format!("- **{}** (id: {}, role: {})", m.name, m.agent_id, m.agent_type.as_deref().unwrap_or("default")))
    .collect::<Vec<_>>()
    .join("\n");

let coordinator_prompt = coordinator_template
    .replace("{team_id}", &team_id)
    .replace("{members_list}", &members_list);

// 注入到当前 thread（lead）
if let Some(agent_control) = session.agent_control() {
    let _ = agent_control
        .inject_developer_message_without_turn(
            turn.thread_id().clone(),
            coordinator_prompt,
        )
        .await;
}
```

**关键 API**：`AgentControl::inject_developer_message_without_turn()`（`control.rs` Line 357）

### 5.3 验证规则

```bash
cargo build -p codex-core
cargo test -p codex-core

# 确认模板文件存在
test -f codex-rs/core/src/agent/builtins/coordinator_prompt.md && echo "OK"

# 确认 include_str! 引用正确
grep -n "coordinator_prompt.md" codex-rs/core/src/tools/handlers/multi_agents/create_team.rs
# 应有 include_str! 引用

cargo clippy -p codex-core -- -D warnings
```

### 5.4 参考信息

- Claude Code coordinator prompt 完整内容：`reference/claude-code-sourcemap/restored-src/src/coordinator/coordinatorMode.ts`（`getCoordinatorSystemPrompt()` 函数）
- `inject_developer_message_without_turn` API：`core/src/agent/control.rs` Line 357

---

## Task 6：Agent Memory（跨 Session 知识）

**目的**：每个角色类型共享一个 MEMORY.md，agent spawn 时自动注入。

### 6.1 新建文件

#### `core/src/agent/memory.rs`

```rust
use std::path::{Path, PathBuf};

/// 获取 agent memory 目录路径
pub(crate) fn agent_memory_dir(codex_home: &Path, role_name: &str) -> PathBuf {
    codex_home.join("agent-memory").join(sanitize_role(role_name))
}

/// 获取 agent memory 文件路径
pub(crate) fn agent_memory_path(codex_home: &Path, role_name: &str) -> PathBuf {
    agent_memory_dir(codex_home, role_name).join("MEMORY.md")
}

/// 读取 agent memory（如果存在）
pub(crate) async fn read_agent_memory(
    codex_home: &Path,
    role_name: &str,
) -> Option<String> {
    let path = agent_memory_path(codex_home, role_name);
    tokio::fs::read_to_string(&path).await.ok()
}

/// 清理角色名（防止路径注入）
fn sanitize_role(role: &str) -> String {
    role.chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}
```

### 6.2 要修改的文件

#### `core/src/agent/mod.rs`

新增 `pub(crate) mod memory;`

#### `core/src/tools/handlers/multi_agents/spawn.rs`（266 行）

在 agent spawn 完成后、发送初始 input 前，检查并注入 memory：

```rust
// 在 spawn_agent_thread_with_options() 成功后：
let role_name = args.agent_type.as_deref().unwrap_or("default");
if let Some(memory_content) = crate::agent::memory::read_agent_memory(
    &session.codex_home(),
    role_name,
).await {
    let memory_prompt = format!(
        "# Agent Memory\nThe following is your persistent memory from previous sessions:\n\n{}",
        memory_content
    );
    let _ = agent_control
        .inject_developer_message_without_turn(agent_id.clone(), memory_prompt)
        .await;
}
```

### 6.3 存储结构

```
$CODEX_HOME/
└── agent-memory/
    ├── explorer/
    │   └── MEMORY.md
    ├── plan/
    │   └── MEMORY.md
    ├── verify/
    │   └── MEMORY.md
    └── coordinator/
        └── MEMORY.md
```

Agent 可以通过文件写入工具更新自己的 MEMORY.md。无需额外的写入 API。

### 6.4 验证规则

```bash
cargo build -p codex-core
cargo test -p codex-core

# 确认模块存在
grep -n "mod memory" codex-rs/core/src/agent/mod.rs
# 应有 pub(crate) mod memory;

# 确认 spawn.rs 中有 memory 注入逻辑
grep -n "read_agent_memory\|agent_memory" codex-rs/core/src/tools/handlers/multi_agents/spawn.rs
# 应有调用

cargo clippy -p codex-core -- -D warnings
```

### 6.5 参考信息

- Claude Code agent memory 实现：`reference/claude-code-sourcemap/restored-src/src/tools/AgentTool/agentMemory.ts`
- 存储路径模式：`~/.claude/agent-memory/{agentType}/MEMORY.md`（Claude Code）→ `$CODEX_HOME/agent-memory/{role}/MEMORY.md`（Codex）

---

## Task 7：最终清理 & 注册更新

**目的**：确保所有工具注册正确，清理残留代码，更新测试。

### 7.1 要修改的文件

#### `core/src/tools/spec.rs` — build_specs() 最终状态

```rust
if config.collab_tools {
    let multi_agent_handler = Arc::new(MultiAgentHandler);
    
    // 7 个核心工具
    let tools = [
        (create_spawn_agent_tool(config), "spawn_agent"),
        (create_send_message_tool(), "send_message"),
        (create_resume_agent_tool(), "resume_agent"),
        (create_wait_tool(), "wait"),
        (create_close_agent_tool(), "close_agent"),
        (create_team_tool(config), "create_team"),
        (create_delete_team_tool(), "delete_team"),
    ];
    
    for (spec, name) in tools {
        builder.push_spec_with_parallel_support(spec, true);
        builder.register_handler(name, multi_agent_handler.clone());
    }
}
```

#### `core/src/tools/handlers/multi_agents.rs` — handle() 最终状态

```rust
async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let (session, turn, call_id, arguments, tool_name) = extract_invocation(invocation);
    
    match tool_name.as_str() {
        "spawn_agent"   => spawn::handle(session, turn, call_id, arguments).await,
        "send_message"  => send_message::handle(session, turn, call_id, arguments).await,
        "resume_agent"  => resume_agent::handle(session, turn, call_id, arguments).await,
        "wait"          => wait::handle(session, turn, call_id, arguments).await,
        "close_agent"   => close_agent::handle(session, turn, call_id, arguments).await,
        "create_team"   => create_team::handle(session, turn, call_id, arguments).await,
        "delete_team"   => delete_team::handle(session, turn, call_id, arguments).await,
        other           => Err(FunctionCallError::UnknownFunction(other.to_string())),
    }
}
```

#### 最终模块声明

```rust
mod spawn;
mod send_message;      // 新建
mod resume_agent;
mod wait;
mod create_team;       // 新建
mod delete_team;       // 新建
pub mod close_agent;   // 保留（pub 因为被外部引用）
#[cfg(test)]
mod tests;
```

#### `core/src/tools/handlers/multi_agents/tests.rs`

- 更新所有测试用例引用新工具名
- 新增测试：
  - `test_send_message_direct` — 直接发送
  - `test_send_message_broadcast` — team 广播
  - `test_send_message_ask_lead` — 向 lead 发送
  - `test_create_team_basic` — 基本 team 创建
  - `test_delete_team_cleanup` — team 删除含 worktree 清理
  - `test_spawn_agent_with_plan_role` — plan 角色 spawn
  - `test_spawn_agent_with_verify_role` — verify 角色 spawn
  - `test_spawn_agent_with_memory` — agent memory 注入

### 7.2 清理残留

**删除未使用的 helper 函数**（在 `multi_agents.rs` 中检查）：
- 如果 `dispatch_teammate_idle_hook()` 不再被任何模块调用，删除它
- 检查 `TEAM_TASKS_DIR` 常量是否已删除
- 检查 `PersistedTeamConfig` 中是否有 tasks 相关字段需要清理

**清理 import**：
- 运行 `cargo clippy` 检查 unused import warnings
- 移除所有 `#[allow(dead_code)]` 如果对应代码已删除

### 7.3 最终验证规则（全量）

```bash
# 1. 编译通过
cargo build -p codex-core

# 2. 全量测试通过
cargo test -p codex-core

# 3. Clippy 无 warning
cargo clippy -p codex-core -- -D warnings

# 4. 确认工具数量正确（7 个）
grep -c "register_handler" codex-rs/core/src/tools/spec.rs | grep "collab" -A 20
# collab_tools block 中应恰好有 7 个 register_handler 调用

# 5. 确认无残留旧工具引用
grep -rn "team_task_\|team_inbox_\|team_broadcast\|team_ask_lead\|team_cleanup\|wait_team\|close_team\|spawn_team\|send_input" \
  codex-rs/core/src/tools/spec.rs \
  codex-rs/core/src/tools/handlers/multi_agents.rs
# 应为空（仅注释中可能残留，实际代码中不应有）

# 6. 确认新角色可用
grep -c "plan\|verify\|coordinator" codex-rs/core/src/agent/role.rs
# 应有 3+ 个匹配（角色定义）

# 7. 确认文件结构正确
ls codex-rs/core/src/tools/handlers/multi_agents/
# 应包含: spawn.rs, send_message.rs, resume_agent.rs, wait.rs,
#          create_team.rs, delete_team.rs, tests.rs
# 不应包含: send_input.rs, team_*.rs, inbox.rs, locks.rs,
#           spawn_team.rs, wait_team.rs, close_team.rs

# 8. 确认 builtins 文件存在
ls codex-rs/core/src/agent/builtins/
# 应包含: explorer.toml, awaiter.toml, plan.toml, verify.toml,
#          coordinator.toml, coordinator_prompt.md
```

---

## 汇总：执行顺序与依赖关系

```
Task 1: 删除 Team Task 系统        ← 无依赖，纯减法，最安全
  ↓
Task 4: 扩展 Agent 角色            ← 独立模块，可与 Task 1 并行
  ↓
Task 2: 合并消息工具                ← 依赖 Task 1（task 相关消息已移除）
  ↓
Task 3: 合并 Team 生命周期          ← 依赖 Task 2（消息工具已就位）
  ↓
Task 5: Coordinator Prompt 注入     ← 依赖 Task 3（create_team.rs 已存在）
  ↓
Task 6: Agent Memory               ← 依赖 Task 4（角色系统已扩展）
  ↓
Task 7: 最终清理 & 验证             ← 依赖所有前置 Task
```

## 变更统计预估

| 类别 | 数量 |
|------|------|
| 删除文件 | 16 个 .rs 文件 |
| 新建文件 | 7 个（3 .toml + 1 .md + 3 .rs） |
| 修改文件 | 4 个（multi_agents.rs, spec.rs, role.rs, spawn.rs） |
| 净减少代码行 | ~1500 行（删除 ~2100 行，新增 ~600 行） |
| 工具数量 | 18 → 7 |
| Agent 角色 | 3 → 6 |
