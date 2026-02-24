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
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
use wiremock::matchers::method;
use wiremock::matchers::path;

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
        requires_openai_auth: false,
        supports_websockets: false,
    }
}

fn anthropic_sse(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|event| {
            let event_name = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message_start");
            format!("event: {event_name}\ndata: {event}\n\n")
        })
        .collect::<String>()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_and_reasoning_delta_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let expected_schema: Value = serde_json::from_str(OUTPUT_SCHEMA)?;
    let schema_string = serde_json::to_string(&expected_schema)?;

    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message": {
                "id":"resp-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"thinking_delta","thinking":"first-step"}
        }),
        json!({
            "type":"content_block_start",
            "index":1,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":1,
            "delta":{"type":"text_delta","text":"{\"answer\":\"ok\"}"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":10,"output_tokens":6,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);

    let schema_matcher = move |req: &Request| {
        let body = serde_json::from_slice::<Value>(&req.body).unwrap_or(Value::Null);
        body.get("system")
            .and_then(Value::as_str)
            .map(|system| system.contains(&schema_string))
            .unwrap_or(false)
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(schema_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

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

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "please produce json".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(expected_schema),
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let reasoning = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ReasoningContentDelta(delta) => Some(delta.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reasoning.delta, "first-step");

    let message = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::AgentMessage(msg) => Some(msg.clone()),
        _ => None,
    })
    .await;
    assert_eq!(message.message, "{\"answer\":\"ok\"}");

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_prefers_api_key_over_bearer_auth() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;

    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message": {
                "id":"resp-auth-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"ok"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);

    let auth_matcher = |req: &Request| {
        req.headers.get("x-api-key").is_some() && req.headers.get("authorization").is_none()
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(auth_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let mut provider = anthropic_provider(server.uri());
            provider.experimental_bearer_token = Some("bearer-token".to_string());
            move |config| {
                config.model_provider = provider;
            }
        })
        .build(&server)
        .await?;
    let model = test.session_configured.model.clone();

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let message = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::AgentMessage(msg) => Some(msg.clone()),
        _ => None,
    })
    .await;
    assert_eq!(message.message, "ok");

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_auto_repairs_invalid_json() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let expected_schema: Value = serde_json::from_str(OUTPUT_SCHEMA)?;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-schema-1","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"not-json"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":8,"output_tokens":4,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("please return strict json")
            && !body.contains("Your previous answer did not satisfy the required JSON Schema.")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-schema-2","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"{\"answer\":\"fixed\"}"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":15,"output_tokens":5,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let second_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("Your previous answer did not satisfy the required JSON Schema.")
            && body.contains("assistant output is not valid JSON")
            && body.contains("not-json")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(second_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

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

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "please return strict json".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(expected_schema),
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut messages = Vec::new();
    let mut completed = false;
    for _ in 0..200 {
        let event = tokio::time::timeout(Duration::from_secs(10), test.codex.next_event())
            .await
            .expect("timeout waiting for event")
            .expect("event stream ended unexpectedly")
            .msg;
        match event {
            EventMsg::AgentMessage(message) => messages.push(message.message),
            EventMsg::Error(error) => {
                panic!("anthropic schema auto-repair failed: {}", error.message)
            }
            EventMsg::TurnComplete(_) => {
                completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(completed, "turn did not complete within event budget");
    assert_eq!(messages, vec![r#"{"answer":"fixed"}"#.to_string()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_extracts_embedded_json_without_retry() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let expected_schema: Value = serde_json::from_str(OUTPUT_SCHEMA)?;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-extract-1","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"<think>analysis</think>\n\n{\"answer\":\"from_extract\"}"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":9,"output_tokens":7,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("extract embedded json")
            && !body.contains("Your previous answer did not satisfy the required JSON Schema.")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

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

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "extract embedded json".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(expected_schema),
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut messages = Vec::new();
    let mut message_deltas = Vec::new();
    let mut completed = false;
    for _ in 0..200 {
        let event = tokio::time::timeout(Duration::from_secs(10), test.codex.next_event())
            .await
            .expect("timeout waiting for event")
            .expect("event stream ended unexpectedly")
            .msg;
        match event {
            EventMsg::AgentMessage(message) => messages.push(message.message),
            EventMsg::AgentMessageContentDelta(event) => message_deltas.push(event.delta),
            EventMsg::Error(error) => panic!(
                "anthropic schema extraction path failed unexpectedly: {}",
                error.message
            ),
            EventMsg::TurnComplete(_) => {
                completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(completed, "turn did not complete within event budget");
    assert_eq!(
        message_deltas,
        vec![r#"{"answer":"from_extract"}"#.to_string()]
    );
    assert_eq!(messages, vec![r#"{"answer":"from_extract"}"#.to_string()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_output_schema_stops_after_retry_budget() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let expected_schema: Value = serde_json::from_str(OUTPUT_SCHEMA)?;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-budget-1","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"bad-one"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":9,"output_tokens":3,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("retry budget scenario")
            && !body.contains("Your previous answer did not satisfy the required JSON Schema.")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-budget-2","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"bad-two"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":13,"output_tokens":4,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let second_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("Your previous answer did not satisfy the required JSON Schema.")
            && body.contains("bad-one")
            && !body.contains("bad-two")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(second_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let third = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-budget-3","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"bad-three"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":18,"output_tokens":4,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let third_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("Your previous answer did not satisfy the required JSON Schema.")
            && body.contains("bad-two")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(third_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(third, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

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

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "retry budget scenario".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(expected_schema),
            cwd: test.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut messages = Vec::new();
    let mut error_message = None;
    for _ in 0..300 {
        let event = tokio::time::timeout(Duration::from_secs(10), test.codex.next_event())
            .await
            .expect("timeout waiting for event")
            .expect("event stream ended unexpectedly")
            .msg;
        match event {
            EventMsg::AgentMessage(message) => messages.push(message.message),
            EventMsg::Error(error) => {
                error_message = Some(error.message);
                break;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }

    assert!(
        messages.is_empty(),
        "should not emit invalid assistant output"
    );
    let error_message = error_message.expect("expected schema failure error event");
    assert!(error_message.contains("anthropic output_schema validation failed after retries"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_tool_use_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-tool-1","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{
                "type":"tool_use",
                "id":"call_time_1",
                "name":"time",
                "input":{"utc_offset":"+00:00"}
            }
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"tool_use","stop_sequence":null},
            "usage":{"input_tokens":7,"output_tokens":4,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("tell utc time") && !body.contains("\"type\":\"tool_result\"")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{"id":"resp-tool-2","type":"message","role":"assistant","content":[]}
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"text","text":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"UTC_OK"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":14,"output_tokens":3,"cache_read_input_tokens":0}
        }),
        json!({"type":"message_stop"}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_string_contains("\"type\":\"tool_result\""))
        .and(body_string_contains("\"tool_use_id\":\"call_time_1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "tell utc time".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await?;

    let first_terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            )
        },
        Duration::from_secs(20),
    )
    .await;
    match first_terminal {
        EventMsg::AgentMessage(message) => {
            assert_eq!(message.message, "UTC_OK");
        }
        EventMsg::Error(error) => {
            panic!("anthropic tool round trip failed: {}", error.message);
        }
        EventMsg::TurnComplete(_) => {
            panic!("turn completed before assistant produced UTC_OK");
        }
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}
