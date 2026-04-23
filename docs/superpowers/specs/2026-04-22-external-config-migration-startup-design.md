# External Config Migration Startup Design

**目标**

在不引入上游 `AppServerSession` / trust-nux 启动链路的前提下，为当前分支补齐 TUI 启动阶段的 external agent config migration prompt，尽量贴近上游语义，同时不影响现有 fork 的 TUI、core、app-server 主链。

**现状**

- 当前仓已经具备 external agent config 的检测与导入能力：
  - `codex-rs/core/src/external_agent_config.rs`
  - `codex-rs/app-server/src/external_agent_config_api.rs`
- 当前仓缺失的能力是：
  - `[notice]` 下 external config migration prompt 的隐藏态与冷却时间持久化
  - TUI 启动阶段的检测、提示、跳过、永久跳过
  - 导入后的配置重载与启动消息
- 当前仓没有上游对应的 `Feature::ExternalMigration`、`AppServerSession`、`entered_trust_nux` 启动门禁，因此不能机械移植上游实现。

**方案**

采用“语义吸纳 + 架构适配”的最小方案：

1. 在 `codex-rs/core/src/config/types.rs` 为 `Notice` 增加 `external_config_migration_prompts` 嵌套状态：
   - `home: Option<bool>`
   - `home_last_prompted_at: Option<i64>`
   - `projects: BTreeMap<String, bool>`
   - `project_last_prompted_at: BTreeMap<String, i64>`
2. 在 `codex-rs/core/src/config/edit.rs` 增加对应 `ConfigEdit` 与 `ConfigEditsBuilder` 写入能力，用于：
   - 记录 home / project 级别“永久不再提示”
   - 记录 home / project 级别 `last_prompted_at`
3. 在 TUI 新增轻量 startup 模块，直接复用 `ExternalAgentConfigService`，不引入新的 app-server 调用层。
4. 在 `App::run` 启动链路中、`ChatWidget` 初始化前接入该模块：
   - 仅在 `SessionSelection::StartFresh | SessionSelection::Exit` 时检测
   - 过滤已隐藏 scope 和 5 天冷却期中的 scope
   - 若存在可见迁移项，则展示简化 prompt
5. 简化 prompt 只提供 4 个结果：
   - `Import`：导入当前可见项
   - `Skip`：本次跳过，不修改隐藏状态
   - `SkipForever`：将当前可见 scope 写入隐藏状态
   - `Exit`：退出启动流程
6. 导入成功后重载 `Config`，并返回一条 startup success message 给现有 TUI 历史流。

**明确不做**

- 不移植上游 1000+ 行的复杂 migration UI
- 不引入 item 级别多选
- 不新增 feature flag
- 不改动 app-server 既有 detect/import API

**触发策略**

由于当前分支没有上游 trust-nux 门禁，本次采用更保守的映射：

- 只在 `SessionSelection::StartFresh | SessionSelection::Exit` 启动路径检测
- 不在 `Resume` / `Fork` 路径提示
- 依赖 `skip forever` 与 5 天冷却减少重复打扰

**错误处理**

- detect 失败：记录 warning，继续启动，不阻断 TUI
- persist shown/dismissal 失败：记录 warning，但不阻断当前 prompt
- import 失败：在 prompt 内显示错误，允许用户重试或改选
- reload config 失败：返回错误并终止本次启动，保持失败显式可见

**测试**

- `core/config/edit.rs`
  - 新增 notice nested table 写入测试
  - 覆盖 home/project hide 与 last_prompted_at
- `tui`
  - 覆盖 visible item 过滤逻辑
  - 覆盖 cooldown 边界
  - 覆盖 success message 生成逻辑

**影响范围**

- `codex-rs/core/src/config/types.rs`
- `codex-rs/core/src/config/edit.rs`
- `codex-rs/tui/src/app.rs`
- `codex-rs/tui/src/external_agent_config_migration.rs`
- `codex-rs/tui/src/external_agent_config_migration_startup.rs`
