# Deny-Read Glob Semantics Design

**目标**

为当前分支补齐 `deny-read glob` 的前置语义层，使配置与协议可以表达 glob 形态的 unreadable rule，并携带 `glob_scan_max_depth`，同时保持现有 runtime enforcement 不变。

**现状**

- 当前分支的 `FileSystemSandboxPolicy` 只有：
  - `kind`
  - `entries`
- 当前分支的 `FileSystemPath` 只有：
  - `Path`
  - `Special`
- 当前分支的 `[permissions.<profile>.filesystem]` 只支持路径映射，不支持：
  - glob 形态路径
  - `glob_scan_max_depth`
- 上游 `0d0abe839a` 把语义层和平台 enforcement 一起推进了：
  - `core/src/config/permissions.rs`
  - `protocol/src/permissions.rs`
  - `linux-sandbox/src/bwrap.rs`
  - `sandboxing/src/seatbelt.rs`
- 当前仓没有上游的 `codex-rs/sandboxing` crate，只有 `codex-rs/linux-sandbox`，因此不能直接机械移植。

**问题定义**

如果直接吸纳上游 `0d0abe839a`：

1. 协议层缺少 glob 路径与 `glob_scan_max_depth`，无法承载配置语义。
2. config 编译层缺少对 glob 的解析、校验与 warning 逻辑。
3. 运行时层没有对应的 Linux/macOS enforcement 落点，容易把“可表达”误当成“已生效”。

因此本轮必须先补一层“只增加表达能力，不改变 enforcement”的最小依赖。

**方案**

采用“两层解耦”的最小方案：

1. 在 `codex-rs/protocol/src/permissions.rs` 扩展协议表达能力：
   - 为 `FileSystemSandboxPolicy` 增加 `glob_scan_max_depth: Option<usize>`
   - 为 `FileSystemPath` 增加 glob 形态分支，保留原有 `Path` / `Special` 语义不变
   - 在 `default()` / `restricted()` / `unrestricted()` / `external_sandbox()` 和 `From<&SandboxPolicy>` 路径上保持该字段的默认值与透传行为一致
2. 在 `codex-rs/core/src/config/permissions.rs` 扩展配置编译能力：
   - `FilesystemPermissionsToml` 增加 `glob_scan_max_depth: Option<usize>`
   - `compile_permission_profile()` 编译后把该值写入 `FileSystemSandboxPolicy`
   - 当 unreadable 规则包含递归 glob（`**`）且未设置 `glob_scan_max_depth` 时，返回 startup warning
3. 在 `codex-rs/core/src/config/mod.rs` 把上述 warning 接入现有 `startup_warnings` 链路，保持失败显式、告警可见。

**语义边界**

- 本轮新增的是“表达与告警”：
  - config 可写
  - protocol 可携带
  - core 可编译
  - startup warnings 可提示
- 本轮不新增“运行时 enforcement”：
  - 不在 `linux-sandbox/src/bwrap.rs` 做 glob 展开或 deny mask
  - 不新增 macOS seatbelt deny 规则
  - 不宣称 glob deny-read 已在当前 runtime 中强制生效

**推荐的 glob 口径**

- 仅把包含通配符的绝对路径字符串编译为 glob path。
- 非 glob 的绝对路径与 special path 继续走现有分支。
- 本轮只要求“可表达 + 可识别 + 可告警”，不要求在 protocol 层做 glob 匹配求值。

**错误处理**

- 非法路径：继续显式报 `InvalidInput`
- 非法 special path 嵌套：继续沿用现有错误
- 非法 glob 路径：
  - 不是绝对路径
  - 含不支持的 special path 组合
  - 编译失败时直接报错，不做静默回退
- 缺少 `glob_scan_max_depth`：
  - 不是错误
  - 只在存在 `**` unreadable glob 时发 warning

**测试**

- `codex-rs/protocol/src/permissions.rs`
  - 新增 unit tests，覆盖：
    - `restricted()` 保留 `glob_scan_max_depth`
    - `default()` / `unrestricted()` / `external_sandbox()` 的默认行为
    - glob path 不参与当前 `get_unreadable_roots_with_cwd()` 的绝对路径解析
- `codex-rs/core/src/config/config_tests.rs`
  - 覆盖 TOML 反序列化 `glob_scan_max_depth`
  - 覆盖 compiled `FileSystemSandboxPolicy` 带上 `glob_scan_max_depth`
  - 覆盖 `**` unreadable glob 且未设置 `glob_scan_max_depth` 时产生 startup warning
  - 覆盖设置了 `glob_scan_max_depth` 后无该 warning

**影响范围**

- `codex-rs/protocol/src/permissions.rs`
- `codex-rs/core/src/config/permissions.rs`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/core/src/config/config_tests.rs`
- `codex-rs/core/config.schema.json`（如果 schema 导出反映该新字段）

**明确不做**

- 不直接吸纳上游 `0d0abe839a` 的 Linux runtime enforcement
- 不补 `codex-rs/sandboxing` crate
- 不改现有非 glob permissions profile 语义
- 不引入新的配置降级路径或隐式 fallback
