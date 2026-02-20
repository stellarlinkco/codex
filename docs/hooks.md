# Hooks

Codex can run user-defined command hooks at key lifecycle boundaries (for example, before a tool runs, or when a turn is about to stop). Hooks are configured in `config.toml` under the `[hooks]` table.

## Where to configure

Hooks can be defined in:

- User config: `~/.codex/config.toml`
- Project config: `./.codex/config.toml` (searched from the current directory up to the project root; the project root is detected via `project_root_markers`, which defaults to `.git`)

Project config precedence is low → high (later layers override earlier ones): project root → … → current directory.

If a project directory is untrusted, Codex will still discover `./.codex/config.toml` but load it as a disabled layer. To trust a project, add an entry in your user config:

```toml
[projects."/absolute/path/to/project"]
trust_level = "trusted"
```

## How hooks start

No separate daemon is required. After you save hook config, start Codex normally:

- Interactive: `codex`
- Non-interactive: `codex exec "your prompt"`

Hooks are loaded at startup and run automatically when matching events occur.

## Hook configuration

Each event key contains a list of hook commands:

```toml
[hooks]

[[hooks.pre_tool_use]]
name = "guard-shell"
command = ["python3", "/Users/me/.codex/hooks/pre_tool_use.py"] # or: "python3 /Users/me/.codex/hooks/pre_tool_use.py"
timeout = 5   # seconds
once = false

[hooks.pre_tool_use.matcher]
tool_name = "shell"
# tool_name_regex = "^(shell|exec)$"
# prompt_regex = "(?i)prod"
```

Notes:

- `command` supports either argv (`["python3", "..."]`) or a shell string (`"python3 ..."`). Shell strings run via `sh -c` (Unix) or `cmd /C` (Windows).
- Hooks run in the order they appear. If a hook blocks an event, later hooks for that event are not run.
- `once = true` runs that configured hook at most once per Codex session.

### Matchers

Matchers are optional filters under `[[hooks.<event>]].matcher`:

- All configured matcher fields are combined with **AND** semantics.
- `tool_name` / `tool_name_regex` only match tool-related events (`pre_tool_use`, `permission_request`, `post_tool_use`, `post_tool_use_failure`).
- `prompt_regex` only matches `user_prompt_submit`.
- Regexes use Rust's `regex` engine (case-sensitive by default; use `(?i)` for case-insensitive matches).

Supported hook event keys:

- `session_start`, `session_end`
- `user_prompt_submit`
- `pre_tool_use`
- `permission_request`
- `post_tool_use`, `post_tool_use_failure`
- `stop`, `subagent_stop`
- `pre_compact`

## Execution model

For each matching hook:

- Codex spawns the configured command with `cwd` set to the payload `cwd`.
- Codex writes a single JSON object to the hook's `stdin`.
- If the hook exits with code `0`, Codex attempts to parse a JSON object from `stdout` (either the full output or the first parseable JSON line).
- Exit code `2` blocks execution for hook events that support blocking (see below). `stdout` is ignored in this case; `stderr` becomes the block reason.
- Any other non-zero exit code is treated as a non-blocking error and execution continues.

Events that support blocking (via `exit 2` or `stdout` decisions): `user_prompt_submit`, `pre_tool_use`, `permission_request`, `stop`, `subagent_stop`, `pre_compact`.

### Failure behavior

- Spawn failures, timeouts, signals, and non-zero exit codes (except `2` on blockable events) are treated as non-blocking errors.
- If `stdout` is not valid JSON, it is ignored.
- Prefer writing logs to `stderr` (so `stdout` can stay machine-readable).

## Hook payload (stdin JSON)

All events share these top-level fields:

- `session_id` (string)
- `transcript_path` (string|null)
- `cwd` (string)
- `permission_mode` (string; for example `on-request`, `on-failure`, `untrusted`, `never`)
- `hook_event_name` (string; PascalCase)

Event-specific fields are flattened at the top level based on `hook_event_name`.

Example (`PreToolUse`):

```json
{
  "session_id": "…",
  "transcript_path": "/path/to/rollout.jsonl",
  "cwd": "/path/to/project",
  "permission_mode": "on-request",
  "hook_event_name": "PreToolUse",
  "tool_name": "shell",
  "tool_input": { "command": ["echo", "hi"] },
  "tool_use_id": "call-123"
}
```

Event payload fields:

- `SessionStart`: `source`, `model`, `agent_type`
- `SessionEnd`: `reason`
- `UserPromptSubmit`: `prompt`
- `PreToolUse`: `tool_name`, `tool_input`, `tool_use_id`
- `PermissionRequest`: `tool_name`, `tool_input`, `tool_use_id`, `permission_suggestions`
- `PostToolUse`: `tool_name`, `tool_input`, `tool_response`, `tool_use_id`
- `PostToolUseFailure`: `tool_name`, `tool_input`, `tool_use_id`, `error`, `is_interrupt`
- `Stop`: `stop_hook_active`, `last_assistant_message`
- `SubagentStop`: `stop_hook_active`, `agent_id`, `agent_type`, `agent_transcript_path`, `last_assistant_message`
- `PreCompact`: `trigger`, `custom_instructions`

## Hook output (stdout JSON)

If the hook exits `0`, it may return a JSON object on `stdout`. Codex recognizes these keys:

- Context injection:
  - `systemMessage` / `system_message` (string)
  - `additionalContext` / `additional_context` (string)
  - `hookSpecificOutput.additionalContext` / `hookSpecificOutput.additional_context` (string)
- Input rewriting:
  - `updatedInput` / `updated_input` (any JSON value; used by `pre_tool_use` to rewrite the tool input)
- Blocking decisions (blockable events only):
  - `decision` (string; one of `block|deny|abort` to block)
  - `reason` / `stopReason` (string; used when `decision` blocks)
- Permission decisions (`permission_request` and `pre_tool_use`):
  - `permissionDecision` / `permission_decision` (string; `allow|deny|ask`)
  - `permissionDecisionReason` / `permission_decision_reason` (string)
  - `hookSpecificOutput.decision.behavior` (string; `allow|deny|ask`) for `permission_request`

Notes:

- `permission_request` can return `permissionDecision=allow|deny` to bypass the approval UI. (`updatedInput` is currently ignored for `permission_request`.)
- `codex exec` currently enforces `approval_policy = "never"` by default, so `permission_request` hooks usually do not fire there. Use interactive `codex` with an approval mode that can ask (for example `on-request`) when validating `permission_request`.
- `post_tool_use` and `post_tool_use_failure` do not support blocking; `decision=block` is ignored.

### Output precedence

If multiple keys are present:

- `updatedInput` prefers top-level (`updatedInput` / `updated_input`) over `hookSpecificOutput.updatedInput`.
- Permission decisions prefer `hookSpecificOutput.permissionDecision` over top-level `permissionDecision`.
- Block reason prefers `reason` → `stopReason` → `permissionDecisionReason` → fallback.

## Event capabilities

| Event | Can block | Consumes `updatedInput` | Consumes `permissionDecision` | Records `additionalContext` |
| --- | --- | --- | --- | --- |
| `session_start` | no | no | no | no |
| `session_end` | no | no | no | no |
| `user_prompt_submit` | yes | no | no | yes |
| `pre_tool_use` | yes | yes | yes (deny/ask blocks) | yes |
| `permission_request` | yes | no | yes | yes |
| `post_tool_use` | no | no | no | yes |
| `post_tool_use_failure` | no | no | no | yes |
| `stop` | yes | no | no | yes |
| `subagent_stop` | yes | no | no | yes |
| `pre_compact` | yes | no | no | yes |

## Minimal examples

Block a stop until the user acknowledges something:

```json
{"decision":"block","reason":"Please confirm before stopping."}
```

Rewrite a tool input (pre-tool hook):

```json
{"updatedInput":{"command":["echo","hello"]}}
```

## End-to-end sanity check

This is intended to run on your machine (not inside Codex's restricted test sandbox).

1) Create a hook that logs every payload:

```bash
mkdir -p ~/.codex/hooks
cat > ~/.codex/hooks/log_event.py <<'PY'
#!/usr/bin/env python3
import json, os, sys, time
path = os.path.expanduser("~/.codex/hooks/e2e-events.jsonl")
payload = json.load(sys.stdin)
with open(path, "a", encoding="utf-8") as f:
  f.write(json.dumps({"ts": time.time(), "hook_event_name": payload.get("hook_event_name"), "payload": payload}) + "\n")
print("{}")
PY
chmod +x ~/.codex/hooks/log_event.py
```

2) (Optional) Create a `permission_request` decision hook (useful for `codex exec`):

```bash
cat > ~/.codex/hooks/permission_decide.py <<'PY'
#!/usr/bin/env python3
import json, os, sys
payload = json.load(sys.stdin)
if payload.get("hook_event_name") != "PermissionRequest":
  print("{}"); raise SystemExit(0)
mode = os.getenv("HOOK_PERMISSION_MODE", "ask").lower()
if mode not in ("allow", "deny", "ask"):
  mode = "ask"
print(json.dumps({"permissionDecision": mode, "permissionDecisionReason": f"hook decided: {mode}"}))
PY
chmod +x ~/.codex/hooks/permission_decide.py
```

3) Wire them into `~/.codex/config.toml`:

```toml
[hooks]

[[hooks.session_start]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.user_prompt_submit]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.pre_tool_use]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.permission_request]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.permission_request]]
command = "python3 \"$HOME/.codex/hooks/permission_decide.py\""

[[hooks.post_tool_use]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.post_tool_use_failure]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.stop]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""
```

4) Trigger events and inspect the log:

```bash
: > ~/.codex/hooks/e2e-events.jsonl
codex exec "只回复 E2E_OK"
codex exec "请使用 shell 工具执行：echo hi"
codex exec "请使用 shell 工具执行一个不存在的命令：__definitely_not_exists_cmd__"
HOOK_PERMISSION_MODE=deny codex exec "请使用 shell 工具执行：touch /tmp/codex_hook_perm_test.txt"
jq -r '.hook_event_name' ~/.codex/hooks/e2e-events.jsonl | sort | uniq -c
```
