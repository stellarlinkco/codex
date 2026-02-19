# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Connecting to MCP servers

Codex can connect to MCP servers configured in `~/.codex/config.toml`. See the configuration reference for the latest MCP server options:

- https://developers.openai.com/codex/config-reference

## Apps (Connectors)

Use `$` in the composer to insert a ChatGPT connector; the popover lists accessible
apps. The `/apps` command lists available and installed apps. Connected apps appear first
and are labeled as connected; others are marked as can be installed.

## Hooks

Codex can run command hooks at lifecycle boundaries such as `session_start`, `user_prompt_submit`, `pre_tool_use`, `post_tool_use`, and `stop`.

Example:

```toml
[hooks]

[[hooks.pre_tool_use]]
command = ["python3", "/Users/me/.codex/hooks/check_tool.py"]
timeout_ms = 5000
abort_on_error = true

[hooks.pre_tool_use.matcher]
tool_name_regex = "^shell|exec"
```

For backward compatibility, `notify = ["..."]` is still supported and mapped to turn-complete hooks.

See the configuration reference for the latest hook settings:

- https://developers.openai.com/codex/config-reference

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## Notices

Codex stores "do not show again" flags for some UI prompts under the `[notice]` table.

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
