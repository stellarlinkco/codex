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

Codex can run command hooks at lifecycle boundaries such as `session_start`, `session_end`, `user_prompt_submit`, `pre_tool_use`, `permission_request`, `post_tool_use`, `post_tool_use_failure`, `stop`, `subagent_stop`, and `pre_compact`.

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

Hooks receive a JSON payload on `stdin`. If the hook exits with code `0`, Codex will attempt to parse a JSON object from `stdout` (either the full output or the first parseable JSON line). Exit code `2` blocks execution for hook events that support blocking; other non-zero exit codes are treated as non-blocking errors.

`command` can be either an argv list (`["python3", "..."]`) or a shell command string (`"python3 ..."`). Matchers can filter by `tool_name`, `tool_name_regex`, or `prompt_regex`.

See `docs/hooks.md` for hook payload fields and `stdout` response options.

Project hooks can also be configured in `./.codex/config.toml`. If the project directory is untrusted, project layers may load as disabled; mark it trusted via your user config (for example, `[projects."/abs/path"].trust_level = "trusted"`).

See the configuration reference for the latest hook settings:

- https://developers.openai.com/codex/config-reference

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## Notices

Codex stores "do not show again" flags for some UI prompts under the `[notice]` table.

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
