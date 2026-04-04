use assert_matches::assert_matches;
use std::sync::Arc;
use std::time::Duration;

use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_sandbox;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_with_timeout;
use regex_lite::Regex;
use serde_json::json;

const TURN_ABORT_TIMEOUT: Duration = Duration::from_secs(60);

async fn submit_user_turn_with_policy(
    fixture: &TestCodex,
    prompt: &str,
    sandbox_policy: codex_protocol::protocol::SandboxPolicy,
) {
    if let Err(err) = fixture
        .codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: fixture.cwd.path().to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy,
            model: fixture.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await
    {
        panic!("submitting user turn with policy should succeed: {err}");
    }
}

/// Integration test: spawn a long‑running shell_command tool via a mocked Responses SSE
/// function call, then interrupt the session and expect TurnAborted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_long_running_tool_emits_turn_aborted() {
    let command = "sleep 60";

    let args = json!({
        "command": command,
        "timeout_ms": 60_000
    })
    .to_string();
    let body = sse(vec![
        ev_function_call("call_sleep", "shell_command", &args),
        ev_completed("done"),
    ]);

    let server = start_mock_server().await;
    mount_sse_once(&server, body).await;

    let codex = test_codex()
        .with_model("gpt-5.1")
        .build(&server)
        .await
        .unwrap()
        .codex;

    // Kick off a turn that triggers the function call.
    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "start sleep".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    // Wait until the exec begins to avoid a race, then interrupt.
    wait_for_event(&codex, |ev| match ev {
        EventMsg::ExecCommandBegin(event) => event.call_id == "call_sleep",
        _ => false,
    })
    .await;

    codex.submit(Op::Interrupt).await.unwrap();

    // Expect TurnAborted soon after.
    wait_for_event_with_timeout(
        &codex,
        |ev| matches!(ev, EventMsg::TurnAborted(_)),
        TURN_ABORT_TIMEOUT,
    )
    .await;
}

/// After an interrupt we expect the next request to the model to include both
/// the original tool call and an `"aborted"` `function_call_output`. This test
/// exercises the follow-up flow: it sends another user turn, inspects the mock
/// responses server, and ensures the model receives the synthesized abort.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_tool_records_history_entries() {
    skip_if_sandbox!();

    let command = "sleep 60";
    let call_id = "call-history";

    let args = json!({
        "command": command,
        "timeout_ms": 60_000
    })
    .to_string();
    let first_body = sse(vec![
        ev_response_created("resp-history"),
        ev_function_call(call_id, "shell_command", &args),
        ev_completed("resp-history"),
    ]);
    let follow_up_body = sse(vec![
        ev_response_created("resp-followup"),
        ev_completed("resp-followup"),
    ]);

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(&server, vec![first_body, follow_up_body]).await;

    let fixture = test_codex()
        .with_model("gpt-5.1")
        .build(&server)
        .await
        .unwrap();
    let codex = Arc::clone(&fixture.codex);

    submit_user_turn_with_policy(
        &fixture,
        "start history recording",
        codex_protocol::protocol::SandboxPolicy::DangerFullAccess,
    )
    .await;

    wait_for_event(&codex, |ev| match ev {
        EventMsg::ExecCommandBegin(event) => event.call_id == call_id,
        _ => false,
    })
    .await;

    codex.submit(Op::Interrupt).await.unwrap();

    wait_for_event_with_timeout(
        &codex,
        |ev| matches!(ev, EventMsg::TurnAborted(_)),
        TURN_ABORT_TIMEOUT,
    )
    .await;
    tokio::time::sleep(Duration::from_secs_f32(0.2)).await;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "follow up".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let requests = response_mock.requests();
    assert!(
        requests.len() == 2,
        "expected two calls to the responses API, got {}",
        requests.len()
    );

    assert!(
        response_mock.saw_function_call(call_id),
        "function call not recorded in responses payload"
    );
    let output = response_mock
        .function_call_output_text(call_id)
        .expect("missing function_call_output text");
    let normalized_output = output.trim().replace("\r\n", "\n");
    let re = Regex::new(
        r"^(?:Wall time: ([0-9]+(?:\.[0-9]+)?) seconds\naborted by user|aborted by user after ([0-9]+(?:\.[0-9]+)?)s)$",
    )
        .expect("compile regex");
    let captures = re.captures(&normalized_output);
    assert_matches!(
        captures.as_ref(),
        Some(caps) if caps.get(1).or_else(|| caps.get(2)).is_some(),
        "aborted message with elapsed seconds: {normalized_output}"
    );
    let captures = captures.expect("aborted message with elapsed seconds");
    let secs: f32 = captures
        .get(1)
        .or_else(|| captures.get(2))
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    assert!(
        secs >= 0.0,
        "expected non-negative elapsed time, got {secs}"
    );
}

/// After an interrupt we persist a model-visible `<turn_aborted>` marker in the conversation
/// history. This test asserts that the marker is included in the next `/responses` request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_persists_turn_aborted_marker_in_next_request() {
    skip_if_sandbox!();

    let command = "sleep 60";
    let call_id = "call-turn-aborted-marker";

    let args = json!({
        "command": command,
        "timeout_ms": 60_000
    })
    .to_string();
    let first_body = sse(vec![
        ev_response_created("resp-marker"),
        ev_function_call(call_id, "shell_command", &args),
        ev_completed("resp-marker"),
    ]);
    let follow_up_body = sse(vec![
        ev_response_created("resp-followup"),
        ev_completed("resp-followup"),
    ]);

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(&server, vec![first_body, follow_up_body]).await;

    let fixture = test_codex()
        .with_model("gpt-5.1")
        .build(&server)
        .await
        .unwrap();
    let codex = Arc::clone(&fixture.codex);

    submit_user_turn_with_policy(
        &fixture,
        "start interrupt marker",
        codex_protocol::protocol::SandboxPolicy::DangerFullAccess,
    )
    .await;

    wait_for_event(&codex, |ev| match ev {
        EventMsg::ExecCommandBegin(event) => event.call_id == call_id,
        _ => false,
    })
    .await;

    codex.submit(Op::Interrupt).await.unwrap();

    wait_for_event_with_timeout(
        &codex,
        |ev| matches!(ev, EventMsg::TurnAborted(_)),
        TURN_ABORT_TIMEOUT,
    )
    .await;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "follow up".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
        })
        .await
        .unwrap();

    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected two calls to the responses API");

    let follow_up_request = &requests[1];
    let user_texts = follow_up_request.message_input_texts("user");
    assert!(
        user_texts
            .iter()
            .any(|text| text.contains("<turn_aborted>")),
        "expected <turn_aborted> marker in follow-up request"
    );
}
