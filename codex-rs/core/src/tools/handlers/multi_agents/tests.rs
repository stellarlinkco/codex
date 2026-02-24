use super::*;
use crate::AuthManager;
use crate::CodexAuth;
use crate::ThreadManager;
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
use codex_hooks::CommandHookConfig;
use codex_hooks::CommandHooksConfig;
use codex_hooks::Hooks;
use codex_hooks::HooksConfig;
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
use tokio::task::JoinSet;
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

fn unwrap_arc<T>(arc: Arc<T>, msg: &str) -> T {
    match Arc::try_unwrap(arc) {
        Ok(value) => value,
        Err(_) => panic!("{msg}"),
    }
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
        FunctionCallError::RespondToModel("Empty message can't be sent to an agent".to_string())
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
    assert_eq!(snapshot.reasoning_effort, None);
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
async fn spawn_agent_dispatches_subagent_start_hook() {
    #[derive(Debug, Deserialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let cwd = tempfile::tempdir().expect("temp dir");
    turn.cwd = cwd.path().to_path_buf();

    let marker_path = turn.config.codex_home.join("subagent_start.log");
    let script = r#"import sys, json; data=json.load(sys.stdin); open(sys.argv[1], "a").write(data["hook_event_name"] + "\n")"#;
    session.services.hooks = Hooks::new(HooksConfig {
        command_hooks: CommandHooksConfig {
            subagent_start: vec![CommandHookConfig {
                command: vec![
                    "python3".to_string(),
                    "-c".to_string(),
                    script.to_string(),
                    marker_path.to_string_lossy().into_owned(),
                ],
                ..Default::default()
            }],
            ..Default::default()
        },
    });

    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "spawn_agent",
        function_payload(json!({
            "message": "inspect this repo"
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

    let hook_events = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("subagent_start hook should write marker");
    assert_eq!(hook_events.trim(), "SubagentStart");

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
        depth: turn.config.agent_max_depth,
        agent_nickname: None,
        agent_role: None,
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
        FunctionCallError::RespondToModel("Empty message can't be sent to an agent".to_string())
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
        depth: turn.config.agent_max_depth,
        agent_nickname: None,
        agent_role: None,
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
    let result: WaitResult = serde_json::from_str(&content).expect("wait result should be json");
    assert_eq!(
        result,
        WaitResult {
            status: HashMap::from([(id_a, AgentStatus::NotFound), (id_b, AgentStatus::NotFound),]),
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
    let result: WaitResult = serde_json::from_str(&content).expect("wait result should be json");
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
    let result: WaitResult = serde_json::from_str(&content).expect("wait result should be json");
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

#[tokio::test]
async fn close_agent_releases_slot_for_already_shutdown_agent() {
    #[derive(Debug, Deserialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let mut config = (*turn.config).clone();
    config.agent_max_threads = Some(1);
    turn.config = Arc::new(config);

    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "hello"})),
    );
    let spawn_output = MultiAgentHandler
        .handle(spawn_invocation)
        .await
        .expect("spawn_agent should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(spawn_content),
        success: spawn_success,
        ..
    } = spawn_output
    else {
        panic!("expected function output");
    };
    assert_eq!(spawn_success, Some(true));
    let spawn_result: SpawnAgentResult =
        serde_json::from_str(&spawn_content).expect("spawn_agent result should be json");
    let agent_thread_id = agent_id(&spawn_result.agent_id).expect("valid agent id");

    let thread = manager
        .get_thread(agent_thread_id)
        .await
        .expect("spawned agent should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                manager.agent_control().get_status(agent_thread_id).await,
                AgentStatus::Shutdown
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent should reach shutdown");

    let blocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "blocked"})),
    );
    let Err(err) = MultiAgentHandler.handle(blocked_invocation).await else {
        panic!("spawn_agent should fail when max threads already reached");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "collab spawn failed: agent thread limit reached (max 1)".to_string()
        )
    );

    let close_invocation = invocation(
        session.clone(),
        turn.clone(),
        "close_agent",
        function_payload(json!({"id": spawn_result.agent_id})),
    );
    MultiAgentHandler
        .handle(close_invocation)
        .await
        .expect("close_agent should succeed");

    let unblocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let unblocked_output = MultiAgentHandler
        .handle(unblocked_invocation)
        .await
        .expect("spawn_agent should succeed after close_agent releases slot");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(unblocked_content),
        success: unblocked_success,
        ..
    } = unblocked_output
    else {
        panic!("expected function output");
    };
    assert_eq!(unblocked_success, Some(true));
    let unblocked_result: SpawnAgentResult =
        serde_json::from_str(&unblocked_content).expect("spawn_agent result should be json");
    let unblocked_thread_id = agent_id(&unblocked_result.agent_id).expect("valid agent id");
    let _ = manager
        .agent_control()
        .shutdown_agent(unblocked_thread_id)
        .await;
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
struct TeamAskLeadResult {
    team_id: String,
    lead_thread_id: String,
    submission_id: String,
    delivered: bool,
    inbox_entry_id: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxMessage {
    id: String,
    created_at: i64,
    from_thread_id: String,
    from_name: Option<String>,
    input_items: Vec<UserInput>,
    prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxPopResult {
    team_id: String,
    thread_id: String,
    messages: Vec<TeamInboxMessage>,
    ack_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxAckResult {
    team_id: String,
    thread_id: String,
    acked: bool,
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
async fn close_team_releases_slot_for_already_shutdown_member() {
    #[derive(Debug, Deserialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let mut config = (*turn.config).clone();
    config.agent_max_threads = Some(1);
    turn.config = Arc::new(config);

    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_team",
        function_payload(json!({
            "members": [
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
        success: spawn_success,
        ..
    } = spawn_output
    else {
        panic!("expected function output");
    };
    assert_eq!(spawn_success, Some(true));
    let spawn_result: SpawnTeamResult =
        serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
    assert_eq!(spawn_result.members.len(), 1);
    let member_thread_id = agent_id(&spawn_result.members[0].agent_id).expect("valid agent id");

    let thread = manager
        .get_thread(member_thread_id)
        .await
        .expect("spawned member should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                manager.agent_control().get_status(member_thread_id).await,
                AgentStatus::Shutdown
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("member should reach shutdown");

    let blocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "blocked"})),
    );
    let Err(err) = MultiAgentHandler.handle(blocked_invocation).await else {
        panic!("spawn_agent should fail when max threads already reached");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "collab spawn failed: agent thread limit reached (max 1)".to_string()
        )
    );

    let close_invocation = invocation(
        session.clone(),
        turn.clone(),
        "close_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    MultiAgentHandler
        .handle(close_invocation)
        .await
        .expect("close_team should succeed");

    let unblocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let unblocked_output = MultiAgentHandler
        .handle(unblocked_invocation)
        .await
        .expect("spawn_agent should succeed after close_team releases slot");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(unblocked_content),
        success: unblocked_success,
        ..
    } = unblocked_output
    else {
        panic!("expected function output");
    };
    assert_eq!(unblocked_success, Some(true));
    let unblocked_result: SpawnAgentResult =
        serde_json::from_str(&unblocked_content).expect("spawn_agent result should be json");
    let unblocked_thread_id = agent_id(&unblocked_result.agent_id).expect("valid agent id");
    let _ = manager
        .agent_control()
        .shutdown_agent(unblocked_thread_id)
        .await;
}

#[tokio::test]
async fn team_cleanup_releases_slot_for_already_shutdown_member() {
    #[derive(Debug, Deserialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let mut config = (*turn.config).clone();
    config.agent_max_threads = Some(1);
    turn.config = Arc::new(config);

    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_team",
        function_payload(json!({
            "members": [
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
        success: spawn_success,
        ..
    } = spawn_output
    else {
        panic!("expected function output");
    };
    assert_eq!(spawn_success, Some(true));
    let spawn_result: SpawnTeamResult =
        serde_json::from_str(&spawn_content).expect("spawn_team result should be json");
    assert_eq!(spawn_result.members.len(), 1);
    let member_thread_id = agent_id(&spawn_result.members[0].agent_id).expect("valid agent id");

    let thread = manager
        .get_thread(member_thread_id)
        .await
        .expect("spawned member should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                manager.agent_control().get_status(member_thread_id).await,
                AgentStatus::Shutdown
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("member should reach shutdown");

    let blocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "blocked"})),
    );
    let Err(err) = MultiAgentHandler.handle(blocked_invocation).await else {
        panic!("spawn_agent should fail when max threads already reached");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "collab spawn failed: agent thread limit reached (max 1)".to_string()
        )
    );

    let cleanup_invocation = invocation(
        session.clone(),
        turn.clone(),
        "team_cleanup",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    MultiAgentHandler
        .handle(cleanup_invocation)
        .await
        .expect("team_cleanup should succeed");

    let unblocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let unblocked_output = MultiAgentHandler
        .handle(unblocked_invocation)
        .await
        .expect("spawn_agent should succeed after team_cleanup releases slot");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(unblocked_content),
        success: unblocked_success,
        ..
    } = unblocked_output
    else {
        panic!("expected function output");
    };
    assert_eq!(unblocked_success, Some(true));
    let unblocked_result: SpawnAgentResult =
        serde_json::from_str(&unblocked_content).expect("spawn_agent result should be json");
    let unblocked_thread_id = agent_id(&unblocked_result.agent_id).expect("valid agent id");
    let _ = manager
        .agent_control()
        .shutdown_agent(unblocked_thread_id)
        .await;
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
    let _team_id = spawn_result.team_id.clone();
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
    let claim_next_result: TeamTaskClaimNextResult =
        serde_json::from_str(&claim_next_content).expect("team_task_claim_next should return json");
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
            "task `{worker_task_id}` has unresolved dependencies"
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
    let claim_worker_result: TeamTaskClaimResult =
        serde_json::from_str(&claim_worker_content).expect("team_task_claim result should json");
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
        FunctionCallError::RespondToModel(format!("task `{worker_task_id}` is already completed"))
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
async fn team_message_persists_inbox_when_delivery_fails() {
    #[derive(Debug, Deserialize)]
    struct TeamMessageDurableResult {
        team_id: String,
        member_name: String,
        agent_id: String,
        submission_id: String,
        delivered: bool,
        inbox_entry_id: String,
        error: Option<String>,
    }

    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let lead_thread_id = session.conversation_id;
    let codex_home = turn.config.codex_home.clone();

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_team",
        function_payload(json!({
            "members": [
                {"name": "planner", "task": "plan"}
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
    let member_id = spawn_result
        .members
        .first()
        .expect("member")
        .agent_id
        .as_str();
    let member_id = agent_id(member_id).expect("valid thread id");

    manager
        .agent_control()
        .shutdown_agent(member_id)
        .await
        .expect("shutdown agent should succeed");

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
        .expect("team_message should succeed even if delivery fails");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(message_content),
        ..
    } = message_output
    else {
        panic!("expected function output");
    };
    let message_result: TeamMessageDurableResult =
        serde_json::from_str(&message_content).expect("team_message result should be json");
    assert_eq!(message_result.member_name, "planner".to_string());
    assert_eq!(message_result.agent_id, member_id.to_string());
    assert_eq!(message_result.delivered, false);
    assert_eq!(message_result.submission_id.is_empty(), true);
    assert_eq!(message_result.inbox_entry_id.is_empty(), false);
    assert_eq!(message_result.error.is_some(), true);

    let inbox_file = inbox::inbox_dir(codex_home.as_path(), &message_result.team_id)
        .join(format!("{member_id}.jsonl"));
    let inbox_raw = tokio::fs::read_to_string(&inbox_file)
        .await
        .expect("inbox file should exist");
    let first_line = inbox_raw.lines().next().expect("inbox should have 1 line");
    let entry: inbox::TeamInboxEntry =
        serde_json::from_str(first_line).expect("inbox line should be json");
    assert_eq!(entry.id, message_result.inbox_entry_id);
    assert_eq!(entry.from_thread_id, lead_thread_id.to_string());
    assert_eq!(entry.from_name, Some("lead".to_string()));
    assert_eq!(entry.to_thread_id, member_id.to_string());
    assert_eq!(entry.prompt.contains("do planning"), true);
}

#[tokio::test]
async fn team_ask_lead_persists_and_lead_can_pop_and_ack() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let lead_thread_id = session.conversation_id;
    let codex_home = turn.config.codex_home.clone();
    let turn = Arc::new(turn);

    let mut session_arc = Arc::new(session);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": ThreadId::new().to_string(),
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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
    let member_id = spawn_result
        .members
        .first()
        .expect("member")
        .agent_id
        .as_str();
    let member_id = agent_id(member_id).expect("valid thread id");
    let team_id = spawn_result.team_id.clone();

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = member_id;

    session_arc = Arc::new(session);
    let ask_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_ask_lead",
            function_payload(json!({
                "team_id": team_id,
                "message": "need guidance"
            })),
        ))
        .await
        .expect("team_ask_lead should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(ask_content),
        ..
    } = ask_output
    else {
        panic!("expected function output");
    };
    let ask_result: TeamAskLeadResult =
        serde_json::from_str(&ask_content).expect("team_ask_lead result should be json");
    assert_eq!(ask_result.team_id.is_empty(), false);
    assert_eq!(ask_result.lead_thread_id, lead_thread_id.to_string());
    if ask_result.delivered {
        assert_eq!(ask_result.submission_id.is_empty(), false);
    } else {
        assert_eq!(ask_result.submission_id.is_empty(), true);
    }
    assert_eq!(ask_result.inbox_entry_id.is_empty(), false);
    assert_eq!(ask_result.error.is_none(), ask_result.delivered);

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = lead_thread_id;
    session_arc = Arc::new(session);

    let pop_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_pop",
            function_payload(json!({
                "team_id": ask_result.team_id,
                "limit": 50
            })),
        ))
        .await
        .expect("team_inbox_pop should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(pop_content),
        ..
    } = pop_output
    else {
        panic!("expected function output");
    };
    let pop_result: TeamInboxPopResult =
        serde_json::from_str(&pop_content).expect("team_inbox_pop result should be json");
    assert_eq!(pop_result.thread_id, lead_thread_id.to_string());
    assert_eq!(pop_result.messages.is_empty(), false);
    assert_eq!(pop_result.ack_token.is_empty(), false);
    assert_eq!(pop_result.messages[0].from_name, Some("worker".to_string()));
    let message = &pop_result.messages[0];
    assert_eq!(message.id.is_empty(), false);
    assert_eq!(message.created_at > 0, true);
    assert_eq!(message.from_thread_id, member_id.to_string());
    assert_eq!(message.input_items.is_empty(), false);
    assert_eq!(message.prompt.is_empty(), false);

    let ack_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_ack",
            function_payload(json!({
                "team_id": pop_result.team_id,
                "ack_token": pop_result.ack_token
            })),
        ))
        .await
        .expect("team_inbox_ack should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(ack_content),
        ..
    } = ack_output
    else {
        panic!("expected function output");
    };
    let ack_result: TeamInboxAckResult =
        serde_json::from_str(&ack_content).expect("team_inbox_ack result should be json");
    assert_eq!(ack_result.acked, true);
    assert_eq!(ack_result.thread_id, lead_thread_id.to_string());

    let pop_again_output = MultiAgentHandler
        .handle(invocation(
            session_arc,
            turn,
            "team_inbox_pop",
            function_payload(json!({
                "team_id": ask_result.team_id,
                "limit": 50
            })),
        ))
        .await
        .expect("team_inbox_pop should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(pop_again_content),
        ..
    } = pop_again_output
    else {
        panic!("expected function output");
    };
    let pop_again_result: TeamInboxPopResult =
        serde_json::from_str(&pop_again_content).expect("team_inbox_pop result should be json");
    assert_eq!(pop_again_result.messages.is_empty(), true);

    let _ = tokio::fs::remove_dir_all(inbox::inbox_dir(codex_home.as_path(), &ask_result.team_id))
        .await;
}

#[tokio::test]
async fn team_inbox_ack_noops_when_token_empty() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let ack_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "team_inbox_ack",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "ack_token": ""
            })),
        ))
        .await
        .expect("team_inbox_ack should succeed with empty token");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(ack_content),
        ..
    } = ack_output
    else {
        panic!("expected function output");
    };
    let ack_result: TeamInboxAckResult =
        serde_json::from_str(&ack_content).expect("team_inbox_ack result should be json");
    assert_eq!(ack_result.acked, false);

    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "team_cleanup",
            function_payload(json!({"team_id": ack_result.team_id})),
        ))
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn team_inbox_ack_rejects_invalid_token() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "team_inbox_ack",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "ack_token": "not-json"
            })),
        ))
        .await
    else {
        panic!("team_inbox_ack should fail for invalid token");
    };
    let FunctionCallError::RespondToModel(message) = err else {
        panic!("expected respond to model error");
    };
    assert_eq!(message.starts_with("ack_token is invalid:"), true);

    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "team_cleanup",
            function_payload(json!({"team_id": spawn_result.team_id})),
        ))
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn team_inbox_ack_rejects_team_id_mismatch() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let lead_thread_id = session.conversation_id;
    let turn = Arc::new(turn);

    let mut session_arc = Arc::new(session);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": ThreadId::new().to_string(),
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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
    let member_id = spawn_result
        .members
        .first()
        .expect("member")
        .agent_id
        .as_str();
    let member_id = agent_id(member_id).expect("valid thread id");

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = member_id;
    session_arc = Arc::new(session);
    MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_ask_lead",
            function_payload(json!({
                "team_id": team_id,
                "message": "need guidance"
            })),
        ))
        .await
        .expect("team_ask_lead should succeed");

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = lead_thread_id;
    session_arc = Arc::new(session);
    let pop_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_pop",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "limit": 1
            })),
        ))
        .await
        .expect("team_inbox_pop should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(pop_content),
        ..
    } = pop_output
    else {
        panic!("expected function output");
    };
    let pop_result: TeamInboxPopResult =
        serde_json::from_str(&pop_content).expect("team_inbox_pop result should be json");
    let mut token: inbox::TeamInboxAckToken =
        serde_json::from_str(&pop_result.ack_token).expect("ack_token json");
    token.team_id = ThreadId::new().to_string();
    let ack_token = serde_json::to_string(&token).expect("ack token serialize");

    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_ack",
            function_payload(json!({
                "team_id": pop_result.team_id,
                "ack_token": ack_token
            })),
        ))
        .await
    else {
        panic!("team_inbox_ack should fail for team_id mismatch");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel("ack_token team_id mismatch".to_string())
    );

    MultiAgentHandler
        .handle(invocation(
            session_arc,
            turn,
            "team_cleanup",
            function_payload(json!({"team_id": pop_result.team_id})),
        ))
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn team_inbox_ack_rejects_thread_id_mismatch() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let lead_thread_id = session.conversation_id;
    let turn = Arc::new(turn);

    let mut session_arc = Arc::new(session);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "team_id": ThreadId::new().to_string(),
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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
    let member_id = spawn_result
        .members
        .first()
        .expect("member")
        .agent_id
        .as_str();
    let member_id = agent_id(member_id).expect("valid thread id");

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = member_id;
    session_arc = Arc::new(session);
    MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_ask_lead",
            function_payload(json!({
                "team_id": team_id,
                "message": "need guidance"
            })),
        ))
        .await
        .expect("team_ask_lead should succeed");

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = lead_thread_id;
    session_arc = Arc::new(session);
    let pop_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_pop",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "limit": 1
            })),
        ))
        .await
        .expect("team_inbox_pop should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(pop_content),
        ..
    } = pop_output
    else {
        panic!("expected function output");
    };
    let pop_result: TeamInboxPopResult =
        serde_json::from_str(&pop_content).expect("team_inbox_pop result should be json");
    let mut token: inbox::TeamInboxAckToken =
        serde_json::from_str(&pop_result.ack_token).expect("ack_token json");
    token.thread_id = ThreadId::new().to_string();
    let ack_token = serde_json::to_string(&token).expect("ack token serialize");

    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_ack",
            function_payload(json!({
                "team_id": pop_result.team_id,
                "ack_token": ack_token
            })),
        ))
        .await
    else {
        panic!("team_inbox_ack should fail for thread_id mismatch");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel("ack_token thread_id mismatch".to_string())
    );

    MultiAgentHandler
        .handle(invocation(
            session_arc,
            turn,
            "team_cleanup",
            function_payload(json!({"team_id": pop_result.team_id})),
        ))
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn team_ask_lead_rejects_when_called_by_lead() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "team_ask_lead",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "message": "hello"
            })),
        ))
        .await
    else {
        panic!("team_ask_lead should fail when called by lead");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel("team_ask_lead cannot be called by the lead".to_string())
    );
}

#[tokio::test]
async fn team_inbox_pop_rejects_non_member() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let lead_thread_id = session.conversation_id;
    let turn = Arc::new(turn);

    let mut session_arc = Arc::new(session);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = ThreadId::new();
    session_arc = Arc::new(session);

    let caller_thread_id = session_arc.conversation_id;
    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "team_inbox_pop",
            function_payload(json!({
                "team_id": team_id
            })),
        ))
        .await
    else {
        panic!("team_inbox_pop should fail for non-member");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(format!(
            "thread `{caller_thread_id}` is not a member of team `{team_id}`"
        ))
    );

    // restore to avoid leaking registry keyed by wrong id
    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = lead_thread_id;
    let session = Arc::new(session);
    MultiAgentHandler
        .handle(invocation(
            session,
            Arc::clone(&turn),
            "team_cleanup",
            function_payload(json!({"team_id": team_id})),
        ))
        .await
        .expect("cleanup");
}

#[cfg(unix)]
#[tokio::test]
async fn team_inbox_appends_are_not_corrupted_under_concurrency() {
    let (_session, turn) = make_session_and_context().await;
    let codex_home = turn.config.codex_home.clone();
    let team_id = ThreadId::new().to_string();
    let receiver = ThreadId::new();
    let sender = ThreadId::new();
    let items = vec![UserInput::Text {
        text: "hello".to_string(),
        text_elements: Vec::new(),
    }];

    let task_count = 20usize;
    let per_task = 25usize;
    let total = task_count * per_task;

    let mut set = JoinSet::new();
    for _ in 0..task_count {
        let codex_home = codex_home.clone();
        let team_id = team_id.clone();
        let items = items.clone();
        set.spawn(async move {
            for _ in 0..per_task {
                inbox::append_inbox_entry(
                    codex_home.as_path(),
                    &team_id,
                    receiver,
                    sender,
                    Some("lead"),
                    &items,
                    "hello",
                )
                .await
                .expect("append should succeed");
            }
        });
    }
    while let Some(result) = set.join_next().await {
        result.expect("task should join");
    }

    let inbox_file =
        inbox::inbox_dir(codex_home.as_path(), &team_id).join(format!("{receiver}.jsonl"));
    let raw = tokio::fs::read_to_string(&inbox_file)
        .await
        .expect("inbox file");
    let mut ids = std::collections::HashSet::new();
    let mut count = 0usize;
    for line in raw.lines() {
        let entry: inbox::TeamInboxEntry = serde_json::from_str(line).expect("valid jsonl");
        assert_eq!(ids.insert(entry.id), true);
        count += 1;
    }
    assert_eq!(count, total);
}

#[tokio::test]
async fn team_task_claim_is_exclusive_under_concurrency() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let list_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "team_task_list",
            function_payload(json!({"team_id": spawn_result.team_id})),
        ))
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
        serde_json::from_str(&list_content).expect("team_task_list result should be json");
    let task_id = list_result.tasks.first().expect("task").task_id.clone();

    let mut set = JoinSet::new();
    for _ in 0..10 {
        let session = session.clone();
        let turn = turn.clone();
        let team_id = list_result.team_id.clone();
        let task_id = task_id.clone();
        set.spawn(async move {
            MultiAgentHandler
                .handle(invocation(
                    session,
                    turn,
                    "team_task_claim",
                    function_payload(json!({"team_id": team_id, "task_id": task_id})),
                ))
                .await
        });
    }

    let mut success = 0usize;
    let mut already_claimed = 0usize;
    while let Some(result) = set.join_next().await {
        let output = result.expect("join");
        match output {
            Ok(_) => success += 1,
            Err(FunctionCallError::RespondToModel(msg)) if msg.contains("already claimed") => {
                already_claimed += 1
            }
            Err(other) => panic!("unexpected result: {other:?}"),
        }
    }

    assert_eq!(success, 1);
    assert_eq!(already_claimed, 9);
}

#[tokio::test]
async fn team_task_complete_dispatches_hook_once_under_concurrency() {
    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let cwd = tempfile::tempdir().expect("temp dir");
    turn.cwd = cwd.path().to_path_buf();

    let marker_path = turn.config.codex_home.join("task_completed.log");
    let script = r#"import json, sys, time; data=json.load(sys.stdin); time.sleep(0.1); open(sys.argv[1], "a").write(data["hook_event_name"] + "\n")"#;
    session.services.hooks = Hooks::new(HooksConfig {
        command_hooks: CommandHooksConfig {
            task_completed: vec![CommandHookConfig {
                command: vec![
                    "python3".to_string(),
                    "-c".to_string(),
                    script.to_string(),
                    marker_path.to_string_lossy().into_owned(),
                ],
                ..Default::default()
            }],
            ..Default::default()
        },
    });

    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan"},
                    {"name": "worker", "task": "work"}
                ]
            })),
        ))
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

    let list_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "team_task_list",
            function_payload(json!({"team_id": spawn_result.team_id})),
        ))
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
        serde_json::from_str(&list_content).expect("team_task_list result should be json");
    let task_id = list_result.tasks.first().expect("task").task_id.clone();

    let mut set = JoinSet::new();
    for _ in 0..10 {
        let session = session.clone();
        let turn = turn.clone();
        let team_id = list_result.team_id.clone();
        let task_id = task_id.clone();
        set.spawn(async move {
            MultiAgentHandler
                .handle(invocation(
                    session,
                    turn,
                    "team_task_complete",
                    function_payload(json!({"team_id": team_id, "task_id": task_id})),
                ))
                .await
        });
    }

    let mut success = 0usize;
    let mut already_completed = 0usize;
    while let Some(result) = set.join_next().await {
        let output = result.expect("join");
        match output {
            Ok(_) => success += 1,
            Err(FunctionCallError::RespondToModel(msg)) if msg.contains("already completed") => {
                already_completed += 1
            }
            Err(other) => panic!("unexpected result: {other:?}"),
        }
    }

    assert_eq!(success, 1);
    assert_eq!(already_completed, 9);

    let hook_events = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("task_completed hook should write marker");
    assert_eq!(
        hook_events
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1
    );
    assert_eq!(hook_events.trim(), "TaskCompleted");
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
