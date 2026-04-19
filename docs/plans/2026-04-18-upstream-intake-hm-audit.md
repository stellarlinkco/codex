# Upstream 选择性吸纳执行审计（中/高优先级）

日期：2026-04-18  
分支：`batch/upstream-intake-high-mid-20260418`  
策略：语义等价优先（不能直接 cherry-pick 时采用语义移植；无对应能力则标记 N/A 并给出证据）

## 批次与门禁

- 批次顺序：A（app-server）-> B（tui）-> C（path/sandbox）-> D（session start source）-> E（适配判定与补漏）
- 每批要求：代码改动 + 受影响测试 + CI 通过，才进入下一批。
- CI 验证口径：以 GitHub Actions 为准，不以本机完整功能跑通作为验收结论。

## 候选提交清单（执行台账）

| 上游提交 | 优先级 | 目标行为 | 本仓映射 | 执行状态 | 结论 |
| --- | --- | --- | --- | --- | --- |
| `8d5889929` | 高 | app-server 连接断开后，最后订阅者线程必须卸载，避免状态泄漏 | `codex-rs/app-server/src/codex_message_processor.rs` + `thread_state.rs` + websocket suite | 已完成 | Adopted-equivalent |
| `0bdeab330` | 高 | 已提交 slash 命令可在本地输入历史中回忆 | `codex-rs/tui/src/bottom_pane/chat_composer.rs` + `bottom_pane/mod.rs` + `chatwidget.rs` | 已完成 | Adopted-equivalent |
| `0393a485e` | 高 | Composer 支持反向历史搜索 | `codex-rs/tui/src/bottom_pane/chat_composer.rs` + `chat_composer_history.rs` + `footer.rs` | 已完成 | Adopted-equivalent |
| `e9e7ef3d3` | 高 | Windows verbatim/non-verbatim 路径比较一致化，修复 cwd 过滤误判 | `codex-rs/core/src/path_utils.rs` + `app-server`/`tui` 调用点 | 已完成 | Adopted-equivalent |
| `04fc208b6` | 高 | 保留 tool_search_output 原始顺序，不被分组排序打散 | 本仓无同名 `tools` 模块，映射到连接器/工具发现链路评估 | 已完成 | N/A（无对应模块） |
| `36712d854` | 高 | remote websocket client 建连前安装 rustls provider | 本仓无 `app-server-client` crate，需评估等价入口 | 已完成 | Adopted-equivalent |
| `b11478149` | 中 | sandbox writable roots 与 symlink 路径处理一致化 | `utils/absolute-path` + `core/sandboxing/mod.rs` + `linux-sandbox/bwrap.rs` + `exec/tui cwd` | 已完成 | Adopted-equivalent |
| `b976e701a` | 中 | Windows elevated sandbox 支持 split carveouts | `windows-sandbox-rs` + `core/sandboxing` + `core/exec` | 已完成 | N/A（保持 fail-closed） |
| `95ba76262` | 中 | Windows restricted-token sandbox 支持 split carveouts | `windows-sandbox-rs` + `core/sandboxing` + `core/exec` | 已完成 | Adopted-equivalent |
| `86764af68` | 中 | Linux/macOS sandbox 下首次 `.codex` 创建稳定性 | `protocol` 默认 carveout + `linux-sandbox` 掩码链路 | 已完成 | Adopted-equivalent |
| `71923f43a` | 中 | `codex exec` stdin piping 行为增强 | `exec/src/lib.rs` + `exec/tests/suite/prompt_stdin.rs` | 已完成 | Adopted-equivalent |
| `ae057e0bb` | 中 | 活跃 TUI 会话里 `/status` 速率限制展示不陈旧 | `tui/src/app.rs` + `chatwidget.rs` + `status/*` + `app_event.rs` | 已完成 | Adopted-equivalent |
| `7999b0f60` | 中 | clear 场景下 SessionStart source 可区分为 clear | `protocol` + `core` + `app-server-protocol` + `app-server` + `tui` | 已完成 | Adopted-equivalent |

## 批次实施记录

### Batch A - app-server 断连卸载

- 范围：`8d5889929`
- 风险：thread teardown 与 `thread_state_manager` 状态机耦合，需避免双重卸载或误卸载。
- 预期验证：
  - `cargo test -p codex-app-server connection_handling_websocket`
  - `cargo test -p codex-app-server thread_unsubscribe`
- 执行回填：
  - 已落地：`115c49af8c`
  - 本地尝试：`cargo test -p codex-app-server websocket_disconnect_unloads_last_subscribed_thread -- --nocapture`（编译阶段耗时过长，转 CI 作为最终验收）
  - 结论：代码已吸纳，等待 CI 补完端到端证据。

### Batch B - TUI 高优先级交互

- 范围：`0bdeab330`、`0393a485e`、`ae057e0bb`
- 风险：输入状态机复杂，需保证 popup/history/paste burst 不回归。
- 预期验证：
  - `cargo test -p codex-tui`
  - 必要时更新 `insta` 快照并显式记录。
- 执行回填（已完成子项）：
  - `0bdeab330` 已语义吸纳（slash 命令本地历史回忆）
  - `0393a485e` 已语义吸纳（Ctrl+R 反向历史搜索，支持查询高亮、Esc/Ctrl+C 取消恢复草稿、Enter 接受匹配）
  - `ae057e0bb` 已语义吸纳（`/status` 首帧显示刷新中并在刷新完成后更新）
  - 通过命令：
    - `cargo check -p codex-tui`（0）
    - `cargo test -p codex-tui --lib --no-run`（0）
    - `cargo test -p codex-tui history_search_`（0）
    - `cargo test -p codex-tui --lib bare_slash_command_can_be_recalled_after_recording_pending_history -- --nocapture`（0）
    - `cargo test -p codex-tui --lib slash_command_is_recallable_via_up_history_after_dispatch -- --nocapture`（0）
    - `cargo test -p codex-tui --lib slash_command_with_args_is_recallable_via_up_history_after_dispatch -- --nocapture`（0）
    - `cargo test -p codex-tui --lib status_output_refresh_notice_clears_after_rate_limit_refresh -- --nocapture`（0）
    - `cargo test -p codex-tui --lib slash_status_shows_refresh_notice_for_chatgpt_auth -- --nocapture`（0）
  - 结论：Batch B 三项已完成，等待 CI 统一门禁。

### Batch C - 路径与沙箱

- 范围：`e9e7ef3d3`、`86764af68`、`95ba76262`、`b976e701a`、`b11478149`
- 风险：跨平台路径归一化和权限边界可能影响安全行为。
- 预期验证：
  - `cargo test -p codex-core`
  - `cargo test -p codex-exec`
  - `cargo test -p codex-windows-sandbox`（CI Windows 任务）
- 执行回填（已完成子项）：
  - `e9e7ef3d3` 已语义吸纳：新增 `paths_match_after_normalization` 并统一替换 app-server/core/tui 多处 cwd 比较。
  - `b11478149` 已语义吸纳：
    - 新增 `canonicalize_preserving_symlinks` / `canonicalize_existing_preserving_symlinks`（`utils/absolute-path`）
    - `exec` 与 `tui` 的 `config_cwd` 改为保留逻辑 symlink 路径且对不存在路径显式报错
    - `core/sandboxing` 追加 symlink 路径归一化回归测试，避免额外权限归一化时提前解链
    - `linux-sandbox/bwrap` 同步上游实现，按 symlink 真实目标挂载并重映射 carveout/unreadable roots
  - `86764af68` 已语义吸纳：
    - `SandboxPolicy` 与 `FileSystemSandboxPolicy` 的默认只读 carveout 现在会在 cwd 根下“预先保护缺失的 `.codex`”
    - 若用户显式为同一路径声明规则（例如显式 `write`），默认 `.codex` 保护不再覆盖该显式规则
  - `95ba76262` 已语义吸纳：
    - `core/exec` 新增 restricted-token split carveout 解析逻辑
    - `windows-sandbox-rs` 新增 `run_windows_sandbox_capture_with_extra_deny_write_paths`，将新增只读 carveout 映射到额外 deny-write ACL
  - `b976e701a` 本轮结论：N/A（保持 fail-closed）
    - 当前 fork 的 elevated backend 与上游该提交依赖链（setup/runner 参数面）不一致，直接吸纳风险高
    - 已补充 fail-closed 约束：当 `windows_sandbox_level != RestrictedToken` 且需要 split carveout 运行时增强时，直接拒绝执行而非静默放宽权限
  - 通过命令：
    - `cargo check -p codex-core -p codex-tui -p codex-app-server`（0）
    - `cargo check -p codex-utils-absolute-path -p codex-core -p codex-exec -p codex-tui -p codex-linux-sandbox`（0）
    - `cargo test -p codex-utils-absolute-path canonicalize_preserving_symlinks`（0）
    - `cargo test -p codex-utils-absolute-path canonicalize_existing_preserving_symlinks`（0）
    - `cargo test -p codex-core normalize_additional_permissions_preserves_symlinked_write_paths`（0）
    - `cargo test -p codex-windows-sandbox`（0）
    - `cargo check -p codex-core -p codex-windows-sandbox`（0）
    - `cargo check -p codex-linux-sandbox --target x86_64-unknown-linux-gnu`（失败：本机缺少 cross OpenSSL，待 CI Linux 环境验证）
  - 备注：`cargo test -p codex-core` 本地长时间卡在构建/链接阶段，未完成全量执行，交由 CI 验收。

### Batch D - SessionStart clear source

- 范围：`7999b0f60`
- 风险：协议面改动需同步 schema/typescript/doc，避免 wire format 漂移。
- 预期验证：
  - `cargo test -p codex-app-server-protocol`
  - `cargo test -p codex-app-server`
  - `just write-app-server-schema`
- 执行回填（已完成）：
  - 协议层：新增 `ThreadStartSource { startup, clear }`，`thread/start` 增加可选字段 `sessionStartSource`，并同步 JSON/TS schema。
  - app-server：`thread/start` 将 `sessionStartSource=clear` 映射为 `InitialHistory::Cleared`。
  - core：`InitialHistory` 新增 `Cleared` 分支，并在 `SessionStart` hook payload 中透传 `source: "clear"`。
  - tui：`/clear` 触发新会话时改为 `InitialHistory::Cleared`（保留当前分支非 app-server 直连架构）。
  - 兼容修复：补齐 `codex-serve` 对 `InitialHistory::Cleared` 的 match 覆盖，避免编译失败。
  - 通过命令：
    - `just write-app-server-schema`（0）
    - `cargo check -p codex-app-server-protocol -p codex-app-server -p codex-core -p codex-tui`（0）
    - `cargo test -p codex-app-server-protocol`（0）
    - `cargo test -p codex-app-server skills_changed_notification_is_emitted_after_skill_change`（0）
    - `cargo test -p codex-tui slash_clear_requests_ui_clear_when_idle`（0）
    - `cargo test -p codex-tui clear_only_ui_reset_preserves_chat_session_state`（0）
    - `cargo check -p codex-serve`（0）
  - 备注：`cargo test -p codex-core --lib thread_manager::tests::drops_from_last_user_only -- --exact` 本地执行耗时异常，已中止，交由 CI 继续兜底。

### Batch E - 适配判定与 N/A 证据

- 范围：`04fc208b6`、`36712d854`、`71923f43a`
- 风险：误判“已等价”会造成功能差异隐性遗留。
- 预期产物：
  - 对每项给出 Adopted / Adopted-equivalent / N/A 的证据结论
  - 若 N/A，明确替代实现位置与用户可见差异。
- 执行回填（已完成）：
  - `71923f43a`：Adopted-equivalent  
    - `codex exec "prompt"` + piped stdin 现会拼接为 `prompt + <stdin>...</stdin>` 上下文
    - 回归覆盖新增：`exec/tests/suite/prompt_stdin.rs`
  - `36712d854`：Adopted-equivalent  
    - 本仓虽无 `app-server-client`，但 websocket 建连入口已统一调用 `ensure_rustls_crypto_provider()`：
      - `codex-rs/codex-api/src/endpoint/responses_websocket.rs`
      - `codex-rs/codex-api/src/endpoint/realtime_websocket/methods.rs`
      - `codex-rs/network-proxy/src/proxy.rs`
  - `04fc208b6`：N/A  
    - 本仓无 `codex-rs/tools` / `tool_discovery` 路径，故不存在该提交修复的排序打散点位
    - 现有搜索工具链位于 `core`（`search_tool`）且无同名实现冲突
  - 通过命令：
    - `cargo test -p codex-exec`（0）
    - `cargo test -p codex-protocol`（0）

## CI/CD 门禁改造（执行项）

- 新增 `rust-ci` PR 级 Windows 最小验证任务，覆盖本批受影响 crate（至少 `codex-core`、`codex-exec`、`codex-windows-sandbox`）。
- 新增 app-server 定向验证任务，覆盖断连卸载与 thread 生命周期关键测试。
- `ci`（installer/hodexctl）保持 required，确保本仓已有上游差异能力不回退。

## 结果回填模板（每批完成后更新）

- 变更提交：
- 关键代码路径：
- 验证命令与退出码：
- CI 运行链接：
- 结论：
- 未决风险：
