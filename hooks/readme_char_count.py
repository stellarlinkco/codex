#!/usr/bin/env python3
from __future__ import annotations

import sys
from pathlib import Path


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def resolve_readme(root: Path) -> Path:
    for name in ("README.md", "README", "readme.md", "Readme.md"):
        candidate = root / name
        if candidate.is_file():
            return candidate
    return root / "README.md"


def main() -> int:
    root = repo_root()
    if len(sys.argv) > 1:
        path = Path(sys.argv[1])
        if not path.is_absolute():
            path = root / path
    else:
        path = resolve_readme(root)

    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        text = path.read_text(encoding="utf-8", errors="replace")

    print(len(text))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
