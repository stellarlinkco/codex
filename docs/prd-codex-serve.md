# PRD: `codex serve` — Web UI via HTTP+SSE

## 1. 概述

新增 `codex serve` 子命令，在本地启动 HTTP 服务器，提供内嵌 Web UI，用户通过浏览器与 Codex 交互。后端通过 HTTP+SSE 原生实现（非 CLI 包装），前端直接复用 `reference/hapi/web` 源码并适配。

### 目标

- 提供浏览器端完整交互体验：多会话管理、聊天、工具审批、终端
- 原生 Rust HTTP 服务 + SSE 推送，零外部进程依赖
- 绑定 localhost + 随机 token 认证

### 非目标

- 远程/多用户访问
- 语音助手集成（hapi voice 功能不复用）
- Telegram 集成
- Push Notifications
- 多 Machine 管理（本地单机场景）

---

## 2. 架构

```
┌─────────────────────────────────────────────────┐
│  Browser (Web UI)                               │
│  - React + Tailwind + assistant-ui              │
│  - HTTP API calls (POST/GET)                    │
│  - SSE stream (GET /api/events)                 │
│  - WebSocket (terminal only)                    │
└──────────────┬──────────────────────────────────┘
               │ HTTP / SSE / WS
┌──────────────▼──────────────────────────────────┐
│  codex-rs/serve (新 crate)                       │
│  ┌────────────────────────────────────────────┐ │
│  │ HTTP Layer (axum)                          │ │
│  │ - Static file serving (embedded assets)    │ │
│  │ - REST API routes                          │ │
│  │ - SSE endpoint                             │ │
│  │ - WebSocket endpoint (terminal)            │ │
│  │ - Token auth middleware                    │ │
│  └────────────────┬───────────────────────────┘ │
│  ┌────────────────▼───────────────────────────┐ │
│  │ Session Bridge                             │ │
│  │ - Maps HTTP sessions → app-server sessions │ │
│  │ - Event routing → SSE streams              │ │
│  │ - Terminal PTY management                  │ │
│  └────────────────┬───────────────────────────┘ │
└───────────────────┼─────────────────────────────┘
                    │ Internal API
┌───────────────────▼─────────────────────────────┐
│  codex-rs/core (ThreadManager)                   │
│  - Thread/Turn lifecycle                         │
│  - Tool execution & approval                     │
│  - Event broadcasting                            │
└─────────────────────────────────────────────────┘
```

### 关键决策

- **新建 `codex-rs/serve` crate**：独立于 app-server，直接依赖 core
- **不走 JSON-RPC**：serve crate 直接调用 core API，避免序列化开销
- **axum 作为 HTTP 框架**：tokio 生态原生，已被 rmcp-client 间接依赖
- **rust-embed 嵌入前端产物**：单二进制分发，无需额外文件

---

## 3. CLI 接口

```
codex serve [OPTIONS]

Options:
  --port <PORT>        监听端口 (默认: 0, 自动分配)
  --host <HOST>        绑定地址 (默认: 127.0.0.1)
  --no-open            不自动打开浏览器
  --token <TOKEN>      指定 token (默认: 随机生成)
```

启动输出示例：
```
Codex Web UI running at http://127.0.0.1:3847?token=a1b2c3d4
```

---

## 4. 安全

### Token 认证

- 启动时生成 32 字节随机 hex token（或用户指定）
- 所有 HTTP 请求需携带 `Authorization: Bearer <token>` 或 `?token=<token>` query param
- SSE 连接通过 query param 传递 token
- WebSocket 连接通过初始握手 query param 验证
- Token 验证失败返回 401

### 网络绑定

- 默认绑定 `127.0.0.1`，仅本地访问
- 如用户指定 `0.0.0.0`，打印安全警告

---

## 5. HTTP API 设计

### 5.1 会话管理

| Method | Path | 说明 |
|--------|------|------|
| GET | `/api/sessions` | 列出所有会话 |
| POST | `/api/sessions` | 创建新会话 |
| GET | `/api/sessions/:id` | 获取会话详情 |
| PATCH | `/api/sessions/:id` | 重命名会话 |
| DELETE | `/api/sessions/:id` | 删除会话 |
| POST | `/api/sessions/:id/resume` | 恢复会话 |
| POST | `/api/sessions/:id/abort` | 中止会话 |
| POST | `/api/sessions/:id/archive` | 归档会话 |

### 5.2 消息

| Method | Path | 说明 |
|--------|------|------|
| GET | `/api/sessions/:id/messages` | 获取消息（分页） |
| POST | `/api/sessions/:id/messages` | 发送消息 |

Query params for GET: `limit`, `before_seq`

### 5.3 工具审批

| Method | Path | 说明 |
|--------|------|------|
| POST | `/api/sessions/:id/permissions/:reqId/approve` | 批准工具调用 |
| POST | `/api/sessions/:id/permissions/:reqId/deny` | 拒绝工具调用 |

### 5.4 配置

| Method | Path | 说明 |
|--------|------|------|
| POST | `/api/sessions/:id/permission-mode` | 切换权限模式 |
| POST | `/api/sessions/:id/model` | 切换模型 |
| GET | `/api/sessions/:id/slash-commands` | 获取 slash commands |
| GET | `/api/sessions/:id/skills` | 获取 skills 列表 |

### 5.5 文件与 Git

| Method | Path | 说明 |
|--------|------|------|
| GET | `/api/sessions/:id/git-status` | Git 状态 |
| GET | `/api/sessions/:id/git-diff-file` | 文件 diff |
| GET | `/api/sessions/:id/file` | 读取文件 |
| GET | `/api/sessions/:id/files` | 搜索文件 |
| GET | `/api/sessions/:id/directory` | 列出目录 |

### 5.6 SSE

| Method | Path | 说明 |
|--------|------|------|
| GET | `/api/events` | SSE 事件流 |

Query params: `sessionId`（可选，订阅特定会话）

### 5.7 静态资源

| Method | Path | 说明 |
|--------|------|------|
| GET | `/*` | 前端静态文件（SPA fallback） |

---

## 6. SSE 事件定义

```
event: session-added
data: {"session": <Session>}

event: session-updated
data: {"session": <Session>}

event: session-removed
data: {"sessionId": "<id>"}

event: message-received
data: {"sessionId": "<id>", "message": <Message>}

event: heartbeat
data: {}
```

### 心跳

- 服务端每 30 秒发送 `heartbeat` 事件
- 客户端检测断连后自动重连（EventSource 原生行为）

---

## 7. 会话模型

### Session

```rust
pub struct WebSession {
    pub id: String,
    pub name: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub active: bool,
    pub thinking: bool,
    pub metadata: SessionMetadata,
    pub agent_state: Option<AgentState>,
    pub permission_mode: PermissionMode,
    pub model_mode: ModelMode,
}
```

### SessionMetadata

```rust
pub struct SessionMetadata {
    pub path: String,           // 工作目录
    pub summary: Option<String>,
    pub tools: Vec<String>,
}
```

### AgentState（工具审批）

```rust
pub struct AgentState {
    pub requests: HashMap<String, ToolRequest>,
    pub completed_requests: HashMap<String, CompletedToolRequest>,
}

pub struct ToolRequest {
    pub tool: String,
    pub arguments: serde_json::Value,
    pub created_at: u64,
}
```

### Message

```rust
pub struct WebMessage {
    pub id: String,
    pub seq: u64,
    pub content: serde_json::Value,  // 兼容 hapi 消息格式
    pub created_at: u64,
}
```

---

## 8. 会话生命周期

```
创建 (POST /api/sessions)
  │
  ▼
活跃 ←──── 恢复 (POST .../resume)
  │              ▲
  │ (发消息/工具执行)
  │              │
  ▼              │
不活跃 ─────────┘
  │
  ├──→ 归档 (POST .../archive)
  │
  └──→ 删除 (DELETE .../id)
```

- 创建会话时初始化 core::ThreadManager 中的 thread
- 每个 Web session 对应一个 core thread
- 会话状态持久化到本地 SQLite（复用 hapi 的 store 模式）

---

## 9. 工具审批流程

```
1. Agent 请求执行工具
2. core 发出 tool_request 事件
3. serve 更新 session.agent_state.requests
4. SSE 推送 session-updated 事件
5. Web UI 显示 PermissionFooter（approve/deny 按钮）
6. 用户点击 approve
7. Web 调用 POST /api/sessions/:id/permissions/:reqId/approve
8. serve 通知 core 继续执行
9. core 执行工具，返回结果
10. SSE 推送 message-received（工具输出）
```

---

## 10. 终端集成

- 终端使用 WebSocket（非 SSE），因为需要双向实时通信
- 端点：`/ws/terminal/:sessionId/:terminalId`
- 后端通过 `portable_pty` 或 tokio 子进程管理 PTY
- 前端复用 hapi/web 的 xterm.js 组件

### WebSocket 消息协议

```json
// Client → Server
{"type": "input", "data": "ls -la\n"}
{"type": "resize", "cols": 80, "rows": 24}

// Server → Client
{"type": "output", "data": "..."}
{"type": "exit", "code": 0}
```

---

## 11. 前端适配

### 复用范围

从 `reference/hapi/web` 直接复用：
- `components/` — 全部 UI 组件
- `chat/` — 聊天状态管理
- `hooks/` — React Query hooks
- `routes/` — 页面路由
- `types/` — TypeScript 类型
- `lib/` — 工具函数和 context

### 需要适配的部分

| 模块 | 变更 |
|------|------|
| `api/client.ts` | 移除 Telegram auth，改用 token auth；baseUrl 指向本地 serve |
| `hooks/useSSE.ts` | 保持不变（已是标准 EventSource） |
| `hooks/useAuth.ts` | 简化为 token-based auth |
| `components/NewSession/` | 移除 machine 选择（本地单机），简化为目录选择 |
| `realtime/` | 移除（不复用语音功能） |
| `components/Terminal/` | WebSocket URL 指向本地 serve |

### 移除的功能

- Telegram 登录/绑定
- 语音助手
- Push Notifications
- 多 Machine 管理
- 远程 session switch

### 构建

- 前端独立构建为静态产物（`npm run build`）
- 使用 `rust-embed` 将 `dist/` 嵌入 serve crate 二进制
- 开发模式下支持 `--dev` flag 从文件系统 serve（方便前端热更新）

---

## 12. 数据持久化

- 使用 SQLite 存储会话和消息（与 hapi/hub 的 store 模式一致）
- 数据库文件位于 `~/.codex/serve.db`
- Schema:
  - `sessions` — 会话元数据
  - `messages` — 消息内容（JSON）
  - `settings` — 用户偏好

---

## 13. Crate 依赖

```toml
[dependencies]
codex-core = { path = "../core" }
axum = "0.8"
axum-extra = { version = "0.10", features = ["typed-header"] }
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["fs", "cors"] }
rust-embed = "8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
rand = "0.8"
hex = "0.4"
tracing = "0.1"
```

---

## 14. 分阶段实施计划

### Phase 1: 基础骨架

- 创建 `codex-rs/serve` crate
- 实现 `codex serve` CLI 子命令
- axum HTTP 服务器 + token auth 中间件
- 静态文件 serving（先用占位 HTML）
- SSE endpoint（心跳）

### Phase 2: 会话管理

- SQLite 存储层
- 会话 CRUD API
- 与 core::ThreadManager 集成
- SSE 会话事件推送

### Phase 3: 聊天交互

- 消息收发 API
- core 事件 → SSE message-received 桥接
- 消息持久化

### Phase 4: 工具审批

- AgentState 管理
- 审批 API（approve/deny）
- SSE session-updated 推送

### Phase 5: 前端集成

- 复制 hapi/web 源码到 `codex-rs/serve/web/`
- 适配 API client 和 auth
- 移除不需要的功能模块
- 构建并嵌入静态产物

### Phase 6: 终端

- WebSocket 终端端点
- PTY 管理
- 前端 xterm.js 适配

### Phase 7: 文件与 Git

- 文件读取/搜索 API
- Git status/diff API
- 前端 SessionFiles 组件适配

### Phase 8: 打磨

- 自动打开浏览器
- 开发模式（`--dev`）
- 错误处理与日志
- 性能优化（SSE 背压、连接清理）

---

## 15. 风险与缓解

| 风险 | 缓解 |
|------|------|
| hapi/web 依赖 Socket.IO，适配 SSE 工作量大 | hapi/web 的 SSE hook 已是标准 EventSource，Socket.IO 仅用于 CLI↔Hub 通信，Web 端不直接使用 |
| core API 不够稳定，serve 直接依赖可能频繁变更 | 定义 serve 内部的 trait 抽象层，隔离 core 变更 |
| 前端构建产物体积影响二进制大小 | gzip 压缩嵌入，典型 React SPA 约 1-3MB |
| SQLite 并发写入限制 | 单进程场景，WAL 模式足够 |
| 终端 PTY 跨平台兼容性 | 使用 portable-pty crate，已有跨平台支持 |
