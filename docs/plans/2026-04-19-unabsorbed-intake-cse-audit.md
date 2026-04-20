# 上游未吸纳清单 CSE 审计（逐项判定）

日期：2026-04-19
分支：`batch/upstream-intake-high-mid-20260418`
范围：`rust-v0.121.0..openai/main` + 历史遗留 N/A（117~121）

## Control Contract
- Primary Setpoint：产出“全量未吸纳项 -> 单项决策”的可执行清单，且不与 fork 自研能力冲突。
- Acceptance：每一项均具备唯一决策（推进吸纳/选择性吸纳/暂缓/不吸纳）与冲突依据。
- Guardrail：不放宽权限边界；不破坏当前 CI 绿态；不改写 fork 已上线语义。
- Sampling Plan：提交级采样 + 文件重叠比（相对 `openai/main...HEAD`）判定冲突等级。
- Boundary：本轮仅审计与决策，不直接提交业务代码。

## 总览
- 未吸纳总数：132（历史遗留 2 + v0.121.0 后 130）
- 决策分布：推进吸纳 12，选择性吸纳 71，暂缓 23，不吸纳 24
- 主要冲突面：Core / TUI / AppServer / Protocol / Plugin（均为 fork 深改区）。

## 2026-04-20 第一层重校准
- 已在当前分支吸纳或具备等价实现：
  - `4cd85b28d2`
  - `8475d51655`
  - `ab82568536`
  - `baaf42b2e4`
  - `fe7c959e90`
  - `c3ecb557d3`
- 当前 fork 结构下无同层落点，按 N/A 处理：
  - `76ea694db5`
- 第一层剩余并已进入本轮实现收口：
  - `22f7ef1cb7`
  - `64177aaa22`

## 历史遗留未吸纳（117~121）
| Commit | 事项 | 决策 | 冲突 | 判定依据 |
| --- | --- | --- | --- | --- |
| `04fc208b6` | 保留 tool_search_output 原始顺序（上游 tools/tool_discovery） | **不吸纳** | 低 | 本仓无对应模块，属于功能域缺失型 N/A。 功能域不存在，直接吸纳无落点。 |
| `b976e701a` | Windows elevated sandbox split carveouts | **暂缓** | 高 | 与当前 fail-closed/elevated backend 设计冲突风险高。 权限边界变化需专门安全设计后再推进。 |

## v0.121.0 之后未吸纳逐项判定
| Commit | Area | 决策 | 冲突 | 判定依据 |
| --- | --- | --- | --- | --- |
| `f2a4925f63` | Auth/Provider | **暂缓** | 低 | 云厂商/合规路由特化能力，需确认本仓部署场景。 重叠 0/4；关键文件 codex-rs/core/src/compact.rs, codex-rs/core/src/compact_tests.rs |
| `0bb438bca6` | TUI | **不吸纳** | 中 | 文档变更，不形成运行时能力增益。 重叠 3/6；关键文件 .github/workflows/bazel.yml, SECURITY.md |
| `ab715021e6` | Misc | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 scripts/start-codex-exec.sh |
| `50d3128269` | AppServer | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/11；关键文件 codex-rs/app-server/src/codex_message_processor.rs, codex-rs/core/src/agent/control_tests.rs |
| `ba36415a30` | Misc | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 codex-rs/exec-server/tests/file_system.rs |
| `bc969b6516` | MCP/Tooling | **选择性吸纳** | 中 | 行为修复价值高，但触达 app-server/protocol 深改区，需语义移植。 重叠 6/12；关键文件 codex-rs/app-server/tests/suite/v2/turn_interrupt.rs, codex-rs/tui/src/app.rs |
| `83dc8da9cc` | TUI | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/3；关键文件 codex-rs/features/src/lib.rs, codex-rs/tui/src/bottom_pane/command_popup.rs |
| `e70ccdeaf7` | Plugin | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 0/3；关键文件 codex-rs/core/src/plugins/installed_marketplaces.rs, codex-rs/core/src/plugins/marketplace.rs |
| `17d94bd1e3` | TUI | **不吸纳** | 中 | 文档变更，不形成运行时能力增益。 重叠 3/6；关键文件 .github/workflows/bazel.yml, SECURITY.md |
| `9e2fc31854` | MCP/Tooling | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 6/20；关键文件 codex-rs/code-mode/src/description.rs, codex-rs/code-mode/src/runtime/callbacks.rs |
| `9d1bf002c6` | TUI | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 4/13；关键文件 codex-rs/Cargo.lock, codex-rs/Cargo.toml |
| `28b76d13fe` | MCP/Tooling | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 7/16；关键文件 codex-rs/core/config.schema.json, codex-rs/core/src/codex.rs |
| `18d61f6923` | Misc | **不吸纳** | 低 | 文档变更，不形成运行时能力增益。 重叠 0/1；关键文件 SECURITY.md |
| `c2bdb7812c` | AppServer | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 5/10；关键文件 codex-rs/app-server/tests/suite/v2/realtime_conversation.rs, codex-rs/codex-api/src/endpoint/realtime_websocket/methods.rs |
| `e2dbe7dfc3` | Core | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 5/5；关键文件 codex-rs/core/src/codex.rs, codex-rs/core/src/codex_tests.rs |
| `f948690fc8` | AppServer | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 codex-rs/app-server/tests/suite/v2/command_exec.rs |
| `d63ba2d5ec` | Misc | **不吸纳** | 低 | 上游内部技能工作流，不是本仓能力缺口。 重叠 0/1；关键文件 .codex/skills/codex-pr-body/SKILL.md |
| `d97bad1272` | TUI | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 3/4；关键文件 codex-rs/tui/src/app.rs, codex-rs/tui/src/chatwidget.rs |
| `bd61737e8a` | TUI | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 6/25；关键文件 codex-rs/app-server-client/src/lib.rs, codex-rs/app-server/src/codex_message_processor.rs |
| `77fe33bf72` | MCP/Tooling | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 0/4；关键文件 codex-rs/core/tests/suite/search_tool.rs, codex-rs/features/src/lib.rs |
| `224dad41ac` | MCP/Tooling | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 8/32；关键文件 codex-rs/app-server-protocol/schema/json/ServerNotification.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `48cf3ed7b0` | Plugin | **暂缓** | 中 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 9/24；关键文件 codex-rs/Cargo.lock, codex-rs/Cargo.toml |
| `4cd85b28d2` | MCP/Tooling | **推进吸纳** | 中 | 修复 MCP 启动取消链路，减少启动悬挂。 重叠 2/4；关键文件 codex-rs/app-server/src/codex_message_processor.rs, codex-rs/codex-mcp/src/mcp_connection_manager.rs |
| `f97be7dfff` | MCP/Tooling | **暂缓** | 低 | 云厂商/合规路由特化能力，需确认本仓部署场景。 重叠 1/11；关键文件 codex-rs/backend-client/src/client.rs, codex-rs/codex-api/src/api_bridge.rs |
| `b0324f9f05` | Core | **不吸纳** | 低 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 0/2；关键文件 codex-rs/core/tests/suite/responses_api_proxy_headers.rs, codex-rs/responses-api-proxy/src/dump.rs |
| `d4223091d0` | Misc | **不吸纳** | 高 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/1；关键文件 codex-rs/state/src/log_db.rs |
| `9c326c4cb4` | Core | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/3；关键文件 codex-rs/config/src/types.rs, codex-rs/config/src/types_tests.rs |
| `de98b1d3e8` | AppServer | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/2；关键文件 .github/scripts/run-bazel-ci.sh, codex-rs/app-server/tests/suite/v2/thread_unsubscribe.rs |
| `18e9ac8c75` | Core | **不吸纳** | 高 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 2/3；关键文件 codex-rs/core/src/codex.rs, codex-rs/core/src/stream_events_utils.rs |
| `b33478c236` | AppServer | **不吸纳** | 高 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 6/8；关键文件 codex-rs/app-server/src/codex_message_processor.rs, codex-rs/cli/src/main.rs |
| `895e2d056f` | Misc | **不吸纳** | 高 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/1；关键文件 codex-rs/protocol/src/protocol.rs |
| `6adba99f4d` | Plugin | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 15/34；关键文件 codex-rs/app-server/src/message_processor.rs, codex-rs/app-server/src/transport/remote_control/websocket.rs |
| `76ea694db5` | AppServer | **不吸纳（当前分支 N/A）** | 低 | 当前 fork 不包含上游 `app-server/src/transport/remote_control/*` 远控传输栈，没有同层补丁落点。该项不是“遗漏未修”，而是分支结构差异导致的功能域 N/A。 |
| `b178d1cf17` | Misc | **不吸纳** | 高 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/1；关键文件 justfile |
| `b4be3617f9` | Plugin | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 3/23；关键文件 codex-rs/cli/src/mcp_cmd.rs, codex-rs/codex-mcp/src/mcp/mod.rs |
| `8475d51655` | TUI | **推进吸纳** | 低 | 去除重复 context 状态项，TUI 展示修复。 重叠 1/5；关键文件 codex-rs/tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__status_line_setup__tests__setup_view_snapshot_uses_runtime_preview_values.snap, codex-rs/tui/src/bottom_pane/status_line_setup.rs |
| `9c6d038622` | MCP/Tooling | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 0/5；关键文件 codex-rs/code-mode/src/description.rs, codex-rs/core/tests/suite/code_mode.rs |
| `9c56e89e4f` | TUI | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/4；关键文件 codex-rs/tui/src/bottom_pane/custom_prompt_view.rs, codex-rs/tui/src/chatwidget.rs |
| `ab82568536` | TUI | **推进吸纳** | 高 | 修复 TUI resume hint 异常，用户可见稳定性收益直接。 重叠 1/1；关键文件 codex-rs/tui/src/app.rs |
| `85203d8872` | Core | **暂缓** | 低 | 默认成本/体验策略变更，需产品与成本口径先确认。 重叠 0/4；关键文件 codex-rs/core/src/tools/spec_tests.rs, codex-rs/features/src/lib.rs |
| `3a4fa77ad7` | Core | **不吸纳** | 高 | 放宽 managed-network 限制与本仓安全收敛方向相反。 重叠 4/4；关键文件 codex-rs/core/src/codex_tests.rs, codex-rs/core/src/tools/js_repl/mod.rs |
| `baaf42b2e4` | TUI | **推进吸纳** | 中 | model menu 弹出问题修复，交互回归收益明确。 重叠 2/5；关键文件 codex-rs/tui/src/bottom_pane/bottom_pane_view.rs, codex-rs/tui/src/bottom_pane/custom_prompt_view.rs |
| `6e72f0dbfd` | CI/Docs | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 3/13；关键文件 MODULE.bazel.lock, codex-rs/Cargo.lock |
| `2ca270d08d` | Core | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/9；关键文件 codex-rs/core/src/unified_exec/process_manager.rs, codex-rs/exec-server/README.md |
| `ff9744fd66` | TUI | **不吸纳** | 低 | 当前 fork 通过 `ListSkillsResponse.errors` 非致命回传 skills 加载错误，根因已被现有事件模型覆盖。 当前结构不存在上游那条直接 `await skills_list()` 的致命失败链。 |
| `6862b9c745` | TUI | **选择性吸纳** | 中 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 10/23；关键文件 codex-rs/Cargo.lock, codex-rs/core/src/codex.rs |
| `109b22a8d0` | Plugin | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 3/7；关键文件 codex-rs/app-server/src/external_agent_config_api.rs, codex-rs/core/src/external_agent_config.rs |
| `faf48489f3` | Plugin | **暂缓** | 低 | 插件交互流程重塑，和现有 fork 插件体验存在冲突风险。 重叠 5/17；关键文件 codex-rs/Cargo.lock, codex-rs/app-server/src/codex_message_processor.rs |
| `ab97c9aaad` | TUI | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 6/13；关键文件 codex-rs/app-server-client/src/lib.rs, codex-rs/app-server/src/codex_message_processor.rs |
| `206dd13c32` | Plugin | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 8/20；关键文件 codex-rs/Cargo.lock, codex-rs/chatgpt/Cargo.toml |
| `71174574ad` | Plugin | **选择性吸纳** | 低 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 4/27；关键文件 codex-rs/cli/src/mcp_cmd.rs, codex-rs/codex-mcp/src/mcp/mod.rs |
| `dfff8a7d03` | Core | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/1；关键文件 codex-rs/core/src/codex.rs |
| `62847e7554` | AppServer | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/2；关键文件 codex-rs/app-server/tests/suite/v2/thread_unsubscribe.rs, codex-rs/core/tests/common/streaming_sse.rs |
| `8720b7bdce` | TUI | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 8/32；关键文件 codex-rs/analytics/src/analytics_client_tests.rs, codex-rs/analytics/src/client.rs |
| `ea34c6ed8d` | Misc | **不吸纳** | 低 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 0/1；关键文件 codex-rs/thread-store/examples/generate-proto.rs |
| `ec8d4bfc77` | AppServer | **选择性吸纳** | 高 | 行为修复价值高，但触达 app-server/protocol 深改区，需语义移植。 重叠 6/10；关键文件 codex-rs/app-server-protocol/src/protocol/thread_history.rs, codex-rs/app-server/README.md |
| `37bf42d5d5` | Core | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/3；关键文件 codex-rs/core/src/realtime_context.rs, codex-rs/core/src/realtime_context_tests.rs |
| `3905f72891` | CI/Docs | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/2；关键文件 .github/scripts/run-bazel-ci.sh, .github/workflows/bazel.yml |
| `0708cc78cb` | Core | **暂缓** | 中 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/2；关键文件 codex-rs/core/src/codex.rs, codex-rs/core/src/codex/handlers.rs |
| `55c3de75cb` | Plugin | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 10/14；关键文件 MODULE.bazel.lock, codex-rs/Cargo.lock |
| `d9c71d41a9` | Core | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/2；关键文件 codex-rs/core/src/hook_runtime.rs, codex-rs/otel/src/metrics/names.rs |
| `6a1ddfc366` | AppServer | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/6；关键文件 codex-rs/app-server/tests/suite/v2/realtime_conversation.rs, codex-rs/codex-api/src/endpoint/realtime_websocket/methods.rs |
| `fa5d14e276` | Plugin | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/8；关键文件 codex-rs/tui/src/bottom_pane/bottom_pane_view.rs, codex-rs/tui/src/bottom_pane/list_selection_view.rs |
| `a1736fcd20` | Core | **暂缓** | 中 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/2；关键文件 codex-rs/core/src/codex.rs, codex-rs/core/src/codex/turn.rs |
| `65cc12d72e` | Core | **不吸纳** | 低 | guardian 自动化策略调整，和本仓流程不对齐。 重叠 0/1；关键文件 codex-rs/core/src/guardian/mod.rs |
| `bf6e7e12aa` | MCP/Tooling | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 0/1；关键文件 codex-rs/app-server/tests/suite/v2/mcp_resource.rs |
| `5818ed6660` | Plugin | **暂缓** | 低 | 插件交互流程重塑，和现有 fork 插件体验存在冲突风险。 重叠 1/4；关键文件 codex-rs/cli/src/main.rs, codex-rs/cli/tests/marketplace_add.rs |
| `0d0abe839a` | Sandbox/Policy | **选择性吸纳** | 中 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 8/21；关键文件 codex-rs/Cargo.lock, codex-rs/config/src/permissions_toml.rs |
| `2967900d81` | Sandbox/Policy | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/3；关键文件 codex-rs/core/tests/suite/deprecation_notice.rs, codex-rs/features/src/lib.rs |
| `9effa0509f` | TUI | **暂缓** | 中 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 9/30；关键文件 codex-rs/Cargo.lock, codex-rs/app-server-protocol/Cargo.toml |
| `7995c66032` | MCP/Tooling | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 8/20；关键文件 codex-rs/apply-patch/src/lib.rs, codex-rs/apply-patch/src/parser.rs |
| `91e8eebd03` | MCP/Tooling | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/5；关键文件 codex-rs/core/src/codex.rs, codex-rs/core/src/codex/mcp.rs |
| `a803790a10` | MCP/Tooling | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 5/45；关键文件 codex-rs/Cargo.lock, codex-rs/Cargo.toml |
| `37161bc76e` | Plugin | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 1/13；关键文件 codex-rs/app-server/tests/suite/v2/plugin_list.rs, codex-rs/app-server/tests/suite/v2/plugin_read.rs |
| `dd00efe781` | Plugin | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/4；关键文件 codex-rs/core/src/plugins/discoverable.rs, codex-rs/core/src/plugins/discoverable_tests.rs |
| `9d6f4f2e2e` | AppServer | **选择性吸纳** | 高 | 行为修复价值高，但触达 app-server/protocol 深改区，需语义移植。 重叠 1/1；关键文件 codex-rs/app-server/src/codex_message_processor.rs |
| `fe7c959e90` | Sandbox/Policy | **推进吸纳** | 中 | exec-policy 解析修复，直接影响权限策略正确性。 重叠 1/2；关键文件 codex-rs/core/src/exec_policy.rs, codex-rs/core/src/exec_policy_tests.rs |
| `22f7ef1cb7` | TUI | **推进吸纳** | 低 | logout 撤销 ChatGPT token，安全收益明确。 重叠 3/13；关键文件 codex-rs/app-server/src/codex_message_processor.rs, codex-rs/cli/src/login.rs |
| `2e038e6d38` | Sandbox/Policy | **不吸纳** | 低 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 0/1；关键文件 codex-rs/core/src/exec_policy_tests.rs |
| `64177aaa22` | Core | **推进吸纳** | 中 | 收紧 writable root，安全护栏增强。 重叠 1/2；关键文件 codex-rs/core/src/memories/phase2.rs, codex-rs/core/src/memories/tests.rs |
| `20b4b80426` | Plugin | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 11/21；关键文件 codex-rs/app-server-protocol/schema/json/ServerNotification.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `d0047de7cb` | MCP/Tooling | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/6；关键文件 codex-rs/core/config.schema.json, codex-rs/core/src/codex_tests.rs |
| `8494e5bd7b` | TUI | **选择性吸纳** | 中 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 18/43；关键文件 codex-rs/analytics/src/events.rs, codex-rs/app-server-protocol/schema/json/ServerNotification.json |
| `3421a107e0` | Core | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/2；关键文件 codex-rs/core/src/memories/phase2.rs, codex-rs/core/src/memories/tests.rs |
| `c3ecb557d3` | TUI | **推进吸纳** | 高 | resume picker 快捷键能力补齐，低风险高可用。 重叠 1/1；关键文件 codex-rs/tui/src/resume_picker.rs |
| `2dd6734dd3` | TUI | **不吸纳** | 低 | 当前 fork 缺少 `codex-rs/tui/src/terminal_title.rs` 这一实现落点，不能机械移植。 若未来恢复独立终端标题模块，可再按 BEL 终止思路语义吸纳。 |
| `dae0608c06` | TUI | **选择性吸纳** | 中 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 3/10；关键文件 codex-rs/app-server/src/config_api.rs, codex-rs/cloud-requirements/src/lib.rs |
| `71e4c6fa17` | MCP/Tooling | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 29/98；关键文件 codex-rs/core/src/agent/agent_resolver.rs, codex-rs/core/src/agent/control.rs |
| `d0eff70383` | Core | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 codex-rs/core/src/config_loader/tests.rs |
| `af7b8d551c` | TUI | **不吸纳** | 低 | 上游自动审查流转策略，不属于本仓目标域。 重叠 6/21；关键文件 codex-rs/app-server-protocol/schema/json/ServerNotification.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `cfc23eee3d` | MCP/Tooling | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 5/14；关键文件 codex-rs/config/src/key_aliases.rs, codex-rs/config/src/lib.rs |
| `ea84537369` | Core | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/2；关键文件 codex-rs/core/src/connectors.rs, codex-rs/core/src/connectors_tests.rs |
| `d3692b14c9` | TUI | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 4/9；关键文件 AGENTS.md, codex-rs/tui/src/app.rs |
| `fad3d0f1d0` | AppServer | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/6；关键文件 codex-rs/app-server/src/codex_message_processor.rs, codex-rs/app-server/tests/suite/v2/thread_read.rs |
| `6991be7ead` | MCP/Tooling | **选择性吸纳** | 低 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 2/11；关键文件 codex-rs/app-server/README.md, codex-rs/core/src/tools/handlers/tool_search.rs |
| `2c2ed51876` | CI/Docs | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/3；关键文件 .github/scripts/run-bazel-ci.sh, .github/workflows/bazel.yml |
| `481ba014a7` | CI/Docs | **不吸纳** | 低 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 0/1；关键文件 .github/CODEOWNERS |
| `29bc2ad2f4` | CI/Docs | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 1/2；关键文件 .github/actions/prepare-bazel-ci/action.yml, .github/workflows/bazel.yml |
| `eaf78e43f2` | MCP/Tooling | **暂缓** | 中 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 23/54；关键文件 codex-rs/app-server-protocol/schema/json/ClientRequest.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `9d3a5cf05e` | Core | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/7；关键文件 codex-rs/core/src/unified_exec/process_tests.rs, codex-rs/exec-server/src/client.rs |
| `a801b999ff` | Core | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/1；关键文件 codex-rs/core/models.json |
| `0f0ef094b6` | TUI | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 3/5；关键文件 codex-rs/tui/src/chatwidget.rs, codex-rs/tui/src/chatwidget/tests/status_command_tests.rs |
| `d8b91f5fa1` | Misc | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 .codex/skills/babysit-pr/SKILL.md |
| `92cf90277d` | MCP/Tooling | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 1/6；关键文件 codex-rs/codex-mcp/src/mcp_connection_manager.rs, codex-rs/rmcp-client/src/lib.rs |
| `48f117d0a2` | TUI | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/2；关键文件 codex-rs/tui/src/app.rs, codex-rs/tui/src/app_event.rs |
| `f017a23835` | Plugin | **暂缓** | 低 | 插件交互流程重塑，和现有 fork 插件体验存在冲突风险。 重叠 2/8；关键文件 codex-rs/tui/src/bottom_pane/mod.rs, codex-rs/tui/src/chatwidget.rs |
| `139fa8b8f2` | TUI | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 8/27；关键文件 codex-rs/app-server-protocol/schema/json/ServerNotification.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `63e4a900c9` | Sandbox/Policy | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/3；关键文件 codex-rs/exec-server/src/fs_sandbox.rs, codex-rs/exec-server/tests/common/exec_server.rs |
| `ecc8599c56` | Misc | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/1；关键文件 codex-rs/connectors/src/lib.rs |
| `1265df0ec2` | MCP/Tooling | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 6/11；关键文件 codex-rs/app-server/src/bespoke_event_handling.rs, codex-rs/app-server/src/codex_message_processor.rs |
| `0e111e08d0` | Plugin | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 6/13；关键文件 codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.v2.schemas.json |
| `c9c4caafd8` | Misc | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 2/5；关键文件 codex-rs/Cargo.lock, codex-rs/code-mode/Cargo.toml |
| `f705f42ba8` | Sandbox/Policy | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/5；关键文件 codex-rs/core/src/session/turn_context.rs, codex-rs/core/src/tools/runtimes/apply_patch.rs |
| `680c4102ae` | CI/Docs | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/9；关键文件 MODULE.bazel, MODULE.bazel.lock |
| `96d35dd640` | TUI | **不吸纳** | 中 | CI/抖动治理类提交，当前分支门禁已覆盖。 重叠 2/4；关键文件 codex-rs/app-server/BUILD.bazel, codex-rs/core/BUILD.bazel |
| `120bbf46c1` | Core | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/2；关键文件 codex-rs/core/tests/suite/view_image.rs, codex-rs/utils/image/src/lib.rs |
| `26d9894a27` | Plugin | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 11/31；关键文件 codex-rs/app-server-protocol/schema/json/ClientRequest.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `06f8ec54db` | Plugin | **暂缓** | 中 | 插件交互流程重塑，和现有 fork 插件体验存在冲突风险。 重叠 3/8；关键文件 codex-rs/tui/src/app.rs, codex-rs/tui/src/app_event.rs |
| `370bed4bf4` | TUI | **选择性吸纳** | 低 | 安全策略增强建议推进，但必须映射到现有策略模型。 重叠 3/12；关键文件 codex-rs/app-server/src/lib.rs, codex-rs/app-server/tests/suite/v2/thread_start.rs |
| `93ff798e5b` | TUI | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 4/14；关键文件 codex-rs/config/src/types.rs, codex-rs/core/config.schema.json |
| `a58a0f083d` | Misc | **选择性吸纳** | 高 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 1/1；关键文件 codex-rs/protocol/src/models.rs |
| `3f7222ec76` | TUI | **选择性吸纳** | 中 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 13/26；关键文件 codex-rs/Cargo.lock, codex-rs/app-server-protocol/schema/json/ServerNotification.json |
| `def6467d2b` | Plugin | **选择性吸纳** | 中 | 能力有价值，但与 fork 自研插件/MCP 路径耦合，需要兼容层。 重叠 1/3；关键文件 codex-rs/app-server/tests/suite/v2/plugin_read.rs, codex-rs/core/src/plugins/manager.rs |
| `6b39d0c657` | MCP/Tooling | **暂缓** | 高 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 12/19；关键文件 codex-rs/app-server-protocol/schema/json/ClientRequest.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `e9c70fff3f` | Plugin | **暂缓** | 低 | 插件交互流程重塑，和现有 fork 插件体验存在冲突风险。 重叠 2/7；关键文件 codex-rs/cli/src/main.rs, codex-rs/cli/src/marketplace_cmd.rs |
| `5bb193aa88` | MCP/Tooling | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 5/17；关键文件 codex-rs/app-server/tests/common/models_cache.rs, codex-rs/codex-api/tests/models_integration.rs |
| `e3c2acb9cd` | Core | **选择性吸纳** | 低 | 功能可评估推进，但与 fork 现状存在耦合，建议分批语义吸纳。 重叠 0/4；关键文件 codex-rs/core/src/session/turn.rs, codex-rs/core/tests/suite/pending_input.rs |
| `53b1570367` | MCP/Tooling | **暂缓** | 低 | 默认成本/体验策略变更，需产品与成本口径先确认。 重叠 9/35；关键文件 codex-rs/app-server-protocol/schema/json/ClientRequest.json, codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json |
| `e3f44ca3b3` | Plugin | **选择性吸纳** | 低 | 当前分支已在共享路径层修复“绝对路径仍依赖 cwd”的根因，并补上 `PluginStore::try_new()` 入口；但未整体迁移上游 `core-plugins` 架构与完整 store API。 现阶段已消除主要 panic 根因，剩余属于结构对齐而非紧急缺陷。 |
| `996aa23e4c` | MCP/Tooling | **暂缓** | 低 | 涉及核心架构或协议面重排，需单独迁移窗口和兼容设计。 重叠 5/31；关键文件 codex-rs/Cargo.lock, codex-rs/app-server/src/codex_message_processor.rs |

## 执行门禁（防冲突）
- Plugin/MCP：先做 schema 与配置兼容层，不直接替换现有管理器主链。
- AppServer/Protocol：新增字段默认可选，禁止破坏既有 RPC 契约。
- TUI：仅先推进修复类；交互重塑类（tabbed menu、clear-context plan）保持暂缓。
- Sandbox/Policy：只吸纳“收紧型/修复型”，拒绝放宽型策略。
