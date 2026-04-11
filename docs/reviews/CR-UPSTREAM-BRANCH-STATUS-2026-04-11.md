# CR-UPSTREAM-BRANCH-STATUS-2026-04-11

## Summary

- 审计对象：
  - 当前执行分支：`fix/ci-watchdog-rust-test-stability-20260409`
  - 参考文档：
    - `CR-INFLIGHT-BRANCH-MATRIX-2026-04-05.md`
    - `CR-UPSTREAM-GAP-MATRIX-2026-04-05.md`
    - `CR-UPSTREAM-BRANCH-STATUS-2026-04-09.md`
- 审计目标：
  - 收口这轮“筛选并吸纳适合当前 fork 基线的 upstream 原子改动”任务。
  - 把代码侧已完成吸纳、已覆盖、明确拒绝和纯文档状态一次写清。

## Control Contract

- Primary Setpoint：
  - 当前分支工作区干净。
  - 已吸纳改动全部完成最小充分验证。
  - `fork/fix/ci-watchdog-rust-test-stability-20260409` 与本地同步。
- Acceptance：
  - 对本轮最终结果给出：
    - 已吸纳提交清单
    - 验证证据
    - 仍未吸纳项的最终裁决
    - 文档状态
- Guardrails：
  - 不把 `git cherry` 的 patch-id 残差误判为真实功能缺口。
  - 不把一次长跑中的单测偶发失败误判为本轮改动回归。
  - 不为了“清空分支”去强吸纳需要前置架构的大补丁。

## Final State

### 1. 已完成吸纳并推送的本轮末段提交

| 提交 | 标题 | 当前状态 | 验证 |
| --- | --- | --- | --- |
| `0a4b94f16` | `tui: stabilize resume picker timestamps` | 已吸纳 | `cargo test -p codex-tui` |
| `891e764bd` | `fix: stabilize Windows permissions escalation test (#16825)` | 已吸纳 | `cargo test -p codex-core rejects_escalated_permissions_when_policy_not_on_request -- --nocapture` |
| `39b45d637` | `core: canonicalize project trust keys` | 已吸纳 | `cargo test -p codex-core`，外加单独复跑 `suite::cli_stream::responses_mode_stream_cli` 通过 |
| `656e42e1c` | `app-server: persist trust after thread start elevation` | 已吸纳 | `cargo test -p codex-app-server --test all` |

### 2. 关键能力结果

- project trust 路径归一化已经落地：
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/src/config_loader/mod.rs`
- app-server 在 `thread/start` 请求中，遇到显式提升到可信工作区语义的 sandbox 时，会持久化 trust，并在后续线程启动中加载项目配置：
  - `codex-rs/app-server/src/codex_message_processor.rs`
  - `codex-rs/app-server/tests/suite/v2/thread_start.rs`
- `resume picker` 时间快照漂移问题已经收口。
- Windows 权限升级相关测试稳定性问题已经收口。

### 3. 已判定不再继续吸纳的代码分支

| 分支 | 最终裁决 | 原因 |
| --- | --- | --- |
| `feat/upstream-tui-interrupt-handled-20260407-194002` | Reject for current baseline | 依赖当前 fork 不存在的 app-server TUI 提交路径，不是孤立补丁 |
| `feat/upstream-app-server-thread-shell-aware-20260408-015900` | Reject for current baseline | 依赖当前 fork 尚不存在的 `thread/shellCommand` 协议与服务端能力 |
| `feat/upstream-mention-popup-ux` | Reject for current baseline | 会覆盖当前 fork 已定制的 mention 标签语义，不适合直接并入 |
| `feat/upstream-zellij-redraw-20260407-195015` | Already handled | 相关等价修复已落地，不再有实际代码缺口 |
| `feat/upstream-tui-context-window-runtime` | Already covered | 当前 fork 基线已具备等价实现与测试 |

### 4. 关于 `git cherry` 残余项

- `feat/upstream-app-server-trust-canonicalization-20260408-084704` 在 `git cherry` 下仍会显示 `+ a6dc973c9`。
- 这不是实际缺口，而是 patch-id 残余：
  - 当前分支已经通过 `39b45d637` 吸纳核心语义。
  - 与 upstream 的实际差异只剩 `codex-rs/core/src/tools/sandboxing.rs` 里一个测试断言形式。
  - 当前 fork 这版断言更贴合实际行为，不再继续追平该残差。

### 5. 文档分支状态

- `docs/upstream-branch-status-20260409`
  - 已处理。
  - 文档提交 `0219a0989` 已吸纳到当前分支工作区。
  - 本次新增本文件，作为截至 `2026-04-11` 的最终收口说明。

## Evidence

### Commands

```bash
git status --short --branch
git cherry fix/ci-watchdog-rust-test-stability-20260409 feat/upstream-app-server-trust-canonicalization-20260408-084704

just fmt

cargo test -p codex-tui
cargo test -p codex-core
cargo test -p codex-core suite::cli_stream::responses_mode_stream_cli -- --nocapture
cargo test -p codex-app-server --test all
```

### Key Outputs

- `git status --short --branch`
  - `## fix/ci-watchdog-rust-test-stability-20260409...fork/fix/ci-watchdog-rust-test-stability-20260409`
- `cargo test -p codex-core`
  - 长跑过程中 `suite::cli_stream::responses_mode_stream_cli` 曾出现一次失败
  - 随后单独复跑该用例通过
- `cargo test -p codex-app-server --test all`
  - `238 passed; 0 failed; 1 ignored`

## Decision

- 当前这轮 upstream 原子改动吸纳任务已经完成。
- 代码侧已没有新的适合当前 fork 基线的原子提交需要继续吸纳。
- 文档侧已经补齐：
  - 历史矩阵
  - 2026-04-09 审计快照
  - 2026-04-11 最终收口说明

## Gate Boundary

- 若后续继续推进 upstream 同步，应开启新一轮任务，而不是在本轮分支上继续无目标筛选。
- 后续若重新评估 `interrupt-handled` 或 `thread-shell-aware`，前提必须先补齐对应架构能力，再进入吸纳流程。
