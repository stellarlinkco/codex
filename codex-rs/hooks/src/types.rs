use std::path::PathBuf;

use codex_protocol::ThreadId;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookPayload {
    pub session_id: ThreadId,
    pub transcript_path: Option<PathBuf>,
    pub cwd: PathBuf,
    pub permission_mode: String,
    #[serde(flatten)]
    pub hook_event: HookEvent,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "hook_event_name", rename_all = "PascalCase")]
pub enum HookEvent {
    SessionStart {
        source: String,
        model: String,
        agent_type: Option<String>,
    },
    SessionEnd {
        reason: String,
    },
    UserPromptSubmit {
        prompt: String,
    },
    PreToolUse {
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
    },
    PermissionRequest {
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
        permission_suggestions: Option<Value>,
    },
    Notification {
        message: String,
        title: Option<String>,
        notification_type: String,
    },
    PostToolUse {
        tool_name: String,
        tool_input: Value,
        tool_response: Value,
        tool_use_id: String,
    },
    PostToolUseFailure {
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
        error: String,
        is_interrupt: Option<bool>,
    },
    Stop {
        stop_hook_active: bool,
        last_assistant_message: Option<String>,
    },
    SubagentStart {
        agent_id: String,
        agent_type: String,
    },
    TeammateIdle {
        teammate_name: String,
        team_name: String,
    },
    TaskCompleted {
        task_id: String,
        task_subject: String,
        task_description: Option<String>,
        teammate_name: Option<String>,
        team_name: Option<String>,
    },
    ConfigChange {
        source: String,
        file_path: Option<PathBuf>,
    },
    SubagentStop {
        stop_hook_active: bool,
        agent_id: String,
        agent_type: String,
        agent_transcript_path: Option<PathBuf>,
        last_assistant_message: Option<String>,
    },
    PreCompact {
        trigger: String,
        custom_instructions: Option<String>,
    },
    WorktreeCreate {
        repo_path: PathBuf,
        worktree_path: PathBuf,
    },
    WorktreeRemove {
        repo_path: PathBuf,
        worktree_path: PathBuf,
    },
}

impl HookEvent {
    pub fn matcher_text_for_matcher(&self) -> Option<&str> {
        match self {
            HookEvent::PreToolUse { tool_name, .. }
            | HookEvent::PermissionRequest { tool_name, .. }
            | HookEvent::PostToolUse { tool_name, .. }
            | HookEvent::PostToolUseFailure { tool_name, .. } => Some(tool_name),
            HookEvent::SessionStart { source, .. } => Some(source),
            HookEvent::SessionEnd { reason } => Some(reason),
            HookEvent::Notification {
                notification_type, ..
            } => Some(notification_type),
            HookEvent::SubagentStart { agent_type, .. }
            | HookEvent::SubagentStop { agent_type, .. } => Some(agent_type),
            HookEvent::PreCompact { trigger, .. } => Some(trigger),
            HookEvent::ConfigChange { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn tool_name_for_matcher(&self) -> Option<&str> {
        match self {
            HookEvent::PreToolUse { tool_name, .. }
            | HookEvent::PermissionRequest { tool_name, .. }
            | HookEvent::PostToolUse { tool_name, .. }
            | HookEvent::PostToolUseFailure { tool_name, .. } => Some(tool_name),
            _ => None,
        }
    }

    pub fn user_prompt_for_matcher(&self) -> Option<&str> {
        match self {
            HookEvent::UserPromptSubmit { prompt } => Some(prompt),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPermissionDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResultControl {
    Continue,
    Block { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookResult {
    pub control: HookResultControl,
    pub permission_decision: Option<HookPermissionDecision>,
    pub permission_decision_reason: Option<String>,
    pub updated_input: Option<Value>,
    pub additional_context: Vec<String>,
    pub error: Option<String>,
}

impl HookResult {
    pub fn success() -> Self {
        Self {
            control: HookResultControl::Continue,
            permission_decision: None,
            permission_decision_reason: None,
            updated_input: None,
            additional_context: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookResponse {
    pub hook_name: String,
    pub result: HookResult,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::HookEvent;
    use super::HookPayload;

    #[test]
    fn hook_payload_serializes_flat_event_fields() {
        let session_id =
            ThreadId::from_string("b5f6c1c2-1111-2222-3333-444455556666").expect("valid thread id");
        let payload = HookPayload {
            session_id,
            transcript_path: Some(PathBuf::from("/tmp/transcript.jsonl")),
            cwd: PathBuf::from("/tmp/project"),
            permission_mode: "never".to_string(),
            hook_event: HookEvent::SessionStart {
                source: "cli".to_string(),
                model: "gpt-5".to_string(),
                agent_type: None,
            },
        };

        let actual = serde_json::to_value(payload).expect("serialize hook payload");
        let expected = json!({
            "session_id": "b5f6c1c2-1111-2222-3333-444455556666",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/tmp/project",
            "permission_mode": "never",
            "hook_event_name": "SessionStart",
            "source": "cli",
            "model": "gpt-5",
            "agent_type": null,
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn event_matcher_accessors_cover_variants() {
        assert_eq!(
            HookEvent::SessionStart {
                source: "cli".to_string(),
                model: "gpt-5".to_string(),
                agent_type: None
            }
            .tool_name_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::UserPromptSubmit {
                prompt: "ship it".to_string()
            }
            .user_prompt_for_matcher(),
            Some("ship it")
        );
        assert_eq!(
            HookEvent::PreToolUse {
                tool_name: "shell".to_string(),
                tool_input: json!({"command": ["echo", "hi"]}),
                tool_use_id: "call-1".to_string(),
            }
            .tool_name_for_matcher(),
            Some("shell")
        );
        assert_eq!(
            HookEvent::PermissionRequest {
                tool_name: "exec_command".to_string(),
                tool_input: json!({"cmd": "pwd"}),
                tool_use_id: "call-2".to_string(),
                permission_suggestions: None,
            }
            .tool_name_for_matcher(),
            Some("exec_command")
        );
        assert_eq!(
            HookEvent::PostToolUse {
                tool_name: "parallel".to_string(),
                tool_input: json!({"tool_uses": []}),
                tool_response: json!({"ok": true}),
                tool_use_id: "call-3".to_string(),
            }
            .tool_name_for_matcher(),
            Some("parallel")
        );
        assert_eq!(
            HookEvent::PostToolUseFailure {
                tool_name: "spawn_team".to_string(),
                tool_input: json!({"members": []}),
                tool_use_id: "call-4".to_string(),
                error: "boom".to_string(),
                is_interrupt: Some(false),
            }
            .tool_name_for_matcher(),
            Some("spawn_team")
        );
        assert_eq!(
            HookEvent::SessionEnd {
                reason: "done".to_string()
            }
            .user_prompt_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::WorktreeCreate {
                repo_path: PathBuf::from("/repo"),
                worktree_path: PathBuf::from("/repo-wt"),
            }
            .tool_name_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::TeammateIdle {
                teammate_name: "planner".to_string(),
                team_name: "my-project".to_string(),
            }
            .tool_name_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::Notification {
                message: "Permission needed".to_string(),
                title: Some("Permission needed".to_string()),
                notification_type: "permission_prompt".to_string(),
            }
            .tool_name_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::SubagentStart {
                agent_id: "agent-1".to_string(),
                agent_type: "Explore".to_string(),
            }
            .tool_name_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::TaskCompleted {
                task_id: "task-1".to_string(),
                task_subject: "Ship it".to_string(),
                task_description: None,
                teammate_name: None,
                team_name: None,
            }
            .user_prompt_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::ConfigChange {
                source: "skills".to_string(),
                file_path: None,
            }
            .user_prompt_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::TeammateIdle {
                teammate_name: "planner".to_string(),
                team_name: "my-project".to_string(),
            }
            .user_prompt_for_matcher(),
            None
        );
        assert_eq!(
            HookEvent::WorktreeRemove {
                repo_path: PathBuf::from("/repo"),
                worktree_path: PathBuf::from("/repo-wt"),
            }
            .user_prompt_for_matcher(),
            None
        );
    }
}
