# CR-UPSTREAM-BRANCH-STATUS-2026-04-12

## Summary

- 审计对象：
  - 当前执行分支：`fix/ci-watchdog-rust-test-stability-20260409`
  - 项目上游：`origin = https://github.com/stellarlinkco/codex.git`
  - 参考源：`openai = https://github.com/openai/codex.git`
- 本轮目标：
  - 在不改变当前 fork 主体控制结构的前提下，重新分析 `be13f03c3..3895ddd6b` 的 upstream 新增量。
  - 只吸纳适合当前基线、可用最小验证闭环证明正确的原子改动。

## Control Contract

- Primary Setpoint：
  - 只吸纳低耦合、小范围、行为明确的 upstream 增量。
  - 不为追平参考源而引入当前 fork 尚不存在的控制面/状态面子系统。
- Boundary：
  - 优先 TUI 行为修复与 plan type 数据兼容。
  - defer 大重构、协议大迁移、guardian 整套能力引入。
- Disturbance：
  - 当前 fork 与 `openai/main` 在 `core`、`tui`、`guardian` 上均已再次分叉。
  - upstream 某些提交依赖当前 fork 不存在的模块或更大范围的前置提交。

## Delta Window

- 基线窗口：`be13f03c3..3895ddd6b`
- `git rev-list --left-right --count origin/main...openai/main`
  - `166 965`

## Decision Matrix

| 提交 | 标题 | 裁决 | 说明 |
| --- | --- | --- | --- |
| `51d58c56d` | `Handle closed TUI input stream as shutdown` | Adopt | 当前 fork 已按本地 `tui/src/app.rs` 结构落地；输入流关闭时统一走 `ShutdownFirst`。 |
| `1e2702836` | `Clear /ps after /stop` | Adopt | 当前 fork 已在 `tui/src/chatwidget.rs` 落地；停止后台终端时同步清空本地 UI 状态。 |
| `7a6266323` | `Restore codex-tui resume hint on exit` | Adopt | 当前 fork 已在 `tui/src/main.rs` 落地；退出时恢复 token usage 与 resume hint 输出。 |
| `3b948d9dd` | `Support prolite plan type` | Adopt | 已按当前 fork 路径做本地适配，覆盖 `backend-client/core/protocol/tui` 的 plan type 映射。 |
| `0bdeab330` | `recall accepted slash commands locally` | Defer | 依赖较大的 `chatwidget` 结构改造和新增分发模块，不是原子补丁。 |
| `39cc85310` | `Add use_agent_identity feature flag` | Defer | 价值较低，且不属于本轮控制目标。 |
| `640d3a036` | `Update issue labeler agent labels` | Reject | 仅工作流标签变更，不是产品主线能力。 |
| `163ae7d3e` | `fix (#17493)` | Defer | 仅 prompt/文案收敛，不是当前 fork 需要优先吸纳的行为差异。 |
| `1325bcd3f` | `refactor name and namespace to single type` | Reject for current round | 32 文件重构，超出本轮 actuator budget。 |
| `ba839c23f` | `changing decision semantics after guardian timeout` | Defer | 当前 `HEAD` 不存在 upstream 对应的 `core/src/guardian/*` 子系统，不是小补丁。 |
| `3895ddd6b` | `Clarify guardian timeout guidance` | Defer | 依赖同一组 guardian 语义改动；单独吸纳没有意义。 |

## Implemented Changes

### 1. TUI 退出与关闭路径

- `codex-rs/tui/src/app.rs`
  - 将 `tui_events.next()` 的关闭分支显式转成 `ExitMode::ShutdownFirst`。
  - 输入流关闭时输出 warning，并走已有 shutdown 路径取消活跃线程和挂起审批。
- `codex-rs/tui/src/chatwidget.rs`
  - `/stop` 清理后台终端时同步清空 `unified_exec_processes`，并刷新 footer。
- `codex-rs/tui/src/main.rs`
  - 恢复退出时的 token usage 输出。
  - 恢复 `codex resume ...` 提示。
  - 保持当前 fork 可编译路径，使用 `codex_core::util::resume_command` 而不是 upstream 当前依赖。

### 2. `prolite` 数据兼容

- `codex-rs/codex-backend-openapi-models/src/models/rate_limit_status_payload.rs`
  - 增加 `ProLite` 和 `Unknown`，保证后端返回新 plan type 时可解码。
- `codex-rs/backend-client/src/client.rs`
  - 增加 `ProLite -> AccountPlanType::ProLite` 映射。
  - 将未知 backend plan 映射到 `AccountPlanType::Unknown`。
- `codex-rs/core/src/token_data.rs`
  - 增加 `KnownPlan::ProLite`。
  - 支持把原始值 `prolite` 解析为已知 plan。
- `codex-rs/core/src/auth.rs`
  - `account_plan_type()` 补上 `ProLite` 对外映射。
- `codex-rs/core/src/error.rs`
  - `UsageLimitReachedError` 将 `Pro` 与 `ProLite` 统一为同一购买 credits 提示。
- `codex-rs/protocol/src/account.rs`
  - 增加协议层 `PlanType::ProLite`。
- `codex-rs/tui/src/status/helpers.rs`
  - 增加 `ProLite -> "Pro Lite"` 的显示特判。
- `codex-rs/tui/src/tooltips.rs`
  - 将 `ProLite` 归入付费 plan 提示分支。

## Evidence

### Commands

```bash
git fetch origin
git fetch openai
git log --oneline --decorate --reverse be13f03c3..openai/main

cd codex-rs
just fmt

cargo test -p codex-backend-client -p codex-core
cargo test -p codex-protocol
cargo test -p codex-tui format_exit_messages -- --nocapture
cargo test -p codex-tui shutdown_first_exit -- --nocapture
cargo test -p codex-tui plan_type_display_name_formats_prolite -- --nocapture
```

### Key Results

- `cargo test -p codex-backend-client -p codex-core`
  - `codex-backend-client`: `9 passed; 0 failed`
  - `codex-core`: `1569 passed; 0 failed; 5 ignored`
- `cargo test -p codex-protocol`
  - `87 passed; 0 failed`
- `cargo test -p codex-tui format_exit_messages -- --nocapture`
  - `src/main.rs` 新增 2 个退出提示测试均通过
- `cargo test -p codex-tui shutdown_first_exit -- --nocapture`
  - 2 个 shutdown 路径测试通过
- `cargo test -p codex-tui plan_type_display_name_formats_prolite -- --nocapture`
  - `Pro Lite` 展示名称测试通过

## Gate Boundary

- 本轮没有吸纳 guardian timeout 语义改动。
- 原因不是“还没做完”，而是当前 fork 缺少 upstream 对应的 guardian 子系统，继续推进会把任务从“小补丁吸纳”升级成“跨控制面能力移植”。
- 若后续要继续评估 guardian 这组差异，应新开一轮任务，先确认：
  - 当前 fork 是否接受这套控制面能力；
  - 是否允许引入 `core/src/guardian/*` 及其相关状态与测试基线；
  - 需要保留还是替换当前 fork 的既有行为。
