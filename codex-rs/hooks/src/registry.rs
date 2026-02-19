use std::io;
use std::io::ErrorKind;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::types::Hook;
use crate::types::HookEvent;
use crate::types::HookPayload;
use crate::types::HookResponse;
use crate::types::HookResult;

#[derive(Debug, Clone, Default)]
pub struct HookMatcherConfig {
    pub tool_name: Option<String>,
    pub tool_name_regex: Option<String>,
    pub prompt_regex: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandHookConfig {
    pub name: Option<String>,
    pub command: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub run_async: bool,
    pub abort_on_error: bool,
    pub matcher: HookMatcherConfig,
}

#[derive(Debug, Clone, Default)]
pub struct CommandHooksConfig {
    pub session_start: Vec<CommandHookConfig>,
    pub session_end: Vec<CommandHookConfig>,
    pub user_prompt_submit: Vec<CommandHookConfig>,
    pub pre_tool_use: Vec<CommandHookConfig>,
    pub post_tool_use: Vec<CommandHookConfig>,
    pub stop: Vec<CommandHookConfig>,
    pub subagent_stop: Vec<CommandHookConfig>,
    pub pre_compact: Vec<CommandHookConfig>,
    pub after_agent: Vec<CommandHookConfig>,
    pub after_tool_use: Vec<CommandHookConfig>,
}

#[derive(Default, Clone)]
pub struct HooksConfig {
    pub legacy_notify_argv: Option<Vec<String>>,
    pub command_hooks: CommandHooksConfig,
}

#[derive(Clone)]
pub struct Hooks {
    session_start: Vec<Hook>,
    session_end: Vec<Hook>,
    user_prompt_submit: Vec<Hook>,
    pre_tool_use: Vec<Hook>,
    after_tool_use: Vec<Hook>,
    after_agent: Vec<Hook>,
    subagent_stop: Vec<Hook>,
    pre_compact: Vec<Hook>,
}

#[derive(Clone)]
struct CompiledMatcher {
    tool_name: Option<String>,
    tool_name_regex: Option<Regex>,
    prompt_regex: Option<Regex>,
}

impl CompiledMatcher {
    fn compile(matcher: &HookMatcherConfig) -> Result<Self, String> {
        let tool_name_regex =
            compile_optional_regex(matcher.tool_name_regex.as_deref(), "tool_name_regex")?;
        let prompt_regex = compile_optional_regex(matcher.prompt_regex.as_deref(), "prompt_regex")?;
        let tool_name = matcher
            .tool_name
            .as_ref()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(std::string::ToString::to_string);

        Ok(Self {
            tool_name,
            tool_name_regex,
            prompt_regex,
        })
    }

    fn matches(&self, event: &HookEvent) -> bool {
        if let Some(expected_tool_name) = self.tool_name.as_deref()
            && event.tool_name_for_matcher() != Some(expected_tool_name)
        {
            return false;
        }

        if let Some(tool_name_regex) = self.tool_name_regex.as_ref() {
            let Some(tool_name) = event.tool_name_for_matcher() else {
                return false;
            };
            if !tool_name_regex.is_match(tool_name) {
                return false;
            }
        }

        if let Some(prompt_regex) = self.prompt_regex.as_ref() {
            let Some(prompt) = event.user_prompt_for_matcher() else {
                return false;
            };
            if !prompt_regex.is_match(prompt) {
                return false;
            }
        }

        true
    }
}

fn compile_optional_regex(
    pattern: Option<&str>,
    field_name: &str,
) -> Result<Option<Regex>, String> {
    let Some(pattern) = pattern.map(str::trim).filter(|pattern| !pattern.is_empty()) else {
        return Ok(None);
    };

    Regex::new(pattern)
        .map(Some)
        .map_err(|error| format!("invalid {field_name}: {error}"))
}

#[derive(Debug, Clone, Copy)]
enum HookEventKey {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    SubagentStop,
    PreCompact,
}

impl HookEventKey {
    fn as_str(self) -> &'static str {
        match self {
            HookEventKey::SessionStart => "session_start",
            HookEventKey::SessionEnd => "session_end",
            HookEventKey::UserPromptSubmit => "user_prompt_submit",
            HookEventKey::PreToolUse => "pre_tool_use",
            HookEventKey::PostToolUse => "post_tool_use",
            HookEventKey::Stop => "stop",
            HookEventKey::SubagentStop => "subagent_stop",
            HookEventKey::PreCompact => "pre_compact",
        }
    }

    fn supports_block_decision(self) -> bool {
        matches!(
            self,
            HookEventKey::UserPromptSubmit
                | HookEventKey::PreToolUse
                | HookEventKey::Stop
                | HookEventKey::SubagentStop
                | HookEventKey::PreCompact
        )
    }
}

#[derive(Debug, Deserialize)]
struct HookDecisionPayload {
    decision: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedDecision {
    Approve,
    Block,
    Ask,
}

impl Default for Hooks {
    fn default() -> Self {
        Self::new(HooksConfig::default())
    }
}

// Hooks are arbitrary, user-specified functions that are deterministically
// executed for specific events in the Codex lifecycle.
impl Hooks {
    pub fn new(config: HooksConfig) -> Self {
        let HooksConfig {
            legacy_notify_argv,
            mut command_hooks,
        } = config;

        command_hooks.stop.extend(command_hooks.after_agent);
        command_hooks
            .post_tool_use
            .extend(command_hooks.after_tool_use);

        let mut after_agent = build_hooks(command_hooks.stop, HookEventKey::Stop);
        let mut subagent_stop =
            build_hooks(command_hooks.subagent_stop, HookEventKey::SubagentStop);
        if let Some(notify_hook) = legacy_notify_argv
            .filter(|argv| !argv.is_empty() && !argv[0].is_empty())
            .map(crate::notify_hook)
        {
            after_agent.push(notify_hook.clone());
            subagent_stop.push(notify_hook);
        }

        Self {
            session_start: build_hooks(command_hooks.session_start, HookEventKey::SessionStart),
            session_end: build_hooks(command_hooks.session_end, HookEventKey::SessionEnd),
            user_prompt_submit: build_hooks(
                command_hooks.user_prompt_submit,
                HookEventKey::UserPromptSubmit,
            ),
            pre_tool_use: build_hooks(command_hooks.pre_tool_use, HookEventKey::PreToolUse),
            after_tool_use: build_hooks(command_hooks.post_tool_use, HookEventKey::PostToolUse),
            after_agent,
            subagent_stop,
            pre_compact: build_hooks(command_hooks.pre_compact, HookEventKey::PreCompact),
        }
    }

    fn hooks_for_event(&self, hook_event: &HookEvent) -> &[Hook] {
        match hook_event {
            HookEvent::SessionStart { .. } => &self.session_start,
            HookEvent::SessionEnd { .. } => &self.session_end,
            HookEvent::UserPromptSubmit { .. } => &self.user_prompt_submit,
            HookEvent::PreToolUse { .. } => &self.pre_tool_use,
            HookEvent::PostToolUse { .. } | HookEvent::AfterToolUse { .. } => &self.after_tool_use,
            HookEvent::Stop { .. } | HookEvent::AfterAgent { .. } => &self.after_agent,
            HookEvent::SubagentStop { .. } => &self.subagent_stop,
            HookEvent::PreCompact { .. } => &self.pre_compact,
        }
    }

    pub async fn dispatch(&self, hook_payload: HookPayload) -> Vec<HookResponse> {
        let hooks = self.hooks_for_event(&hook_payload.hook_event);
        let mut outcomes = Vec::with_capacity(hooks.len());
        for hook in hooks {
            let outcome = hook.execute(&hook_payload).await;
            let should_abort_operation = outcome.result.should_abort_operation();
            outcomes.push(outcome);
            if should_abort_operation {
                break;
            }
        }

        outcomes
    }
}

fn build_hooks(configs: Vec<CommandHookConfig>, event_key: HookEventKey) -> Vec<Hook> {
    configs
        .into_iter()
        .enumerate()
        .filter_map(|(index, config)| command_hook(config, event_key, index))
        .collect()
}

fn command_hook(config: CommandHookConfig, event_key: HookEventKey, index: usize) -> Option<Hook> {
    if config.command.is_empty() || config.command[0].trim().is_empty() {
        return None;
    }

    let matcher = match CompiledMatcher::compile(&config.matcher) {
        Ok(matcher) => matcher,
        Err(error) => {
            let name = config
                .name
                .unwrap_or_else(|| format!("{}-{}", event_key.as_str(), index + 1));
            return Some(Hook {
                name,
                func: Arc::new(move |_| {
                    let message = error.clone();
                    Box::pin(async move {
                        HookResult::FailedContinue(
                            io::Error::new(ErrorKind::InvalidInput, message).into(),
                        )
                    })
                }),
            });
        }
    };

    let hook_name = config
        .name
        .unwrap_or_else(|| format!("{}-{}", event_key.as_str(), index + 1));
    let command = Arc::new(config.command);
    let timeout_ms = config.timeout_ms;
    let run_async = config.run_async;
    let abort_on_error = config.abort_on_error;
    let supports_block_decision = event_key.supports_block_decision();

    Some(Hook {
        name: hook_name,
        func: Arc::new(move |payload: &HookPayload| {
            let command = Arc::clone(&command);
            let matcher = matcher.clone();
            Box::pin(async move {
                if !matcher.matches(&payload.hook_event) {
                    return HookResult::Success;
                }

                execute_command_hook(
                    payload,
                    command.as_ref(),
                    timeout_ms,
                    run_async,
                    abort_on_error,
                    supports_block_decision,
                )
                .await
            })
        }),
    })
}

async fn execute_command_hook(
    payload: &HookPayload,
    argv: &[String],
    timeout_ms: Option<u64>,
    run_async: bool,
    abort_on_error: bool,
    supports_block_decision: bool,
) -> HookResult {
    let mut command = match command_from_argv(argv) {
        Some(command) => command,
        None => return HookResult::Success,
    };

    command
        .current_dir(&payload.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return hook_error(error, abort_on_error),
    };

    let payload_json = match serde_json::to_string(payload) {
        Ok(payload_json) => payload_json,
        Err(error) => return hook_error(io::Error::other(error), abort_on_error),
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(payload_json.as_bytes()).await
    {
        return hook_error(error, abort_on_error);
    }

    if run_async {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        return HookResult::Success;
    }

    let output = match wait_for_output(child, timeout_ms).await {
        Ok(output) => output,
        Err(error) => return hook_error(error, abort_on_error),
    };

    if supports_block_decision && let Some((decision, reason)) = parse_decision(&output.stdout) {
        return match decision {
            ParsedDecision::Approve => HookResult::Success,
            ParsedDecision::Block => HookResult::FailedAbort(
                io::Error::new(
                    ErrorKind::PermissionDenied,
                    reason.unwrap_or_else(|| "hook blocked operation".to_string()),
                )
                .into(),
            ),
            ParsedDecision::Ask => HookResult::FailedAbort(
                io::Error::new(
                    ErrorKind::PermissionDenied,
                    reason
                        .unwrap_or_else(|| "hook requested an explicit user approval".to_string()),
                )
                .into(),
            ),
        };
    }

    if output.status.success() {
        HookResult::Success
    } else {
        let stderr_preview = preview_bytes(&output.stderr);
        let message = if stderr_preview.is_empty() {
            format!("hook command exited with {}", output.status)
        } else {
            format!(
                "hook command exited with {}: {stderr_preview}",
                output.status
            )
        };
        hook_error(io::Error::other(message), abort_on_error)
    }
}

async fn wait_for_output(
    child: tokio::process::Child,
    timeout_ms: Option<u64>,
) -> io::Result<Output> {
    match timeout_ms {
        Some(timeout_ms) => {
            let duration = Duration::from_millis(timeout_ms);
            tokio::time::timeout(duration, child.wait_with_output())
                .await
                .map_err(|_| io::Error::new(ErrorKind::TimedOut, "hook command timed out"))?
        }
        None => child.wait_with_output().await,
    }
}

fn hook_error(error: io::Error, abort_on_error: bool) -> HookResult {
    if abort_on_error {
        HookResult::FailedAbort(error.into())
    } else {
        HookResult::FailedContinue(error.into())
    }
}

fn preview_bytes(bytes: &[u8]) -> String {
    const PREVIEW_LIMIT: usize = 300;
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    let mut preview = text.chars().take(PREVIEW_LIMIT).collect::<String>();
    if text.chars().count() > PREVIEW_LIMIT {
        preview.push('…');
    }
    preview
}

fn parse_decision(stdout: &[u8]) -> Option<(ParsedDecision, Option<String>)> {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<HookDecisionPayload>(line) else {
            continue;
        };
        let decision = payload.decision?.to_lowercase();
        let parsed_decision = match decision.as_str() {
            "allow" | "approve" | "continue" => ParsedDecision::Approve,
            "block" | "deny" | "abort" => ParsedDecision::Block,
            "ask" => ParsedDecision::Ask,
            _ => continue,
        };
        return Some((parsed_decision, payload.reason));
    }
    None
}

pub fn command_from_argv(argv: &[String]) -> Option<Command> {
    let (program, args) = argv.split_first()?;
    if program.is_empty() {
        return None;
    }
    let mut command = Command::new(program);
    command.args(args);
    Some(command)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use anyhow::Result;
    use chrono::TimeZone;
    use chrono::Utc;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::types::HookEventAfterAgent;
    use crate::types::HookEventAfterToolUse;
    use crate::types::HookEventPostToolUse;
    use crate::types::HookEventPreCompact;
    use crate::types::HookEventPreToolUse;
    use crate::types::HookEventSessionEnd;
    use crate::types::HookEventSessionStart;
    use crate::types::HookEventUserPromptSubmit;
    use crate::types::HookToolInput;
    use crate::types::HookToolKind;

    const CWD: &str = "/tmp";
    const INPUT_MESSAGE: &str = "hello";

    #[cfg(windows)]
    fn echo_command_argv() -> Vec<String> {
        vec![
            "cmd".to_string(),
            "/C".to_string(),
            "echo hello world".to_string(),
        ]
    }

    #[cfg(not(windows))]
    fn echo_command_argv() -> Vec<String> {
        vec!["echo".to_string(), "hello".to_string(), "world".to_string()]
    }

    #[cfg(not(windows))]
    fn shell_argv(script: &str) -> Vec<String> {
        vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()]
    }

    fn payload_with_event(hook_event: HookEvent) -> HookPayload {
        HookPayload {
            session_id: ThreadId::new(),
            cwd: PathBuf::from(CWD),
            triggered_at: Utc
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            hook_event,
        }
    }

    fn hook_payload(label: &str) -> HookPayload {
        payload_with_event(HookEvent::AfterAgent {
            event: HookEventAfterAgent {
                thread_id: ThreadId::new(),
                turn_id: format!("turn-{label}"),
                input_messages: vec![INPUT_MESSAGE.to_string()],
                last_assistant_message: Some("hi".to_string()),
            },
        })
    }

    fn stop_payload(label: &str) -> HookPayload {
        payload_with_event(HookEvent::Stop {
            event: HookEventAfterAgent {
                thread_id: ThreadId::new(),
                turn_id: format!("turn-{label}"),
                input_messages: vec![INPUT_MESSAGE.to_string()],
                last_assistant_message: Some("hi".to_string()),
            },
        })
    }

    fn subagent_stop_payload(label: &str) -> HookPayload {
        payload_with_event(HookEvent::SubagentStop {
            event: HookEventAfterAgent {
                thread_id: ThreadId::new(),
                turn_id: format!("turn-{label}"),
                input_messages: vec![INPUT_MESSAGE.to_string()],
                last_assistant_message: Some("hi".to_string()),
            },
        })
    }

    fn session_start_payload(source: &str) -> HookPayload {
        payload_with_event(HookEvent::SessionStart {
            event: HookEventSessionStart {
                source: source.to_string(),
            },
        })
    }

    fn session_end_payload(source: &str) -> HookPayload {
        payload_with_event(HookEvent::SessionEnd {
            event: HookEventSessionEnd {
                source: source.to_string(),
            },
        })
    }

    fn pre_compact_payload(model: &str) -> HookPayload {
        payload_with_event(HookEvent::PreCompact {
            event: HookEventPreCompact {
                turn_id: "turn-compact".to_string(),
                model: model.to_string(),
            },
        })
    }

    fn post_tool_use_payload(label: &str) -> HookPayload {
        payload_with_event(HookEvent::PostToolUse {
            event: HookEventPostToolUse {
                turn_id: format!("turn-{label}"),
                call_id: format!("call-{label}"),
                tool_name: "apply_patch".to_string(),
                tool_kind: HookToolKind::Custom,
                tool_input: HookToolInput::Custom {
                    input: "*** Begin Patch".to_string(),
                },
                executed: true,
                success: true,
                duration_ms: 1,
                mutating: true,
                sandbox: "none".to_string(),
                sandbox_policy: "danger-full-access".to_string(),
                output_preview: "ok".to_string(),
            },
        })
    }

    fn pre_tool_use_payload(tool_name: &str) -> HookPayload {
        payload_with_event(HookEvent::PreToolUse {
            event: HookEventPreToolUse {
                turn_id: "turn-1".to_string(),
                call_id: "call-1".to_string(),
                tool_name: tool_name.to_string(),
                tool_kind: HookToolKind::Function,
                tool_input: HookToolInput::Function {
                    arguments: "{}".to_string(),
                },
                mutating: false,
                sandbox: "none".to_string(),
                sandbox_policy: "danger-full-access".to_string(),
            },
        })
    }

    fn after_tool_use_payload(label: &str) -> HookPayload {
        payload_with_event(HookEvent::AfterToolUse {
            event: HookEventAfterToolUse {
                turn_id: format!("turn-{label}"),
                call_id: format!("call-{label}"),
                tool_name: "apply_patch".to_string(),
                tool_kind: HookToolKind::Custom,
                tool_input: HookToolInput::Custom {
                    input: "*** Begin Patch".to_string(),
                },
                executed: true,
                success: true,
                duration_ms: 1,
                mutating: true,
                sandbox: "none".to_string(),
                sandbox_policy: "danger-full-access".to_string(),
                output_preview: "ok".to_string(),
            },
        })
    }

    fn user_prompt_payload(prompt: &str) -> HookPayload {
        payload_with_event(HookEvent::UserPromptSubmit {
            event: HookEventUserPromptSubmit {
                turn_id: "turn-user".to_string(),
                prompt: prompt.to_string(),
            },
        })
    }

    fn counting_success_hook(calls: &Arc<AtomicUsize>, name: &str) -> Hook {
        let hook_name = name.to_string();
        let calls = Arc::clone(calls);
        Hook {
            name: hook_name,
            func: Arc::new(move |_| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    HookResult::Success
                })
            }),
        }
    }

    fn failing_abort_hook(calls: &Arc<AtomicUsize>, name: &str, message: &str) -> Hook {
        let hook_name = name.to_string();
        let message = message.to_string();
        let calls = Arc::clone(calls);
        Hook {
            name: hook_name,
            func: Arc::new(move |_| {
                let calls = Arc::clone(&calls);
                let message = message.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    HookResult::FailedAbort(io::Error::other(message).into())
                })
            }),
        }
    }

    #[test]
    fn command_from_argv_returns_none_for_empty_args() {
        assert!(command_from_argv(&[]).is_none());
        assert!(command_from_argv(&["".to_string()]).is_none());
    }

    #[tokio::test]
    async fn command_from_argv_builds_command() -> Result<()> {
        let argv = echo_command_argv();
        let mut command = command_from_argv(&argv).ok_or_else(|| anyhow::anyhow!("command"))?;
        let output = command.output().await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim_end_matches(['\r', '\n']);
        assert_eq!(trimmed, "hello world");
        Ok(())
    }

    #[test]
    fn hooks_new_requires_program_name() {
        assert!(Hooks::new(HooksConfig::default()).after_agent.is_empty());
        assert!(
            Hooks::new(HooksConfig {
                legacy_notify_argv: Some(vec![]),
                ..HooksConfig::default()
            })
            .after_agent
            .is_empty()
        );
        assert!(
            Hooks::new(HooksConfig {
                legacy_notify_argv: Some(vec!["".to_string()]),
                ..HooksConfig::default()
            })
            .after_agent
            .is_empty()
        );
        assert_eq!(
            Hooks::new(HooksConfig {
                legacy_notify_argv: Some(vec!["notify-send".to_string()]),
                ..HooksConfig::default()
            })
            .after_agent
            .len(),
            1
        );
    }

    #[tokio::test]
    async fn dispatch_executes_hook() {
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Hooks {
            after_agent: vec![counting_success_hook(&calls, "counting")],
            ..Hooks::default()
        };

        let outcomes = hooks.dispatch(hook_payload("1")).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].hook_name, "counting");
        assert!(matches!(outcomes[0].result, HookResult::Success));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_stops_when_hook_requests_abort() {
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Hooks {
            after_agent: vec![
                failing_abort_hook(&calls, "abort", "hook failed"),
                counting_success_hook(&calls, "counting"),
            ],
            ..Hooks::default()
        };

        let outcomes = hooks.dispatch(hook_payload("3")).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].hook_name, "abort");
        assert!(matches!(outcomes[0].result, HookResult::FailedAbort(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_executes_after_tool_use_hooks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Hooks {
            after_tool_use: vec![counting_success_hook(&calls, "counting")],
            ..Hooks::default()
        };

        let outcomes = hooks.dispatch(after_tool_use_payload("p")).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].hook_name, "counting");
        assert!(matches!(outcomes[0].result, HookResult::Success));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn parse_decision_supports_block_and_ask() {
        let block = parse_decision(br#"{"decision":"block","reason":"stop"}"#);
        assert_eq!(
            block,
            Some((ParsedDecision::Block, Some("stop".to_string())))
        );

        let ask = parse_decision(br#"{"decision":"ask"}"#);
        assert_eq!(ask, Some((ParsedDecision::Ask, None)));
    }

    #[tokio::test]
    async fn matcher_filters_out_non_matching_tool_names() {
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    name: Some("tool-filter".to_string()),
                    command: vec!["definitely-not-a-command".to_string()],
                    matcher: HookMatcherConfig {
                        tool_name: Some("different_tool".to_string()),
                        ..HookMatcherConfig::default()
                    },
                    ..CommandHookConfig::default()
                }],
                ..CommandHooksConfig::default()
            },
            ..HooksConfig::default()
        });

        let outcomes = hooks.dispatch(pre_tool_use_payload("apply_patch")).await;
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0].result, HookResult::Success));
    }

    #[test]
    fn hook_event_key_names_and_decision_support_are_stable() {
        let cases = [
            (HookEventKey::SessionStart, "session_start", false),
            (HookEventKey::SessionEnd, "session_end", false),
            (HookEventKey::UserPromptSubmit, "user_prompt_submit", true),
            (HookEventKey::PreToolUse, "pre_tool_use", true),
            (HookEventKey::PostToolUse, "post_tool_use", false),
            (HookEventKey::Stop, "stop", true),
            (HookEventKey::SubagentStop, "subagent_stop", true),
            (HookEventKey::PreCompact, "pre_compact", true),
        ];

        for (key, expected_name, expected_block_support) in cases {
            assert_eq!(key.as_str(), expected_name);
            assert_eq!(key.supports_block_decision(), expected_block_support);
        }
    }

    #[test]
    fn compiled_matcher_handles_tool_and_prompt_regex_branches() {
        let tool_name_matcher = CompiledMatcher::compile(&HookMatcherConfig {
            tool_name_regex: Some("^apply_.*$".to_string()),
            ..HookMatcherConfig::default()
        })
        .expect("compile tool matcher");
        let matching_event = pre_tool_use_payload("apply_patch");
        let non_matching_event = pre_tool_use_payload("view_logs");
        let no_tool_event = hook_payload("no-tool");

        assert!(tool_name_matcher.matches(&matching_event.hook_event));
        assert!(!tool_name_matcher.matches(&non_matching_event.hook_event));
        assert!(!tool_name_matcher.matches(&no_tool_event.hook_event));

        let prompt_matcher = CompiledMatcher::compile(&HookMatcherConfig {
            prompt_regex: Some("run tests$".to_string()),
            ..HookMatcherConfig::default()
        })
        .expect("compile prompt matcher");
        let matching_prompt_event = user_prompt_payload("please run tests");
        let non_matching_prompt_event = user_prompt_payload("please run lint");
        let no_prompt_event = pre_tool_use_payload("apply_patch");

        assert!(prompt_matcher.matches(&matching_prompt_event.hook_event));
        assert!(!prompt_matcher.matches(&non_matching_prompt_event.hook_event));
        assert!(!prompt_matcher.matches(&no_prompt_event.hook_event));
    }

    #[tokio::test]
    async fn dispatch_routes_each_event_variant_to_expected_bucket() {
        let counters = HashMap::from([
            ("session_start", Arc::new(AtomicUsize::new(0))),
            ("session_end", Arc::new(AtomicUsize::new(0))),
            ("user_prompt_submit", Arc::new(AtomicUsize::new(0))),
            ("pre_tool_use", Arc::new(AtomicUsize::new(0))),
            ("after_tool_use", Arc::new(AtomicUsize::new(0))),
            ("after_agent", Arc::new(AtomicUsize::new(0))),
            ("subagent_stop", Arc::new(AtomicUsize::new(0))),
            ("pre_compact", Arc::new(AtomicUsize::new(0))),
        ]);
        let hooks = Hooks {
            session_start: vec![counting_success_hook(
                &counters["session_start"],
                "session_start",
            )],
            session_end: vec![counting_success_hook(
                &counters["session_end"],
                "session_end",
            )],
            user_prompt_submit: vec![counting_success_hook(
                &counters["user_prompt_submit"],
                "user_prompt_submit",
            )],
            pre_tool_use: vec![counting_success_hook(
                &counters["pre_tool_use"],
                "pre_tool_use",
            )],
            after_tool_use: vec![counting_success_hook(
                &counters["after_tool_use"],
                "after_tool_use",
            )],
            after_agent: vec![counting_success_hook(
                &counters["after_agent"],
                "after_agent",
            )],
            subagent_stop: vec![counting_success_hook(
                &counters["subagent_stop"],
                "subagent_stop",
            )],
            pre_compact: vec![counting_success_hook(
                &counters["pre_compact"],
                "pre_compact",
            )],
        };

        let cases = vec![
            (session_start_payload("cli"), "session_start"),
            (session_end_payload("cli"), "session_end"),
            (user_prompt_payload("run tests"), "user_prompt_submit"),
            (pre_tool_use_payload("apply_patch"), "pre_tool_use"),
            (post_tool_use_payload("post"), "after_tool_use"),
            (after_tool_use_payload("after"), "after_tool_use"),
            (stop_payload("stop"), "after_agent"),
            (hook_payload("after-agent"), "after_agent"),
            (subagent_stop_payload("subagent"), "subagent_stop"),
            (pre_compact_payload("gpt-5"), "pre_compact"),
        ];

        for (payload, expected_hook_name) in cases {
            let outcomes = hooks.dispatch(payload).await;
            assert_eq!(outcomes.len(), 1);
            assert_eq!(outcomes[0].hook_name, expected_hook_name);
        }

        assert_eq!(counters["session_start"].load(Ordering::SeqCst), 1);
        assert_eq!(counters["session_end"].load(Ordering::SeqCst), 1);
        assert_eq!(counters["user_prompt_submit"].load(Ordering::SeqCst), 1);
        assert_eq!(counters["pre_tool_use"].load(Ordering::SeqCst), 1);
        assert_eq!(counters["after_tool_use"].load(Ordering::SeqCst), 2);
        assert_eq!(counters["after_agent"].load(Ordering::SeqCst), 2);
        assert_eq!(counters["subagent_stop"].load(Ordering::SeqCst), 1);
        assert_eq!(counters["pre_compact"].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn command_hook_requires_non_empty_program() {
        assert!(
            command_hook(
                CommandHookConfig {
                    command: Vec::new(),
                    ..CommandHookConfig::default()
                },
                HookEventKey::Stop,
                0,
            )
            .is_none()
        );
        assert!(
            command_hook(
                CommandHookConfig {
                    command: vec![" ".to_string()],
                    ..CommandHookConfig::default()
                },
                HookEventKey::Stop,
                0,
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn command_hook_with_invalid_regex_returns_failed_continue() {
        let hook = command_hook(
            CommandHookConfig {
                command: vec!["echo".to_string()],
                matcher: HookMatcherConfig {
                    tool_name_regex: Some("[".to_string()),
                    ..HookMatcherConfig::default()
                },
                ..CommandHookConfig::default()
            },
            HookEventKey::PreToolUse,
            0,
        )
        .expect("invalid matcher should still produce a hook");
        let outcome = hook.execute(&pre_tool_use_payload("apply_patch")).await;

        assert_eq!(outcome.hook_name, "pre_tool_use-1");
        match outcome.result {
            HookResult::FailedContinue(error) => {
                let io_error = error.downcast::<io::Error>().expect("io::Error");
                assert_eq!(io_error.kind(), ErrorKind::InvalidInput);
                assert!(io_error.to_string().contains("invalid tool_name_regex"));
            }
            HookResult::Success => panic!("expected failed continue"),
            HookResult::FailedAbort(_) => panic!("expected failed continue"),
        }
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn command_hook_executes_when_matcher_matches() {
        let hook = command_hook(
            CommandHookConfig {
                command: shell_argv("cat >/dev/null; exit 0"),
                matcher: HookMatcherConfig {
                    tool_name_regex: Some("^apply_patch$".to_string()),
                    ..HookMatcherConfig::default()
                },
                ..CommandHookConfig::default()
            },
            HookEventKey::PreToolUse,
            0,
        )
        .expect("valid command hook");
        let outcome = hook.execute(&pre_tool_use_payload("apply_patch")).await;

        assert!(matches!(outcome.result, HookResult::Success));
    }

    fn failure_kind_and_message(
        result: HookResult,
        should_abort: bool,
    ) -> (std::io::ErrorKind, String) {
        let error = match result {
            HookResult::FailedAbort(error) if should_abort => error,
            HookResult::FailedContinue(error) if !should_abort => error,
            HookResult::FailedAbort(_) => panic!("expected failed continue"),
            HookResult::FailedContinue(_) => panic!("expected failed abort"),
            HookResult::Success => panic!("expected failed hook result"),
        };
        let io_error = error.downcast::<io::Error>().expect("io::Error");
        (io_error.kind(), io_error.to_string())
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn execute_command_hook_covers_spawn_timeout_and_exit_paths() {
        let payload = hook_payload("execute");

        assert!(matches!(
            execute_command_hook(&payload, &[], None, false, false, false).await,
            HookResult::Success
        ));

        let missing_program = vec!["definitely-not-a-real-program".to_string()];
        let (kind, _) = failure_kind_and_message(
            execute_command_hook(&payload, &missing_program, None, false, false, false).await,
            false,
        );
        assert_eq!(kind, ErrorKind::NotFound);
        let (kind, _) = failure_kind_and_message(
            execute_command_hook(&payload, &missing_program, None, false, true, false).await,
            true,
        );
        assert_eq!(kind, ErrorKind::NotFound);

        assert!(matches!(
            execute_command_hook(
                &payload,
                &shell_argv("cat >/dev/null; sleep 0.05"),
                None,
                true,
                false,
                false
            )
            .await,
            HookResult::Success
        ));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let (kind, message) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv("sleep 1"),
                Some(1),
                false,
                false,
                false,
            )
            .await,
            false,
        );
        assert_eq!(kind, ErrorKind::TimedOut);
        assert!(message.contains("hook command timed out"));

        let (kind, message) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv("cat >/dev/null; exit 7"),
                None,
                false,
                false,
                false,
            )
            .await,
            false,
        );
        assert_eq!(kind, ErrorKind::Other);
        assert!(message.contains("hook command exited with"));

        let stderr = "x".repeat(320);
        let script = format!("cat >/dev/null; echo {stderr} 1>&2; exit 9");
        let (kind, message) = failure_kind_and_message(
            execute_command_hook(&payload, &shell_argv(&script), None, false, false, false).await,
            false,
        );
        assert_eq!(kind, ErrorKind::Other);
        assert!(message.contains("hook command exited with"));
        assert!(message.ends_with('…'));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn execute_command_hook_reports_write_errors() {
        let payload = user_prompt_payload(&"x".repeat(1_000_000));
        let (kind, _) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv("exec 0<&-; sleep 0.2"),
                None,
                false,
                false,
                false,
            )
            .await,
            false,
        );

        assert_eq!(kind, ErrorKind::BrokenPipe);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[tokio::test]
    async fn execute_command_hook_reports_payload_serialization_errors() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let tmp = tempfile::tempdir().expect("temp dir");
        let invalid_name = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        let invalid_dir = tmp.path().join(PathBuf::from(invalid_name));
        std::fs::create_dir(&invalid_dir).expect("create invalid utf8 dir");
        let payload = HookPayload {
            session_id: ThreadId::new(),
            cwd: invalid_dir,
            triggered_at: Utc
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            hook_event: HookEvent::AfterAgent {
                event: HookEventAfterAgent {
                    thread_id: ThreadId::new(),
                    turn_id: "turn-invalid-cwd".to_string(),
                    input_messages: vec![INPUT_MESSAGE.to_string()],
                    last_assistant_message: Some("hi".to_string()),
                },
            },
        };
        let (kind, message) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv("cat >/dev/null; sleep 0.05"),
                None,
                false,
                false,
                false,
            )
            .await,
            false,
        );

        assert_eq!(kind, ErrorKind::Other);
        assert!(message.contains("path contains invalid UTF-8"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn execute_command_hook_honors_decisions_and_fallbacks() {
        let payload = user_prompt_payload("ship it");

        assert!(matches!(
            execute_command_hook(
                &payload,
                &shell_argv(r#"cat >/dev/null; echo '{"decision":"approve"}'; exit 5"#),
                None,
                false,
                false,
                true
            )
            .await,
            HookResult::Success
        ));

        let (kind, message) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv(r#"cat >/dev/null; echo '{"decision":"block","reason":"no"}'"#),
                None,
                false,
                false,
                true,
            )
            .await,
            true,
        );
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert_eq!(message, "no");

        let (kind, message) = failure_kind_and_message(
            execute_command_hook(
                &payload,
                &shell_argv(r#"cat >/dev/null; echo '{"decision":"ask"}'"#),
                None,
                false,
                false,
                true,
            )
            .await,
            true,
        );
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert_eq!(message, "hook requested an explicit user approval");

        assert!(matches!(
            execute_command_hook(
                &payload,
                &shell_argv(r#"cat >/dev/null; echo 'not json'; echo '{"decision":"unknown"}'"#),
                None,
                false,
                false,
                true
            )
            .await,
            HookResult::Success
        ));
    }

    #[test]
    fn parse_decision_supports_aliases_and_skips_invalid_lines() {
        let approve = parse_decision(b"\nnot-json\n{\"decision\":\"continue\"}\n");
        assert_eq!(approve, Some((ParsedDecision::Approve, None)));

        let deny = parse_decision(br#"{"decision":"deny","reason":"blocked"}"#);
        assert_eq!(
            deny,
            Some((ParsedDecision::Block, Some("blocked".to_string())))
        );

        let unknown = parse_decision(br#"{"decision":"unknown"}"#);
        assert_eq!(unknown, None);

        let missing_decision = parse_decision(br#"{"reason":"missing"}"#);
        assert_eq!(missing_decision, None);
    }

    #[test]
    fn preview_bytes_trims_and_truncates() {
        assert_eq!(preview_bytes(b"  hi there \n"), "hi there");

        let preview = preview_bytes("x".repeat(320).as_bytes());
        assert_eq!(preview.chars().count(), 301);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn hook_error_uses_abort_flag() {
        assert!(matches!(
            hook_error(io::Error::other("continue"), false),
            HookResult::FailedContinue(_)
        ));
        assert!(matches!(
            hook_error(io::Error::other("abort"), true),
            HookResult::FailedAbort(_)
        ));
    }

    #[test]
    #[should_panic(expected = "expected failed continue")]
    fn failure_kind_and_message_panics_for_abort_when_continue_expected() {
        let _ = failure_kind_and_message(
            HookResult::FailedAbort(io::Error::other("abort").into()),
            false,
        );
    }

    #[test]
    #[should_panic(expected = "expected failed abort")]
    fn failure_kind_and_message_panics_for_continue_when_abort_expected() {
        let _ = failure_kind_and_message(
            HookResult::FailedContinue(io::Error::other("continue").into()),
            true,
        );
    }

    #[test]
    #[should_panic(expected = "expected failed hook result")]
    fn failure_kind_and_message_panics_for_success_result() {
        let _ = failure_kind_and_message(HookResult::Success, false);
    }
}
