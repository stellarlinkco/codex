# Codex Serve Kanban Workspace Architecture

> Status: Draft v1.0
> Date: 2026-03-14
> Companion: [PRD](./kanban-workspace-prd.md)

## 1. System Overview

```
                          +------------------+
                          |   Mobile / Web   |
                          |   Browser (PWA)  |
                          +--------+---------+
                                   |
                          HTTPS / WSS (tunnel optional)
                                   |
+------------------------------------------------------------------+
|                        codex serve (Axum)                         |
|                                                                  |
|  +------------------+  +------------------+  +-----------------+ |
|  |   REST API       |  |   SSE Channel    |  |  WS Channels    | |
|  |   /api/*         |  |   /api/events    |  |  /ws/logs/{jid} | |
|  +--------+---------+  +--------+---------+  +--------+--------+ |
|           |                      |                     |         |
|  +--------v---------+  +--------v---------+  +---------v-------+ |
|  |  Workspace Mgr   |  |  Event Bus       |  |  Log Broker     | |
|  |  (CRUD + Board)  |  |  (broadcast)     |  |  (per-job bc)   | |
|  +--------+---------+  +------------------+  +---------+-------+ |
|           |                                            |         |
|  +--------v-----------------------------------------+  |         |
|  |                  Scheduler                        | |         |
|  |  Priority Queue + Concurrency Control + State FSM | |         |
|  +--------+-----------------------------------------+  |         |
|           |                                            |         |
|  +--------v-----------------------------------------+  |         |
|  |                Worker Pool (N workers)            <--+         |
|  |  Each worker:                                     |           |
|  |    1. Clone/fetch repo                            |           |
|  |    2. Create worktree                             |           |
|  |    3. Spawn codex exec (subprocess)               |           |
|  |    4. Stream stdout -> Log Broker                 |           |
|  |    5. Parse state transitions -> Event Bus        |           |
|  +---------------------------------------------------+           |
|                                                                  |
|  +------------------+                                            |
|  | GitHub Webhook   |  POST /github/webhook                     |
|  | Handler          |  (HMAC auth, no token)                    |
|  +------------------+                                            |
|                                                                  |
|  +------------------+                                            |
|  | GitHub Sync      |  Webhook-driven + 15min polling fallback  |
|  | Loop             |                                            |
|  +------------------+                                            |
+------------------------------------------------------------------+

Storage:
  ~/.codex/workspaces/{id}/     workspace.json, epics.json, cards.json
  ~/.codex/github-repos/        repo caches + worktrees
  ~/.codex/github/deliveries/   delivery markers
```

## 2. Data Model

### 2.1 Workspace

A Workspace is the top-level organizational unit. It groups multiple GitHub
repos and owns a single Board configuration.

```rust
struct Workspace {
    id: String,                         // UUID v4
    name: String,                       // "E-commerce Platform v2"
    repos: Vec<RepoRef>,               // 1..N repos
    board: BoardConfig,
    default_exec: ExecConfig,           // workspace-level execution defaults
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct RepoRef {
    full_name: String,                  // "owner/repo"
    color: String,                      // hex color for UI badge, e.g. "#3B82F6"
    short_label: String,                // "fe", "be", "infra"
    default_branch: String,             // "main"
}
```

Storage: `~/.codex/workspaces/{id}/workspace.json`

### 2.2 Epic

An Epic is anchored to a GitHub Issue and groups child Issues across repos.

```rust
struct Epic {
    anchor: IssueRef,                   // the GitHub issue that IS the Epic
    title: String,
    color: String,                      // swimlane color
    children: Vec<IssueRef>,            // child issues, any repo in workspace
    exec_config: ExecConfig,            // epic-level overrides
    progress: Progress,                 // computed, not stored
}

struct IssueRef {
    repo: String,                       // "owner/repo"
    number: u64,                        // issue number
}

struct Progress {
    total: u32,
    done: u32,
    in_progress: u32,
    failed: u32,
}
```

Storage: `~/.codex/workspaces/{id}/epics.json`

### 2.3 Board

```rust
struct BoardConfig {
    columns: Vec<Column>,
    swimlane_mode: SwimLaneMode,
    wip_limits: HashMap<String, u8>,    // column_id -> limit
    filters: BoardFilters,
}

struct Column {
    id: String,                         // "backlog", "running", etc.
    name: String,                       // display name
    auto_trigger: Option<AutoTrigger>,  // action when card enters this column
}

enum AutoTrigger {
    StartExecution,                     // "running" column
    CloseIssue,                         // "done" column
}

enum SwimLaneMode {
    ByEpic,
    ByRepo,
    ByAssignee,
    None,
}

struct BoardFilters {
    repos: Option<Vec<String>>,
    epics: Option<Vec<IssueRef>>,
    labels: Option<Vec<String>>,
    assignees: Option<Vec<String>>,
}
```

### 2.4 Issue Card

```rust
struct IssueCard {
    issue: IssueRef,
    title: String,
    body: String,                       // GitHub issue body (cached)
    epic: Option<IssueRef>,             // parent Epic anchor ref
    column_id: String,
    position: f64,                      // sort order within column+swimlane
    labels: Vec<String>,
    assignee: Option<String>,
    repo_color: String,                 // inherited from RepoRef
    repo_label: String,                 // inherited from RepoRef
    exec_config: ExecConfig,            // card-level overrides
    job: Option<JobSnapshot>,           // current/last job
    dependencies: Vec<IssueRef>,        // "blocked by" issues
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct JobSnapshot {
    job_id: String,
    status: JobStatus,
    progress_pct: Option<u8>,
    started_at: Option<DateTime<Utc>>,
    elapsed_secs: Option<u64>,
    log_tail: Option<String>,           // last line preview
    pr_url: Option<String>,             // if PR was created
}
```

Storage: `~/.codex/workspaces/{id}/cards.json`

### 2.5 Execution Config (Cascading)

```rust
/// All fields are Option -- unset means "inherit from parent level".
#[derive(Default, Clone, Serialize, Deserialize)]
struct ExecConfig {
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    sandbox: Option<SandboxMode>,
    system_prompt: Option<String>,
    prompt: Option<String>,
    timeout_minutes: Option<u32>,
    auto_pr: Option<bool>,
    auto_test: Option<bool>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum ReasoningEffort { Low, Medium, High }

#[derive(Clone, Copy, Serialize, Deserialize)]
enum SandboxMode { ReadOnly, WorkspaceWrite, FullAccess }
```

Resolution order: **Card > Epic > Workspace > Global defaults**.

```rust
fn resolve_exec_config(
    card: &ExecConfig,
    epic: &ExecConfig,
    workspace: &ExecConfig,
    global: &ExecConfig,
) -> ResolvedExecConfig {
    ResolvedExecConfig {
        model: card.model.clone()
            .or_else(|| epic.model.clone())
            .or_else(|| workspace.model.clone())
            .or_else(|| global.model.clone())
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string()),

        reasoning_effort: card.reasoning_effort
            .or(epic.reasoning_effort)
            .or(workspace.reasoning_effort)
            .or(global.reasoning_effort)
            .unwrap_or(ReasoningEffort::Medium),

        sandbox: card.sandbox
            .or(epic.sandbox)
            .or(workspace.sandbox)
            .or(global.sandbox)
            .unwrap_or(SandboxMode::WorkspaceWrite),

        // system_prompt is CONCATENATED (workspace + epic), not overridden
        system_prompt: [
            workspace.system_prompt.as_deref(),
            epic.system_prompt.as_deref(),
        ]
        .iter()
        .filter_map(|s| *s)
        .collect::<Vec<_>>()
        .join("\n\n"),

        // prompt is OVERRIDDEN (card only), combined with issue body at runtime
        prompt: card.prompt.clone().unwrap_or_default(),

        timeout_minutes: card.timeout_minutes
            .or(epic.timeout_minutes)
            .or(workspace.timeout_minutes)
            .or(global.timeout_minutes)
            .unwrap_or(30),

        auto_pr: card.auto_pr
            .or(epic.auto_pr)
            .or(workspace.auto_pr)
            .or(global.auto_pr)
            .unwrap_or(true),

        auto_test: card.auto_test
            .or(epic.auto_test)
            .or(workspace.auto_test)
            .or(global.auto_test)
            .unwrap_or(true),
    }
}
```

### 2.6 Job Status State Machine

```
              enqueue
                |
                v
            +--------+
            | Queued  |
            +----+---+
                 |
                 v
            +--------+
            | Cloning |  -- git clone / fetch + worktree setup
            +----+---+
                 |
                 v
            +---------+
            | Running |  -- codex exec is executing
            +----+----+
                 |
                 v
            +---------+
            | Testing |  -- auto_test: running test suite
            +----+----+
                 |
                 v
            +------------+
            | Committing |  -- git add + commit + push
            +----+-------+
                 |
                 v
            +------------+
            | PR_Created |  -- auto_pr: gh pr create
            +----+-------+
                 |
                 v
            +--------+
            | Review |  -- waiting for CI / human review
            +----+---+
                 |
                 v
            +------+
            | Done |
            +------+

    Any active state ---> Failed (with error message + last log)
```

Status detection strategy in the worker:

| Transition | Detection Method |
|-----------|-----------------|
| Queued -> Cloning | Worker picks up job; starts git operations |
| Cloning -> Running | Worktree ready; `codex exec` process spawned |
| Running -> Testing | Parse stdout: detect test runner invocation patterns |
| Testing -> Committing | Parse stdout: detect `git commit` patterns |
| Committing -> PR_Created | Parse stdout: detect `gh pr create` or PR URL |
| PR_Created -> Review | PR created; waiting for CI checks |
| Review -> Done | CI passes + optional human approval |
| * -> Failed | Non-zero exit code or timeout |

## 3. Real-time Communication

### 3.1 SSE: Structured Events

Endpoint: `GET /api/events?token={token}[&workspaceId={id}]`

All state changes are broadcast as SSE events. The client maintains local
state by applying these events.

```
Event types:
  workspace-created          { workspace }
  workspace-updated          { workspace }
  workspace-deleted          { workspace_id }
  epic-created               { workspace_id, epic }
  epic-updated               { workspace_id, epic }
  epic-deleted               { workspace_id, anchor }
  card-created               { workspace_id, card }
  card-updated               { workspace_id, card }
  card-moved                 { workspace_id, issue, from_col, to_col }
  card-deleted               { workspace_id, issue }
  job-updated                { workspace_id, job_id, old_status, new_status, snapshot }
  board-config-updated       { workspace_id, board }
  github-sync-completed      { workspace_id, added, removed, updated }
  heartbeat                  { timestamp }
  connection-changed         { status, subscription_id }
```

Implementation: `tokio::sync::broadcast::channel::<SyncEvent>(4096)`.
Workspace filter applied server-side to reduce client traffic.

### 3.2 WebSocket: Log Streaming

Endpoint: `WS /ws/logs/{job_id}?token={token}`

Dedicated per-job channel for high-volume log data. Separated from SSE to
avoid flooding the event bus.

```
Server -> Client messages (JSON):
  { "type": "log",     "line": "...", "ts": 1710000000, "kind": "agent" }
  { "type": "log",     "line": "...", "ts": 1710000001, "kind": "tool" }
  { "type": "log",     "line": "...", "ts": 1710000002, "kind": "test" }
  { "type": "log",     "line": "...", "ts": 1710000003, "kind": "system" }
  { "type": "status",  "status": "testing" }
  { "type": "done",    "exit_code": 0, "summary": "..." }
  { "type": "error",   "message": "timeout after 30m" }

Client -> Server messages (JSON):
  { "type": "ping" }
```

Backend implementation per job:

```rust
struct JobRuntime {
    job_id: String,
    log_tx: broadcast::Sender<LogLine>,     // capacity 8192
    log_file: PathBuf,                       // persistent log
    status: Arc<AtomicU8>,                   // current JobStatus as u8
}

// Worker writes each line to both:
fn on_stdout_line(runtime: &JobRuntime, line: &str) {
    // 1. Persist
    append_to_file(&runtime.log_file, line);
    // 2. Broadcast to all connected WS clients
    let _ = runtime.log_tx.send(LogLine {
        line: line.to_string(),
        ts: Utc::now(),
        kind: classify_line(line),
    });
}
```

WebSocket handler:

```rust
async fn ws_log_handler(job_id: &str, mut ws: WebSocket, state: &AppState) {
    let runtime = state.job_runtimes.get(job_id);

    // Phase 1: replay history from log file
    if let Ok(lines) = read_log_file(&runtime.log_file).await {
        for line in lines {
            ws.send(json!({"type": "log", "line": line.line,
                           "ts": line.ts, "kind": line.kind})).await;
        }
    }

    // Phase 2: subscribe to live stream
    let mut rx = runtime.log_tx.subscribe();
    loop {
        tokio::select! {
            Ok(line) = rx.recv() => {
                if ws.send(json!({"type": "log", "line": line.line,
                                  "ts": line.ts, "kind": line.kind})).await.is_err() {
                    break; // client disconnected
                }
            }
            msg = ws.recv() => {
                match msg {
                    Some(Ok(_)) => {} // ping or other client msg
                    _ => break,       // client disconnected
                }
            }
        }
    }
}
```

## 4. Scheduler

### 4.1 Priority Queue

```rust
struct Scheduler {
    queue: Mutex<BinaryHeap<PrioritizedJob>>,
    running: DashMap<String, JobHandle>,
    semaphore: Arc<Semaphore>,              // max_workers (default 4)
    event_tx: broadcast::Sender<SyncEvent>,
}

struct PrioritizedJob {
    priority: u8,                           // 0 = highest
    enqueued_at: Instant,
    workspace_id: String,
    card: IssueRef,
    resolved_config: ResolvedExecConfig,
}

impl Ord for PrioritizedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        other.priority.cmp(&self.priority)  // lower number = higher prio
            .then(self.enqueued_at.cmp(&other.enqueued_at)) // FIFO tiebreak
    }
}
```

Priority inference:

| Source | Priority |
|--------|----------|
| GitHub label `priority: critical` | P0 |
| GitHub label `priority: high` | P1 |
| Default | P2 |
| GitHub label `priority: low` | P3 |

### 4.2 Worker Lifecycle

```
Scheduler.enqueue(job):
  1. Insert into priority queue
  2. Broadcast job-updated { status: Queued }
  3. Spawn dispatcher task (if not already running)

Dispatcher loop:
  1. semaphore.acquire()           // wait for a free worker slot
  2. queue.pop()                   // highest priority job
  3. tokio::spawn(run_worker(job)) // run in background

run_worker(job):
  1. Broadcast job-updated { status: Cloning }
  2. Clone/fetch repo -> create worktree
  3. Broadcast job-updated { status: Running }
  4. Spawn `codex exec` subprocess with resolved config
     - model:            --model {model}
     - reasoning:        --reasoning-effort {low|medium|high}
     - sandbox:          --sandbox {mode}
     - approval:         --approval-policy never
     - output:           -o {last_message_path}
     - prompt:           system_prompt + "\n\n" + card_prompt + "\n\n" + issue_body
  5. Stream stdout line by line:
     - Write to log file + broadcast via JobRuntime.log_tx
     - Detect state transitions (testing, committing, PR)
     - Broadcast job-updated on each transition
  6. On exit:
     - exit 0 -> Broadcast job-updated { status: Done }
     - exit != 0 -> Broadcast job-updated { status: Failed }
  7. Release semaphore permit
```

## 5. GitHub Integration

### 5.1 Webhook Routing

When a webhook arrives at `POST /github/webhook`:

```
1. Validate HMAC signature (unchanged)
2. Extract repo full_name from payload
3. Find workspace(s) that contain this repo
4. For each matching workspace:
   a. If it's an issue event (opened/closed/labeled):
      -> Update card state, broadcast SSE event
   b. If it's a /codex command:
      -> Enqueue job via Scheduler (existing behavior)
   c. If it's a push event:
      -> Handle as before (no workspace routing needed)
```

### 5.2 GitHub Sync

```
Two sync paths:

1. Webhook-driven (instant, <1s):
   - issues.opened      -> create card in Backlog
   - issues.closed       -> move card to Done
   - issues.labeled      -> update card labels
   - issues.edited       -> update card title/body, re-parse Epic children

2. Polling fallback (every 15 min):
   - For each repo in each workspace:
     GET /repos/{owner}/{repo}/issues?state=open&per_page=100
   - Reconcile with local cards (add missing, remove closed)
   - Broadcast github-sync-completed event
```

### 5.3 Epic Discovery

When syncing or processing an issue event:

```rust
fn discover_epic_children(issue_body: &str, workspace_repos: &[RepoRef]) -> Vec<IssueRef> {
    // Parse GitHub tasklist syntax:
    //   - [ ] #43
    //   - [x] #44
    //   - [ ] owner/other-repo#12
    //   - [ ] https://github.com/owner/repo/issues/15
    let mut children = Vec::new();
    for line in issue_body.lines() {
        if let Some(cap) = TASKLIST_RE.captures(line) {
            let issue_ref = parse_issue_ref(cap, workspace_repos);
            if let Some(r) = issue_ref {
                children.push(r);
            }
        }
    }
    children
}
```

An issue is considered an Epic if:
1. It has a `epic` label, OR
2. Its body contains 2+ tasklist items referencing other issues

## 6. Storage Layout

```
~/.codex/
  workspaces/
    index.json                              # [{id, name, repo_count}]
    {workspace_id}/
      workspace.json                        # Workspace struct
      epics.json                            # Vec<Epic>
      cards.json                            # Vec<IssueCard>
      jobs/
        {job_id}.json                       # Job metadata + status
        {job_id}.log                        # Full log output

  github-repos/                             # repo caches (unchanged)
    {owner}/{repo}/
      repo/                                 # bare clone
      issues/{number}/                      # worktree
      pulls/{number}/                       # worktree
      pushes/{hash}/                        # worktree

  github/
    deliveries/*.marker                     # dedup markers (unchanged)
    threads/{owner}/{repo}/*                # thread state (unchanged)
```

All JSON writes use atomic rename (`write tmp + rename`) to prevent corruption.

## 7. API Design

### 7.1 Workspace

```
GET    /api/workspaces                                  -> Vec<WorkspaceSummary>
POST   /api/workspaces                                  -> Workspace
       body: { name, repos: [{full_name, color, short_label}] }
GET    /api/workspaces/{ws_id}                          -> Workspace
PUT    /api/workspaces/{ws_id}                          -> Workspace
       body: { name?, repos?, default_exec? }
DELETE /api/workspaces/{ws_id}                           -> 204
```

### 7.2 Board

```
GET    /api/workspaces/{ws_id}/board                    -> BoardState
       response: { config, columns_with_cards, swimlanes }
PUT    /api/workspaces/{ws_id}/board/config             -> BoardConfig
       body: { columns?, swimlane_mode?, wip_limits?, filters? }
```

### 7.3 Cards

```
GET    /api/workspaces/{ws_id}/cards                    -> Vec<IssueCard>
       query: ?repo=&epic=&label=&assignee=&column=
POST   /api/workspaces/{ws_id}/cards                    -> IssueCard
       body: { issue: {repo, number} }    // manually add a card
PUT    /api/workspaces/{ws_id}/cards/{repo}/{number}/move -> IssueCard
       body: { column_id, position }
       side-effect: if column has AutoTrigger::StartExecution -> enqueue job
PUT    /api/workspaces/{ws_id}/cards/{repo}/{number}/config -> ExecConfig
       body: { model?, reasoning_effort?, prompt?, ... }
GET    /api/workspaces/{ws_id}/cards/{repo}/{number}/resolved-config
       -> ResolvedExecConfig
       // Preview: shows final merged config without executing
DELETE /api/workspaces/{ws_id}/cards/{repo}/{number}    -> 204
```

### 7.4 Epics

```
GET    /api/workspaces/{ws_id}/epics                    -> Vec<Epic>
POST   /api/workspaces/{ws_id}/epics                    -> Epic
       body: { anchor: {repo, number} }
       // Auto-discovers children from issue body
PUT    /api/workspaces/{ws_id}/epics/{repo}/{number}    -> Epic
       body: { exec_config?, children? }
DELETE /api/workspaces/{ws_id}/epics/{repo}/{number}    -> 204
GET    /api/workspaces/{ws_id}/epics/{repo}/{number}/refresh -> Epic
       // Re-fetch from GitHub + re-parse children
```

### 7.5 Jobs

```
GET    /api/workspaces/{ws_id}/jobs                     -> Vec<JobSnapshot>
       query: ?status=&card_repo=&card_number=
GET    /api/workspaces/{ws_id}/jobs/{job_id}            -> JobDetail
GET    /api/workspaces/{ws_id}/jobs/{job_id}/log        -> text/plain
       // Full log file (for non-WS clients / download)
POST   /api/workspaces/{ws_id}/jobs/{job_id}/cancel     -> 204
       // Send SIGTERM to codex exec subprocess
POST   /api/workspaces/{ws_id}/jobs/{job_id}/retry      -> JobSnapshot
       // Re-enqueue with same config
```

### 7.6 Real-time

```
GET    /api/events?token={t}&workspaceId={ws_id}        -> SSE stream
WS     /ws/logs/{job_id}?token={t}                      -> WebSocket
```

### 7.7 Sync

```
POST   /api/workspaces/{ws_id}/sync                     -> SyncResult
       // Manually trigger full GitHub sync for this workspace
       response: { added: 3, removed: 1, updated: 5 }
```

## 8. Frontend Architecture

### 8.1 Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Framework | React 19 + TypeScript | Existing codebase uses React |
| Styling | Tailwind CSS | Fast iteration, small bundle |
| State | Zustand | Lightweight; SSE events update store directly |
| Data fetching | @tanstack/react-query | Cache, dedup, background refetch |
| Drag & drop | @dnd-kit/core | Accessible, performant, supports swimlanes |
| Virtual scroll | @tanstack/react-virtual | Required for log panel (100k+ lines) |
| WebSocket | reconnecting-websocket | Auto-reconnect with backoff |
| ANSI rendering | ansi-to-html or xterm.js | Terminal color support in log panel |
| Charts | Lightweight (sparkline) | Epic progress bars, no heavy charting lib |

### 8.2 Page Structure

```
/                                   -> Workspace selector (if multiple)
/w/{workspace_id}                   -> Board View (default)
/w/{workspace_id}/timeline          -> Timeline View (P2)
/w/{workspace_id}/backlog           -> Backlog View (P2)
/w/{workspace_id}/settings          -> Workspace settings + exec defaults
/w/{workspace_id}/jobs/{job_id}     -> Full-screen log view
```

### 8.3 Component Tree

```
<App>
  <WorkspaceSwitcher />
  <BoardView>
    <Sidebar>
      <EpicList />                  // filter by Epic
      <RepoList />                  // filter by Repo
      <GroupBySelector />           // swimlane mode toggle
    </Sidebar>
    <Board>
      <DndContext>
        {swimlanes.map(lane =>
          <Swimlane key={lane.id} epic={lane.epic}>
            {columns.map(col =>
              <Column key={col.id} wip={col.wip_limit}>
                {cards.map(card =>
                  <IssueCard key={card.issue}
                    card={card}
                    onConfigClick={openConfigPanel}
                    onLogClick={openLogDrawer}
                  />
                )}
              </Column>
            )}
          </Swimlane>
        )}
      </DndContext>
    </Board>
    <LogDrawer>                     // bottom drawer, resizable
      <LogToolbar>
        <SearchInput />
        <FilterButtons />           // Agent | Tool | Test | System
        <AutoScrollToggle />
      </LogToolbar>
      <VirtualLogList />            // @tanstack/react-virtual
    </LogDrawer>
  </BoardView>
  <ConfigDialog />                  // card/epic config popup
  <QuickRunDialog />                // drag-to-Running prompt popup
</App>
```

### 8.4 State Management

```typescript
// Zustand store -- SSE events update this directly
interface WorkspaceStore {
  workspace: Workspace | null;
  epics: Epic[];
  cards: IssueCard[];
  jobs: Map<string, JobSnapshot>;

  // Derived
  cardsByColumn: Map<string, IssueCard[]>;
  cardsByEpic: Map<string, IssueCard[]>;
  epicProgress: Map<string, Progress>;

  // Actions
  moveCard(issue: IssueRef, toColumn: string, position: number): void;
  updateCardConfig(issue: IssueRef, config: Partial<ExecConfig>): void;
  applySSEEvent(event: SyncEvent): void;
}

// SSE connection -- feeds into store
function connectSSE(workspaceId: string, store: WorkspaceStore) {
  const es = new EventSource(`/api/events?workspaceId=${workspaceId}&token=...`);
  es.onmessage = (e) => {
    const event = JSON.parse(e.data);
    store.applySSEEvent(event);
  };
}

// WebSocket log stream -- independent per job
function connectLogStream(jobId: string): Observable<LogLine> {
  const ws = new ReconnectingWebSocket(`/ws/logs/${jobId}?token=...`);
  // returns observable of LogLine for the log panel to consume
}
```

### 8.5 Optimistic Updates

Card drag flow:

```
1. User drags card from Backlog to Running
2. UI immediately moves card (optimistic)
3. If card has no prompt configured:
   -> Show QuickRunDialog (model, thinking, prompt)
   -> User clicks "Start"
4. PUT /api/workspaces/{ws}/cards/{repo}/{num}/move { column: "running" }
5. Backend:
   a. Update cards.json
   b. Broadcast card-moved event
   c. If column has AutoTrigger::StartExecution:
      -> resolve_exec_config()
      -> scheduler.enqueue(job)
      -> Broadcast job-updated { status: Queued }
6. If PUT fails:
   -> Revert card position in UI
   -> Show error toast
```

## 9. Prompt Assembly

When a job is executed, the final prompt sent to `codex exec` is assembled
from multiple sources:

```
+-------------------------------------------------------+
| SYSTEM PROMPT (accumulated)                           |
|                                                       |
| [Workspace system_prompt]                             |
| "Always run tests before committing. Use              |
|  conventional commits format."                        |
|                                                       |
| [Epic system_prompt]                                  |
| "This epic involves user authentication.              |
|  All passwords must use bcrypt. Tokens use RS256."    |
+-------------------------------------------------------+

+-------------------------------------------------------+
| TASK PROMPT (card-level only)                         |
|                                                       |
| [Card prompt]                                         |
| "Implement login page with React Hook Form.           |
|  Support email + phone login. Integrate be#43 API."   |
+-------------------------------------------------------+

+-------------------------------------------------------+
| CONTEXT (auto-generated)                              |
|                                                       |
| [GitHub Issue body]                                   |
| "As a user, I want to log in..."                      |
|                                                       |
| [.codex_github_context.md]                            |
| Issue comments, linked PRs, related discussions       |
+-------------------------------------------------------+
```

The system prompt is passed via `--system-prompt` flag or environment variable.
The task prompt + context are concatenated as the user prompt to `codex exec`.

## 10. Migration from Current Design

### 10.1 Backward Compatibility

The existing `github-kanban.json` and `github-jobs.json` will be migrated
to the new workspace format on first startup:

```rust
fn migrate_legacy_kanban(codex_home: &Path) -> Result<()> {
    let old_kanban = codex_home.join("github-kanban.json");
    if !old_kanban.exists() { return Ok(()); }

    // 1. Read legacy kanban
    let legacy: LegacyGithubKanban = read_json(&old_kanban)?;

    // 2. Infer repos from existing work items
    let repos: HashSet<String> = legacy.work_items.iter()
        .map(|wi| wi.repo.clone())
        .collect();

    // 3. Create a "Migrated" workspace
    let ws = Workspace {
        id: Uuid::new_v4().to_string(),
        name: "Migrated Workspace".to_string(),
        repos: repos.into_iter().map(|r| RepoRef {
            full_name: r.clone(),
            color: assign_color(&r),
            short_label: short_label(&r),
            default_branch: "main".to_string(),
        }).collect(),
        board: BoardConfig::default(),
        default_exec: ExecConfig::default(),
        ..Default::default()
    };

    // 4. Convert work items to cards
    let cards = legacy.work_items.iter().map(|wi| {
        IssueCard {
            issue: IssueRef { repo: wi.repo.clone(), number: wi.number },
            column_id: map_legacy_column(&wi.column),
            exec_config: ExecConfig {
                model: wi.settings.model.clone(),
                reasoning_effort: wi.settings.reasoning_effort,
                prompt: wi.settings.prompt_prefix.clone(),
                ..Default::default()
            },
            ..Default::default()
        }
    }).collect();

    // 5. Write new workspace
    persist_workspace(&ws, &cards)?;

    // 6. Rename legacy file (don't delete)
    rename(&old_kanban, old_kanban.with_extension("json.migrated"))?;

    Ok(())
}
```

### 10.2 Config Compatibility

The existing `config.toml` `[github_webhook]` section remains valid.
New fields are added:

```toml
[github_webhook]
# ... existing fields unchanged ...
max_concurrency = 4              # renamed from max_concurrency (was 2)

[workspace]
# New section; optional. If absent, workspaces are managed via API only.
auto_create = true               # auto-create workspace from allow_repos
default_model = "claude-sonnet-4-6"
default_reasoning_effort = "medium"
default_timeout_minutes = 30
```

## 11. Security Considerations

| Concern | Mitigation |
|---------|-----------|
| Workspace API requires auth | All `/api/workspaces/*` endpoints require bearer token (existing auth) |
| WebSocket auth | Token passed as query param; validated on upgrade |
| Prompt injection via issue body | `codex exec` runs in sandboxed mode; system prompt is prepended |
| Cross-workspace isolation | Jobs run in per-repo worktrees; filesystem isolation maintained |
| Repo access | GitHub token / App must have access to all repos in workspace |

## 12. Observability

| Signal | Implementation |
|--------|---------------|
| Job duration histogram | Log start/end times; compute in API response |
| Success/failure rate per workspace | Aggregate from job status in `cards.json` |
| Active workers gauge | `scheduler.running.len()` exposed via `/api/stats` |
| Log volume per job | File size of `{job_id}.log` |
| SSE client count | `events_tx.receiver_count()` |
| WS client count per job | `JobRuntime.log_tx.receiver_count()` |
