# Deferred Upstream Alignment Rewrite Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不破坏 fork 现有 `thread/app-server/plugin/MCP/安全策略` 主链的前提下，把当前仍处于“暂缓 / 不吸纳”的上游高耦合能力改为可桥接、可分批落地的对齐实现。

**Architecture:** 先冻结共享边界，再在状态面、控制面和数据面分别引入兼容适配层。所有新增能力必须先挂到适配层后面，默认行为保持当前 fork 口径，只有在兼容链路稳定后才允许切换入口或开放新默认值。

**Tech Stack:** Rust workspace (`codex-rs`), app-server protocol v2, TUI, plugin/marketplace subsystem, MCP runtime, Windows/Linux sandbox policy, GitHub Actions CI.

---

## Control Contract

- **Primary Setpoint**
  - 把 deferred 能力从“提交级同步问题”转成“系统级桥接问题”，并形成不会覆盖 fork 二开主链的实施路径。
- **Acceptance**
  - 现有 `thread/list`、`thread/read`、`plugin/list`、`plugin/install`、审批与沙箱默认行为保持兼容。
  - 新增能力以可选字段、可选入口、兼容 facade 或 feature gate 方式进入。
  - CI 继续保持当前 required workflows 全绿。
- **Guardrails**
  - 不放宽现有网络审批、Windows sandbox、图像能力默认策略。
  - 不直接替换现有线程持久化事实源。
  - 不在本轮把 fork 自研插件管理器替换成原版 `core-plugins` 结构。
- **Sampling Plan**
  - 每个工作流独立 PR 或原子提交。
  - 每次仅扩一个共享边界：协议、状态事实源、插件/MCP facade、策略 profile 四类不能混做。
- **Rollback Trigger**
  - 任一改动要求重写现有 RPC wire shape。
  - 任一改动需要把现有 manager/store 直接换成另一套事实源。
  - 任一改动放宽默认权限或跳过现有审批。
- **Constraints**
  - 用户验证口径以 CI/CD 为准，不在本机跑完整功能验证。
  - 文档、schema、协议说明必须与实现同步。
- **Boundary**
  - 允许新增 facade / repository / runtime adapter。
  - 不允许直接重构整个 `codex_message_processor` 或 `PluginsManager` 主链。

## 当前代码事实

- 线程相关能力已经存在，不是空白基线：
  - [common.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/app-server-protocol/src/protocol/common.rs#L261)
  - [v2.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/app-server-protocol/src/protocol/v2.rs#L3068)
  - [README.md](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/app-server/README.md#L131)
- 插件与 marketplace 已有自研主链：
  - [codex_message_processor.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/app-server/src/codex_message_processor.rs#L5252)
  - [manager.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/core/src/plugins/manager.rs)
  - [store.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/core/src/plugins/store.rs#L68)
- 图像 detail、image generation、network approval 已进入显式特性/审批模型：
  - [features.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/core/src/features.rs#L124)
  - [view_image.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/core/src/tools/handlers/view_image.rs#L88)
  - [network_proxy_spec.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/core/src/config/network_proxy_spec.rs#L350)
- Windows sandbox 已有 setup/orchestration，但没有上游那套 elevated split carveout 拆分：
  - [setup_orchestrator.rs](/Volumes/Work/code/stellarlinkco-codex-src/codex-rs/windows-sandbox-rs/src/setup_orchestrator.rs#L534)

## 复杂性转移账本

| 主题 | 复杂性原位置 | 新位置 | 收益 | 新成本 | 失效模式 |
| --- | --- | --- | --- | --- | --- |
| ThreadStore 对齐 | app-server 直接操作现有线程事实源 | `ThreadRepository` facade | 新增 API 能力不再直接侵入主链 | 需要维护双层抽象 | facade 与底层状态语义漂移 |
| Provider runtime 对齐 | `model_provider_id` 散落分支 | `ProviderRuntime` facade | provider 差异集中治理 | 过渡期双入口 | 某些 provider 仍绕过 facade |
| Plugin/MCP 对齐 | `PluginsManager` / `PluginStore` 与启动链直接耦合 | facade + launcher/runtime adapter | 可逐步吸纳原版 marketplace/MCP 能力 | 适配层增加 | 新入口与旧入口状态不同步 |
| 策略项对齐 | 默认配置直接控制行为 | 显式 profile / feature gate | 可提供原版能力而不改默认值 | 配置矩阵增多 | profile 组合导致行为不清晰 |

## 文件结构与职责

### 状态面

- Create: `codex-rs/app-server/src/thread_repository.rs`
  - 定义 app-server 可依赖的线程查询/读取/归档 facade。
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
  - 把 `thread/list`、`thread/read`、archive/unarchive 入口改为通过 facade 调用。
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
  - 保持 RPC method 入口定义。
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - 增加可选分页/turn-list 参数与响应类型。
- Modify: `codex-rs/app-server/README.md`
  - 更新线程 API 说明。
- Test: `codex-rs/app-server/tests/suite/v2/thread_list.rs`
- Test: `codex-rs/app-server/tests/suite/v2/thread_read.rs`
- Test: `codex-rs/app-server/tests/suite/v2/thread_archive.rs`

### 控制面

- Create: `codex-rs/core/src/provider_runtime.rs`
  - 定义 provider 行为抽象，不替换现有 provider 配置。
- Modify: `codex-rs/core/src/codex.rs`
  - 通过 facade 获取 provider 差异能力。
- Modify: `codex-rs/core/src/compact_remote.rs`
  - 把 remote compaction provider 分支收口。
- Modify: `codex-rs/codex-api/src/provider.rs`
  - 统一 provider 类型检测与能力标识。
- Test: `codex-rs/core/tests/suite/client.rs`
- Test: `codex-rs/core/tests/suite/remote_models.rs`

### 插件与 MCP

- Create: `codex-rs/core/src/plugins/facade.rs`
  - 提供 marketplace add/remove/list/install/uninstall 的稳定入口。
- Create: `codex-rs/core/src/mcp/launcher.rs`
  - 抽出 stdio launcher 兼容层。
- Modify: `codex-rs/core/src/plugins/manager.rs`
  - 下沉为 facade 的内部实现，而不是直接暴露给 UI / app-server。
- Modify: `codex-rs/core/src/plugins/store.rs`
  - 保持缓存事实源不变，仅补 facade 所需能力。
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
  - 改为通过 facade 暴露 plugin 管理动作。
- Modify: `codex-rs/cli/src/main.rs`
  - 新入口只作为 facade 包装层，不直接动 store。
- Modify: `codex-rs/tui/src/app.rs`
  - `/plugins` 新 UI 只能消费 facade，不直接触底层 manager。
- Test: `codex-rs/app-server/tests/suite/v2/plugin_list.rs`
- Test: `codex-rs/app-server/tests/suite/v2/plugin_install.rs`
- Test: `codex-rs/app-server/tests/suite/v2/plugin_uninstall.rs`

### 策略层

- Create: `codex-rs/core/src/policy_profiles.rs`
  - 定义原版能力对齐 profile，但默认关闭。
- Modify: `codex-rs/core/src/features.rs`
  - 将 image/detail/image-generation 能力映射到 profile。
- Modify: `codex-rs/core/src/config/network_proxy_spec.rs`
  - 保持当前 managed baseline，不允许 YOLO 默认放宽。
- Modify: `codex-rs/windows-sandbox-rs/src/setup_orchestrator.rs`
  - 仅为 elevated carveout 预留 profile 钩子，不改变当前 fail-closed。
- Modify: `codex-rs/core/src/tools/handlers/view_image.rs`
  - 由 capability/profile 共同决定 `detail: "original"` 暴露。
- Test: `codex-rs/core/tests/suite/approvals.rs`
- Test: `codex-rs/core/tests/suite/view_image.rs`
- Test: `codex-rs/app-server/tests/suite/v2/windows_sandbox_setup.rs`

## 实施任务

### Task 1: 冻结共享边界并引入 ThreadRepository facade

**Files:**
- Create: `codex-rs/app-server/src/thread_repository.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/README.md`
- Test: `codex-rs/app-server/tests/suite/v2/thread_list.rs`
- Test: `codex-rs/app-server/tests/suite/v2/thread_read.rs`

- [x] **Step 1: 定义线程 facade 契约**

```rust
#[async_trait]
pub trait ThreadRepository: Send + Sync {
    async fn list(&self, params: ThreadListQuery) -> Result<ThreadListPage>;
    async fn read(&self, thread_id: &str, include_turns: bool) -> Result<StoredThread>;
    async fn archive(&self, thread_id: &str) -> Result<()>;
    async fn unarchive(&self, thread_id: &str) -> Result<()>;
}
```

- [x] **Step 2: 用现有 app-server 线程实现填充 facade**

```rust
pub struct LocalThreadRepository {
    // 包装当前 codex_message_processor 依赖的线程事实源
}
```

- [x] **Step 3: 把 `thread/list` 与 `thread/read` 入口改成只依赖 facade**

```rust
let page = self.thread_repository.list(query).await?;
self.send_response(request_id, ThreadListResponse { data: page.data, next_cursor: page.next_cursor }).await?;
```

- [ ] **Step 4: 在 CI 中验证线程 API 不回归**

Run in CI:
- `cargo test -p codex-app-server thread_list`
- `cargo test -p codex-app-server thread_read`

Expected:
- 现有 `thread/list` / `thread/read` 测试全部通过。
- 当前客户端行为无 wire breakage。

- [ ] **Step 5: 提交**

```bash
git add codex-rs/app-server/src/thread_repository.rs \
  codex-rs/app-server/src/codex_message_processor.rs \
  codex-rs/app-server/README.md \
  codex-rs/app-server/tests/suite/v2/thread_list.rs \
  codex-rs/app-server/tests/suite/v2/thread_read.rs
git commit -m "refactor: add thread repository facade"
```

### Task 2: 以增量协议方式补 thread turns pagination 能力

**Files:**
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server/README.md`
- Modify: `codex-rs/app-server/tests/common/mcp_process.rs`
- Test: `codex-rs/app-server/tests/suite/v2/thread_turns_list.rs`

- [x] **Step 1: 新增可选协议结构，不破坏现有请求**

```rust
pub struct ThreadTurnsListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub backwards_cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}
```

- [x] **Step 2: 保持 `thread/list` 线协议不变，把反向分页能力收敛到独立的 `thread/turns/list` 响应字段**

```rust
pub struct ThreadTurnsListResponse {
    pub data: Vec<Turn>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}
```

- [x] **Step 3: 先由 ThreadRepository 提供兼容实现，再挂到 RPC method**

```rust
ThreadTurnsList => "thread/turns/list" {
    params: v2::ThreadTurnsListParams,
    response: v2::ThreadTurnsListResponse,
},
```

- [ ] **Step 4: 更新 schema 和 CI**

Run in CI:
- `just write-app-server-schema`
- `cargo test -p codex-app-server-protocol`
- `cargo test -p codex-app-server thread_list`
- `cargo test -p codex-app-server thread_turns_list`

Expected:
- schema 漂移受控。
- 新增 `thread/turns/list` 不改变既有 `thread/list` / `thread/read` wire shape。

- [ ] **Step 5: 提交**

```bash
git add codex-rs/app-server-protocol/src/protocol/common.rs \
  codex-rs/app-server-protocol/src/protocol/v2.rs \
  codex-rs/app-server/README.md
git commit -m "feat: add incremental thread turns pagination api"
```

### Task 3: 引入 ProviderRuntime facade，集中 provider 差异

**Files:**
- Create: `codex-rs/core/src/provider_runtime.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/compact_remote.rs`
- Modify: `codex-rs/codex-api/src/provider.rs`
- Test: `codex-rs/core/tests/suite/client.rs`
- Test: `codex-rs/core/tests/suite/remote_models.rs`

- [ ] **Step 1: 定义 provider 行为抽象**

```rust
pub trait ProviderRuntime {
    fn provider_id(&self) -> &str;
    fn supports_remote_compaction(&self) -> bool;
    fn supports_image_detail_original(&self) -> bool;
}
```

- [ ] **Step 2: 给现有 `model_provider_id` 建默认 facade 适配**

```rust
pub fn runtime_for_provider(provider_id: &str) -> Box<dyn ProviderRuntime> {
    match provider_id {
        "openai" => Box::new(OpenAiRuntime::default()),
        "azure" => Box::new(AzureRuntime::default()),
        other => Box::new(DefaultRuntime::new(other)),
    }
}
```

- [ ] **Step 3: 把 compaction / image detail 判断迁到 facade**

```rust
let runtime = runtime_for_provider(config.model_provider_id.as_str());
if runtime.supports_remote_compaction() {
    run_remote_compact_task(...).await?;
}
```

- [ ] **Step 4: 在 CI 中验证 provider 行为不回归**

Run in CI:
- `cargo test -p codex-core client`
- `cargo test -p codex-core remote_models`

Expected:
- 现有 provider 切换能力保持不变。
- 新 facade 只收口分支，不改变默认 provider 选择。

- [ ] **Step 5: 提交**

```bash
git add codex-rs/core/src/provider_runtime.rs \
  codex-rs/core/src/codex.rs \
  codex-rs/core/src/compact_remote.rs \
  codex-rs/codex-api/src/provider.rs
git commit -m "refactor: add provider runtime facade"
```

### Task 4: 为 plugin / marketplace / MCP stdio 建 facade 与 launcher seam

**Files:**
- Create: `codex-rs/core/src/plugins/facade.rs`
- Create: `codex-rs/core/src/mcp/launcher.rs`
- Modify: `codex-rs/core/src/plugins/manager.rs`
- Modify: `codex-rs/core/src/plugins/store.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/cli/src/main.rs`
- Modify: `codex-rs/tui/src/app.rs`
- Test: `codex-rs/app-server/tests/suite/v2/plugin_list.rs`
- Test: `codex-rs/app-server/tests/suite/v2/plugin_install.rs`
- Test: `codex-rs/app-server/tests/suite/v2/plugin_uninstall.rs`

- [ ] **Step 1: 把当前 manager/store 包到稳定 facade**

```rust
pub trait PluginFacade {
    fn list_marketplaces(&self, cwds: &[AbsolutePathBuf]) -> Result<Vec<PluginMarketplaceEntry>>;
    fn install(&self, params: PluginInstallParams) -> Result<PluginInstallResponse>;
    fn uninstall(&self, plugin_id: &PluginId) -> Result<()>;
}
```

- [ ] **Step 2: 为 MCP stdio 抽出 launcher 接口，不切默认 runtime**

```rust
pub trait McpLauncher {
    async fn spawn_stdio(&self, spec: &McpServerSpec) -> Result<LaunchedMcpServer>;
}
```

- [ ] **Step 3: 让 CLI/TUI/app-server 只依赖 facade，不直接触 store/manager**

```rust
let marketplaces = self.plugin_facade.list_marketplaces(&roots)?;
```

- [ ] **Step 4: 在 facade 稳定后再补 `plugin remove` / 新入口**

```rust
PluginRemove => "plugin/remove" {
    params: v2::PluginRemoveParams,
    response: v2::PluginRemoveResponse,
},
```

- [ ] **Step 5: 在 CI 中验证 plugin 管理链路**

Run in CI:
- `cargo test -p codex-app-server plugin_list`
- `cargo test -p codex-app-server plugin_install`
- `cargo test -p codex-app-server plugin_uninstall`

Expected:
- marketplace 发现、安装、卸载保持兼容。
- 新 facade 不改变插件缓存布局。

- [ ] **Step 6: 提交**

```bash
git add codex-rs/core/src/plugins/facade.rs \
  codex-rs/core/src/mcp/launcher.rs \
  codex-rs/core/src/plugins/manager.rs \
  codex-rs/core/src/plugins/store.rs \
  codex-rs/app-server/src/codex_message_processor.rs \
  codex-rs/cli/src/main.rs \
  codex-rs/tui/src/app.rs
git commit -m "refactor: add plugin and mcp facades"
```

### Task 5: 用策略 profile 对齐原版能力，但保持本仓默认值

**Files:**
- Create: `codex-rs/core/src/policy_profiles.rs`
- Modify: `codex-rs/core/src/features.rs`
- Modify: `codex-rs/core/src/config/network_proxy_spec.rs`
- Modify: `codex-rs/core/src/tools/handlers/view_image.rs`
- Modify: `codex-rs/windows-sandbox-rs/src/setup_orchestrator.rs`
- Test: `codex-rs/core/tests/suite/approvals.rs`
- Test: `codex-rs/core/tests/suite/view_image.rs`
- Test: `codex-rs/app-server/tests/suite/v2/windows_sandbox_setup.rs`

- [ ] **Step 1: 定义只增不替换的策略 profile**

```rust
pub enum PolicyProfile {
    CurrentForkBaseline,
    UpstreamCompatibleOptional,
}
```

- [ ] **Step 2: 把 image/detail/image-generation 绑定到 profile + capability**

```rust
let allow_original_detail =
    profile.allows_original_image_detail() && turn.model_info.supports_image_detail_original;
```

- [ ] **Step 3: 明确禁止 YOLO 默认绕过 managed-network enforcement**

```rust
if profile == PolicyProfile::CurrentForkBaseline {
    return managed_network_baseline();
}
```

- [ ] **Step 4: Windows elevated carveout 仅预留 profile 钩子，不改变 fail-closed**

```rust
if profile.allows_elevated_split_carveout() {
    return Err(anyhow!("not enabled on this fork baseline"));
}
```

- [ ] **Step 5: 在 CI 中验证策略未放宽**

Run in CI:
- `cargo test -p codex-core approvals`
- `cargo test -p codex-core view_image`
- `cargo test -p codex-app-server windows_sandbox_setup`

Expected:
- 当前默认 profile 行为与现状一致。
- 兼容 profile 只暴露能力，不自动成为默认。

- [ ] **Step 6: 提交**

```bash
git add codex-rs/core/src/policy_profiles.rs \
  codex-rs/core/src/features.rs \
  codex-rs/core/src/config/network_proxy_spec.rs \
  codex-rs/core/src/tools/handlers/view_image.rs \
  codex-rs/windows-sandbox-rs/src/setup_orchestrator.rs
git commit -m "feat: add optional upstream compatibility policy profiles"
```

## CI Gate

- 状态面 gate
  - `codex-app-server`
  - `codex-app-server-protocol`
- 控制面 gate
  - `codex-core`
  - `codex-api`
- 插件/MCP gate
  - `codex-app-server`
  - `codex-core`
- 策略层 gate
  - Linux sandbox job
  - Windows sandbox job
  - app-server protocol schema job

## 停止条件

- 出现需要改写现有 `ClientRequest` / `ServerNotification` 非兼容字段结构的设计。
- 出现需要直接替换 `PluginStore` 缓存布局的设计。
- 出现要求默认开启 image generation / high detail / YOLO 放宽网络策略的设计。
- 出现必须直接引入 remote thread store 作为唯一事实源的设计。

## 自审

- 已覆盖的需求：
  - thread-store / archive-unarchive / turns pagination
  - provider runtime abstraction
  - plugin / marketplace / `/plugins` / plugin remove
  - MCP stdio launcher / executor-backed 链路前置重构
  - Windows carveout / managed-network / image default 策略对齐
- 本计划刻意不做的事：
  - 不直接替换 fork 主链
  - 不改变默认安全口径
  - 不把 UI 改造放在 facade 之前
