# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Custom model providers

You can define custom providers in `~/.codex/config.toml` and select them via `model_provider`.

Example Anthropic provider:

```toml
[model_providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic"

model_provider = "anthropic"
model = "claude-sonnet-4-5"
```

You can also set provider overrides inside agent role config files (for example `~/.codex/agents/researcher.toml`):

```toml
model_provider = "anthropic"

[model_providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic"
```

Before running Codex, set your key in the environment:

```bash
export ANTHROPIC_API_KEY="..."
```

## Connecting to MCP servers

Codex can connect to MCP servers configured in `~/.codex/config.toml`. See the configuration reference for the latest MCP server options:

- https://developers.openai.com/codex/config-reference

## Apps (Connectors)

Use `$` in the composer to insert a ChatGPT connector; the popover lists accessible
apps. The `/apps` command lists available and installed apps. Connected apps appear first
and are labeled as connected; others are marked as can be installed.

## Hooks

Codex can run hooks at lifecycle boundaries such as `session_start`, `session_end`, `user_prompt_submit`, `pre_tool_use`, `permission_request`, `notification`, `post_tool_use`, `post_tool_use_failure`, `stop`, `subagent_start`, `subagent_stop`, `teammate_idle`, `task_completed`, `config_change`, and `pre_compact`.

Example:

```toml
[hooks]

[[hooks.pre_tool_use]]
command = ["python3", "/Users/me/.codex/hooks/check_tool.py"]
timeout = 5
once = true

[hooks.pre_tool_use.matcher]
tool_name_regex = "^(shell|exec)$"
```

Hooks receive a JSON payload on `stdin`. If the hook exits with code `0`, Codex will attempt to parse a JSON object from `stdout` (either the full output or the first parseable JSON line). Exit code `2` blocks execution for hook events that support blocking; other non-zero exit codes are treated as non-blocking errors. All matching hooks run in parallel and identical handlers are deduplicated.

`command` can be either an argv list (`["python3", "..."]`) or a shell command string (`"python3 ..."`). Matchers can filter by `matcher`, and tool events can also filter by `tool_name` / `tool_name_regex`.

See `docs/hooks.md` for hook payload fields and `stdout` response options.

Project hooks can also be configured in `./.codex/config.toml`. If the project directory is untrusted, project layers may load as disabled; mark it trusted via your user config (for example, `[projects."/abs/path"].trust_level = "trusted"`).

See the configuration reference for the latest hook settings:

- https://developers.openai.com/codex/config-reference

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## Notices

Codex stores "do not show again" flags for some UI prompts under the `[notice]` table.

## Plan mode defaults

`plan_mode_reasoning_effort` lets you set a Plan-mode-specific default reasoning
effort override. When unset, Plan mode uses the built-in Plan preset default
(currently `medium`). When explicitly set (including `none`), it overrides the
Plan preset. The string value `none` means "no reasoning" (an explicit Plan
override), not "inherit the global default". There is currently no separate
config value for "follow the global default in Plan mode".

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
