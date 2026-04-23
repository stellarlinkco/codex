# External Config Migration Startup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a startup-time external config migration prompt to the TUI using the current branch architecture.

**Architecture:** Extend notice persistence in core config, then add a small TUI startup module that reuses `ExternalAgentConfigService` directly. Keep the UI minimal and scoped to import, skip, skip forever, and exit.

**Tech Stack:** Rust, Tokio, ratatui, toml_edit

---

### Task 1: Extend notice persistence for external migration prompts

**Files:**
- Modify: `codex-rs/core/src/config/types.rs`
- Modify: `codex-rs/core/src/config/edit.rs`

- [ ] Step 1: Write failing tests for home/project hide and timestamp persistence in `codex-rs/core/src/config/edit.rs`.
- [ ] Step 2: Run the targeted core config edit tests and verify they fail on missing external migration edits.
- [ ] Step 3: Add `ExternalConfigMigrationPrompts` and the required `ConfigEdit` / `ConfigEditsBuilder` support.
- [ ] Step 4: Re-run the targeted core config edit tests and verify they pass.

### Task 2: Add startup filtering helpers and minimal prompt module

**Files:**
- Create: `codex-rs/tui/src/external_agent_config_migration.rs`
- Create: `codex-rs/tui/src/external_agent_config_migration_startup.rs`

- [ ] Step 1: Write failing tests for hidden-scope filtering, cooldown filtering, and success message generation.
- [ ] Step 2: Run the targeted TUI tests and verify they fail because the module does not exist yet.
- [ ] Step 3: Add the minimal startup helper and prompt implementation.
- [ ] Step 4: Re-run the targeted TUI tests and verify they pass.

### Task 3: Wire startup flow into `App::run`

**Files:**
- Modify: `codex-rs/tui/src/app.rs`

- [ ] Step 1: Add a failing TUI-level test or compile-time integration point covering the startup hook placement.
- [ ] Step 2: Run the targeted TUI test and verify it fails.
- [ ] Step 3: Call the startup migration handler before `ChatWidget` initialization and surface the success message in the transcript.
- [ ] Step 4: Re-run the targeted TUI test and verify it passes.

### Task 4: Verify and format

**Files:**
- Modify: files touched above only

- [ ] Step 1: Run `cargo test -p codex-core config::edit -- --nocapture` or the exact targeted tests added for Task 1.
- [ ] Step 2: Run `cargo test -p codex-tui external_agent_config_migration -- --nocapture` or the exact targeted tests added for Task 2 and Task 3.
- [ ] Step 3: Run `cd codex-rs && just fmt`.
- [ ] Step 4: Review `git diff --name-only` and confirm no unrelated files were changed.
