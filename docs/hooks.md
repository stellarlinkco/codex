# Hooks

Hooks let Codex run user-defined handlers (command / prompt / agent) when specific lifecycle
events fire. They are useful for policy checks (for example, blocking a risky tool call),
for harness-style loops (for example, blocking `stop` until a condition is met), and for
injecting additional context into the next model turn.

This document describes:

- Where hooks can be configured
- How hooks execute (parallelism, deduplication, blocking)
- The hook payload contract (stdin JSON)
- The hook output contract (stdout JSON)

## Where to configure

### Global/project hooks (`config.toml`)

Command hooks can be configured in `config.toml` under the `[hooks]` table.

- User config: `~/.codex/config.toml`
- Project config: `./.codex/config.toml` (searched upward to the project root)

If a project directory is untrusted, Codex will still discover `./.codex/config.toml` but load
it as a disabled layer. To trust a project, add an entry in your user config:

```toml
[projects."/absolute/path/to/project"]
trust_level = "trusted"
```

### Skill-scoped hooks (`SKILL.md` frontmatter)

Skills may define hook handlers in the YAML frontmatter of `SKILL.md` under `hooks:`.

These hooks are installed as **scoped hooks** for the duration of the turn in which the skill
is active, and removed automatically at the end of the turn.

This is the closest analogue to Claude Code’s “hooks in skills”.

## Execution model

When an event fires:

- All **matching** hooks run in parallel.
- **Identical handlers** are deduplicated automatically (so the same handler is only run once for
  that event fire, even if it appears in multiple hook sources or matcher groups).
- Synchronous hooks are awaited before the triggering action proceeds.
- `once = true` runs that handler at most once per Codex session.
- `timeout` applies per handler (seconds). If unset:
  - Command hooks have no timeout.
  - Prompt hooks default to 30s.
  - Agent hooks default to 60s.

### Background (async) hooks

Only command hooks support `async = true`.

An async hook:

- Runs in the background and **never blocks** the triggering action.
- Cannot reliably block, rewrite inputs, or auto-approve permissions (the action already continued).
- Delivers its `additionalContext` / `systemMessage` output on the **next turn** (if the session is
  idle, it waits until the next user interaction).
- Does not deduplicate across repeated firings of the same hook (each fire spawns a new process).

## Handler types

### Command hooks (`type: command`)

Command hooks run a shell command. Codex:

- Writes a single JSON object (the hook payload) to the process `stdin`.
- Reads `stdout` and attempts to parse a JSON object (either the full output or the first parseable
  JSON line).

Exit codes:

- `0`: success; `stdout` JSON (if any) is applied.
- `2`: blocks **blockable events**; `stdout` is ignored and `stderr` becomes the block reason.
- Any other non-zero: treated as a non-blocking error and execution continues.

### Prompt hooks (`type: prompt`)

Prompt hooks send a single-turn evaluation prompt to the model. The prompt can reference the hook
input JSON via `$ARGUMENTS`:

- If the prompt contains `$ARGUMENTS`, Codex replaces it with the serialized input JSON.
- Otherwise Codex appends a `$ARGUMENTS:` section automatically.

The model must return JSON only:

```json
{ "ok": true }
```

or:

```json
{ "ok": false, "reason": "..." }
```

`ok: false` is treated as a blocking decision (for blockable events).

### Agent hooks (`type: agent`)

Agent hooks spawn a verifier subagent that can use tools (Read/Grep/Glob/etc) and must return a
final JSON-only message with the same `{ok, reason}` shape as prompt hooks.

## Matchers

Matchers are optional filters. A matcher is only applied for events that support matching.

### `matcher` (recommended)

`matcher` is a Rust `regex` pattern that matches an event-specific string:

- Tool events (`pre_tool_use`, `permission_request`, `post_tool_use`, `post_tool_use_failure`): `tool_name`
- `session_start`: `source`
- `session_end`: `reason`
- `notification`: `notification_type`
- `subagent_start` / `subagent_stop`: `agent_type`
- `pre_compact`: `trigger`
- `config_change`: `source`

Special case: `matcher = "*"` means “match all” (equivalent to omitting it).

### Tool-only convenience fields

For tool events, you may also use:

- `tool_name` (exact match)
- `tool_name_regex` (Rust regex)

### Events that do not support matchers

These events ignore matchers:

- `user_prompt_submit`
- `stop`
- `teammate_idle`
- `task_completed`
- `worktree_create`, `worktree_remove`

## Hook payload (stdin JSON)

All events share these top-level fields:

- `session_id` (string)
- `transcript_path` (string|null)
- `cwd` (string)
- `permission_mode` (string; for example `on-request`, `on-failure`, `untrusted`, `never`)
- `hook_event_name` (string; PascalCase)

Event-specific fields are flattened at the top level based on `hook_event_name`.

Event payload fields:

- `SessionStart`: `source`, `model`, `agent_type`
- `SessionEnd`: `reason`
- `UserPromptSubmit`: `prompt`
- `PreToolUse`: `tool_name`, `tool_input`, `tool_use_id`
- `PermissionRequest`: `tool_name`, `tool_input`, `tool_use_id`, `permission_suggestions`
- `Notification`: `message`, `title`, `notification_type`
- `PostToolUse`: `tool_name`, `tool_input`, `tool_response`, `tool_use_id`
- `PostToolUseFailure`: `tool_name`, `tool_input`, `tool_use_id`, `error`, `is_interrupt`
- `Stop`: `stop_hook_active`, `last_assistant_message`
- `SubagentStart`: `agent_id`, `agent_type`
- `TeammateIdle`: `teammate_name`, `team_name`
- `TaskCompleted`: `task_id`, `task_subject`, `task_description`, `teammate_name`, `team_name`
- `ConfigChange`: `source`, `file_path`
- `SubagentStop`: `stop_hook_active`, `agent_id`, `agent_type`, `agent_transcript_path`, `last_assistant_message`
- `PreCompact`: `trigger`, `custom_instructions`
- `WorktreeCreate`: `repo_path`, `worktree_path`
- `WorktreeRemove`: `repo_path`, `worktree_path`

Notes on when the multi-agent events fire:

- `SubagentStart`: after `spawn_agent` / `spawn_team` returns a new `agent_id`.
- `TeammateIdle`: after `wait_team` returns a final status for one or more teammates.
- `TaskCompleted`: when `team_task_complete` is called (and can block completion before it is persisted).

## Hook output (stdout JSON)

If the hook exits `0`, it may return a JSON object on `stdout`. Codex recognizes these keys:

- Context injection:
  - `systemMessage` / `system_message` (string)
  - `additionalContext` / `additional_context` (string)
  - `hookSpecificOutput.additionalContext` / `hookSpecificOutput.additional_context` (string)
- Input rewriting:
  - `updatedInput` / `updated_input` (any JSON value; only consumed by `pre_tool_use`)
- Blocking decisions (supported events only):
  - `decision` (string)
    - For `user_prompt_submit`, `stop`, `subagent_stop`, `config_change`: `block|deny|abort` blocks
    - For `pre_tool_use`: `deny|ask` blocks
    - For `permission_request`: `allow|deny|ask` maps to `permissionDecision` behavior
  - `reason` / `stopReason` (string; used when `decision` blocks)
- Permission decisions (`permission_request` and `pre_tool_use`):
  - `permissionDecision` / `permission_decision` (string; `allow|deny|ask`)
  - `permissionDecisionReason` / `permission_decision_reason` (string)
  - `hookSpecificOutput.decision.behavior` (string; `allow|deny|ask`) for `permission_request`

Output precedence when multiple keys are present:

- `updatedInput` prefers top-level over `hookSpecificOutput.updatedInput`.
- Permission decisions prefer `hookSpecificOutput.permissionDecision` over top-level `permissionDecision`.
- Block reason prefers `reason` → `stopReason` → `permissionDecisionReason` → fallback.

## Event capabilities (summary)

- Events that can be blocked (via `exit 2`):
  - `user_prompt_submit`, `pre_tool_use`, `permission_request`
  - `stop`, `subagent_stop`
  - `teammate_idle`, `task_completed`
  - `config_change`
- Events that honor `stdout` decisions (`decision` / `permissionDecision`):
  - `user_prompt_submit`, `pre_tool_use`, `permission_request`, `stop`, `subagent_stop`, `config_change`
- `updatedInput` is only consumed for `pre_tool_use`.
- Permission decisions are consumed for `pre_tool_use` and `permission_request`:
  - `permission_request`: `allow|deny` bypasses the approval UI; `ask` keeps the UI path.
  - `pre_tool_use`: `deny|ask` blocks; `allow` continues.

## Hook configuration examples

### `config.toml` (command hooks)

```toml
[hooks]

[[hooks.pre_tool_use]]
name = "guard-shell"
command = ["python3", "/Users/me/.codex/hooks/pre_tool_use.py"]
timeout = 5
once = false

[hooks.pre_tool_use.matcher]
matcher = "shell"
```

### Skill frontmatter (command/prompt/agent hooks)

In a skill `SKILL.md`, add YAML frontmatter:

```yaml
---
name: ralph-wiggum
description: Block stop until the user promises
hooks:
  Stop:
    - hooks:
        - type: command
          command: "python3 .claude/hooks/ralph-wiggum-stop-hook.py"
---
```

Example: prompt-based guard on tool calls:

```yaml
---
hooks:
  PreToolUse:
    - matcher: "shell|exec_command"
      hooks:
        - type: prompt
          timeout: 15
          prompt: |
            Decide if this tool call is safe. Return JSON only: {"ok": true} or {"ok": false, "reason": "..."}.

            $ARGUMENTS
---
```

## End-to-end sanity check (command hooks)

This is intended to run on your machine (not inside Codex's restricted test sandbox).

1. Create a hook that logs every payload:

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

2. Wire it into `~/.codex/config.toml`:

```toml
[hooks]

[[hooks.session_start]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.pre_tool_use]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""

[[hooks.stop]]
command = "python3 \"$HOME/.codex/hooks/log_event.py\""
```

3. Trigger events and inspect the log:

```bash
: > ~/.codex/hooks/e2e-events.jsonl
codex exec "只回复 E2E_OK"
codex exec "请使用 shell 工具执行：echo hi"
jq -r '.hook_event_name' ~/.codex/hooks/e2e-events.jsonl | sort | uniq -c
```
