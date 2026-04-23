# Deny-Read Glob Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add config and protocol support for deny-read glob semantics and `glob_scan_max_depth` without changing runtime sandbox enforcement.

**Architecture:** Extend the protocol to carry glob-shaped filesystem paths and an optional `glob_scan_max_depth`, then extend core config compilation to emit those structures and attach warnings to the existing startup warning pipeline when recursive unreadable globs have no scan depth bound.

**Tech Stack:** Rust, serde, schemars, ts-rs, cargo test

---

### Task 1: Extend protocol permission types

**Files:**
- Modify: `codex-rs/protocol/src/permissions.rs`

- [ ] Step 1: Write failing unit tests in `codex-rs/protocol/src/permissions.rs` for `glob_scan_max_depth` persistence and the new glob path variant.
- [ ] Step 2: Run `cargo test -p codex-protocol permissions -- --nocapture` and verify the new tests fail for the missing field and variant.
- [ ] Step 3: Add `glob_scan_max_depth: Option<usize>` to `FileSystemSandboxPolicy`, add a glob-shaped `FileSystemPath` variant, and update constructors/helpers to preserve current non-glob behavior.
- [ ] Step 4: Re-run `cargo test -p codex-protocol permissions -- --nocapture` and verify the protocol tests pass.

### Task 2: Compile glob semantics from config

**Files:**
- Modify: `codex-rs/core/src/config/permissions.rs`
- Modify: `codex-rs/core/src/config/config_tests.rs`

- [ ] Step 1: Write failing config tests that deserialize `glob_scan_max_depth`, compile a deny-read glob entry into the runtime policy, and expect a warning candidate when recursive glob deny rules omit the depth bound.
- [ ] Step 2: Run `cargo test -p codex-core config::config_tests -- --nocapture` and verify the new assertions fail against the current implementation.
- [ ] Step 3: Extend `FilesystemPermissionsToml` and `compile_permission_profile()` to compile glob-shaped unreadable entries, preserve `glob_scan_max_depth`, and surface warning strings for unbounded recursive deny globs.
- [ ] Step 4: Re-run `cargo test -p codex-core config::config_tests -- --nocapture` and verify the targeted tests pass.

### Task 3: Feed warnings into config startup warnings and refresh schema

**Files:**
- Modify: `codex-rs/core/src/config/mod.rs`
- Modify: `codex-rs/core/config.schema.json` if regenerated output changes

- [ ] Step 1: Add a failing config test that loads a permissions profile with a recursive unreadable glob and expects the startup warning to appear in `config.startup_warnings`.
- [ ] Step 2: Run `cargo test -p codex-core config::config_tests -- --nocapture` and verify it fails because warnings are not propagated yet.
- [ ] Step 3: Thread the warning output from `compile_permission_profile()` into `startup_warnings`, then run `cd codex-rs && just write-config-schema` if the config schema changes.
- [ ] Step 4: Re-run `cargo test -p codex-core config::config_tests -- --nocapture` and verify the warning coverage passes.

### Task 4: Verify and format

**Files:**
- Modify: files touched above only

- [ ] Step 1: Run `cargo test -p codex-protocol permissions -- --nocapture`.
- [ ] Step 2: Run `cargo test -p codex-core config::config_tests -- --nocapture`.
- [ ] Step 3: Run `cd codex-rs && just fmt`.
- [ ] Step 4: Run `git diff --name-only` and confirm the change scope is limited to the glob semantics task plus the pre-existing uncommitted work.
