#!/usr/bin/env python3

from __future__ import annotations

import argparse
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class CoverageHit:
    hits: int


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check changed-line coverage against one or more lcov.info files."
    )
    parser.add_argument(
        "--lcov",
        action="append",
        required=True,
        help="Path to lcov.info (repeatable).",
    )
    parser.add_argument(
        "--threshold",
        type=float,
        default=99.0,
        help="Minimum percent coverage required for changed executable lines (default: 99.0).",
    )
    parser.add_argument(
        "--git-diff-args",
        nargs=argparse.REMAINDER,
        help=(
            "Args passed to `git diff` (default: check both unstaged and staged diffs with -U0). "
            "Example: --git-diff-args origin/main...HEAD"
        ),
    )
    args = parser.parse_args()

    repo_root = git_repo_root()
    lcov_paths = [Path(p) for p in args.lcov]
    for path in lcov_paths:
        if not path.is_file():
            print(f"ERROR: lcov file not found: {path}", file=sys.stderr)
            return 2

    changed_lines = parse_changed_lines(git_diff_u0(args.git_diff_args, cached=False))
    merge_changed_lines(
        changed_lines, parse_changed_lines(git_diff_u0(args.git_diff_args, cached=True))
    )
    if not changed_lines:
        print("No changed lines found in git diff.")
        return 0

    coverage_by_file = load_lcov_coverage(lcov_paths, repo_root)
    result = compute_diff_coverage(changed_lines, coverage_by_file)
    print(format_summary(result))

    if result.total_measurable_lines == 0:
        return 0
    return 0 if result.percent >= args.threshold else 1


def git_repo_root() -> Path:
    stdout = subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], text=True
    ).strip()
    return Path(stdout)


def git_diff_u0(git_diff_args: list[str] | None, cached: bool) -> str:
    cmd = ["git", "diff", "--no-color", "-U0"]
    if cached:
        cmd.append("--cached")
    if git_diff_args:
        cmd.extend(git_diff_args)
    return subprocess.check_output(cmd, text=True)


def parse_changed_lines(diff_text: str) -> dict[str, set[int]]:
    out: dict[str, set[int]] = {}
    current_file: str | None = None
    new_line_no: int | None = None

    for raw in diff_text.splitlines():
        if raw.startswith("+++ "):
            path = raw.removeprefix("+++ ").strip()
            if path == "/dev/null":
                current_file = None
                continue
            if path.startswith("b/"):
                path = path[2:]
            current_file = path
            out.setdefault(current_file, set())
            continue

        if raw.startswith("@@ "):
            if current_file is None:
                continue
            # @@ -oldStart,oldLen +newStart,newLen @@
            try:
                header = raw.split("@@")[1].strip()
                plus = next(part for part in header.split() if part.startswith("+"))
                plus = plus[1:]
                start = plus.split(",")[0]
                new_line_no = int(start)
            except Exception:
                new_line_no = None
            continue

        if current_file is None or new_line_no is None:
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            out[current_file].add(new_line_no)
            new_line_no += 1
        elif raw.startswith("-") and not raw.startswith("---"):
            continue
        elif raw.startswith(" "):
            new_line_no += 1
        elif raw.startswith("\\"):
            continue
        else:
            continue

    return out


def merge_changed_lines(
    target: dict[str, set[int]], other: dict[str, set[int]]
) -> None:
    for file_path, lines in other.items():
        existing = target.get(file_path)
        if existing is None:
            target[file_path] = set(lines)
        else:
            existing.update(lines)


def load_lcov_coverage(
    lcov_paths: Iterable[Path],
    repo_root: Path,
) -> dict[str, dict[int, CoverageHit]]:
    out: dict[str, dict[int, CoverageHit]] = {}
    for lcov_path in lcov_paths:
        for file_path, hits_by_line in parse_lcov_file(lcov_path, repo_root).items():
            existing = out.setdefault(file_path, {})
            for lineno, hit in hits_by_line.items():
                prev = existing.get(lineno)
                if prev is None:
                    existing[lineno] = hit
                else:
                    existing[lineno] = CoverageHit(hits=prev.hits + hit.hits)
    return out


def parse_lcov_file(
    lcov_path: Path,
    repo_root: Path,
) -> dict[str, dict[int, CoverageHit]]:
    out: dict[str, dict[int, CoverageHit]] = {}
    current_file: str | None = None

    for raw in lcov_path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw.strip()
        if line.startswith("SF:"):
            sf = line.removeprefix("SF:").strip()
            resolved = resolve_sf_path(sf, lcov_path, repo_root)
            if resolved is None:
                current_file = None
                continue
            current_file = resolved
            out.setdefault(current_file, {})
            continue

        if line.startswith("DA:") and current_file is not None:
            # DA:<line>,<hit>[,<checksum>]
            payload = line.removeprefix("DA:")
            parts = payload.split(",")
            if len(parts) < 2:
                continue
            try:
                lineno = int(parts[0])
                hits = int(parts[1])
            except ValueError:
                continue
            out[current_file][lineno] = CoverageHit(hits=hits)
            continue

        if line == "end_of_record":
            current_file = None

    return out


def resolve_sf_path(sf: str, lcov_path: Path, repo_root: Path) -> str | None:
    sf_path = Path(sf)
    candidates: list[Path] = []
    if sf_path.is_absolute():
        candidates.append(sf_path)
    else:
        candidates.append(repo_root / sf_path)
        candidates.append(lcov_path.parent / sf_path)
        if lcov_path.parent.parent != lcov_path.parent:
            candidates.append(lcov_path.parent.parent / sf_path)

    resolved: Path | None = None
    for candidate in candidates:
        if candidate.exists():
            resolved = candidate.resolve()
            break
    if resolved is None:
        return None

    try:
        return resolved.relative_to(repo_root).as_posix()
    except ValueError:
        return resolved.as_posix()


@dataclass(frozen=True)
class DiffCoverageResult:
    total_measurable_lines: int
    covered_lines: int
    per_file: dict[str, tuple[int, int]]  # file -> (covered, total)
    missing_by_file: dict[str, list[int]]

    @property
    def percent(self) -> float:
        if self.total_measurable_lines == 0:
            return 100.0
        return self.covered_lines * 100.0 / self.total_measurable_lines


def compute_diff_coverage(
    changed_lines: dict[str, set[int]],
    coverage_by_file: dict[str, dict[int, CoverageHit]],
) -> DiffCoverageResult:
    total = 0
    covered = 0
    per_file: dict[str, tuple[int, int]] = {}
    missing_by_file: dict[str, list[int]] = {}

    for file_path, lines in sorted(changed_lines.items()):
        hits_by_line = coverage_by_file.get(file_path)
        if not hits_by_line:
            continue

        file_total = 0
        file_covered = 0
        missing: list[int] = []
        for line_no in sorted(lines):
            hit = hits_by_line.get(line_no)
            if hit is None:
                continue
            file_total += 1
            if hit.hits > 0:
                file_covered += 1
            else:
                missing.append(line_no)

        if file_total == 0:
            continue
        per_file[file_path] = (file_covered, file_total)
        missing_by_file[file_path] = missing
        total += file_total
        covered += file_covered

    return DiffCoverageResult(
        total_measurable_lines=total,
        covered_lines=covered,
        per_file=per_file,
        missing_by_file=missing_by_file,
    )


def format_summary(result: DiffCoverageResult) -> str:
    lines: list[str] = []
    lines.append(
        f"Diff coverage: {result.covered_lines}/{result.total_measurable_lines} = {result.percent:.2f}%"
    )
    if not result.per_file:
        lines.append("No measurable changed lines were found in the provided lcov files.")
        return "\n".join(lines)

    lines.append("")
    lines.append("Per-file:")
    for file_path, (covered, total) in result.per_file.items():
        pct = 100.0 if total == 0 else covered * 100.0 / total
        suffix = ""
        missing = result.missing_by_file.get(file_path) or []
        if missing:
            preview = ", ".join(str(n) for n in missing[:10])
            extra = "" if len(missing) <= 10 else f", +{len(missing) - 10} more"
            suffix = f" (missing: {preview}{extra})"
        lines.append(f"  {file_path}: {covered}/{total} = {pct:.2f}%{suffix}")

    return "\n".join(lines)


if __name__ == "__main__":
    sys.exit(main())
