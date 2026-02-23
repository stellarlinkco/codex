use crate::agent::AgentStatus;
use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::config::Config;
use crate::config::Constrained;
use crate::error::CodexErr;
use crate::features::Feature;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use async_trait::async_trait;
use codex_hooks::HookEvent;
use codex_hooks::HookPayload;
use codex_hooks::HookResultControl;
use codex_protocol::ThreadId;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CollabAgentInteractionBeginEvent;
use codex_protocol::protocol::CollabAgentInteractionEndEvent;
use codex_protocol::protocol::CollabAgentSpawnBeginEvent;
use codex_protocol::protocol::CollabAgentSpawnEndEvent;
use codex_protocol::protocol::CollabCloseBeginEvent;
use codex_protocol::protocol::CollabCloseEndEvent;
use codex_protocol::protocol::CollabResumeBeginEvent;
use codex_protocol::protocol::CollabResumeEndEvent;
use codex_protocol::protocol::CollabWaitingBeginEvent;
use codex_protocol::protocol::CollabWaitingEndEvent;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use futures::FutureExt;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::process::Command;
use tokio::sync::watch::Receiver;
use tokio::time::Instant;
use tokio::time::timeout_at;
use tracing::warn;

pub struct MultiAgentHandler;

/// Minimum wait timeout to prevent tight polling loops from burning CPU.
pub(crate) const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: i64 = 300_000;
pub(crate) const TEAM_SPAWN_CALL_PREFIX: &str = "team/spawn:";
pub(crate) const TEAM_WAIT_CALL_PREFIX: &str = "team/wait:";
pub(crate) const TEAM_CLOSE_CALL_PREFIX: &str = "team/close:";
const TEAM_CONFIG_DIR: &str = "teams";
const TEAM_TASKS_DIR: &str = "tasks";
const WORKTREE_ROOT_DIR: &str = "worktrees";

#[derive(Debug, Deserialize)]
struct CloseAgentArgs {
    id: String,
}

#[derive(Debug, Clone)]
struct TeamMember {
    name: String,
    agent_id: ThreadId,
    agent_type: Option<String>,
}

#[derive(Debug, Clone)]
struct TeamRecord {
    members: Vec<TeamMember>,
    created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitMode {
    Any,
    All,
}

type TeamRegistry = HashMap<ThreadId, HashMap<String, TeamRecord>>;

fn team_registry() -> &'static Mutex<TeamRegistry> {
    static REGISTRY: OnceLock<Mutex<TeamRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
struct WorktreeLease {
    repo_root: PathBuf,
    worktree_path: PathBuf,
}

type WorktreeLeaseRegistry = HashMap<ThreadId, WorktreeLease>;

fn worktree_leases() -> &'static Mutex<WorktreeLeaseRegistry> {
    static REGISTRY: OnceLock<Mutex<WorktreeLeaseRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTeamConfig {
    team_name: String,
    lead_thread_id: String,
    created_at: i64,
    members: Vec<PersistedTeamMember>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTeamMember {
    name: String,
    agent_id: String,
    agent_type: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PersistedTaskState {
    Pending,
    Claimed,
    Completed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTeamTask {
    id: String,
    title: String,
    state: PersistedTaskState,
    depends_on: Vec<String>,
    assignee: PersistedTeamTaskAssignee,
    updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTeamTaskAssignee {
    name: String,
    agent_id: String,
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn team_dir(codex_home: &Path, team_id: &str) -> PathBuf {
    codex_home.join(TEAM_CONFIG_DIR).join(team_id)
}

fn team_config_path(codex_home: &Path, team_id: &str) -> PathBuf {
    team_dir(codex_home, team_id).join("config.json")
}

async fn read_persisted_team_config(
    codex_home: &Path,
    team_id: &str,
) -> Result<PersistedTeamConfig, FunctionCallError> {
    let config_path = team_config_path(codex_home, team_id);
    let raw = match tokio::fs::read_to_string(&config_path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(FunctionCallError::RespondToModel(format!(
                "team `{team_id}` not found"
            )));
        }
        Err(err) => return Err(team_persistence_error("read team config", team_id, err)),
    };

    serde_json::from_str::<PersistedTeamConfig>(&raw)
        .map_err(|err| team_persistence_error("parse team config", team_id, err))
}

fn assert_team_member_or_lead(
    team_id: &str,
    config: &PersistedTeamConfig,
    caller_thread_id: ThreadId,
) -> Result<(), FunctionCallError> {
    let caller_thread_id = caller_thread_id.to_string();
    if caller_thread_id == config.lead_thread_id
        || config
            .members
            .iter()
            .any(|member| member.agent_id == caller_thread_id)
    {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "thread `{caller_thread_id}` is not a member of team `{team_id}`"
    )))
}

fn team_tasks_dir(codex_home: &Path, team_id: &str) -> PathBuf {
    codex_home.join(TEAM_TASKS_DIR).join(team_id)
}

async fn lock_team_tasks(
    codex_home: &Path,
    team_id: &str,
) -> Result<locks::FileLockGuard, FunctionCallError> {
    let tasks_dir = team_tasks_dir(codex_home, team_id);
    tokio::fs::create_dir_all(&tasks_dir)
        .await
        .map_err(|err| team_persistence_error("create team tasks directory", team_id, err))?;
    let lock_path = tasks_dir.join("tasks.lock");
    locks::lock_file_exclusive(&lock_path)
        .await
        .map_err(|err| team_persistence_error("lock team tasks", team_id, err))
}

fn team_task_path(codex_home: &Path, team_id: &str, task_id: &str) -> PathBuf {
    team_tasks_dir(codex_home, team_id).join(format!("{task_id}.json"))
}

fn team_persistence_error(
    action: impl std::fmt::Display,
    team_id: &str,
    err: impl std::fmt::Display,
) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!("failed to {action} for team `{team_id}`: {err}"))
}

async fn remove_dir_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

async fn write_json_atomic<T: Serialize>(path: &Path, payload: &T) -> Result<(), std::io::Error> {
    let data = serde_json::to_vec_pretty(payload).map_err(std::io::Error::other)?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent"))?;
    tokio::fs::create_dir_all(parent).await?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("payload.json");
    let tmp_path = parent.join(format!(".{file_name}.tmp-{}", ThreadId::new()));
    tokio::fs::write(&tmp_path, data).await?;

    if let Err(err) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(err);
    }

    Ok(())
}

fn persisted_team_config(
    sender_thread_id: ThreadId,
    team_id: &str,
    team: &TeamRecord,
) -> PersistedTeamConfig {
    PersistedTeamConfig {
        team_name: team_id.to_string(),
        lead_thread_id: sender_thread_id.to_string(),
        created_at: team.created_at,
        members: team
            .members
            .iter()
            .map(|member| PersistedTeamMember {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                agent_type: member.agent_type.clone(),
            })
            .collect(),
    }
}

fn build_initial_team_tasks(
    requested_members: &[spawn_team::SpawnTeamMemberArgs],
    spawned_members: &[TeamMember],
    updated_at: i64,
) -> Vec<PersistedTeamTask> {
    requested_members
        .iter()
        .zip(spawned_members)
        .map(|(requested, spawned)| PersistedTeamTask {
            id: ThreadId::new().to_string(),
            title: requested.task.trim().to_string(),
            state: PersistedTaskState::Pending,
            depends_on: Vec::new(),
            assignee: PersistedTeamTaskAssignee {
                name: spawned.name.clone(),
                agent_id: spawned.agent_id.to_string(),
            },
            updated_at,
        })
        .collect()
}

async fn persist_team_state(
    codex_home: &Path,
    sender_thread_id: ThreadId,
    team_id: &str,
    team: &TeamRecord,
    initial_tasks: Option<&[PersistedTeamTask]>,
) -> Result<(), FunctionCallError> {
    let config = persisted_team_config(sender_thread_id, team_id, team);
    let config_path = team_config_path(codex_home, team_id);
    write_json_atomic(&config_path, &config)
        .await
        .map_err(|err| team_persistence_error("write team config", team_id, err))?;

    if let Some(tasks) = initial_tasks {
        let tasks_dir = team_tasks_dir(codex_home, team_id);
        remove_dir_if_exists(&tasks_dir)
            .await
            .map_err(|err| team_persistence_error("reset team tasks", team_id, err))?;
        tokio::fs::create_dir_all(&tasks_dir)
            .await
            .map_err(|err| team_persistence_error("create team tasks directory", team_id, err))?;

        for task in tasks {
            let task_path = team_task_path(codex_home, team_id, &task.id);
            write_json_atomic(&task_path, task)
                .await
                .map_err(|err| team_persistence_error("write team task", team_id, err))?;
        }
    }

    Ok(())
}

async fn remove_team_persistence(
    codex_home: &Path,
    team_id: &str,
) -> Result<(), FunctionCallError> {
    remove_dir_if_exists(&team_dir(codex_home, team_id))
        .await
        .map_err(|err| team_persistence_error("remove team config directory", team_id, err))?;
    remove_dir_if_exists(&team_tasks_dir(codex_home, team_id))
        .await
        .map_err(|err| team_persistence_error("remove team tasks directory", team_id, err))?;
    Ok(())
}

fn required_non_empty<'a>(value: &'a str, field: &str) -> Result<&'a str, FunctionCallError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{field} must be non-empty"
        )));
    }
    Ok(trimmed)
}

fn find_team_member(
    team: &TeamRecord,
    team_id: &str,
    member_name: &str,
) -> Result<TeamMember, FunctionCallError> {
    let member_name = required_non_empty(member_name, "member_name")?;
    team.members
        .iter()
        .find(|member| member.name == member_name)
        .cloned()
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "member `{member_name}` not found in team `{team_id}`"
            ))
        })
}

async fn read_team_tasks(
    codex_home: &Path,
    team_id: &str,
) -> Result<Vec<PersistedTeamTask>, FunctionCallError> {
    let tasks_dir = team_tasks_dir(codex_home, team_id);
    let mut dir = match tokio::fs::read_dir(&tasks_dir).await {
        Ok(dir) => dir,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(team_persistence_error(
                "read team tasks directory",
                team_id,
                err,
            ));
        }
    };

    let mut tasks = Vec::new();
    while let Some(entry) = dir
        .next_entry()
        .await
        .map_err(|err| team_persistence_error("iterate team tasks directory", team_id, err))?
    {
        let metadata = entry
            .metadata()
            .await
            .map_err(|err| team_persistence_error("read task metadata", team_id, err))?;
        if !metadata.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let task_raw = tokio::fs::read_to_string(&path).await.map_err(|err| {
            team_persistence_error(format!("read task file `{}`", path.display()), team_id, err)
        })?;
        let task: PersistedTeamTask = serde_json::from_str(&task_raw).map_err(|err| {
            team_persistence_error(
                format!("parse task file `{}`", path.display()),
                team_id,
                err,
            )
        })?;
        tasks.push(task);
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(tasks)
}

async fn read_team_task(
    codex_home: &Path,
    team_id: &str,
    task_id: &str,
) -> Result<PersistedTeamTask, FunctionCallError> {
    let task_id = required_non_empty(task_id, "task_id")?;
    let task_path = team_task_path(codex_home, team_id, task_id);
    let raw = match tokio::fs::read_to_string(&task_path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{task_id}` not found in team `{team_id}`"
            )));
        }
        Err(err) => return Err(team_persistence_error("read team task", team_id, err)),
    };

    serde_json::from_str::<PersistedTeamTask>(&raw)
        .map_err(|err| team_persistence_error("parse team task", team_id, err))
}

async fn write_team_task(
    codex_home: &Path,
    team_id: &str,
    task: &PersistedTeamTask,
) -> Result<(), FunctionCallError> {
    let task_path = team_task_path(codex_home, team_id, &task.id);
    write_json_atomic(&task_path, task)
        .await
        .map_err(|err| team_persistence_error("write team task", team_id, err))
}

fn dependencies_satisfied(task: &PersistedTeamTask, tasks: &[PersistedTeamTask]) -> bool {
    task.depends_on.iter().all(|dependency| {
        tasks.iter().any(|candidate| {
            candidate.id == *dependency && candidate.state == PersistedTaskState::Completed
        })
    })
}

#[derive(Debug, Serialize)]
struct TeamTaskOutput {
    task_id: String,
    title: String,
    state: PersistedTaskState,
    depends_on: Vec<String>,
    assignee_name: String,
    assignee_agent_id: String,
    updated_at: i64,
}

impl From<PersistedTeamTask> for TeamTaskOutput {
    fn from(value: PersistedTeamTask) -> Self {
        Self {
            task_id: value.id,
            title: value.title,
            state: value.state,
            depends_on: value.depends_on,
            assignee_name: value.assignee.name,
            assignee_agent_id: value.assignee.agent_id,
            updated_at: value.updated_at,
        }
    }
}

async fn send_input_to_member(
    session: &std::sync::Arc<Session>,
    turn: &std::sync::Arc<TurnContext>,
    call_id: String,
    receiver_thread_id: ThreadId,
    input_items: Vec<UserInput>,
    prompt: String,
    interrupt: bool,
) -> Result<String, FunctionCallError> {
    if interrupt {
        session
            .services
            .agent_control
            .interrupt_agent(receiver_thread_id)
            .await
            .map_err(|err| collab_agent_error(receiver_thread_id, err))?;
    }
    session
        .send_event(
            turn,
            CollabAgentInteractionBeginEvent {
                call_id: call_id.clone(),
                sender_thread_id: session.conversation_id,
                receiver_thread_id,
                prompt: prompt.clone(),
            }
            .into(),
        )
        .await;
    let result = session
        .services
        .agent_control
        .send_input(receiver_thread_id, input_items)
        .await
        .map_err(|err| collab_agent_error(receiver_thread_id, err));
    let status = session
        .services
        .agent_control
        .get_status(receiver_thread_id)
        .await;
    session
        .send_event(
            turn,
            CollabAgentInteractionEndEvent {
                call_id,
                sender_thread_id: session.conversation_id,
                receiver_thread_id,
                receiver_agent_nickname: None,
                receiver_agent_role: None,
                prompt,
                status,
            }
            .into(),
        )
        .await;
    result
}

#[async_trait]
impl ToolHandler for MultiAgentHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tool_name,
            payload,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "collab handler received unsupported payload".to_string(),
                ));
            }
        };

        match tool_name.as_str() {
            "spawn_agent" => spawn::handle(session, turn, call_id, arguments).await,
            "send_input" => send_input::handle(session, turn, call_id, arguments).await,
            "resume_agent" => resume_agent::handle(session, turn, call_id, arguments).await,
            "wait" => wait::handle(session, turn, call_id, arguments).await,
            "close_agent" => close_agent::handle(session, turn, call_id, arguments).await,
            "spawn_team" => spawn_team::handle(session, turn, call_id, arguments).await,
            "wait_team" => wait_team::handle(session, turn, call_id, arguments).await,
            "close_team" => close_team::handle(session, turn, call_id, arguments).await,
            "team_task_list" => team_task_list::handle(session, turn, call_id, arguments).await,
            "team_task_claim" => team_task_claim::handle(session, turn, call_id, arguments).await,
            "team_task_claim_next" => {
                team_task_claim_next::handle(session, turn, call_id, arguments).await
            }
            "team_task_complete" => {
                team_task_complete::handle(session, turn, call_id, arguments).await
            }
            "team_message" => team_message::handle(session, turn, call_id, arguments).await,
            "team_broadcast" => team_broadcast::handle(session, turn, call_id, arguments).await,
            "team_ask_lead" => team_ask_lead::handle(session, turn, call_id, arguments).await,
            "team_inbox_pop" => team_inbox_pop::handle(session, turn, call_id, arguments).await,
            "team_inbox_ack" => team_inbox_ack::handle(session, turn, call_id, arguments).await,
            "team_cleanup" => team_cleanup::handle(session, turn, call_id, arguments).await,
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported collab tool {other}"
            ))),
        }
    }
}

mod locks;

mod inbox;

mod team_ask_lead;

mod team_inbox_pop;

mod team_inbox_ack;

mod spawn;

mod send_input;

mod resume_agent;

mod wait;

#[derive(Debug)]
struct WaitForAgentsResult {
    statuses: Vec<(ThreadId, AgentStatus)>,
    timed_out: bool,
}

fn normalize_wait_timeout(timeout_ms: Option<i64>) -> Result<i64, FunctionCallError> {
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    match timeout_ms {
        ms if ms <= 0 => Err(FunctionCallError::RespondToModel(
            "timeout_ms must be greater than zero".to_owned(),
        )),
        ms => Ok(ms.clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS)),
    }
}

async fn wait_for_final_status(
    session: std::sync::Arc<Session>,
    thread_id: ThreadId,
    mut status_rx: Receiver<AgentStatus>,
) -> Option<(ThreadId, AgentStatus)> {
    let mut status = status_rx.borrow().clone();
    if crate::agent::status::is_final(&status) {
        return Some((thread_id, status));
    }

    loop {
        if status_rx.changed().await.is_err() {
            let latest = session.services.agent_control.get_status(thread_id).await;
            return crate::agent::status::is_final(&latest).then_some((thread_id, latest));
        }
        status = status_rx.borrow().clone();
        if crate::agent::status::is_final(&status) {
            return Some((thread_id, status));
        }
    }
}

async fn wait_for_agents(
    session: std::sync::Arc<Session>,
    receiver_thread_ids: &[ThreadId],
    timeout_ms: i64,
    mode: WaitMode,
) -> Result<WaitForAgentsResult, (ThreadId, CodexErr)> {
    let mut status_rxs = Vec::with_capacity(receiver_thread_ids.len());
    let mut final_statuses = HashMap::new();

    for id in receiver_thread_ids {
        match session.services.agent_control.subscribe_status(*id).await {
            Ok(rx) => {
                let status = rx.borrow().clone();
                if crate::agent::status::is_final(&status) {
                    final_statuses.insert(*id, status);
                } else {
                    status_rxs.push((*id, rx));
                }
            }
            Err(CodexErr::ThreadNotFound(_)) => {
                final_statuses.insert(*id, AgentStatus::NotFound);
            }
            Err(err) => return Err((*id, err)),
        }
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    match mode {
        WaitMode::Any => {
            if final_statuses.is_empty() {
                let mut futures = FuturesUnordered::new();
                for (id, rx) in status_rxs {
                    let session = session.clone();
                    futures.push(wait_for_final_status(session, id, rx));
                }

                let mut results = Vec::new();
                loop {
                    match timeout_at(deadline, futures.next()).await {
                        Ok(Some(Some(result))) => {
                            results.push(result);
                            break;
                        }
                        Ok(Some(None)) => continue,
                        Ok(None) | Err(_) => break,
                    }
                }

                if !results.is_empty() {
                    loop {
                        match futures.next().now_or_never() {
                            Some(Some(Some(result))) => results.push(result),
                            Some(Some(None)) => continue,
                            Some(None) | None => break,
                        }
                    }
                }

                for (id, status) in results {
                    final_statuses.insert(id, status);
                }
            }

            let statuses = receiver_thread_ids
                .iter()
                .filter_map(|id| final_statuses.get(id).cloned().map(|status| (*id, status)))
                .collect::<Vec<_>>();
            Ok(WaitForAgentsResult {
                timed_out: statuses.is_empty(),
                statuses,
            })
        }
        WaitMode::All => {
            if final_statuses.len() < receiver_thread_ids.len() {
                let mut futures = FuturesUnordered::new();
                for (id, rx) in status_rxs {
                    let session = session.clone();
                    futures.push(wait_for_final_status(session, id, rx));
                }

                while final_statuses.len() < receiver_thread_ids.len() {
                    match timeout_at(deadline, futures.next()).await {
                        Ok(Some(Some((id, status)))) => {
                            final_statuses.insert(id, status);
                        }
                        Ok(Some(None)) => continue,
                        Ok(None) | Err(_) => break,
                    }
                }
            }

            let timed_out = final_statuses.len() < receiver_thread_ids.len();
            let statuses = receiver_thread_ids
                .iter()
                .filter_map(|id| final_statuses.get(id).cloned().map(|status| (*id, status)))
                .collect::<Vec<_>>();

            Ok(WaitForAgentsResult {
                statuses,
                timed_out,
            })
        }
    }
}

fn normalized_team_id(team_id: &str) -> Result<String, FunctionCallError> {
    let team_id = team_id.trim();
    if team_id.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "team_id must be non-empty".to_string(),
        ));
    }
    Ok(team_id.to_string())
}

fn optional_non_empty<'a>(
    value: &'a Option<String>,
    field: &str,
) -> Result<Option<&'a str>, FunctionCallError> {
    match value {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(FunctionCallError::RespondToModel(format!(
                    "{field} must be non-empty when provided"
                )))
            } else {
                Ok(Some(trimmed))
            }
        }
        None => Ok(None),
    }
}

fn apply_member_model_overrides(
    config: &mut Config,
    model_provider_id: Option<&str>,
    model: Option<&str>,
) -> Result<(), FunctionCallError> {
    if let Some(provider_id) = model_provider_id {
        let provider = config
            .model_providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "model_provider `{provider_id}` not found"
                ))
            })?;
        config.model_provider_id = provider_id.to_string();
        config.model_provider = provider;
    }

    if let Some(model) = model {
        config.model = Some(model.to_string());
    }

    Ok(())
}

fn prefixed_team_call_id(prefix: &str, call_id: &str) -> String {
    format!("{prefix}{call_id}")
}

fn team_member_names(members: &[TeamMember]) -> HashMap<ThreadId, String> {
    members
        .iter()
        .map(|member| {
            let agent_type = member
                .agent_type
                .as_deref()
                .map(str::trim)
                .filter(|agent_type| !agent_type.is_empty())
                .unwrap_or("default");
            (member.agent_id, format!("{} [{agent_type}]", member.name))
        })
        .collect()
}

fn get_team_record(
    sender_thread_id: ThreadId,
    team_id: &str,
) -> Result<TeamRecord, FunctionCallError> {
    let registry = team_registry()
        .lock()
        .map_err(|_| FunctionCallError::Fatal("team registry poisoned".to_string()))?;
    let Some(teams) = registry.get(&sender_thread_id) else {
        return Err(FunctionCallError::RespondToModel(format!(
            "team `{team_id}` not found"
        )));
    };
    teams
        .get(team_id)
        .cloned()
        .ok_or_else(|| FunctionCallError::RespondToModel(format!("team `{team_id}` not found")))
}

fn insert_team_record(
    sender_thread_id: ThreadId,
    team_id: String,
    record: TeamRecord,
) -> Result<(), FunctionCallError> {
    let mut registry = team_registry()
        .lock()
        .map_err(|_| FunctionCallError::Fatal("team registry poisoned".to_string()))?;
    let teams = registry.entry(sender_thread_id).or_default();
    if teams.contains_key(&team_id) {
        return Err(FunctionCallError::RespondToModel(format!(
            "team `{team_id}` already exists"
        )));
    }
    teams.insert(team_id, record);
    Ok(())
}

fn remove_team_record(sender_thread_id: ThreadId, team_id: &str) -> Result<(), FunctionCallError> {
    let mut registry = team_registry()
        .lock()
        .map_err(|_| FunctionCallError::Fatal("team registry poisoned".to_string()))?;
    let Some(teams) = registry.get_mut(&sender_thread_id) else {
        return Ok(());
    };
    teams.remove(team_id);
    if teams.is_empty() {
        registry.remove(&sender_thread_id);
    }
    Ok(())
}

fn remove_members_from_team(
    sender_thread_id: ThreadId,
    team_id: &str,
    member_names: &[String],
) -> Result<Option<TeamRecord>, FunctionCallError> {
    let mut registry = team_registry()
        .lock()
        .map_err(|_| FunctionCallError::Fatal("team registry poisoned".to_string()))?;
    let teams = registry.entry(sender_thread_id).or_default();
    let team = teams
        .get_mut(team_id)
        .ok_or_else(|| FunctionCallError::RespondToModel(format!("team `{team_id}` not found")))?;

    team.members
        .retain(|member| !member_names.iter().any(|name| name == &member.name));
    let remove_team = team.members.is_empty();
    let remaining = (!remove_team).then(|| team.clone());
    if remove_team {
        teams.remove(team_id);
    }
    if teams.is_empty() {
        registry.remove(&sender_thread_id);
    }
    Ok(remaining)
}

fn register_worktree_lease(agent_id: ThreadId, lease: WorktreeLease) {
    let mut registry = match worktree_leases().lock() {
        Ok(registry) => registry,
        Err(poisoned) => poisoned.into_inner(),
    };
    registry.insert(agent_id, lease);
}

fn take_worktree_lease(agent_id: ThreadId) -> Option<WorktreeLease> {
    let mut registry = match worktree_leases().lock() {
        Ok(registry) => registry,
        Err(poisoned) => poisoned.into_inner(),
    };
    registry.remove(&agent_id)
}

fn approval_policy_for_hooks(policy: AskForApproval) -> &'static str {
    match policy {
        AskForApproval::UnlessTrusted => "untrusted",
        AskForApproval::OnFailure => "on-failure",
        AskForApproval::OnRequest => "on-request",
        AskForApproval::Reject(_) => "reject",
        AskForApproval::Never => "never",
    }
}

fn git_error_text(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    format!("git exited with status {}", output.status)
}

async fn dispatch_subagent_start_hook(
    session: &Session,
    turn: &TurnContext,
    agent_id: ThreadId,
    agent_type: &str,
) {
    let outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::SubagentStart {
                agent_id: agent_id.to_string(),
                agent_type: agent_type.to_string(),
            },
        })
        .await;

    let mut additional_context = Vec::new();
    for outcome in outcomes {
        let hook_name = outcome.hook_name;
        let result = outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                hook_name = %hook_name,
                error,
                "subagent_start hook failed; continuing"
            );
        }

        if let HookResultControl::Block { reason } = result.control {
            warn!(
                hook_name = %hook_name,
                reason,
                "subagent_start hook returned a blocking decision; ignoring"
            );
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
}

async fn dispatch_teammate_idle_hook(
    session: &Session,
    turn: &TurnContext,
    team_id: &str,
    teammate_name: &str,
) -> Option<String> {
    let outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::TeammateIdle {
                teammate_name: teammate_name.to_string(),
                team_name: team_id.to_string(),
            },
        })
        .await;

    let mut additional_context = Vec::new();
    let mut blocked = None;
    for outcome in outcomes {
        let hook_name = outcome.hook_name;
        let result = outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                hook_name = %hook_name,
                error,
                "teammate_idle hook failed; continuing"
            );
        }

        if blocked.is_none()
            && let HookResultControl::Block { reason } = result.control
        {
            blocked = Some((hook_name, reason));
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
    blocked.map(|(hook_name, reason)| format!("teammate_idle hook '{hook_name}' blocked: {reason}"))
}

async fn dispatch_task_completed_hook(
    session: &Session,
    turn: &TurnContext,
    team_id: &str,
    task_id: &str,
    task_subject: &str,
    teammate_name: Option<&str>,
) -> Option<String> {
    let outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::TaskCompleted {
                task_id: task_id.to_string(),
                task_subject: task_subject.to_string(),
                task_description: None,
                teammate_name: teammate_name.map(std::string::ToString::to_string),
                team_name: Some(team_id.to_string()),
            },
        })
        .await;

    let mut additional_context = Vec::new();
    let mut blocked = None;
    for outcome in outcomes {
        let hook_name = outcome.hook_name;
        let result = outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                hook_name = %hook_name,
                error,
                "task_completed hook failed; continuing"
            );
        }

        if blocked.is_none()
            && let HookResultControl::Block { reason } = result.control
        {
            blocked = Some((hook_name, reason));
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
    blocked
        .map(|(hook_name, reason)| format!("task_completed hook '{hook_name}' blocked: {reason}"))
}

async fn dispatch_worktree_hook(
    session: &Session,
    turn: &TurnContext,
    repo_path: PathBuf,
    worktree_path: PathBuf,
    created: bool,
) {
    let hook_event = if created {
        HookEvent::WorktreeCreate {
            repo_path,
            worktree_path,
        }
    } else {
        HookEvent::WorktreeRemove {
            repo_path,
            worktree_path,
        }
    };
    let outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event,
        })
        .await;

    let mut additional_context = Vec::new();
    for outcome in outcomes {
        let hook_name = outcome.hook_name;
        let result = outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(hook_name = %hook_name, error, "worktree hook failed; continuing");
        }

        if let HookResultControl::Block { reason } = result.control {
            warn!(
                hook_name = %hook_name,
                reason,
                "worktree hook returned a blocking decision; ignoring"
            );
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
}

async fn create_agent_worktree(
    session: &Session,
    turn: &TurnContext,
) -> Result<WorktreeLease, FunctionCallError> {
    let Some(repo_root) = crate::git_info::resolve_root_git_project_for_trust(&turn.cwd) else {
        return Err(FunctionCallError::RespondToModel(
            "worktree=true requires running inside a git repository".to_string(),
        ));
    };

    let root = turn
        .config
        .codex_home
        .join(WORKTREE_ROOT_DIR)
        .join(session.conversation_id.to_string());
    tokio::fs::create_dir_all(&root).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to create worktree root: {err}"))
    })?;

    let worktree_path = root.join(ThreadId::new().to_string());
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["worktree", "add", "--detach"])
        .arg(&worktree_path)
        .arg("HEAD")
        .output()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to run git worktree add: {err}"))
        })?;

    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "failed to create worktree `{}`: {}",
            worktree_path.display(),
            git_error_text(&output)
        )));
    }

    dispatch_worktree_hook(
        session,
        turn,
        repo_root.clone(),
        worktree_path.clone(),
        true,
    )
    .await;
    Ok(WorktreeLease {
        repo_root,
        worktree_path,
    })
}

async fn remove_worktree_lease(
    session: &Session,
    turn: &TurnContext,
    lease: WorktreeLease,
) -> Result<(), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&lease.repo_root)
        .args(["worktree", "remove", "--force"])
        .arg(&lease.worktree_path)
        .output()
        .await
        .map_err(|err| format!("failed to run git worktree remove: {err}"))?;

    if !output.status.success() {
        let err_text = git_error_text(&output);
        let ignored_error = err_text.contains("is not a working tree")
            || err_text.contains("No such file or directory")
            || err_text.contains("does not exist");
        if !ignored_error {
            return Err(format!(
                "failed to remove worktree `{}`: {err_text}",
                lease.worktree_path.display()
            ));
        }
    }

    let _ = remove_dir_if_exists(&lease.worktree_path).await;
    dispatch_worktree_hook(
        session,
        turn,
        lease.repo_root.clone(),
        lease.worktree_path.clone(),
        false,
    )
    .await;
    Ok(())
}

async fn cleanup_agent_worktree(
    session: &Session,
    turn: &TurnContext,
    agent_id: ThreadId,
) -> Result<(), String> {
    let Some(lease) = take_worktree_lease(agent_id) else {
        return Ok(());
    };
    remove_worktree_lease(session, turn, lease).await
}

async fn shutdown_team_members(session: &std::sync::Arc<Session>, members: &[TeamMember]) {
    for member in members {
        let _ = session
            .services
            .agent_control
            .shutdown_agent(member.agent_id)
            .await;
    }
}

async fn cleanup_spawned_team_members(
    session: &std::sync::Arc<Session>,
    turn: &std::sync::Arc<TurnContext>,
    members: &[TeamMember],
) {
    shutdown_team_members(session, members).await;
    for member in members {
        let _ = cleanup_agent_worktree(session.as_ref(), turn.as_ref(), member.agent_id).await;
    }
}

mod spawn_team;

mod wait_team;

mod close_team;

mod team_task_list;

mod team_task_claim;

mod team_task_claim_next;

mod team_task_complete;

mod team_message;

mod team_broadcast;

mod team_cleanup;

pub mod close_agent {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize, Serialize)]
    pub(super) struct CloseAgentResult {
        pub(super) status: AgentStatus,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: CloseAgentArgs = parse_arguments(&arguments)?;
        let agent_id = agent_id(&args.id)?;
        session
            .send_event(
                &turn,
                CollabCloseBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id: agent_id,
                }
                .into(),
            )
            .await;
        let status = match session
            .services
            .agent_control
            .subscribe_status(agent_id)
            .await
        {
            Ok(mut status_rx) => status_rx.borrow_and_update().clone(),
            Err(_) => session.services.agent_control.get_status(agent_id).await,
        };
        let result = session
            .services
            .agent_control
            .shutdown_agent(agent_id)
            .await;
        session
            .send_event(
                &turn,
                CollabCloseEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id: agent_id,
                    receiver_agent_nickname: None,
                    receiver_agent_role: None,
                    status: status.clone(),
                }
                .into(),
            )
            .await;
        match result {
            Ok(_) => {}
            Err(err) => {
                if !matches!(status, AgentStatus::Shutdown | AgentStatus::NotFound) {
                    return Err(collab_agent_error(agent_id, err));
                }
            }
        }
        if let Err(err) = cleanup_agent_worktree(session.as_ref(), turn.as_ref(), agent_id).await {
            return Err(FunctionCallError::RespondToModel(err));
        }

        let content = serde_json::to_string(&CloseAgentResult { status }).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize close_agent result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

fn agent_id(id: &str) -> Result<ThreadId, FunctionCallError> {
    ThreadId::from_string(id)
        .map_err(|e| FunctionCallError::RespondToModel(format!("invalid agent id {id}: {e:?}")))
}

fn collab_spawn_error(err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::UnsupportedOperation(_) => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        err => FunctionCallError::RespondToModel(format!("collab spawn failed: {err}")),
    }
}

fn collab_agent_error(agent_id: ThreadId, err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("agent with id {id} not found"))
        }
        CodexErr::InternalAgentDied => {
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} is closed"))
        }
        CodexErr::UnsupportedOperation(_) => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        err => FunctionCallError::RespondToModel(format!("collab tool failed: {err}")),
    }
}

fn thread_spawn_source(parent_thread_id: ThreadId, depth: i32) -> SessionSource {
    SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth,
        agent_nickname: None,
        agent_role: None,
    })
}

fn parse_collab_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Vec<UserInput>, FunctionCallError> {
    match (message, items) {
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "Provide either message or items, but not both".to_string(),
        )),
        (None, None) => Err(FunctionCallError::RespondToModel(
            "Provide one of: message or items".to_string(),
        )),
        (Some(message), None) => {
            if message.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Empty message can't be sent to an agent".to_string(),
                ));
            }
            Ok(vec![UserInput::Text {
                text: message,
                text_elements: Vec::new(),
            }])
        }
        (None, Some(items)) => {
            if items.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Items can't be empty".to_string(),
                ));
            }
            Ok(items)
        }
    }
}

fn input_preview(items: &[UserInput]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|item| match item {
            UserInput::Text { text, .. } => text.clone(),
            UserInput::Image { .. } => "[image]".to_string(),
            UserInput::LocalImage { path } => format!("[local_image:{}]", path.display()),
            UserInput::Skill { name, path } => {
                format!("[skill:${name}]({})", path.display())
            }
            UserInput::Mention { name, path } => format!("[mention:${name}]({path})"),
            _ => "[input]".to_string(),
        })
        .collect();

    parts.join("\n")
}

fn build_agent_spawn_config(
    base_instructions: &BaseInstructions,
    turn: &TurnContext,
    child_depth: i32,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn, child_depth)?;
    config.base_instructions = Some(base_instructions.text.clone());
    Ok(config)
}

fn build_agent_resume_config(
    turn: &TurnContext,
    child_depth: i32,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn, child_depth)?;
    // For resume, keep base instructions sourced from rollout/session metadata.
    config.base_instructions = None;
    Ok(config)
}

fn build_agent_shared_config(
    turn: &TurnContext,
    child_depth: i32,
) -> Result<Config, FunctionCallError> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    config.model = Some(turn.model_info.slug.clone());
    config.model_provider = turn.provider.clone();
    config.model_reasoning_effort = turn.reasoning_effort;
    config.model_reasoning_summary = turn.reasoning_summary;
    config.developer_instructions = turn.developer_instructions.clone();
    config.compact_prompt = turn.compact_prompt.clone();
    config.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    config.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    config.cwd = turn.cwd.clone();
    config
        .permissions
        .sandbox_policy
        .set(turn.sandbox_policy.clone())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("sandbox_policy is invalid: {err}"))
        })?;
    apply_spawn_agent_overrides(&mut config, child_depth);

    Ok(config)
}

fn apply_spawn_agent_overrides(config: &mut Config, child_depth: i32) {
    config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
    if exceeds_thread_spawn_depth_limit(child_depth + 1, config.agent_max_depth) {
        config.features.disable(Feature::Collab);
    }
}

#[cfg(test)]
mod tests;
