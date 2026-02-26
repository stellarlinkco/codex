#!/usr/bin/env python3
import json
import os
import sys
import time


def main() -> int:
    payload = json.load(sys.stdin)
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), os.pardir))
    path = os.path.join(repo_root, ".codex", "hooks-e2e-events.jsonl")
    os.makedirs(os.path.dirname(path), exist_ok=True)

    event = {
        "ts": time.time(),
        "hook_event_name": payload.get("hook_event_name"),
        "tool_name": payload.get("tool_name"),
    }
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps({"event": event, "payload": payload}, ensure_ascii=False) + "\n")
    print("{}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

