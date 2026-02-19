use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use serde::Serialize;

use crate::Hook;
use crate::HookEvent;
use crate::HookPayload;
use crate::HookResult;
use crate::command_from_argv;

/// Legacy notify payload appended as the final argv argument for backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum UserNotification {
    #[serde(rename_all = "kebab-case")]
    AgentTurnComplete {
        thread_id: String,
        turn_id: String,
        cwd: String,

        /// Messages that the user sent to the agent to initiate the turn.
        input_messages: Vec<String>,

        /// The last message sent by the assistant in the turn.
        last_assistant_message: Option<String>,
    },
}

pub fn legacy_notify_json(hook_event: &HookEvent, cwd: &Path) -> Result<String, serde_json::Error> {
    match hook_event {
        HookEvent::AfterAgent { event }
        | HookEvent::Stop { event }
        | HookEvent::SubagentStop { event } => {
            serde_json::to_string(&UserNotification::AgentTurnComplete {
                thread_id: event.thread_id.to_string(),
                turn_id: event.turn_id.clone(),
                cwd: cwd.display().to_string(),
                input_messages: event.input_messages.clone(),
                last_assistant_message: event.last_assistant_message.clone(),
            })
        }
        _ => Err(serde_json::Error::io(std::io::Error::other(
            "legacy notify payload is only supported for stop/after_agent",
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
                    None => return HookResult::Success,
                };
                if let Ok(notify_payload) = legacy_notify_json(&payload.hook_event, &payload.cwd) {
                    command.arg(notify_payload);
                }

                // Backwards-compat: match legacy notify behavior (argv + JSON arg, fire-and-forget).
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());

                match command.spawn() {
                    Ok(_) => HookResult::Success,
                    Err(err) => HookResult::FailedContinue(err.into()),
                }
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::TimeZone;
    use chrono::Utc;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use serde_json::Value;
    use serde_json::json;

    use super::*;

    #[cfg(not(windows))]
    fn successful_notify_argv() -> Vec<String> {
        vec!["/bin/echo".to_string()]
    }

    #[cfg(windows)]
    fn successful_notify_argv() -> Vec<String> {
        vec![
            "cmd".to_string(),
            "/C".to_string(),
            "echo notification".to_string(),
        ]
    }

    fn payload_with_event(hook_event: HookEvent) -> HookPayload {
        HookPayload {
            session_id: ThreadId::new(),
            cwd: PathBuf::from("/Users/example/project"),
            triggered_at: Utc
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            hook_event,
        }
    }

    fn sample_after_agent_event() -> crate::HookEventAfterAgent {
        crate::HookEventAfterAgent {
            thread_id: ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666")
                .expect("valid thread id"),
            turn_id: "12345".to_string(),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        }
    }

    fn expected_notification_json() -> Value {
        json!({
            "type": "agent-turn-complete",
            "thread-id": "b5f6c1c2-1111-2222-3333-444455556666",
            "turn-id": "12345",
            "cwd": "/Users/example/project",
            "input-messages": ["Rename `foo` to `bar` and update the callsites."],
            "last-assistant-message": "Rename complete and verified `cargo build` succeeds.",
        })
    }

    #[test]
    fn test_user_notification() {
        let notification = UserNotification::AgentTurnComplete {
            thread_id: "b5f6c1c2-1111-2222-3333-444455556666".to_string(),
            turn_id: "12345".to_string(),
            cwd: "/Users/example/project".to_string(),
            input_messages: vec!["Rename `foo` to `bar` and update the callsites.".to_string()],
            last_assistant_message: Some(
                "Rename complete and verified `cargo build` succeeds.".to_string(),
            ),
        };
        let serialized =
            serde_json::to_string(&notification).expect("serialize user notification for test");
        let actual: Value =
            serde_json::from_str(&serialized).expect("parse serialized notification");
        assert_eq!(actual, expected_notification_json());
    }

    #[test]
    fn legacy_notify_json_matches_historical_wire_shape() {
        let hook_event = HookEvent::AfterAgent {
            event: sample_after_agent_event(),
        };

        let serialized = legacy_notify_json(&hook_event, Path::new("/Users/example/project"))
            .expect("serialize legacy notify payload");
        let actual: Value = serde_json::from_str(&serialized).expect("parse legacy payload");
        assert_eq!(actual, expected_notification_json());
    }

    #[test]
    fn legacy_notify_json_supports_stop_and_subagent_stop() {
        let stop_event = HookEvent::Stop {
            event: sample_after_agent_event(),
        };
        let subagent_stop_event = HookEvent::SubagentStop {
            event: sample_after_agent_event(),
        };

        let stop_serialized = legacy_notify_json(&stop_event, Path::new("/Users/example/project"))
            .expect("serialize stop payload");
        let subagent_stop_serialized =
            legacy_notify_json(&subagent_stop_event, Path::new("/Users/example/project"))
                .expect("serialize subagent_stop payload");
        let stop_json: Value =
            serde_json::from_str(&stop_serialized).expect("parse stop payload JSON");
        let subagent_stop_json: Value = serde_json::from_str(&subagent_stop_serialized)
            .expect("parse subagent_stop payload JSON");

        assert_eq!(stop_json, expected_notification_json());
        assert_eq!(subagent_stop_json, expected_notification_json());
    }

    #[test]
    fn legacy_notify_json_rejects_non_turn_events() {
        let event = HookEvent::SessionStart {
            event: crate::HookEventSessionStart {
                source: "cli".to_string(),
            },
        };
        let error = legacy_notify_json(&event, Path::new("/Users/example/project"))
            .expect_err("session_start should be rejected");

        assert!(
            error
                .to_string()
                .contains("legacy notify payload is only supported")
        );
    }

    #[tokio::test]
    async fn notify_hook_returns_success_for_empty_argv() {
        let hook = notify_hook(Vec::new());
        let payload = payload_with_event(HookEvent::AfterAgent {
            event: sample_after_agent_event(),
        });
        let outcome = hook.execute(&payload).await;

        assert!(matches!(outcome.result, HookResult::Success));
    }

    #[tokio::test]
    async fn notify_hook_returns_failed_continue_for_missing_program() {
        let hook = notify_hook(vec!["definitely-not-a-real-program".to_string()]);
        let payload = payload_with_event(HookEvent::AfterAgent {
            event: sample_after_agent_event(),
        });
        let outcome = hook.execute(&payload).await;

        assert!(matches!(outcome.result, HookResult::FailedContinue(_)));
    }

    #[tokio::test]
    async fn notify_hook_spawns_successfully_for_supported_and_unsupported_events() {
        let hook = notify_hook(successful_notify_argv());
        let turn_payload = payload_with_event(HookEvent::Stop {
            event: sample_after_agent_event(),
        });
        let non_turn_payload = payload_with_event(HookEvent::SessionStart {
            event: crate::HookEventSessionStart {
                source: "cli".to_string(),
            },
        });

        let turn_outcome = hook.execute(&turn_payload).await;
        let non_turn_outcome = hook.execute(&non_turn_payload).await;

        assert!(matches!(turn_outcome.result, HookResult::Success));
        assert!(matches!(non_turn_outcome.result, HookResult::Success));
    }
}
