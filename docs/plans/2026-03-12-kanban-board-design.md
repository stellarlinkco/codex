# Kanban Board（codex serve）— 设计落地记录（MVP）

来源：`docs/kanban-board-prd.md`（2026-03-12，Final）。

## 目标（MVP）

- 在 `codex serve` Web UI 增加看板视图，与现有会话列表共存，可切换并用 localStorage 记住选择。
- 看板持久化到 `CODEX_HOME/kanban.json`（默认 `~/.codex/kanban.json`），服务重启后保持列与卡片位置。
- 支持跨列移动与列内排序；移动结果通过 SSE 同步到所有打开的客户端。

## 关键约束与决策

- 看板列与会话生命周期（active/inactive/archived）完全解耦；拖拽不触发归档或其他生命周期变化。
- 列 ID 使用稳定字符串（`backlog` / `in-progress` / `review` / `done`），避免首次创建/恢复时因随机 UUID 引发不必要抖动。
- 服务端以“最后写入者胜出”为一致性模型；客户端以 SSE + 拉取修正最终状态。

## API / SSE（MVP）

- `GET /api/kanban`：返回 `{ columns, cardPositions }` 快照。
- `PUT /api/kanban/cards/{sessionId}`：移动/排序单张卡片。
- `PUT /api/kanban/cards/batch`：批量更新（拖拽排序优化）。
- SSE：新增 `kanban-updated`（全量快照）与 `card-moved`（单卡提示，供前端快速更新/回退）。

