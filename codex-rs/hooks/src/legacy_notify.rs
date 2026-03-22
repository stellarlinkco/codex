use std::process::Stdio;
use std::sync::Arc;

use serde::Serialize;

use crate::Hook;
use crate::HookEvent;
use crate::HookPayload;
use crate::HookResponse;
use crate::HookResult;
use crate::command_from_argv;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        turn_id: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        input_messages: Vec<String>,
        last_assistant_message: Option<String>,
    },
}

pub fn legacy_notify_json(payload: &HookPayload) -> Result<String, serde_json::Error> {
    match &payload.hook_event {
        HookEvent::AfterAgent { event } => {
            serde_json::to_string(&UserNotification::AgentTurnComplete {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: payload.cwd.display().to_string(),
                client: payload.client.clone(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
            })
        }
        HookEvent::AfterToolUse { .. } => Err(serde_json::Error::io(std::io::Error::other(
            "legacy notify payload is only supported for after_agent",
        ))),
        _ => Err(serde_json::Error::io(std::io::Error::other(
            "legacy notify payload is only supported for legacy hook events",
        ))),
    }
}

pub fn notify_hook(argv: Vec<String>) -> Hook {
    let argv = Arc::new(argv);
    Hook {
        name: "legacy_notify".to_string(),
        func: Arc::new(move |payload: &HookPayload| {
            let argv = Arc::clone(&argv);
            Box::pin(async move {
                let mut command = match command_from_argv(&argv) {
                    Some(command) => command,
                    None => return HookResult::success(),
                };
                if let Ok(notify_payload) = legacy_notify_json(payload) {
                    command.arg(notify_payload);
                }

                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());

                match command.spawn() {
                    Ok(_) => HookResult::success(),
                    Err(err) => HookResult {
                        error: Some(err.to_string()),
                        ..HookResult::success()
                    },
                }
            })
        }),
    }
}

pub async fn dispatch_legacy_notify(
    argv: &[String],
    payload: &HookPayload,
) -> Option<HookResponse> {
    let hook = notify_hook(argv.to_vec());
    Some(hook.execute(payload).await)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use chrono::TimeZone;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;

    use super::*;
    use crate::HookEventAfterAgent;

    fn expected_notification_json() -> Value {
        json!({
            "type": "agent-turn-complete",
            "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
            "turn-id": "12345",
            "cwd": "/Users/example/project",
            "client": "codex-tui",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds.",
        })
    }

    #[test]
    fn test_user_notification() -> Result<()> {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            client: Some("codex-tui".to_string()),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        };
        let serialized = serde_json::to_string(&notification)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());
        Ok(())
    }

    #[test]
    fn legacy_notify_json_matches_historical_wire_shape() -> Result<()> {
        let payload = HookPayload {
            session_id: ThreadId::new(),
            transcript_path: None,
            cwd: std::path::Path::new("/Users/example/project").to_path_buf(),
            permission_mode: "never".to_string(),
            client: Some("codex-tui".to_string()),
            triggered_at: Some(
                chrono::Utc
                    .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid timestamp"),
            ),
            hook_event: HookEvent::AfterAgent {
                event: HookEventAfterAgent {
                    thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                        .expect("valid thread id"),
                    turn_id: "12345".to_string(),
                    input_messages: vec![
                        "Rename `foo` to `bar` and update the callsites.".to_string(),
                    ],
                    last_assistant_message: Some(
                        "Rename complete and verified `cargo build` succeeds.".to_string(),
                    ),
                },
            },
        };

        let serialized = legacy_notify_json(&payload)?;
        let actual: Value = serde_json::from_str(&serialized)?;
        assert_eq!(actual, expected_notification_json());

        Ok(())
    }
}
