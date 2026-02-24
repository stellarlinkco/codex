use crate::Cli;
use anyhow::Context;
use anyhow::bail;
use axum::Json;
use axum::Router;
use axum::body::Body;
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
use base64::Engine;
use chrono::DateTime;
use codex_core::AuthManager;
use codex_core::CodexThread;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::config::ConfigOverrides;
use codex_core::skills::SkillLoadOutcome;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::Settings;
use codex_protocol::custom_prompts::CustomPrompt;
use codex_protocol::custom_prompts::PROMPTS_CMD_PREFIX;
use codex_protocol::openai_models::ReasoningEffort;
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
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::warn;

static WEB_ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/web");

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
    use super::custom_prompts_to_slash_commands;
    use super::extract_reasoning_effort_from_history;
    use super::handle_machine_spawn;
    use super::handle_post_message;
    use super::handle_resume_session;
    use super::handle_skills;
    use super::handle_slash_commands;
    use super::plan_mode_developer_instructions;
    use super::safe_join;
    use super::skills_outcome_to_summaries;
    use axum::Json;
    use axum::extract::Path;
    use axum::extract::State;
    use axum::http::StatusCode;
    use codex_core::AuthManager;
    use codex_core::ThreadManager;
    use codex_core::config::Config;
    use codex_core::config::ConfigOverrides;
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

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &PathBuf) -> Self {
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
            permissions: None,
            path: PathBuf::from("/tmp/a/SKILL.md"),
            scope: SkillScope::User,
        };
        let disabled_skill = SkillMetadata {
            name: "b".to_string(),
            description: "B".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permissions: None,
            path: PathBuf::from("/tmp/b/SKILL.md"),
            scope: SkillScope::User,
        };
        let mut disabled_paths = HashSet::new();
        disabled_paths.insert(disabled_skill.path.clone());
        let outcome = SkillLoadOutcome {
            skills: vec![enabled_skill.clone(), disabled_skill],
            errors: Vec::new(),
            disabled_paths,
        };

        let skills = skills_outcome_to_summaries(outcome);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], enabled_skill.name);
    }

    #[test]
    fn extract_reasoning_effort_prefers_collaboration_mode_over_effort_field() {
        let ctx = TurnContextItem {
            turn_id: None,
            cwd: PathBuf::from("/tmp"),
            approval_policy: AskForApproval::OnRequest,
            sandbox_policy: SandboxPolicy::new_workspace_write_policy(),
            network: None,
            model: "gpt-5.2".to_string(),
            personality: None,
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Default,
                settings: Settings {
                    model: "gpt-5.2".to_string(),
                    reasoning_effort: Some(ReasoningEffort::High),
                    developer_instructions: None,
                },
            }),
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
        let _lock = ENV_LOCK.lock().expect("lock env");
        codex_core::test_support::set_thread_manager_test_mode(true);

        let codex_home = temp_dir("codex-home");
        let _env = EnvVarGuard::set("CODEX_HOME", &codex_home);

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
        ));
        let (events_tx, _) = broadcast::channel(64);

        let state = AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
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
        for _ in 0..50 {
            if tokio::fs::try_exists(&rollout_path).await.unwrap_or(false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let history = codex_core::RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .expect("load rollout history");
        assert_eq!(
            extract_reasoning_effort_from_history(&history),
            Some(ReasoningEffort::High)
        );

        let _ = session.thread.submit(Op::Shutdown).await;
        state.sessions.write().await.remove(&session_id);

        let resp = handle_resume_session(State(state.clone()), Path(session_id.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(state.sessions.read().await.contains_key(&session_id));

        let _ = state.thread_manager.remove_and_close_all_threads().await;
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
    let state = AppState {
        token: Arc::new(token.clone()),
        static_dir,
        config: Arc::new(config),
        cli_overrides,
        base_overrides,
        auth_manager,
        thread_manager,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        events_tx,
    };

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
    }
}

fn sse_json(event: &SyncEvent) -> SseEvent {
    let Ok(data) = serde_json::to_string(event) else {
        return SseEvent::default().data("{\"type\":\"toast\",\"data\":{\"title\":\"Serialize error\",\"body\":\"\",\"sessionId\":\"\",\"url\":\"\"}}");
    };
    SseEvent::default().data(data)
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
    let _ = state
        .events_tx
        .send(SyncEvent::SessionRemoved { session_id: id });
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

    let _ = state
        .events_tx
        .send(SyncEvent::SessionRemoved { session_id: id });
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
            summary: ReasoningSummaryConfig::Auto,
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
    let spawned = match codex_utils_pty::spawn_pty_process(&shell, &args, &cwd, &env, &None).await {
        Ok(spawned) => spawned,
        Err(_) => return,
    };
    let session = spawned.session;
    let mut output_rx = spawned.output_rx;
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
                                    let _ = session.resize(cols, rows);
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
