use crate::Cli;
use crate::kanban;
use crate::workspace;
use anyhow::Context;
use anyhow::bail;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::ws::Message as WsMessage;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::Event as SseEvent;
use axum::response::sse::Sse;
use axum::routing::get;
use axum::routing::post;
use axum::routing::put;
use base64::Engine;
use chrono::DateTime;
use codex_core::AuthManager;
use codex_core::CodexThread;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::config::ConfigOverrides;
use codex_core::config::load_config_as_toml_with_cli_overrides;
use codex_core::git_info::collect_git_info;
use codex_core::models_manager::collaboration_mode_presets::CollaborationModesConfig;
use codex_core::models_manager::manager::RefreshStrategy;
use codex_core::skills::SkillLoadOutcome;
use codex_github_webhook::GithubCodexJobOutput;
use codex_github_webhook::GithubCodexRunOverrides;
use codex_github_webhook::GithubRepoWorkItem as GithubRepoWorkItemRaw;
use codex_github_webhook::GithubWebhook;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::Settings;
use codex_protocol::custom_prompts::CustomPrompt;
use codex_protocol::custom_prompts::PROMPTS_CMD_PREFIX;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::request_user_input::RequestUserInputAnswer;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::StreamExt;
use futures::stream;
use include_dir::Dir;
use include_dir::include_dir;
use mime_guess::mime;
use rand::RngCore;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path as FsPath;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::warn;

static WEB_ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/web");

const GITHUB_KANBAN_FILE_NAME: &str = "github-kanban.json";
const GITHUB_WORK_ITEMS_FILE_NAME: &str = "github-work-items.json";
const GITHUB_REPOS_FILE_NAME: &str = "github-repos.json";
const GITHUB_JOBS_FILE_NAME: &str = "github-jobs.json";
const GITHUB_JOB_LOGS_DIR: &str = "github-job-logs";
const GITHUB_JOB_LOG_MAX_BYTES: u64 = 200_000;
const GITHUB_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
const WORKSPACE_WORK_ITEMS_FILE_NAME: &str = "work-items.json";
const WORKSPACE_KANBAN_FILE_NAME: &str = "kanban.json";

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    static_dir: Option<PathBuf>,
    config: Arc<Config>,
    cli_overrides: Vec<(String, toml::Value)>,
    base_overrides: ConfigOverrides,
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    sessions: Arc<RwLock<HashMap<String, Arc<ActiveSession>>>>,
    kanban: Arc<RwLock<kanban::KanbanConfig>>,
    workspaces: Arc<RwLock<workspace::WorkspaceStore>>,
    github_webhook: Option<GithubWebhook>,
    github_repos: Arc<RwLock<Vec<String>>>,
    github_work_items: Arc<RwLock<GithubWorkItemsSnapshot>>,
    github_kanban: Arc<RwLock<kanban::KanbanConfig>>,
    github_jobs: Arc<RwLock<HashMap<String, GithubJob>>>,
    github_sync_lock: Arc<Mutex<()>>,
    workspace_kanban_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    events_tx: broadcast::Sender<SyncEvent>,
}

struct ActiveSession {
    thread_id: ThreadId,
    thread: Arc<CodexThread>,
    rollout_path: Option<PathBuf>,
    state: RwLock<SessionState>,
}

struct SessionState {
    name: Option<String>,
    cwd: PathBuf,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    created_at: u64,
    updated_at: u64,
    active: bool,
    active_at: u64,
    thinking: bool,
    thinking_at: u64,
    permission_mode: String,
    model_mode: String,
    metadata_version: u64,
    agent_state_version: u64,
    agent_state: WebAgentState,
    next_seq: u64,
    messages: Vec<WebDecryptedMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubLabel {
    name: String,
    color: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubWorkItem {
    #[serde(rename = "workItemKey")]
    work_item_key: String,
    repo: String,
    kind: String,
    number: u64,
    title: String,
    state: String,
    url: String,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
    labels: Vec<GithubLabel>,
    comments: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GithubWorkItemsSnapshot {
    fetched_at: u64,
    items: Vec<GithubWorkItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubJob {
    job_id: String,
    #[serde(rename = "workItemKey")]
    work_item_key: String,
    status: String,
    created_at: u64,
    started_at: Option<u64>,
    finished_at: Option<u64>,
    last_error: Option<String>,
    result_summary: Option<String>,
    thread_id: Option<String>,
    log_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
enum SyncEvent {
    #[serde(rename = "session-added")]
    SessionAdded {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    #[serde(rename = "session-updated")]
    SessionUpdated {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    #[serde(rename = "session-removed")]
    SessionRemoved {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    #[serde(rename = "message-received")]
    MessageReceived {
        #[serde(rename = "sessionId")]
        session_id: String,
        message: WebDecryptedMessage,
    },
    #[serde(rename = "connection-changed")]
    ConnectionChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<ConnectionChangedData>,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat,
    #[serde(rename = "kanban-updated")]
    KanbanUpdated { data: JsonValue },
    #[serde(rename = "card-moved")]
    CardMoved {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "columnId")]
        column_id: String,
        position: u32,
    },
    #[serde(rename = "github-work-items-updated")]
    GithubWorkItemsUpdated,
    #[serde(rename = "github-kanban-updated")]
    GithubKanbanUpdated { data: JsonValue },
    #[serde(rename = "github-card-moved")]
    GithubCardMoved {
        #[serde(rename = "workItemKey")]
        work_item_key: String,
        #[serde(rename = "columnId")]
        column_id: String,
        position: u32,
    },
    #[serde(rename = "github-job-updated")]
    GithubJobUpdated {
        #[serde(rename = "jobId")]
        job_id: String,
        #[serde(rename = "workItemKey")]
        work_item_key: String,
        status: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionChangedData {
    status: String,
    subscription_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionsResponse {
    sessions: Vec<SessionSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    session: Session,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessagesResponse {
    messages: Vec<WebDecryptedMessage>,
    page: MessagesPage,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessagesPage {
    limit: u64,
    before_seq: Option<u64>,
    next_before_seq: Option<u64>,
    has_more: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachinesResponse {
    machines: Vec<Machine>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Machine {
    id: String,
    active: bool,
    metadata: Option<MachineMetadata>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineMetadata {
    host: String,
    platform: String,
    happy_cli_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnSuccess {
    #[serde(rename = "type")]
    kind: &'static str,
    session_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnError {
    #[serde(rename = "type")]
    kind: &'static str,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckPathsExistsResponse {
    exists: HashMap<String, bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSearchItem {
    file_name: String,
    file_path: String,
    full_path: String,
    file_type: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSearchResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<FileSearchItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryEntry {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListDirectoryResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<DirectoryEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileReadResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadFileResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteUploadResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SlashCommandsResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    commands: Option<Vec<JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillsResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<Vec<JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PushVapidPublicKeyResponse {
    public_key: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceTokenResponse {
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    token: String,
    user: AuthUser,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthUser {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummary {
    id: String,
    active: bool,
    thinking: bool,
    active_at: u64,
    updated_at: u64,
    metadata: Option<SessionSummaryMetadata>,
    todo_progress: Option<TodoProgress>,
    pending_requests_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_mode: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TodoProgress {
    completed: u64,
    total: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummaryMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<SessionSummaryText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<JsonValue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummaryText {
    text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    namespace: String,
    seq: u64,
    created_at: u64,
    updated_at: u64,
    active: bool,
    active_at: u64,
    metadata: Option<Metadata>,
    metadata_version: u64,
    agent_state: Option<WebAgentState>,
    agent_state_version: u64,
    thinking: bool,
    thinking_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_mode: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    path: String,
    host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<MetadataSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataSummary {
    text: String,
    updated_at: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebAgentState {
    #[serde(skip_serializing_if = "Option::is_none")]
    controlled_by_user: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requests: Option<HashMap<String, WebAgentRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_requests: Option<HashMap<String, WebAgentCompletedRequest>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebAgentRequest {
    tool: String,
    arguments: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebAgentCompletedRequest {
    tool: String,
    arguments: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<u64>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    answers: Option<JsonValue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebDecryptedMessage {
    id: String,
    seq: Option<u64>,
    local_id: Option<String>,
    content: JsonValue,
    created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AuthRequest {
    InitData {
        #[serde(alias = "initData")]
        init_data: String,
    },
    AccessToken {
        #[serde(alias = "accessToken")]
        access_token: String,
    },
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use super::AuthRequest;
    use super::MessagePostRequest;
    use super::ReasoningSummaryConfig;
    use super::SpawnRequest;
    use super::WEB_ASSETS;
    use super::build_router;
    use super::custom_prompts_to_slash_commands;
    use super::extract_reasoning_effort_from_history;
    use super::handle_machine_spawn;
    use super::handle_move_kanban_card;
    use super::handle_post_message;
    use super::handle_resume_session;
    use super::handle_skills;
    use super::handle_slash_commands;
    use super::plan_mode_developer_instructions;
    use super::safe_join;
    use super::skills_outcome_to_summaries;
    use axum::Json;
    use axum::body::Body;
    use axum::extract::Path;
    use axum::extract::State;
    use axum::http::Request;
    use axum::http::StatusCode;
    use codex_core::AuthManager;
    use codex_core::ThreadManager;
    use codex_core::config::Config;
    use codex_core::config::ConfigOverrides;
    use codex_core::models_manager::collaboration_mode_presets::CollaborationModesConfig;
    use codex_core::skills::SkillLoadOutcome;
    use codex_core::skills::SkillMetadata;
    use codex_protocol::config_types::CollaborationMode;
    use codex_protocol::config_types::CollaborationModeMask;
    use codex_protocol::config_types::ModeKind;
    use codex_protocol::config_types::Settings;
    use codex_protocol::custom_prompts::CustomPrompt;
    use codex_protocol::openai_models::ReasoningEffort;
    use codex_protocol::protocol::AskForApproval;
    use codex_protocol::protocol::InitialHistory;
    use codex_protocol::protocol::Op;
    use codex_protocol::protocol::RolloutItem;
    use codex_protocol::protocol::SandboxPolicy;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::SkillScope;
    use codex_protocol::protocol::TurnContextItem;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tokio::sync::broadcast;
    use tower::util::ServiceExt;

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(key);
            // Safety: guarded by ENV_LOCK so tests don't concurrently mutate the process env.
            unsafe { std::env::set_var(key, value.as_os_str()) };
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                // Safety: guarded by ENV_LOCK so tests don't concurrently mutate the process env.
                unsafe { std::env::set_var(self.key, previous) };
            } else {
                // Safety: guarded by ENV_LOCK so tests don't concurrently mutate the process env.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    async fn collect_body_bytes(body: Body) -> anyhow::Result<Vec<u8>> {
        use futures::StreamExt;

        let mut stream = body.into_data_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    #[test]
    fn auth_request_accepts_access_token_aliases() {
        let camel: AuthRequest = serde_json::from_str(r#"{"accessToken":"devtoken"}"#).unwrap();
        match camel {
            AuthRequest::AccessToken { access_token } => assert_eq!(access_token, "devtoken"),
            _ => panic!("expected AccessToken"),
        }

        let snake: AuthRequest = serde_json::from_str(r#"{"access_token":"devtoken"}"#).unwrap();
        match snake {
            AuthRequest::AccessToken { access_token } => assert_eq!(access_token, "devtoken"),
            _ => panic!("expected AccessToken"),
        }
    }

    #[test]
    fn auth_request_accepts_init_data_aliases() {
        let camel: AuthRequest = serde_json::from_str(r#"{"initData":"x"}"#).unwrap();
        match camel {
            AuthRequest::InitData { init_data } => assert_eq!(init_data, "x"),
            _ => panic!("expected InitData"),
        }

        let snake: AuthRequest = serde_json::from_str(r#"{"init_data":"x"}"#).unwrap();
        match snake {
            AuthRequest::InitData { init_data } => assert_eq!(init_data, "x"),
            _ => panic!("expected InitData"),
        }
    }

    #[test]
    fn safe_join_rejects_parent_and_absolute_paths() {
        let root = PathBuf::from("/tmp/root");

        assert!(safe_join(&root, "../etc/passwd").is_err());

        let abs = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(safe_join(&root, &abs).is_err());
    }

    #[test]
    fn safe_join_allows_normal_paths() {
        let root = PathBuf::from("/tmp/root");

        let joined = safe_join(&root, "foo/./bar").unwrap();
        assert!(joined.ends_with(PathBuf::from("foo").join("bar")));

        let joined = safe_join(&root, "foo..bar").unwrap();
        assert!(joined.ends_with("foo..bar"));
    }

    #[test]
    fn spawn_request_accepts_reasoning_effort() {
        let req: SpawnRequest =
            serde_json::from_str(r#"{"directory":"x","reasoningEffort":"high"}"#).unwrap();
        assert_eq!(req.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn update_github_kanban_card_settings_request_accepts_reasoning_effort() {
        let req: super::UpdateGithubKanbanCardSettingsRequest =
            serde_json::from_str(r#"{"workItemKey":"owner/repo#1","reasoningEffort":"high"}"#)
                .unwrap();
        assert_eq!(req.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn embedded_web_assets_include_session_ux_features() {
        fn js_assets_contain_marker(dir: &include_dir::Dir<'_>, marker: &str) -> bool {
            for entry in dir.entries() {
                match entry {
                    include_dir::DirEntry::Dir(subdir) => {
                        if js_assets_contain_marker(subdir, marker) {
                            return true;
                        }
                    }
                    include_dir::DirEntry::File(file) => {
                        if file.path().extension().and_then(|ext| ext.to_str()) == Some("js")
                            && std::str::from_utf8(file.contents())
                                .is_ok_and(|contents| contents.contains(marker))
                        {
                            return true;
                        }
                    }
                }
            }

            false
        }

        let index = WEB_ASSETS
            .get_file("index.html")
            .expect("embedded serve assets include index.html");
        let index_html = std::str::from_utf8(index.contents()).expect("index.html is utf-8");

        let marker = "src=\"/assets/index-";
        let start = index_html
            .find(marker)
            .expect("index.html includes main JS bundle script tag");
        let path_start = start + "src=\"/".len();
        let path_end = index_html[path_start..]
            .find('"')
            .expect("index.html script src attribute is quoted");
        let bundle_path = &index_html[path_start..path_start + path_end];

        WEB_ASSETS
            .get_file(bundle_path)
            .unwrap_or_else(|| panic!("embedded serve assets include {bundle_path}"));

        assert!(
            js_assets_contain_marker(&WEB_ASSETS, "reasoningEffort"),
            "embedded Web UI bundle missing reasoningEffort (run `just write-serve-web-assets`)"
        );
        assert!(
            js_assets_contain_marker(&WEB_ASSETS, "spawn_team"),
            "embedded Web UI bundle missing agent teams tool support (run `just write-serve-web-assets`)"
        );
    }

    #[test]
    fn plan_mode_developer_instructions_extracts_plan_preset() {
        let masks = vec![
            CollaborationModeMask {
                name: "Default".to_string(),
                mode: Some(ModeKind::Default),
                model: None,
                reasoning_effort: None,
                developer_instructions: Some(Some("default".to_string())),
            },
            CollaborationModeMask {
                name: "Plan".to_string(),
                mode: Some(ModeKind::Plan),
                model: None,
                reasoning_effort: None,
                developer_instructions: Some(Some("plan instructions".to_string())),
            },
        ];

        assert_eq!(
            plan_mode_developer_instructions(&masks),
            Some("plan instructions".to_string())
        );
    }

    #[test]
    fn custom_prompts_to_slash_commands_formats_prompt_names() {
        let prompt = CustomPrompt {
            name: "my-prompt".to_string(),
            path: PathBuf::from("/tmp/my-prompt.md"),
            content: "Hello".to_string(),
            description: Some("desc".to_string()),
            argument_hint: None,
        };
        let cmds = custom_prompts_to_slash_commands(vec![prompt]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0]["name"], "prompts:my-prompt");
        assert_eq!(cmds[0]["source"], "user");
        assert_eq!(cmds[0]["content"], "Hello");
    }

    #[test]
    fn skills_outcome_to_summaries_filters_disabled_paths() {
        let enabled_skill = SkillMetadata {
            name: "a".to_string(),
            description: "A".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            path_to_skills_md: PathBuf::from("/tmp/a/SKILL.md"),
            scope: SkillScope::User,
        };
        let disabled_skill = SkillMetadata {
            name: "b".to_string(),
            description: "B".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            path_to_skills_md: PathBuf::from("/tmp/b/SKILL.md"),
            scope: SkillScope::User,
        };
        let mut disabled_paths = HashSet::new();
        disabled_paths.insert(disabled_skill.path_to_skills_md.clone());
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = vec![enabled_skill.clone(), disabled_skill];
        outcome.disabled_paths = disabled_paths;

        let skills = skills_outcome_to_summaries(outcome);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], enabled_skill.name);
    }

    #[test]
    fn extract_reasoning_effort_prefers_collaboration_mode_over_effort_field() {
        let ctx = TurnContextItem {
            turn_id: None,
            cwd: PathBuf::from("/tmp"),
            current_date: None,
            timezone: None,
            approval_policy: AskForApproval::OnRequest,
            sandbox_policy: SandboxPolicy::new_workspace_write_policy(),
            network: None,
            model: "gpt-5.2".to_string(),
            personality: None,
            trace_id: None,
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Default,
                settings: Settings {
                    model: "gpt-5.2".to_string(),
                    reasoning_effort: Some(ReasoningEffort::High),
                    developer_instructions: None,
                },
            }),
            realtime_active: None,
            effort: Some(ReasoningEffort::Low),
            summary: ReasoningSummaryConfig::Auto,
            user_instructions: None,
            developer_instructions: None,
            final_output_json_schema: None,
            truncation_policy: None,
        };
        let history = InitialHistory::Forked(vec![RolloutItem::TurnContext(ctx)]);
        assert_eq!(
            extract_reasoning_effort_from_history(&history),
            Some(ReasoningEffort::High)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn web_handlers_smoke_spawn_plan_resume_and_aux_endpoints() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let prompts_dir = state.config.codex_home.join("prompts");
        tokio::fs::create_dir_all(&prompts_dir)
            .await
            .expect("create prompts dir");
        tokio::fs::write(prompts_dir.join("hello.md"), "Hello")
            .await
            .expect("write prompt");

        let resp = handle_slash_commands(State(state.clone()), Path("any".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let session_dir = temp_dir("session-cwd");
        let spawn_req = SpawnRequest {
            directory: session_dir.display().to_string(),
            agent: Some("codex".to_string()),
            model: None,
            reasoning_effort: Some(ReasoningEffort::High),
            yolo: Some(false),
        };
        let resp = handle_machine_spawn(
            State(state.clone()),
            Path("local".to_string()),
            Json(spawn_req),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let (session_id, session) = {
            let sessions = state.sessions.read().await;
            let (id, session) = sessions.iter().next().expect("spawned session");
            (id.clone(), Arc::clone(session))
        };
        assert_eq!(
            session.state.read().await.reasoning_effort,
            Some(ReasoningEffort::High)
        );

        session.state.write().await.permission_mode = "plan".to_string();
        let msg = MessagePostRequest {
            text: "hi".to_string(),
            local_id: None,
            attachments: None,
        };
        let resp =
            handle_post_message(State(state.clone()), Path(session_id.clone()), Json(msg)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(session.state.read().await.messages.len(), 1);

        let resp = handle_skills(State(state.clone()), Path(session_id.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = handle_skills(State(state.clone()), Path("missing-session".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let rollout_path = session.rollout_path.clone().expect("rollout path");
        let mut effort = None;
        for _ in 0..100 {
            if tokio::fs::try_exists(&rollout_path).await.unwrap_or(false)
                && let Ok(history) =
                    codex_core::RolloutRecorder::get_rollout_history(&rollout_path).await
            {
                effort = extract_reasoning_effort_from_history(&history);
                if effort == Some(ReasoningEffort::High) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(effort, Some(ReasoningEffort::High));

        let _ = session.thread.submit(Op::Shutdown).await;
        state.sessions.write().await.remove(&session_id);

        let msg = MessagePostRequest {
            text: "inactive".to_string(),
            local_id: None,
            attachments: None,
        };
        let resp =
            handle_post_message(State(state.clone()), Path(session_id.clone()), Json(msg)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = handle_resume_session(State(state.clone()), Path(session_id.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(state.sessions.read().await.contains_key(&session_id));

        let msg = MessagePostRequest {
            text: "resumed".to_string(),
            local_id: None,
            attachments: None,
        };
        let resp =
            handle_post_message(State(state.clone()), Path(session_id.clone()), Json(msg)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let _ = state.thread_manager.remove_and_close_all_threads().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn web_handlers_kanban_smoke_move_and_persist() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let session_dir = temp_dir("session-cwd");
        let spawn_req = SpawnRequest {
            directory: session_dir.display().to_string(),
            agent: Some("codex".to_string()),
            model: None,
            reasoning_effort: Some(ReasoningEffort::High),
            yolo: Some(false),
        };
        let resp = handle_machine_spawn(
            State(state.clone()),
            Path("local".to_string()),
            Json(spawn_req),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let session_id = state
            .sessions
            .read()
            .await
            .keys()
            .next()
            .cloned()
            .expect("spawned session id");

        let pos = state
            .kanban
            .read()
            .await
            .card_positions
            .get(&session_id)
            .cloned()
            .expect("session added to kanban");
        assert_eq!(pos.column_id, "backlog");

        let path = codex_home.join("kanban.json");
        assert!(tokio::fs::metadata(&path).await.is_ok());

        let resp = handle_move_kanban_card(
            State(state.clone()),
            Path(session_id.clone()),
            Json(super::MoveKanbanCardRequest {
                column_id: "done".to_string(),
                position: 0,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let pos = state
            .kanban
            .read()
            .await
            .card_positions
            .get(&session_id)
            .cloned()
            .expect("session still present in kanban");
        assert_eq!(pos.column_id, "done");

        let _ = state.thread_manager.remove_and_close_all_threads().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn web_handlers_workspaces_crud_persists_across_reload() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let app = build_router(state.clone());
        let create_req = Request::builder()
            .method("POST")
            .uri("/api/workspaces")
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "WS1",
                    "repos": [{ "fullName": "owner/repo" }]
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.oneshot(create_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let workspace_id = {
            let store = state.workspaces.read().await;
            let list = store.list();
            assert_eq!(list.len(), 1);
            list[0].id.clone()
        };

        let reloaded = crate::workspace::WorkspaceStore::load_or_default(&codex_home).await;
        assert!(reloaded.get(&workspace_id).is_some());

        let update_req = Request::builder()
            .method("PUT")
            .uri(format!("/api/workspaces/{workspace_id}"))
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "name": "WS2" }).to_string()))
            .unwrap();
        let res = build_router(state.clone())
            .oneshot(update_req)
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let ws2_name = {
            let store = state.workspaces.read().await;
            store.get(&workspace_id).unwrap().name
        };
        assert_eq!(ws2_name, "WS2");

        let delete_req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/workspaces/{workspace_id}"))
            .header("authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap();
        let res = build_router(state.clone())
            .oneshot(delete_req)
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let reloaded = crate::workspace::WorkspaceStore::load_or_default(&codex_home).await;
        assert!(reloaded.get(&workspace_id).is_none());

        let _ = state.thread_manager.remove_and_close_all_threads().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn web_handlers_workspaces_kanban_move_persists_to_workspace_dir() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let app = build_router(state.clone());
        let create_req = Request::builder()
            .method("POST")
            .uri("/api/workspaces")
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "WS1",
                    "repos": [{ "fullName": "owner/repo" }]
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.oneshot(create_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let workspace_id = {
            let store = state.workspaces.read().await;
            store.list()[0].id.clone()
        };

        let move_req = Request::builder()
            .method("PUT")
            .uri(format!("/api/workspaces/{workspace_id}/kanban/cards"))
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "workItemKey": "owner/repo#1:issue",
                    "columnId": "running",
                    "position": 0
                })
                .to_string(),
            ))
            .unwrap();
        let res = build_router(state.clone()).oneshot(move_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let kanban_path = codex_home
            .join("workspaces")
            .join(&workspace_id)
            .join("kanban.json");
        let raw = tokio::fs::read(&kanban_path).await.expect("read kanban");
        let parsed: crate::kanban::KanbanConfig =
            serde_json::from_slice(&raw).expect("parse kanban");
        let pos = parsed
            .card_positions
            .get("owner/repo#1:issue")
            .expect("card position");
        assert_eq!(pos.column_id, "running");

        let _ = state.thread_manager.remove_and_close_all_threads().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn closed_loop_workspace_kanban_v1_writes_evidence_package() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let run_id = uuid::Uuid::new_v4().to_string();
        let evidence_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../run")
            .join(format!("kanban-workspace-closed-loop-{stamp}-{run_id}"));
        tokio::fs::create_dir_all(&evidence_root)
            .await
            .expect("create evidence root");

        let req_res_dir = evidence_root.join("request-response");
        let db_dir = evidence_root.join("db");
        let logs_dir = evidence_root.join("logs");
        let metrics_dir = evidence_root.join("metrics");
        let trace_dir = evidence_root.join("trace");
        let ui_dir = evidence_root.join("ui");
        tokio::fs::create_dir_all(&req_res_dir)
            .await
            .expect("create request-response dir");
        tokio::fs::create_dir_all(&db_dir)
            .await
            .expect("create db dir");
        tokio::fs::create_dir_all(&logs_dir)
            .await
            .expect("create logs dir");
        tokio::fs::create_dir_all(&metrics_dir)
            .await
            .expect("create metrics dir");
        tokio::fs::create_dir_all(&trace_dir)
            .await
            .expect("create trace dir");
        tokio::fs::create_dir_all(&ui_dir)
            .await
            .expect("create ui dir");
        tokio::fs::create_dir_all(logs_dir.join("queries"))
            .await
            .expect("create logs queries dir");
        tokio::fs::create_dir_all(metrics_dir.join("queries"))
            .await
            .expect("create metrics queries dir");
        tokio::fs::create_dir_all(trace_dir.join("queries"))
            .await
            .expect("create trace queries dir");

        let observability_note = "\
# Observability (artifacts-only)\n\
\n\
本次闭环运行没有接入可本地查询的 logs/metrics/trace 栈（例如 Loki/Prometheus/Tempo）。\n\
因此只做：HTTP request/response 证据 + 文件落盘状态校验。\n\
\n\
如需升级到 query-backed（V2/V3）：\n\
- 为每次 run 注入可过滤的 run_id/workspace_id，并落到日志字段/trace span attribute\n\
- 记录 LogQL/PromQL/TraceQL 及其结果快照到对应 queries/ 下\n\
";
        tokio::fs::write(
            logs_dir.join("queries").join("README.md"),
            observability_note,
        )
        .await
        .expect("write logs queries note");
        tokio::fs::write(
            metrics_dir.join("queries").join("README.md"),
            observability_note,
        )
        .await
        .expect("write metrics queries note");
        tokio::fs::write(
            trace_dir.join("queries").join("README.md"),
            observability_note,
        )
        .await
        .expect("write trace queries note");
        tokio::fs::write(
            ui_dir.join("README.md"),
            "# UI\n\nV1 未做浏览器截图/录像；仅验证静态嵌入资源与 API 行为。\n",
        )
        .await
        .expect("write ui note");

        let mut checks: Vec<serde_json::Value> = Vec::new();
        let mut primary_workspace_id: Option<String> = None;
        let mut status = "fail";
        let mut failure: Option<String> = None;

        let result: anyhow::Result<()> = async {
            let app = build_router(state.clone());

            async fn write_pair(
                req_res_dir: &std::path::Path,
                seq: u32,
                name: &str,
                request: &str,
                status_code: StatusCode,
                response: &str,
            ) -> anyhow::Result<()> {
                let req_path = req_res_dir.join(format!("{seq:02}-{name}.request.json"));
                let res_path = req_res_dir.join(format!("{seq:02}-{name}.response.txt"));
                let status_path = req_res_dir.join(format!("{seq:02}-{name}.status.txt"));
                tokio::fs::write(&req_path, request).await?;
                tokio::fs::write(&res_path, response).await?;
                tokio::fs::write(&status_path, status_code.as_str()).await?;
                Ok(())
            }

            async fn snapshot_file(
                db_dir: &std::path::Path,
                src_path: &std::path::Path,
                name: &str,
            ) -> anyhow::Result<()> {
                match tokio::fs::read(src_path).await {
                    Ok(content) => {
                        tokio::fs::write(db_dir.join(name), content).await?;
                        Ok(())
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(err) => Err(err.into()),
                }
            }

            let create_payload = serde_json::json!({
                "name": "WS1",
                "repos": [{ "fullName": "owner/repo" }]
            })
            .to_string();
            let create_req = Request::builder()
                .method("POST")
                .uri("/api/workspaces")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from(create_payload.clone()))?;
            let res = app.clone().oneshot(create_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                1,
                "create-workspace",
                &create_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "create workspace: {status_code}"
            );
            checks.push(serde_json::json!({
                "name": "workspace_create",
                "blocking": true,
                "result": "pass"
            }));

            let created: serde_json::Value = serde_json::from_slice(&body)?;
            let workspace_id = created
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing workspace id"))?
                .to_string();
            primary_workspace_id = Some(workspace_id.clone());

            let workspaces_root = codex_home.join("workspaces");
            let ws_dir = workspaces_root.join(&workspace_id);
            snapshot_file(
                &db_dir,
                &workspaces_root.join("index.json"),
                "workspaces.index.json",
            )
            .await?;
            snapshot_file(&db_dir, &ws_dir.join("workspace.json"), "workspace.json").await?;

            let list_payload = "{}";
            let list_req = Request::builder()
                .method("GET")
                .uri("/api/workspaces")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(list_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                2,
                "list-workspaces",
                list_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "list workspaces: {status_code}"
            );
            let listed: serde_json::Value = serde_json::from_slice(&body)?;
            anyhow::ensure!(
                listed
                    .as_array()
                    .is_some_and(|arr| arr.iter().any(|v| v.get("id") == Some(&created["id"]))),
                "workspace id missing from list"
            );
            checks.push(serde_json::json!({
                "name": "workspace_list_contains_created",
                "blocking": true,
                "result": "pass"
            }));

            let get_payload = "{}";
            let get_req = Request::builder()
                .method("GET")
                .uri(format!("/api/workspaces/{workspace_id}"))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(get_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                3,
                "get-workspace",
                get_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "get workspace: {status_code}"
            );
            checks.push(serde_json::json!({
                "name": "workspace_get",
                "blocking": true,
                "result": "pass"
            }));

            let work_items_payload = "{}";
            let work_items_req = Request::builder()
                .method("GET")
                .uri(format!("/api/workspaces/{workspace_id}/work-items"))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(work_items_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                4,
                "workspace-work-items",
                work_items_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "workspace work-items: {status_code}"
            );
            checks.push(serde_json::json!({
                "name": "workspace_work_items_get",
                "blocking": true,
                "result": "pass"
            }));
            snapshot_file(&db_dir, &ws_dir.join("work-items.json"), "work-items.json").await?;

            let kanban_payload = "{}";
            let kanban_req = Request::builder()
                .method("GET")
                .uri(format!("/api/workspaces/{workspace_id}/kanban"))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(kanban_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                5,
                "workspace-kanban",
                kanban_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "workspace kanban: {status_code}"
            );
            let kanban_path = codex_home
                .join("workspaces")
                .join(&workspace_id)
                .join("kanban.json");
            anyhow::ensure!(
                tokio::fs::metadata(&kanban_path).await.is_ok(),
                "workspace kanban.json not persisted"
            );
            snapshot_file(&db_dir, &kanban_path, "kanban.json").await?;
            checks.push(serde_json::json!({
                "name": "workspace_kanban_get_and_persist",
                "blocking": true,
                "result": "pass"
            }));

            let move_payload = serde_json::json!({
                "workItemKey": "owner/repo#1:issue",
                "columnId": "running",
                "position": 0
            })
            .to_string();
            let move_req = Request::builder()
                .method("PUT")
                .uri(format!("/api/workspaces/{workspace_id}/kanban/cards"))
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from(move_payload.clone()))?;
            let res = build_router(state.clone()).oneshot(move_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                6,
                "move-card",
                &move_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(status_code == StatusCode::OK, "move card: {status_code}");

            let raw = tokio::fs::read(&kanban_path).await?;
            let parsed: crate::kanban::KanbanConfig = serde_json::from_slice(&raw)?;
            let pos = parsed
                .card_positions
                .get("owner/repo#1:issue")
                .ok_or_else(|| anyhow::anyhow!("card position missing"))?;
            anyhow::ensure!(pos.column_id == "running", "card not in running");
            snapshot_file(&db_dir, &kanban_path, "kanban.after-move.json").await?;
            checks.push(serde_json::json!({
                "name": "workspace_kanban_move_persist",
                "blocking": true,
                "result": "pass"
            }));

            let jobs_payload = "{}";
            let jobs_req = Request::builder()
                .method("GET")
                .uri(format!("/api/workspaces/{workspace_id}/jobs"))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(jobs_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                7,
                "workspace-jobs",
                jobs_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::OK,
                "workspace jobs: {status_code}"
            );
            let jobs: serde_json::Value = serde_json::from_slice(&body)?;
            anyhow::ensure!(
                jobs.get("jobs")
                    .and_then(|v| v.as_array())
                    .is_some_and(Vec::is_empty),
                "expected empty jobs list without github_webhook"
            );
            checks.push(serde_json::json!({
                "name": "workspace_jobs_empty_without_github",
                "blocking": true,
                "result": "pass"
            }));

            let delete_payload = "{}";
            let delete_req = Request::builder()
                .method("DELETE")
                .uri(format!("/api/workspaces/{workspace_id}"))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())?;
            let res = build_router(state.clone()).oneshot(delete_req).await?;
            let status_code = res.status();
            let body = collect_body_bytes(res.into_body()).await?;
            let body_text = String::from_utf8_lossy(&body).to_string();
            write_pair(
                &req_res_dir,
                8,
                "delete-workspace",
                delete_payload,
                status_code,
                &body_text,
            )
            .await?;
            anyhow::ensure!(
                status_code == StatusCode::NO_CONTENT,
                "delete workspace: {status_code}"
            );
            checks.push(serde_json::json!({
                "name": "workspace_delete",
                "blocking": true,
                "result": "pass"
            }));

            Ok(())
        }
        .await;

        if let Err(err) = result {
            failure = Some(format!("{err:#}"));
        } else {
            status = "pass";
        }

        let verdict = serde_json::json!({
            "status": status,
            "generatedAt": chrono::Utc::now().to_rfc3339(),
            "date": chrono::Utc::now().format("%Y-%m-%d").to_string(),
            "slice": "kanban-workspace-v1",
            "version": 1,
            "mode": "artifacts-only",
            "runId": run_id,
            "primaryIds": {
                "workspaceId": primary_workspace_id
            },
            "checks": checks,
            "error": failure,
            "risks": [
                "未做真实 GitHub API / webhook e2e；Workspace `/sync` 与 `Done->close` 依赖 github_webhook enabled。",
                "未实现 PRD Phase 2+（WebSocket 日志流、细粒度 job 状态机、Epic/泳道等）。"
            ]
        });
        let verdict_path = evidence_root.join("verdict.json");
        let mut verdict_bytes = serde_json::to_vec_pretty(&verdict).expect("serialize verdict");
        verdict_bytes.push(b'\n');
        tokio::fs::write(&verdict_path, verdict_bytes)
            .await
            .expect("write verdict");

        let report = format!(
            "\
# Closed Loop Report: kanban-workspace-v1\n\
\n\
- status: {status}\n\
- mode: artifacts-only\n\
- runId: {run_id}\n\
- workspaceId: {}\n\
- evidence: {}\n\
\n\
## Entrypoints\n\
\n\
- POST /api/workspaces\n\
- GET /api/workspaces\n\
- GET /api/workspaces/{{id}}\n\
- GET /api/workspaces/{{id}}/work-items\n\
- GET /api/workspaces/{{id}}/kanban\n\
- PUT /api/workspaces/{{id}}/kanban/cards\n\
- GET /api/workspaces/{{id}}/jobs\n\
- DELETE /api/workspaces/{{id}}\n\
\n\
## Blocking Checks\n\
\n\
见 `verdict.json` 的 `checks[]`（均为 blocking）。\n\
\n\
## Evidence\n\
\n\
- request/response：`request-response/`\n\
- 文件落盘快照：`db/`（workspace/work-items/kanban 等）\n\
- logs/metrics/trace：未接入本地可查询栈，仅写入说明文件（见各自 `queries/README.md`）\n\
",
            primary_workspace_id
                .clone()
                .unwrap_or_else(|| "<missing>".to_string()),
            evidence_root.display(),
        );
        tokio::fs::write(evidence_root.join("REPORT.md"), report)
            .await
            .expect("write report");

        let _ = state.thread_manager.remove_and_close_all_threads().await;
        if let Some(err) = failure {
            panic!("closed loop failed: {err}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn github_webhook_route_is_not_token_protected() {
        let _lock = ENV_LOCK.lock().await;
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", codex_home.as_path());

        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");

        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(crate::workspace::WorkspaceStore::default())),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(super::GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        };

        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/github/webhook")
            .body(Body::from("{}"))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnRequest {
    directory: String,
    agent: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    yolo: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckPathsExistsRequest {
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagePostRequest {
    text: String,
    #[serde(default)]
    local_id: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<JsonValue>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionModeRequest {
    mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelModeRequest {
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameSessionRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadFileRequest {
    filename: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteUploadRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovePermissionRequest {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    allow_tools: Option<Vec<String>>,
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    answers: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DenyPermissionRequest {
    #[serde(default)]
    decision: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesQuery {
    query: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagesQuery {
    #[serde(rename = "beforeSeq")]
    before_seq: Option<u64>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileReadQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffFileQuery {
    path: String,
    staged: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitDiffNumstatQuery {
    staged: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery {
    token: Option<String>,
    session_id: Option<String>,
}

pub async fn run(cli: Cli, codex_linux_sandbox_exe: Option<PathBuf>) -> anyhow::Result<()> {
    let cli_overrides = cli
        .config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let cwd = std::env::current_dir()?;
    let base_overrides = ConfigOverrides {
        cwd: Some(cwd.clone()),
        codex_linux_sandbox_exe,
        ..Default::default()
    };
    let config = Config::load_with_cli_overrides_and_harness_overrides(
        cli_overrides.clone(),
        base_overrides.clone(),
    )
    .await?;

    let auth_manager = AuthManager::shared(
        config.codex_home.clone(),
        false,
        config.cli_auth_credentials_store_mode,
    );
    let thread_manager = Arc::new(ThreadManager::new(
        config.codex_home.clone(),
        auth_manager.clone(),
        SessionSource::Cli,
        config.model_catalog.clone(),
        CollaborationModesConfig {
            default_mode_request_user_input: config
                .features
                .enabled(codex_core::features::Feature::DefaultModeRequestUserInput),
        },
    ));

    let static_dir = if cli.dev {
        let candidates = ["web/dist", "../web/dist", "../../web/dist"];
        let mut found = None;
        for relative in candidates {
            let dir = cwd.join(relative);
            if dir.join("index.html").is_file() {
                found = Some(dir);
                break;
            }
        }
        let Some(found) = found else {
            bail!("--dev enabled but web/dist/index.html not found (run `npm run build` in ./web)");
        };
        Some(found)
    } else {
        None
    };

    let token = cli.token.unwrap_or_else(generate_token);
    let (events_tx, _) = broadcast::channel::<SyncEvent>(2048);
    let config_cwd = AbsolutePathBuf::current_dir().context("resolve config cwd")?;
    let config_toml = load_config_as_toml_with_cli_overrides(
        &config.codex_home,
        &config_cwd,
        cli_overrides.clone(),
    )
    .await
    .context("load config.toml for embedded GitHub webhook")?;
    let github_webhook = GithubWebhook::try_from_config(
        &config.codex_home,
        config_toml.github_webhook.as_ref(),
        std::env::current_exe().context("resolve current executable")?,
        cli.config_overrides.raw_overrides.clone(),
    )
    .context("init embedded GitHub webhook")?;
    if let Some(webhook) = github_webhook.as_ref() {
        webhook.spawn_gc_loop_if_needed();
    }

    let config = Arc::new(config);
    let kanban = kanban::load_or_default(&config.codex_home).await;
    let workspaces = workspace::WorkspaceStore::load_or_default(&config.codex_home).await;
    let github_repos = if github_webhook.is_some() {
        let repos = load_github_repos(&config.codex_home).await;
        if repos.is_empty() {
            resolve_github_repos_for_kanban(&config_toml, &config_cwd).await
        } else {
            repos
        }
    } else {
        Vec::new()
    };
    let github_work_items = if github_webhook.is_some() {
        load_github_work_items_snapshot(&config.codex_home).await
    } else {
        GithubWorkItemsSnapshot::default()
    };
    let github_kanban = if github_webhook.is_some() {
        kanban::load_or_default_from(&config.codex_home, GITHUB_KANBAN_FILE_NAME).await
    } else {
        kanban::KanbanConfig::default()
    };
    let github_jobs = if github_webhook.is_some() {
        load_github_jobs(&config.codex_home).await
    } else {
        HashMap::new()
    };
    let state = AppState {
        token: Arc::new(token.clone()),
        static_dir,
        config: Arc::clone(&config),
        cli_overrides,
        base_overrides,
        auth_manager,
        thread_manager,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        kanban: Arc::new(RwLock::new(kanban)),
        workspaces: Arc::new(RwLock::new(workspaces)),
        github_webhook,
        github_repos: Arc::new(RwLock::new(github_repos)),
        github_work_items: Arc::new(RwLock::new(github_work_items)),
        github_kanban: Arc::new(RwLock::new(github_kanban)),
        github_jobs: Arc::new(RwLock::new(github_jobs)),
        github_sync_lock: Arc::new(Mutex::new(())),
        workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
        events_tx,
    };

    if state.github_webhook.is_some() {
        tokio::spawn(github_sync_loop(state.clone()));
    }

    let listener = TcpListener::bind(SocketAddr::new(cli.host, cli.port))
        .await
        .context("bind serve listener")?;
    let local_addr = listener.local_addr().context("get local addr")?;

    if cli.host.is_unspecified() || cli.host.is_multicast() {
        warn!("listening on potentially unsafe host {}", cli.host);
    }
    if cli.host.to_string() == "0.0.0.0" {
        warn!("binding to 0.0.0.0 exposes Codex to your network");
    }

    let url = format!(
        "http://{}:{}?token={token}",
        local_addr.ip(),
        local_addr.port()
    );
    println!("Codex Web UI running at {url}");
    if !cli.no_open {
        let _ = webbrowser::open(&url);
    }

    let app = build_router(state.clone());

    axum::serve(listener, app.into_make_service())
        .await
        .context("http serve")?;

    Ok(())
}

fn build_router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/events", get(handle_events))
        .route("/sessions", get(handle_sessions))
        .route("/kanban", get(handle_get_kanban))
        .route("/models/catalog", get(handle_models_catalog))
        .route("/kanban/cards/{session_id}", put(handle_move_kanban_card))
        .route("/kanban/cards/batch", put(handle_batch_move_kanban_cards))
        .route(
            "/github/repos",
            get(handle_github_repos).put(handle_set_github_repos),
        )
        .route("/github/work-items", get(handle_github_work_items))
        .route(
            "/github/work-items/detail",
            get(handle_github_work_item_detail),
        )
        .route(
            "/github/work-items/close",
            post(handle_github_work_item_close),
        )
        .route("/github/sync", post(handle_github_sync))
        .route("/github/kanban", get(handle_get_github_kanban))
        .route("/github/kanban/cards", put(handle_move_github_kanban_card))
        .route(
            "/github/kanban/cards/settings",
            put(handle_update_github_kanban_card_settings),
        )
        .route("/github/jobs", get(handle_github_jobs))
        .route("/github/jobs/{job_id}/log", get(handle_github_job_log))
        .route(
            "/workspaces",
            get(handle_list_workspaces).post(handle_create_workspace),
        )
        .route(
            "/workspaces/{workspace_id}",
            get(handle_get_workspace)
                .put(handle_update_workspace)
                .delete(handle_delete_workspace),
        )
        .route(
            "/workspaces/{workspace_id}/sync",
            post(handle_workspace_sync),
        )
        .route(
            "/workspaces/{workspace_id}/work-items",
            get(handle_workspace_work_items),
        )
        .route(
            "/workspaces/{workspace_id}/kanban",
            get(handle_workspace_kanban),
        )
        .route(
            "/workspaces/{workspace_id}/kanban/cards",
            put(handle_move_workspace_kanban_card),
        )
        .route(
            "/workspaces/{workspace_id}/kanban/cards/settings",
            put(handle_update_workspace_kanban_card_settings),
        )
        .route(
            "/workspaces/{workspace_id}/jobs",
            get(handle_workspace_jobs),
        )
        .route(
            "/workspaces/{workspace_id}/jobs/{job_id}/log",
            get(handle_workspace_job_log),
        )
        .route(
            "/sessions/{id}",
            get(handle_session)
                .patch(handle_rename_session)
                .delete(handle_delete_session),
        )
        .route(
            "/sessions/{id}/messages",
            get(handle_messages).post(handle_post_message),
        )
        .route("/sessions/{id}/resume", post(handle_resume_session))
        .route("/sessions/{id}/abort", post(handle_abort_session))
        .route("/sessions/{id}/archive", post(handle_archive_session))
        .route(
            "/sessions/{id}/permission-mode",
            post(handle_set_permission_mode),
        )
        .route("/sessions/{id}/model", post(handle_set_model_mode))
        .route(
            "/sessions/{id}/permissions/{req_id}/approve",
            post(handle_approve_permission),
        )
        .route(
            "/sessions/{id}/permissions/{req_id}/deny",
            post(handle_deny_permission),
        )
        .route("/sessions/{id}/git-status", get(handle_git_status))
        .route(
            "/sessions/{id}/git-diff-numstat",
            get(handle_git_diff_numstat),
        )
        .route("/sessions/{id}/git-diff-file", get(handle_git_diff_file))
        .route("/sessions/{id}/files", get(handle_search_files))
        .route("/sessions/{id}/file", get(handle_read_file))
        .route("/sessions/{id}/directory", get(handle_list_directory))
        .route("/sessions/{id}/upload", post(handle_upload_file))
        .route("/sessions/{id}/upload/delete", post(handle_delete_upload))
        .route("/sessions/{id}/slash-commands", get(handle_slash_commands))
        .route("/sessions/{id}/skills", get(handle_skills))
        .route("/machines", get(handle_machines))
        .route(
            "/machines/{machine_id}/paths/exists",
            post(handle_machine_paths_exists),
        )
        .route("/machines/{machine_id}/spawn", post(handle_machine_spawn))
        .route("/push/vapid-public-key", get(handle_push_vapid_key))
        .route(
            "/push/subscribe",
            post(handle_push_subscribe).delete(handle_push_unsubscribe),
        )
        .route("/visibility", post(handle_visibility))
        .route("/voice/token", post(handle_voice_token))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    Router::new()
        .route("/github/webhook", post(handle_github_webhook))
        .route("/api/auth", post(handle_auth))
        .route("/api/bind", post(handle_bind))
        .nest("/api", authed)
        .route(
            "/ws/terminal/{session_id}/{terminal_id}",
            get(handle_terminal_ws),
        )
        .fallback(get(handle_static))
        .with_state(state)
}

async fn handle_github_webhook(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let Some(webhook) = state.github_webhook.clone() else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    webhook.handle_webhook(headers, body).await
}

async fn require_token(
    State(state): State<AppState>,
    req: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let token = bearer_token(req.headers())
        .or_else(|| token_from_query(req.uri().query()))
        .unwrap_or_default();
    if token != state.token.as_str() {
        return (StatusCode::UNAUTHORIZED, Json(json_error("unauthorized"))).into_response();
    }
    next.run(req).await
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let value = value.trim();
    let rest = value.strip_prefix("Bearer ")?;
    Some(rest.trim().to_string())
}

fn token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let Some(key) = parts.next() else {
            continue;
        };
        if key != "token" {
            continue;
        }
        return Some(parts.next().unwrap_or_default().to_string());
    }
    None
}

async fn handle_auth(State(state): State<AppState>, Json(body): Json<AuthRequest>) -> Response {
    match body {
        AuthRequest::AccessToken { access_token } => {
            if access_token != state.token.as_str() {
                return (StatusCode::UNAUTHORIZED, Json(json_error("unauthorized")))
                    .into_response();
            }
            Json(AuthResponse {
                token: state.token.as_str().to_string(),
                user: AuthUser {
                    id: 1,
                    username: Some("local".to_string()),
                    first_name: None,
                    last_name: None,
                },
            })
            .into_response()
        }
        AuthRequest::InitData { init_data } => {
            let _ = init_data;
            (
                StatusCode::UNAUTHORIZED,
                Json(json_error("telegram_not_supported")),
            )
                .into_response()
        }
    }
}

async fn handle_bind() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json_error("binding_not_supported")),
    )
        .into_response()
}

async fn handle_events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Response {
    let token = query.token.unwrap_or_default();
    if token != state.token.as_str() {
        return (StatusCode::UNAUTHORIZED, Json(json_error("unauthorized"))).into_response();
    }

    let subscription_id = uuid::Uuid::new_v4().to_string();
    let connect = SyncEvent::ConnectionChanged {
        data: Some(ConnectionChangedData {
            status: "connected".to_string(),
            subscription_id: Some(subscription_id),
        }),
    };

    let session_filter = query.session_id;
    let connect_event = sse_json(&connect);
    let stream = stream::once(
        async move { Ok::<SseEvent, std::convert::Infallible>(connect_event) },
    )
    .chain(stream::unfold(
        (
            state.events_tx.subscribe(),
            session_filter,
            tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(30),
                Duration::from_secs(30),
            ),
        ),
        |(mut rx, session_filter, mut heartbeat)| async move {
            loop {
                tokio::select! {
                    _ = heartbeat.tick() => {
                        let event = SyncEvent::Heartbeat;
                        return Some((Ok(sse_json(&event)), (rx, session_filter, heartbeat)));
                    }
                    msg = rx.recv() => {
	                        match msg {
	                            Ok(event) => {
	                                if let Some(wanted) = session_filter.as_deref()
	                                    && !event_matches_session(&event, wanted)
	                                {
	                                    continue;
	                                }
	                                return Some((Ok(sse_json(&event)), (rx, session_filter, heartbeat)));
	                            }
	                            Err(broadcast::error::RecvError::Closed) => return None,
	                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        }
                    }
                }
            }
        },
    ));

    Sse::new(stream).into_response()
}

fn event_matches_session(event: &SyncEvent, session_id: &str) -> bool {
    match event {
        SyncEvent::SessionAdded { session_id: id, .. } => id == session_id,
        SyncEvent::SessionUpdated { session_id: id, .. } => id == session_id,
        SyncEvent::SessionRemoved { session_id: id } => id == session_id,
        SyncEvent::MessageReceived { session_id: id, .. } => id == session_id,
        SyncEvent::ConnectionChanged { .. } => true,
        SyncEvent::Heartbeat => true,
        SyncEvent::KanbanUpdated { .. } => false,
        SyncEvent::CardMoved { .. } => false,
        SyncEvent::GithubWorkItemsUpdated => false,
        SyncEvent::GithubKanbanUpdated { .. } => false,
        SyncEvent::GithubCardMoved { .. } => false,
        SyncEvent::GithubJobUpdated { .. } => false,
    }
}

fn sse_json(event: &SyncEvent) -> SseEvent {
    let Ok(data) = serde_json::to_string(event) else {
        return SseEvent::default().data("{\"type\":\"toast\",\"data\":{\"title\":\"Serialize error\",\"body\":\"\",\"sessionId\":\"\",\"url\":\"\"}}");
    };
    SseEvent::default().data(data)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveKanbanCardRequest {
    column_id: String,
    position: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchMoveKanbanCardsRequest {
    moves: Vec<BatchMoveKanbanCard>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchMoveKanbanCard {
    session_id: String,
    column_id: String,
    position: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveGithubKanbanCardRequest {
    #[serde(rename = "workItemKey")]
    work_item_key: String,
    column_id: String,
    position: u32,
    #[serde(default, rename = "promptPrefix")]
    prompt_prefix: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateGithubKanbanCardSettingsRequest {
    #[serde(rename = "workItemKey")]
    work_item_key: String,
    #[serde(default, rename = "promptPrefix")]
    prompt_prefix: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRepoInput {
    full_name: String,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    short_label: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkspaceRequest {
    name: String,
    repos: Vec<WorkspaceRepoInput>,
    #[serde(default)]
    board: Option<workspace::BoardConfig>,
    #[serde(default)]
    default_exec: Option<workspace::ExecConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWorkspaceRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    repos: Option<Vec<WorkspaceRepoInput>>,
    #[serde(default)]
    board: Option<workspace::BoardConfig>,
    #[serde(default)]
    default_exec: Option<workspace::ExecConfig>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelsCatalogResponse {
    models: Vec<ModelCatalogModel>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogModel {
    id: String,
    display_name: String,
    description: String,
    is_default: bool,
    show_in_picker: bool,
    default_reasoning_effort: ReasoningEffort,
    supported_reasoning_efforts: Vec<ReasoningEffortPreset>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GithubReposResponse {
    repos: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetGithubReposRequest {
    repos: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubWorkItemDetailQuery {
    #[serde(rename = "workItemKey")]
    work_item_key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseGithubWorkItemRequest {
    #[serde(rename = "workItemKey")]
    work_item_key: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GithubJobLogResponse {
    #[serde(rename = "jobId")]
    job_id: String,
    #[serde(rename = "logText")]
    log_text: String,
    truncated: bool,
}

async fn handle_get_kanban(State(state): State<AppState>) -> Response {
    let mut session_ids: HashSet<String> = state.sessions.read().await.keys().cloned().collect();
    if let Ok(page) = codex_core::RolloutRecorder::list_threads(
        state.config.as_ref(),
        2000,
        None,
        codex_core::ThreadSortKey::UpdatedAt,
        codex_core::INTERACTIVE_SESSION_SOURCES,
        None,
        &state.config.model_provider_id,
        None,
    )
    .await
    {
        for item in page.items {
            if let Some(thread_id) = item.thread_id {
                session_ids.insert(thread_id.to_string());
            }
        }
    }

    let mut kanban = state.kanban.write().await;
    let changed = kanban.reconcile_sessions(&session_ids);
    let snapshot = kanban.clone();
    drop(kanban);

    if changed {
        kanban::persist(&state.config.codex_home, &snapshot).await;
    }

    Json(snapshot).into_response()
}

async fn handle_move_kanban_card(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<MoveKanbanCardRequest>,
) -> Response {
    let mut kanban = state.kanban.write().await;
    if !kanban.has_column(&body.column_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("kanban_unknown_column")),
        )
            .into_response();
    }

    let changed = kanban.move_card(&session_id, &body.column_id, body.position);
    let snapshot = kanban.clone();
    let final_position = snapshot
        .card_positions
        .get(&session_id)
        .map(|pos| pos.position)
        .unwrap_or(body.position);
    drop(kanban);

    if changed {
        kanban::persist(&state.config.codex_home, &snapshot).await;
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
        let _ = state.events_tx.send(SyncEvent::CardMoved {
            session_id,
            column_id: body.column_id,
            position: final_position,
        });
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_batch_move_kanban_cards(
    State(state): State<AppState>,
    Json(body): Json<BatchMoveKanbanCardsRequest>,
) -> Response {
    let moves: Vec<(String, kanban::CardPosition)> = body
        .moves
        .into_iter()
        .map(|m| {
            (
                m.session_id,
                kanban::CardPosition {
                    column_id: m.column_id,
                    position: m.position,
                },
            )
        })
        .collect();

    let mut kanban = state.kanban.write().await;
    let changed = kanban.apply_moves(&moves);
    let snapshot = kanban.clone();
    drop(kanban);

    if changed {
        kanban::persist(&state.config.codex_home, &snapshot).await;
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_github_repos(State(state): State<AppState>) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let repos = state.github_repos.read().await.clone();
    Json(GithubReposResponse { repos }).into_response()
}

async fn handle_set_github_repos(
    State(state): State<AppState>,
    Json(body): Json<SetGithubReposRequest>,
) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let repos = normalize_github_repos(body.repos);
    persist_github_repos(&state.config.codex_home, &repos).await;
    {
        let mut guard = state.github_repos.write().await;
        *guard = repos;
    }
    {
        let _guard = state.github_sync_lock.lock().await;
        if let Err(err) = sync_github_work_items(&state).await {
            warn!("github sync after repos update failed: {err:#}");
        }
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_github_work_items(State(state): State<AppState>) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let full = state.github_work_items.read().await.clone();
    let filtered = GithubWorkItemsSnapshot {
        fetched_at: full.fetched_at,
        items: full
            .items
            .into_iter()
            .filter(|i| i.state.eq_ignore_ascii_case("open"))
            .collect(),
    };
    Json(filtered).into_response()
}

async fn handle_github_work_item_detail(
    State(state): State<AppState>,
    Query(q): Query<GithubWorkItemDetailQuery>,
) -> Response {
    let Some(webhook) = state.github_webhook.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    };
    let work_item_key = q.work_item_key.trim().to_string();
    let Some((repo, number, _kind)) = parse_github_work_item_key(&work_item_key) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    };

    match webhook.fetch_work_item_detail(&repo, number).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => {
            warn!("failed to fetch github work item detail for {work_item_key}: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_detail_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_github_work_item_close(
    State(state): State<AppState>,
    Json(body): Json<CloseGithubWorkItemRequest>,
) -> Response {
    let Some(webhook) = state.github_webhook.as_ref() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    };
    let work_item_key = body.work_item_key.trim().to_string();
    let Some((repo, number, _kind)) = parse_github_work_item_key(&work_item_key) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    };

    if let Err(err) = webhook.set_work_item_state(&repo, number, "closed").await {
        warn!("failed to close github work item {work_item_key}: {err:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("github_close_failed")),
        )
            .into_response();
    }

    // Best-effort sync so the board reflects the new state quickly.
    {
        let _guard = state.github_sync_lock.lock().await;
        if let Err(err) = sync_github_work_items(&state).await {
            warn!("github sync after close failed: {err:#}");
        }
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_github_jobs(State(state): State<AppState>) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let jobs = state.github_jobs.read().await;
    let mut out: Vec<GithubJob> = jobs.values().cloned().collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Json(serde_json::json!({ "jobs": out })).into_response()
}

async fn handle_github_job_log(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }

    let job = {
        let jobs = state.github_jobs.read().await;
        jobs.get(job_id.trim()).cloned()
    };
    let Some(job) = job else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_job_not_found")),
        )
            .into_response();
    };
    let Some(rel) = job.log_path.clone() else {
        return (StatusCode::NOT_FOUND, Json(json_error("github_job_no_log"))).into_response();
    };

    let path = match safe_join(&state.config.codex_home, &rel) {
        Ok(path) => path,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_path_invalid")),
            )
                .into_response();
        }
    };

    match tokio::fs::metadata(&path).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error("github_job_log_not_found")),
            )
                .into_response();
        }
        Err(err) => {
            warn!(
                "failed to stat github job log for {} at {}: {err}",
                job.job_id,
                path.display()
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_read_failed")),
            )
                .into_response();
        }
    }

    match read_tail_file(&path, GITHUB_JOB_LOG_MAX_BYTES).await {
        Ok((log_text, truncated)) => Json(GithubJobLogResponse {
            job_id: job.job_id,
            log_text,
            truncated,
        })
        .into_response(),
        Err(err) => {
            warn!("failed to read github job log for {}: {err:#}", job.job_id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_read_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_github_sync(State(state): State<AppState>) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    if state.github_repos.read().await.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_no_repos_configured")),
        )
            .into_response();
    }
    let _guard = state.github_sync_lock.lock().await;
    if let Err(err) = sync_github_work_items(&state).await {
        warn!("github sync failed: {err:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("github_sync_failed")),
        )
            .into_response();
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_list_workspaces(State(state): State<AppState>) -> Response {
    let store = state.workspaces.read().await;
    Json(store.list()).into_response()
}

async fn handle_create_workspace(
    State(state): State<AppState>,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Response {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("workspace_invalid_name")),
        )
            .into_response();
    }
    if body.repos.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("workspace_invalid_repos")),
        )
            .into_response();
    }
    {
        let mut seen = HashSet::new();
        for repo in &body.repos {
            let full_name = repo.full_name.trim();
            if !is_valid_repo_full_name(full_name) || !seen.insert(full_name.to_string()) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error("workspace_invalid_repos")),
                )
                    .into_response();
            }
        }
    }

    let repos = body
        .repos
        .into_iter()
        .map(|repo| workspace::RepoInput {
            full_name: repo.full_name,
            color: repo.color,
            short_label: repo.short_label,
            default_branch: repo.default_branch,
        })
        .collect::<Vec<_>>();

    let mut store = state.workspaces.write().await;
    match store
        .create(
            &state.config.codex_home,
            workspace::CreateWorkspaceInput {
                name,
                repos,
                board: body.board,
                default_exec: body.default_exec,
                now_ms: now_ms(),
            },
        )
        .await
    {
        Ok(ws) => Json(ws).into_response(),
        Err(err) => {
            warn!("failed to create workspace: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("workspace_create_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_get_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    let store = state.workspaces.read().await;
    let Some(ws) = store.get(workspace_id.trim()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    };
    Json(ws).into_response()
}

async fn handle_update_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<UpdateWorkspaceRequest>,
) -> Response {
    if let Some(name) = body.name.as_deref()
        && name.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("workspace_invalid_name")),
        )
            .into_response();
    }
    if let Some(repos) = body.repos.as_ref()
        && repos.is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("workspace_invalid_repos")),
        )
            .into_response();
    }
    if let Some(repos) = body.repos.as_ref() {
        let mut seen = HashSet::new();
        for repo in repos {
            let full_name = repo.full_name.trim();
            if !is_valid_repo_full_name(full_name) || !seen.insert(full_name.to_string()) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error("workspace_invalid_repos")),
                )
                    .into_response();
            }
        }
    }

    let repos = body.repos.map(|repos| {
        repos
            .into_iter()
            .map(|repo| workspace::RepoInput {
                full_name: repo.full_name,
                color: repo.color,
                short_label: repo.short_label,
                default_branch: repo.default_branch,
            })
            .collect()
    });

    let mut store = state.workspaces.write().await;
    match store
        .update(
            &state.config.codex_home,
            workspace_id.trim(),
            workspace::UpdateWorkspaceInput {
                name: body.name.map(|name| name.trim().to_string()),
                repos,
                board: body.board,
                default_exec: body.default_exec,
                now_ms: now_ms(),
            },
        )
        .await
    {
        Ok(Some(ws)) => Json(ws).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response(),
        Err(err) => {
            warn!("failed to update workspace {workspace_id}: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("workspace_update_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_delete_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    let mut store = state.workspaces.write().await;
    match store
        .delete(&state.config.codex_home, workspace_id.trim())
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response(),
        Err(err) => {
            warn!("failed to delete workspace {workspace_id}: {err:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("workspace_delete_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_workspace_sync(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    }
    let exists = state.workspaces.read().await.get(workspace_id).is_some();
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    }

    if let Err(err) = sync_workspace_work_items(&state, workspace_id).await {
        warn!("workspace sync failed for {workspace_id}: {err:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("github_sync_failed")),
        )
            .into_response();
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_workspace_work_items(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let exists = state.workspaces.read().await.get(workspace_id).is_some();
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    }
    let full = load_workspace_work_items_snapshot(&state.config.codex_home, workspace_id).await;
    let filtered = GithubWorkItemsSnapshot {
        fetched_at: full.fetched_at,
        items: full
            .items
            .into_iter()
            .filter(|i| i.state.eq_ignore_ascii_case("open"))
            .collect(),
    };
    Json(filtered).into_response()
}

async fn handle_workspace_kanban(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let workspace = state.workspaces.read().await.get(workspace_id);
    let Some(workspace) = workspace else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    };

    let lock = get_workspace_kanban_lock(&state, workspace_id).await;
    let _guard = lock.lock().await;

    let keys: HashSet<String> =
        load_workspace_work_items_snapshot(&state.config.codex_home, workspace_id)
            .await
            .items
            .iter()
            .filter(|i| i.state.eq_ignore_ascii_case("open"))
            .map(|i| i.work_item_key.clone())
            .collect();

    let mut kanban =
        load_or_init_workspace_kanban(&state.config.codex_home, workspace_id, &workspace.board)
            .await;
    let changed = kanban.reconcile_sessions(&keys);
    let snapshot = kanban.clone();
    if changed {
        persist_workspace_kanban(&state.config.codex_home, workspace_id, &snapshot).await;
    }
    Json(snapshot).into_response()
}

async fn handle_update_workspace_kanban_card_settings(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<UpdateGithubKanbanCardSettingsRequest>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let workspace = state.workspaces.read().await.get(workspace_id);
    let Some(workspace) = workspace else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    };

    let work_item_key = body.work_item_key.trim().to_string();
    if work_item_key.is_empty() || parse_github_work_item_key(&work_item_key).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    }

    let lock = get_workspace_kanban_lock(&state, workspace_id).await;
    let _guard = lock.lock().await;

    let prompt_prefix = normalize_optional_text(body.prompt_prefix.as_deref());
    let model = normalize_optional_text(body.model.as_deref());
    let reasoning_effort = body.reasoning_effort;

    let mut kanban =
        load_or_init_workspace_kanban(&state.config.codex_home, workspace_id, &workspace.board)
            .await;
    let mut changed = false;
    let current = kanban
        .card_settings
        .get(&work_item_key)
        .cloned()
        .unwrap_or_default();
    let next = kanban::KanbanCardSettings {
        prompt_prefix,
        model,
        reasoning_effort,
    };
    if current != next {
        changed = true;
        if next.prompt_prefix.is_none() && next.model.is_none() && next.reasoning_effort.is_none() {
            kanban.card_settings.remove(&work_item_key);
        } else {
            kanban.card_settings.insert(work_item_key.clone(), next);
        }
    }
    let snapshot = kanban.clone();

    if changed {
        persist_workspace_kanban(&state.config.codex_home, workspace_id, &snapshot).await;
        // TODO: add a dedicated workspace kanban SSE variant
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state
            .events_tx
            .send(SyncEvent::GithubKanbanUpdated { data });
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_move_workspace_kanban_card(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(body): Json<MoveGithubKanbanCardRequest>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let workspace = state.workspaces.read().await.get(workspace_id);
    let Some(workspace) = workspace else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    };

    let work_item_key = body.work_item_key.trim().to_string();
    if work_item_key.is_empty() || parse_github_work_item_key(&work_item_key).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    }

    // Keep a clone of the board config for autoTrigger lookup after kanban ops
    let board = workspace.board.clone();

    let lock = get_workspace_kanban_lock(&state, workspace_id).await;
    let _guard = lock.lock().await;

    let mut kanban =
        load_or_init_workspace_kanban(&state.config.codex_home, workspace_id, &workspace.board)
            .await;
    if !kanban.has_column(&body.column_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("kanban_unknown_column")),
        )
            .into_response();
    }

    let mut settings_changed = false;
    let prompt_prefix = normalize_optional_text(body.prompt_prefix.as_deref());
    let model = normalize_optional_text(body.model.as_deref());
    if body.prompt_prefix.is_some() || body.model.is_some() || body.reasoning_effort.is_some() {
        let current = kanban
            .card_settings
            .get(&work_item_key)
            .cloned()
            .unwrap_or_default();
        let mut next = current.clone();
        if body.prompt_prefix.is_some() {
            next.prompt_prefix = prompt_prefix;
        }
        if body.model.is_some() {
            next.model = model;
        }
        if body.reasoning_effort.is_some() {
            next.reasoning_effort = body.reasoning_effort;
        }
        if next != current {
            settings_changed = true;
            if next.prompt_prefix.is_none()
                && next.model.is_none()
                && next.reasoning_effort.is_none()
            {
                kanban.card_settings.remove(&work_item_key);
            } else {
                kanban.card_settings.insert(work_item_key.clone(), next);
            }
        }
    }

    let prev_col = kanban
        .card_positions
        .get(&work_item_key)
        .map(|pos| pos.column_id.clone());
    let changed = kanban.move_card(&work_item_key, &body.column_id, body.position);
    let run_settings = kanban
        .card_settings
        .get(&work_item_key)
        .cloned()
        .unwrap_or_default();
    let snapshot = kanban.clone();
    if changed || settings_changed {
        persist_workspace_kanban(&state.config.codex_home, workspace_id, &snapshot).await;
        // TODO: add a dedicated workspace kanban SSE variant
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state
            .events_tx
            .send(SyncEvent::GithubKanbanUpdated { data });
    }

    // Use autoTrigger from board columns instead of hardcoded column names
    let target_trigger = board
        .columns
        .iter()
        .find(|c| c.id == body.column_id)
        .and_then(|c| c.auto_trigger);

    if changed
        && matches!(target_trigger, Some(workspace::AutoTrigger::StartExecution))
        && prev_col.as_deref() != Some(&body.column_id)
        && state.github_webhook.is_some()
        && let Err(err) =
            enqueue_workspace_github_job(&state, workspace_id, &work_item_key, run_settings).await
    {
        warn!("failed to enqueue github job for {work_item_key}: {err:#}");
    }

    if changed
        && matches!(target_trigger, Some(workspace::AutoTrigger::CloseIssue))
        && prev_col.as_deref() != Some(&body.column_id)
        && let Some(webhook) = state.github_webhook.as_ref()
        && let Some((repo, number, kind)) = parse_github_work_item_key(&work_item_key)
        && kind == "issue"
        && let Err(err) = webhook.set_work_item_state(&repo, number, "closed").await
    {
        warn!("failed to close github issue {repo}#{number} for workspace {workspace_id}: {err:#}");
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_workspace_jobs(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let exists = state.workspaces.read().await.get(workspace_id).is_some();
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    }

    let keys: HashSet<String> =
        load_workspace_work_items_snapshot(&state.config.codex_home, workspace_id)
            .await
            .items
            .iter()
            .map(|i| i.work_item_key.clone())
            .collect();
    let jobs = state.github_jobs.read().await;
    let mut out: Vec<GithubJob> = jobs
        .values()
        .filter(|job| keys.contains(&job.work_item_key))
        .cloned()
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Json(serde_json::json!({ "jobs": out })).into_response()
}

async fn handle_workspace_job_log(
    State(state): State<AppState>,
    Path((workspace_id, job_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = workspace_id.trim();
    let exists = state.workspaces.read().await.get(workspace_id).is_some();
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("workspace_not_found")),
        )
            .into_response();
    }

    let keys: HashSet<String> =
        load_workspace_work_items_snapshot(&state.config.codex_home, workspace_id)
            .await
            .items
            .iter()
            .map(|i| i.work_item_key.clone())
            .collect();

    let job = {
        let jobs = state.github_jobs.read().await;
        jobs.get(job_id.trim()).cloned()
    };
    let Some(job) = job else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_job_not_found")),
        )
            .into_response();
    };
    if !keys.contains(&job.work_item_key) {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_job_not_found")),
        )
            .into_response();
    }

    let Some(rel) = job.log_path.clone() else {
        return (StatusCode::NOT_FOUND, Json(json_error("github_job_no_log"))).into_response();
    };

    let path = match safe_join(&state.config.codex_home, &rel) {
        Ok(path) => path,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_path_invalid")),
            )
                .into_response();
        }
    };

    match tokio::fs::metadata(&path).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error("github_job_log_not_found")),
            )
                .into_response();
        }
        Err(err) => {
            warn!(
                "failed to stat github job log for {} at {}: {err}",
                job.job_id,
                path.display()
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_read_failed")),
            )
                .into_response();
        }
    }

    match read_tail_file(&path, GITHUB_JOB_LOG_MAX_BYTES).await {
        Ok((log_text, truncated)) => Json(GithubJobLogResponse {
            job_id: job.job_id,
            log_text,
            truncated,
        })
        .into_response(),
        Err(err) => {
            warn!("failed to read github job log for {}: {err:#}", job.job_id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("github_job_log_read_failed")),
            )
                .into_response()
        }
    }
}

async fn handle_update_github_kanban_card_settings(
    State(state): State<AppState>,
    Json(body): Json<UpdateGithubKanbanCardSettingsRequest>,
) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let work_item_key = body.work_item_key.trim().to_string();
    if work_item_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    }

    let prompt_prefix = normalize_optional_text(body.prompt_prefix.as_deref());
    let model = normalize_optional_text(body.model.as_deref());
    let reasoning_effort = body.reasoning_effort;

    let mut kanban = state.github_kanban.write().await;
    let mut changed = false;
    let current = kanban
        .card_settings
        .get(&work_item_key)
        .cloned()
        .unwrap_or_default();
    let next = kanban::KanbanCardSettings {
        prompt_prefix,
        model,
        reasoning_effort,
    };
    if current != next {
        changed = true;
        if next.prompt_prefix.is_none() && next.model.is_none() && next.reasoning_effort.is_none() {
            kanban.card_settings.remove(&work_item_key);
        } else {
            kanban.card_settings.insert(work_item_key.clone(), next);
        }
    }
    let snapshot = kanban.clone();
    drop(kanban);

    if changed {
        kanban::persist_to(&state.config.codex_home, GITHUB_KANBAN_FILE_NAME, &snapshot).await;
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state
            .events_tx
            .send(SyncEvent::GithubKanbanUpdated { data });
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_get_github_kanban(State(state): State<AppState>) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let keys: HashSet<String> = state
        .github_work_items
        .read()
        .await
        .items
        .iter()
        .filter(|i| i.state.eq_ignore_ascii_case("open"))
        .map(|i| i.work_item_key.clone())
        .collect();

    let mut kanban = state.github_kanban.write().await;
    let changed = kanban.reconcile_sessions(&keys);
    let snapshot = kanban.clone();
    drop(kanban);
    if changed {
        kanban::persist_to(&state.config.codex_home, GITHUB_KANBAN_FILE_NAME, &snapshot).await;
    }
    Json(snapshot).into_response()
}

async fn handle_move_github_kanban_card(
    State(state): State<AppState>,
    Json(body): Json<MoveGithubKanbanCardRequest>,
) -> Response {
    if state.github_webhook.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("github_not_enabled")),
        )
            .into_response();
    }
    let work_item_key = body.work_item_key.trim().to_string();
    if work_item_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("github_invalid_work_item_key")),
        )
            .into_response();
    }

    let mut kanban = state.github_kanban.write().await;
    if !kanban.has_column(&body.column_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("kanban_unknown_column")),
        )
            .into_response();
    }
    let mut settings_changed = false;
    let prompt_prefix = normalize_optional_text(body.prompt_prefix.as_deref());
    let model = normalize_optional_text(body.model.as_deref());
    if body.prompt_prefix.is_some() || body.model.is_some() || body.reasoning_effort.is_some() {
        let current = kanban
            .card_settings
            .get(&work_item_key)
            .cloned()
            .unwrap_or_default();
        let mut next = current.clone();
        if body.prompt_prefix.is_some() {
            next.prompt_prefix = prompt_prefix;
        }
        if body.model.is_some() {
            next.model = model;
        }
        if body.reasoning_effort.is_some() {
            next.reasoning_effort = body.reasoning_effort;
        }
        if next != current {
            settings_changed = true;
            if next.prompt_prefix.is_none()
                && next.model.is_none()
                && next.reasoning_effort.is_none()
            {
                kanban.card_settings.remove(&work_item_key);
            } else {
                kanban.card_settings.insert(work_item_key.clone(), next);
            }
        }
    }
    let prev_col = kanban
        .card_positions
        .get(&work_item_key)
        .map(|pos| pos.column_id.clone());
    let changed = kanban.move_card(&work_item_key, &body.column_id, body.position);
    let run_settings = kanban
        .card_settings
        .get(&work_item_key)
        .cloned()
        .unwrap_or_default();
    let snapshot = kanban.clone();
    let final_position = snapshot
        .card_positions
        .get(&work_item_key)
        .map(|pos| pos.position)
        .unwrap_or(body.position);
    drop(kanban);

    if changed || settings_changed {
        kanban::persist_to(&state.config.codex_home, GITHUB_KANBAN_FILE_NAME, &snapshot).await;
        let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
        let _ = state
            .events_tx
            .send(SyncEvent::GithubKanbanUpdated { data });
        let _ = state.events_tx.send(SyncEvent::GithubCardMoved {
            work_item_key: work_item_key.clone(),
            column_id: body.column_id.clone(),
            position: final_position,
        });
    }

    if changed
        && body.column_id == "in-progress"
        && prev_col.as_deref() != Some("in-progress")
        && let Err(err) = enqueue_github_job(&state, &work_item_key, run_settings).await
    {
        warn!("failed to enqueue github job for {work_item_key}: {err:#}");
    }

    Json(serde_json::json!({})).into_response()
}

async fn handle_sessions(State(state): State<AppState>) -> Response {
    let page = match codex_core::RolloutRecorder::list_threads(
        state.config.as_ref(),
        2000,
        None,
        codex_core::ThreadSortKey::UpdatedAt,
        codex_core::INTERACTIVE_SESSION_SOURCES,
        None,
        &state.config.model_provider_id,
        None,
    )
    .await
    {
        Ok(page) => page,
        Err(_) => {
            return Json(SessionsResponse {
                sessions: Vec::new(),
            })
            .into_response();
        }
    };

    let mut ids: HashSet<ThreadId> = HashSet::new();
    for item in &page.items {
        if let Some(thread_id) = item.thread_id {
            ids.insert(thread_id);
        }
    }
    let names = codex_core::find_thread_names_by_ids(&state.config.codex_home, &ids)
        .await
        .unwrap_or_default();

    let active = state.sessions.read().await;
    let now = now_ms();
    let mut sessions = Vec::new();
    for item in page.items {
        let Some(thread_id) = item.thread_id else {
            continue;
        };

        let id = thread_id.to_string();
        let name = names.get(&thread_id).cloned().or_else(|| {
            active
                .get(&id)
                .and_then(|s| s.state.try_read().ok()?.name.clone())
        });
        let cwd = item
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let updated_at = item
            .updated_at
            .as_deref()
            .and_then(parse_rfc3339_ms)
            .unwrap_or(now);
        let active_session = active.get(&id);
        let (is_active, thinking, active_at, pending) = if let Some(session) = active_session {
            let guard = session.state.read().await;
            let pending = guard
                .agent_state
                .requests
                .as_ref()
                .map(|m| m.len() as u64)
                .unwrap_or(0);
            (guard.active, guard.thinking, guard.active_at, pending)
        } else {
            (false, false, updated_at, 0)
        };

        sessions.push(SessionSummary {
            id,
            active: is_active,
            thinking,
            active_at,
            updated_at,
            metadata: Some(SessionSummaryMetadata {
                name,
                path: cwd,
                machine_id: Some("local".to_string()),
                summary: None,
                flavor: Some("codex".to_string()),
                worktree: None,
            }),
            todo_progress: None,
            pending_requests_count: pending,
            model_mode: None,
        });
    }

    Json(SessionsResponse { sessions }).into_response()
}

async fn handle_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if let Some(session) = state.sessions.read().await.get(&id).cloned() {
        let s = build_session_json(&session).await;
        return Json(SessionResponse { session: s }).into_response();
    }

    let now = now_ms();

    let rollout_path = match codex_core::find_thread_path_by_id_str(&state.config.codex_home, &id)
        .await
        .ok()
        .flatten()
    {
        Some(path) => Some(path),
        None => codex_core::find_archived_thread_path_by_id_str(&state.config.codex_home, &id)
            .await
            .ok()
            .flatten(),
    };
    let Some(rollout_path) = rollout_path else {
        return (StatusCode::NOT_FOUND, Json(json_error("session_not_found"))).into_response();
    };

    let history = match codex_core::RolloutRecorder::get_rollout_history(&rollout_path).await {
        Ok(history) => history,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_error(&format!("resume_failed: {err}"))),
            )
                .into_response();
        }
    };

    let items = history.get_rollout_items();
    let created_at = items
        .iter()
        .find_map(|item| match item {
            codex_protocol::protocol::RolloutItem::SessionMeta(meta_line) => {
                parse_rfc3339_ms(&meta_line.meta.timestamp)
            }
            _ => None,
        })
        .unwrap_or(now);

    let updated_at = tokio::fs::metadata(&rollout_path)
        .await
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(created_at);

    let cwd = history
        .session_cwd()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let name = match ThreadId::from_string(&id) {
        Ok(thread_id) => codex_core::find_thread_name_by_id(&state.config.codex_home, &thread_id)
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };

    let seq = items
        .iter()
        .filter(|item| {
            matches!(
                item,
                codex_protocol::protocol::RolloutItem::ResponseItem(
                    codex_protocol::models::ResponseItem::Message { .. }
                )
            )
        })
        .count() as u64;

    Json(SessionResponse {
        session: Session {
            id,
            namespace: "local".to_string(),
            seq,
            created_at,
            updated_at,
            active: false,
            active_at: updated_at,
            metadata: Some(Metadata {
                path: cwd.display().to_string(),
                host: "local".to_string(),
                name,
                machine_id: Some("local".to_string()),
                tools: None,
                flavor: Some("codex".to_string()),
                summary: None,
            }),
            metadata_version: 0,
            agent_state: None,
            agent_state_version: 0,
            thinking: false,
            thinking_at: updated_at,
            permission_mode: Some("default".to_string()),
            model_mode: Some("default".to_string()),
        },
    })
    .into_response()
}

async fn handle_resume_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if state.sessions.read().await.contains_key(&id) {
        return Json(serde_json::json!({ "sessionId": id })).into_response();
    }

    let Some(rollout_path) = codex_core::find_thread_path_by_id_str(&state.config.codex_home, &id)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, Json(json_error("session_not_found"))).into_response();
    };

    let initial_history =
        match codex_core::RolloutRecorder::get_rollout_history(&rollout_path).await {
            Ok(history) => history,
            Err(err) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error(&format!("resume_failed: {err}"))),
                )
                    .into_response();
            }
        };
    let cwd = initial_history
        .session_cwd()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let recovered_effort = extract_reasoning_effort_from_history(&initial_history);
    let mut overrides = state.base_overrides.clone();
    overrides.cwd = Some(cwd);
    let mut config = match Config::load_with_cli_overrides_and_harness_overrides(
        state.cli_overrides.clone(),
        overrides,
    )
    .await
    {
        Ok(cfg) => cfg,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error(&format!("config_load_failed: {err}"))),
            )
                .into_response();
        }
    };
    if let Some(effort) = recovered_effort {
        config.model_reasoning_effort = Some(effort);
    }

    let new_thread = match state
        .thread_manager
        .resume_thread_with_history(config, initial_history, state.auth_manager.clone(), true)
        .await
    {
        Ok(new_thread) => new_thread,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error(&format!("resume_failed: {err}"))),
            )
                .into_response();
        }
    };

    let thread_id = new_thread.thread_id;
    let session_id = thread_id.to_string();
    let session = Arc::new(ActiveSession {
        thread_id,
        thread: new_thread.thread,
        rollout_path: new_thread.session_configured.rollout_path.clone(),
        state: RwLock::new(SessionState {
            name: new_thread.session_configured.thread_name.clone(),
            cwd: new_thread.session_configured.cwd.clone(),
            model: new_thread.session_configured.model.clone(),
            reasoning_effort: new_thread.session_configured.reasoning_effort,
            created_at: now_ms(),
            updated_at: now_ms(),
            active: true,
            active_at: now_ms(),
            thinking: false,
            thinking_at: now_ms(),
            permission_mode: "default".to_string(),
            model_mode: "default".to_string(),
            metadata_version: 0,
            agent_state_version: 0,
            agent_state: WebAgentState::default(),
            next_seq: 1,
            messages: Vec::new(),
        }),
    });

    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), session.clone());
    tokio::spawn(session_event_loop(
        state.clone(),
        session_id.clone(),
        session,
    ));

    {
        let mut kanban = state.kanban.write().await;
        let changed = kanban.ensure_session(&session_id);
        let snapshot = kanban.clone();
        drop(kanban);
        if changed {
            kanban::persist(&state.config.codex_home, &snapshot).await;
            let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
            let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
        }
    }
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: session_id.clone(),
        data: None,
    });
    Json(serde_json::json!({ "sessionId": session_id })).into_response()
}

async fn handle_abort_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return Json(serde_json::json!({})).into_response();
    };
    let _ = session.thread.submit(Op::Interrupt).await;
    Json(serde_json::json!({})).into_response()
}

async fn handle_archive_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let rollout_path = if let Some(session) = state.sessions.read().await.get(&id).cloned() {
        session.rollout_path.clone()
    } else {
        codex_core::find_thread_path_by_id_str(&state.config.codex_home, &id)
            .await
            .ok()
            .flatten()
    };

    let Some(path) = rollout_path else {
        return Json(serde_json::json!({})).into_response();
    };

    let archived_root = state
        .config
        .codex_home
        .join(codex_core::ARCHIVED_SESSIONS_SUBDIR);
    if let Err(err) = tokio::fs::create_dir_all(&archived_root).await {
        warn!("failed to create archived_sessions dir: {err}");
    }
    let file_name = path.file_name().map(|n| n.to_string_lossy().to_string());
    let dest = file_name
        .map(|name| archived_root.join(name))
        .unwrap_or_else(|| archived_root.join(format!("{id}.jsonl")));
    let _ = tokio::fs::rename(&path, &dest).await;

    state.sessions.write().await.remove(&id);
    let _ = state.events_tx.send(SyncEvent::SessionRemoved {
        session_id: id.clone(),
    });

    {
        let mut kanban = state.kanban.write().await;
        let changed = kanban.remove_session(&id);
        let snapshot = kanban.clone();
        drop(kanban);
        if changed {
            kanban::persist(&state.config.codex_home, &snapshot).await;
            let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
            let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
        }
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_delete_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    state.sessions.write().await.remove(&id);

    if let Some(path) = codex_core::find_thread_path_by_id_str(&state.config.codex_home, &id)
        .await
        .ok()
        .flatten()
    {
        let _ = tokio::fs::remove_file(path).await;
    }
    if let Some(path) =
        codex_core::find_archived_thread_path_by_id_str(&state.config.codex_home, &id)
            .await
            .ok()
            .flatten()
    {
        let _ = tokio::fs::remove_file(path).await;
    }

    let _ = state.events_tx.send(SyncEvent::SessionRemoved {
        session_id: id.clone(),
    });

    {
        let mut kanban = state.kanban.write().await;
        let changed = kanban.remove_session(&id);
        let snapshot = kanban.clone();
        drop(kanban);
        if changed {
            kanban::persist(&state.config.codex_home, &snapshot).await;
            let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
            let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
        }
    }
    Json(serde_json::json!({})).into_response()
}

async fn handle_rename_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RenameSessionRequest>,
) -> Response {
    let normalized = codex_core::util::normalize_thread_name(&body.name);
    let thread_id = state
        .sessions
        .read()
        .await
        .get(&id)
        .map(|s| s.thread_id)
        .or_else(|| ThreadId::from_string(&id).ok());

    if let (Some(thread_id), Some(name)) = (thread_id, normalized.as_deref()) {
        match persist_thread_name(&state.config.codex_home, thread_id, name).await {
            Ok(()) => {}
            Err(err) => warn!("failed to persist thread name: {err}"),
        }
    }

    if let Some(session) = state.sessions.read().await.get(&id).cloned() {
        session.state.write().await.name = normalized.clone();
        if let Some(name) = normalized {
            let _ = session.thread.submit(Op::SetThreadName { name }).await;
        }
    }
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: id,
        data: None,
    });
    Json(serde_json::json!({})).into_response()
}

async fn handle_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as usize;
    let before_seq = query.before_seq;
    let mut all_messages = if let Some(session) = state.sessions.read().await.get(&id).cloned() {
        session.state.read().await.messages.clone()
    } else {
        load_messages_from_rollout(&state, &id)
            .await
            .unwrap_or_default()
    };

    all_messages.sort_by(|a, b| a.seq.unwrap_or(0).cmp(&b.seq.unwrap_or(0)));
    let filtered: Vec<WebDecryptedMessage> = match before_seq {
        Some(before) => all_messages
            .into_iter()
            .filter(|m| m.seq.unwrap_or(0) < before)
            .collect(),
        None => all_messages,
    };

    let has_more = filtered.len() > limit;
    let slice = if filtered.len() <= limit {
        filtered
    } else {
        filtered[filtered.len() - limit..].to_vec()
    };
    let next_before_seq = slice.first().and_then(|m| m.seq);

    Json(MessagesResponse {
        messages: slice,
        page: MessagesPage {
            limit: limit as u64,
            before_seq,
            next_before_seq,
            has_more,
        },
    })
    .into_response()
}

async fn handle_post_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MessagePostRequest>,
) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("session_inactive")),
        )
            .into_response();
    };

    let created_at = now_ms();
    let text = body.text;
    let text_for_content = text.clone();
    let local_id = body.local_id;
    let attachments = body.attachments;
    let (message, op) = {
        let mut guard = session.state.write().await;
        let seq = guard.next_seq;
        guard.next_seq += 1;
        guard.updated_at = created_at;
        guard.active = true;
        guard.active_at = created_at;

        let content = JsonValue::Object({
            let mut obj = serde_json::Map::new();
            obj.insert("role".to_string(), JsonValue::String("user".to_string()));
            obj.insert(
                "content".to_string(),
                serde_json::json!({
                    "type": "text",
                    "text": text_for_content,
                    "attachments": attachments,
                }),
            );
            obj
        });

        let message = WebDecryptedMessage {
            id: uuid::Uuid::new_v4().to_string(),
            seq: Some(seq),
            local_id: local_id.clone(),
            content,
            created_at,
            status: None,
            original_text: None,
        };
        guard.messages.push(message.clone());

        let (approval_policy, sandbox_policy) = permission_mode_to_policies(&guard.permission_mode);
        let collaboration_mode = if guard.permission_mode == "plan" {
            let masks = state.thread_manager.list_collaboration_modes();
            let developer_instructions = plan_mode_developer_instructions(&masks);
            Some(CollaborationMode {
                mode: ModeKind::Plan,
                settings: Settings {
                    model: guard.model.clone(),
                    reasoning_effort: guard.reasoning_effort,
                    developer_instructions,
                },
            })
        } else {
            None
        };

        let op = Op::UserTurn {
            items: vec![UserInput::Text {
                text,
                text_elements: Vec::new(),
            }],
            cwd: guard.cwd.clone(),
            approval_policy,
            sandbox_policy,
            model: guard.model.clone(),
            effort: guard.reasoning_effort,
            summary: Some(ReasoningSummaryConfig::Auto),
            service_tier: None,
            final_output_json_schema: None,
            collaboration_mode,
            personality: None,
        };

        (message, op)
    };

    let _ = state.events_tx.send(SyncEvent::MessageReceived {
        session_id: id.clone(),
        message: message.clone(),
    });
    let _ = session.thread.submit(op).await;
    Json(serde_json::json!({})).into_response()
}

async fn handle_set_permission_mode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PermissionModeRequest>,
) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return Json(serde_json::json!({})).into_response();
    };
    session.state.write().await.permission_mode = body.mode;
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: id,
        data: None,
    });
    Json(serde_json::json!({})).into_response()
}

async fn handle_set_model_mode(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ModelModeRequest>,
) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return Json(serde_json::json!({})).into_response();
    };
    session.state.write().await.model_mode = body.model;
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: id,
        data: None,
    });
    Json(serde_json::json!({})).into_response()
}

async fn handle_approve_permission(
    State(state): State<AppState>,
    Path((id, req_id)): Path<(String, String)>,
    Json(body): Json<ApprovePermissionRequest>,
) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return Json(serde_json::json!({})).into_response();
    };

    let completed_at = now_ms();
    let op = {
        let mut guard = session.state.write().await;
        let Some(requests) = guard.agent_state.requests.as_mut() else {
            return Json(serde_json::json!({})).into_response();
        };
        let Some(pending) = requests.remove(&req_id) else {
            return Json(serde_json::json!({})).into_response();
        };
        let decision = body.decision.as_deref().unwrap_or("approved").to_string();

        let op = build_permission_op(&pending.arguments, &decision, body.answers.clone());

        let completed = WebAgentCompletedRequest {
            tool: pending.tool.clone(),
            arguments: pending.arguments.clone(),
            created_at: pending.created_at,
            completed_at: Some(completed_at),
            status: "approved".to_string(),
            reason: None,
            mode: body.mode.clone(),
            decision: Some(decision),
            allow_tools: body.allow_tools.clone(),
            answers: body.answers.clone(),
        };
        guard
            .agent_state
            .completed_requests
            .get_or_insert_with(HashMap::new)
            .insert(req_id.clone(), completed);
        guard.agent_state_version += 1;

        op
    };

    if let Some(op) = op {
        let _ = session.thread.submit(op).await;
    }
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: id,
        data: None,
    });
    Json(serde_json::json!({ "ok": true })).into_response()
}

async fn handle_deny_permission(
    State(state): State<AppState>,
    Path((id, req_id)): Path<(String, String)>,
    Json(body): Json<DenyPermissionRequest>,
) -> Response {
    let Some(session) = state.sessions.read().await.get(&id).cloned() else {
        return Json(serde_json::json!({})).into_response();
    };

    let completed_at = now_ms();
    let op = {
        let mut guard = session.state.write().await;
        let Some(requests) = guard.agent_state.requests.as_mut() else {
            return Json(serde_json::json!({})).into_response();
        };
        let Some(pending) = requests.remove(&req_id) else {
            return Json(serde_json::json!({})).into_response();
        };
        let decision = body.decision.as_deref().unwrap_or("denied").to_string();

        let op = build_permission_op(&pending.arguments, &decision, None);

        let completed = WebAgentCompletedRequest {
            tool: pending.tool.clone(),
            arguments: pending.arguments.clone(),
            created_at: pending.created_at,
            completed_at: Some(completed_at),
            status: "denied".to_string(),
            reason: None,
            mode: None,
            decision: Some(decision),
            allow_tools: None,
            answers: None,
        };
        guard
            .agent_state
            .completed_requests
            .get_or_insert_with(HashMap::new)
            .insert(req_id.clone(), completed);
        guard.agent_state_version += 1;
        op
    };

    if let Some(op) = op {
        let _ = session.thread.submit(op).await;
    }
    let _ = state.events_tx.send(SyncEvent::SessionUpdated {
        session_id: id,
        data: None,
    });
    Json(serde_json::json!({ "ok": true })).into_response()
}

fn build_permission_op(
    arguments: &JsonValue,
    decision: &str,
    answers: Option<JsonValue>,
) -> Option<Op> {
    let kind = arguments
        .get("kind")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    match kind {
        "exec" => Some(Op::ExecApproval {
            id: arguments
                .get("approvalId")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string(),
            turn_id: arguments
                .get("turnId")
                .and_then(JsonValue::as_str)
                .map(ToOwned::to_owned),
            decision: decision_to_review_decision(decision, arguments),
        }),
        "patch" => Some(Op::PatchApproval {
            id: arguments
                .get("callId")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string(),
            decision: decision_to_review_decision(decision, arguments),
        }),
        "request_user_input" => {
            let turn_id = arguments
                .get("turnId")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            let response = answers_to_user_input_response(answers.unwrap_or(JsonValue::Null));
            Some(Op::UserInputAnswer {
                id: turn_id,
                response,
            })
        }
        _ => None,
    }
}

fn decision_to_review_decision(decision: &str, arguments: &JsonValue) -> ReviewDecision {
    match decision {
        "abort" => ReviewDecision::Abort,
        "approved_for_session" => ReviewDecision::ApprovedForSession,
        "approved" => ReviewDecision::Approved,
        "approved_with_amendment" | "approved_execpolicy_amendment" => {
            let amendment = arguments
                .get("proposedExecpolicyAmendment")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
            if let Some(amendment) = amendment {
                ReviewDecision::ApprovedExecpolicyAmendment {
                    proposed_execpolicy_amendment: amendment,
                }
            } else {
                ReviewDecision::Approved
            }
        }
        _ => ReviewDecision::Denied,
    }
}

fn answers_to_user_input_response(raw: JsonValue) -> RequestUserInputResponse {
    let mut answers = HashMap::new();
    let JsonValue::Object(map) = raw else {
        return RequestUserInputResponse { answers };
    };
    for (k, v) in map {
        let extracted: Vec<String> = match v {
            JsonValue::Array(arr) => arr
                .into_iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect(),
            JsonValue::Object(obj) => obj
                .get("answers")
                .and_then(JsonValue::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        answers.insert(k, RequestUserInputAnswer { answers: extracted });
    }
    RequestUserInputResponse { answers }
}

async fn handle_machines() -> Response {
    let host = "localhost".to_string();
    Json(MachinesResponse {
        machines: vec![Machine {
            id: "local".to_string(),
            active: true,
            metadata: Some(MachineMetadata {
                host,
                platform: std::env::consts::OS.to_string(),
                happy_cli_version: "codex".to_string(),
                display_name: Some("Local".to_string()),
            }),
        }],
    })
    .into_response()
}

async fn handle_machine_paths_exists(
    Path(machine_id): Path<String>,
    Json(body): Json<CheckPathsExistsRequest>,
) -> Response {
    if machine_id != "local" {
        return (StatusCode::NOT_FOUND, Json(json_error("machine_not_found"))).into_response();
    }
    let mut exists = HashMap::new();
    for path in body.paths {
        exists.insert(path.clone(), FsPath::new(&path).exists());
    }
    Json(CheckPathsExistsResponse { exists }).into_response()
}

async fn handle_machine_spawn(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Json(body): Json<SpawnRequest>,
) -> Response {
    if machine_id != "local" {
        return Json(SpawnError {
            kind: "error",
            message: "machine not found".to_string(),
        })
        .into_response();
    }
    let directory = PathBuf::from(body.directory);
    if !directory.is_dir() {
        return Json(SpawnError {
            kind: "error",
            message: "directory not found".to_string(),
        })
        .into_response();
    }

    if let Some(agent) = body.agent.as_deref()
        && agent != "codex"
    {
        return Json(SpawnError {
            kind: "error",
            message: format!("unsupported agent: {agent}"),
        })
        .into_response();
    }

    let mut overrides = state.base_overrides.clone();
    overrides.cwd = Some(directory.clone());
    if let Some(model) = body.model.clone() {
        overrides.model = Some(model);
    }
    let mut config = match Config::load_with_cli_overrides_and_harness_overrides(
        state.cli_overrides.clone(),
        overrides,
    )
    .await
    {
        Ok(cfg) => cfg,
        Err(err) => {
            return Json(SpawnError {
                kind: "error",
                message: format!("config load failed: {err}"),
            })
            .into_response();
        }
    };
    if let Some(effort) = body.reasoning_effort {
        config.model_reasoning_effort = Some(effort);
    }

    let new_thread = match state
        .thread_manager
        .start_thread_with_tools(config, Vec::new(), true)
        .await
    {
        Ok(new_thread) => new_thread,
        Err(err) => {
            return Json(SpawnError {
                kind: "error",
                message: format!("spawn failed: {err}"),
            })
            .into_response();
        }
    };

    let thread_id = new_thread.thread_id;
    let session_id = thread_id.to_string();
    let permission_mode = if body.yolo.unwrap_or(false) {
        "yolo".to_string()
    } else {
        "default".to_string()
    };

    let session = Arc::new(ActiveSession {
        thread_id,
        thread: new_thread.thread,
        rollout_path: new_thread.session_configured.rollout_path.clone(),
        state: RwLock::new(SessionState {
            name: new_thread.session_configured.thread_name.clone(),
            cwd: directory,
            model: new_thread.session_configured.model.clone(),
            reasoning_effort: new_thread.session_configured.reasoning_effort,
            created_at: now_ms(),
            updated_at: now_ms(),
            active: true,
            active_at: now_ms(),
            thinking: false,
            thinking_at: now_ms(),
            permission_mode,
            model_mode: "default".to_string(),
            metadata_version: 0,
            agent_state_version: 0,
            agent_state: WebAgentState::default(),
            next_seq: 1,
            messages: Vec::new(),
        }),
    });

    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), session.clone());
    tokio::spawn(session_event_loop(
        state.clone(),
        session_id.clone(),
        session,
    ));

    {
        let mut kanban = state.kanban.write().await;
        let changed = kanban.ensure_session(&session_id);
        let snapshot = kanban.clone();
        drop(kanban);
        if changed {
            kanban::persist(&state.config.codex_home, &snapshot).await;
            let data = serde_json::to_value(&snapshot).unwrap_or(JsonValue::Null);
            let _ = state.events_tx.send(SyncEvent::KanbanUpdated { data });
        }
    }

    let _ = state.events_tx.send(SyncEvent::SessionAdded {
        session_id: session_id.clone(),
        data: None,
    });

    Json(SpawnSuccess {
        kind: "success",
        session_id,
    })
    .into_response()
}

async fn handle_git_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(GitCommandResponse {
                success: false,
                stdout: None,
                stderr: None,
                exit_code: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    Json(run_git(&cwd, ["status", "--porcelain=v2", "--branch"]).await).into_response()
}

async fn handle_git_diff_numstat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitDiffNumstatQuery>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(GitCommandResponse {
                success: false,
                stdout: None,
                stderr: None,
                exit_code: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    if query.staged.unwrap_or(false) {
        return Json(run_git(&cwd, ["diff", "--numstat", "--staged"]).await).into_response();
    }
    Json(run_git(&cwd, ["diff", "--numstat"]).await).into_response()
}

async fn handle_git_diff_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GitDiffFileQuery>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(GitCommandResponse {
                success: false,
                stdout: None,
                stderr: None,
                exit_code: None,
                error: Some(err),
            })
            .into_response();
        }
    };

    let path = query.path;
    let staged = query.staged.unwrap_or(false);
    if staged {
        return Json(run_git(&cwd, ["diff", "--staged", "--", &path]).await).into_response();
    }
    Json(run_git(&cwd, ["diff", "--", &path]).await).into_response()
}

async fn run_git<const N: usize>(cwd: &PathBuf, args: [&str; N]) -> GitCommandResponse {
    let mut cmd = tokio::process::Command::new("git");
    cmd.current_dir(cwd);
    cmd.args(args);
    let out = match cmd.output().await {
        Ok(out) => out,
        Err(err) => {
            return GitCommandResponse {
                success: false,
                stdout: None,
                stderr: None,
                exit_code: None,
                error: Some(err.to_string()),
            };
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    GitCommandResponse {
        success: out.status.success(),
        stdout: Some(stdout),
        stderr: Some(stderr),
        exit_code: out.status.code(),
        error: None,
    }
}

async fn handle_search_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Response {
    let Some(q) = query.query.filter(|q| !q.trim().is_empty()) else {
        return Json(FileSearchResponse {
            success: true,
            files: Some(Vec::new()),
            error: None,
        })
        .into_response();
    };
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(FileSearchResponse {
                success: false,
                files: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    let limit = query.limit.unwrap_or(200).clamp(1, 500) as usize;

    let matches = tokio::task::spawn_blocking(move || {
        let options = codex_file_search::FileSearchOptions {
            limit: std::num::NonZero::new(limit).unwrap_or(std::num::NonZero::<usize>::MIN),
            ..Default::default()
        };
        codex_file_search::run(&q, vec![cwd], options, None)
    })
    .await
    .map_err(|err| err.to_string())
    .and_then(|res| res.map_err(|err| err.to_string()));

    match matches {
        Ok(result) => {
            let files = result
                .matches
                .into_iter()
                .map(|m| {
                    let path = m.path.to_string_lossy().to_string();
                    let file_name = codex_file_search::file_name_from_path(&path);
                    let file_path = PathBuf::from(&path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    FileSearchItem {
                        file_name,
                        file_path,
                        full_path: path,
                        file_type: "file",
                    }
                })
                .collect();
            Json(FileSearchResponse {
                success: true,
                files: Some(files),
                error: None,
            })
            .into_response()
        }
        Err(err) => Json(FileSearchResponse {
            success: false,
            files: None,
            error: Some(err),
        })
        .into_response(),
    }
}

async fn handle_list_directory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DirectoryQuery>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(ListDirectoryResponse {
                success: false,
                entries: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    let rel = query.path.unwrap_or_default();
    let dir = match safe_join(&cwd, &rel) {
        Ok(dir) => dir,
        Err(err) => {
            return Json(ListDirectoryResponse {
                success: false,
                entries: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    let mut entries = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(err) => {
            return Json(ListDirectoryResponse {
                success: false,
                entries: None,
                error: Some(err.to_string()),
            })
            .into_response();
        }
    };
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = if meta.is_file() {
            "file"
        } else if meta.is_dir() {
            "directory"
        } else {
            "other"
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        entries.push(DirectoryEntry {
            name,
            kind,
            size: meta.is_file().then_some(meta.len()),
            modified,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Json(ListDirectoryResponse {
        success: true,
        entries: Some(entries),
        error: None,
    })
    .into_response()
}

async fn handle_read_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileReadQuery>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(FileReadResponse {
                success: false,
                content: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    let path = match safe_join(&cwd, &query.path) {
        Ok(path) => path,
        Err(err) => {
            return Json(FileReadResponse {
                success: false,
                content: None,
                error: Some(err),
            })
            .into_response();
        }
    };
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Json(FileReadResponse {
            success: true,
            content: Some(content),
            error: None,
        })
        .into_response(),
        Err(err) => Json(FileReadResponse {
            success: false,
            content: None,
            error: Some(err.to_string()),
        })
        .into_response(),
    }
}

async fn handle_upload_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UploadFileRequest>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(UploadFileResponse {
                success: false,
                path: None,
                error: Some(err),
            })
            .into_response();
        }
    };

    let file_name = FsPath::new(&body.filename)
        .file_name()
        .map(|s| s.to_string_lossy().to_string());
    let Some(file_name) = file_name else {
        return Json(UploadFileResponse {
            success: false,
            path: None,
            error: Some("invalid filename".to_string()),
        })
        .into_response();
    };

    let uploads_dir = cwd.join(".codex_uploads");
    let _ = tokio::fs::create_dir_all(&uploads_dir).await;

    let id = uuid::Uuid::new_v4().to_string();
    let rel_path = format!(".codex_uploads/{id}-{file_name}");
    let full_path = uploads_dir.join(format!("{id}-{file_name}"));

    let bytes = match base64::engine::general_purpose::STANDARD.decode(body.content.as_bytes()) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Json(UploadFileResponse {
                success: false,
                path: None,
                error: Some(err.to_string()),
            })
            .into_response();
        }
    };
    if let Err(err) = tokio::fs::write(full_path, bytes).await {
        return Json(UploadFileResponse {
            success: false,
            path: None,
            error: Some(err.to_string()),
        })
        .into_response();
    }

    Json(UploadFileResponse {
        success: true,
        path: Some(rel_path),
        error: None,
    })
    .into_response()
}

async fn handle_delete_upload(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DeleteUploadRequest>,
) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(DeleteUploadResponse {
                success: false,
                error: Some(err),
            })
            .into_response();
        }
    };
    if !body.path.starts_with(".codex_uploads/") {
        return Json(DeleteUploadResponse {
            success: false,
            error: Some("invalid upload path".to_string()),
        })
        .into_response();
    }
    let full = match safe_join(&cwd, &body.path) {
        Ok(full) => full,
        Err(err) => {
            return Json(DeleteUploadResponse {
                success: false,
                error: Some(err),
            })
            .into_response();
        }
    };
    let _ = tokio::fs::remove_file(full).await;
    Json(DeleteUploadResponse {
        success: true,
        error: None,
    })
    .into_response()
}

async fn handle_slash_commands(State(state): State<AppState>, Path(_id): Path<String>) -> Response {
    let prompts_dir = state.config.codex_home.join("prompts");
    let prompts = codex_core::custom_prompts::discover_prompts_in(&prompts_dir).await;
    let commands = custom_prompts_to_slash_commands(prompts);

    Json(SlashCommandsResponse {
        success: true,
        commands: Some(commands),
        error: None,
    })
    .into_response()
}

async fn handle_skills(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let cwd = match resolve_session_cwd(&state, &id).await {
        Ok(cwd) => cwd,
        Err(err) => {
            return Json(SkillsResponse {
                success: false,
                skills: None,
                error: Some(err),
            })
            .into_response();
        }
    };

    let outcome = state
        .thread_manager
        .skills_manager()
        .skills_for_cwd(&cwd, false)
        .await;
    let skills = skills_outcome_to_summaries(outcome);

    Json(SkillsResponse {
        success: true,
        skills: Some(skills),
        error: None,
    })
    .into_response()
}

async fn handle_push_vapid_key() -> Response {
    Json(PushVapidPublicKeyResponse {
        public_key: "".to_string(),
    })
    .into_response()
}

async fn handle_push_subscribe() -> Response {
    Json(serde_json::json!({})).into_response()
}

async fn handle_push_unsubscribe() -> Response {
    Json(serde_json::json!({})).into_response()
}

async fn handle_visibility(Json(_body): Json<JsonValue>) -> Response {
    Json(serde_json::json!({})).into_response()
}

async fn handle_voice_token() -> Response {
    Json(VoiceTokenResponse {
        allowed: false,
        token: None,
        agent_id: None,
        error: None,
    })
    .into_response()
}

async fn handle_terminal_ws(
    State(state): State<AppState>,
    Path((session_id, terminal_id)): Path<(String, String)>,
    ws: WebSocketUpgrade,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let token = query.get("token").cloned().unwrap_or_default();
    if token != state.token.as_str() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if !state.sessions.read().await.contains_key(&session_id) {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    }
    ws.on_upgrade(move |socket| terminal_ws_loop(state, socket, session_id, terminal_id))
}

async fn terminal_ws_loop(
    state: AppState,
    mut socket: WebSocket,
    session_id: String,
    _terminal_id: String,
) {
    let session = { state.sessions.read().await.get(&session_id).cloned() };
    let cwd = match session {
        Some(session) => session.state.read().await.cwd.clone(),
        None => PathBuf::from("."),
    };

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let args: Vec<String> = Vec::new();
    let env: HashMap<String, String> = std::env::vars().collect();
    let spawned = match codex_utils_pty::spawn_pty_process(
        &shell,
        &args,
        &cwd,
        &env,
        &None,
        codex_utils_pty::TerminalSize::default(),
    )
    .await
    {
        Ok(spawned) => spawned,
        Err(_) => return,
    };
    let session = spawned.session;
    let mut output_rx =
        codex_utils_pty::combine_output_receivers(spawned.stdout_rx, spawned.stderr_rx);
    let mut exit_rx = spawned.exit_rx;

    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    WsMessage::Text(text) => {
                        if let Ok(cmd) = serde_json::from_str::<TerminalClientMessage>(&text) {
                            match cmd {
                                TerminalClientMessage::Input { data } => {
                                    let _ = session.writer_sender().send(data.into_bytes()).await;
                                }
                                TerminalClientMessage::Resize { cols, rows } => {
                                    // codex-utils-pty intentionally does not expose a resize API.
                                    let _ = (cols, rows);
                                }
                            }
                        }
                    }
                    WsMessage::Binary(bytes) => {
                        let _ = session.writer_sender().send(bytes.to_vec()).await;
                    }
                    WsMessage::Close(_) => break,
                    _ => {}
                }
            }
            chunk = output_rx.recv() => {
                match chunk {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        let payload = serde_json::to_string(&TerminalServerMessage::Output { data: text }).unwrap_or_default();
                        if socket.send(WsMessage::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
            code = &mut exit_rx => {
                let code = code.unwrap_or(-1);
                let payload = serde_json::to_string(&TerminalServerMessage::Exit { code }).unwrap_or_default();
                let _ = socket.send(WsMessage::Text(payload.into())).await;
                break;
            }
        }
    }
    session.terminate();
}

#[derive(Debug, Serialize)]
struct SessionIndexEntry {
    id: ThreadId,
    thread_name: String,
    updated_at: String,
}

async fn persist_thread_name(codex_home: &FsPath, id: ThreadId, name: &str) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(codex_home.join("session_index.jsonl"))
        .await?;
    let entry = SessionIndexEntry {
        id,
        thread_name: name.to_string(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut line = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
    line.push('\n');
    use tokio::io::AsyncWriteExt as _;
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TerminalClientMessage {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TerminalServerMessage {
    Output { data: String },
    Exit { code: i32 },
}

async fn handle_static(State(state): State<AppState>, req: axum::http::Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };

    if let Some(dir) = state.static_dir.as_ref() {
        let mut safe = PathBuf::new();
        for component in FsPath::new(candidate).components() {
            match component {
                std::path::Component::Normal(part) => safe.push(part),
                _ => return StatusCode::NOT_FOUND.into_response(),
            }
        }

        let requested = dir.join(&safe);
        let (served_path, bytes) = match tokio::fs::read(&requested).await {
            Ok(bytes) => (requested, bytes),
            Err(_) => {
                let index = dir.join("index.html");
                let Ok(bytes) = tokio::fs::read(&index).await else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                (index, bytes)
            }
        };

        let mime = mime_guess::from_path(&served_path).first_or(mime::APPLICATION_OCTET_STREAM);
        let mut res = Response::new(Body::from(bytes));
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        );
        res.headers_mut().insert(
            header::HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        );
        res.headers_mut().insert(
            header::HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        return res;
    }

    let file = WEB_ASSETS
        .get_file(candidate)
        .or_else(|| WEB_ASSETS.get_file("index.html"));

    let Some(file) = file else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_guess::from_path(file.path()).first_or(mime::APPLICATION_OCTET_STREAM);
    let mut res = Response::new(Body::from(file.contents()));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    res.headers_mut().insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    res.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    res
}

async fn session_event_loop(state: AppState, session_id: String, session: Arc<ActiveSession>) {
    loop {
        let event = match session.thread.next_event().await {
            Ok(event) => event,
            Err(_) => break,
        };
        match event.msg {
            EventMsg::TurnStarted(_) => {
                let now = now_ms();
                {
                    let mut guard = session.state.write().await;
                    guard.thinking = true;
                    guard.thinking_at = now;
                    guard.updated_at = now;
                }
                let _ = state.events_tx.send(SyncEvent::SessionUpdated {
                    session_id: session_id.clone(),
                    data: None,
                });
            }
            EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_) => {
                let now = now_ms();
                {
                    let mut guard = session.state.write().await;
                    guard.thinking = false;
                    guard.thinking_at = now;
                    guard.updated_at = now;
                }
                let _ = state.events_tx.send(SyncEvent::SessionUpdated {
                    session_id: session_id.clone(),
                    data: None,
                });
            }
            EventMsg::AgentMessage(ev) => {
                let created_at = now_ms();
                let message = {
                    let mut guard = session.state.write().await;
                    let seq = guard.next_seq;
                    guard.next_seq += 1;
                    guard.updated_at = created_at;

                    let content = serde_json::json!({
                        "role": "agent",
                        "content": {
                            "type": "output",
                            "data": {
                                "type": "assistant",
                                "message": { "content": ev.message },
                            }
                        }
                    });
                    let message = WebDecryptedMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        seq: Some(seq),
                        local_id: None,
                        content,
                        created_at,
                        status: None,
                        original_text: None,
                    };
                    guard.messages.push(message.clone());
                    message
                };
                let _ = state.events_tx.send(SyncEvent::MessageReceived {
                    session_id: session_id.clone(),
                    message,
                });
            }
            EventMsg::ExecApprovalRequest(ev) => {
                let now = now_ms();
                let approval_id = ev.effective_approval_id();
                {
                    let mut guard = session.state.write().await;
                    let req = WebAgentRequest {
                        tool: "shell".to_string(),
                        arguments: serde_json::json!({
                            "kind": "exec",
                            "approvalId": approval_id,
                            "callId": ev.call_id,
                            "turnId": ev.turn_id,
                            "command": ev.command,
                            "cwd": ev.cwd.display().to_string(),
                            "reason": ev.reason,
                            "proposedExecpolicyAmendment": ev.proposed_execpolicy_amendment,
                        }),
                        created_at: Some(now),
                    };
                    guard
                        .agent_state
                        .requests
                        .get_or_insert_with(HashMap::new)
                        .insert(req_id_from_str(&approval_id), req);
                    guard.agent_state_version += 1;
                    guard.updated_at = now;
                }
                let _ = state.events_tx.send(SyncEvent::SessionUpdated {
                    session_id: session_id.clone(),
                    data: None,
                });
            }
            EventMsg::ApplyPatchApprovalRequest(ev) => {
                let now = now_ms();
                let call_id = ev.call_id.clone();
                {
                    let mut guard = session.state.write().await;
                    let req = WebAgentRequest {
                        tool: "apply_patch".to_string(),
                        arguments: serde_json::json!({
                            "kind": "patch",
                            "callId": call_id,
                            "turnId": ev.turn_id,
                            "reason": ev.reason,
                        }),
                        created_at: Some(now),
                    };
                    guard
                        .agent_state
                        .requests
                        .get_or_insert_with(HashMap::new)
                        .insert(req_id_from_str(&call_id), req);
                    guard.agent_state_version += 1;
                    guard.updated_at = now;
                }
                let _ = state.events_tx.send(SyncEvent::SessionUpdated {
                    session_id: session_id.clone(),
                    data: None,
                });
            }
            EventMsg::RequestUserInput(ev) => {
                let now = now_ms();
                let call_id = ev.call_id.clone();
                {
                    let mut guard = session.state.write().await;
                    let req = WebAgentRequest {
                        tool: "request_user_input".to_string(),
                        arguments: serde_json::json!({
                            "kind": "request_user_input",
                            "callId": call_id,
                            "turnId": ev.turn_id,
                            "questions": ev.questions,
                        }),
                        created_at: Some(now),
                    };
                    guard
                        .agent_state
                        .requests
                        .get_or_insert_with(HashMap::new)
                        .insert(req_id_from_str(&call_id), req);
                    guard.agent_state_version += 1;
                    guard.updated_at = now;
                }
                let _ = state.events_tx.send(SyncEvent::SessionUpdated {
                    session_id: session_id.clone(),
                    data: None,
                });
            }
            _ => {}
        }
    }
}

fn req_id_from_str(id: &str) -> String {
    id.to_string()
}

async fn build_session_json(session: &ActiveSession) -> Session {
    let guard = session.state.read().await;
    Session {
        id: session.thread_id.to_string(),
        namespace: "local".to_string(),
        seq: guard.next_seq.saturating_sub(1),
        created_at: guard.created_at,
        updated_at: guard.updated_at,
        active: guard.active,
        active_at: guard.active_at,
        metadata: Some(Metadata {
            path: guard.cwd.display().to_string(),
            host: "local".to_string(),
            name: guard.name.clone(),
            machine_id: Some("local".to_string()),
            tools: None,
            flavor: Some("codex".to_string()),
            summary: None,
        }),
        metadata_version: guard.metadata_version,
        agent_state: Some(guard.agent_state.clone()),
        agent_state_version: guard.agent_state_version,
        thinking: guard.thinking,
        thinking_at: guard.thinking_at,
        permission_mode: Some(guard.permission_mode.clone()),
        model_mode: Some(guard.model_mode.clone()),
    }
}

fn extract_reasoning_effort_from_history(history: &InitialHistory) -> Option<ReasoningEffort> {
    let items = match history {
        InitialHistory::New => return None,
        InitialHistory::Resumed(resumed) => &resumed.history,
        InitialHistory::Forked(items) => items,
    };

    items.iter().rev().find_map(|item| match item {
        RolloutItem::TurnContext(ctx) => ctx
            .collaboration_mode
            .as_ref()
            .and_then(CollaborationMode::reasoning_effort)
            .or(ctx.effort),
        _ => None,
    })
}

fn plan_mode_developer_instructions(masks: &[CollaborationModeMask]) -> Option<String> {
    masks
        .iter()
        .find(|mask| mask.mode == Some(ModeKind::Plan))
        .and_then(|mask| mask.developer_instructions.clone().flatten())
}

fn custom_prompts_to_slash_commands(prompts: Vec<CustomPrompt>) -> Vec<JsonValue> {
    prompts
        .into_iter()
        .map(|prompt| {
            serde_json::json!({
                "name": format!("{PROMPTS_CMD_PREFIX}:{}", prompt.name),
                "description": prompt.description,
                "source": "user",
                "content": prompt.content,
            })
        })
        .collect()
}

fn skills_outcome_to_summaries(outcome: SkillLoadOutcome) -> Vec<JsonValue> {
    let mut out = outcome
        .skills_with_enabled()
        .filter(|(_, enabled)| *enabled)
        .map(|(skill, _)| {
            let description = skill
                .short_description
                .as_ref()
                .filter(|desc| !desc.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| skill.description.clone());
            (
                skill.name.clone(),
                serde_json::json!({
                    "name": skill.name,
                    "description": description,
                }),
            )
        })
        .collect::<Vec<_>>();
    out.sort_by(|(a, _), (b, _)| a.cmp(b));
    out.into_iter().map(|(_, value)| value).collect()
}

fn permission_mode_to_policies(mode: &str) -> (AskForApproval, SandboxPolicy) {
    match mode {
        "read-only" => (
            AskForApproval::OnRequest,
            SandboxPolicy::new_read_only_policy(),
        ),
        "safe-yolo" => (
            AskForApproval::Never,
            SandboxPolicy::new_workspace_write_policy(),
        ),
        "yolo" | "bypassPermissions" => (AskForApproval::Never, SandboxPolicy::DangerFullAccess),
        _ => (
            AskForApproval::OnRequest,
            SandboxPolicy::new_workspace_write_policy(),
        ),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_rfc3339_ms(input: &str) -> Option<u64> {
    let dt = DateTime::parse_from_rfc3339(input).ok()?;
    Some(dt.timestamp_millis() as u64)
}

fn parse_repo_full_name_from_git_remote_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let path = if let Some((_, rest)) = input.split_once("://") {
        // scheme://[userinfo@]host[:port]/owner/repo(.git)
        let (_, rest) = rest.split_once('/')?;
        rest
    } else if let Some((_, rest)) = input.split_once('@') {
        // scp-like: user@host:owner/repo(.git)
        let (_, rest) = rest.rsplit_once(':')?;
        rest
    } else {
        return None;
    };

    let path = path.trim().trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/').filter(|p| !p.trim().is_empty());
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn is_valid_repo_full_name(input: &str) -> bool {
    let input = input.trim();
    let Some((owner, repo)) = input.split_once('/') else {
        return false;
    };
    if owner.trim().is_empty() || repo.trim().is_empty() {
        return false;
    }
    if owner.contains(' ') || repo.contains(' ') {
        return false;
    }
    !repo.contains('/')
}

async fn resolve_github_repos_for_kanban(
    config_toml: &codex_core::config::ConfigToml,
    config_cwd: &AbsolutePathBuf,
) -> Vec<String> {
    let allow_repos = config_toml
        .github_webhook
        .as_ref()
        .and_then(|cfg| cfg.allow_repos.clone())
        .unwrap_or_default();
    if !allow_repos.is_empty() {
        return allow_repos;
    }

    let git_info = collect_git_info(config_cwd.as_path()).await;
    let Some(remote_url) = git_info.and_then(|info| info.repository_url) else {
        return Vec::new();
    };
    let Some(repo) = parse_repo_full_name_from_git_remote_url(&remote_url) else {
        warn!("unable to parse repo from git remote: {remote_url}");
        return Vec::new();
    };
    vec![repo]
}

fn github_work_item_key(repo: &str, number: u64, kind: &str) -> String {
    format!("{repo}#{number}:{kind}")
}

fn parse_github_work_item_key(key: &str) -> Option<(String, u64, String)> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let (repo, rest) = key.split_once('#')?;
    let (number, kind) = rest.split_once(':')?;
    let repo = repo.trim();
    let kind = kind.trim().to_ascii_lowercase();
    if repo.is_empty() || kind.is_empty() {
        return None;
    }
    let number = number.trim().parse::<u64>().ok()?;
    Some((repo.to_string(), number, kind))
}

fn convert_work_item(raw: GithubRepoWorkItemRaw) -> GithubWorkItem {
    GithubWorkItem {
        work_item_key: github_work_item_key(&raw.repo, raw.number, &raw.kind),
        repo: raw.repo,
        kind: raw.kind,
        number: raw.number,
        title: raw.title,
        state: raw.state,
        url: raw.url,
        updated_at: parse_rfc3339_ms(&raw.updated_at).unwrap_or(0),
        labels: raw
            .labels
            .into_iter()
            .map(|l| GithubLabel {
                name: l.name,
                color: l.color,
            })
            .collect(),
        comments: raw.comments,
    }
}

async fn load_github_work_items_snapshot(codex_home: &FsPath) -> GithubWorkItemsSnapshot {
    let path = codex_home.join(GITHUB_WORK_ITEMS_FILE_NAME);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return GithubWorkItemsSnapshot::default();
        }
        Err(err) => {
            warn!("failed to read {GITHUB_WORK_ITEMS_FILE_NAME}: {err}");
            return GithubWorkItemsSnapshot::default();
        }
    };
    match serde_json::from_slice::<GithubWorkItemsSnapshot>(&content) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            warn!("failed to parse {GITHUB_WORK_ITEMS_FILE_NAME}: {err}");
            GithubWorkItemsSnapshot::default()
        }
    }
}

async fn persist_github_work_items_snapshot(
    codex_home: &FsPath,
    snapshot: &GithubWorkItemsSnapshot,
) {
    if let Err(err) = tokio::fs::create_dir_all(codex_home).await {
        warn!("failed to create codex home dir for {GITHUB_WORK_ITEMS_FILE_NAME}: {err}");
        return;
    }
    let path = codex_home.join(GITHUB_WORK_ITEMS_FILE_NAME);
    let tmp_path = path.with_extension("json.tmp");
    let mut body = match serde_json::to_vec_pretty(snapshot) {
        Ok(body) => body,
        Err(err) => {
            warn!("failed to serialize {GITHUB_WORK_ITEMS_FILE_NAME}: {err}");
            return;
        }
    };
    body.push(b'\n');
    if let Err(err) = tokio::fs::write(&tmp_path, body).await {
        warn!("failed to write {GITHUB_WORK_ITEMS_FILE_NAME} tmp: {err}");
        return;
    }
    if let Err(_err) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        if let Err(err) = tokio::fs::rename(&tmp_path, &path).await {
            warn!("failed to persist {GITHUB_WORK_ITEMS_FILE_NAME}: {err}");
        }
    }
}

async fn load_github_jobs(codex_home: &FsPath) -> HashMap<String, GithubJob> {
    let path = codex_home.join(GITHUB_JOBS_FILE_NAME);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(err) => {
            warn!("failed to read {GITHUB_JOBS_FILE_NAME}: {err}");
            return HashMap::new();
        }
    };
    match serde_json::from_slice::<Vec<GithubJob>>(&content) {
        Ok(list) => list.into_iter().map(|j| (j.job_id.clone(), j)).collect(),
        Err(err) => {
            warn!("failed to parse {GITHUB_JOBS_FILE_NAME}: {err}");
            HashMap::new()
        }
    }
}

async fn persist_github_jobs(codex_home: &FsPath, jobs: &HashMap<String, GithubJob>) {
    if let Err(err) = tokio::fs::create_dir_all(codex_home).await {
        warn!("failed to create codex home dir for {GITHUB_JOBS_FILE_NAME}: {err}");
        return;
    }
    let mut list: Vec<GithubJob> = jobs.values().cloned().collect();
    list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let path = codex_home.join(GITHUB_JOBS_FILE_NAME);
    let tmp_path = path.with_extension("json.tmp");
    let mut body = match serde_json::to_vec_pretty(&list) {
        Ok(body) => body,
        Err(err) => {
            warn!("failed to serialize {GITHUB_JOBS_FILE_NAME}: {err}");
            return;
        }
    };
    body.push(b'\n');
    if let Err(err) = tokio::fs::write(&tmp_path, body).await {
        warn!("failed to write {GITHUB_JOBS_FILE_NAME} tmp: {err}");
        return;
    }
    if let Err(_err) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        if let Err(err) = tokio::fs::rename(&tmp_path, &path).await {
            warn!("failed to persist {GITHUB_JOBS_FILE_NAME}: {err}");
        }
    }
}

async fn get_workspace_kanban_lock(state: &AppState, workspace_id: &str) -> Arc<Mutex<()>> {
    let locks = state.workspace_kanban_locks.read().await;
    if let Some(lock) = locks.get(workspace_id) {
        return Arc::clone(lock);
    }
    drop(locks);
    let mut locks = state.workspace_kanban_locks.write().await;
    let lock = locks
        .entry(workspace_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())));
    Arc::clone(lock)
}

fn workspace_storage_dir(codex_home: &FsPath, workspace_id: &str) -> anyhow::Result<PathBuf> {
    if uuid::Uuid::parse_str(workspace_id).is_err() {
        anyhow::bail!("invalid workspace id format");
    }
    Ok(codex_home.join("workspaces").join(workspace_id))
}

async fn load_workspace_work_items_snapshot(
    codex_home: &FsPath,
    workspace_id: &str,
) -> GithubWorkItemsSnapshot {
    let dir = match workspace_storage_dir(codex_home, workspace_id) {
        Ok(dir) => dir,
        Err(_) => return GithubWorkItemsSnapshot::default(),
    };
    let path = dir.join(WORKSPACE_WORK_ITEMS_FILE_NAME);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return GithubWorkItemsSnapshot::default();
        }
        Err(err) => {
            warn!("failed to read workspace work items snapshot for {workspace_id}: {err}");
            return GithubWorkItemsSnapshot::default();
        }
    };
    match serde_json::from_slice::<GithubWorkItemsSnapshot>(&content) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            warn!("failed to parse workspace work items snapshot for {workspace_id}: {err}");
            GithubWorkItemsSnapshot::default()
        }
    }
}

async fn persist_workspace_work_items_snapshot(
    codex_home: &FsPath,
    workspace_id: &str,
    snapshot: &GithubWorkItemsSnapshot,
) {
    let dir = match workspace_storage_dir(codex_home, workspace_id) {
        Ok(dir) => dir,
        Err(err) => {
            warn!("skipping persist workspace work items for {workspace_id}: {err}");
            return;
        }
    };
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        warn!("failed to create workspace dir for {workspace_id}: {err}");
        return;
    }
    let path = dir.join(WORKSPACE_WORK_ITEMS_FILE_NAME);
    let tmp_path = path.with_extension("json.tmp");
    let mut body = match serde_json::to_vec_pretty(snapshot) {
        Ok(body) => body,
        Err(err) => {
            warn!("failed to serialize workspace work items snapshot for {workspace_id}: {err}");
            return;
        }
    };
    body.push(b'\n');
    if let Err(err) = tokio::fs::write(&tmp_path, body).await {
        warn!("failed to write workspace work items snapshot tmp for {workspace_id}: {err}");
        return;
    }
    if let Err(_err) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        if let Err(err) = tokio::fs::rename(&tmp_path, &path).await {
            warn!("failed to persist workspace work items snapshot for {workspace_id}: {err}");
        }
    }
}

fn workspace_default_kanban(board: &workspace::BoardConfig) -> kanban::KanbanConfig {
    let mut columns = board
        .columns
        .iter()
        .map(|col| kanban::KanbanColumn {
            id: col.id.clone(),
            name: col.name.clone(),
            position: col.position,
        })
        .collect::<Vec<_>>();
    columns.sort_by(|a, b| a.position.cmp(&b.position));
    kanban::KanbanConfig {
        columns,
        card_positions: HashMap::new(),
        card_settings: HashMap::new(),
    }
}

async fn load_or_init_workspace_kanban(
    codex_home: &FsPath,
    workspace_id: &str,
    board: &workspace::BoardConfig,
) -> kanban::KanbanConfig {
    let dir = match workspace_storage_dir(codex_home, workspace_id) {
        Ok(dir) => dir,
        Err(_) => return workspace_default_kanban(board),
    };
    let path = dir.join(WORKSPACE_KANBAN_FILE_NAME);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let cfg = workspace_default_kanban(board);
            persist_workspace_kanban(codex_home, workspace_id, &cfg).await;
            return cfg;
        }
        Err(err) => {
            warn!("failed to read workspace kanban for {workspace_id}: {err}");
            return workspace_default_kanban(board);
        }
    };

    match serde_json::from_slice::<kanban::KanbanConfig>(&content) {
        Ok(mut cfg) => {
            let desired_columns = workspace_default_kanban(board).columns;
            let columns_match = cfg.columns.len() == desired_columns.len()
                && cfg
                    .columns
                    .iter()
                    .zip(desired_columns.iter())
                    .all(|(a, b)| a.id == b.id && a.name == b.name && a.position == b.position);

            let mut changed = false;
            if cfg.columns.is_empty() || !columns_match {
                cfg.columns = desired_columns;
                changed = true;
            }

            let column_ids = cfg
                .columns
                .iter()
                .map(|col| col.id.clone())
                .collect::<HashSet<_>>();
            if let Some(first_col) = cfg
                .columns
                .iter()
                .min_by_key(|c| c.position)
                .map(|c| c.id.as_str())
            {
                for pos in cfg.card_positions.values_mut() {
                    if !column_ids.contains(&pos.column_id) {
                        pos.column_id = first_col.to_string();
                        changed = true;
                    }
                }
            }

            if changed {
                persist_workspace_kanban(codex_home, workspace_id, &cfg).await;
            }
            cfg
        }
        Err(err) => {
            warn!("failed to parse workspace kanban for {workspace_id}, using default: {err}");
            workspace_default_kanban(board)
        }
    }
}

async fn persist_workspace_kanban(
    codex_home: &FsPath,
    workspace_id: &str,
    cfg: &kanban::KanbanConfig,
) {
    let dir = match workspace_storage_dir(codex_home, workspace_id) {
        Ok(dir) => dir,
        Err(err) => {
            warn!("skipping persist workspace kanban for {workspace_id}: {err}");
            return;
        }
    };
    kanban::persist_to(&dir, WORKSPACE_KANBAN_FILE_NAME, cfg).await;
}

async fn sync_github_work_items(state: &AppState) -> anyhow::Result<()> {
    let Some(webhook) = state.github_webhook.as_ref() else {
        return Ok(());
    };
    let repos = state.github_repos.read().await.clone();
    if repos.is_empty() {
        return Ok(());
    }

    let mut all = Vec::new();
    for repo in repos.iter() {
        let items = webhook.list_repo_work_items(repo).await?;
        all.extend(items);
    }

    let mut by_key: HashMap<String, GithubWorkItem> = HashMap::new();
    for raw in all {
        let item = convert_work_item(raw);
        by_key.insert(item.work_item_key.clone(), item);
    }
    let mut items: Vec<GithubWorkItem> = by_key.into_values().collect();
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let snapshot = GithubWorkItemsSnapshot {
        fetched_at: now_ms(),
        items,
    };

    *state.github_work_items.write().await = snapshot.clone();
    persist_github_work_items_snapshot(&state.config.codex_home, &snapshot).await;
    let _ = state.events_tx.send(SyncEvent::GithubWorkItemsUpdated);
    Ok(())
}

async fn sync_workspace_work_items(state: &AppState, workspace_id: &str) -> anyhow::Result<()> {
    let Some(webhook) = state.github_webhook.as_ref() else {
        return Ok(());
    };
    let Some(workspace) = state.workspaces.read().await.get(workspace_id) else {
        return Ok(());
    };
    if workspace.repos.is_empty() {
        return Ok(());
    }

    let mut all = Vec::new();
    for repo in &workspace.repos {
        let items = webhook.list_repo_work_items(&repo.full_name).await?;
        all.extend(items);
    }

    let mut by_key: HashMap<String, GithubWorkItem> = HashMap::new();
    for raw in all {
        let item = convert_work_item(raw);
        by_key.insert(item.work_item_key.clone(), item);
    }
    let mut items: Vec<GithubWorkItem> = by_key.into_values().collect();
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let snapshot = GithubWorkItemsSnapshot {
        fetched_at: now_ms(),
        items,
    };

    persist_workspace_work_items_snapshot(&state.config.codex_home, workspace_id, &snapshot).await;
    Ok(())
}

async fn github_sync_loop(state: AppState) {
    {
        let _guard = state.github_sync_lock.lock().await;
        if let Err(err) = sync_github_work_items(&state).await {
            warn!("github sync failed: {err:#}");
        }
        sync_all_workspace_work_items(&state).await;
    }

    let mut interval = tokio::time::interval(GITHUB_SYNC_INTERVAL);
    interval.tick().await;
    loop {
        interval.tick().await;
        let _guard = state.github_sync_lock.lock().await;
        if let Err(err) = sync_github_work_items(&state).await {
            warn!("github sync failed: {err:#}");
        }
        sync_all_workspace_work_items(&state).await;
    }
}

async fn sync_all_workspace_work_items(state: &AppState) {
    let workspace_ids: Vec<String> = state
        .workspaces
        .read()
        .await
        .list()
        .iter()
        .map(|s| s.id.clone())
        .collect();
    for ws_id in workspace_ids {
        if let Err(err) = sync_workspace_work_items(state, &ws_id).await {
            warn!("workspace sync failed for {ws_id}: {err:#}");
        }
    }
}

async fn enqueue_github_job(
    state: &AppState,
    work_item_key: &str,
    run_settings: kanban::KanbanCardSettings,
) -> anyhow::Result<()> {
    let work_item_key = work_item_key.trim().to_string();
    let Some(webhook) = state.github_webhook.clone() else {
        anyhow::bail!("github not enabled");
    };
    let Some((repo, number, kind)) = parse_github_work_item_key(&work_item_key) else {
        anyhow::bail!("invalid work item key: {work_item_key}");
    };

    let snapshot = state.github_work_items.read().await;
    let Some(item) = snapshot
        .items
        .iter()
        .find(|i| i.work_item_key == work_item_key)
    else {
        return Ok(());
    };
    if !item.state.eq_ignore_ascii_case("open") {
        return Ok(());
    }
    let title = item.title.clone();
    drop(snapshot);

    enqueue_github_job_inner(state, webhook, repo, number, kind, &title, run_settings).await
}

async fn enqueue_workspace_github_job(
    state: &AppState,
    workspace_id: &str,
    work_item_key: &str,
    run_settings: kanban::KanbanCardSettings,
) -> anyhow::Result<()> {
    // Validate workspace_id early so callers get an error
    workspace_storage_dir(&state.config.codex_home, workspace_id)?;

    let work_item_key = work_item_key.trim().to_string();
    let Some(webhook) = state.github_webhook.clone() else {
        anyhow::bail!("github not enabled");
    };
    let Some((repo, number, kind)) = parse_github_work_item_key(&work_item_key) else {
        anyhow::bail!("invalid work item key: {work_item_key}");
    };

    // Merge workspace default_exec with card-level run_settings (card wins)
    let (default_model, default_reasoning_effort, default_prompt) = {
        let workspaces = state.workspaces.read().await;
        match workspaces.get(workspace_id) {
            Some(workspace) => (
                workspace.default_exec.model,
                workspace.default_exec.reasoning_effort,
                workspace.default_exec.prompt,
            ),
            None => (None, None, None),
        }
    };
    let merged_settings = kanban::KanbanCardSettings {
        model: run_settings.model.or(default_model),
        reasoning_effort: run_settings.reasoning_effort.or(default_reasoning_effort),
        prompt_prefix: run_settings.prompt_prefix.or(default_prompt),
    };

    let snapshot = load_workspace_work_items_snapshot(&state.config.codex_home, workspace_id).await;
    let Some(item) = snapshot
        .items
        .iter()
        .find(|i| i.work_item_key == work_item_key)
    else {
        return Ok(());
    };
    if !item.state.eq_ignore_ascii_case("open") {
        return Ok(());
    }
    let title = item.title.clone();

    enqueue_github_job_inner(state, webhook, repo, number, kind, &title, merged_settings).await
}

async fn enqueue_github_job_inner(
    state: &AppState,
    webhook: GithubWebhook,
    repo: String,
    number: u64,
    kind: String,
    title: &str,
    run_settings: kanban::KanbanCardSettings,
) -> anyhow::Result<()> {
    let base_prompt = if title.is_empty() {
        format!("Work on {repo} {kind} #{number}.")
    } else {
        format!("Work on {repo} {kind} #{number}: {title}")
    };
    let prompt = run_settings
        .prompt_prefix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|prefix| format!("{prefix}\n\n{base_prompt}"))
        .unwrap_or(base_prompt);

    let work_item_key = github_work_item_key(&repo, number, &kind);
    let job_id = uuid::Uuid::new_v4().to_string();
    let created_at = now_ms();
    let log_rel = format!("{GITHUB_JOB_LOGS_DIR}/{job_id}.log");
    let log_path = safe_join(&state.config.codex_home, &log_rel).ok();
    {
        let mut jobs = state.github_jobs.write().await;
        jobs.insert(
            job_id.clone(),
            GithubJob {
                job_id: job_id.clone(),
                work_item_key: work_item_key.clone(),
                status: "queued".to_string(),
                created_at,
                started_at: None,
                finished_at: None,
                last_error: None,
                result_summary: None,
                thread_id: None,
                log_path: Some(log_rel),
            },
        );
        persist_github_jobs(&state.config.codex_home, &jobs).await;
    }
    let _ = state.events_tx.send(SyncEvent::GithubJobUpdated {
        job_id: job_id.clone(),
        work_item_key: work_item_key.clone(),
        status: "queued".to_string(),
    });

    let jobs = Arc::clone(&state.github_jobs);
    let events_tx = state.events_tx.clone();
    let codex_home = state.config.codex_home.clone();
    let work_item_key = work_item_key.clone();
    let model = run_settings.model.clone();
    let reasoning_effort = run_settings
        .reasoning_effort
        .map(|effort| effort.to_string());
    tokio::spawn(async move {
        {
            let mut map = jobs.write().await;
            if let Some(job) = map.get_mut(&job_id) {
                job.status = "running".to_string();
                job.started_at = Some(now_ms());
            }
            persist_github_jobs(&codex_home, &map).await;
        }
        let _ = events_tx.send(SyncEvent::GithubJobUpdated {
            job_id: job_id.clone(),
            work_item_key: work_item_key.clone(),
            status: "running".to_string(),
        });

        let overrides = GithubCodexRunOverrides {
            model,
            reasoning_effort,
        };
        let result: anyhow::Result<GithubCodexJobOutput> = webhook
            .run_codex_for_work_item(&repo, &kind, number, prompt, overrides, log_path)
            .await;

        {
            let mut map = jobs.write().await;
            if let Some(job) = map.get_mut(&job_id) {
                job.finished_at = Some(now_ms());
                match result {
                    Ok(output) => {
                        job.status = "succeeded".to_string();
                        job.thread_id = output.thread_id.clone();
                        job.result_summary = Some(truncate_summary(&output.last_message, 280));
                    }
                    Err(err) => {
                        job.status = "failed".to_string();
                        job.last_error = Some(truncate_summary(&format!("{err:#}"), 400));
                    }
                }
            }
            persist_github_jobs(&codex_home, &map).await;
        }

        let final_status = {
            let map = jobs.read().await;
            map.get(&job_id)
                .map(|j| j.status.clone())
                .unwrap_or_else(|| "unknown".to_string())
        };
        let _ = events_tx.send(SyncEvent::GithubJobUpdated {
            job_id,
            work_item_key: work_item_key.clone(),
            status: final_status,
        });
    });

    Ok(())
}

async fn handle_models_catalog(State(state): State<AppState>) -> Response {
    let models = state
        .thread_manager
        .list_models(RefreshStrategy::Offline)
        .await
        .into_iter()
        .map(|preset| ModelCatalogModel {
            id: preset.model,
            display_name: preset.display_name,
            description: preset.description,
            is_default: preset.is_default,
            show_in_picker: preset.show_in_picker,
            default_reasoning_effort: preset.default_reasoning_effort,
            supported_reasoning_efforts: preset.supported_reasoning_efforts,
        })
        .collect();
    Json(ModelsCatalogResponse { models }).into_response()
}

fn truncate_summary(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut out = s
        .chars()
        .take(max_len.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn safe_join(root: &FsPath, rel: &str) -> Result<PathBuf, String> {
    let rel_path = FsPath::new(rel);
    let mut out = root.to_path_buf();
    for component in rel_path.components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err("invalid path".to_string());
            }
        }
    }
    Ok(out)
}

fn normalize_optional_text(input: Option<&str>) -> Option<String> {
    let s = input.unwrap_or_default().trim();
    if s.is_empty() {
        return None;
    }
    Some(s.to_string())
}

fn normalize_github_repos(repos: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in repos {
        let repo = raw.trim().trim_matches('/').to_string();
        if repo.is_empty() {
            continue;
        }
        if seen.insert(repo.clone()) {
            out.push(repo);
        }
    }
    out
}

async fn load_github_repos(codex_home: &FsPath) -> Vec<String> {
    let path = codex_home.join(GITHUB_REPOS_FILE_NAME);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            warn!("failed to read {GITHUB_REPOS_FILE_NAME}: {err}");
            return Vec::new();
        }
    };
    match serde_json::from_slice::<Vec<String>>(&content) {
        Ok(repos) => normalize_github_repos(repos),
        Err(err) => {
            warn!("failed to parse {GITHUB_REPOS_FILE_NAME}: {err}");
            Vec::new()
        }
    }
}

async fn persist_github_repos(codex_home: &FsPath, repos: &[String]) {
    if let Err(err) = tokio::fs::create_dir_all(codex_home).await {
        warn!("failed to create codex home dir for {GITHUB_REPOS_FILE_NAME}: {err}");
        return;
    }
    let path = codex_home.join(GITHUB_REPOS_FILE_NAME);
    let tmp_path = path.with_extension("json.tmp");
    let mut body = match serde_json::to_vec_pretty(repos) {
        Ok(body) => body,
        Err(err) => {
            warn!("failed to serialize {GITHUB_REPOS_FILE_NAME}: {err}");
            return;
        }
    };
    body.push(b'\n');
    if let Err(err) = tokio::fs::write(&tmp_path, body).await {
        warn!("failed to write {GITHUB_REPOS_FILE_NAME} tmp: {err}");
        return;
    }
    if let Err(_err) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        if let Err(err) = tokio::fs::rename(&tmp_path, &path).await {
            warn!("failed to persist {GITHUB_REPOS_FILE_NAME}: {err}");
        }
    }
}

async fn read_tail_file(path: &FsPath, max_bytes: u64) -> anyhow::Result<(String, bool)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let meta = file
        .metadata()
        .await
        .with_context(|| format!("stat {}", path.display()))?;
    let len = meta.len();
    let truncated = len > max_bytes;
    let start = len.saturating_sub(max_bytes);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .with_context(|| format!("seek {}", path.display()))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok((text, truncated))
}

async fn resolve_session_cwd(state: &AppState, session_id: &str) -> Result<PathBuf, String> {
    if let Some(session) = state.sessions.read().await.get(session_id).cloned() {
        return Ok(session.state.read().await.cwd.clone());
    }
    let Some(path) = codex_core::find_thread_path_by_id_str(&state.config.codex_home, session_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("session not found".to_string());
    };
    let history = codex_core::RolloutRecorder::get_rollout_history(&path)
        .await
        .map_err(|e| e.to_string())?;
    history
        .session_cwd()
        .ok_or_else(|| "missing session cwd".to_string())
}

async fn load_messages_from_rollout(
    state: &AppState,
    session_id: &str,
) -> Option<Vec<WebDecryptedMessage>> {
    let path = codex_core::find_thread_path_by_id_str(&state.config.codex_home, session_id)
        .await
        .ok()
        .flatten()?;
    let history = codex_core::RolloutRecorder::get_rollout_history(&path)
        .await
        .ok()?;
    let items = history.get_rollout_items();
    let mut out = Vec::new();
    let mut seq: u64 = 1;

    for item in items {
        let codex_protocol::protocol::RolloutItem::ResponseItem(item) = item else {
            continue;
        };
        let codex_protocol::models::ResponseItem::Message { role, content, .. } = item else {
            continue;
        };
        let text = codex_core::content_items_to_text(&content);
        let (wrapper_role, wrapper_content) = if role == "user" {
            ("user", serde_json::json!({ "type": "text", "text": text }))
        } else if role == "assistant" {
            (
                "agent",
                serde_json::json!({
                    "type":"output",
                    "data": { "type":"assistant", "message": { "content": text } }
                }),
            )
        } else {
            continue;
        };

        let msg = WebDecryptedMessage {
            id: uuid::Uuid::new_v4().to_string(),
            seq: Some(seq),
            local_id: None,
            content: serde_json::json!({ "role": wrapper_role, "content": wrapper_content }),
            created_at: now_ms(),
            status: None,
            original_text: None,
        };
        seq += 1;
        out.push(msg);
    }
    Some(out)
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn json_error(code: &str) -> JsonValue {
    serde_json::json!({ "error": code })
}
