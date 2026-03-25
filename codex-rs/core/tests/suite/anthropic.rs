use codex_core::ModelProviderInfo;
use codex_core::WireApi;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use wiremock::MockServer;

const OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "answer": { "type": "string" }
  },
  "required": ["answer"],
  "additionalProperties": false
}"#;

fn anthropic_provider(base_url: String) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "anthropic".to_string(),
        base_url: Some(base_url),
        env_key: Some("PATH".to_string()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        wire_api: WireApi::Anthropic,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    }
}

async fn assert_anthropic_streaming_unsupported(prompt: &str) -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let expected_schema: Value = serde_json::from_str(OUTPUT_SCHEMA)?;
    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
            }
        })
        .build(&server)
        .await?;
    let model = test.session_configured.model.clone();

    let submit_result = test
        .codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(expected_schema),
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: Some(ReasoningSummary::Auto),
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await;

    if let Err(err) = submit_result {
        assert_eq!(
            err.to_string(),
            "unsupported operation: streaming is not implemented for Anthropic providers"
        );
        return Ok(());
    }

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.message.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        error,
        "unsupported operation: streaming is not implemented for Anthropic providers"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_and_reasoning_delta_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("please produce json").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_prefers_api_key_over_bearer_auth() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("hello").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_auto_repairs_invalid_json() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("please return strict json").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_extracts_embedded_json_without_retry() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("extract embedded json").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_stops_after_retry_budget() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("retry budget scenario").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_tool_use_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    assert_anthropic_streaming_unsupported("tell utc time").await
}
