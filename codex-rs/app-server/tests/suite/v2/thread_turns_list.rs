use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadTurnsListParams;
use codex_app_server_protocol::ThreadTurnsListResponse;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

async fn init_mcp(codex_home: &Path) -> Result<McpProcess> {
    let mut mcp = McpProcess::new(codex_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn create_thread_with_turns(mcp: &mut McpProcess, inputs: &[&str]) -> Result<String> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(response)?;

    for input in inputs {
        let request_id = mcp
            .send_turn_start_request(TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: (*input).to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_notification_message("turn/completed"),
        )
        .await??;
    }

    Ok(thread.id)
}

async fn read_all_turns(mcp: &mut McpProcess, thread_id: &str) -> Result<Vec<Turn>> {
    let request_id = mcp
        .send_thread_read_request(ThreadReadParams {
            thread_id: thread_id.to_string(),
            include_turns: true,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadReadResponse { thread } = to_response::<ThreadReadResponse>(response)?;
    Ok(thread.turns)
}

async fn list_thread_turns(
    mcp: &mut McpProcess,
    params: ThreadTurnsListParams,
) -> Result<ThreadTurnsListResponse> {
    let request_id = mcp.send_thread_turns_list_request(params).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadTurnsListResponse>(response)
}

#[tokio::test]
async fn thread_turns_list_pages_forward_and_backward() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = create_thread_with_turns(&mut mcp, &["first", "second", "third"]).await?;
    let expected_turns = read_all_turns(&mut mcp, &thread_id).await?;
    assert_eq!(expected_turns.len(), 3);

    let first_page = list_thread_turns(
        &mut mcp,
        ThreadTurnsListParams {
            thread_id: thread_id.clone(),
            cursor: None,
            backwards_cursor: None,
            limit: Some(2),
        },
    )
    .await?;
    assert_eq!(first_page.data, expected_turns[..2].to_vec());
    assert_eq!(first_page.next_cursor.as_deref(), Some("2"));
    assert_eq!(first_page.backwards_cursor, None);

    let second_page = list_thread_turns(
        &mut mcp,
        ThreadTurnsListParams {
            thread_id: thread_id.clone(),
            cursor: first_page.next_cursor.clone(),
            backwards_cursor: None,
            limit: Some(2),
        },
    )
    .await?;
    assert_eq!(second_page.data, expected_turns[2..].to_vec());
    assert_eq!(second_page.next_cursor, None);
    assert_eq!(second_page.backwards_cursor.as_deref(), Some("0"));

    let previous_page = list_thread_turns(
        &mut mcp,
        ThreadTurnsListParams {
            thread_id,
            cursor: None,
            backwards_cursor: second_page.backwards_cursor.clone(),
            limit: Some(2),
        },
    )
    .await?;
    assert_eq!(previous_page.data, expected_turns[..2].to_vec());
    assert_eq!(previous_page.next_cursor.as_deref(), Some("2"));
    assert_eq!(previous_page.backwards_cursor, None);

    Ok(())
}

#[tokio::test]
async fn thread_turns_list_rejects_mutually_exclusive_cursors() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let thread_id = create_thread_with_turns(&mut mcp, &["first"]).await?;

    let request_id = mcp
        .send_thread_turns_list_request(ThreadTurnsListParams {
            thread_id,
            cursor: Some("0".to_string()),
            backwards_cursor: Some("0".to_string()),
            limit: Some(1),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "cursor and backwardsCursor are mutually exclusive"
    );

    Ok(())
}

#[tokio::test]
async fn thread_turns_list_rejects_unmaterialized_loaded_thread() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = init_mcp(codex_home.path()).await?;
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(response)?;
    let thread_path = thread.path.clone().expect("thread path");
    assert!(
        !thread_path.exists(),
        "fresh thread rollout should not be materialized yet"
    );

    let request_id = mcp
        .send_thread_turns_list_request(ThreadTurnsListParams {
            thread_id: thread.id,
            cursor: None,
            backwards_cursor: None,
            limit: Some(1),
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        error
            .error
            .message
            .contains("includeTurns is unavailable before first user message"),
        "unexpected error: {}",
        error.error.message
    );

    Ok(())
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
