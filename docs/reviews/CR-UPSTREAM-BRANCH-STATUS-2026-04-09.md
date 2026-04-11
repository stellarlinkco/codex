# CR-UPSTREAM-BRANCH-STATUS-2026-04-09

> 历史快照。后续实际收口结果见 `CR-UPSTREAM-BRANCH-STATUS-2026-04-11.md`。

## Summary

- 审计对象：
  - `feat/upstream-zellij-redraw-20260407-195015`
  - `feat/upstream-tui-context-window-runtime`
  - `feat/upstream-tui-interrupt-handled-20260407-194002`
  - `feat/upstream-app-server-thread-shell-aware-20260408-015900`
- 审计目标：
  - 收口 2026-04-08 识别出的 4 个“空分支 / 未闭环分支”。
  - 判断每个候选提交在当前 fork 基线下的真实状态：
    - 已吸纳
    - 已被基线覆盖
    - 依赖缺失，不适合直接吸纳

## Control Contract

- Primary Setpoint：
  - 每个目标分支都要得到明确裁决和本地验证证据。
- Acceptance：
  - 对每个分支给出：
    - 对应上游提交
    - 当前裁决
    - 证据
    - 若不吸纳，说明根因
- Guardrails：
  - 不把“cherry-pick 为空”误判为“未处理”。
  - 不把“测试过滤错误导致 0 tests”误判为“验证通过”。
  - 不把缺失前置协议 / 架构能力的测试补丁硬塞进当前 fork。

## State Estimate

### 1. `feat/upstream-zellij-redraw-20260407-195015`

- 上游提交：
  - `0bd31dc38` `fix(tui): handle zellij redraw and composer rendering`
- 当前裁决：
  - Adopt
- 落地结果：
  - 已完成等价吸纳。
  - 为适配当前 fork 基线，zellij 检测改为复用现有 `codex_core::terminal`，没有引入当前仓库不存在的 `codex_terminal_detection` crate。
  - 分支头 / 远端头一致：`dcebe5a8e`
- 本地验证：
  - `cargo test -p codex-tui zellij -- --nocapture`
  - 结果：2 个相关测试通过
    - `insert_history::tests::vt100_zellij_mode_inserts_history_and_updates_viewport`
    - `bottom_pane::chat_composer::tests::zellij_empty_composer_snapshot`

### 2. `feat/upstream-tui-context-window-runtime`

- 上游提交：
  - `e8d7ede83` `Fix TUI context window display before first TokenCount`
- 当前裁决：
  - Already covered
- 观察结果：
  - `git cherry-pick e8d7ede83` 为空。
  - 对应核心实现和回归测试已在当前 fork 基线中存在：
    - `apply_turn_started_context_window`
    - `turn_started_uses_runtime_context_window_before_first_token_count`
    - `live_turn_started_refreshes_status_line_with_runtime_context_window`
  - 分支头 / 远端头一致：`6bb1e4fdc`
- 本地验证：
  - `cargo test -p codex-tui live_turn_started_refreshes_status_line_with_runtime_context_window -- --nocapture`
  - `cargo test -p codex-tui turn_started_uses_runtime_context_window_before_first_token_count -- --nocapture`
  - 结果：2 个回归测试均通过

### 3. `feat/upstream-tui-interrupt-handled-20260407-194002`

- 上游提交：
  - `74d714913` `Fix regression: "not available in TUI" error message`
- 当前裁决：
  - Defer / Not directly applicable
- 根因：
  - 该提交依赖当前 fork 基线中不存在的一整套 app-server TUI 提交路径，包括：
    - `try_submit_active_thread_op_via_app_server`
    - 对 `AppCommandView::Interrupt` 的 app-server 转发逻辑
    - 对应的 app-server TUI regression test scaffold
  - 在当前分支上直接 cherry-pick 会落在错误上下文，属于“前置架构未到位”的补丁，不是孤立可吸纳修复。
- 结论：
  - 当前 fork 不具备直接吸纳条件。
  - 分支保持不变，分支头 / 远端头一致：`6bb1e4fdc`

### 4. `feat/upstream-app-server-thread-shell-aware-20260408-015900`

- 上游提交：
  - `862158b9e` `app-server: make thread/shellCommand tests shell-aware`
- 当前裁决：
  - Reject for current baseline
- 根因：
  - 该提交不是纯测试文本修补，而是建立在当前 fork 尚不存在的 app-server v2 能力之上：
    - `ThreadShellCommandParams`
    - `ThreadShellCommandResponse`
    - `CommandExecutionSource`
    - `send_thread_shell_command_request`
  - 在当前 fork 上恢复该测试文件后，编译立即失败，说明这不是“测试不稳”，而是“协议 / 行为能力未落地”。
- 处理结果：
  - 本地曾做一次试验性 cherry-pick 与回退，用于确认不适配根因。
  - 当前相对远端分支的文件级净 diff 为 0。
  - 远端头仍是未吸纳状态：`6bb1e4fdc`

## Evidence

### Commands

```bash
git cherry-pick 0bd31dc38
git cherry-pick e8d7ede83
git cherry-pick 74d714913
git cherry-pick 862158b9e

cargo test -p codex-tui zellij -- --nocapture
cargo test -p codex-tui live_turn_started_refreshes_status_line_with_runtime_context_window -- --nocapture
cargo test -p codex-tui turn_started_uses_runtime_context_window_before_first_token_count -- --nocapture
```

### Key Outputs

- `feat/upstream-zellij-redraw-20260407-195015`
  - 本地 / 远端：`dcebe5a8e`
  - `cargo test -p codex-tui zellij -- --nocapture` 通过
- `feat/upstream-tui-context-window-runtime`
  - `git cherry-pick e8d7ede83` 为空
  - 2 个上下文窗口回归测试通过
- `feat/upstream-tui-interrupt-handled-20260407-194002`
  - cherry-pick 命中错误上下文，所需 app-server TUI 路径缺失
- `feat/upstream-app-server-thread-shell-aware-20260408-015900`
  - 恢复测试文件后编译失败，直接缺少 `ThreadShellCommand*` / `CommandExecutionSource` / request helper

## Decision

- 已完成真实吸纳：
  - `feat/upstream-zellij-redraw-20260407-195015`
- 已被当前 fork 基线覆盖：
  - `feat/upstream-tui-context-window-runtime`
- 当前 fork 基线下不应直接吸纳：
  - `feat/upstream-tui-interrupt-handled-20260407-194002`
  - `feat/upstream-app-server-thread-shell-aware-20260408-015900`

## Gate Boundary

- 后续若要继续推进 `interrupt-handled`，前提不是“补 cherry-pick 冲突”，而是先确认当前 fork 是否真的要恢复那套 app-server TUI 提交路径。
- 后续若要继续推进 `thread-shell-aware`，前提不是“修测试”，而是先实现 `thread/shellCommand` 对应协议与服务端行为。
