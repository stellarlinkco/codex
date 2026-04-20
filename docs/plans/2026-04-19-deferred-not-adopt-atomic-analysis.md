# 暂缓 / 不吸纳项原子级代码功能分析

日期：2026-04-19  
分支：`batch/upstream-intake-high-mid-20260418`

说明：
- 仅分析当前已判定为`暂缓`或`不吸纳`的上游提交。
- 口径聚焦三件事：代码功能点、当前不吸纳原因、若继续不吸纳相对原版缺失什么。
- “影响”默认指与 `openai/codex` 当前实现相比的功能差异，不把纯 CI/文档项夸大成用户功能缺失。

## 2026-04-20 第一层重校准补记
- 第一层原清单中，`4cd85b28d2`、`8475d51655`、`ab82568536`、`baaf42b2e4`、`fe7c959e90`、`c3ecb557d3` 已确认在当前分支具备等价实现，不再视为待吸纳项。
- `76ea694db5` 原先按“推进吸纳”登记，但复核后确认当前 fork 不包含上游 `app-server` remote-control transport 栈，因此该项应改按当前分支结构下的 N/A 处理，而不是继续排进第一层实施列表。
- 第一层真正剩余的代码差距只剩：
  - `22f7ef1cb7` ChatGPT logout revoke
  - `64177aaa22` memory phase2 writable-root 收缩

## 暂缓项

| Commit | 代码原子功能 | 当前暂缓原因 | 若不吸纳的影响 / 相对原版缺失 |
| --- | --- | --- | --- |
| `b976e701a` | 在 `windows-sandbox-rs` elevated 路径支持 split carveouts，允许更细粒度读写 carveout。 | 与本仓当前 fail-closed 的 elevated sandbox 策略冲突，直接吸纳有权限放宽风险。 | Windows elevated sandbox 不能像原版那样细粒度拆分 carveout；遇到该场景会更保守地拒绝，而不是继续执行。 |
| `f2a4925f63` | `compact.rs` 针对 Azure provider 增加 remote compaction 分支。 | 依赖 provider/部署场景判断；本仓未确认 Azure Responses provider 是否作为正式目标。 | 若使用 Azure provider，压缩/裁剪路径不会走原版远端优化分支。 |
| `50d3128269` | 把 archive/unarchive 操作迁到 local `ThreadStore`。 | 涉及线程持久化事实源迁移，和本仓 `app-server`/线程生命周期自研改动耦合。 | 原版基于 local thread store 的 archive/unarchive 一致性与复用路径本仓暂未对齐。 |
| `48cf3ed7b0` | 将插件加载与 marketplace 逻辑抽到 `codex-core-plugins` crate。 | 属于插件架构重排，不是局部修复；直接吸纳容易和本仓现有 plugin 管理主链冲突。 | 本仓不会获得原版插件加载/marketplace 的模块化拆分收益，后续跟进插件功能的合并成本更高。 |
| `f97be7dfff` | ChatGPT Fed 会话经 Fed edge 路由。 | 属于合规/部署特化路由，不是通用功能缺口。 | 若使用该特定 Fed ChatGPT auth 场景，本仓不会具备原版同样的路由策略。 |
| `85203d8872` | 将 image generation 默认打开。 | 这是默认策略变化，不是修复；会影响成本、能力暴露与安全口径。 | 原版开箱即用图像生成，本仓仍需显式开启或保持当前默认策略。 |
| `6e72f0dbfd` | 增加 remote `ThreadStore` 实现与 proto。 | 涉及新状态面与依赖，不适合和当前线程存储实现混合落地。 | 本仓缺少原版 remote thread store 后端，线程存储能力仍以现有本地/自研路径为主。 |
| `faf48489f3` | 对 configured marketplaces 增加自动升级流程。 | 会改变插件源升级控制面，与本仓插件配置治理冲突风险高。 | 原版可自动升级已配置 marketplace，本仓仍需手动管理升级。 |
| `0708cc78cb` | 把 `codex.rs` 的 op handlers 拆到 `codex/handlers.rs`。 | 纯核心结构重排；在本仓 `core/src/codex.rs` 深改前提下吸纳收益小、冲突高。 | 几乎没有直接用户功能差异，主要缺少原版代码结构拆分。 |
| `a1736fcd20` | 把 turn 逻辑拆到 `codex/turn.rs`。 | 同上，属于内部重构。 | 基本无用户功能缺失，主要是后续对齐原版 turn 逻辑时合并成本更高。 |
| `5818ed6660` | CLI 把 `marketplace add` 收口到 `plugin` 命令树下。 | 属于命令入口重组，和本仓现有插件 CLI 习惯未统一。 | 命令入口与原版不同；用户无法使用原版新的 `plugin ...` 子命令路径添加 marketplace。 |
| `9effa0509f` | config loading 统一改为 filesystem abstraction。 | 触及 `config`、`protocol`、`core`、`app-server` 多层，属于架构换骨。 | 本仓缺少原版统一文件系统抽象层，某些后续配置能力无法直接平移。 |
| `91e8eebd03` | 将 session 相关代码拆成 `mcp/review/session/turn_context` 子模块。 | 属于内部拆分，风险高于收益。 | 直接用户功能差异很小，主要缺少原版 session 模块化结构。 |
| `a803790a10` | 引入 provider runtime abstraction，可按 provider 切换运行时行为。 | 是 provider 抽象层升级，会影响 `codex-api/login/cli/core` 多域。 | 本仓缺少原版 provider runtime 抽象，未来接更多 provider 时扩展成本更高。 |
| `71e4c6fa17` | 将 `codex` 主模块整体下沉到 `session` 目录体系。 | 改动范围覆盖 `core` 大片深改区，属于高冲突重构。 | 基本无直接用户功能缺失，主要是内部结构未与原版对齐。 |
| `fad3d0f1d0` | `thread/read` 持久化改走 `thread-store`。 | 涉及 app-server 持久化事实源切换。 | 原版 `thread/read` 与 thread-store 的统一持久化路径本仓未对齐。 |
| `eaf78e43f2` | app-server 增加 `thread/list` 排序/`backwardsCursor` 与 `thread/turns/list` API。 | 明确触碰共享协议与 schema；不能在当前分支顺手改写。 | 本仓缺少这些新的 app-server API 能力，客户端不能按原版方式分页列线程与 turns。 |
| `92cf90277d` | 抽象 MCP stdio server launcher。 | 属于后续 executor-backed MCP stdio 铺路提交。 | 本仓没有原版那套独立 launcher 抽象，MCP stdio 管理能力较旧。 |
| `f017a23835` | `/plugins` 改为 v2 tabbed marketplace 菜单。 | 属于显著 UI/UX 重塑，会和本仓现有插件弹窗体验直接冲突。 | 原版插件菜单有 tabbed marketplace 视图，本仓仍是现有单路径 UI。 |
| `06f8ec54db` | `/plugins` 菜单支持 inline enable/disable toggle。 | 同样属于插件 UI 交互重塑。 | 原版可在 TUI 菜单内直接启停插件，本仓需要走当前已有配置/命令路径。 |
| `6b39d0c657` | app-server 新增 owner nudge API（发送 add credits nudge email）。 | 明显是新业务 API，且会改 schema 与通知面。 | 本仓没有原版 owner nudge 业务接口。 |
| `e9c70fff3f` | CLI/core 增加 marketplace remove 命令及共享逻辑。 | 命令面与插件治理策略一起变化，需与现有插件命令体系统一设计。 | 原版可直接移除 marketplace，本仓缺少同等 CLI 管理能力。 |
| `53b1570367` | 图像输出默认 high detail，并更新协议字段处理。 | 属于默认体验/成本策略变更，不宜直接同步。 | 原版图像输出默认更高细节，本仓保持当前 detail 默认值。 |
| `996aa23e4c` | 把 MCP stdio 真正接到 executor-backed 路径。 | 这是一串连续架构变更的后半段；单独吸纳会造成执行链断裂。 | 本仓缺少原版 executor-backed MCP stdio 能力，MCP stdio 仍走现有实现。 |

## 不吸纳项

| Commit | 代码原子功能 | 当前不吸纳原因 | 若不吸纳的影响 / 相对原版缺失 |
| --- | --- | --- | --- |
| `04fc208b6` | `tools/src/tool_discovery.rs` 保持 `tool_search_output` 结果原顺序。 | 本仓无对应 `tools/tool_discovery` 模块，实现落点不存在。 | 无实际缺失；这是功能域 N/A，不是漏功能。 |
| `0bb438bca6` | 增加 `SECURITY.md` 引用和部分 Bazel 绑定。 | 文档/构建维护项。 | 对终端用户无功能差异。 |
| `17d94bd1e3` | 回滚前一版 `SECURITY.md` 附带改动。 | 文档回滚，不影响运行时。 | 无用户功能差异。 |
| `18d61f6923` | 继续恢复 `SECURITY.md` 内容。 | 纯文档。 | 无用户功能差异。 |
| `d63ba2d5ec` | 新增 `codex-pr-body` skill。 | 上游内部/配套工作流 skill，不属于本仓产品能力基线。 | 本仓不会获得该自动生成 PR 描述的 skill。 |
| `b0324f9f05` | 修一个 responses-api-proxy flake。 | 主要是测试稳态修复，非产品功能。 | 基本无用户功能缺失；只是在对应测试场景下缺少上游稳定性修补。 |
| `d4223091d0` | 修 Windows flake（`state/src/log_db.rs`）。 | 目标是消除 flake，不是功能升级。 | 无确定用户功能缺失。 |
| `9c326c4cb4` | 给 memories config 增加最小值限制。 | 更像参数护栏细化，当前本仓未把它视为必须对齐项。 | 若用户输入极小 memories 配置，原版会更早约束；本仓仍按当前口径处理。 |
| `de98b1d3e8` | 为 Windows flake 做调试/脚本修补。 | CI 调试提交。 | 无用户功能差异。 |
| `18e9ac8c75` | 增强 stream pollution filtering。 | 更偏观测/内部事件过滤调优，当前收益不足以覆盖冲突。 | 原版内部流事件噪声过滤可能更干净；本仓主要差异在内部事件面，不是核心功能缺失。 |
| `b33478c236` | 统一 memory drop endpoint。 | 主要是内部接口收口；本仓已有自己的 memory 清理链路。 | 命令/接口一致性不如原版统一，但当前功能可用。 |
| `895e2d056f` | 移除一个 `expect`。 | 纯实现清理。 | 无用户功能差异。 |
| `6adba99f4d` | 大量 Bazel/测试稳定化修复。 | 主要是上游 CI 环境的维护项。 | 对本仓最终用户无直接功能缺失。 |
| `b178d1cf17` | `justfile` 使用 `justfile_directory`。 | 构建脚本细节优化。 | 无用户功能差异。 |
| `3a4fa77ad7` | YOLO 模式跳过 managed-network tool enforcement。 | 这会降低网络工具约束，和本仓偏保守安全策略相反。 | 原版在 YOLO 下更宽松，本仓会继续执行现有网络审批/限制，不会放宽。 |
| `ea34c6ed8d` | 修 example 里的 clippy。 | 示例代码维护。 | 无用户功能差异。 |
| `3905f72891` | 限制 Windows Bazel 测试并发。 | CI 稳定化。 | 无用户功能差异。 |
| `65cc12d72e` | guardian reviews 切到 `codex-auto-review`。 | 上游内部审查流程选择，不是产品能力缺口。 | 本仓不会得到同样的 guardian review 自动化路由。 |
| `2e038e6d38` | 修 Windows exec policy test flake。 | 测试稳定项。 | 无用户功能差异。 |
| `3421a107e0` | 调整 memories phase2 ephemeral 细节。 | 更偏内部参数/测试口径微调，当前不是 fork 主缺口。 | 原版 phase2 memories 细节可能稍不同，但不是核心用户能力缺失。 |
| `af7b8d551c` | Guardian 事件名/协议从 Guardian 转向 Auto-Review。 | 它会改协议语义与通知命名，但属于上游工作流命名重构。 | 本仓不会与原版保持同样的 auto-review 通知命名和状态模型。 |
| `2c2ed51876` | 让 Windows Bazel clippy 覆盖 core test imports。 | 纯 CI。 | 无用户功能差异。 |
| `481ba014a7` | 增加 core CODEOWNERS。 | 仓库治理文件。 | 无用户功能差异。 |
| `29bc2ad2f4` | Bazel repository cache 按 job 隔离。 | 纯 CI 性能/稳定优化。 | 无用户功能差异。 |
| `96d35dd640` | Bazel native rust test sharding。 | 构建门禁优化，不是运行时功能。 | 无用户功能差异。 |

## 结论

- `暂缓`项大多不是“没价值”，而是落在高耦合区：`AppServer/Protocol`、`Plugin/MCP`、`ThreadStore`、`Core 架构拆分`。这些项如果直接吸纳，最容易覆盖掉 fork 现有实现。
- `不吸纳`项里绝大多数是 `CI/文档/治理/测试稳定化`，真正会形成用户可感知差异的只有少数几项：
  - `3a4fa77ad7`：本仓不会像原版那样在 YOLO 下放宽 managed-network enforcement。
  - `d63ba2d5ec` / `65cc12d72e` / `af7b8d551c`：本仓缺少对应的 review/PR/body/guardian 工作流能力或命名收口。
  - `9c326c4cb4` / `3421a107e0`：在 memories 参数/细节口径上与原版存在轻微行为差异。
