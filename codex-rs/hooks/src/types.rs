use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::models::SandboxPermissions;
use futures::future::BoxFuture;
use serde::Serialize;
use serde::Serializer;

pub type HookFn = Arc<dyn for<'a> Fn(&'a HookPayload) -> BoxFuture<'a, HookResult> + Send + Sync>;

#[derive(Debug)]
pub enum HookResult {
    /// Success: hook completed successfully.
    Success,
    /// FailedContinue: hook failed, but other subsequent hooks should still execute and the
    /// operation should continue.
    FailedContinue(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// FailedAbort: hook failed, other subsequent hooks should not execute, and the operation
    /// should be aborted.
    FailedAbort(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl HookResult {
    pub fn should_abort_operation(&self) -> bool {
        matches!(self, Self::FailedAbort(_))
    }
}

#[derive(Debug)]
pub struct HookResponse {
    pub hook_name: String,
    pub result: HookResult,
}

#[derive(Clone)]
pub struct Hook {
    pub name: String,
    pub func: HookFn,
}

impl Default for Hook {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            func: Arc::new(|_| Box::pin(async { HookResult::Success })),
        }
    }
}

impl Hook {
    pub async fn execute(&self, payload: &HookPayload) -> HookResponse {
        HookResponse {
            hook_name: self.name.clone(),
            result: (self.func)(payload).await,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct HookPayload {
    pub session_id: ThreadId,
    pub cwd: PathBuf,
    #[serde(serialize_with = "serialize_triggered_at")]
    pub triggered_at: DateTime<Utc>,
    pub hook_event: HookEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HookEventAfterAgent {
    pub thread_id: ThreadId,
    pub turn_id: String,
    pub input_messages: Vec<String>,
    pub last_assistant_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookToolKind {
    Function,
    Custom,
    LocalShell,
    Mcp,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolInputLocalShell {
    pub command: Vec<String>,
    pub workdir: Option<String>,
    pub timeout_ms: Option<u64>,
    pub sandbox_permissions: Option<SandboxPermissions>,
    pub prefix_rule: Option<Vec<String>>,
    pub justification: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "input_type", rename_all = "snake_case")]
pub enum HookToolInput {
    Function {
        arguments: String,
    },
    Custom {
        input: String,
    },
    LocalShell {
        params: HookToolInputLocalShell,
    },
    Mcp {
        server: String,
        tool: String,
        arguments: String,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventAfterToolUse {
    pub turn_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub tool_kind: HookToolKind,
    pub tool_input: HookToolInput,
    pub executed: bool,
    pub success: bool,
    pub duration_ms: u64,
    pub mutating: bool,
    pub sandbox: String,
    pub sandbox_policy: String,
    pub output_preview: String,
}

pub type HookEventStop = HookEventAfterAgent;
pub type HookEventSubagentStop = HookEventAfterAgent;
pub type HookEventPostToolUse = HookEventAfterToolUse;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventSessionStart {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventSessionEnd {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventUserPromptSubmit {
    pub turn_id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventPreToolUse {
    pub turn_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub tool_kind: HookToolKind,
    pub tool_input: HookToolInput,
    pub mutating: bool,
    pub sandbox: String,
    pub sandbox_policy: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HookEventPreCompact {
    pub turn_id: String,
    pub model: String,
}

fn serialize_triggered_at<S>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart {
        #[serde(flatten)]
        event: HookEventSessionStart,
    },
    SessionEnd {
        #[serde(flatten)]
        event: HookEventSessionEnd,
    },
    UserPromptSubmit {
        #[serde(flatten)]
        event: HookEventUserPromptSubmit,
    },
    PreToolUse {
        #[serde(flatten)]
        event: HookEventPreToolUse,
    },
    PostToolUse {
        #[serde(flatten)]
        event: HookEventPostToolUse,
    },
    Stop {
        #[serde(flatten)]
        event: HookEventStop,
    },
    SubagentStop {
        #[serde(flatten)]
        event: HookEventSubagentStop,
    },
    PreCompact {
        #[serde(flatten)]
        event: HookEventPreCompact,
    },
    /// Legacy alias for `stop`.
    AfterAgent {
        #[serde(flatten)]
        event: HookEventAfterAgent,
    },
    /// Legacy alias for `post_tool_use`.
    AfterToolUse {
        #[serde(flatten)]
        event: HookEventAfterToolUse,
    },
}

impl HookEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            HookEvent::SessionStart { .. } => "session_start",
            HookEvent::SessionEnd { .. } => "session_end",
            HookEvent::UserPromptSubmit { .. } => "user_prompt_submit",
            HookEvent::PreToolUse { .. } => "pre_tool_use",
            HookEvent::PostToolUse { .. } => "post_tool_use",
            HookEvent::Stop { .. } => "stop",
            HookEvent::SubagentStop { .. } => "subagent_stop",
            HookEvent::PreCompact { .. } => "pre_compact",
            HookEvent::AfterAgent { .. } => "after_agent",
            HookEvent::AfterToolUse { .. } => "after_tool_use",
        }
    }

    pub fn tool_name_for_matcher(&self) -> Option<&str> {
        match self {
            HookEvent::PreToolUse { event } => Some(&event.tool_name),
            HookEvent::PostToolUse { event } => Some(&event.tool_name),
            HookEvent::AfterToolUse { event } => Some(&event.tool_name),
            _ => None,
        }
    }

    pub fn user_prompt_for_matcher(&self) -> Option<&str> {
        match self {
            HookEvent::UserPromptSubmit { event } => Some(&event.prompt),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::TimeZone;
    use chrono::Utc;
    use codex_protocol::ThreadId;
    use codex_protocol::models::SandboxPermissions;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::Hook;
    use super::HookEvent;
    use super::HookEventAfterAgent;
    use super::HookEventAfterToolUse;
    use super::HookEventPreCompact;
    use super::HookEventPreToolUse;
    use super::HookEventSessionEnd;
    use super::HookEventSessionStart;
    use super::HookEventUserPromptSubmit;
    use super::HookPayload;
    use super::HookToolInput;
    use super::HookToolInputLocalShell;
    use super::HookToolKind;

    fn payload_with_event(hook_event: HookEvent) -> HookPayload {
        HookPayload {
            session_id: ThreadId::new(),
            cwd: PathBuf::from("tmp"),
            triggered_at: Utc
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            hook_event,
        }
    }

    fn sample_after_agent_event() -> HookEventAfterAgent {
        HookEventAfterAgent {
            thread_id: ThreadId::new(),
            turn_id: "turn-1".to_string(),
            input_messages: vec!["hello".to_string()],
            last_assistant_message: Some("hi".to_string()),
        }
    }

    fn sample_after_tool_use_event() -> HookEventAfterToolUse {
        HookEventAfterToolUse {
            turn_id: "turn-2".to_string(),
            call_id: "call-1".to_string(),
            tool_name: "local_shell".to_string(),
            tool_kind: HookToolKind::LocalShell,
            tool_input: HookToolInput::LocalShell {
                params: HookToolInputLocalShell {
                    command: vec!["cargo".to_string(), "fmt".to_string()],
                    workdir: Some("codex-rs".to_string()),
                    timeout_ms: Some(60_000),
                    sandbox_permissions: Some(SandboxPermissions::UseDefault),
                    justification: None,
                    prefix_rule: None,
                },
            },
            executed: true,
            success: true,
            duration_ms: 42,
            mutating: true,
            sandbox: "none".to_string(),
            sandbox_policy: "danger-full-access".to_string(),
            output_preview: "ok".to_string(),
        }
    }

    fn sample_pre_tool_use_event() -> HookEventPreToolUse {
        HookEventPreToolUse {
            turn_id: "turn-3".to_string(),
            call_id: "call-2".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_kind: HookToolKind::Function,
            tool_input: HookToolInput::Function {
                arguments: "{}".to_string(),
            },
            mutating: true,
            sandbox: "none".to_string(),
            sandbox_policy: "danger-full-access".to_string(),
        }
    }

    #[test]
    fn hook_payload_serializes_stable_wire_shape() {
        let session_id = ThreadId::new();
        let thread_id = ThreadId::new();
        let payload = payload_with_event(HookEvent::AfterAgent {
            event: HookEventAfterAgent {
                thread_id,
                turn_id: "turn-1".to_string(),
                input_messages: vec!["hello".to_string()],
                last_assistant_message: Some("hi".to_string()),
            },
        });
        let payload = HookPayload {
            session_id,
            ..payload
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": session_id.to_string(),
            "cwd": "tmp",
            "triggered_at": "2025-01-01T00:00:00Z",
            "hook_event": {
                "event_type": "after_agent",
                "thread_id": thread_id.to_string(),
                "turn_id": "turn-1",
                "input_messages": ["hello"],
                "last_assistant_message": "hi",
            },
        });

        assert_eq!(actual, expected);
    }

    #[test]
    fn after_tool_use_payload_serializes_stable_wire_shape() {
        let session_id = ThreadId::new();
        let payload = payload_with_event(HookEvent::AfterToolUse {
            event: sample_after_tool_use_event(),
        });
        let payload = HookPayload {
            session_id,
            ..payload
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": session_id.to_string(),
            "cwd": "tmp",
            "triggered_at": "2025-01-01T00:00:00Z",
            "hook_event": {
                "event_type": "after_tool_use",
                "turn_id": "turn-2",
                "call_id": "call-1",
                "tool_name": "local_shell",
                "tool_kind": "local_shell",
                "tool_input": {
                    "input_type": "local_shell",
                    "params": {
                        "command": ["cargo", "fmt"],
                        "workdir": "codex-rs",
                        "timeout_ms": 60000,
                        "sandbox_permissions": "use_default",
                        "justification": null,
                        "prefix_rule": null,
                    },
                },
                "executed": true,
                "success": true,
                "duration_ms": 42,
                "mutating": true,
                "sandbox": "none",
                "sandbox_policy": "danger-full-access",
                "output_preview": "ok",
            },
        });

        assert_eq!(actual, expected);
    }

    #[test]
    fn user_prompt_submit_payload_serializes_stable_wire_shape() {
        let session_id = ThreadId::new();
        let payload = payload_with_event(HookEvent::UserPromptSubmit {
            event: HookEventUserPromptSubmit {
                turn_id: "turn-3".to_string(),
                prompt: "please run tests".to_string(),
            },
        });
        let payload = HookPayload {
            session_id,
            ..payload
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": session_id.to_string(),
            "cwd": "tmp",
            "triggered_at": "2025-01-01T00:00:00Z",
            "hook_event": {
                "event_type": "user_prompt_submit",
                "turn_id": "turn-3",
                "prompt": "please run tests",
            },
        });

        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn default_hook_executes_successfully() {
        let payload = payload_with_event(HookEvent::AfterAgent {
            event: sample_after_agent_event(),
        });
        let response = Hook::default().execute(&payload).await;

        assert_eq!(response.hook_name, "default");
        assert!(matches!(response.result, super::HookResult::Success));
    }

    #[test]
    fn hook_event_accessors_cover_all_variants() {
        let after_agent_event = sample_after_agent_event();
        let after_tool_use_event = sample_after_tool_use_event();
        let pre_tool_use_event = sample_pre_tool_use_event();
        let cases = vec![
            (
                HookEvent::SessionStart {
                    event: HookEventSessionStart {
                        source: "cli".to_string(),
                    },
                },
                "session_start",
                None,
                None,
            ),
            (
                HookEvent::SessionEnd {
                    event: HookEventSessionEnd {
                        source: "cli".to_string(),
                    },
                },
                "session_end",
                None,
                None,
            ),
            (
                HookEvent::UserPromptSubmit {
                    event: HookEventUserPromptSubmit {
                        turn_id: "turn-user".to_string(),
                        prompt: "ship it".to_string(),
                    },
                },
                "user_prompt_submit",
                None,
                Some("ship it"),
            ),
            (
                HookEvent::PreToolUse {
                    event: pre_tool_use_event,
                },
                "pre_tool_use",
                Some("apply_patch"),
                None,
            ),
            (
                HookEvent::PostToolUse {
                    event: after_tool_use_event.clone(),
                },
                "post_tool_use",
                Some("local_shell"),
                None,
            ),
            (
                HookEvent::Stop {
                    event: after_agent_event.clone(),
                },
                "stop",
                None,
                None,
            ),
            (
                HookEvent::SubagentStop {
                    event: after_agent_event.clone(),
                },
                "subagent_stop",
                None,
                None,
            ),
            (
                HookEvent::PreCompact {
                    event: HookEventPreCompact {
                        turn_id: "turn-compact".to_string(),
                        model: "gpt-5".to_string(),
                    },
                },
                "pre_compact",
                None,
                None,
            ),
            (
                HookEvent::AfterAgent {
                    event: after_agent_event,
                },
                "after_agent",
                None,
                None,
            ),
            (
                HookEvent::AfterToolUse {
                    event: after_tool_use_event,
                },
                "after_tool_use",
                Some("local_shell"),
                None,
            ),
        ];

        for (event, expected_name, expected_tool_name, expected_prompt) in cases {
            assert_eq!(event.event_name(), expected_name);
            assert_eq!(event.tool_name_for_matcher(), expected_tool_name);
            assert_eq!(event.user_prompt_for_matcher(), expected_prompt);
        }
    }

    #[test]
    fn session_start_payload_serializes_stable_wire_shape() {
        let session_id = ThreadId::new();
        let payload = payload_with_event(HookEvent::SessionStart {
            event: HookEventSessionStart {
                source: "cli".to_string(),
            },
        });
        let payload = HookPayload {
            session_id,
            ..payload
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": session_id.to_string(),
            "cwd": "tmp",
            "triggered_at": "2025-01-01T00:00:00Z",
            "hook_event": {
                "event_type": "session_start",
                "source": "cli",
            },
        });

        assert_eq!(actual, expected);
    }

    #[test]
    fn pre_compact_payload_serializes_stable_wire_shape() {
        let session_id = ThreadId::new();
        let payload = payload_with_event(HookEvent::PreCompact {
            event: HookEventPreCompact {
                turn_id: "turn-compact".to_string(),
                model: "gpt-5".to_string(),
            },
        });
        let payload = HookPayload {
            session_id,
            ..payload
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": session_id.to_string(),
            "cwd": "tmp",
            "triggered_at": "2025-01-01T00:00:00Z",
            "hook_event": {
                "event_type": "pre_compact",
                "turn_id": "turn-compact",
                "model": "gpt-5",
            },
        });

        assert_eq!(actual, expected);
    }
}
