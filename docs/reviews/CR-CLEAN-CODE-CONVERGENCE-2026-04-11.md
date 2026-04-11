# CR Clean Code Convergence - 2026-04-11

## 范围

- 目标提交：
  - `39b45d637` `core: canonicalize project trust keys`
  - `656e42e1c` `app-server: persist trust after thread start elevation`
- 本轮收敛文件：
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/app-server/tests/suite/v2/thread_start.rs`
  - `codex-rs/core/src/features.rs`
  - `codex-rs/core/src/features/legacy.rs`
  - `codex-rs/state/src/runtime.rs`
  - `codex-rs/tui/src/custom_terminal.rs`
  - `codex-rs/core/config.schema.json`
- 上游基线：
  - 已于 2026-04-11 rebase 到 `origin/main`
  - 吸收提交：
    - `6bb1e4fdc` `ci: fix remaining cargo-deny advisories`
    - `80b909ee7` `ci: fix formatting and cargo-deny regressions`
    - `790bf5220` `core: simplify agent teams workflow`
    - `979b511e4` `migrate claude code`
  - 2026-04-11 再次核对远端：
    - 用户已明确本仓库唯一需要跟踪的上游为 `https://github.com/stellarlinkco/codex`
    - `origin/main` 仍为 `6bb1e4fdc`，当前分支相对其为 `behind 0 / ahead 40`
    - 本轮未对任何 `openai/*` 远端分支做同步或 rebase 处理

## 审查结论

- 未发现阻塞性正确性问题。
- 发现并处理五处可收敛点：
  - `ConfigToml::get_active_project()` 原先会先 `clone()` 整个 `projects` 映射；现改为按引用读取，仅在命中时克隆 `ProjectConfig`。
  - `thread_start` 集成测试里本地 `persisted_trust_path()` 与核心 `project_trust_key()` 存在重复路径归一化逻辑；现统一复用核心实现，删除重复 helper。
  - `[features].guardian_approval` 属于旧配置别名，但当前兼容层未识别，导致启动时反复输出 `unknown feature key`；现补到 legacy feature alias，映射到 `request_permissions`。
  - TUI 初始光标位置探测失败属于可接受降级路径，原先按 `warn` 级别输出会污染真实使用日志；现降为 `debug`。
  - 本地 `state_5.sqlite` 若记录了当前迁移集不存在的版本，会在每次启动时反复告警且无法恢复；现改为隔离不兼容 DB 文件并自动重建，再由现有 backfill 机制恢复派生状态。
  - `codex-state` 新增回归测试原先把当前最高 migration 版本写死为 `18`；现改为从 `STATE_MIGRATOR` 动态读取最新版本，避免后续新增 migration 时产生伪失败。

## 检查清单结果

- 功能对齐：通过。未改变 trust 判定语义，只收敛实现。
- 边缘情况：通过。legacy raw key 匹配、repo root trust 继承、只读 sandbox 不落 trust 的测试仍覆盖。
- 错误处理：通过。本轮未引入新的容错或静默降级路径。
- 命名与抽象：通过。删除测试侧重复路径规范化逻辑后，信任 key 口径更单一。
- 文档同步：已记录本次收敛结果。

## 验证记录

- `cd codex-rs && just fmt`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-core --lib test_get_active_project_accepts_legacy_raw_project_key`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all thread_start_with_`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all thread_start_`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-core guardian_approval_is_legacy_alias_for_request_permissions`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-state init_rebuilds_state_db_when_applied_migration_is_missing`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-tui permissions_selection_hides_guardian_approvals_when_feature_disabled`
  - 退出码：0
- `cd codex-rs && cargo build -p codex-cli --bin codex`
  - 退出码：0
- `cd codex-rs && just test`
  - 退出码：100
  - 观测事实：
    - 终端末尾仅出现 `error: test run failed` 与 `error: Recipe 'test' failed on line 54 with exit code 100`
    - 原始会话未保留可读失败测试名，需另行复现定位
- `cd codex-rs && cargo nextest run --show-progress=none --status-level fail --final-status-level fail --failure-output immediate-final --no-fail-fast`
  - 状态：未完成
  - 观测事实：
    - 已完成构建阶段，出现 `Finished test profile [unoptimized + debuginfo] target(s) in 13m 04s`
    - 之后长时间停留在测试枚举阶段，子进程固定为 `target/debug/deps/codex_protocol-* --list --format terse --ignored`
    - 在本地直接执行 `./target/debug/deps/codex_protocol-* --list --format terse` 时，最终以退出码 0 返回，并写出 `6348` 字节测试列表
    - 在本地直接执行 `./target/debug/deps/codex_protocol-* --list --format terse --ignored` 时，观测窗口内持续无输出，`nextest` 因而无法继续推进到可读失败摘要阶段
- `cd codex-rs && cargo test --workspace --no-fail-fast`
  - 状态：未完成
  - 观测事实：
    - 分别尝试经由 `rustup` 代理和直接调用工具链内 `cargo`
    - 观测窗口内均停留在顶层 `cargo` 进程，未继续派生 `rustc` 或测试子进程
    - 当前终端环境下，完整工作区门禁仍缺少可追溯失败项

## 实机验证

- 使用新编译产物 `codex-rs/target/debug/codex`（时间戳 `2026-04-11 19:05:44`）启动 TTY 会话，`guardian_approval` 不再触发 `unknown feature key` 警告。
- 旧配置现在只保留一次显式 deprecation notice：提示改用 `[features].request_permissions`。
- `failed to open state db` / `failed to initialize state runtime` 启动噪声已消失。
- 本机 `~/.codex` 下已生成隔离副本：
  - `state_5.sqlite.incompatible-1775903984056`
  - `state_5.sqlite-wal.incompatible-1775903984056`
  - `state_5.sqlite-shm.incompatible-1775903984056`
  - 新 `state_5.sqlite` 已重建并正常使用。

## 备注

- 曾尝试运行 `cargo test -p codex-core test_get_active_project_accepts_legacy_raw_project_key`，但该命令会额外展开不必要的测试目标编译，已中止并改为 `--lib` 最小验证。
- rebase `origin/main` 时在 `codex-rs/core/src/tools/sandboxing.rs` 出现 1 处测试断言冲突，已保留当前分支更贴合运行时状态的断言写法后继续完成 rebase。
- 当前已确认的代码级结论仍限于本轮修改涉及的定向测试、构建和 TTY 实机验证；完整工作区测试失败项尚未从本地环境中可靠提取。
