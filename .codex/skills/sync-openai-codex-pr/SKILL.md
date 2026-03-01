---
name: sync-openai-codex-pr
description: Sync upstream openai/codex into this repo using a dedicated worktree branched from main. Resolve conflicts with local code priority; only block for user choice when two implementations provide the same feature and you must pick one. Push updates and require CI all-green before merge.
---

# Sync openai/codex → PR

## Objective
Pull `openai/codex` `main` into the current repo, push to a new branch, and open/update a PR for review. This is a *sync* workflow: minimal diffs, local code priority, and CI must be green before merge.

## Hard Rules
- **Use a worktree**: do not contaminate the current branch; always branch from `main`.
- **Local priority**: prefer the local repo’s behavior/architecture by default.
- **Conflict policy**:
  - If it’s a *simple merge conflict* (mechanical overlap, moved code, formatting, rename, import order): resolve directly with minimal, correct merging.
  - If it’s a *real functional conflict* but not “same feature twice”: merge to preserve local behavior and incorporate upstream changes where safe.
  - Only when it’s **the same feature implemented in two different ways and you must choose one**, do **not** decide silently: write a blocking PR comment and ask the user to choose local vs upstream.
- **CI gate**: do not merge until required checks are all green.

## Core Workflow

### 1) Create worktree from `main`
From repo root:

```bash
ts="$(date +%Y%m%d-%H%M%S)"
branch="sync/openai-codex-$ts"
path=".worktrees/sync-openai-codex-$ts"
git fetch origin main
git worktree add -b "$branch" "$path" origin/main
```

Work inside the worktree:

```bash
cd "$path"
```

### 2) Fetch upstream and merge

```bash
git remote add openai https://github.com/openai/codex.git 2>/dev/null || true
git fetch openai main
git merge --no-edit openai/main
```

If conflicts appear, *pause merge resolution* and do a quick code-review classification:
- Identify whether the conflict is mechanical vs behavioral.
- For behavioral conflicts, decide whether it’s “merge both” vs “same feature, must pick one”.

### 3) Blocking PR comment only for “same feature, must pick one”
When and only when you find two competing implementations of the same feature:
- Add a PR comment with:
  - Exact file paths + function names
  - What each implementation does (behavioral delta)
  - Pros/cons (correctness, complexity, performance, maintainability)
  - What breaks if you pick the wrong one
  - The decision needed: **keep local** or **take upstream**
- Stop the flow until the user chooses.

### 4) Finish merge, format, and run targeted checks
Rust (after Rust changes):

```bash
cd codex-rs
just fmt
```

Run the narrowest relevant tests first (examples):

```bash
cargo test -p codex-core
```

If `Cargo.toml` / `Cargo.lock` changed:

```bash
cd ..
just bazel-lock-update
just bazel-lock-check
```

### 5) Commit + push

```bash
git status
git add -A
git commit -m "sync: openai/codex @ <sha>"
git push -u origin HEAD
```

### 6) Open or update PR
Create PR:

```bash
gh pr create --base main --head "$branch" --title "sync: openai/codex @ <sha>" --body "Sync upstream openai/codex main. Local code prioritized; see commit(s) for conflict resolutions."
```

If the PR already exists, pushing new commits is enough.

### 7) Monitor CI until green (required)
Use `gh pr checks` and fix branch-related failures; retry likely flaky jobs up to 3 times. If user choice is required (same-feature conflict), block as described above.

