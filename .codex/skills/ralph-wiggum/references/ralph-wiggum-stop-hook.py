#!/usr/bin/env python3

import json
import os
import re
import sys
from dataclasses import dataclass
from typing import Optional


STATE_FILE = os.path.join(".claude", "ralph-loop.local.md")


@dataclass
class RalphState:
    iteration: int
    max_iterations: int
    completion_promise: Optional[str]
    prompt_text: str


def _warn(msg: str) -> None:
    sys.stderr.write(msg.rstrip() + "\n")


def _read_hook_input() -> dict:
    raw = sys.stdin.read()
    if not raw.strip():
        return {}
    try:
        return json.loads(raw)
    except Exception as exc:  # noqa: BLE001
        _warn(f"⚠️  Ralph loop: invalid hook input JSON ({exc})")
        return {}


def _extract_frontmatter(text: str) -> Optional[tuple[str, str]]:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return None
    try:
        end = next(i for i in range(1, len(lines)) if lines[i].strip() == "---")
    except StopIteration:
        return None
    front = "\n".join(lines[1:end])
    body = "\n".join(lines[end + 1 :])
    return front, body


def _parse_int(front: str, key: str) -> Optional[int]:
    m = re.search(rf"(?m)^{re.escape(key)}:\s*(\d+)\s*$", front)
    if not m:
        return None
    try:
        return int(m.group(1))
    except ValueError:
        return None


def _parse_str(front: str, key: str) -> Optional[str]:
    m = re.search(rf"(?m)^{re.escape(key)}:\s*(.+?)\s*$", front)
    if not m:
        return None
    value = m.group(1).strip()
    if value.lower() == "null":
        return None
    if len(value) >= 2 and value[0] == value[-1] == '"':
        value = value[1:-1]
    value = value.strip()
    return value or None


def _load_state() -> Optional[RalphState]:
    if not os.path.exists(STATE_FILE):
        return None
    try:
        raw = open(STATE_FILE, "r", encoding="utf-8").read()
    except OSError as exc:
        _warn(f"⚠️  Ralph loop: failed to read state file ({exc}); stopping loop")
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass
        return None

    fm = _extract_frontmatter(raw)
    if fm is None:
        _warn("⚠️  Ralph loop: state file missing/unterminated frontmatter; stopping loop")
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass
        return None

    front, body = fm
    iteration = _parse_int(front, "iteration")
    max_iterations = _parse_int(front, "max_iterations")
    completion_promise = _parse_str(front, "completion_promise")

    if iteration is None or max_iterations is None:
        _warn("⚠️  Ralph loop: state file corrupted (iteration/max_iterations); stopping loop")
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass
        return None

    prompt_text = body.strip("\n")
    if not prompt_text.strip():
        _warn("⚠️  Ralph loop: state file has no prompt text; stopping loop")
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass
        return None

    return RalphState(
        iteration=iteration,
        max_iterations=max_iterations,
        completion_promise=completion_promise,
        prompt_text=prompt_text,
    )


def _write_state(state: RalphState) -> None:
    completion = "null" if state.completion_promise is None else json.dumps(state.completion_promise)
    text = (
        "---\n"
        f"iteration: {state.iteration}\n"
        f"max_iterations: {state.max_iterations}\n"
        f"completion_promise: {completion}\n"
        "---\n"
        f"{state.prompt_text}\n"
    )
    os.makedirs(os.path.dirname(STATE_FILE), exist_ok=True)
    tmp = f"{STATE_FILE}.tmp.{os.getpid()}"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, STATE_FILE)


def main() -> int:
    state = _load_state()
    if state is None:
        return 0

    if state.max_iterations > 0 and state.iteration >= state.max_iterations:
        _warn(f"🛑 Ralph loop: max iterations ({state.max_iterations}) reached; stopping loop")
        try:
            os.remove(STATE_FILE)
        except OSError:
            pass
        return 0

    hook_input = _read_hook_input()
    last_msg = hook_input.get("last_assistant_message") or ""

    if state.completion_promise:
        token = f"<promise>{state.completion_promise}</promise>"
        if token in last_msg:
            _warn(f"✅ Ralph loop: detected {token}; stopping loop")
            try:
                os.remove(STATE_FILE)
            except OSError:
                pass
            return 0

    next_iteration = state.iteration + 1
    state.iteration = next_iteration
    _write_state(state)

    if state.completion_promise:
        sys_msg = (
            f"🔄 Ralph iteration {next_iteration} | To stop: output <promise>{state.completion_promise}</promise> "
            "(ONLY when true; do not lie to exit)"
        )
    else:
        sys_msg = f"🔄 Ralph iteration {next_iteration} | No completion promise set - loop runs infinitely"

    out = {
        "decision": "block",
        "reason": state.prompt_text,
        "systemMessage": sys_msg,
    }
    sys.stdout.write(json.dumps(out, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
