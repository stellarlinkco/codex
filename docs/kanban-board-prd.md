# Product Requirements Document: Kanban Board (codex serve)

> 注意（2026-03-13）：当前仓库新增了 GitHub 驱动的看板重设想，见 `docs/kanban-board-prd-github.md`。本文件仍描述“以 codex sessions 为卡片”的看板方案，因此会包含 `thinking/active/idle` 等会话运行态。

**Version**: 1.0
**Date**: 2026-03-12
**Author**: Sarah (Product Owner)
**Quality Score**: 92/100
**Status**: Final

---

## Executive Summary

在现有 `codex serve` Web UI 基础上新增看板视图，将 codex 会话（session/thread）以看板卡片形式可视化管理。用户可以通过拖拽卡片在自定义列之间移动，直观地跟踪多个编码任务的进度。该功能完全复用 `codex serve` 已有的后端基础设施（HTTP API、SSE 实时推送、会话生命周期），前端在 React Web UI 中新增看板路由页面。

核心价值：当用户同时运行多个 codex 会话（如多个 bug 修复、多个 feature 开发）时，现有的会话列表视图缺乏进度跟踪和优先级管理能力。看板视图将会话组织为可视化工作流，提升多任务并行场景下的管理效率。

---

## Problem Statement

**Current Situation**: `codex serve` Web UI 当前以线性列表展示所有会话，用户只能按时间排序查看。当会话数量增多（>5 个并行任务），用户无法快速区分哪些任务在进行中、哪些等待审批、哪些已完成。会话的 active/inactive/archived 三态过于粗糙，无法表达真实的工作流阶段。

**Proposed Outcome**: 用户打开 Web UI 后可以切换到看板视图，看到所有会话按工作阶段分布在不同列中。支持拖拽移动卡片、自定义列名、按优先级排序。看板状态通过 SSE 实时同步，多浏览器标签页之间保持一致。

**Why Now**: `codex serve` 的核心基础设施（会话管理、SSE、Web UI）已经就绪（server.rs 3400+ 行，完整的 REST API），看板是在此基础上投入产出比最高的增量功能。

---

## Goals

- G1: 在 Web UI 中提供看板视图，与现有会话列表视图并存，用户可自由切换
- G2: 看板列（columns）可自定义名称和顺序，默认提供 Backlog / In Progress / Review / Done 四列
- G3: 会话卡片支持拖拽在列之间移动，列内支持拖拽排序
- G4: 看板状态通过 SSE 实时同步到所有连接的客户端
- G5: 看板配置和卡片位置持久化，服务重启后保留

## Non-Goals

- NG1: 不实现跨项目/跨工作目录的全局看板（每个 `codex serve` 实例一个看板）
- NG2: 不实现看板模板市场或预设工作流模板
- NG3: 不实现卡片依赖关系（前置/后置任务）或甘特图
- NG4: 不实现多用户协作（`codex serve` 本身是单用户本地服务）
- NG5: 不修改现有会话生命周期（active/inactive/archived），看板列是独立的元数据层
- NG6: 不实现 WIP（Work-In-Progress）限制自动执行

---

## Confirmed Facts, Assumptions, and Open Questions

### Confirmed Facts

- `codex serve` 已有完整的会话 CRUD API（GET/POST/PATCH/DELETE /api/sessions）
- SSE 事件流已实现 session-added / session-updated / session-removed / message-received 事件
- 前端已是 React + Tailwind SPA，通过 `rust-embed` 嵌入二进制
- `AppState` 中 `sessions: Arc<RwLock<HashMap<String, Arc<ActiveSession>>>>` 管理所有活跃会话
- `SessionState` 已包含 name, cwd, model, active, thinking, permission_mode 等字段
- 后端使用 `broadcast::Sender<SyncEvent>` 广播事件到所有 SSE 客户端

### Working Assumptions

- A1: 看板元数据（列定义、卡片位置）存储在本地文件 `~/.codex/kanban.json` 中，无需引入 SQLite 依赖（保持 KISS，当前 serve crate 未使用 SQLite）
- A2: 每个会话自动出现在看板中（创建会话 = 创建卡片），用户可以将卡片移动到任意列
- A3: 前端拖拽使用 `@dnd-kit/core`（React 生态轻量级拖拽库，无重依赖）
- A4: 看板视图和列表视图共享同一套会话数据，看板只增加 column_id 和 position 两个维度

### Open Questions (Non-Blocking)

- Q1: 是否需要支持为卡片添加自定义标签/颜色？（建议 Phase 2 再考虑）
- Q2: 归档的会话是否在看板中显示？（建议默认隐藏，提供"显示已归档"开关）
- Q3: 前端构建产物是否需要更新 `rust-embed` 嵌入流程？（现有流程应直接适用）

### Build Blockers (Must Resolve Before Build or Verification)

- 无。所有必要基础设施已就绪。

---

## Users and Primary Jobs

### Primary User

- **Role**: 使用 `codex serve` 管理多个并行编码任务的开发者
- **Goal**: 直观查看所有进行中的 codex 会话的工作阶段，快速识别哪些任务需要关注（等待审批、出错、空闲）
- **Pain Point**: 当有 5+ 个会话同时存在时，线性列表无法提供工作流概览，需要逐个点击查看状态

---

## User Stories & Acceptance Criteria

### Story 1: 查看看板视图

**As a** 开发者
**I want to** 在 Web UI 中切换到看板视图
**So that** 我能一眼看到所有会话按工作阶段的分布

**Acceptance Criteria:**

- [ ] Web UI 顶部或侧边栏有"列表/看板"视图切换按钮
- [ ] 看板默认显示 4 列：Backlog / In Progress / Review / Done
- [ ] 每张卡片显示：会话名称、模型名称、工作目录、状态指示器（thinking/active/idle）、最后更新时间
- [ ] 空列显示占位提示"拖拽会话到此列"
- [ ] 视图选择持久化到 localStorage，刷新后保留

### Story 2: 拖拽移动卡片

**As a** 开发者
**I want to** 通过拖拽将会话卡片从一列移动到另一列
**So that** 我能手动更新任务的工作阶段

**Acceptance Criteria:**

- [ ] 卡片可被拖拽到任意列
- [ ] 拖拽过程中有视觉反馈（卡片半透明、目标列高亮）
- [ ] 放下后卡片位置立即更新，并通过 API 持久化
- [ ] 列内卡片支持拖拽排序（上下调整优先级）
- [ ] 移动操作通过 SSE 实时同步到其他标签页

### Story 3: 自定义看板列

**As a** 开发者
**I want to** 添加、删除、重命名看板列
**So that** 我能按自己的工作流组织任务

**Acceptance Criteria:**

- [ ] 看板右侧有"+"按钮添加新列
- [ ] 列标题可双击编辑
- [ ] 列可被删除（需确认，列中卡片自动移到第一列）
- [ ] 列顺序支持拖拽调整
- [ ] 列配置变更持久化到后端

### Story 4: 新会话自动入看板

**As a** 开发者
**I want to** 新创建的会话自动出现在看板的第一列（Backlog）
**So that** 我不需要手动将每个新会话添加到看板

**Acceptance Criteria:**

- [ ] 通过 POST /api/sessions 创建的新会话自动出现在第一列底部
- [ ] SSE session-added 事件触发看板实时更新
- [ ] 删除会话时（DELETE /api/sessions/:id）卡片从看板移除
- [ ] 归档会话时卡片从看板隐藏（可通过开关显示）

### Story 5: 从看板卡片快速操作

**As a** 开发者
**I want to** 从看板卡片上快速执行常用操作
**So that** 我不需要切换到列表视图再操作

**Acceptance Criteria:**

- [ ] 卡片点击跳转到该会话的聊天页面
- [ ] 卡片右键菜单或三点菜单提供：重命名、归档、删除、中止
- [ ] 卡片上显示实时状态徽章（thinking 时有动画，待审批时有警告图标）

---

## Functional Requirements

### Requirement 1: 看板数据模型

- Description: 后端维护看板配置（列定义）和卡片位置（session-to-column 映射）
- 数据结构:

```rust
// 看板持久化结构（~/.codex/kanban.json）
struct KanbanConfig {
    columns: Vec<KanbanColumn>,           // 列定义，有序
    card_positions: HashMap<String, CardPosition>,  // session_id -> position
}

struct KanbanColumn {
    id: String,       // UUID
    name: String,     // 显示名称
    position: u32,    // 列排序
}

struct CardPosition {
    column_id: String,  // 所在列
    position: u32,      // 列内排序
}
```

- Trigger: 服务启动时从文件加载，变更时写回文件
- Expected Result: 看板配置和卡片位置在服务重启后完整恢复

### Requirement 2: 看板 HTTP API

- Description: 新增看板管理 REST API
- 新增路由:

| Method | Path                              | Description                            |
| ------ | --------------------------------- | -------------------------------------- |
| GET    | `/api/kanban`                     | 获取看板配置（列定义 + 全部卡片位置）  |
| PUT    | `/api/kanban/columns`             | 更新列定义（全量替换）                 |
| POST   | `/api/kanban/columns`             | 添加新列                               |
| DELETE | `/api/kanban/columns/{column_id}` | 删除列                                 |
| PATCH  | `/api/kanban/columns/{column_id}` | 重命名列                               |
| PUT    | `/api/kanban/cards/{session_id}`  | 移动卡片（设置 column_id 和 position） |
| PUT    | `/api/kanban/cards/batch`         | 批量更新卡片位置（拖拽排序优化）       |

- Trigger: 前端拖拽操作或列管理操作
- Expected Result: 变更持久化并通过 SSE 广播

### Requirement 3: 看板 SSE 事件

- Description: 扩展现有 SSE 事件流，新增看板相关事件
- 新增事件:

```
event: kanban-updated
data: {"columns": [...], "cardPositions": {...}}

event: card-moved
data: {"sessionId": "<id>", "columnId": "<id>", "position": <n>}
```

- Trigger: 任何看板状态变更（列增删改、卡片移动）
- Expected Result: 所有 SSE 客户端实时接收看板变更
- Notes: 复用现有 `broadcast::Sender<SyncEvent>` 机制，在 `SyncEvent` enum 中新增变体

### Requirement 4: 前端看板视图组件

- Description: React 看板视图页面，包含列容器、可拖拽卡片、列管理
- Trigger: 用户点击"看板"视图切换按钮
- Expected Result: 渲染看板布局，卡片按列分组显示，支持拖拽交互
- Notes: 使用 `@dnd-kit/core` + `@dnd-kit/sortable` 实现拖拽

### Requirement 5: 看板-会话生命周期同步

- Description: 会话创建/删除/归档事件自动反映到看板
- Trigger: 现有 session-added / session-removed SSE 事件
- Expected Result:
  - 新会话自动加入第一列（Backlog）底部
  - 删除会话自动从看板移除卡片
  - 归档会话卡片标记为 archived（可配置是否显示）
- Notes: 在 `session_event_loop` 中 `SessionAdded` 分支添加看板卡片初始化逻辑

---

## Acceptance Matrix

| ID  | Requirement                 | Priority | How to Verify                                                  |
| --- | --------------------------- | -------- | -------------------------------------------------------------- |
| A1  | 看板视图正确渲染 4 个默认列 | P0       | 启动 `codex serve`，打开浏览器，切换到看板视图，验证 4 列显示  |
| A2  | 卡片拖拽移动到其他列        | P0       | 拖拽一张卡片到另一列，刷新页面确认位置持久化                   |
| A3  | SSE 实时同步                | P0       | 打开两个标签页，在一个标签页拖拽卡片，另一个标签页实时看到变化 |
| A4  | 新会话自动入看板            | P0       | 创建新会话，看板第一列自动出现新卡片                           |
| A5  | 列的增删改                  | P1       | 添加/删除/重命名列，刷新后配置保留                             |
| A6  | 卡片点击跳转聊天            | P1       | 点击看板卡片，跳转到对应会话聊天页面                           |
| A7  | 归档会话卡片隐藏            | P1       | 归档一个会话，看板中卡片消失                                   |
| A8  | 列内排序                    | P1       | 在同一列内上下拖拽卡片，刷新后顺序保留                         |
| A9  | 服务重启后看板恢复          | P0       | 停止 `codex serve`，重新启动，看板列和卡片位置与停止前一致     |
| A10 | 卡片状态徽章实时更新        | P2       | 会话进入 thinking 状态时，卡片徽章实时变为"thinking"动画       |

---

## Edge Cases & Failure Handling

- **Case**: `kanban.json` 文件不存在（首次启动）

  - Expected behavior: 自动创建默认 4 列配置，所有现有会话放入第一列

- **Case**: `kanban.json` 中引用了已不存在的 session_id

  - Expected behavior: 加载时清理孤立的 card_positions 条目，不报错

- **Case**: `kanban.json` 文件损坏（JSON 解析失败）

  - Expected behavior: 日志警告，回退到默认配置，不影响服务启动

- **Case**: 删除看板列时该列有卡片

  - Expected behavior: 弹出确认对话框，确认后将卡片移到第一列

- **Case**: 拖拽过程中后端返回错误（如磁盘写入失败）

  - Expected behavior: 前端乐观更新回滚到拖拽前状态，显示错误提示

- **Case**: 多标签页同时拖拽同一张卡片

  - Expected behavior: 最后写入者胜出（last-write-wins），SSE 最终同步所有客户端到一致状态

- **Case**: 会话正在 thinking 状态时被拖拽

  - Expected behavior: 允许拖拽，看板位置是元数据层，不影响会话执行

- **Recovery**: 如果看板数据丢失
  - Expected behavior: 用户可以通过删除 `~/.codex/kanban.json` 触发重建，所有会话重新放入第一列

---

## Technical Constraints & Non-Functional Requirements

### Performance

- 看板 API 响应延迟 < 50ms（本地文件读写）
- 拖拽操作的前端视觉反馈延迟 < 16ms（60fps）
- SSE 事件从后端发出到前端渲染 < 100ms
- 支持同时显示 50+ 张卡片（不做虚拟滚动，看板场景通常卡片数有限）

### Security & Compliance

- 看板 API 复用现有 token auth 中间件（`require_token`）
- `kanban.json` 文件权限与其他 `~/.codex/` 文件一致
- 无敏感数据存储（看板仅存储列名和 session_id 引用）

### Integration & Dependencies

- **codex-core**: 复用 `ThreadManager` 和 `CodexThread` 获取会话状态
- **SSE 事件流**: 扩展现有 `SyncEvent` enum，复用 `broadcast::Sender`
- **前端构建**: 新增 `@dnd-kit/core` 和 `@dnd-kit/sortable` npm 依赖
- **持久化**: 使用 `serde_json` 序列化到 `~/.codex/kanban.json`

### Platform Constraints

- 拖拽交互需要支持 mouse 和 touch 事件（`@dnd-kit` 原生支持）
- 后端文件持久化需要处理 `~/.codex/` 目录不存在的情况（创建目录）
- 嵌入的前端产物体积增量：`@dnd-kit` 约 30KB gzipped，影响可忽略

---

## MVP Scope & Delivery

### Must Have (MVP)

- 看板视图页面（4 个默认列 + 卡片渲染）
- 卡片拖拽移动（跨列 + 列内排序）
- 看板 REST API（GET kanban, PUT cards）
- 看板 SSE 实时同步
- 本地文件持久化（kanban.json）
- 新会话自动入看板
- 删除/归档会话自动从看板移除
- 卡片点击跳转聊天
- 卡片状态徽章（thinking/active/idle/waiting-approval）

### Nice to Have (Later)

- 自定义列增删改（Phase 2）
- 卡片标签/颜色标记（Phase 2）
- 列 WIP 限制提示（Phase 2）
- 看板筛选/搜索（Phase 2）
- 列折叠/展开（Phase 2）
- 看板统计面板（每列卡片数、平均停留时间）（Phase 3）

### Rollout Notes

- 看板视图作为可选视图发布，不替换现有列表视图
- 默认视图仍为列表，用户手动切换后记住选择
- 无数据迁移需求，看板是纯增量功能

---

## Examples and Counterexamples

### Good Outcome Example

用户启动 `codex serve`，创建了 3 个会话分别处理不同的 bug。打开看板视图，3 张卡片都在 Backlog 列。用户将"修复登录问题"拖到 In Progress 列，会话开始执行后卡片显示 thinking 动画。执行完成后卡片变为 idle 状态，用户将其拖到 Review 列。另一个标签页实时看到了这些变化。

### Counterexample

- 错误行为：拖拽卡片到 Done 列会自动归档会话 -- 这是错误的。看板列是纯元数据标记，不应触发会话生命周期变更。用户可能想把会话放在 Done 列但保持其可恢复。
- 错误行为：看板状态存储在浏览器 localStorage -- 这会导致不同浏览器/标签页看到不同的看板状态，违反一致性要求。必须存储在后端。

---

## Risks & Dependencies

| Risk                                     | Probability | Impact | Mitigation                                                     |
| ---------------------------------------- | ----------- | ------ | -------------------------------------------------------------- |
| `@dnd-kit` 与现有前端框架版本不兼容      | Low         | Medium | 开发初期验证依赖兼容性；备选方案 `react-beautiful-dnd`         |
| kanban.json 文件并发写入冲突             | Low         | Low    | 单进程场景无实际并发；使用 `RwLock` 保护内存状态，原子写入文件 |
| 前端产物体积增长影响首屏加载             | Low         | Low    | `@dnd-kit` 约 30KB gzipped；看板视图代码拆分为独立 chunk       |
| 卡片数量过多导致看板渲染性能问题         | Low         | Medium | MVP 阶段不做虚拟化；50+ 卡片场景下观察性能再优化               |
| 会话状态与看板位置语义不一致导致用户困惑 | Medium      | Medium | 明确文档和 UI 提示：看板列是手动标记，不等同于会话系统状态     |

**Dependencies:**

- `codex serve` 后端已就绪（server.rs 完整 API + SSE）
- 前端 React + Tailwind 构建流程已就绪（assets/web/）
- `~/.codex/` 目录约定已存在

---

## Architecture Integration

### 后端变更范围

```
codex-rs/serve/src/
  server.rs          -- 新增 kanban API 路由和处理函数（约 200 行）
  kanban.rs (新文件) -- KanbanConfig 数据模型、文件持久化、内存状态管理（约 150 行）
```

在 `AppState` 中新增:

```rust
kanban: Arc<RwLock<KanbanConfig>>,
```

在 `SyncEvent` enum 中新增:

```rust
#[serde(rename = "kanban-updated")]
KanbanUpdated { data: JsonValue },

#[serde(rename = "card-moved")]
CardMoved {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "columnId")]
    column_id: String,
    position: u32,
},
```

在 `build_router` 中新增路由:

```rust
.route("/kanban", get(handle_get_kanban))
.route("/kanban/columns", put(handle_update_columns).post(handle_add_column))
.route("/kanban/columns/{column_id}", delete(handle_delete_column).patch(handle_rename_column))
.route("/kanban/cards/{session_id}", put(handle_move_card))
.route("/kanban/cards/batch", put(handle_batch_move_cards))
```

### 前端变更范围

```
assets/web/src/
  pages/KanbanView.tsx (新文件)    -- 看板主视图组件
  components/kanban/
    KanbanColumn.tsx (新文件)      -- 列容器组件
    KanbanCard.tsx (新文件)        -- 卡片组件
    KanbanHeader.tsx (新文件)      -- 看板顶栏（视图切换）
  hooks/useKanban.ts (新文件)      -- 看板数据获取和拖拽逻辑
  api/kanban.ts (新文件)           -- 看板 API 客户端
```

---

## Implementation Phases

### Phase 1: 后端 API + 持久化 (MVP)

- 新建 `kanban.rs`，实现 `KanbanConfig` 数据模型和文件读写
- 在 `server.rs` 中新增看板路由（GET/PUT/POST/DELETE）
- 扩展 `SyncEvent` 添加 kanban 事件
- 在 `session_event_loop` 中添加新会话自动入看板逻辑
- 服务启动时加载 `kanban.json`（不存在则创建默认配置）

### Phase 2: 前端看板视图 (MVP)

- 安装 `@dnd-kit/core` + `@dnd-kit/sortable`
- 实现 `KanbanView` 页面组件和子组件
- 实现 `useKanban` hook（数据获取 + 拖拽处理 + SSE 监听）
- 实现视图切换按钮（列表/看板）
- 构建并嵌入新的前端产物

### Phase 3: 列管理 + 打磨

- 实现列的增删改拖拽
- 卡片右键菜单（重命名/归档/删除）
- 归档会话显示/隐藏开关
- 拖拽动画和视觉打磨
- 错误处理和边界情况

---

## Handoff Notes for Implementation and Testing

- **Engineering must not misunderstand**: 看板列与会话生命周期（active/inactive/archived）是完全独立的。拖拽卡片到任何列都不应触发 `POST /api/sessions/:id/archive` 或其他生命周期变更。看板是纯粹的元数据标记层。
- **Testing should verify first**: A1（看板渲染）、A2（拖拽持久化）、A9（重启恢复）、A3（SSE 同步）-- 这四个是核心路径。
- **Still ambiguous but non-blocking**: 卡片标签/颜色功能推迟到 Phase 2；归档会话在看板中的精确显示行为可在实现中微调。
- **Fastest trustworthy verification path**: 启动 `codex serve` -> 创建 2 个会话 -> 切换到看板视图确认卡片出现 -> 拖拽一张卡片到另一列 -> 刷新页面确认位置保留 -> 打开第二个标签页确认 SSE 同步 -> 重启服务确认看板恢复。

---

_This PRD was created through interactive requirements gathering and is optimized for implementation handoff, testability, and scope clarity._
