# Codex Serve Kanban Workspace PRD

> Status: Draft v1.0
> Date: 2026-03-14
> Author: codex-serve team

## 1. Problem Statement

Current `codex serve` provides a basic GitHub webhook integration and a simple
kanban board, but it suffers from several critical limitations that prevent it
from being a true async, concurrent, remote development platform:

1. **No real-time log visibility** -- job logs are written to files; the
   frontend can only poll `GET /api/github/jobs/{id}/log`. Users cannot tell
   what the agent is doing *right now*.
2. **Coarse task status** -- jobs have only four states
   (`queued/running/succeeded/failed`). There is no way to know whether the
   agent is cloning, coding, testing, or creating a PR.
3. **Single-repo assumption** -- `allow_repos` is a flat list; there is no
   concept of a project that spans multiple repositories.
4. **No Epic / hierarchy** -- all issues are dumped into one flat kanban.
   There is no way to group related issues or manage cross-repo features.
5. **No per-task execution config** -- every job uses the same model and
   reasoning effort. Users cannot choose opus for a hard task or haiku for a
   typo fix.
6. **UI jank** -- no virtual scrolling for logs, no optimistic updates, no
   swimlane grouping.
7. **Slow GitHub sync** -- polling every 5 minutes; no instant reaction to
   webhook events.

## 2. Vision

Transform `codex serve` into a **Workspace-centric, async, concurrent AI
development platform** where:

- A **Workspace** groups multiple GitHub repos into one project view.
- **Epics** organize issues across repos into coherent features.
- A **Kanban Board** visualizes all issues, grouped by Epic (swimlanes).
- **Dragging a card to Running** triggers AI execution with configurable
  model, reasoning effort, and custom prompt.
- **Real-time WebSocket log streaming** lets users watch agent progress live.
- **Fine-grained task status** (8+ states) gives full lifecycle visibility.
- The whole system works from a phone over the internet via inner-tunnel.

## 3. User Personas

| Persona | Description |
|---------|-------------|
| **Solo Dev** | Uses codex serve locally or via tunnel; manages 1-3 repos; wants to queue tasks and check results later. |
| **Tech Lead** | Manages a multi-repo project; writes PRDs, breaks them into Epics + Issues; monitors progress on kanban. |
| **Mobile Reviewer** | Uses phone via tunnel to check task status, read logs, approve PRs; needs fast, lightweight UI. |

## 4. Requirements

### 4.1 Workspace (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| WS-1 | A Workspace contains 1..N GitHub repos | User can create a workspace and add `owner/repo` references; each repo gets a color and short label. |
| WS-2 | Workspace has a single Board View | One kanban board shows issues from ALL repos in the workspace. |
| WS-3 | Multiple Workspaces can coexist | User can switch between workspaces; each has independent board config. |
| WS-4 | Workspace persists across server restarts | Stored in `~/.codex/workspaces/{id}/`. |
| WS-5 | Workspace-level default execution config | Default model, reasoning effort, sandbox mode, system prompt, timeout, auto-PR, auto-test. |
| WS-6 | Webhook routing by workspace | Incoming GitHub webhooks are routed to the correct workspace based on repo membership. |

### 4.2 Epic (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| EP-1 | Epic is anchored to a GitHub Issue in any workspace repo | Epic references `{repo}#{number}` as its anchor. |
| EP-2 | Epic contains child Issues from any repo in the workspace | Children can span multiple repos: `fe#18`, `be#43`, `shared#7`. |
| EP-3 | Epic children are discovered from the anchor issue body | Parse GitHub tasklist syntax (`- [ ] #N`, `- [ ] owner/repo#N`). |
| EP-4 | Epic progress is computed from children | `{done}/{total}` count and percentage. |
| EP-5 | Epic-level execution config override | Can set model/reasoning/system-prompt at Epic level; inherited by children unless overridden. |
| EP-6 | Unassigned issues shown in a "(No Epic)" group | Issues not linked to any Epic appear in a catch-all swimlane. |

### 4.3 Kanban Board (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| KB-1 | Default columns: Backlog, Running, Testing, Review, Done | Columns are customizable per workspace. |
| KB-2 | Swimlane grouping modes: by Epic, by Repo, by Assignee, None | User can switch modes; default is by Epic. |
| KB-3 | Drag a card to Running triggers execution | `enqueue_job` is called with the card's resolved config. |
| KB-4 | Card displays: repo color badge, title, Epic, config summary, status, progress, log button | All info visible at a glance. |
| KB-5 | Repo color badge on each card | Distinct color per repo in workspace (configurable). |
| KB-6 | WIP limits per column | Configurable; warn when exceeded. |
| KB-7 | Filter by: Repo, Epic, Label, Assignee | Left sidebar filter panel. |
| KB-8 | Drag to Done closes the GitHub issue | Via GitHub API. |
| KB-9 | Optimistic UI updates with rollback on error | Card moves instantly; reverts if backend fails. |

### 4.4 Task Execution Config (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| TC-1 | Three-level cascade: Card > Epic > Workspace > Global | `resolve_config()` merges all levels; Card wins. |
| TC-2 | Configurable model per task | Dropdown: claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5, etc. |
| TC-3 | Configurable reasoning effort | Low / Medium / High selector. |
| TC-4 | Custom prompt per card | Text area; supplements the GitHub issue body. |
| TC-5 | System prompt accumulation | Workspace system prompt + Epic system prompt are concatenated; Card prompt is separate. |
| TC-6 | Sandbox mode per task | read-only / workspace-write / full-access. |
| TC-7 | Timeout per task | Default 30 min; overridable per card. |
| TC-8 | Auto-PR toggle | When enabled, agent creates a PR on completion. |
| TC-9 | Auto-test toggle | When enabled, agent runs tests before committing. |
| TC-10 | Quick config popup on drag-to-Running | If card has no prompt, show a lightweight config dialog before executing. |
| TC-11 | Config summary badge on card | Shows `opus/high/30min` or similar shorthand; hidden if matches workspace default. |

### 4.5 Real-time Log Streaming (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| LS-1 | WebSocket endpoint per job | `WS /ws/logs/{job_id}?token=xxx` |
| LS-2 | History replay on connect | Client receives all existing log lines first, then live stream. |
| LS-3 | Log lines are typed | `[Agent]`, `[Tool]`, `[Test]`, `[System]` prefixes for filtering. |
| LS-4 | Virtual scrolling in log panel | Handles 100k+ lines without jank; uses `@tanstack/react-virtual` or equivalent. |
| LS-5 | Auto-scroll with lock | Scrolls to bottom automatically; pauses when user scrolls up; resumes on click. |
| LS-6 | ANSI color support | Terminal colors are preserved in the browser. |
| LS-7 | Log search and filter | Search within logs; filter by type (`Agent`/`Tool`/`Test`). |
| LS-8 | Log panel opens from card | Click `[log]` button on a Running/Testing card to open bottom drawer. |

### 4.6 Task Status Machine (P0)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| SM-1 | 8+ job states | `Queued -> Cloning -> Running -> Testing -> Committing -> PR_Created -> Review -> Done` with `Failed` reachable from any active state. |
| SM-2 | State badge on kanban card | Color-coded badge showing current sub-state. |
| SM-3 | State transitions broadcast via SSE | `github-job-updated` event includes `old_status` and `new_status`. |
| SM-4 | Duration timer on Running cards | Elapsed time since execution started. |
| SM-5 | Last agent message preview on card | One-line preview of the most recent agent output. |

### 4.7 GitHub Sync (P1)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| GS-1 | Webhook-driven instant sync | When a webhook event arrives for a workspace repo, update board state immediately (<1s). |
| GS-2 | Polling fallback every 15 min | Catch any missed webhooks or external changes. |
| GS-3 | New issues auto-appear on board | When a new issue is opened in a workspace repo, a card appears in Backlog. |
| GS-4 | Issue close syncs to Done column | External close (via PR merge or manual) moves card to Done. |
| GS-5 | Label changes sync to card | Label adds/removes reflected on card tags. |

### 4.8 Cross-repo Dependencies (P2)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| CD-1 | Issue dependency detection | Parse "blocked by" / "depends on" links from GitHub issue body. |
| CD-2 | Lock icon on blocked cards | Card shows lock icon and dependency list; cannot be dragged to Running. |
| CD-3 | Auto-unlock on dependency completion | When blocking issue reaches Done, dependent card becomes draggable. |

### 4.9 Views (P2)

| ID | Requirement | Acceptance Criteria |
|----|------------|---------------------|
| VW-1 | Timeline View | Gantt-style view showing Epics as bars with child issue progress. |
| VW-2 | Backlog View | List view for unassigned issues; drag to assign to Epics. |

## 5. Non-functional Requirements

| ID | Requirement | Target |
|----|------------|--------|
| NF-1 | Log panel renders 100k lines without frame drops | <16ms per frame (60fps) |
| NF-2 | Card drag latency | <50ms visual feedback |
| NF-3 | WebSocket log latency | <200ms from agent output to browser render |
| NF-4 | SSE event delivery | <500ms from state change to UI update |
| NF-5 | Concurrent job execution | Default 4 workers; configurable up to 16 |
| NF-6 | Mobile-responsive UI | Usable on 375px-width screens |
| NF-7 | PWA / offline awareness | Show offline indicator; queue operations for replay |
| NF-8 | Server restart resilience | Running jobs are detected and status recovered on restart |

## 6. Out of Scope (v1)

- Multi-user collaboration / real-time cursors
- GitHub Actions integration (triggering CI from codex serve)
- Billing / usage tracking
- Custom agent support (only `codex exec` as executor)
- Sprint / iteration planning

## 7. Success Metrics

| Metric | Target |
|--------|--------|
| Time from "drag to Running" to "see first log line" | <5s |
| Number of concurrent jobs without UI degradation | >= 8 |
| Full Epic (5 issues) completion without human intervention | >= 80% success rate |
| Mobile usability score (Lighthouse) | >= 90 |

## 8. Implementation Priority

| Phase | Items | Dependencies |
|-------|-------|-------------|
| **Phase 1** (P0-core) | Workspace CRUD, Board View, Card move triggers execution, SSE events | None |
| **Phase 2** (P0-realtime) | WebSocket log streaming, Task status machine (8 states), Log panel UI | Phase 1 |
| **Phase 3** (P0-config) | Three-level execution config, Quick config popup, Config summary on cards | Phase 1 |
| **Phase 4** (P0-epic) | Epic discovery, Swimlane grouping, Epic-level config | Phase 1 |
| **Phase 5** (P1-sync) | Webhook-driven instant sync, Auto-appear/close | Phase 1 |
| **Phase 6** (P2) | Dependencies, Timeline View, Backlog View | Phase 4 |
