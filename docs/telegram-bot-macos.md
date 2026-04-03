# Telegram Bot for Codex on macOS

## Summary

本文定义一个仅支持 macOS 的 Telegram Bot 集成方案，让 Telegram 可以查看和控制 Codex 会话，并与本地终端或 TUI 中正在运行的会话保持同步。

目标能力：

- 查看项目列表
- 查看每个项目下的会话和状态
- 打开运行中的会话
- 打开未运行的历史会话
- 查看当前会话是否仍在生成、已停止、等待审批或等待用户输入
- 本地终端有新消息时，Telegram 同步可见
- Telegram 发出的消息可同步到当前打开的终端会话
- Telegram 可处理审批和 `request_user_input`

v1 范围不包含：

- PTY 终端画面镜像
- shell 实时屏幕同步
- 多用户或多租户
- 非 macOS 支持

## Product Behavior

### 1. 会话类型

Telegram 看到的会话分为三类：

- `liveWindow`
  - 已有本地终端或 TUI 正在运行该会话
  - Telegram 通过本地 live bridge 附着到它
- `headless`
  - 由 `codex serve` 恢复并托管的运行中会话
  - 没有本地可见窗口也可继续使用
- `stored`
  - 已落盘但当前未运行的历史会话
  - Telegram 默认只读打开

### 2. 打开未运行会话

当用户在 Telegram 中打开一个未运行会话时：

- 默认只读取历史 transcript 和当前元数据
- 不立即恢复成运行态
- 只有用户明确执行“继续”或直接发送消息时，才把它激活为运行态

如果该会话已归档：

- Telegram 仍可打开查看
- 当用户继续该会话时，系统先把归档会话恢复到活动目录，再恢复为运行态

### 3. 运行中会话同步

对于运行中的会话：

- Telegram 可实时看到 assistant 增量输出
- Telegram 可看到 turn 是否在进行中
- Telegram 可看到当前是否卡在审批或 `request_user_input`
- 本地终端或 TUI 继续显示该会话的原始交互
- Telegram 不是替代终端，而是第二个控制端

### 4. 输入与控制权模型

同一条会话只允许存在一个运行时 owner，但允许多个 controller。

固定规则：

- 同一个 `thread` 只能有一个 owner runtime
- owner 可能是：
  - 本地 CLI 或 TUI 进程
  - `codex serve` 恢复出的 headless 会话
- controller 可以同时来自：
  - 本地终端输入
  - Telegram 输入

写入规则：

- Telegram 和终端都可写
- 它们写入的是同一个 owner
- 所有输入由 owner 串行处理
- 不允许为同一个 `thread` 同时恢复出两个独立运行时

当 turn 正在运行时：

- 新输入走 `steer_input`

当 turn 空闲时：

- 新输入走正常的新 turn 提交

### 5. 审批与用户提问

如果会话在运行中产生：

- shell、patch、network 等审批请求
- `request_user_input`

Telegram 必须能够直接处理：

- 批准
- 拒绝
- 回答问题

处理结果会立即同步回 owner runtime，因此本地终端和 Telegram 会保持一致。

## Architecture

### 1. High-Level Design

系统由两部分组成：

- `codex serve`
  - 作为聚合器
  - 提供 HTTP 和 SSE API
  - 托管 Telegram bot worker
  - 汇总 live 会话与存量会话
- 本地运行中的 CLI 或 TUI 进程
  - 继续按现有方式启动
  - 每个进程额外暴露一个 macOS 本地 live bridge

设计原则：

- 不要求用户必须通过 daemon 启动 Codex
- 不改变现有 CLI 或 TUI 的主交互模式
- Telegram 是附加控制面，而不是新的唯一运行入口

### 2. macOS Local IPC

仅支持 macOS，本地桥接统一使用 Unix domain sockets。

每个运行中的交互式进程会：

- 在 `CODEX_HOME/runtime/live/` 下注册自己的元数据
- 暴露一个本地 UDS socket
- 周期性刷新 heartbeat

注册信息至少包含：

- `windowId`
- `threadId`
- `pid`
- `cwd`
- `socketPath`
- `lastHeartbeatAt`

### 3. Session Backing Model

`serve` 内部统一把会话表示为：

- `LiveWindow`
- `HeadlessThread`
- `StoredRollout`

这样 Telegram 和 Web UI 都只面对统一的会话视图，而不需要关心底层会话来自哪里。

## Local Live Bridge

### 1. Why It Exists

当前终端或 TUI 直接连接 `core::ThreadManager` 与 `CodexThread`，不是通过 `serve` 运行。

因此如果 Telegram 要与“已经打开的终端窗口”实时同步，必须引入一个本地 bridge，让 `serve` 能附着到现有会话，而不是强制接管它。

### 2. Bridge Commands

live bridge 使用内部 JSON frame 协议，至少支持：

- `snapshot`
- `subscribe`
- `submit_input`
- `steer_input`
- `interrupt`
- `approve`
- `deny`
- `answer_user_input`

### 3. Bridge Events

bridge 至少推送以下事件：

- `session_state`
- `turn_started`
- `turn_completed`
- `message_delta`
- `message_finalized`
- `approval_requested`
- `request_user_input_requested`
- `owner_closed`

## Ownership and Lease

### 1. Problem

如果 Telegram 和终端分别各自去 `resume` 同一个历史会话，会出现：

- rollout 并发写入
- turn 状态分叉
- 会话内容不一致

因此必须引入本地 owner lease。

### 2. Lease Rule

每个 `thread_id` 只有一个 owner lease。

lease 文件路径固定为：

- `CODEX_HOME/runtime/leases/<thread_id>.lock`

owner 获取 lease 后写入：

- owner 类型
- pid
- windowId
- 创建时间
- 最近 heartbeat 时间

### 3. Attach Semantics

当 Telegram 或 `serve` 试图激活某个会话时：

- 如果该 `thread` 已有 live owner 且可达：
  - 只附着，不重复恢复
- 如果没有 owner：
  - 才允许恢复为新的 owner

### 4. Stale Lease Recovery

当 lease 疑似失效时，需要同时满足以下条件才可回收：

- heartbeat 超时
- pid 已不存在

## Serve API Changes

v1 不新增 app-server v2 公共协议；变更只落在 `serve` 内部 API 与事件。

### 1. Session Fields

`/api/sessions` 和 `/api/sessions/{id}` 扩展字段：

- `backing`
  - `liveWindow | headless | stored`
- `liveState`
  - `idle | generating | waitingApproval | waitingUserInput | stopped | unavailable`
- `projectKey`
- `runtimeOwner`
  - `tui | cli | serve`
- `windowId`
- `controllerCount`

### 2. Events

`/api/events` 新增事件：

- `message-delta`
- `message-finalized`
- `session-live-attached`
- `session-live-detached`

现有 `session-updated` 继续保留，用于粗粒度状态刷新。

### 3. Project Listing

新增：

- `/api/projects`

返回：

- 项目标识
- 项目路径
- 活跃会话数
- 总会话数
- 最近更新时间

项目聚合优先使用 git root；如果无法识别 git 仓库，则回退到 `cwd`。

### 4. Activate / Resume Semantics

`/api/sessions/{id}/resume` 在语义上升级为激活动作：

- 已有 owner 时：
  - 附着到 owner
- 普通未运行会话：
  - 恢复为 `headless`
- archived 会话：
  - 先 restore 再恢复为 `headless`

## Telegram Bot UX

### 1. Access Control

v1 为单用户 allowlist 模式：

- bot token 从环境变量读取
- 只允许预配置的 Telegram chat id 访问

### 2. Navigation

主要交互路径：

- `/projects`
  - 查看项目列表
- 选择项目
  - 查看该项目下的会话
- 选择会话
  - 打开 transcript
  - 查看状态
  - 继续、停止、审批、回答问题

### 3. Session Ordering

项目下会话展示顺序：

1. 运行中的会话
2. 未归档的历史会话
3. archived 会话入口

### 4. Active Watched Session

Telegram 当前聊天上下文只跟踪一个 active watched session。

行为规则：

- 打开某会话后，该会话成为当前关注会话
- 该会话的新输出主动推送到 Telegram
- 其他会话不主动刷屏，只在用户刷新时展示

### 5. Output Rendering

assistant 正在生成时：

- Telegram 维护一条可编辑消息
- 增量输出通过 edit message 更新
- turn 结束后 finalize

审批和 `request_user_input`：

- 单独发送状态卡片
- 提供快捷按钮或结构化回答入口

## Implementation Notes

### 1. `serve`

`serve` 需要新增：

- live registry 扫描器
- live bridge client
- Telegram polling worker
- session backing 聚合层
- dormant 或 archived activation 流程
- delta 级别事件广播

### 2. CLI / TUI

CLI 或 TUI 需要新增：

- live bridge server
- runtime registry writer
- owner heartbeat
- 远端输入到本地 UI 的可见同步
- owner 退出时 registry 和 socket 清理

### 3. Streaming

`serve` 当前只镜像完整 assistant message，不足以表达 Telegram 实时流式更新。

需要补齐：

- `AgentMessageContentDelta`
- `ReasoningContentDelta`
- turn 开始或结束状态同步
- pending request 状态同步

## Risks

主要风险：

- 已运行终端会话与 `serve` 的状态漂移
- stale lease 未及时清理导致无法继续会话
- Telegram 增量编辑过于频繁导致限流
- archived 会话恢复路径与 active 会话路径不一致
- 本地 UI 与 Telegram 同时写入时的顺序感知不清晰

对应缓解：

- owner 单写模型
- heartbeat + pid 双重 stale 检测
- Telegram 输出节流
- archived 恢复统一走 activate 流程
- 所有 controller 输入串行入队

## Testing

### 1. `serve` Tests

需要覆盖：

- 会话列表同时汇总 `liveWindow`、`headless`、`stored`
- dormant session 只读打开
- dormant session 首条消息触发 activate
- archived session 可查看并继续
- delta 事件正确广播
- 审批与 `request_user_input` 经 Telegram 路径完成

### 2. Runtime Bridge Tests

需要覆盖：

- live bridge 注册成功
- heartbeat 正常刷新
- `submit_input`、`steer_input`、`interrupt` 可达
- 远端输入能出现在本地运行中的会话视图里
- owner 退出后 socket 和 registry 正确清理

### 3. Ownership Tests

需要覆盖：

- 同一 `thread_id` 不会同时出现两个 owner
- 有 live owner 时 `serve` 只附着不重启
- stale lease 可回收
- `serve` 重启后可重新发现 live windows

### 4. Telegram Worker Tests

需要覆盖：

- allowlist 生效
- active watched session 才主动推送
- 增量输出经过节流
- 审批按钮与回答问题能正确映射为 owner 操作

## Non-Goals

以下内容不在 v1：

- Telegram 内嵌终端
- shell 输出逐字符镜像
- 多用户共享同一 `serve`
- 跨机器远程附着
- Windows 或 Linux 支持

## Default Decisions

本方案固定采用以下默认值：

- 平台：仅 macOS
- 本地 IPC：Unix domain sockets
- Telegram 认证：bot token + allowed chat ids
- dormant session：默认只读打开
- archived session：可查看，继续时 restore + activate
- 输入模型：多端可写，但单 owner 串行执行
- Telegram 审批：允许直接处理
- 启动模型：保持对现有 CLI 或 TUI 启动方式透明兼容

## Runtime Configuration

当前 v1 实现通过环境变量启用 Telegram worker。

必填环境变量：

- `CODEX_TELEGRAM_BOT_TOKEN`
  - Telegram bot token
  - 未设置时，`codex serve` 仍可启动，但不会启用 Telegram worker
- `CODEX_TELEGRAM_ALLOWED_CHAT_IDS`
  - 允许访问的 chat id 列表
  - 支持逗号、分号或空白分隔
  - 例如：`12345678,87654321`

可选环境变量：

- `CODEX_TELEGRAM_API_BASE_URL`
  - 默认值：`https://api.telegram.org`
  - 仅在自建代理或测试替身场景下覆盖
- `CODEX_TELEGRAM_POLL_TIMEOUT_SECS`
  - 默认值：`30`
  - 控制 Telegram long polling 超时
- `CODEX_TELEGRAM_EDIT_THROTTLE_MS`
  - 默认值：`1200`
  - 控制 assistant 增量输出的 edit-message 节流窗口

## Startup and Operations

### 1. 启动前提

- 仅在 macOS 上支持本地 live bridge 附着
- 需要本地已有 `codex` CLI 可执行文件
- 需要至少一个 Telegram bot token 和已知的 Telegram chat id

### 2. 启动 `serve`

示例：

```bash
export CODEX_TELEGRAM_BOT_TOKEN="123456:telegram-token"
export CODEX_TELEGRAM_ALLOWED_CHAT_IDS="123456789"
codex serve --host 127.0.0.1 --port 8787
```

行为说明：

- `serve` 启动后会扫描 `CODEX_HOME/runtime/live/`，尝试发现本机正在运行的 CLI 或 TUI 会话
- 如果检测到有效 live owner，Telegram 打开的运行中会话会附着到该 owner，而不是重新恢复一个新 runtime
- 如果打开的是 dormant 或 archived 会话，默认只读；只有执行“继续”或直接发送消息时才进入运行态

### 3. Telegram 侧操作入口

当前 worker 支持的主要入口：

- `/projects`
  - 查看项目列表并进入会话列表
- `/refresh`
  - 刷新当前 watched session
- `/continue`
  - 激活当前 watched session
- `/stop`
  - 中断当前 watched session
- 直接发送文本
  - 当 watched session 空闲时，提交新 turn
  - 当 watched session 正在运行时，输入走 `steer_input`

审批与 `request_user_input` 通过按钮和结构化回答入口处理，不依赖额外命令。

## Troubleshooting

### 1. Telegram 没有任何响应

优先检查：

- `CODEX_TELEGRAM_BOT_TOKEN` 是否已设置
- `CODEX_TELEGRAM_ALLOWED_CHAT_IDS` 是否包含当前 chat id
- `serve` 日志中是否出现 `telegram bot disabled due to invalid configuration`

如果 token 存在但 `allowed chat ids` 缺失或格式错误，worker 会直接禁用。

### 2. Telegram 提示未授权

症状：

- bot 回复 `This bot is not enabled for this chat.`

处理：

- 重新确认当前聊天的真实 chat id
- 把该 id 加入 `CODEX_TELEGRAM_ALLOWED_CHAT_IDS`
- 重启 `codex serve`

### 3. Telegram 提示没有 watched session

症状：

- `/refresh`、`/continue`、`/stop` 返回 `No watched session. Use /projects first.`

处理：

- 先执行 `/projects`
- 进入目标项目并打开目标会话
- 只有当前 chat 正在 watch 的会话会主动推送输出

### 4. 运行中的本地会话没有出现在 Telegram

优先检查：

- 当前机器是否为 macOS
- 本地 CLI 或 TUI 是否仍在运行
- `CODEX_HOME/runtime/live/` 下是否存在 live registry 元数据
- `CODEX_HOME/runtime/leases/` 下是否存在失效但未清理的 lease

如果 session owner 已退出但 lease 仍残留，系统只有在 heartbeat 超时且 pid 不存在时才会回收该 lease。

### 5. 增量输出刷新过慢或过快

当前 Telegram 输出经过节流，默认窗口为 `1200ms`。

处理：

- 如果编辑频率过高导致 Telegram 限流，增大 `CODEX_TELEGRAM_EDIT_THROTTLE_MS`
- 如果希望更快地看到增量输出，减小该值，但需要接受更高的 Telegram edit 频率风险

### 6. archived 会话无法继续

优先检查：

- 该会话是否能在 Telegram 中以只读方式打开
- 激活时是否走了统一的 activate 流程，而不是手工恢复旧 rollout

v1 的固定语义是：

- archived 会话可查看
- 继续时先 restore，再恢复为 `headless` 或附着到现有 live owner

## End-to-End Validation

以下路径用于验证“本地终端运行中的会话可以被 Telegram 查看并控制”：

1. 在 macOS 上打开一个本地 CLI 或 TUI 会话，并确认它正在运行某个 thread。
2. 在同一台机器上设置 `CODEX_TELEGRAM_BOT_TOKEN` 与 `CODEX_TELEGRAM_ALLOWED_CHAT_IDS`，启动 `codex serve`。
3. 在 Telegram 中给 bot 发送 `/projects`，进入对应项目并打开该运行中会话。
4. 确认 Telegram 能看到：
   - 最近 transcript
   - 当前 `liveState`
   - assistant 增量输出
5. 在本地终端继续触发一次输出，确认 Telegram 中 watched session 会收到流式更新。
6. 在 Telegram 中直接发送一条消息，确认该消息被路由回同一个 owner runtime，并在本地终端或 TUI 中可见。
7. 触发一次审批或 `request_user_input`，在 Telegram 中完成批准、拒绝或回答，确认本地终端状态同步更新。
8. 对一个 dormant 或 archived 会话重复上述流程，确认默认先只读打开，只有继续或直接发送消息时才激活。

当以上步骤全部成立时，即可视为 v1 的 Telegram 第二控制端闭环已验证完成。
