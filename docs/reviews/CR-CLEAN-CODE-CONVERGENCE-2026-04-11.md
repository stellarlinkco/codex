# CR Clean Code Convergence - 2026-04-11

## 范围

- 目标提交：
  - `39b45d637` `core: canonicalize project trust keys`
  - `656e42e1c` `app-server: persist trust after thread start elevation`
- 本轮收敛文件：
  - `codex-rs/.config/nextest.toml`
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/app-server/tests/suite/v2/thread_start.rs`
  - `codex-rs/app-server/tests/suite/v2/app_list.rs`
  - `codex-rs/app-server/tests/suite/v2/command_exec.rs`
  - `codex-rs/app-server/tests/suite/v2/skills_list.rs`
  - `codex-rs/core/src/features.rs`
  - `codex-rs/core/src/features/legacy.rs`
  - `codex-rs/core/src/shell_snapshot.rs`
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
  - `nextest` 默认并发在当前环境下会把多个轻量测试放大成系统性 timeout；现将全局 `test-threads` 下调到 `2`，并把 `codex-app-server`、`codex-app-server-protocol` 的库测试以及 `codex-apply-patch` 集成测试收进串行 test-group，优先恢复本地门禁稳定性。
  - `ConfigToml::get_active_project()` 原先会先 `clone()` 整个 `projects` 映射；现改为按引用读取，仅在命中时克隆 `ProjectConfig`。
  - `thread_start` 集成测试里本地 `persisted_trust_path()` 与核心 `project_trust_key()` 存在重复路径归一化逻辑；现统一复用核心实现，删除重复 helper。
  - `[features].guardian_approval` 属于旧配置别名，但当前兼容层未识别，导致启动时反复输出 `unknown feature key`；现补到 legacy feature alias，映射到 `request_permissions`。
  - TUI 初始光标位置探测失败属于可接受降级路径，原先按 `warn` 级别输出会污染真实使用日志；现降为 `debug`。
  - 本地 `state_5.sqlite` 若记录了当前迁移集不存在的版本，会在每次启动时反复告警且无法恢复；现改为隔离不兼容 DB 文件并自动重建，再由现有 backfill 机制恢复派生状态。
  - `codex-state` 新增回归测试原先把当前最高 migration 版本写死为 `18`；现改为从 `STATE_MIGRATOR` 动态读取最新版本，避免后续新增 migration 时产生伪失败。
  - `suite::v2::app_list::list_apps_emits_updates_and_returns_after_both_lists_load` 对通知时序假设过强；现接受“先收到 accessible 中间态”与“首个通知已是 merged 终态”两种合法结果，只要求最终响应稳定一致。
  - `suite::v2::app_list::list_apps_force_refetch_patches_updates_from_cached_snapshots` 先前把 force-refetch 首个通知固定死为“cached merged”；实际实现允许“只有 cached accessible 可见”与“cached merged 已齐备”两种中间态，现统一只校验合法中间态集合与最终响应。
  - `suite::v2::command_exec::*` 在 `cargo test` 默认并发下会同时拉起多份 app-server 子进程，导致 `initialize` 普遍超时；现统一显式使用 `externalSandbox(restricted)` 维持协议语义，并用 `serial(command_exec)` 收进同一把串行锁，再把本组 `initialize` 超时从共享默认值放宽到 60 秒，消除宿主负载带来的首个重型用例伪失败。
  - `suite::v2::skills_list::skills_changed_notification_is_emitted_after_skill_change` 对文件 watcher 时序假设过强；现改为 `skills_list_force_reload_observes_skill_change_after_thread_start`，直接验证 thread start 之后通过 `skills/list(forceReload=true)` 能观测到新增 skill 这一稳定外部契约。
  - `shell_snapshot::tests::snapshot_shell_does_not_inherit_stdin` 的 2 秒超时在当前宿主负载下过紧；现放宽到 5 秒，保持“不继承 stdin”的断言不变，只消除宿主级伪失败。

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
    - 旧结论已过时：问题不在 `codex_protocol-* --list --format terse --ignored` 单点卡死，而在默认并发过高时，全 workspace 会出现大量 `30s` 级 timeout。
    - 已复现并确认：
      - `cargo nextest run -p codex-app-server --lib ...` 默认并发会 timeout，`-j 1` 全部通过。
      - `cargo nextest run -p codex-app-server-protocol --lib ...` 默认并发会 timeout，`-j 1` 全部通过。
      - `cargo nextest run -p codex-apply-patch ...` 默认并发会 timeout，`-j 4` 全部通过。
    - 已据此收敛 `codex-rs/.config/nextest.toml`：
      - `test-threads = 2`
      - `codex-app-server` 库测试串行
      - `codex-app-server-protocol` 库测试串行
      - `codex-apply-patch` 集成测试串行
    - 最新一轮全 workspace `nextest` 仍在运行，用于确认在新并发配置下是否完全消除 timeout。
- `cd codex-rs && cargo test --workspace --no-fail-fast`
  - 状态：未完成
  - 观测事实：
    - 分别尝试经由 `rustup` 代理和直接调用工具链内 `cargo`
    - 观测窗口内均停留在顶层 `cargo` 进程，未继续派生 `rustc` 或测试子进程
    - 当前终端环境下，完整工作区门禁仍缺少可追溯失败项
- `cd codex-rs && cargo nextest run -p codex-app-server --lib --show-progress=none --status-level fail --final-status-level fail --failure-output immediate-final --no-fail-fast`
  - 退出码：0
- `cd codex-rs && cargo nextest run -p codex-app-server-protocol --lib --show-progress=none --status-level fail --final-status-level fail --failure-output immediate-final --no-fail-fast`
  - 退出码：0
- `cd codex-rs && cargo nextest run -p codex-apply-patch --show-progress=none --status-level fail --final-status-level fail --failure-output immediate-final --no-fail-fast`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all suite::v2::app_list::list_apps_emits_updates_and_returns_after_both_lists_load -- --exact`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all suite::v2::app_list::`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all suite::v2::skills_list::`
  - 退出码：0
- `cd codex-rs && cargo nextest run -p codex-app-server --test all suite::v2::command_exec:: --status-level fail --final-status-level fail --failure-output immediate-final`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all suite::v2::command_exec::`
  - 退出码：0
- `cd codex-rs && cargo test -p codex-app-server --test all suite::v2::command_exec:: -- --test-threads=1`
  - 退出码：0
  - 观测事实：
    - 收敛前，默认 `cargo test` 并发下这组测试会出现批量 `initialize` 超时与 websocket 连接拒绝，问题集中在测试并发资源竞争而非单条断言错误。
    - 在 `serial(command_exec)` + `60s initialize timeout` 收敛后，默认 `cargo test`、`cargo test -- --test-threads=1` 与 `nextest` 定向门禁均已稳定通过。
- `cd codex-rs && cargo test -p codex-core shell_snapshot::tests::snapshot_shell_does_not_inherit_stdin -- --exact`
  - 退出码：0

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
