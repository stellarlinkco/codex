use super::*;
use crate::AuthManager;
use crate::CodexAuth;
use crate::ThreadManager;
use crate::built_in_model_providers;
use crate::codex::make_session_and_context;
use crate::codex::make_session_and_context_with_rx;
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
use std::future::Future;
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

fn run_async_test_with_large_stack<F>(name: &str, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime")
                .block_on(future);
        })
        .expect("spawn large-stack test thread")
        .join()
        .expect("large-stack test thread should finish");
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

fn team_tasks_dir(codex_home: &Path, team_id: &str) -> PathBuf {
    codex_home.join("tasks").join(team_id)
}

#[test]
fn team_member_refs_formats_agent_type() {
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

    let refs = team_member_refs(&members);
    assert_eq!(refs.len(), 3);

    let typed = refs
        .iter()
        .find(|agent| agent.thread_id == typed_id)
        .expect("typed member");
    assert_eq!(
        typed.agent_nickname.as_deref(),
        Some("typed"),
        "typed member nickname"
    );
    assert_eq!(
        typed.agent_role.as_deref(),
        Some("reviewer"),
        "typed member role"
    );

    let blank = refs
        .iter()
        .find(|agent| agent.thread_id == blank_id)
        .expect("blank member");
    assert_eq!(
        blank.agent_role.as_deref(),
        Some("default"),
        "blank member defaults role"
    );

    let none = refs
        .iter()
        .find(|agent| agent.thread_id == none_id)
        .expect("none member");
    assert_eq!(
        none.agent_role.as_deref(),
        Some("default"),
        "none member defaults role"
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
async fn spawn_agent_uses_explorer_role_and_inherits_approval_policy() {
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
    turn.approval_policy
        .set(AskForApproval::OnRequest)
        .expect("approval policy should be set");
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
    assert_eq!(snapshot.approval_policy, AskForApproval::OnRequest);
}

#[tokio::test]
async fn spawn_agent_injects_role_memory_when_present() {
    #[derive(Debug, Deserialize)]
    struct SpawnAgentResult {
        agent_id: String,
    }

    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let codex_home = turn.config.codex_home.clone();

    let memory_path = codex_home
        .join("agent-memory")
        .join("plan")
        .join("MEMORY.md");
    tokio::fs::create_dir_all(memory_path.parent().expect("memory parent"))
        .await
        .expect("create memory dir");
    tokio::fs::write(&memory_path, "remember-me")
        .await
        .expect("write memory file");

    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "spawn_agent",
        function_payload(json!({
            "message": "hello",
            "agent_type": "plan"
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
    let spawned_agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");

    let thread = manager
        .get_thread(spawned_agent_id)
        .await
        .expect("spawned agent thread should exist");
    let history = thread.codex.session.clone_history().await;
    let has_memory = history.raw_items().iter().any(|item| match item {
        ResponseItem::Message { role, content, .. } if role == "developer" => {
            content.iter().any(|item| match item {
                ContentItem::InputText { text } => {
                    text.contains("# Agent Memory") && text.contains("remember-me")
                }
                _ => false,
            })
        }
        _ => false,
    });
    assert_eq!(has_memory, true);
}

#[tokio::test]
async fn spawn_agent_accepts_verify_role() {
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
            "message": "verify this",
            "agent_type": "verify"
        })),
    );
    let output = MultiAgentHandler
        .handle(invocation)
        .await
        .expect("spawn_agent should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success,
        ..
    } = output
    else {
        panic!("expected function output");
    };
    assert_eq!(success, Some(true));
    let result: SpawnAgentResult =
        serde_json::from_str(&content).expect("spawn_agent result should be json");
    let agent_id = agent_id(&result.agent_id).expect("agent_id should be valid");

    let status = manager.agent_control().get_status(agent_id).await;
    assert_ne!(status, AgentStatus::NotFound);
    let _ = manager.agent_control().shutdown_agent(agent_id).await;
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

#[test]
fn spawn_agent_accepts_backendground_alias() {
    run_async_test_with_large_stack("spawn_agent_accepts_backendground_alias", async move {
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
    });
}

#[test]
fn spawn_agent_accepts_background_field() {
    run_async_test_with_large_stack("spawn_agent_accepts_background_field", async move {
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
    });
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

    std::fs::create_dir_all(&turn.config.codex_home).expect("create codex_home");

    let marker_path = turn.config.codex_home.join("subagent_start.log");
    let injected_context = "subagent_start injected context";
    let script = r#"import sys, json; data=json.load(sys.stdin); open(sys.argv[1], "a").write(data["hook_event_name"] + "\n"); print(json.dumps({"additionalContext": "subagent_start injected context"}))"#;
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

    let thread = manager
        .get_thread(agent_id)
        .await
        .expect("spawned agent should exist");

    let mut injected_index = None;
    let mut prompt_index = None;
    for _ in 0..50 {
        injected_index = None;
        prompt_index = None;
        let history = thread.codex.session.clone_history().await;
        let items = history.raw_items();
        for (index, item) in items.iter().enumerate() {
            let text = serde_json::to_string(item).expect("response item should serialize");
            if injected_index.is_none() && text.contains(injected_context) {
                injected_index = Some(index);
            }
            if prompt_index.is_none() && text.contains("inspect this repo") {
                prompt_index = Some(index);
            }
            if injected_index.is_some() && prompt_index.is_some() {
                break;
            }
        }
        if injected_index.is_some() && prompt_index.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let injected_index = injected_index.expect("subagent_start context should be injected");
    let prompt_index = prompt_index.expect("prompt should be recorded");
    assert!(injected_index < prompt_index);

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
async fn send_message_rejects_empty_message() {
    let (session, turn) = make_session_and_context().await;
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({"to": ThreadId::new().to_string(), "message": ""})),
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
async fn send_message_rejects_when_message_and_items_are_both_set() {
    let (session, turn) = make_session_and_context().await;
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({
            "to": ThreadId::new().to_string(),
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
async fn send_message_rejects_invalid_id() {
    let (session, turn) = make_session_and_context().await;
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({"to": "not-a-uuid", "message": "hi"})),
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
async fn send_message_reports_missing_agent() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let agent_id = ThreadId::new();
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({"to": agent_id.to_string(), "message": "hi"})),
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
async fn send_message_interrupts_before_prompt() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let config = turn.config.as_ref().clone();
    let thread = manager.start_thread(config).await.expect("start thread");
    let agent_id = thread.thread_id;
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({
            "to": agent_id.to_string(),
            "message": "hi",
            "interrupt": true
        })),
    );
    MultiAgentHandler
        .handle(invocation)
        .await
        .expect("send_message should succeed");

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
async fn send_message_accepts_structured_items() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let config = turn.config.as_ref().clone();
    let thread = manager.start_thread(config).await.expect("start thread");
    let agent_id = thread.thread_id;
    let invocation = invocation(
        Arc::new(session),
        Arc::new(turn),
        "send_message",
        function_payload(json!({
            "to": agent_id.to_string(),
            "items": [
                {"type": "mention", "name": "drive", "path": "app://google_drive"},
                {"type": "text", "text": "read the folder"}
            ]
        })),
    );
    MultiAgentHandler
        .handle(invocation)
        .await
        .expect("send_message should succeed");

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
async fn send_message_includes_receiver_metadata_in_events() {
    let (mut session, turn, rx) = make_session_and_context_with_rx().await;
    let manager = thread_manager();
    Arc::get_mut(&mut session)
        .expect("session should be unique")
        .services
        .agent_control = manager.agent_control();

    let config = turn.config.as_ref().clone();
    let (agent_id, _notification_source) = session
        .services
        .agent_control
        .spawn_agent_thread(
            config,
            Some(thread_spawn_source(session.conversation_id, 1)),
        )
        .await
        .expect("spawn_agent_thread should succeed");
    let (expected_nickname, expected_role) = session
        .services
        .agent_control
        .get_agent_nickname_and_role(agent_id)
        .await
        .expect("spawned agent should have metadata");
    assert!(
        expected_nickname
            .as_deref()
            .is_some_and(|nickname| !nickname.trim().is_empty()),
        "agent nickname should be populated"
    );

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn,
            "send_message",
            function_payload(json!({
                "to": agent_id.to_string(),
                "message": "hi"
            })),
        ))
        .await
        .expect("send_message should succeed");

    let interaction_end = timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.expect("event should be received");
            match event.msg {
                codex_protocol::protocol::EventMsg::CollabAgentInteractionEnd(ev)
                    if ev.call_id == "call-1" =>
                {
                    break ev;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("send_message should emit a CollabAgentInteractionEnd event");

    assert_eq!(interaction_end.receiver_thread_id, agent_id);
    assert_eq!(interaction_end.receiver_agent_nickname, expected_nickname);
    assert_eq!(interaction_end.receiver_agent_role, expected_role);

    let _ = manager.agent_control().shutdown_agent(agent_id).await;
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
        "send_message",
        function_payload(json!({"to": agent_id.to_string(), "message": "hello"})),
    );
    let output = MultiAgentHandler
        .handle(send_invocation)
        .await
        .expect("send_message should succeed after resume");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success,
        ..
    } = output
    else {
        panic!("expected function output");
    };
    let result: serde_json::Value =
        serde_json::from_str(&content).expect("send_message result should be json");
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
async fn wait_includes_receiver_metadata_in_events() {
    let (mut session, turn, rx) = make_session_and_context_with_rx().await;
    let manager = thread_manager();
    Arc::get_mut(&mut session)
        .expect("session should be unique")
        .services
        .agent_control = manager.agent_control();

    let config = turn.config.as_ref().clone();
    let (agent_id, _notification_source) = session
        .services
        .agent_control
        .spawn_agent_thread(
            config,
            Some(thread_spawn_source(session.conversation_id, 1)),
        )
        .await
        .expect("spawn_agent_thread should succeed");
    let (expected_nickname, expected_role) = session
        .services
        .agent_control
        .get_agent_nickname_and_role(agent_id)
        .await
        .expect("spawned agent should have metadata");
    assert!(
        expected_nickname
            .as_deref()
            .is_some_and(|nickname| !nickname.trim().is_empty()),
        "agent nickname should be populated"
    );

    let mut status_rx = manager
        .agent_control()
        .subscribe_status(agent_id)
        .await
        .expect("subscribe should succeed");
    let thread = manager
        .get_thread(agent_id)
        .await
        .expect("spawned agent should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    let _ = timeout(Duration::from_secs(5), status_rx.changed())
        .await
        .expect("shutdown status should arrive");

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "wait",
            function_payload(json!({
                "ids": [agent_id.to_string()],
                "timeout_ms": 1000
            })),
        ))
        .await
        .expect("wait should succeed");

    let (waiting_begin, waiting_end) = timeout(Duration::from_secs(5), async {
        let mut waiting_begin = None;
        let mut waiting_end = None;
        while waiting_begin.is_none() || waiting_end.is_none() {
            let event = rx.recv().await.expect("event should be received");
            match event.msg {
                codex_protocol::protocol::EventMsg::CollabWaitingBegin(ev)
                    if ev.call_id == "call-1" =>
                {
                    waiting_begin = Some(ev);
                }
                codex_protocol::protocol::EventMsg::CollabWaitingEnd(ev)
                    if ev.call_id == "call-1" =>
                {
                    waiting_end = Some(ev);
                }
                _ => {}
            }
        }
        (waiting_begin.unwrap(), waiting_end.unwrap())
    })
    .await
    .expect("wait should emit CollabWaitingBegin and CollabWaitingEnd events");

    let begin_ref = waiting_begin
        .receiver_agents
        .iter()
        .find(|receiver| receiver.thread_id == agent_id)
        .expect("waiting begin should include receiver agent ref");
    assert_eq!(begin_ref.agent_nickname, expected_nickname);
    assert_eq!(begin_ref.agent_role, expected_role);

    let end_entry = waiting_end
        .agent_statuses
        .iter()
        .find(|entry| entry.thread_id == agent_id)
        .expect("waiting end should include agent status entry");
    assert_eq!(end_entry.agent_nickname, expected_nickname);
    assert_eq!(end_entry.agent_role, expected_role);

    let _ = manager.agent_control().shutdown_agent(agent_id).await;
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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&agent_thread_id), true);

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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.contains(&agent_thread_id), false);

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

#[tokio::test]
async fn spawn_agent_reaps_shutdown_agent_on_thread_limit() {
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
    let first_thread_id = agent_id(&spawn_result.agent_id).expect("valid agent id");

    let thread = manager
        .get_thread(first_thread_id)
        .await
        .expect("spawned agent should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                manager.agent_control().get_status(first_thread_id).await,
                AgentStatus::Shutdown
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent should reach shutdown");

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&first_thread_id), true);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let spawn_output = MultiAgentHandler
        .handle(spawn_invocation)
        .await
        .expect("spawn_agent should succeed by reaping shutdown agent");
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
    let second_thread_id = agent_id(&spawn_result.agent_id).expect("valid agent id");
    assert_eq!(second_thread_id == first_thread_id, false);

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&first_thread_id), false);
    assert_eq!(spawned_threads.contains(&second_thread_id), true);

    let _ = manager
        .agent_control()
        .shutdown_agent(second_thread_id)
        .await;
}

#[tokio::test]
async fn spawn_team_reaps_shutdown_agent_on_thread_limit() {
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
    let first_thread_id = agent_id(&spawn_result.agent_id).expect("valid agent id");

    let thread = manager
        .get_thread(first_thread_id)
        .await
        .expect("spawned agent should exist");
    let _ = thread
        .submit(Op::Shutdown {})
        .await
        .expect("shutdown should submit");
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                manager.agent_control().get_status(first_thread_id).await,
                AgentStatus::Shutdown
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent should reach shutdown");

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&first_thread_id), true);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
        function_payload(json!({
            "members": [
                {"name": "worker", "task": "work"}
            ]
        })),
    );
    let spawn_output = MultiAgentHandler
        .handle(spawn_invocation)
        .await
        .expect("spawn_team should succeed by reaping shutdown agent");
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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&first_thread_id), false);
    assert_eq!(spawned_threads.contains(&member_thread_id), true);

    let _ = manager
        .agent_control()
        .shutdown_agent(member_thread_id)
        .await;
}

#[tokio::test]
async fn spawn_agent_fails_when_limit_reached_without_reclaimable_threads() {
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
    let first_thread_id = agent_id(&spawn_result.agent_id).expect("valid agent id");

    let status = session
        .services
        .agent_control
        .get_status(first_thread_id)
        .await;
    assert_eq!(
        matches!(status, AgentStatus::PendingInit | AgentStatus::Running),
        true
    );

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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&first_thread_id), true);

    let _ = manager
        .agent_control()
        .shutdown_agent(first_thread_id)
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
struct DeleteTeamMemberResult {
    name: String,
    agent_id: String,
    ok: bool,
    status: AgentStatus,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteTeamResult {
    team_id: String,
    removed_from_registry: bool,
    removed_team_config: bool,
    removed_task_dir: bool,
    closed: Vec<DeleteTeamMemberResult>,
}

#[tokio::test]
async fn delete_team_releases_slot_for_already_shutdown_member() {
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
        "create_team",
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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&member_thread_id), true);

    let delete_invocation = invocation(
        session.clone(),
        turn.clone(),
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.contains(&member_thread_id), false);

    let unblocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let unblocked_output = MultiAgentHandler
        .handle(unblocked_invocation)
        .await
        .expect("spawn_agent should succeed after delete_team releases slot");
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
async fn delete_team_closes_active_teammate_and_releases_slot() {
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
        "create_team",
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

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.len(), 1);
    assert_eq!(spawned_threads.contains(&member_thread_id), true);

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "delete_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        ))
        .await
        .expect("delete_team should succeed");

    let spawned_threads = session.services.agent_control.spawned_thread_ids();
    assert_eq!(spawned_threads.contains(&member_thread_id), false);

    let unblocked_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "unblocked"})),
    );
    let unblocked_output = MultiAgentHandler
        .handle(unblocked_invocation)
        .await
        .expect("spawn_agent should succeed after delete_team releases slot");
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

#[test]
fn insert_team_record_allows_multiple_teams_per_session() {
    let lead_thread_id = ThreadId::new();
    let first_record = TeamRecord {
        members: vec![TeamMember {
            name: "worker".to_string(),
            agent_id: ThreadId::new(),
            agent_type: None,
        }],
        created_at: 0,
    };
    let second_record = TeamRecord {
        members: vec![TeamMember {
            name: "reviewer".to_string(),
            agent_id: ThreadId::new(),
            agent_type: None,
        }],
        created_at: 0,
    };
    insert_team_record(lead_thread_id, "team-1".to_string(), first_record)
        .expect("first insert should succeed");
    insert_team_record(lead_thread_id, "team-2".to_string(), second_record.clone())
        .expect("second insert should succeed");
    let err = insert_team_record(lead_thread_id, "team-2".to_string(), second_record)
        .expect_err("duplicate team id should fail");
    assert_eq!(
        err,
        FunctionCallError::RespondToModel("team `team-2` already exists".to_string())
    );
    remove_team_record(lead_thread_id, "team-1").expect("cleanup should succeed");
    remove_team_record(lead_thread_id, "team-2").expect("cleanup should succeed");
}

#[tokio::test]
async fn spawn_is_rejected_for_agent_team_teammates() {
    let (mut session, turn) = make_session_and_context().await;
    let lead_thread_id = session.conversation_id;
    let member_thread_id = ThreadId::new();
    insert_team_record(
        lead_thread_id,
        "team-1".to_string(),
        TeamRecord {
            members: vec![TeamMember {
                name: "worker".to_string(),
                agent_id: member_thread_id,
                agent_type: None,
            }],
            created_at: 0,
        },
    )
    .expect("insert team record should succeed");

    session.conversation_id = member_thread_id;
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_agent_invocation = invocation(
        session.clone(),
        turn.clone(),
        "spawn_agent",
        function_payload(json!({"message": "do work"})),
    );
    let Err(err) = MultiAgentHandler.handle(spawn_agent_invocation).await else {
        panic!("spawn_agent should fail for agent team teammates");
    };
    let FunctionCallError::RespondToModel(message) = err else {
        panic!("expected RespondToModel error");
    };
    assert!(message.contains("spawn_agent is disabled for agent team teammates"));

    let spawn_team_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
        function_payload(json!({"members": [{"name": "worker", "task": "work"}]})),
    );
    let Err(err) = MultiAgentHandler.handle(spawn_team_invocation).await else {
        panic!("spawn_team should fail for agent team teammates");
    };
    let FunctionCallError::RespondToModel(message) = err else {
        panic!("expected RespondToModel error");
    };
    assert!(message.contains("create_team is disabled for agent team teammates"));

    remove_team_record(lead_thread_id, "team-1").expect("cleanup should succeed");
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
        "create_team",
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
    assert_eq!(
        tokio::fs::metadata(&persisted_tasks_dir).await.is_err(),
        true
    );

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
        "wait",
        function_payload(json!({
            "team_id": spawn_result.team_id,
            "mode": "all",
            "timeout_ms": 1_000
        })),
    );
    let wait_output = MultiAgentHandler
        .handle(wait_invocation)
        .await
        .expect("wait should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(wait_content),
        success: wait_success,
        ..
    } = wait_output
    else {
        panic!("expected function output");
    };
    let wait_result: WaitResult =
        serde_json::from_str(&wait_content).expect("wait result should be json");
    assert_eq!(wait_result.timed_out, false);
    for status in wait_result.status.values() {
        assert!(matches!(
            status,
            AgentStatus::NotFound | AgentStatus::Shutdown
        ));
    }
    assert_eq!(wait_success, None);

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "delete_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        ))
        .await
        .expect("delete_team should succeed");

    let wait_missing_invocation = invocation(
        session,
        turn.clone(),
        "wait",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    let Err(err) = MultiAgentHandler.handle(wait_missing_invocation).await else {
        panic!("wait should fail after delete_team removed the team");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(format!("team `{}` not found", spawn_result.team_id))
    );
    assert_eq!(
        tokio::fs::metadata(team_dir(
            turn.config.codex_home.as_path(),
            &spawn_result.team_id
        ))
        .await
        .is_err(),
        true
    );
    assert_eq!(
        tokio::fs::metadata(team_tasks_dir(
            turn.config.codex_home.as_path(),
            &spawn_result.team_id,
        ))
        .await
        .is_err(),
        true
    );
}

#[tokio::test]
async fn wait_team_any_includes_non_final_member_statuses_in_events() {
    let (mut session, turn, rx) = make_session_and_context_with_rx().await;
    let manager = thread_manager();
    Arc::get_mut(&mut session)
        .expect("session should be unique")
        .services
        .agent_control = manager.agent_control();

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "members": [
                    {"name": "planner", "task": "plan the work"},
                    {"name": "worker", "task": "execute the task"}
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

    let planner = spawn_result
        .members
        .iter()
        .find(|member| member.name == "planner")
        .expect("planner should exist");
    let worker = spawn_result
        .members
        .iter()
        .find(|member| member.name == "worker")
        .expect("worker should exist");
    let planner_id = agent_id(&planner.agent_id).expect("valid planner agent id");
    let worker_id = agent_id(&worker.agent_id).expect("valid worker agent id");

    manager
        .agent_control()
        .shutdown_agent(planner_id)
        .await
        .expect("shutdown planner");

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "wait",
            function_payload(json!({
                "team_id": spawn_result.team_id,
                "mode": "any",
                "timeout_ms": 1_000
            })),
        ))
        .await
        .expect("wait should succeed");

    let waiting_end = timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.expect("event should be received");
            match event.msg {
                codex_protocol::protocol::EventMsg::CollabWaitingEnd(ev)
                    if ev.call_id == "team/wait:call-1" =>
                {
                    break ev;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("wait should emit a CollabWaitingEnd event");

    let worker_status = waiting_end
        .agent_statuses
        .iter()
        .find(|entry| entry.thread_id == worker_id)
        .map(|entry| &entry.status)
        .expect("worker should have a status entry");
    assert!(!matches!(worker_status, &AgentStatus::NotFound));

    manager
        .agent_control()
        .shutdown_agent(worker_id)
        .await
        .expect("shutdown worker");
    remove_team_record(session.conversation_id, &spawn_result.team_id)
        .expect("team record should be removed");
}

#[tokio::test]
async fn delete_team_only_removes_requested_team_when_multiple_teams_exist() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_team_a_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "team_id": "team-a",
                "members": [{"name": "planner", "task": "plan"}]
            })),
        ))
        .await
        .expect("spawn_team team-a should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(spawn_team_a_content),
        ..
    } = spawn_team_a_output
    else {
        panic!("expected function output");
    };
    let spawn_team_a_result: SpawnTeamResult =
        serde_json::from_str(&spawn_team_a_content).expect("spawn_team result should be json");
    let team_a_member_id = spawn_team_a_result
        .members
        .first()
        .expect("team-a member")
        .agent_id
        .as_str();
    let team_a_member_id = agent_id(team_a_member_id).expect("valid team-a member id");

    let spawn_team_b_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "team_id": "team-b",
                "members": [{"name": "worker", "task": "work"}]
            })),
        ))
        .await
        .expect("spawn_team team-b should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(spawn_team_b_content),
        ..
    } = spawn_team_b_output
    else {
        panic!("expected function output");
    };
    let spawn_team_b_result: SpawnTeamResult =
        serde_json::from_str(&spawn_team_b_content).expect("spawn_team result should be json");
    let team_b_member_id = spawn_team_b_result
        .members
        .first()
        .expect("team-b member")
        .agent_id
        .as_str();
    let team_b_member_id = agent_id(team_b_member_id).expect("valid team-b member id");

    manager
        .agent_control()
        .shutdown_agent(team_a_member_id)
        .await
        .expect("shutdown team-a member should succeed");

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "delete_team",
            function_payload(json!({"team_id": spawn_team_a_result.team_id})),
        ))
        .await
        .expect("delete_team team-a should succeed");

    let team_a_dir_exists = tokio::fs::metadata(team_dir(
        turn.config.codex_home.as_path(),
        &spawn_team_a_result.team_id,
    ))
    .await
    .is_ok();
    let team_b_dir_exists = tokio::fs::metadata(team_dir(
        turn.config.codex_home.as_path(),
        &spawn_team_b_result.team_id,
    ))
    .await
    .is_ok();
    assert_eq!(team_a_dir_exists, false);
    assert_eq!(team_b_dir_exists, true);
    let team_b_tasks_exist = tokio::fs::metadata(team_tasks_dir(
        turn.config.codex_home.as_path(),
        &spawn_team_b_result.team_id,
    ))
    .await
    .is_ok();
    assert_eq!(team_b_tasks_exist, false);

    let wait_team_a_invocation = invocation(
        session.clone(),
        turn.clone(),
        "wait",
        function_payload(json!({"team_id": spawn_team_a_result.team_id})),
    );
    let Err(err) = MultiAgentHandler.handle(wait_team_a_invocation).await else {
        panic!("wait should fail after team-a delete_team removed the team");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(format!(
            "team `{}` not found",
            spawn_team_a_result.team_id
        ))
    );

    manager
        .agent_control()
        .shutdown_agent(team_b_member_id)
        .await
        .expect("shutdown team-b member should succeed");

    let wait_team_b_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "wait",
            function_payload(json!({
                "team_id": spawn_team_b_result.team_id,
                "mode": "all",
                "timeout_ms": 1_000
            })),
        ))
        .await
        .expect("wait team-b should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(wait_team_b_content),
        success: wait_team_b_success,
        ..
    } = wait_team_b_output
    else {
        panic!("expected function output");
    };
    let wait_team_b_result: WaitResult =
        serde_json::from_str(&wait_team_b_content).expect("wait result should be json");
    assert_eq!(wait_team_b_success, None);
    assert_eq!(wait_team_b_result.timed_out, false);
    assert_eq!(wait_team_b_result.status.len(), 1);
    let state = wait_team_b_result
        .status
        .values()
        .next()
        .cloned()
        .unwrap_or(AgentStatus::NotFound);
    assert!(matches!(
        state,
        AgentStatus::NotFound | AgentStatus::Shutdown
    ));

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "delete_team",
            function_payload(json!({"team_id": spawn_team_b_result.team_id})),
        ))
        .await
        .expect("delete_team team-b should succeed");
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
        "create_team",
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

    let delete_invocation = invocation(
        session,
        turn,
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    let delete_output = MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");
    let ToolOutput::Function {
        success: delete_success,
        ..
    } = delete_output
    else {
        panic!("expected function output");
    };
    assert_eq!(delete_success, Some(true));
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
        "create_team",
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

    let delete_invocation = invocation(
        session,
        turn,
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    let delete_output = MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");
    let ToolOutput::Function {
        success: delete_success,
        ..
    } = delete_output
    else {
        panic!("expected function output");
    };
    assert_eq!(delete_success, Some(true));
}

#[tokio::test]
async fn spawn_team_background_member_auto_closes_after_shutdown() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
        function_payload(json!({
            "members": [
                {"name": "worker", "task": "work", "background": true}
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

    if let Ok(thread) = manager.get_thread(member_thread_id).await {
        let _ = thread
            .submit(Op::Shutdown {})
            .await
            .expect("shutdown should submit");
        timeout(Duration::from_secs(5), async {
            loop {
                if matches!(
                    session
                        .services
                        .agent_control
                        .get_status(member_thread_id)
                        .await,
                    AgentStatus::Shutdown | AgentStatus::NotFound
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background member should reach shutdown");
    }

    timeout(Duration::from_secs(5), async {
        loop {
            if !session
                .services
                .agent_control
                .spawned_thread_ids()
                .contains(&member_thread_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background member should be auto-closed");

    let delete_invocation = invocation(
        session,
        turn,
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");
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
        "create_team",
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
        "wait",
        function_payload(json!({
            "team_id": team_id
        })),
    );
    let Err(wait_err) = MultiAgentHandler.handle(wait_invocation).await else {
        panic!("wait should fail because the failed team was never created");
    };
    assert_eq!(
        wait_err,
        FunctionCallError::RespondToModel(format!("team `{team_id}` not found"))
    );
}

#[tokio::test]
async fn delete_team_cleans_worktree_leases_for_worktree_members() {
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
        "create_team",
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

    let delete_invocation = invocation(
        session,
        turn,
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    let delete_output = MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(delete_content),
        success: delete_success,
        ..
    } = delete_output
    else {
        panic!("expected function output");
    };
    let delete_result: DeleteTeamResult =
        serde_json::from_str(&delete_content).expect("delete_team result should be json");
    assert_eq!(delete_result.closed.len(), 2);
    assert_eq!(delete_success, Some(true));
    for member in &delete_result.closed {
        assert!(!member.name.trim().is_empty());
        assert!(!member.agent_id.trim().is_empty());
        let _ = &member.status;
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
async fn delete_team_removes_worktrees_when_members_are_already_shutdown() {
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
        "create_team",
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

    let delete_invocation = invocation(
        session,
        turn,
        "delete_team",
        function_payload(json!({
            "team_id": spawn_result.team_id
        })),
    );
    let delete_output = MultiAgentHandler
        .handle(delete_invocation)
        .await
        .expect("delete_team should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(delete_content),
        success: delete_success,
        ..
    } = delete_output
    else {
        panic!("expected function output");
    };
    let delete_result: DeleteTeamResult =
        serde_json::from_str(&delete_content).expect("delete_team result should be json");
    assert_eq!(delete_success, Some(true));
    assert_eq!(delete_result.removed_from_registry, true);
    assert_eq!(delete_result.removed_team_config, true);
    assert_eq!(delete_result.removed_task_dir, true);
    assert_eq!(delete_result.closed.len(), 2);
    for member in &delete_result.closed {
        assert!(!member.name.trim().is_empty());
        assert!(!member.agent_id.trim().is_empty());
        let _ = &member.status;
        assert_eq!(member.ok, true);
        assert_eq!(member.error, None);
    }
    assert_eq!(
        list_worktree_paths(codex_home.as_path(), lead_thread_id).is_empty(),
        true
    );
    assert_eq!(
        tokio::fs::metadata(team_config_path(
            codex_home.as_path(),
            &delete_result.team_id
        ))
        .await
        .is_err(),
        true
    );
    assert_eq!(
        tokio::fs::metadata(team_tasks_dir(codex_home.as_path(), &delete_result.team_id))
            .await
            .is_err(),
        true
    );
}

#[tokio::test]
async fn create_team_injects_coordinator_prompt_into_lead_history() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
        function_payload(json!({
            "team_id": "team-1",
            "members": [
                {"name": "planner", "task": "plan"}
            ]
        })),
    );
    let spawn_output = MultiAgentHandler
        .handle(spawn_invocation)
        .await
        .expect("create_team should succeed");
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
        serde_json::from_str(&spawn_content).expect("create_team result should be json");
    assert_eq!(spawn_result.team_id, "team-1");

    let pending = session.get_pending_input().await;
    let history = session.clone_history().await;
    let wants_prompt = |text: &str| {
        text.contains("# Team Coordinator Instructions")
            && text.contains("team-1")
            && text.contains("planner")
    };
    let pending_has_prompt = pending.iter().any(|item| match item {
        codex_protocol::models::ResponseInputItem::Message { role, content }
            if role == "developer" =>
        {
            content.iter().any(|item| match item {
                ContentItem::InputText { text } => wants_prompt(text),
                _ => false,
            })
        }
        _ => false,
    });
    let history_has_prompt = history.raw_items().iter().any(|item| match item {
        ResponseItem::Message { role, content, .. } if role == "developer" => {
            content.iter().any(|item| match item {
                ContentItem::InputText { text } => wants_prompt(text),
                _ => false,
            })
        }
        _ => false,
    });
    let has_prompt = pending_has_prompt || history_has_prompt;
    assert_eq!(has_prompt, true);
}

#[tokio::test]
async fn send_message_team_and_broadcast_send_inputs() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
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
        "send_message",
        function_payload(json!({
            "team_id": spawn_result.team_id,
            "to": "planner",
            "message": "do planning",
            "interrupt": false
        })),
    );
    let message_output = MultiAgentHandler
        .handle(message_invocation)
        .await
        .expect("send_message should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(message_content),
        ..
    } = message_output
    else {
        panic!("expected function output");
    };
    let message_result: serde_json::Value =
        serde_json::from_str(&message_content).expect("send_message result should be json");
    assert_eq!(message_result["route"].as_str(), Some("team_member"));
    assert_eq!(message_result["member_name"].as_str(), Some("planner"));
    assert_eq!(
        message_result["submission_id"].as_str().map(str::is_empty),
        Some(false)
    );

    let broadcast_invocation = invocation(
        session.clone(),
        turn.clone(),
        "send_message",
        function_payload(json!({
            "team_id": spawn_result.team_id,
            "to": "*",
            "message": "status update",
            "broadcast": true,
            "interrupt": false
        })),
    );
    let broadcast_output = MultiAgentHandler
        .handle(broadcast_invocation)
        .await
        .expect("send_message broadcast should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(broadcast_content),
        ..
    } = broadcast_output
    else {
        panic!("expected function output");
    };
    let broadcast_result: serde_json::Value =
        serde_json::from_str(&broadcast_content).expect("send_message result should be json");
    assert_eq!(broadcast_result["route"].as_str(), Some("broadcast"));
    assert_eq!(
        broadcast_result["sent"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            + broadcast_result["failed"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
        spawn_result.members.len()
    );
    for sent in broadcast_result["sent"]
        .as_array()
        .expect("sent should be an array")
    {
        assert_eq!(sent["member_name"].as_str().map(str::is_empty), Some(false));
        assert_eq!(sent["agent_id"].as_str().map(str::is_empty), Some(false));
        assert_eq!(
            sent["submission_id"].as_str().map(str::is_empty),
            Some(false)
        );
    }
    for failed in broadcast_result["failed"]
        .as_array()
        .expect("failed should be an array")
    {
        assert_eq!(
            failed["member_name"].as_str().map(str::is_empty),
            Some(false)
        );
        assert_eq!(failed["agent_id"].as_str().map(str::is_empty), Some(false));
        assert_eq!(failed["error"].as_str().map(str::is_empty), Some(false));
    }

    let user_input_count = manager
        .captured_ops()
        .iter()
        .filter(|(id, op)| member_ids.contains(id) && matches!(op, Op::UserInput { .. }))
        .count();
    assert_eq!(user_input_count > 0, true);

    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "delete_team",
            function_payload(json!({
                "team_id": spawn_result.team_id
            })),
        ))
        .await
        .expect("delete_team should succeed");
}

#[tokio::test]
async fn send_message_uses_team_id_when_member_names_overlap() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_team_a_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "team_id": "team-a",
                "members": [{"name": "worker", "task": "plan"}]
            })),
        ))
        .await
        .expect("spawn_team team-a should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(spawn_team_a_content),
        ..
    } = spawn_team_a_output
    else {
        panic!("expected function output");
    };
    let spawn_team_a_result: SpawnTeamResult =
        serde_json::from_str(&spawn_team_a_content).expect("spawn_team result should be json");
    let team_a_member_id = spawn_team_a_result
        .members
        .first()
        .expect("team-a member")
        .agent_id
        .clone();

    let spawn_team_b_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "team_id": "team-b",
                "members": [{"name": "worker", "task": "execute"}]
            })),
        ))
        .await
        .expect("spawn_team team-b should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(spawn_team_b_content),
        ..
    } = spawn_team_b_output
    else {
        panic!("expected function output");
    };
    let spawn_team_b_result: SpawnTeamResult =
        serde_json::from_str(&spawn_team_b_content).expect("spawn_team result should be json");
    let team_b_member_id = spawn_team_b_result
        .members
        .first()
        .expect("team-b member")
        .agent_id
        .clone();

    let message_team_a_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "send_message",
            function_payload(json!({
                "team_id": spawn_team_a_result.team_id,
                "to": "worker",
                "message": "plan now"
            })),
        ))
        .await
        .expect("send_message team-a should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(message_team_a_content),
        ..
    } = message_team_a_output
    else {
        panic!("expected function output");
    };
    let message_team_a_result: serde_json::Value =
        serde_json::from_str(&message_team_a_content).expect("send_message result should be json");
    assert_eq!(message_team_a_result["route"].as_str(), Some("team_member"));
    assert_eq!(
        message_team_a_result["agent_id"].as_str(),
        Some(team_a_member_id.as_str())
    );

    let message_team_b_output = MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "send_message",
            function_payload(json!({
                "team_id": spawn_team_b_result.team_id,
                "to": "worker",
                "message": "execute now"
            })),
        ))
        .await
        .expect("send_message team-b should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(message_team_b_content),
        ..
    } = message_team_b_output
    else {
        panic!("expected function output");
    };
    let message_team_b_result: serde_json::Value =
        serde_json::from_str(&message_team_b_content).expect("send_message result should be json");
    assert_eq!(message_team_b_result["route"].as_str(), Some("team_member"));
    assert_eq!(
        message_team_b_result["agent_id"].as_str(),
        Some(team_b_member_id.as_str())
    );

    let team_a_user_input_count = manager
        .captured_ops()
        .iter()
        .filter(|(id, op)| {
            *id == agent_id(&team_a_member_id).expect("valid team-a member id")
                && matches!(op, Op::UserInput { .. })
        })
        .count();
    let team_b_user_input_count = manager
        .captured_ops()
        .iter()
        .filter(|(id, op)| {
            *id == agent_id(&team_b_member_id).expect("valid team-b member id")
                && matches!(op, Op::UserInput { .. })
        })
        .count();
    assert_eq!(team_a_user_input_count > 0, true);
    assert_eq!(team_b_user_input_count > 0, true);

    MultiAgentHandler
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "delete_team",
            function_payload(json!({"team_id": spawn_team_a_result.team_id})),
        ))
        .await
        .expect("delete_team team-a should succeed");
    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "delete_team",
            function_payload(json!({"team_id": spawn_team_b_result.team_id})),
        ))
        .await
        .expect("delete_team team-b should succeed");
}

#[tokio::test]
async fn send_message_reports_delivery_failure_without_erroring() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_invocation = invocation(
        session.clone(),
        turn.clone(),
        "create_team",
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
    let member_id_str = member_id.to_string();

    manager
        .agent_control()
        .shutdown_agent(member_id)
        .await
        .expect("shutdown agent should succeed");

    let message_invocation = invocation(
        session.clone(),
        turn.clone(),
        "send_message",
        function_payload(json!({
            "team_id": spawn_result.team_id,
            "to": "planner",
            "message": "do planning"
        })),
    );
    let message_output = MultiAgentHandler
        .handle(message_invocation)
        .await
        .expect("send_message should succeed even if delivery fails");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(message_content),
        ..
    } = message_output
    else {
        panic!("expected function output");
    };
    let message_result: serde_json::Value =
        serde_json::from_str(&message_content).expect("send_message result should be json");
    assert_eq!(message_result["route"].as_str(), Some("team_member"));
    assert_eq!(message_result["member_name"].as_str(), Some("planner"));
    assert_eq!(
        message_result["agent_id"].as_str(),
        Some(member_id_str.as_str())
    );
    assert_eq!(message_result["delivered"].as_bool(), Some(false));
    assert_eq!(
        message_result["submission_id"].as_str().map(str::is_empty),
        Some(true)
    );
    assert_eq!(
        message_result["error"].as_str().map(str::is_empty),
        Some(false)
    );

    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "delete_team",
            function_payload(json!({"team_id": spawn_result.team_id})),
        ))
        .await
        .expect("delete_team should succeed");
}

#[tokio::test]
async fn send_message_ask_lead_delivers_or_reports_failure() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let lead_thread_id = session.conversation_id;
    let lead_thread_id_str = lead_thread_id.to_string();
    let turn = Arc::new(turn);

    let mut session_arc = Arc::new(session);
    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "create_team",
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
            "send_message",
            function_payload(json!({
                "team_id": team_id,
                "to": "lead",
                "message": "need guidance"
            })),
        ))
        .await
        .expect("send_message should succeed");
    let ToolOutput::Function {
        body: FunctionCallOutputBody::Text(ask_content),
        ..
    } = ask_output
    else {
        panic!("expected function output");
    };

    let ask_result: serde_json::Value =
        serde_json::from_str(&ask_content).expect("send_message result should be json");
    assert_eq!(ask_result["route"].as_str(), Some("ask_lead"));
    assert_eq!(
        ask_result["lead_thread_id"].as_str(),
        Some(lead_thread_id_str.as_str())
    );

    let delivered = ask_result["delivered"].as_bool().expect("delivered bool");
    if delivered {
        assert_eq!(
            ask_result["submission_id"].as_str().map(str::is_empty),
            Some(false)
        );
        assert_eq!(ask_result["error"].as_str(), None);
    } else {
        assert_eq!(
            ask_result["submission_id"].as_str().map(str::is_empty),
            Some(true)
        );
        assert_eq!(ask_result["error"].as_str().map(str::is_empty), Some(false));
    }

    let team_id = ask_result["team_id"].as_str().unwrap_or_default();

    let mut session = unwrap_arc(session_arc, "only one session ref");
    session.conversation_id = lead_thread_id;
    let session = Arc::new(session);

    MultiAgentHandler
        .handle(invocation(
            session,
            turn,
            "delete_team",
            function_payload(json!({"team_id": team_id})),
        ))
        .await
        .expect("delete_team should succeed");
}

#[tokio::test]
async fn send_message_ask_lead_fails_when_called_by_lead() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = thread_manager();
    session.services.agent_control = manager.agent_control();
    let session_arc = Arc::new(session);
    let turn = Arc::new(turn);

    let spawn_output = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "create_team",
            function_payload(json!({
                "team_id": ThreadId::new().to_string(),
                "members": [{"name": "worker", "task": "work"}]
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

    let Err(err) = MultiAgentHandler
        .handle(invocation(
            session_arc.clone(),
            turn.clone(),
            "send_message",
            function_payload(json!({
                "team_id": team_id,
                "to": "lead",
                "message": "hello"
            })),
        ))
        .await
    else {
        panic!("send_message should fail when called by lead with to=lead");
    };
    assert_eq!(
        err,
        FunctionCallError::RespondToModel(
            "send_message cannot be called by the lead when to=lead".to_string()
        )
    );
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
    let sandbox_policy = pick_allowed_sandbox_policy(
        &turn.config.permissions.sandbox_policy,
        turn.config.permissions.sandbox_policy.get().clone(),
    );
    turn.sandbox_policy
        .set(sandbox_policy.clone())
        .expect("sandbox policy set");
    turn.approval_policy
        .set(AskForApproval::OnRequest)
        .expect("approval policy set");

    let config = build_agent_spawn_config(&base_instructions, &turn, 0).expect("spawn config");
    let mut expected = (*turn.config).clone();
    expected.base_instructions = Some(base_instructions.text);
    expected.model = Some(turn.model_info.slug.clone());
    expected.model_provider = turn.provider.clone();
    expected.model_reasoning_effort = turn.reasoning_effort;
    expected.model_reasoning_summary = Some(turn.reasoning_summary);
    expected.developer_instructions = turn.developer_instructions.clone();
    expected.compact_prompt = turn.compact_prompt.clone();
    expected.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    expected.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    expected.cwd = turn.cwd.clone();
    expected
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest)
        .expect("approval policy set");
    expected
        .permissions
        .sandbox_policy
        .set(sandbox_policy)
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
    turn.approval_policy
        .set(AskForApproval::OnRequest)
        .expect("approval policy set");

    let config = build_agent_resume_config(&turn, 0).expect("resume config");

    let mut expected = (*turn.config).clone();
    expected.base_instructions = None;
    expected.model = Some(turn.model_info.slug.clone());
    expected.model_provider = turn.provider.clone();
    expected.model_reasoning_effort = turn.reasoning_effort;
    expected.model_reasoning_summary = Some(turn.reasoning_summary);
    expected.developer_instructions = turn.developer_instructions.clone();
    expected.compact_prompt = turn.compact_prompt.clone();
    expected.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    expected.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    expected.cwd = turn.cwd.clone();
    expected
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest)
        .expect("approval policy set");
    expected
        .permissions
        .sandbox_policy
        .set(turn.sandbox_policy.get().clone())
        .expect("sandbox policy set");
    assert_eq!(config, expected);
}
