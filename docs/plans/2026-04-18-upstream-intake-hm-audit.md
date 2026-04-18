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
| `8d5889929` | 高 | app-server 连接断开后，最后订阅者线程必须卸载，避免状态泄漏 | `codex-rs/app-server/src/codex_message_processor.rs` + `thread_state.rs` + websocket suite | 进行中 | 待定 |
| `0bdeab330` | 高 | 已提交 slash 命令可在本地输入历史中回忆 | `codex-rs/tui/src/bottom_pane/chat_composer.rs` | 未开始 | 待定 |
| `0393a485e` | 高 | Composer 支持反向历史搜索 | `codex-rs/tui/src/bottom_pane/chat_composer.rs` + `chat_composer_history.rs` + `footer.rs` | 未开始 | 待定 |
| `e9e7ef3d3` | 高 | Windows verbatim/non-verbatim 路径比较一致化，修复 cwd 过滤误判 | `codex-rs/core/src/path_utils.rs` + `app-server`/`tui`/`exec` 调用点 | 未开始 | 待定 |
| `04fc208b6` | 高 | 保留 tool_search_output 原始顺序，不被分组排序打散 | 本仓无同名 `tools` 模块，映射到连接器/工具发现链路评估 | 未开始 | 待定 |
| `36712d854` | 高 | remote websocket client 建连前安装 rustls provider | 本仓无 `app-server-client` crate，需评估等价入口 | 未开始 | 待定 |
| `b11478149` | 中 | sandbox writable roots 与 symlink 路径处理一致化 | `core/path_utils` + `protocol/permissions` + `linux-sandbox` | 未开始 | 待定 |
| `b976e701a` | 中 | Windows elevated sandbox 支持 split carveouts | `windows-sandbox-rs` + `core/sandboxing` + `core/exec` | 未开始 | 待定 |
| `95ba76262` | 中 | Windows restricted-token sandbox 支持 split carveouts | `windows-sandbox-rs` + `core/sandboxing` + `core/exec` | 未开始 | 待定 |
| `86764af68` | 中 | Linux/macOS sandbox 下首次 `.codex` 创建稳定性 | `core/codex_thread` + `sandboxing` + 相关测试 | 未开始 | 待定 |
| `71923f43a` | 中 | `codex exec` stdin piping 行为增强 | 现仓 `exec` 已含 stdin 路径，做差异复核 | 未开始 | 待定 |
| `ae057e0bb` | 中 | 活跃 TUI 会话里 `/status` 速率限制展示不陈旧 | `tui/src/app.rs` + `chatwidget.rs` + `status/*` | 未开始 | 待定 |
| `7999b0f60` | 中 | clear 场景下 SessionStart source 可区分为 clear | `protocol` + `core` + `app-server-protocol` + `app-server` + `tui` | 未开始 | 待定 |

## 批次实施记录

### Batch A - app-server 断连卸载

- 范围：`8d5889929`
- 风险：thread teardown 与 `thread_state_manager` 状态机耦合，需避免双重卸载或误卸载。
- 预期验证：
  - `cargo test -p codex-app-server connection_handling_websocket`
  - `cargo test -p codex-app-server thread_unsubscribe`

### Batch B - TUI 高优先级交互

- 范围：`0bdeab330`、`0393a485e`、`ae057e0bb`
- 风险：输入状态机复杂，需保证 popup/history/paste burst 不回归。
- 预期验证：
  - `cargo test -p codex-tui`
  - 必要时更新 `insta` 快照并显式记录。

### Batch C - 路径与沙箱

- 范围：`e9e7ef3d3`、`86764af68`、`95ba76262`、`b976e701a`、`b11478149`
- 风险：跨平台路径归一化和权限边界可能影响安全行为。
- 预期验证：
  - `cargo test -p codex-core`
  - `cargo test -p codex-exec`
  - `cargo test -p codex-windows-sandbox`（CI Windows 任务）

### Batch D - SessionStart clear source

- 范围：`7999b0f60`
- 风险：协议面改动需同步 schema/typescript/doc，避免 wire format 漂移。
- 预期验证：
  - `cargo test -p codex-app-server-protocol`
  - `cargo test -p codex-app-server`
  - `just write-app-server-schema`

### Batch E - 适配判定与 N/A 证据

- 范围：`04fc208b6`、`36712d854`、`71923f43a`
- 风险：误判“已等价”会造成功能差异隐性遗留。
- 预期产物：
  - 对每项给出 Adopted / Adopted-equivalent / N/A 的证据结论
  - 若 N/A，明确替代实现位置与用户可见差异。

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
