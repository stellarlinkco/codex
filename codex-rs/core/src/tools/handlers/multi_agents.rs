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

fn team_tasks_dir(codex_home: &Path, team_id: &str) -> PathBuf {
    codex_home.join(TEAM_TASKS_DIR).join(team_id)
}

fn team_task_path(codex_home: &Path, team_id: &str, task_id: &str) -> PathBuf {
    team_tasks_dir(codex_home, team_id).join(format!("{task_id}.json"))
}

fn team_persistence_error(
    action: &str,
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
        .map(|(requested, spawned)| {
            let id = ThreadId::new().to_string();
            PersistedTeamTask {
                id: id.clone(),
                title: requested.task.trim().to_string(),
                state: PersistedTaskState::Pending,
                depends_on: Vec::new(),
                assignee: PersistedTeamTaskAssignee {
                    name: spawned.name.clone(),
                    agent_id: spawned.agent_id.to_string(),
                },
                updated_at,
            }
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
        let task_raw = tokio::fs::read_to_string(entry.path())
            .await
            .map_err(|err| team_persistence_error("read task file", team_id, err))?;
        let task: PersistedTeamTask = serde_json::from_str(&task_raw)
            .map_err(|err| team_persistence_error("parse task file", team_id, err))?;
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
            "team_cleanup" => team_cleanup::handle(session, turn, call_id, arguments).await,
            other => Err(FunctionCallError::RespondToModel(format!(
                "unsupported collab tool {other}"
            ))),
        }
    }
}

mod spawn {
    use super::*;
    use crate::agent::role::apply_role_to_config;

    use crate::agent::exceeds_thread_spawn_depth_limit;
    use crate::agent::next_thread_spawn_depth;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct SpawnAgentArgs {
        message: Option<String>,
        items: Option<Vec<UserInput>>,
        agent_type: Option<String>,
        model_provider: Option<String>,
        model: Option<String>,
        #[serde(default)]
        worktree: bool,
        #[serde(default, alias = "backendground")]
        background: bool,
    }

    #[derive(Debug, Serialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: SpawnAgentArgs = parse_arguments(&arguments)?;
        let role_name = args
            .agent_type
            .as_deref()
            .map(str::trim)
            .filter(|role| !role.is_empty());
        let model_provider = optional_non_empty(&args.model_provider, "model_provider")?;
        let model = optional_non_empty(&args.model, "model")?;
        let use_worktree = args.worktree;
        let _background = args.background;
        let input_items = parse_collab_input(args.message, args.items)?;
        let prompt = input_preview(&input_items);
        let session_source = turn.session_source.clone();
        let child_depth = next_thread_spawn_depth(&session_source);
        if exceeds_thread_spawn_depth_limit(child_depth) {
            return Err(FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string(),
            ));
        }
        session
            .send_event(
                &turn,
                CollabAgentSpawnBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    prompt: prompt.clone(),
                }
                .into(),
            )
            .await;
        let mut config = build_agent_spawn_config(
            &session.get_base_instructions().await,
            turn.as_ref(),
            child_depth,
        )?;
        apply_role_to_config(&mut config, role_name)
            .await
            .map_err(FunctionCallError::RespondToModel)?;
        apply_member_model_overrides(&mut config, model_provider, model)?;
        apply_spawn_agent_overrides(&mut config, child_depth);
        let worktree_lease = if use_worktree {
            let lease = create_agent_worktree(&session, &turn).await?;
            config.cwd = lease.worktree_path.clone();
            Some(lease)
        } else {
            None
        };

        let result = session
            .services
            .agent_control
            .spawn_agent(
                config,
                input_items,
                Some(thread_spawn_source(session.conversation_id, child_depth)),
            )
            .await
            .map_err(collab_spawn_error);
        match (&result, worktree_lease) {
            (Ok(thread_id), Some(lease)) => register_worktree_lease(*thread_id, lease),
            (Err(_), Some(lease)) => {
                let _ = remove_worktree_lease(&session, &turn, lease).await;
            }
            _ => {}
        }
        let (new_thread_id, status) = match &result {
            Ok(thread_id) => (
                Some(*thread_id),
                session.services.agent_control.get_status(*thread_id).await,
            ),
            Err(_) => (None, AgentStatus::NotFound),
        };
        session
            .send_event(
                &turn,
                CollabAgentSpawnEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    new_thread_id,
                    prompt,
                    status,
                }
                .into(),
            )
            .await;
        let new_thread_id = result?;

        let content = serde_json::to_string(&SpawnAgentResult {
            agent_id: new_thread_id.to_string(),
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize spawn_agent result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod send_input {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct SendInputArgs {
        id: String,
        message: Option<String>,
        items: Option<Vec<UserInput>>,
        #[serde(default)]
        interrupt: bool,
    }

    #[derive(Debug, Serialize)]
    struct SendInputResult {
        submission_id: String,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: SendInputArgs = parse_arguments(&arguments)?;
        let receiver_thread_id = agent_id(&args.id)?;
        let input_items = parse_collab_input(args.message, args.items)?;
        let prompt = input_preview(&input_items);
        let submission_id = send_input_to_member(
            &session,
            &turn,
            call_id,
            receiver_thread_id,
            input_items,
            prompt,
            args.interrupt,
        )
        .await?;

        let content = serde_json::to_string(&SendInputResult { submission_id }).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize send_input result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod resume_agent {
    use super::*;
    use crate::agent::next_thread_spawn_depth;
    use crate::rollout::find_thread_path_by_id_str;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct ResumeAgentArgs {
        id: String,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub(super) struct ResumeAgentResult {
        pub(super) status: AgentStatus,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ResumeAgentArgs = parse_arguments(&arguments)?;
        let receiver_thread_id = agent_id(&args.id)?;
        let child_depth = next_thread_spawn_depth(&turn.session_source);
        if exceeds_thread_spawn_depth_limit(child_depth) {
            return Err(FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string(),
            ));
        }

        session
            .send_event(
                &turn,
                CollabResumeBeginEvent {
                    call_id: call_id.clone(),
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id,
                }
                .into(),
            )
            .await;

        let mut status = session
            .services
            .agent_control
            .get_status(receiver_thread_id)
            .await;
        let error = if matches!(status, AgentStatus::NotFound) {
            // If the thread is no longer active, attempt to restore it from rollout.
            match try_resume_closed_agent(
                &session,
                &turn,
                receiver_thread_id,
                &args.id,
                child_depth,
            )
            .await
            {
                Ok(resumed_status) => {
                    status = resumed_status;
                    None
                }
                Err(err) => {
                    status = session
                        .services
                        .agent_control
                        .get_status(receiver_thread_id)
                        .await;
                    Some(err)
                }
            }
        } else {
            None
        };

        session
            .send_event(
                &turn,
                CollabResumeEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id,
                    status: status.clone(),
                }
                .into(),
            )
            .await;

        if let Some(err) = error {
            return Err(err);
        }

        let content = serde_json::to_string(&ResumeAgentResult { status }).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize resume_agent result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }

    async fn try_resume_closed_agent(
        session: &Arc<Session>,
        turn: &Arc<TurnContext>,
        receiver_thread_id: ThreadId,
        receiver_id: &str,
        child_depth: i32,
    ) -> Result<AgentStatus, FunctionCallError> {
        let rollout_path = find_thread_path_by_id_str(
            turn.config.codex_home.as_path(),
            receiver_id,
        )
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "tool failed: failed to locate rollout for agent {receiver_thread_id}: {err}"
            ))
        })?
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "agent with id {receiver_thread_id} not found"
            ))
        })?;

        let config = build_agent_resume_config(turn.as_ref(), child_depth)?;
        let resumed_thread_id = session
            .services
            .agent_control
            .resume_agent_from_rollout(
                config,
                rollout_path,
                thread_spawn_source(session.conversation_id, child_depth),
            )
            .await
            .map_err(|err| collab_agent_error(receiver_thread_id, err))?;

        Ok(session
            .services
            .agent_control
            .get_status(resumed_thread_id)
            .await)
    }
}

mod wait {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct WaitArgs {
        ids: Vec<String>,
        timeout_ms: Option<i64>,
    }

    #[derive(Debug, Serialize)]
    struct WaitResult {
        status: HashMap<ThreadId, AgentStatus>,
        timed_out: bool,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: WaitArgs = parse_arguments(&arguments)?;
        if args.ids.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "ids must be non-empty".to_owned(),
            ));
        }
        let receiver_thread_ids = args
            .ids
            .iter()
            .map(|id| agent_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let timeout_ms = normalize_wait_timeout(args.timeout_ms)?;

        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: receiver_thread_ids.clone(),
                    receiver_names: HashMap::new(),
                    call_id: call_id.clone(),
                }
                .into(),
            )
            .await;

        let wait_result = match wait_for_agents(
            session.clone(),
            &receiver_thread_ids,
            timeout_ms,
            WaitMode::Any,
        )
        .await
        {
            Ok(result) => result,
            Err((id, err)) => {
                let statuses =
                    HashMap::from([(id, session.services.agent_control.get_status(id).await)]);
                session
                    .send_event(
                        &turn,
                        CollabWaitingEndEvent {
                            sender_thread_id: session.conversation_id,
                            call_id: call_id.clone(),
                            statuses,
                            receiver_names: HashMap::new(),
                        }
                        .into(),
                    )
                    .await;
                return Err(collab_agent_error(id, err));
            }
        };

        // Convert payload.
        let statuses_map = wait_result
            .statuses
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>();
        let result = WaitResult {
            status: statuses_map.clone(),
            timed_out: wait_result.timed_out,
        };

        // Final event emission.
        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id,
                    statuses: statuses_map,
                    receiver_names: HashMap::new(),
                }
                .into(),
            )
            .await;

        let content = serde_json::to_string(&result).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize wait result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: None,
        })
    }
}

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

mod spawn_team {
    use super::*;
    use crate::agent::next_thread_spawn_depth;
    use crate::agent::role::apply_role_to_config;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct SpawnTeamArgs {
        team_id: Option<String>,
        members: Vec<SpawnTeamMemberArgs>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct SpawnTeamMemberArgs {
        pub(super) name: String,
        pub(super) task: String,
        pub(super) agent_type: Option<String>,
        pub(super) model_provider: Option<String>,
        pub(super) model: Option<String>,
        #[serde(default)]
        pub(super) worktree: bool,
        #[serde(default, alias = "backendground")]
        pub(super) background: bool,
    }

    #[derive(Debug, Serialize)]
    struct SpawnTeamMemberResult {
        name: String,
        agent_id: String,
        status: AgentStatus,
    }

    #[derive(Debug, Serialize)]
    struct SpawnTeamResult {
        team_id: String,
        members: Vec<SpawnTeamMemberResult>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let SpawnTeamArgs {
            team_id: provided_team_id,
            members: requested_members,
        } = parse_arguments(&arguments)?;
        if requested_members.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "members must be non-empty".to_string(),
            ));
        }

        let mut seen_names = HashSet::new();
        for member in &requested_members {
            let name = member.name.trim();
            if name.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "member name must be non-empty".to_string(),
                ));
            }
            if !seen_names.insert(name.to_string()) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "duplicate member name `{name}`"
                )));
            }
            if member.task.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "task for member `{name}` must be non-empty"
                )));
            }
        }

        let team_id = match provided_team_id {
            Some(team_id) => normalized_team_id(&team_id)?,
            None => ThreadId::new().to_string(),
        };

        let child_depth = next_thread_spawn_depth(&turn.session_source);
        if exceeds_thread_spawn_depth_limit(child_depth) {
            return Err(FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string(),
            ));
        }
        let created_at = now_unix_seconds();

        let event_call_id = prefixed_team_call_id(TEAM_SPAWN_CALL_PREFIX, &call_id);
        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: Vec::new(),
                    receiver_names: HashMap::new(),
                    call_id: event_call_id.clone(),
                }
                .into(),
            )
            .await;

        let mut statuses = HashMap::new();
        let mut spawned_members = Vec::new();

        for member in &requested_members {
            let member_name = member.name.trim().to_string();
            let role_name = optional_non_empty(&member.agent_type, "agent_type")?;
            let model_provider = optional_non_empty(&member.model_provider, "model_provider")?;
            let model = optional_non_empty(&member.model, "model")?;
            let use_worktree = member.worktree;
            let _background = member.background;

            let mut config = build_agent_spawn_config(
                &session.get_base_instructions().await,
                turn.as_ref(),
                child_depth,
            )?;
            apply_role_to_config(&mut config, role_name)
                .await
                .map_err(FunctionCallError::RespondToModel)?;
            apply_member_model_overrides(&mut config, model_provider, model)?;
            apply_spawn_agent_overrides(&mut config, child_depth);
            let worktree_lease = if use_worktree {
                match create_agent_worktree(&session, &turn).await {
                    Ok(lease) => {
                        config.cwd = lease.worktree_path.clone();
                        Some(lease)
                    }
                    Err(err) => {
                        cleanup_spawned_team_members(&session, &turn, &spawned_members).await;
                        session
                            .send_event(
                                &turn,
                                CollabWaitingEndEvent {
                                    sender_thread_id: session.conversation_id,
                                    call_id: event_call_id,
                                    statuses,
                                    receiver_names: team_member_names(&spawned_members),
                                }
                                .into(),
                            )
                            .await;
                        return Err(err);
                    }
                }
            } else {
                None
            };

            let input_items = vec![UserInput::Text {
                text: member.task.trim().to_string(),
                text_elements: Vec::new(),
            }];
            let spawn_result = session
                .services
                .agent_control
                .spawn_agent(
                    config,
                    input_items,
                    Some(thread_spawn_source(session.conversation_id, child_depth)),
                )
                .await
                .map_err(collab_spawn_error);

            let agent_id = match spawn_result {
                Ok(agent_id) => {
                    if let Some(lease) = worktree_lease {
                        register_worktree_lease(agent_id, lease);
                    }
                    agent_id
                }
                Err(err) => {
                    if let Some(lease) = worktree_lease {
                        let _ = remove_worktree_lease(&session, &turn, lease).await;
                    }
                    cleanup_spawned_team_members(&session, &turn, &spawned_members).await;
                    session
                        .send_event(
                            &turn,
                            CollabWaitingEndEvent {
                                sender_thread_id: session.conversation_id,
                                call_id: event_call_id,
                                statuses,
                                receiver_names: team_member_names(&spawned_members),
                            }
                            .into(),
                        )
                        .await;
                    return Err(err);
                }
            };

            let status = session.services.agent_control.get_status(agent_id).await;
            statuses.insert(agent_id, status);
            spawned_members.push(TeamMember {
                name: member_name,
                agent_id,
                agent_type: member.agent_type.clone(),
            });
        }
        let team_record = TeamRecord {
            members: spawned_members.clone(),
            created_at,
        };

        if let Err(err) = insert_team_record(
            session.conversation_id,
            team_id.clone(),
            team_record.clone(),
        ) {
            cleanup_spawned_team_members(&session, &turn, &spawned_members).await;
            session
                .send_event(
                    &turn,
                    CollabWaitingEndEvent {
                        sender_thread_id: session.conversation_id,
                        call_id: event_call_id,
                        statuses,
                        receiver_names: team_member_names(&spawned_members),
                    }
                    .into(),
                )
                .await;
            return Err(err);
        }
        let initial_tasks =
            build_initial_team_tasks(&requested_members, &spawned_members, created_at);
        if let Err(err) = persist_team_state(
            turn.config.codex_home.as_path(),
            session.conversation_id,
            &team_id,
            &team_record,
            Some(&initial_tasks),
        )
        .await
        {
            let _ = remove_team_record(session.conversation_id, &team_id);
            let _ = remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await;
            cleanup_spawned_team_members(&session, &turn, &spawned_members).await;
            session
                .send_event(
                    &turn,
                    CollabWaitingEndEvent {
                        sender_thread_id: session.conversation_id,
                        call_id: event_call_id,
                        statuses,
                        receiver_names: team_member_names(&spawned_members),
                    }
                    .into(),
                )
                .await;
            return Err(err);
        }

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id: event_call_id,
                    statuses: statuses.clone(),
                    receiver_names: team_member_names(&spawned_members),
                }
                .into(),
            )
            .await;

        let members = spawned_members
            .into_iter()
            .map(|member| SpawnTeamMemberResult {
                status: statuses
                    .get(&member.agent_id)
                    .cloned()
                    .unwrap_or(AgentStatus::NotFound),
                name: member.name,
                agent_id: member.agent_id.to_string(),
            })
            .collect::<Vec<_>>();
        let content =
            serde_json::to_string(&SpawnTeamResult { team_id, members }).map_err(|err| {
                FunctionCallError::Fatal(format!("failed to serialize spawn_team result: {err}"))
            })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod wait_team {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum WaitTeamModeArg {
        Any,
        All,
    }

    #[derive(Debug, Deserialize)]
    struct WaitTeamArgs {
        team_id: String,
        mode: Option<WaitTeamModeArg>,
        timeout_ms: Option<i64>,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "lowercase")]
    enum WaitTeamMode {
        Any,
        All,
    }

    #[derive(Debug, Serialize)]
    struct WaitTeamTriggeredMember {
        name: String,
        agent_id: String,
    }

    #[derive(Debug, Serialize)]
    struct WaitTeamMemberStatus {
        name: String,
        agent_id: String,
        state: AgentStatus,
    }

    #[derive(Debug, Serialize)]
    struct WaitTeamResult {
        completed: bool,
        mode: WaitTeamMode,
        triggered_member: Option<WaitTeamTriggeredMember>,
        member_statuses: Vec<WaitTeamMemberStatus>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: WaitTeamArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        if team.members.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "team `{team_id}` has no members"
            )));
        }

        let (wait_mode, output_mode) = match args.mode.unwrap_or(WaitTeamModeArg::All) {
            WaitTeamModeArg::Any => (WaitMode::Any, WaitTeamMode::Any),
            WaitTeamModeArg::All => (WaitMode::All, WaitTeamMode::All),
        };
        let timeout_ms = normalize_wait_timeout(args.timeout_ms)?;
        let receiver_thread_ids = team
            .members
            .iter()
            .map(|member| member.agent_id)
            .collect::<Vec<_>>();
        let receiver_names = team_member_names(&team.members);
        let event_call_id = prefixed_team_call_id(TEAM_WAIT_CALL_PREFIX, &call_id);

        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: receiver_thread_ids.clone(),
                    receiver_names: receiver_names.clone(),
                    call_id: event_call_id.clone(),
                }
                .into(),
            )
            .await;

        let wait_result =
            match wait_for_agents(session.clone(), &receiver_thread_ids, timeout_ms, wait_mode)
                .await
            {
                Ok(result) => result,
                Err((id, err)) => {
                    let statuses =
                        HashMap::from([(id, session.services.agent_control.get_status(id).await)]);
                    session
                        .send_event(
                            &turn,
                            CollabWaitingEndEvent {
                                sender_thread_id: session.conversation_id,
                                call_id: event_call_id,
                                statuses,
                                receiver_names: receiver_names.clone(),
                            }
                            .into(),
                        )
                        .await;
                    return Err(collab_agent_error(id, err));
                }
            };

        let final_statuses = wait_result
            .statuses
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>();
        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id: event_call_id,
                    statuses: final_statuses.clone(),
                    receiver_names: receiver_names.clone(),
                }
                .into(),
            )
            .await;

        let mut member_statuses = Vec::with_capacity(team.members.len());
        for member in &team.members {
            let state = match final_statuses.get(&member.agent_id) {
                Some(state) => state.clone(),
                None => {
                    session
                        .services
                        .agent_control
                        .get_status(member.agent_id)
                        .await
                }
            };
            member_statuses.push(WaitTeamMemberStatus {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                state,
            });
        }

        let triggered_member = if wait_mode == WaitMode::Any && !wait_result.statuses.is_empty() {
            let (triggered_id, _) = wait_result.statuses[0];
            team.members
                .iter()
                .find(|member| member.agent_id == triggered_id)
                .map(|member| WaitTeamTriggeredMember {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                })
        } else {
            None
        };

        let completed = match wait_mode {
            WaitMode::Any => !wait_result.timed_out && !wait_result.statuses.is_empty(),
            WaitMode::All => {
                !wait_result.timed_out
                    && member_statuses
                        .iter()
                        .all(|entry| crate::agent::status::is_final(&entry.state))
            }
        };

        let content = serde_json::to_string(&WaitTeamResult {
            completed,
            mode: output_mode,
            triggered_member,
            member_statuses,
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize wait_team result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod close_team {
    use super::*;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct CloseTeamArgs {
        team_id: String,
        members: Option<Vec<String>>,
    }

    #[derive(Debug, Serialize)]
    struct CloseTeamMemberResult {
        name: String,
        agent_id: String,
        ok: bool,
        status: AgentStatus,
        error: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct CloseTeamResult {
        team_id: String,
        closed: Vec<CloseTeamMemberResult>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: CloseTeamArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        if team.members.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "team `{team_id}` has no members"
            )));
        }

        let selected_names = match args.members {
            Some(names) => {
                if names.is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "members must be non-empty when provided".to_string(),
                    ));
                }
                let mut selected = HashSet::new();
                for name in names {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return Err(FunctionCallError::RespondToModel(
                            "member name must be non-empty".to_string(),
                        ));
                    }
                    selected.insert(name);
                }
                selected
            }
            None => team
                .members
                .iter()
                .map(|member| member.name.clone())
                .collect(),
        };

        let selected_members = team
            .members
            .iter()
            .filter(|member| selected_names.contains(&member.name))
            .cloned()
            .collect::<Vec<_>>();
        if selected_members.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "no matching team members found".to_string(),
            ));
        }

        let event_call_id = prefixed_team_call_id(TEAM_CLOSE_CALL_PREFIX, &call_id);
        let receiver_names = team_member_names(&selected_members);
        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: selected_members
                        .iter()
                        .map(|member| member.agent_id)
                        .collect(),
                    receiver_names: receiver_names.clone(),
                    call_id: event_call_id.clone(),
                }
                .into(),
            )
            .await;

        let mut statuses = HashMap::new();
        let mut closed = Vec::with_capacity(selected_members.len());
        for member in &selected_members {
            let status_before = session
                .services
                .agent_control
                .get_status(member.agent_id)
                .await;
            let close_result =
                if matches!(status_before, AgentStatus::Shutdown | AgentStatus::NotFound) {
                    Ok(String::new())
                } else {
                    session
                        .services
                        .agent_control
                        .shutdown_agent(member.agent_id)
                        .await
                };
            let status_after = session
                .services
                .agent_control
                .get_status(member.agent_id)
                .await;
            let event_status = match (&status_before, &close_result, status_after) {
                (_, Err(_), status_after) => status_after,
                (AgentStatus::NotFound, Ok(_), _) => AgentStatus::NotFound,
                (AgentStatus::Shutdown, Ok(_), _) => AgentStatus::Shutdown,
                (_, Ok(_), AgentStatus::NotFound) => AgentStatus::Shutdown,
                (_, Ok(_), status_after) => status_after,
            };
            statuses.insert(member.agent_id, event_status);
            let cleanup_error =
                cleanup_agent_worktree(session.as_ref(), turn.as_ref(), member.agent_id)
                    .await
                    .err();

            match (close_result, cleanup_error) {
                (Ok(_), None) => closed.push(CloseTeamMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: true,
                    status: status_before,
                    error: None,
                }),
                (Ok(_), Some(cleanup_err)) => closed.push(CloseTeamMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(cleanup_err),
                }),
                (Err(err), None) => closed.push(CloseTeamMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(format!("{err}")),
                }),
                (Err(err), Some(cleanup_err)) => closed.push(CloseTeamMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(format!("{err}; {cleanup_err}")),
                }),
            }
        }

        let remaining_team = remove_members_from_team(
            session.conversation_id,
            &team_id,
            &selected_members
                .iter()
                .map(|member| member.name.clone())
                .collect::<Vec<_>>(),
        )?;
        let persistence_result = if let Some(team) = remaining_team.as_ref() {
            persist_team_state(
                turn.config.codex_home.as_path(),
                session.conversation_id,
                &team_id,
                team,
                None,
            )
            .await
        } else {
            remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await
        };
        if let Err(err) = persistence_result {
            session
                .send_event(
                    &turn,
                    CollabWaitingEndEvent {
                        sender_thread_id: session.conversation_id,
                        call_id: event_call_id,
                        statuses,
                        receiver_names,
                    }
                    .into(),
                )
                .await;
            return Err(err);
        }

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id: event_call_id,
                    statuses,
                    receiver_names,
                }
                .into(),
            )
            .await;

        let content =
            serde_json::to_string(&CloseTeamResult { team_id, closed }).map_err(|err| {
                FunctionCallError::Fatal(format!("failed to serialize close_team result: {err}"))
            })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_task_list {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamTaskListArgs {
        team_id: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamTaskListResult {
        team_id: String,
        tasks: Vec<TeamTaskOutput>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        _call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamTaskListArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        let valid_member_agent_ids = team
            .members
            .iter()
            .map(|member| member.agent_id.to_string())
            .collect::<HashSet<_>>();

        let tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id)
            .await?
            .into_iter()
            .filter(|task| valid_member_agent_ids.contains(&task.assignee.agent_id))
            .map(TeamTaskOutput::from)
            .collect::<Vec<_>>();

        let content =
            serde_json::to_string(&TeamTaskListResult { team_id, tasks }).map_err(|err| {
                FunctionCallError::Fatal(format!(
                    "failed to serialize team_task_list result: {err}"
                ))
            })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_task_claim {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamTaskClaimArgs {
        team_id: String,
        task_id: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamTaskClaimResult {
        team_id: String,
        claimed: bool,
        task: TeamTaskOutput,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        _call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamTaskClaimArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let _ = get_team_record(session.conversation_id, &team_id)?;
        let mut task =
            read_team_task(turn.config.codex_home.as_path(), &team_id, &args.task_id).await?;

        match task.state {
            PersistedTaskState::Pending => {}
            PersistedTaskState::Claimed => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "task `{}` is already claimed",
                    task.id
                )));
            }
            PersistedTaskState::Completed => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "task `{}` is already completed",
                    task.id
                )));
            }
        }

        let tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
        if !dependencies_satisfied(&task, &tasks) {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` has unresolved dependencies",
                task.id
            )));
        }

        task.state = PersistedTaskState::Claimed;
        task.updated_at = now_unix_seconds();
        write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;

        let content = serde_json::to_string(&TeamTaskClaimResult {
            team_id,
            claimed: true,
            task: task.into(),
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize team_task_claim result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_task_claim_next {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamTaskClaimNextArgs {
        team_id: String,
        member_name: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct TeamTaskClaimNextResult {
        team_id: String,
        claimed: bool,
        task: Option<TeamTaskOutput>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        _call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamTaskClaimNextArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        let target_member = args
            .member_name
            .as_deref()
            .map(|member_name| find_team_member(&team, &team_id, member_name))
            .transpose()?;

        let mut tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
        let mut selected_index = None;
        for index in 0..tasks.len() {
            let candidate = &tasks[index];
            if candidate.state != PersistedTaskState::Pending {
                continue;
            }
            if let Some(member) = target_member.as_ref()
                && (candidate.assignee.name != member.name
                    || candidate.assignee.agent_id != member.agent_id.to_string())
            {
                continue;
            }
            if dependencies_satisfied(candidate, &tasks) {
                selected_index = Some(index);
                break;
            }
        }

        let result = if let Some(index) = selected_index {
            let mut task = tasks.swap_remove(index);
            task.state = PersistedTaskState::Claimed;
            task.updated_at = now_unix_seconds();
            write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;
            TeamTaskClaimNextResult {
                team_id,
                claimed: true,
                task: Some(task.into()),
            }
        } else {
            TeamTaskClaimNextResult {
                team_id,
                claimed: false,
                task: None,
            }
        };

        let content = serde_json::to_string(&result).map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize team_task_claim_next result: {err}"
            ))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_task_complete {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamTaskCompleteArgs {
        team_id: String,
        task_id: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamTaskCompleteResult {
        team_id: String,
        completed: bool,
        task: TeamTaskOutput,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        _call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamTaskCompleteArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let _ = get_team_record(session.conversation_id, &team_id)?;
        let mut task =
            read_team_task(turn.config.codex_home.as_path(), &team_id, &args.task_id).await?;

        if task.state == PersistedTaskState::Completed {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is already completed",
                task.id
            )));
        }

        task.state = PersistedTaskState::Completed;
        task.updated_at = now_unix_seconds();
        write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;

        let content = serde_json::to_string(&TeamTaskCompleteResult {
            team_id,
            completed: true,
            task: task.into(),
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize team_task_complete result: {err}"
            ))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_message {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamMessageArgs {
        team_id: String,
        member_name: String,
        message: Option<String>,
        items: Option<Vec<UserInput>>,
        #[serde(default)]
        interrupt: bool,
    }

    #[derive(Debug, Serialize)]
    struct TeamMessageResult {
        team_id: String,
        member_name: String,
        agent_id: String,
        submission_id: String,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamMessageArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        let member = find_team_member(&team, &team_id, &args.member_name)?;
        let input_items = parse_collab_input(args.message, args.items)?;
        let prompt = input_preview(&input_items);
        let submission_id = send_input_to_member(
            &session,
            &turn,
            call_id,
            member.agent_id,
            input_items,
            prompt,
            args.interrupt,
        )
        .await?;

        let content = serde_json::to_string(&TeamMessageResult {
            team_id,
            member_name: member.name,
            agent_id: member.agent_id.to_string(),
            submission_id,
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize team_message result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_broadcast {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamBroadcastArgs {
        team_id: String,
        message: Option<String>,
        items: Option<Vec<UserInput>>,
        #[serde(default)]
        interrupt: bool,
    }

    #[derive(Debug, Serialize)]
    struct TeamBroadcastSent {
        member_name: String,
        agent_id: String,
        submission_id: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamBroadcastFailed {
        member_name: String,
        agent_id: String,
        error: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamBroadcastResult {
        team_id: String,
        sent: Vec<TeamBroadcastSent>,
        failed: Vec<TeamBroadcastFailed>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamBroadcastArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        let input_items = parse_collab_input(args.message, args.items)?;
        let prompt = input_preview(&input_items);
        let mut sent = Vec::new();
        let mut failed = Vec::new();

        for member in &team.members {
            let member_call_id = format!("{call_id}:{}", member.name);
            match send_input_to_member(
                &session,
                &turn,
                member_call_id,
                member.agent_id,
                input_items.clone(),
                prompt.clone(),
                args.interrupt,
            )
            .await
            {
                Ok(submission_id) => sent.push(TeamBroadcastSent {
                    member_name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    submission_id,
                }),
                Err(err) => failed.push(TeamBroadcastFailed {
                    member_name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    error: err.to_string(),
                }),
            }
        }

        let content = serde_json::to_string(&TeamBroadcastResult {
            team_id,
            sent,
            failed,
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize team_broadcast result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

mod team_cleanup {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TeamCleanupArgs {
        team_id: String,
    }

    #[derive(Debug, Serialize)]
    struct TeamCleanupMemberResult {
        name: String,
        agent_id: String,
        ok: bool,
        status: AgentStatus,
        error: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct TeamCleanupResult {
        team_id: String,
        removed_from_registry: bool,
        removed_team_config: bool,
        removed_task_dir: bool,
        closed: Vec<TeamCleanupMemberResult>,
    }

    pub async fn handle(
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        call_id: String,
        arguments: String,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: TeamCleanupArgs = parse_arguments(&arguments)?;
        let team_id = normalized_team_id(&args.team_id)?;
        let team = get_team_record(session.conversation_id, &team_id)?;
        let selected_members = team.members.clone();
        let receiver_names = team_member_names(&selected_members);
        let event_call_id = prefixed_team_call_id(TEAM_CLOSE_CALL_PREFIX, &call_id);
        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    sender_thread_id: session.conversation_id,
                    receiver_thread_ids: selected_members
                        .iter()
                        .map(|member| member.agent_id)
                        .collect(),
                    receiver_names: receiver_names.clone(),
                    call_id: event_call_id.clone(),
                }
                .into(),
            )
            .await;

        let mut statuses = HashMap::new();
        let mut closed = Vec::with_capacity(selected_members.len());
        for member in &selected_members {
            let status_before = session
                .services
                .agent_control
                .get_status(member.agent_id)
                .await;
            let close_result =
                if matches!(status_before, AgentStatus::Shutdown | AgentStatus::NotFound) {
                    Ok(String::new())
                } else {
                    session
                        .services
                        .agent_control
                        .shutdown_agent(member.agent_id)
                        .await
                };
            let status_after = session
                .services
                .agent_control
                .get_status(member.agent_id)
                .await;
            let event_status = match (&status_before, &close_result, status_after) {
                (_, Err(_), status_after) => status_after,
                (AgentStatus::NotFound, Ok(_), _) => AgentStatus::NotFound,
                (AgentStatus::Shutdown, Ok(_), _) => AgentStatus::Shutdown,
                (_, Ok(_), AgentStatus::NotFound) => AgentStatus::Shutdown,
                (_, Ok(_), status_after) => status_after,
            };
            statuses.insert(member.agent_id, event_status);

            let cleanup_error =
                cleanup_agent_worktree(session.as_ref(), turn.as_ref(), member.agent_id)
                    .await
                    .err();
            match (close_result, cleanup_error) {
                (Ok(_), None) => closed.push(TeamCleanupMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: true,
                    status: status_before,
                    error: None,
                }),
                (Ok(_), Some(cleanup_err)) => closed.push(TeamCleanupMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(cleanup_err),
                }),
                (Err(err), None) => closed.push(TeamCleanupMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(err.to_string()),
                }),
                (Err(err), Some(cleanup_err)) => closed.push(TeamCleanupMemberResult {
                    name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    ok: false,
                    status: status_before,
                    error: Some(format!("{err}; {cleanup_err}")),
                }),
            }
        }

        let remaining = remove_members_from_team(
            session.conversation_id,
            &team_id,
            &selected_members
                .iter()
                .map(|member| member.name.clone())
                .collect::<Vec<_>>(),
        )?;
        let removed_from_registry = remaining.is_none();
        if removed_from_registry {
            remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await?;
        }

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.conversation_id,
                    call_id: event_call_id,
                    statuses,
                    receiver_names,
                }
                .into(),
            )
            .await;

        let content = serde_json::to_string(&TeamCleanupResult {
            team_id: team_id.clone(),
            removed_from_registry,
            removed_team_config: removed_from_registry,
            removed_task_dir: removed_from_registry,
            closed,
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize team_cleanup result: {err}"))
        })?;

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        })
    }
}

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
            Err(err) => {
                let status = session.services.agent_control.get_status(agent_id).await;
                session
                    .send_event(
                        &turn,
                        CollabCloseEndEvent {
                            call_id: call_id.clone(),
                            sender_thread_id: session.conversation_id,
                            receiver_thread_id: agent_id,
                            status,
                        }
                        .into(),
                    )
                    .await;
                return Err(collab_agent_error(agent_id, err));
            }
        };
        let result = if !matches!(status, AgentStatus::Shutdown) {
            session
                .services
                .agent_control
                .shutdown_agent(agent_id)
                .await
                .map_err(|err| collab_agent_error(agent_id, err))
                .map(|_| ())
        } else {
            Ok(())
        };
        session
            .send_event(
                &turn,
                CollabCloseEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    receiver_thread_id: agent_id,
                    status: status.clone(),
                }
                .into(),
            )
            .await;
        result?;
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
    if exceeds_thread_spawn_depth_limit(child_depth + 1) {
        config.features.disable(Feature::Collab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthManager;
    use crate::CodexAuth;
    use crate::ThreadManager;
    use crate::agent::MAX_THREAD_SPAWN_DEPTH;
    use crate::built_in_model_providers;
    use crate::codex::make_session_and_context;
    use crate::config::types::ShellEnvironmentPolicy;
    use crate::function_tool::FunctionCallError;
    use crate::protocol::AskForApproval;
    use crate::protocol::Op;
    use crate::protocol::SandboxPolicy;
    use crate::protocol::SessionSource;
    use crate::protocol::SubAgentSource;
    use crate::turn_diff_tracker::TurnDiffTracker;
    use codex_protocol::ThreadId;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::protocol::InitialHistory;
    use codex_protocol::protocol::RolloutItem;
    use pretty_assertions::assert_eq;
    use serde::Deserialize;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio::time::timeout;

    fn invocation(
        session: Arc<crate::codex::Session>,
        turn: Arc<TurnContext>,
        tool_name: &str,
        payload: ToolPayload,
    ) -> ToolInvocation {
        ToolInvocation {
            session,
            turn,
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "call-1".to_string(),
            tool_name: tool_name.to_string(),
            payload,
        }
    }

    fn function_payload(args: serde_json::Value) -> ToolPayload {
        ToolPayload::Function {
            arguments: args.to_string(),
        }
    }

    fn thread_manager() -> ThreadManager {
        ThreadManager::with_models_provider_for_tests(
            CodexAuth::from_api_key("dummy"),
            built_in_model_providers()["openai"].clone(),
        )
    }

    fn run_git(path: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .expect("git command should run");
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn init_git_repo(path: &Path) {
        run_git(path, &["init", "--initial-branch=main"]);
        run_git(path, &["config", "user.name", "Codex Tests"]);
        run_git(path, &["config", "user.email", "codex-tests@example.com"]);
        std::fs::write(path.join("README.md"), "seed\n").expect("write seed file");
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-m", "seed"]);
    }

    fn list_worktree_paths(codex_home: &Path, lead_thread_id: ThreadId) -> Vec<PathBuf> {
        let root = codex_home
            .join(WORKTREE_ROOT_DIR)
            .join(lead_thread_id.to_string());
        if !root.exists() {
            return Vec::new();
        }

        let mut worktrees = Vec::new();
        for entry in std::fs::read_dir(root).expect("read worktree root") {
            let entry = entry.expect("read worktree dir entry");
            let metadata = entry.metadata().expect("read worktree metadata");
            if metadata.is_dir() {
                worktrees.push(entry.path());
            }
        }
        worktrees.sort();
        worktrees
    }

    #[test]
    fn team_member_names_formats_agent_type() {
        let typed_id = ThreadId::new();
        let blank_id = ThreadId::new();
        let none_id = ThreadId::new();
        let members = vec![
            TeamMember {
                name: "typed".to_string(),
                agent_id: typed_id,
                agent_type: Some(" reviewer ".to_string()),
            },
            TeamMember {
                name: "blank".to_string(),
                agent_id: blank_id,
                agent_type: Some("   ".to_string()),
            },
            TeamMember {
                name: "none".to_string(),
                agent_id: none_id,
                agent_type: None,
            },
        ];

        let names = team_member_names(&members);
        assert_eq!(names.len(), 3);
        assert_eq!(
            names.get(&typed_id).map(std::string::String::as_str),
            Some("typed [reviewer]")
        );
        assert_eq!(
            names.get(&blank_id).map(std::string::String::as_str),
            Some("blank [default]")
        );
        assert_eq!(
            names.get(&none_id).map(std::string::String::as_str),
            Some("none [default]")
        );
    }

    #[tokio::test]
    async fn handler_rejects_non_function_payloads() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            ToolPayload::Custom {
                input: "hello".to_string(),
            },
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("payload should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "collab handler received unsupported payload".to_string()
            )
        );
    }

    #[tokio::test]
    async fn handler_rejects_unknown_tool() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "unknown_tool",
            function_payload(json!({})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("tool should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("unsupported collab tool unknown_tool".to_string())
        );
    }

    #[tokio::test]
    async fn spawn_agent_rejects_empty_message() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({"message": "   "})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("empty message should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Empty message can't be sent to an agent".to_string()
            )
        );
    }

    #[tokio::test]
    async fn spawn_agent_rejects_when_message_and_items_are_both_set() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "hello",
                "items": [{"type": "mention", "name": "drive", "path": "app://drive"}]
            })),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("message+items should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Provide either message or items, but not both".to_string()
            )
        );
    }

    #[tokio::test]
    async fn spawn_agent_uses_explorer_role_and_sets_never_approval_policy() {
        #[derive(Debug, Deserialize)]
        struct SpawnAgentResult {
            agent_id: String,
        }

        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let expected_model = turn.model_info.slug.clone();
        let mut config = (*turn.config).clone();
        config
            .permissions
            .approval_policy
            .set(AskForApproval::OnRequest)
            .expect("approval policy should be set");
        turn.config = Arc::new(config);
        let explorer_config_path = turn.config.codex_home.join("agents").join("explorer.toml");
        tokio::fs::create_dir_all(
            explorer_config_path
                .parent()
                .expect("explorer config should have a parent dir"),
        )
        .await
        .expect("create agents directory");
        tokio::fs::write(&explorer_config_path, "model_reasoning_effort = \"high\"")
            .await
            .expect("write explorer role config");

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "agent_type": "explorer"
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("spawn_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: SpawnAgentResult =
            serde_json::from_str(&content).expect("spawn_agent result should be json");
        let agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");
        let snapshot = manager
            .get_thread(agent_id)
            .await
            .expect("spawned agent thread should exist")
            .config_snapshot()
            .await;
        assert_eq!(snapshot.model, expected_model);
        assert_eq!(
            snapshot.reasoning_effort,
            Some(codex_protocol::openai_models::ReasoningEffort::High)
        );
        assert_eq!(snapshot.approval_policy, AskForApproval::Never);
    }

    #[tokio::test]
    async fn spawn_agent_accepts_model_provider_and_model_overrides() {
        #[derive(Debug, Deserialize)]
        struct SpawnAgentResult {
            agent_id: String,
        }

        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "model_provider": "openai",
                "model": "gpt-5"
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("spawn_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: SpawnAgentResult =
            serde_json::from_str(&content).expect("spawn_agent result should be json");
        let agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");
        let snapshot = manager
            .get_thread(agent_id)
            .await
            .expect("spawned agent thread should exist")
            .config_snapshot()
            .await;
        assert_eq!(snapshot.model_provider_id, "openai");
        assert_eq!(snapshot.model, "gpt-5");
    }

    #[tokio::test]
    async fn spawn_agent_accepts_backendground_alias() {
        #[derive(Debug, Deserialize)]
        struct SpawnAgentResult {
            agent_id: String,
        }

        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "backendground": true
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("spawn_agent should accept backendground alias");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: SpawnAgentResult =
            serde_json::from_str(&content).expect("spawn_agent result should be json");
        let agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");
        let status = manager.agent_control().get_status(agent_id).await;
        assert_ne!(status, AgentStatus::NotFound);

        let _ = manager
            .agent_control()
            .shutdown_agent(agent_id)
            .await
            .expect("shutdown spawned agent");
    }

    #[tokio::test]
    async fn spawn_agent_accepts_background_field() {
        #[derive(Debug, Deserialize)]
        struct SpawnAgentResult {
            agent_id: String,
        }

        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "background": true
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("spawn_agent should accept background field");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: SpawnAgentResult =
            serde_json::from_str(&content).expect("spawn_agent result should be json");
        let agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");
        let status = manager.agent_control().get_status(agent_id).await;
        assert_ne!(status, AgentStatus::NotFound);

        let _ = manager
            .agent_control()
            .shutdown_agent(agent_id)
            .await
            .expect("shutdown spawned agent");
    }

    #[tokio::test]
    async fn spawn_agent_rejects_worktree_outside_git_repo() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let non_repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = non_repo_dir.path().to_path_buf();

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "worktree": true
            })),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("spawn_agent should fail when cwd is not in a git repo");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "worktree=true requires running inside a git repository".to_string()
            )
        );
    }

    #[tokio::test]
    async fn spawn_agent_worktree_sets_cwd_and_close_agent_cleans_up() {
        #[derive(Debug, Deserialize)]
        struct SpawnAgentResult {
            agent_id: String,
        }

        #[derive(Debug, Deserialize)]
        struct CloseAgentResult {
            status: AgentStatus,
        }

        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = repo_dir.path().to_path_buf();

        init_git_repo(turn.cwd.as_path());
        let lead_thread_id = session.conversation_id;
        let codex_home = turn.config.codex_home.clone();
        let expected_worktree_root = codex_home
            .join(WORKTREE_ROOT_DIR)
            .join(lead_thread_id.to_string());
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "worktree": true
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_agent with worktree should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnAgentResult =
            serde_json::from_str(&spawn_content).expect("spawn_agent result should be json");
        assert_eq!(spawn_success, Some(true));

        let agent_id = agent_id(&spawn_result.agent_id).expect("agent id should be valid");
        let snapshot = manager
            .get_thread(agent_id)
            .await
            .expect("spawned agent should exist")
            .config_snapshot()
            .await;
        assert_eq!(snapshot.cwd.starts_with(&expected_worktree_root), true);
        assert_ne!(snapshot.cwd, turn.cwd);
        assert_eq!(snapshot.cwd.exists(), true);
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).len(),
            1
        );

        let close_invocation = invocation(
            session,
            turn,
            "close_agent",
            function_payload(json!({
                "id": spawn_result.agent_id
            })),
        );
        let close_output = MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(close_content),
            success: close_success,
            ..
        } = close_output
        else {
            panic!("expected function output");
        };
        let close_result: CloseAgentResult =
            serde_json::from_str(&close_content).expect("close_agent result should be json");
        assert!(matches!(
            close_result.status,
            AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Shutdown
        ));
        assert_eq!(close_success, Some(true));
        assert_eq!(std::fs::metadata(&snapshot.cwd).is_err(), true);
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).is_empty(),
            true
        );
    }

    #[tokio::test]
    async fn spawn_agent_rejects_unknown_model_provider_override() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "inspect this repo",
                "model_provider": "missing-provider"
            })),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("unknown model provider should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "model_provider `missing-provider` not found".to_string()
            )
        );
    }

    #[tokio::test]
    async fn spawn_agent_errors_when_manager_dropped() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({"message": "hello"})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("spawn should fail without a manager");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn spawn_agent_rejects_when_depth_limit_exceeded() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        turn.session_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: session.conversation_id,
            depth: MAX_THREAD_SPAWN_DEPTH,
        });

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({"message": "hello"})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("spawn should fail when depth limit exceeded");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string()
            )
        );
    }

    #[tokio::test]
    async fn send_input_rejects_empty_message() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({"id": ThreadId::new().to_string(), "message": ""})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("empty message should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Empty message can't be sent to an agent".to_string()
            )
        );
    }

    #[tokio::test]
    async fn send_input_rejects_when_message_and_items_are_both_set() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({
                "id": ThreadId::new().to_string(),
                "message": "hello",
                "items": [{"type": "mention", "name": "drive", "path": "app://drive"}]
            })),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("message+items should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Provide either message or items, but not both".to_string()
            )
        );
    }

    #[tokio::test]
    async fn send_input_rejects_invalid_id() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({"id": "not-a-uuid", "message": "hi"})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("invalid id should be rejected");
        };
        let FunctionCallError::RespondToModel(msg) = err else {
            panic!("expected respond-to-model error");
        };
        assert!(msg.starts_with("invalid agent id not-a-uuid:"));
    }

    #[tokio::test]
    async fn send_input_reports_missing_agent() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let agent_id = ThreadId::new();
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({"id": agent_id.to_string(), "message": "hi"})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("missing agent should be reported");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} not found"))
        );
    }

    #[tokio::test]
    async fn send_input_interrupts_before_prompt() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({
                "id": agent_id.to_string(),
                "message": "hi",
                "interrupt": true
            })),
        );
        MultiAgentHandler
            .handle(invocation)
            .await
            .expect("send_input should succeed");

        let ops = manager.captured_ops();
        let ops_for_agent: Vec<&Op> = ops
            .iter()
            .filter_map(|(id, op)| (*id == agent_id).then_some(op))
            .collect();
        assert_eq!(ops_for_agent.len(), 2);
        assert!(matches!(ops_for_agent[0], Op::Interrupt));
        assert!(matches!(ops_for_agent[1], Op::UserInput { .. }));

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
    }

    #[tokio::test]
    async fn send_input_accepts_structured_items() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "send_input",
            function_payload(json!({
                "id": agent_id.to_string(),
                "items": [
                    {"type": "mention", "name": "drive", "path": "app://google_drive"},
                    {"type": "text", "text": "read the folder"}
                ]
            })),
        );
        MultiAgentHandler
            .handle(invocation)
            .await
            .expect("send_input should succeed");

        let expected = Op::UserInput {
            items: vec![
                UserInput::Mention {
                    name: "drive".to_string(),
                    path: "app://google_drive".to_string(),
                },
                UserInput::Text {
                    text: "read the folder".to_string(),
                    text_elements: Vec::new(),
                },
            ],
            final_output_json_schema: None,
        };
        let captured = manager
            .captured_ops()
            .into_iter()
            .find(|(id, op)| *id == agent_id && *op == expected);
        assert_eq!(captured, Some((agent_id, expected)));

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
    }

    #[tokio::test]
    async fn resume_agent_rejects_invalid_id() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "resume_agent",
            function_payload(json!({"id": "not-a-uuid"})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("invalid id should be rejected");
        };
        let FunctionCallError::RespondToModel(msg) = err else {
            panic!("expected respond-to-model error");
        };
        assert!(msg.starts_with("invalid agent id not-a-uuid:"));
    }

    #[tokio::test]
    async fn resume_agent_reports_missing_agent() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let agent_id = ThreadId::new();
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "resume_agent",
            function_payload(json!({"id": agent_id.to_string()})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("missing agent should be reported");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} not found"))
        );
    }

    #[tokio::test]
    async fn resume_agent_noops_for_active_agent() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let status_before = manager.agent_control().get_status(agent_id).await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "resume_agent",
            function_payload(json!({"id": agent_id.to_string()})),
        );

        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("resume_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: resume_agent::ResumeAgentResult =
            serde_json::from_str(&content).expect("resume_agent result should be json");
        assert_eq!(result.status, status_before);
        assert_eq!(success, Some(true));

        let thread_ids = manager.list_thread_ids().await;
        assert_eq!(thread_ids, vec![agent_id]);

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
    }

    #[tokio::test]
    async fn resume_agent_restores_closed_agent_and_accepts_send_input() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager
            .resume_thread_with_history(
                config,
                InitialHistory::Forked(vec![RolloutItem::ResponseItem(ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "materialized".to_string(),
                    }],
                    end_turn: None,
                    phase: None,
                })]),
                AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
                false,
            )
            .await
            .expect("start thread");
        let agent_id = thread.thread_id;
        let _ = manager
            .agent_control()
            .shutdown_agent(agent_id)
            .await
            .expect("shutdown agent");
        assert_eq!(
            manager.agent_control().get_status(agent_id).await,
            AgentStatus::NotFound
        );
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let resume_invocation = invocation(
            session.clone(),
            turn.clone(),
            "resume_agent",
            function_payload(json!({"id": agent_id.to_string()})),
        );
        let output = MultiAgentHandler
            .handle(resume_invocation)
            .await
            .expect("resume_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: resume_agent::ResumeAgentResult =
            serde_json::from_str(&content).expect("resume_agent result should be json");
        assert_ne!(result.status, AgentStatus::NotFound);
        assert_eq!(success, Some(true));

        let send_invocation = invocation(
            session,
            turn,
            "send_input",
            function_payload(json!({"id": agent_id.to_string(), "message": "hello"})),
        );
        let output = MultiAgentHandler
            .handle(send_invocation)
            .await
            .expect("send_input should succeed after resume");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: serde_json::Value =
            serde_json::from_str(&content).expect("send_input result should be json");
        let submission_id = result
            .get("submission_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert!(!submission_id.is_empty());
        assert_eq!(success, Some(true));

        let _ = manager
            .agent_control()
            .shutdown_agent(agent_id)
            .await
            .expect("shutdown resumed agent");
    }

    #[tokio::test]
    async fn resume_agent_rejects_when_depth_limit_exceeded() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();

        turn.session_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: session.conversation_id,
            depth: MAX_THREAD_SPAWN_DEPTH,
        });

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "resume_agent",
            function_payload(json!({"id": ThreadId::new().to_string()})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("resume should fail when depth limit exceeded");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "Agent depth limit reached. Solve the task yourself.".to_string()
            )
        );
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct WaitResult {
        status: HashMap<ThreadId, AgentStatus>,
        timed_out: bool,
    }

    #[tokio::test]
    async fn wait_rejects_non_positive_timeout() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({
                "ids": [ThreadId::new().to_string()],
                "timeout_ms": 0
            })),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("non-positive timeout should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("timeout_ms must be greater than zero".to_string())
        );
    }

    #[tokio::test]
    async fn wait_rejects_invalid_id() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({"ids": ["invalid"]})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("invalid id should be rejected");
        };
        let FunctionCallError::RespondToModel(msg) = err else {
            panic!("expected respond-to-model error");
        };
        assert!(msg.starts_with("invalid agent id invalid:"));
    }

    #[tokio::test]
    async fn wait_rejects_empty_ids() {
        let (session, turn) = make_session_and_context().await;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({"ids": []})),
        );
        let Err(err) = MultiAgentHandler.handle(invocation).await else {
            panic!("empty ids should be rejected");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("ids must be non-empty".to_string())
        );
    }

    #[tokio::test]
    async fn wait_returns_not_found_for_missing_agents() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let id_a = ThreadId::new();
        let id_b = ThreadId::new();
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({
                "ids": [id_a.to_string(), id_b.to_string()],
                "timeout_ms": 1000
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("wait should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: WaitResult =
            serde_json::from_str(&content).expect("wait result should be json");
        assert_eq!(
            result,
            WaitResult {
                status: HashMap::from([
                    (id_a, AgentStatus::NotFound),
                    (id_b, AgentStatus::NotFound),
                ]),
                timed_out: false
            }
        );
        assert_eq!(success, None);
    }

    #[tokio::test]
    async fn wait_times_out_when_status_is_not_final() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({
                "ids": [agent_id.to_string()],
                "timeout_ms": MIN_WAIT_TIMEOUT_MS
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("wait should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: WaitResult =
            serde_json::from_str(&content).expect("wait result should be json");
        assert_eq!(
            result,
            WaitResult {
                status: HashMap::new(),
                timed_out: true
            }
        );
        assert_eq!(success, None);

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
    }

    #[tokio::test]
    async fn wait_clamps_short_timeouts_to_minimum() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({
                "ids": [agent_id.to_string()],
                "timeout_ms": 10
            })),
        );

        let early = timeout(
            Duration::from_millis(50),
            MultiAgentHandler.handle(invocation),
        )
        .await;
        assert!(
            early.is_err(),
            "wait should not return before the minimum timeout clamp"
        );

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
    }

    #[tokio::test]
    async fn wait_returns_final_status_without_timeout() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let mut status_rx = manager
            .agent_control()
            .subscribe_status(agent_id)
            .await
            .expect("subscribe should succeed");

        let _ = thread
            .thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
        let _ = timeout(Duration::from_secs(1), status_rx.changed())
            .await
            .expect("shutdown status should arrive");

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "wait",
            function_payload(json!({
                "ids": [agent_id.to_string()],
                "timeout_ms": 1000
            })),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("wait should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: WaitResult =
            serde_json::from_str(&content).expect("wait result should be json");
        assert_eq!(
            result,
            WaitResult {
                status: HashMap::from([(agent_id, AgentStatus::Shutdown)]),
                timed_out: false
            }
        );
        assert_eq!(success, None);
    }

    #[tokio::test]
    async fn close_agent_submits_shutdown_and_returns_status() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let config = turn.config.as_ref().clone();
        let thread = manager.start_thread(config).await.expect("start thread");
        let agent_id = thread.thread_id;
        let status_before = manager.agent_control().get_status(agent_id).await;

        let invocation = invocation(
            Arc::new(session),
            Arc::new(turn),
            "close_agent",
            function_payload(json!({"id": agent_id.to_string()})),
        );
        let output = MultiAgentHandler
            .handle(invocation)
            .await
            .expect("close_agent should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success,
            ..
        } = output
        else {
            panic!("expected function output");
        };
        let result: close_agent::CloseAgentResult =
            serde_json::from_str(&content).expect("close_agent result should be json");
        assert_eq!(result.status, status_before);
        assert_eq!(success, Some(true));

        let ops = manager.captured_ops();
        let submitted_shutdown = ops
            .iter()
            .any(|(id, op)| *id == agent_id && matches!(op, Op::Shutdown));
        assert_eq!(submitted_shutdown, true);

        let status_after = manager.agent_control().get_status(agent_id).await;
        assert_eq!(status_after, AgentStatus::NotFound);
    }

    #[derive(Debug, Deserialize)]
    struct SpawnTeamResult {
        team_id: String,
        members: Vec<SpawnTeamMemberResult>,
    }

    #[derive(Debug, Deserialize)]
    struct SpawnTeamMemberResult {
        name: String,
        agent_id: String,
        status: AgentStatus,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum WaitTeamMode {
        Any,
        All,
    }

    #[derive(Debug, Deserialize)]
    struct WaitTeamTriggeredMember {
        name: String,
        agent_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct WaitTeamMemberStatus {
        name: String,
        agent_id: String,
        state: AgentStatus,
    }

    #[derive(Debug, Deserialize)]
    struct WaitTeamResult {
        completed: bool,
        mode: WaitTeamMode,
        triggered_member: Option<WaitTeamTriggeredMember>,
        member_statuses: Vec<WaitTeamMemberStatus>,
    }

    #[derive(Debug, Deserialize)]
    struct CloseTeamResult {
        team_id: String,
        closed: Vec<CloseTeamMemberResult>,
    }

    #[derive(Debug, Deserialize)]
    struct CloseTeamMemberResult {
        name: String,
        agent_id: String,
        ok: bool,
        status: AgentStatus,
        error: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct TeamTaskResult {
        task_id: String,
        title: String,
        state: PersistedTaskState,
        depends_on: Vec<String>,
        assignee_name: String,
        assignee_agent_id: String,
        updated_at: i64,
    }

    #[derive(Debug, Deserialize)]
    struct TeamTaskListResult {
        team_id: String,
        tasks: Vec<TeamTaskResult>,
    }

    #[derive(Debug, Deserialize)]
    struct TeamTaskClaimResult {
        team_id: String,
        claimed: bool,
        task: TeamTaskResult,
    }

    #[derive(Debug, Deserialize)]
    struct TeamTaskClaimNextResult {
        team_id: String,
        claimed: bool,
        task: Option<TeamTaskResult>,
    }

    #[derive(Debug, Deserialize)]
    struct TeamTaskCompleteResult {
        team_id: String,
        completed: bool,
        task: TeamTaskResult,
    }

    #[derive(Debug, Deserialize)]
    struct TeamMessageResult {
        team_id: String,
        member_name: String,
        agent_id: String,
        submission_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct TeamBroadcastSent {
        member_name: String,
        agent_id: String,
        submission_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct TeamBroadcastFailed {
        member_name: String,
        agent_id: String,
        error: String,
    }

    #[derive(Debug, Deserialize)]
    struct TeamBroadcastResult {
        team_id: String,
        sent: Vec<TeamBroadcastSent>,
        failed: Vec<TeamBroadcastFailed>,
    }

    #[derive(Debug, Deserialize)]
    struct TeamCleanupMemberResult {
        name: String,
        agent_id: String,
        ok: bool,
        status: AgentStatus,
        error: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct TeamCleanupResult {
        team_id: String,
        removed_from_registry: bool,
        removed_team_config: bool,
        removed_task_dir: bool,
        closed: Vec<TeamCleanupMemberResult>,
    }

    #[tokio::test]
    async fn spawn_team_wait_team_and_close_team_flow() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan the work"},
                    {"name": "worker", "task": "execute the task"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_success, Some(true));
        assert_eq!(spawn_result.members.len(), 2);
        assert_eq!(
            spawn_result
                .members
                .iter()
                .map(|member| member.name.clone())
                .collect::<Vec<_>>(),
            vec!["planner".to_string(), "worker".to_string()]
        );
        for member in &spawn_result.members {
            assert_eq!(member.status, AgentStatus::PendingInit);
        }
        let persisted_config_path =
            team_config_path(turn.config.codex_home.as_path(), &spawn_result.team_id);
        let persisted_config_raw = tokio::fs::read_to_string(&persisted_config_path)
            .await
            .expect("team config should be persisted");
        let persisted_config: PersistedTeamConfig =
            serde_json::from_str(&persisted_config_raw).expect("team config should be valid json");
        assert_eq!(persisted_config.team_name, spawn_result.team_id);
        assert_eq!(
            persisted_config.lead_thread_id,
            session.conversation_id.to_string()
        );
        assert_eq!(persisted_config.members.len(), 2);

        let persisted_tasks_dir =
            team_tasks_dir(turn.config.codex_home.as_path(), &spawn_result.team_id);
        let mut persisted_tasks = Vec::new();
        let mut tasks_dir = tokio::fs::read_dir(&persisted_tasks_dir)
            .await
            .expect("team tasks dir should exist");
        while let Some(entry) = tasks_dir
            .next_entry()
            .await
            .expect("tasks dir read should succeed")
        {
            let metadata = entry
                .metadata()
                .await
                .expect("task metadata read should succeed");
            if !metadata.is_file() {
                continue;
            }
            let task_raw = tokio::fs::read_to_string(entry.path())
                .await
                .expect("task file should be readable");
            let task: PersistedTeamTask =
                serde_json::from_str(&task_raw).expect("task file should be json");
            persisted_tasks.push(task);
        }
        assert_eq!(persisted_tasks.len(), 2);
        let mut task_titles = persisted_tasks
            .iter()
            .map(|task| task.title.clone())
            .collect::<Vec<_>>();
        task_titles.sort();
        assert_eq!(
            task_titles,
            vec!["execute the task".to_string(), "plan the work".to_string()]
        );
        for task in persisted_tasks {
            assert_eq!(task.state, PersistedTaskState::Pending);
            assert_eq!(task.depends_on.is_empty(), true);
        }

        for member in &spawn_result.members {
            let agent_id = agent_id(&member.agent_id).expect("valid agent id");
            let _ = manager
                .agent_control()
                .shutdown_agent(agent_id)
                .await
                .expect("shutdown spawned team member");
        }

        let wait_invocation = invocation(
            session.clone(),
            turn.clone(),
            "wait_team",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "mode": "all",
                "timeout_ms": 1_000
            })),
        );
        let wait_output = MultiAgentHandler
            .handle(wait_invocation)
            .await
            .expect("wait_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(wait_content),
            success: wait_success,
            ..
        } = wait_output
        else {
            panic!("expected function output");
        };
        let wait_result: WaitTeamResult =
            serde_json::from_str(&wait_content).expect("wait_team result should be json");
        assert_eq!(wait_success, Some(true));
        assert_eq!(wait_result.completed, true);
        assert!(matches!(wait_result.mode, WaitTeamMode::All));
        assert!(wait_result.triggered_member.is_none());
        assert_eq!(wait_result.member_statuses.len(), 2);
        for status in &wait_result.member_statuses {
            assert!(matches!(
                status.state,
                AgentStatus::NotFound | AgentStatus::Shutdown
            ));
        }

        let close_invocation = invocation(
            session.clone(),
            turn.clone(),
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let close_output = MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(close_content),
            success: close_success,
            ..
        } = close_output
        else {
            panic!("expected function output");
        };
        let close_result: CloseTeamResult =
            serde_json::from_str(&close_content).expect("close_team result should be json");
        assert_eq!(close_success, Some(true));
        assert_eq!(close_result.closed.len(), 2);
        assert_eq!(
            close_result
                .closed
                .iter()
                .map(|member| member.name.clone())
                .collect::<Vec<_>>(),
            vec!["planner".to_string(), "worker".to_string()]
        );
        for member in &close_result.closed {
            assert_eq!(member.ok, true);
            assert_eq!(member.error, None);
            assert!(!member.agent_id.is_empty());
            assert!(matches!(
                member.status,
                AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::NotFound
            ));
        }

        let wait_missing_invocation = invocation(
            session,
            turn.clone(),
            "wait_team",
            function_payload(json!({
                "team_id": close_result.team_id
            })),
        );
        let Err(err) = MultiAgentHandler.handle(wait_missing_invocation).await else {
            panic!("wait_team should fail after close_team removed the team");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(format!("team `{}` not found", close_result.team_id))
        );
        assert_eq!(
            tokio::fs::metadata(team_dir(
                turn.config.codex_home.as_path(),
                &close_result.team_id
            ))
            .await
            .is_err(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(
                turn.config.codex_home.as_path(),
                &close_result.team_id,
            ))
            .await
            .is_err(),
            true
        );
    }

    #[tokio::test]
    async fn spawn_team_accepts_backendground_alias() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {
                        "name": "planner",
                        "task": "plan the work",
                        "backendground": true
                    }
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should accept backendground alias");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_success, Some(true));
        assert_eq!(spawn_result.team_id, team_id);
        assert_eq!(spawn_result.members.len(), 1);

        let close_invocation = invocation(
            session,
            turn,
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let close_output = MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_team should succeed");
        let ToolOutput::Function {
            success: close_success,
            ..
        } = close_output
        else {
            panic!("expected function output");
        };
        assert_eq!(close_success, Some(true));
    }

    #[tokio::test]
    async fn spawn_team_accepts_background_field() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {
                        "name": "planner",
                        "task": "plan the work",
                        "background": true
                    }
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should accept background field");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_success, Some(true));
        assert_eq!(spawn_result.team_id, team_id);
        assert_eq!(spawn_result.members.len(), 1);

        let close_invocation = invocation(
            session,
            turn,
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let close_output = MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_team should succeed");
        let ToolOutput::Function {
            success: close_success,
            ..
        } = close_output
        else {
            panic!("expected function output");
        };
        assert_eq!(close_success, Some(true));
    }

    #[tokio::test]
    async fn spawn_team_worktree_failure_cleans_already_spawned_members() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let non_repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = non_repo_dir.path().to_path_buf();
        let team_id = ThreadId::new().to_string();
        let codex_home = turn.config.codex_home.clone();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work", "worktree": true}
                ]
            })),
        );
        let Err(err) = MultiAgentHandler.handle(spawn_invocation).await else {
            panic!("spawn_team should fail when worktree=true is used outside a git repo");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "worktree=true requires running inside a git repository".to_string()
            )
        );

        let shutdown_submitted = manager
            .captured_ops()
            .iter()
            .any(|(_, op)| matches!(op, Op::Shutdown));
        assert_eq!(shutdown_submitted, true);
        assert_eq!(
            tokio::fs::metadata(team_dir(codex_home.as_path(), &team_id))
                .await
                .is_err(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(codex_home.as_path(), &team_id))
                .await
                .is_err(),
            true
        );

        let wait_invocation = invocation(
            session,
            turn,
            "wait_team",
            function_payload(json!({
                "team_id": team_id
            })),
        );
        let Err(wait_err) = MultiAgentHandler.handle(wait_invocation).await else {
            panic!("wait_team should fail because the failed team was never persisted");
        };
        assert_eq!(
            wait_err,
            FunctionCallError::RespondToModel(format!("team `{team_id}` not found"))
        );
    }

    #[tokio::test]
    async fn close_team_cleans_worktree_leases_for_worktree_members() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = repo_dir.path().to_path_buf();

        init_git_repo(turn.cwd.as_path());
        let lead_thread_id = session.conversation_id;
        let codex_home = turn.config.codex_home.clone();
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {"name": "planner", "task": "plan", "worktree": true},
                    {"name": "worker", "task": "work", "worktree": true}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team with worktree members should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_result.members.len(), 2);
        assert_eq!(spawn_success, Some(true));
        let expected_worktree_root = codex_home
            .join(WORKTREE_ROOT_DIR)
            .join(lead_thread_id.to_string());
        for member in &spawn_result.members {
            let member_id = agent_id(&member.agent_id).expect("member agent id should be valid");
            let snapshot = manager
                .get_thread(member_id)
                .await
                .expect("spawned member should exist")
                .config_snapshot()
                .await;
            assert_eq!(snapshot.cwd.starts_with(&expected_worktree_root), true);
            assert_eq!(snapshot.cwd.exists(), true);
        }

        let worktree_paths = list_worktree_paths(codex_home.as_path(), lead_thread_id);
        assert_eq!(worktree_paths.len(), 2);
        for worktree_path in &worktree_paths {
            assert_eq!(worktree_path.exists(), true);
            assert_eq!(worktree_path.starts_with(&turn.cwd), false);
        }

        let close_invocation = invocation(
            session,
            turn,
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let close_output = MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(close_content),
            success: close_success,
            ..
        } = close_output
        else {
            panic!("expected function output");
        };
        let close_result: CloseTeamResult =
            serde_json::from_str(&close_content).expect("close_team result should be json");
        assert_eq!(close_result.closed.len(), 2);
        assert_eq!(close_success, Some(true));
        for member in &close_result.closed {
            assert_eq!(member.ok, true);
            assert_eq!(member.error, None);
        }
        for worktree_path in worktree_paths {
            assert_eq!(std::fs::metadata(worktree_path).is_err(), true);
        }
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).is_empty(),
            true
        );
    }

    #[tokio::test]
    async fn close_team_partial_close_removes_only_selected_member_worktree() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = repo_dir.path().to_path_buf();

        init_git_repo(turn.cwd.as_path());
        let lead_thread_id = session.conversation_id;
        let codex_home = turn.config.codex_home.clone();
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {"name": "planner", "task": "plan", "worktree": true},
                    {"name": "worker", "task": "work", "worktree": true}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team with worktree members should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_result.members.len(), 2);
        assert_eq!(spawn_success, Some(true));
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).len(),
            2
        );

        let partial_close_invocation = invocation(
            session.clone(),
            turn.clone(),
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "members": ["planner"]
            })),
        );
        let partial_close_output = MultiAgentHandler
            .handle(partial_close_invocation)
            .await
            .expect("close_team should close selected member");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(partial_close_content),
            success: partial_close_success,
            ..
        } = partial_close_output
        else {
            panic!("expected function output");
        };
        let partial_close_result: CloseTeamResult =
            serde_json::from_str(&partial_close_content).expect("close_team result should be json");
        assert_eq!(partial_close_success, Some(true));
        assert_eq!(partial_close_result.closed.len(), 1);
        assert_eq!(partial_close_result.closed[0].name, "planner".to_string());
        assert_eq!(partial_close_result.closed[0].ok, true);
        assert_eq!(partial_close_result.closed[0].error, None);
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).len(),
            1
        );

        let persisted_config_path = team_config_path(codex_home.as_path(), &spawn_result.team_id);
        let persisted_config_raw = tokio::fs::read_to_string(&persisted_config_path)
            .await
            .expect("team config should remain after partial close");
        let persisted_config: PersistedTeamConfig =
            serde_json::from_str(&persisted_config_raw).expect("team config should be valid json");
        assert_eq!(persisted_config.members.len(), 1);
        assert_eq!(persisted_config.members[0].name, "worker".to_string());

        let final_close_invocation = invocation(
            session,
            turn,
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let final_close_output = MultiAgentHandler
            .handle(final_close_invocation)
            .await
            .expect("close remaining team member");
        let ToolOutput::Function {
            success: final_close_success,
            ..
        } = final_close_output
        else {
            panic!("expected function output");
        };
        assert_eq!(final_close_success, Some(true));
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).is_empty(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_config_path(
                codex_home.as_path(),
                &spawn_result.team_id
            ))
            .await
            .is_err(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(codex_home.as_path(), &spawn_result.team_id))
                .await
                .is_err(),
            true
        );
    }

    #[tokio::test]
    async fn team_cleanup_removes_worktrees_when_members_are_already_shutdown() {
        let (mut session, mut turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let repo_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = repo_dir.path().to_path_buf();

        init_git_repo(turn.cwd.as_path());
        let lead_thread_id = session.conversation_id;
        let codex_home = turn.config.codex_home.clone();
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let team_id = ThreadId::new().to_string();

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": team_id,
                "members": [
                    {"name": "planner", "task": "plan", "worktree": true},
                    {"name": "worker", "task": "work", "worktree": true}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team with worktree members should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            success: spawn_success,
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        assert_eq!(spawn_result.members.len(), 2);
        assert_eq!(spawn_success, Some(true));
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).len(),
            2
        );

        for member in &spawn_result.members {
            let member_id = agent_id(&member.agent_id).expect("member agent id should be valid");
            manager
                .agent_control()
                .shutdown_agent(member_id)
                .await
                .expect("shutdown member before cleanup");
        }

        let cleanup_invocation = invocation(
            session,
            turn,
            "team_cleanup",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let cleanup_output = MultiAgentHandler
            .handle(cleanup_invocation)
            .await
            .expect("team_cleanup should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(cleanup_content),
            success: cleanup_success,
            ..
        } = cleanup_output
        else {
            panic!("expected function output");
        };
        let cleanup_result: TeamCleanupResult =
            serde_json::from_str(&cleanup_content).expect("team_cleanup result should be json");
        assert_eq!(cleanup_success, Some(true));
        assert_eq!(cleanup_result.closed.len(), 2);
        assert_eq!(cleanup_result.removed_from_registry, true);
        for member in &cleanup_result.closed {
            assert_eq!(member.ok, true);
            assert_eq!(member.error, None);
            assert!(matches!(
                member.status,
                AgentStatus::Shutdown | AgentStatus::NotFound
            ));
        }
        assert_eq!(
            list_worktree_paths(codex_home.as_path(), lead_thread_id).is_empty(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_config_path(
                codex_home.as_path(),
                &cleanup_result.team_id
            ))
            .await
            .is_err(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(
                codex_home.as_path(),
                &cleanup_result.team_id
            ))
            .await
            .is_err(),
            true
        );
    }

    #[tokio::test]
    async fn wait_team_any_returns_triggered_member() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "worker", "task": "do work"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        let member = &spawn_result.members[0];
        let member_id = agent_id(&member.agent_id).expect("valid agent id");
        let _ = manager
            .agent_control()
            .shutdown_agent(member_id)
            .await
            .expect("shutdown spawned team member");

        let wait_invocation = invocation(
            session.clone(),
            turn.clone(),
            "wait_team",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "mode": "any",
                "timeout_ms": 1_000
            })),
        );
        let wait_output = MultiAgentHandler
            .handle(wait_invocation)
            .await
            .expect("wait_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(wait_content),
            ..
        } = wait_output
        else {
            panic!("expected function output");
        };
        let wait_result: WaitTeamResult =
            serde_json::from_str(&wait_content).expect("wait_team result should be json");
        assert_eq!(wait_result.completed, true);
        assert!(matches!(wait_result.mode, WaitTeamMode::Any));
        let triggered = wait_result
            .triggered_member
            .expect("any mode should set triggered_member");
        assert_eq!(triggered.name, "worker".to_string());
        assert_eq!(triggered.agent_id, member.agent_id);
        assert_eq!(wait_result.member_statuses.len(), 1);
        assert_eq!(wait_result.member_statuses[0].name, "worker".to_string());
        assert_eq!(wait_result.member_statuses[0].agent_id, member.agent_id);

        let close_invocation = invocation(
            session,
            turn,
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        MultiAgentHandler
            .handle(close_invocation)
            .await
            .expect("close_team should clean up");
    }

    #[tokio::test]
    async fn close_team_partial_close_updates_persisted_team_config() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");

        let partial_close_invocation = invocation(
            session.clone(),
            turn.clone(),
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "members": ["planner"]
            })),
        );
        let partial_close_output = MultiAgentHandler
            .handle(partial_close_invocation)
            .await
            .expect("close_team should succeed for selected members");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(partial_close_content),
            ..
        } = partial_close_output
        else {
            panic!("expected function output");
        };
        let partial_close_result: CloseTeamResult =
            serde_json::from_str(&partial_close_content).expect("close_team result should be json");
        assert_eq!(partial_close_result.closed.len(), 1);
        assert_eq!(partial_close_result.closed[0].name, "planner".to_string());

        let persisted_config_path =
            team_config_path(turn.config.codex_home.as_path(), &spawn_result.team_id);
        let persisted_config_raw = tokio::fs::read_to_string(&persisted_config_path)
            .await
            .expect("team config should still exist");
        let persisted_config: PersistedTeamConfig =
            serde_json::from_str(&persisted_config_raw).expect("team config should be valid json");
        assert_eq!(persisted_config.members.len(), 1);
        assert_eq!(persisted_config.members[0].name, "worker".to_string());
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(
                turn.config.codex_home.as_path(),
                &spawn_result.team_id,
            ))
            .await
            .is_ok(),
            true
        );

        let full_close_invocation = invocation(
            session,
            turn.clone(),
            "close_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        MultiAgentHandler
            .handle(full_close_invocation)
            .await
            .expect("close remaining team member");
        assert_eq!(
            tokio::fs::metadata(team_config_path(
                turn.config.codex_home.as_path(),
                &spawn_result.team_id,
            ))
            .await
            .is_err(),
            true
        );
    }

    #[tokio::test]
    async fn team_task_lifecycle_and_team_cleanup_flow() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");

        let list_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_list",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        );
        let list_output = MultiAgentHandler
            .handle(list_invocation)
            .await
            .expect("team_task_list should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(list_content),
            ..
        } = list_output
        else {
            panic!("expected function output");
        };
        let list_result: TeamTaskListResult =
            serde_json::from_str(&list_content).expect("team_task_list should return json");
        assert_eq!(list_result.tasks.len(), 2);
        assert_eq!(list_result.team_id, spawn_result.team_id);
        for task in &list_result.tasks {
            assert_eq!(task.state, PersistedTaskState::Pending);
            assert_eq!(task.depends_on.is_empty(), true);
            assert_eq!(task.updated_at > 0, true);
            assert_eq!(task.title.is_empty(), false);
            assert_eq!(task.assignee_agent_id.is_empty(), false);
        }

        let claim_next_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_claim_next",
            function_payload(json!({
                "team_id": list_result.team_id,
                "member_name": "planner"
            })),
        );
        let claim_next_output = MultiAgentHandler
            .handle(claim_next_invocation)
            .await
            .expect("team_task_claim_next should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(claim_next_content),
            ..
        } = claim_next_output
        else {
            panic!("expected function output");
        };
        let claim_next_result: TeamTaskClaimNextResult = serde_json::from_str(&claim_next_content)
            .expect("team_task_claim_next should return json");
        assert_eq!(claim_next_result.claimed, true);
        let claimed_task = claim_next_result
            .task
            .expect("team_task_claim_next should return task");
        assert_eq!(claimed_task.assignee_name, "planner".to_string());
        assert_eq!(claimed_task.state, PersistedTaskState::Claimed);

        let complete_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_complete",
            function_payload(json!({
                "team_id": claim_next_result.team_id,
                "task_id": claimed_task.task_id
            })),
        );
        let complete_output = MultiAgentHandler
            .handle(complete_invocation)
            .await
            .expect("team_task_complete should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(complete_content),
            ..
        } = complete_output
        else {
            panic!("expected function output");
        };
        let complete_result: TeamTaskCompleteResult =
            serde_json::from_str(&complete_content).expect("team_task_complete should return json");
        assert_eq!(complete_result.completed, true);
        assert_eq!(complete_result.task.state, PersistedTaskState::Completed);

        let worker_task_id = list_result
            .tasks
            .iter()
            .find(|task| task.assignee_name == "worker")
            .expect("expected worker task")
            .task_id
            .clone();
        let claim_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_claim",
            function_payload(json!({
                "team_id": complete_result.team_id,
                "task_id": worker_task_id
            })),
        );
        let claim_output = MultiAgentHandler
            .handle(claim_invocation)
            .await
            .expect("team_task_claim should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(claim_content),
            ..
        } = claim_output
        else {
            panic!("expected function output");
        };
        let claim_result: TeamTaskClaimResult =
            serde_json::from_str(&claim_content).expect("team_task_claim should return json");
        assert_eq!(claim_result.claimed, true);
        assert_eq!(claim_result.task.state, PersistedTaskState::Claimed);
        assert_eq!(claim_result.task.assignee_name, "worker".to_string());

        let cleanup_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_cleanup",
            function_payload(json!({
                "team_id": claim_result.team_id
            })),
        );
        let cleanup_output = MultiAgentHandler
            .handle(cleanup_invocation)
            .await
            .expect("team_cleanup should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(cleanup_content),
            ..
        } = cleanup_output
        else {
            panic!("expected function output");
        };
        let cleanup_result: TeamCleanupResult =
            serde_json::from_str(&cleanup_content).expect("team_cleanup should return json");
        assert_eq!(cleanup_result.removed_from_registry, true);
        assert_eq!(cleanup_result.removed_team_config, true);
        assert_eq!(cleanup_result.removed_task_dir, true);
        assert_eq!(cleanup_result.closed.len(), 2);
        for member in &cleanup_result.closed {
            assert_eq!(member.name.is_empty(), false);
            assert_eq!(member.agent_id.is_empty(), false);
            assert_eq!(member.ok || member.error.is_some(), true);
            assert!(matches!(
                member.status,
                AgentStatus::PendingInit
                    | AgentStatus::Running
                    | AgentStatus::NotFound
                    | AgentStatus::Shutdown
            ));
        }
        assert_eq!(
            tokio::fs::metadata(team_dir(
                turn.config.codex_home.as_path(),
                &cleanup_result.team_id,
            ))
            .await
            .is_err(),
            true
        );
        assert_eq!(
            tokio::fs::metadata(team_tasks_dir(
                turn.config.codex_home.as_path(),
                &cleanup_result.team_id,
            ))
            .await
            .is_err(),
            true
        );
    }

    #[tokio::test]
    async fn team_task_claim_and_complete_error_paths() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        let team_id = spawn_result.team_id.clone();

        let tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id)
            .await
            .expect("tasks should exist after spawn_team");
        let planner_task = tasks
            .iter()
            .find(|task| task.assignee.name == "planner")
            .expect("planner task should exist")
            .clone();
        let mut worker_task = tasks
            .iter()
            .find(|task| task.assignee.name == "worker")
            .expect("worker task should exist")
            .clone();
        let planner_task_id = planner_task.id.clone();
        let worker_task_id = worker_task.id.clone();
        worker_task.depends_on = vec![planner_task.id.clone()];
        write_team_task(turn.config.codex_home.as_path(), &team_id, &worker_task)
            .await
            .expect("write worker task dependencies");

        let claim_worker_before_dependency_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_claim",
            function_payload(json!({
                "team_id": team_id.clone(),
                "task_id": worker_task_id.clone()
            })),
        );
        let Err(err) = MultiAgentHandler
            .handle(claim_worker_before_dependency_invocation)
            .await
        else {
            panic!("team_task_claim should fail with unresolved dependency");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(format!(
                "task `{}` has unresolved dependencies",
                worker_task_id
            ))
        );

        let complete_planner_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_complete",
            function_payload(json!({
                "team_id": team_id.clone(),
                "task_id": planner_task_id.clone()
            })),
        );
        let complete_planner_output = MultiAgentHandler
            .handle(complete_planner_invocation)
            .await
            .expect("team_task_complete for planner should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(complete_planner_content),
            ..
        } = complete_planner_output
        else {
            panic!("expected function output");
        };
        let complete_planner_result: TeamTaskCompleteResult =
            serde_json::from_str(&complete_planner_content)
                .expect("team_task_complete result should be json");
        assert_eq!(complete_planner_result.completed, true);
        assert_eq!(
            complete_planner_result.task.state,
            PersistedTaskState::Completed
        );

        let claim_worker_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_claim",
            function_payload(json!({
                "team_id": team_id.clone(),
                "task_id": worker_task_id.clone()
            })),
        );
        let claim_worker_output = MultiAgentHandler
            .handle(claim_worker_invocation)
            .await
            .expect("team_task_claim should succeed after dependency completion");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(claim_worker_content),
            ..
        } = claim_worker_output
        else {
            panic!("expected function output");
        };
        let claim_worker_result: TeamTaskClaimResult = serde_json::from_str(&claim_worker_content)
            .expect("team_task_claim result should json");
        assert_eq!(claim_worker_result.claimed, true);
        assert_eq!(claim_worker_result.task.state, PersistedTaskState::Claimed);

        let complete_worker_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_complete",
            function_payload(json!({
                "team_id": team_id.clone(),
                "task_id": worker_task_id.clone()
            })),
        );
        let complete_worker_output = MultiAgentHandler
            .handle(complete_worker_invocation)
            .await
            .expect("team_task_complete for worker should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(complete_worker_content),
            ..
        } = complete_worker_output
        else {
            panic!("expected function output");
        };
        let complete_worker_result: TeamTaskCompleteResult =
            serde_json::from_str(&complete_worker_content)
                .expect("team_task_complete result should be json");
        assert_eq!(complete_worker_result.completed, true);
        assert_eq!(
            complete_worker_result.task.state,
            PersistedTaskState::Completed
        );

        let complete_worker_again_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_task_complete",
            function_payload(json!({
                "team_id": team_id.clone(),
                "task_id": worker_task_id.clone()
            })),
        );
        let Err(err) = MultiAgentHandler
            .handle(complete_worker_again_invocation)
            .await
        else {
            panic!("team_task_complete should fail for completed task");
        };
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(format!(
                "task `{}` is already completed",
                worker_task_id
            ))
        );

        let cleanup_invocation = invocation(
            session,
            turn,
            "team_cleanup",
            function_payload(json!({
                "team_id": claim_worker_result.team_id
            })),
        );
        MultiAgentHandler
            .handle(cleanup_invocation)
            .await
            .expect("team_cleanup should succeed");
    }

    #[tokio::test]
    async fn team_message_and_team_broadcast_send_inputs() {
        let (mut session, turn) = make_session_and_context().await;
        let manager = thread_manager();
        session.services.agent_control = manager.agent_control();
        let session = Arc::new(session);
        let turn = Arc::new(turn);

        let spawn_invocation = invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        );
        let spawn_output = MultiAgentHandler
            .handle(spawn_invocation)
            .await
            .expect("spawn_team should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(spawn_content),
            ..
        } = spawn_output
        else {
            panic!("expected function output");
        };
        let spawn_result: SpawnTeamResult =
            serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
        let member_ids = spawn_result
            .members
            .iter()
            .map(|member| agent_id(&member.agent_id).expect("valid member agent id"))
            .collect::<std::collections::HashSet<_>>();

        let message_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_message",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "member_name": "planner",
                "message": "do planning"
            })),
        );
        let message_output = MultiAgentHandler
            .handle(message_invocation)
            .await
            .expect("team_message should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(message_content),
            ..
        } = message_output
        else {
            panic!("expected function output");
        };
        let message_result: TeamMessageResult =
            serde_json::from_str(&message_content).expect("team_message should return json");
        assert_eq!(message_result.member_name, "planner".to_string());
        assert!(!message_result.submission_id.is_empty());
        assert!(!message_result.agent_id.is_empty());

        let broadcast_invocation = invocation(
            session.clone(),
            turn.clone(),
            "team_broadcast",
            function_payload(json!({
                "team_id": message_result.team_id,
                "message": "status update"
            })),
        );
        let broadcast_output = MultiAgentHandler
            .handle(broadcast_invocation)
            .await
            .expect("team_broadcast should succeed");
        let ToolOutput::Function {
            body: FunctionCallOutputBody::Text(broadcast_content),
            ..
        } = broadcast_output
        else {
            panic!("expected function output");
        };
        let broadcast_result: TeamBroadcastResult =
            serde_json::from_str(&broadcast_content).expect("team_broadcast should return json");
        assert_eq!(
            broadcast_result.sent.len() + broadcast_result.failed.len(),
            spawn_result.members.len()
        );
        for sent in &broadcast_result.sent {
            assert_eq!(sent.member_name.is_empty(), false);
            assert_eq!(sent.agent_id.is_empty(), false);
            assert_eq!(sent.submission_id.is_empty(), false);
        }
        for failed in &broadcast_result.failed {
            assert_eq!(failed.member_name.is_empty(), false);
            assert_eq!(failed.agent_id.is_empty(), false);
            assert_eq!(failed.error.is_empty(), false);
        }

        let user_input_count = manager
            .captured_ops()
            .iter()
            .filter(|(id, op)| member_ids.contains(id) && matches!(op, Op::UserInput { .. }))
            .count();
        assert_eq!(user_input_count > 0, true);

        let cleanup_invocation = invocation(
            session,
            turn,
            "team_cleanup",
            function_payload(json!({
                "team_id": broadcast_result.team_id
            })),
        );
        MultiAgentHandler
            .handle(cleanup_invocation)
            .await
            .expect("team_cleanup should succeed");
    }

    #[tokio::test]
    async fn build_agent_spawn_config_uses_turn_context_values() {
        fn pick_allowed_sandbox_policy(
            constraint: &crate::config::Constrained<SandboxPolicy>,
            base: SandboxPolicy,
        ) -> SandboxPolicy {
            let candidates = [
                SandboxPolicy::new_read_only_policy(),
                SandboxPolicy::new_workspace_write_policy(),
                SandboxPolicy::DangerFullAccess,
            ];
            candidates
                .into_iter()
                .find(|candidate| *candidate != base && constraint.can_set(candidate).is_ok())
                .unwrap_or(base)
        }

        let (_session, mut turn) = make_session_and_context().await;
        let base_instructions = BaseInstructions {
            text: "base".to_string(),
        };
        turn.developer_instructions = Some("dev".to_string());
        turn.compact_prompt = Some("compact".to_string());
        turn.shell_environment_policy = ShellEnvironmentPolicy {
            use_profile: true,
            ..ShellEnvironmentPolicy::default()
        };
        let temp_dir = tempfile::tempdir().expect("temp dir");
        turn.cwd = temp_dir.path().to_path_buf();
        turn.codex_linux_sandbox_exe = Some(PathBuf::from("/bin/echo"));
        turn.sandbox_policy = pick_allowed_sandbox_policy(
            &turn.config.permissions.sandbox_policy,
            turn.config.permissions.sandbox_policy.get().clone(),
        );

        let config = build_agent_spawn_config(&base_instructions, &turn, 0).expect("spawn config");
        let mut expected = (*turn.config).clone();
        expected.base_instructions = Some(base_instructions.text);
        expected.model = Some(turn.model_info.slug.clone());
        expected.model_provider = turn.provider.clone();
        expected.model_reasoning_effort = turn.reasoning_effort;
        expected.model_reasoning_summary = turn.reasoning_summary;
        expected.developer_instructions = turn.developer_instructions.clone();
        expected.compact_prompt = turn.compact_prompt.clone();
        expected.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
        expected.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
        expected.cwd = turn.cwd.clone();
        expected
            .permissions
            .approval_policy
            .set(AskForApproval::Never)
            .expect("approval policy set");
        expected
            .permissions
            .sandbox_policy
            .set(turn.sandbox_policy)
            .expect("sandbox policy set");
        assert_eq!(config, expected);
    }

    #[tokio::test]
    async fn build_agent_spawn_config_preserves_base_user_instructions() {
        let (_session, mut turn) = make_session_and_context().await;
        let mut base_config = (*turn.config).clone();
        base_config.user_instructions = Some("base-user".to_string());
        turn.user_instructions = Some("resolved-user".to_string());
        turn.config = Arc::new(base_config.clone());
        let base_instructions = BaseInstructions {
            text: "base".to_string(),
        };

        let config = build_agent_spawn_config(&base_instructions, &turn, 0).expect("spawn config");

        assert_eq!(config.user_instructions, base_config.user_instructions);
    }

    #[tokio::test]
    async fn build_agent_resume_config_clears_base_instructions() {
        let (_session, mut turn) = make_session_and_context().await;
        let mut base_config = (*turn.config).clone();
        base_config.base_instructions = Some("caller-base".to_string());
        turn.config = Arc::new(base_config);

        let config = build_agent_resume_config(&turn, 0).expect("resume config");

        let mut expected = (*turn.config).clone();
        expected.base_instructions = None;
        expected.model = Some(turn.model_info.slug.clone());
        expected.model_provider = turn.provider.clone();
        expected.model_reasoning_effort = turn.reasoning_effort;
        expected.model_reasoning_summary = turn.reasoning_summary;
        expected.developer_instructions = turn.developer_instructions.clone();
        expected.compact_prompt = turn.compact_prompt.clone();
        expected.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
        expected.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
        expected.cwd = turn.cwd.clone();
        expected
            .permissions
            .approval_policy
            .set(AskForApproval::Never)
            .expect("approval policy set");
        expected
            .permissions
            .sandbox_policy
            .set(turn.sandbox_policy)
            .expect("sandbox policy set");
        assert_eq!(config, expected);
    }
}
