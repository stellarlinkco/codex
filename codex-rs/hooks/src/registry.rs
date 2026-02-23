use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::io::ErrorKind;
use std::pin::Pin;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::types::HookEvent;
use crate::types::HookPayload;
use crate::types::HookPermissionDecision;
use crate::types::HookResponse;
use crate::types::HookResult;
use crate::types::HookResultControl;

#[derive(Debug, Clone, Default)]
pub struct HookMatcherConfig {
    pub tool_name: Option<String>,
    pub tool_name_regex: Option<String>,
    pub prompt_regex: Option<String>,
    pub matcher: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HookHandlerType {
    #[default]
    Command,
    Prompt,
    Agent,
}

pub trait NonCommandHookExecutor: Send + Sync {
    fn execute_prompt(
        self: Arc<Self>,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> Pin<Box<dyn Future<Output = HookResult> + Send>>;

    fn execute_agent(
        self: Arc<Self>,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> Pin<Box<dyn Future<Output = HookResult> + Send>>;
}

#[derive(Debug, Clone, Default)]
pub struct CommandHookConfig {
    pub name: Option<String>,
    pub handler_type: HookHandlerType,
    pub command: Vec<String>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub async_: bool,
    /// Timeout in seconds.
    pub timeout: Option<u64>,
    pub status_message: Option<String>,
    pub once: bool,
    pub matcher: HookMatcherConfig,
}

#[derive(Debug, Clone, Default)]
pub struct CommandHooksConfig {
    pub session_start: Vec<CommandHookConfig>,
    pub session_end: Vec<CommandHookConfig>,
    pub user_prompt_submit: Vec<CommandHookConfig>,
    pub pre_tool_use: Vec<CommandHookConfig>,
    pub permission_request: Vec<CommandHookConfig>,
    pub notification: Vec<CommandHookConfig>,
    pub post_tool_use: Vec<CommandHookConfig>,
    pub post_tool_use_failure: Vec<CommandHookConfig>,
    pub stop: Vec<CommandHookConfig>,
    pub teammate_idle: Vec<CommandHookConfig>,
    pub task_completed: Vec<CommandHookConfig>,
    pub config_change: Vec<CommandHookConfig>,
    pub subagent_start: Vec<CommandHookConfig>,
    pub subagent_stop: Vec<CommandHookConfig>,
    pub pre_compact: Vec<CommandHookConfig>,
    pub worktree_create: Vec<CommandHookConfig>,
    pub worktree_remove: Vec<CommandHookConfig>,
}

#[derive(Default, Clone)]
pub struct HooksConfig {
    pub command_hooks: CommandHooksConfig,
}

#[derive(Clone)]
enum HookHandler {
    Command {
        argv: Arc<Vec<String>>,
        async_: bool,
    },
    Prompt {
        prompt: Arc<String>,
        model: Option<String>,
    },
    Agent {
        prompt: Arc<String>,
        model: Option<String>,
    },
}

#[derive(Clone)]
struct Hook {
    name: String,
    handler_identity: String,
    handler: HookHandler,
    timeout: Option<Duration>,
    matcher: CompiledMatcher,
    once: bool,
    config_error: Option<String>,
}

#[derive(Clone)]
pub struct Hooks {
    session_start: Vec<Hook>,
    session_end: Vec<Hook>,
    user_prompt_submit: Vec<Hook>,
    pre_tool_use: Vec<Hook>,
    permission_request: Vec<Hook>,
    notification: Vec<Hook>,
    post_tool_use: Vec<Hook>,
    post_tool_use_failure: Vec<Hook>,
    stop: Vec<Hook>,
    teammate_idle: Vec<Hook>,
    task_completed: Vec<Hook>,
    config_change: Vec<Hook>,
    subagent_start: Vec<Hook>,
    subagent_stop: Vec<Hook>,
    pre_compact: Vec<Hook>,
    worktree_create: Vec<Hook>,
    worktree_remove: Vec<Hook>,
    ran_once: Arc<Mutex<HashSet<String>>>,
    async_results_tx: Option<mpsc::UnboundedSender<HookResponse>>,
    non_command_executor: Option<Arc<dyn NonCommandHookExecutor>>,
    scoped_hooks: Arc<std::sync::Mutex<HashMap<String, ScopedHooks>>>,
}

#[derive(Clone, Default)]
struct ScopedHooks {
    session_start: Vec<Hook>,
    session_end: Vec<Hook>,
    user_prompt_submit: Vec<Hook>,
    pre_tool_use: Vec<Hook>,
    permission_request: Vec<Hook>,
    notification: Vec<Hook>,
    post_tool_use: Vec<Hook>,
    post_tool_use_failure: Vec<Hook>,
    stop: Vec<Hook>,
    teammate_idle: Vec<Hook>,
    task_completed: Vec<Hook>,
    config_change: Vec<Hook>,
    subagent_start: Vec<Hook>,
    subagent_stop: Vec<Hook>,
    pre_compact: Vec<Hook>,
    worktree_create: Vec<Hook>,
    worktree_remove: Vec<Hook>,
}

impl ScopedHooks {
    fn hooks_for_event(&self, hook_event: &HookEvent) -> &[Hook] {
        match hook_event {
            HookEvent::SessionStart { .. } => &self.session_start,
            HookEvent::SessionEnd { .. } => &self.session_end,
            HookEvent::UserPromptSubmit { .. } => &self.user_prompt_submit,
            HookEvent::PreToolUse { .. } => &self.pre_tool_use,
            HookEvent::PermissionRequest { .. } => &self.permission_request,
            HookEvent::Notification { .. } => &self.notification,
            HookEvent::PostToolUse { .. } => &self.post_tool_use,
            HookEvent::PostToolUseFailure { .. } => &self.post_tool_use_failure,
            HookEvent::Stop { .. } => &self.stop,
            HookEvent::TeammateIdle { .. } => &self.teammate_idle,
            HookEvent::TaskCompleted { .. } => &self.task_completed,
            HookEvent::ConfigChange { .. } => &self.config_change,
            HookEvent::SubagentStart { .. } => &self.subagent_start,
            HookEvent::SubagentStop { .. } => &self.subagent_stop,
            HookEvent::PreCompact { .. } => &self.pre_compact,
            HookEvent::WorktreeCreate { .. } => &self.worktree_create,
            HookEvent::WorktreeRemove { .. } => &self.worktree_remove,
        }
    }
}

#[derive(Clone)]
struct CompiledMatcher {
    tool_name: Option<String>,
    tool_name_regex: Option<Regex>,
    prompt_regex: Option<Regex>,
    matcher_regex: Option<Regex>,
}

impl CompiledMatcher {
    fn compile(matcher: &HookMatcherConfig) -> Result<Self, String> {
        let matcher_regex = compile_matcher_regex(matcher.matcher.as_deref())?;
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
            matcher_regex,
        })
    }

    fn matches(&self, event: &HookEvent) -> bool {
        if let Some(matcher_regex) = self.matcher_regex.as_ref() {
            let Some(matcher_text) = event.matcher_text_for_matcher() else {
                return false;
            };
            if !matcher_regex.is_match(matcher_text) {
                return false;
            }
        }

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

fn compile_matcher_regex(pattern: Option<&str>) -> Result<Option<Regex>, String> {
    let Some(pattern) = pattern.map(str::trim).filter(|pattern| !pattern.is_empty()) else {
        return Ok(None);
    };
    if pattern == "*" {
        return Ok(None);
    }
    Regex::new(pattern)
        .map(Some)
        .map_err(|error| format!("invalid matcher: {error}"))
}

#[derive(Debug, Clone, Copy)]
enum HookEventKey {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PermissionRequest,
    Notification,
    PostToolUse,
    PostToolUseFailure,
    Stop,
    TeammateIdle,
    TaskCompleted,
    ConfigChange,
    SubagentStart,
    SubagentStop,
    PreCompact,
    WorktreeCreate,
    WorktreeRemove,
}

impl HookEventKey {
    fn as_str(self) -> &'static str {
        match self {
            HookEventKey::SessionStart => "session_start",
            HookEventKey::SessionEnd => "session_end",
            HookEventKey::UserPromptSubmit => "user_prompt_submit",
            HookEventKey::PreToolUse => "pre_tool_use",
            HookEventKey::PermissionRequest => "permission_request",
            HookEventKey::Notification => "notification",
            HookEventKey::PostToolUse => "post_tool_use",
            HookEventKey::PostToolUseFailure => "post_tool_use_failure",
            HookEventKey::Stop => "stop",
            HookEventKey::TeammateIdle => "teammate_idle",
            HookEventKey::TaskCompleted => "task_completed",
            HookEventKey::ConfigChange => "config_change",
            HookEventKey::SubagentStart => "subagent_start",
            HookEventKey::SubagentStop => "subagent_stop",
            HookEventKey::PreCompact => "pre_compact",
            HookEventKey::WorktreeCreate => "worktree_create",
            HookEventKey::WorktreeRemove => "worktree_remove",
        }
    }

    fn supports_exit_2_block(self) -> bool {
        matches!(
            self,
            HookEventKey::UserPromptSubmit
                | HookEventKey::PreToolUse
                | HookEventKey::PermissionRequest
                | HookEventKey::Stop
                | HookEventKey::TeammateIdle
                | HookEventKey::TaskCompleted
                | HookEventKey::ConfigChange
                | HookEventKey::SubagentStop
        )
    }

    fn supports_output_decisions(self) -> bool {
        matches!(
            self,
            HookEventKey::UserPromptSubmit
                | HookEventKey::PreToolUse
                | HookEventKey::PermissionRequest
                | HookEventKey::Stop
                | HookEventKey::SubagentStop
                | HookEventKey::ConfigChange
        )
    }

    fn supports_prompt_and_agent_hooks(self) -> bool {
        matches!(
            self,
            HookEventKey::PermissionRequest
                | HookEventKey::PostToolUse
                | HookEventKey::PostToolUseFailure
                | HookEventKey::PreToolUse
                | HookEventKey::Stop
                | HookEventKey::SubagentStop
                | HookEventKey::TaskCompleted
                | HookEventKey::UserPromptSubmit
        )
    }

    fn supports_matchers(self) -> bool {
        matches!(
            self,
            HookEventKey::PreToolUse
                | HookEventKey::PostToolUse
                | HookEventKey::PostToolUseFailure
                | HookEventKey::PermissionRequest
                | HookEventKey::SessionStart
                | HookEventKey::SessionEnd
                | HookEventKey::Notification
                | HookEventKey::SubagentStart
                | HookEventKey::PreCompact
                | HookEventKey::SubagentStop
                | HookEventKey::ConfigChange
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedDecision {
    Allow,
    Deny,
    Ask,
}

impl Default for Hooks {
    fn default() -> Self {
        Self::new(HooksConfig::default())
    }
}

// Hooks are arbitrary, user-specified command handlers that are deterministically
// executed for specific events in the Codex lifecycle.
impl Hooks {
    pub fn new(config: HooksConfig) -> Self {
        let HooksConfig { command_hooks } = config;
        Self {
            session_start: build_hooks(command_hooks.session_start, HookEventKey::SessionStart),
            session_end: build_hooks(command_hooks.session_end, HookEventKey::SessionEnd),
            user_prompt_submit: build_hooks(
                command_hooks.user_prompt_submit,
                HookEventKey::UserPromptSubmit,
            ),
            pre_tool_use: build_hooks(command_hooks.pre_tool_use, HookEventKey::PreToolUse),
            permission_request: build_hooks(
                command_hooks.permission_request,
                HookEventKey::PermissionRequest,
            ),
            notification: build_hooks(command_hooks.notification, HookEventKey::Notification),
            post_tool_use: build_hooks(command_hooks.post_tool_use, HookEventKey::PostToolUse),
            post_tool_use_failure: build_hooks(
                command_hooks.post_tool_use_failure,
                HookEventKey::PostToolUseFailure,
            ),
            stop: build_hooks(command_hooks.stop, HookEventKey::Stop),
            teammate_idle: build_hooks(command_hooks.teammate_idle, HookEventKey::TeammateIdle),
            task_completed: build_hooks(command_hooks.task_completed, HookEventKey::TaskCompleted),
            config_change: build_hooks(command_hooks.config_change, HookEventKey::ConfigChange),
            subagent_start: build_hooks(command_hooks.subagent_start, HookEventKey::SubagentStart),
            subagent_stop: build_hooks(command_hooks.subagent_stop, HookEventKey::SubagentStop),
            pre_compact: build_hooks(command_hooks.pre_compact, HookEventKey::PreCompact),
            worktree_create: build_hooks(
                command_hooks.worktree_create,
                HookEventKey::WorktreeCreate,
            ),
            worktree_remove: build_hooks(
                command_hooks.worktree_remove,
                HookEventKey::WorktreeRemove,
            ),
            ran_once: Arc::new(Mutex::new(HashSet::new())),
            async_results_tx: None,
            non_command_executor: None,
            scoped_hooks: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn set_async_results_tx(&mut self, tx: mpsc::UnboundedSender<HookResponse>) {
        self.async_results_tx = Some(tx);
    }

    pub fn set_non_command_executor(&mut self, executor: Arc<dyn NonCommandHookExecutor>) {
        self.non_command_executor = Some(executor);
    }

    pub fn insert_scoped_command_hooks(&self, scope_id: String, command_hooks: CommandHooksConfig) {
        let scoped = build_scoped_hooks(&scope_id, command_hooks);
        let mut guard = self
            .scoped_hooks
            .lock()
            .expect("scoped_hooks mutex poisoned");
        guard.insert(scope_id, scoped);
    }

    pub fn remove_scoped_hooks(&self, scope_id: &str) {
        let mut guard = self
            .scoped_hooks
            .lock()
            .expect("scoped_hooks mutex poisoned");
        guard.remove(scope_id);
    }

    fn hooks_for_event(&self, hook_event: &HookEvent) -> (HookEventKey, &[Hook]) {
        match hook_event {
            HookEvent::SessionStart { .. } => (HookEventKey::SessionStart, &self.session_start),
            HookEvent::SessionEnd { .. } => (HookEventKey::SessionEnd, &self.session_end),
            HookEvent::UserPromptSubmit { .. } => {
                (HookEventKey::UserPromptSubmit, &self.user_prompt_submit)
            }
            HookEvent::PreToolUse { .. } => (HookEventKey::PreToolUse, &self.pre_tool_use),
            HookEvent::PermissionRequest { .. } => {
                (HookEventKey::PermissionRequest, &self.permission_request)
            }
            HookEvent::Notification { .. } => (HookEventKey::Notification, &self.notification),
            HookEvent::PostToolUse { .. } => (HookEventKey::PostToolUse, &self.post_tool_use),
            HookEvent::PostToolUseFailure { .. } => (
                HookEventKey::PostToolUseFailure,
                &self.post_tool_use_failure,
            ),
            HookEvent::Stop { .. } => (HookEventKey::Stop, &self.stop),
            HookEvent::TeammateIdle { .. } => (HookEventKey::TeammateIdle, &self.teammate_idle),
            HookEvent::TaskCompleted { .. } => (HookEventKey::TaskCompleted, &self.task_completed),
            HookEvent::ConfigChange { .. } => (HookEventKey::ConfigChange, &self.config_change),
            HookEvent::SubagentStart { .. } => (HookEventKey::SubagentStart, &self.subagent_start),
            HookEvent::SubagentStop { .. } => (HookEventKey::SubagentStop, &self.subagent_stop),
            HookEvent::PreCompact { .. } => (HookEventKey::PreCompact, &self.pre_compact),
            HookEvent::WorktreeCreate { .. } => {
                (HookEventKey::WorktreeCreate, &self.worktree_create)
            }
            HookEvent::WorktreeRemove { .. } => {
                (HookEventKey::WorktreeRemove, &self.worktree_remove)
            }
        }
    }

    fn scoped_hooks_for_event(&self, hook_event: &HookEvent) -> Vec<Hook> {
        let guard = self
            .scoped_hooks
            .lock()
            .expect("scoped_hooks mutex poisoned");
        let mut entries = guard.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(scope_id, _)| scope_id.as_str());

        entries
            .into_iter()
            .flat_map(|(_scope_id, scoped)| scoped.hooks_for_event(hook_event).iter().cloned())
            .collect()
    }

    pub async fn dispatch(&self, hook_payload: HookPayload) -> Vec<HookResponse> {
        let (event_key, hooks) = self.hooks_for_event(&hook_payload.hook_event);
        let scoped_hooks = self.scoped_hooks_for_event(&hook_payload.hook_event);
        let mut seen = HashSet::new();
        let mut outcomes = Vec::with_capacity(hooks.len() + scoped_hooks.len());
        let mut hook_names_by_outcome_index = Vec::with_capacity(hooks.len() + scoped_hooks.len());
        let mut join_set = JoinSet::new();

        for hook in hooks.iter().chain(scoped_hooks.iter()) {
            if let Some(error) = hook.config_error.as_deref() {
                outcomes.push(Some(HookResponse {
                    hook_name: hook.name.clone(),
                    result: HookResult {
                        error: Some(error.to_string()),
                        ..HookResult::success()
                    },
                }));
                hook_names_by_outcome_index.push(None);
                continue;
            }

            if event_key.supports_matchers() && !hook.matcher.matches(&hook_payload.hook_event) {
                continue;
            }

            if !seen.insert(hook.handler_identity.clone()) {
                continue;
            }

            if hook.once {
                let mut guard = self.ran_once.lock().await;
                if !guard.insert(hook.handler_identity.clone()) {
                    continue;
                }
            }

            let outcome_index = outcomes.len();
            outcomes.push(None);
            hook_names_by_outcome_index.push(Some(hook.name.clone()));

            let payload = hook_payload.clone();
            let hook_name = hook.name.clone();
            let handler = hook.handler.clone();
            let timeout = hook.timeout;
            let async_results_tx = self.async_results_tx.clone();
            let non_command_executor = self.non_command_executor.clone();

            join_set.spawn(async move {
                let result = match handler {
                    HookHandler::Command { argv, async_ } => {
                        if async_ {
                            if let Some(tx) = async_results_tx {
                                let payload = payload.clone();
                                let hook_name = hook_name.clone();
                                let argv = Arc::clone(&argv);
                                tokio::spawn(async move {
                                    let mut result =
                                        execute_command_hook(&payload, &argv, timeout, event_key)
                                            .await;
                                    result.control = HookResultControl::Continue;
                                    result.permission_decision = None;
                                    result.permission_decision_reason = None;
                                    let _ = tx.send(HookResponse { hook_name, result });
                                });
                            }
                            HookResult::success()
                        } else {
                            execute_command_hook(&payload, &argv, timeout, event_key).await
                        }
                    }
                    HookHandler::Prompt { prompt, model } => match non_command_executor {
                        Some(executor) => {
                            executor
                                .execute_prompt(
                                    payload.clone(),
                                    prompt.as_ref().to_string(),
                                    model.clone(),
                                    timeout,
                                )
                                .await
                        }
                        None => HookResult {
                            error: Some("prompt hooks are not configured".to_string()),
                            ..HookResult::success()
                        },
                    },
                    HookHandler::Agent { prompt, model } => match non_command_executor {
                        Some(executor) => {
                            executor
                                .execute_agent(
                                    payload.clone(),
                                    prompt.as_ref().to_string(),
                                    model.clone(),
                                    timeout,
                                )
                                .await
                        }
                        None => HookResult {
                            error: Some("agent hooks are not configured".to_string()),
                            ..HookResult::success()
                        },
                    },
                };

                (outcome_index, HookResponse { hook_name, result })
            });
        }

        while let Some(joined) = join_set.join_next().await {
            if let Ok((index, response)) = joined
                && let Some(slot) = outcomes.get_mut(index)
            {
                *slot = Some(response);
            }
        }

        for (index, slot) in outcomes.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let hook_name = hook_names_by_outcome_index
                .get(index)
                .and_then(Option::as_deref)
                .unwrap_or("<unknown>");
            *slot = Some(HookResponse {
                hook_name: hook_name.to_string(),
                result: HookResult {
                    error: Some("hook task failed".to_string()),
                    ..HookResult::success()
                },
            });
        }

        outcomes.into_iter().flatten().collect()
    }
}

fn build_hooks(configs: Vec<CommandHookConfig>, event_key: HookEventKey) -> Vec<Hook> {
    configs
        .into_iter()
        .enumerate()
        .filter_map(|(index, config)| hook_from_config(config, event_key, index, None))
        .collect()
}

fn hook_from_config(
    config: CommandHookConfig,
    event_key: HookEventKey,
    index: usize,
    once_key_prefix: Option<&str>,
) -> Option<Hook> {
    let once = config.once;
    let base_once_key = format!("{}-{}", event_key.as_str(), index + 1);
    let once_key = match once_key_prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{base_once_key}"),
        _ => base_once_key,
    };
    let name = config.name.unwrap_or_else(|| once_key.clone());
    let timeout = config.timeout.map(Duration::from_secs);
    let (matcher, config_error) = if event_key.supports_matchers() {
        match CompiledMatcher::compile(&config.matcher) {
            Ok(matcher) => (matcher, None),
            Err(error) => (
                CompiledMatcher {
                    tool_name: None,
                    tool_name_regex: None,
                    prompt_regex: None,
                    matcher_regex: None,
                },
                Some(error),
            ),
        }
    } else {
        (
            CompiledMatcher {
                tool_name: None,
                tool_name_regex: None,
                prompt_regex: None,
                matcher_regex: None,
            },
            None,
        )
    };

    let (handler, handler_error) = match config.handler_type {
        HookHandlerType::Command => {
            if config.command.is_empty() || config.command[0].trim().is_empty() {
                return None;
            }
            (
                HookHandler::Command {
                    argv: Arc::new(config.command),
                    async_: config.async_,
                },
                None,
            )
        }
        HookHandlerType::Prompt => {
            let Some(prompt) = config
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            else {
                return None;
            };
            if !event_key.supports_prompt_and_agent_hooks() {
                (
                    HookHandler::Command {
                        argv: Arc::new(Vec::new()),
                        async_: false,
                    },
                    Some(format!(
                        "prompt hooks are not supported for {}",
                        event_key.as_str()
                    )),
                )
            } else {
                (
                    HookHandler::Prompt {
                        prompt: Arc::new(prompt.to_string()),
                        model: config.model.clone(),
                    },
                    None,
                )
            }
        }
        HookHandlerType::Agent => {
            let Some(prompt) = config
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            else {
                return None;
            };
            if !event_key.supports_prompt_and_agent_hooks() {
                (
                    HookHandler::Command {
                        argv: Arc::new(Vec::new()),
                        async_: false,
                    },
                    Some(format!(
                        "agent hooks are not supported for {}",
                        event_key.as_str()
                    )),
                )
            } else {
                (
                    HookHandler::Agent {
                        prompt: Arc::new(prompt.to_string()),
                        model: config.model.clone(),
                    },
                    None,
                )
            }
        }
    };

    let config_error = config_error.or(handler_error);
    let handler_identity = hook_handler_identity(event_key, &handler, timeout, once);

    Some(Hook {
        name,
        handler_identity,
        handler,
        timeout,
        matcher,
        once,
        config_error,
    })
}

fn hook_handler_identity(
    event_key: HookEventKey,
    handler: &HookHandler,
    timeout: Option<Duration>,
    once: bool,
) -> String {
    let timeout_secs = timeout.map(|duration| duration.as_secs());
    let timeout_key = timeout_secs.map_or_else(|| "none".to_string(), |secs| secs.to_string());

    match handler {
        HookHandler::Command { argv, async_ } => {
            let argv_json =
                serde_json::to_string(argv.as_ref()).unwrap_or_else(|_| "[]".to_string());
            format!(
                "{}|command|async={async_}|timeout={timeout_key}|once={once}|argv={argv_json}",
                event_key.as_str(),
            )
        }
        HookHandler::Prompt { prompt, model } => {
            let prompt_json = serde_json::to_string(prompt.as_ref()).unwrap_or_default();
            let model_json = serde_json::to_string(model).unwrap_or_else(|_| "null".to_string());
            format!(
                "{}|prompt|timeout={timeout_key}|once={once}|model={model_json}|prompt={prompt_json}",
                event_key.as_str(),
            )
        }
        HookHandler::Agent { prompt, model } => {
            let prompt_json = serde_json::to_string(prompt.as_ref()).unwrap_or_default();
            let model_json = serde_json::to_string(model).unwrap_or_else(|_| "null".to_string());
            format!(
                "{}|agent|timeout={timeout_key}|once={once}|model={model_json}|prompt={prompt_json}",
                event_key.as_str(),
            )
        }
    }
}

fn build_scoped_hooks(scope_id: &str, command_hooks: CommandHooksConfig) -> ScopedHooks {
    ScopedHooks {
        session_start: build_hooks_with_prefix(
            scope_id,
            command_hooks.session_start,
            HookEventKey::SessionStart,
        ),
        session_end: build_hooks_with_prefix(
            scope_id,
            command_hooks.session_end,
            HookEventKey::SessionEnd,
        ),
        user_prompt_submit: build_hooks_with_prefix(
            scope_id,
            command_hooks.user_prompt_submit,
            HookEventKey::UserPromptSubmit,
        ),
        pre_tool_use: build_hooks_with_prefix(
            scope_id,
            command_hooks.pre_tool_use,
            HookEventKey::PreToolUse,
        ),
        permission_request: build_hooks_with_prefix(
            scope_id,
            command_hooks.permission_request,
            HookEventKey::PermissionRequest,
        ),
        notification: build_hooks_with_prefix(
            scope_id,
            command_hooks.notification,
            HookEventKey::Notification,
        ),
        post_tool_use: build_hooks_with_prefix(
            scope_id,
            command_hooks.post_tool_use,
            HookEventKey::PostToolUse,
        ),
        post_tool_use_failure: build_hooks_with_prefix(
            scope_id,
            command_hooks.post_tool_use_failure,
            HookEventKey::PostToolUseFailure,
        ),
        stop: build_hooks_with_prefix(scope_id, command_hooks.stop, HookEventKey::Stop),
        teammate_idle: build_hooks_with_prefix(
            scope_id,
            command_hooks.teammate_idle,
            HookEventKey::TeammateIdle,
        ),
        task_completed: build_hooks_with_prefix(
            scope_id,
            command_hooks.task_completed,
            HookEventKey::TaskCompleted,
        ),
        config_change: build_hooks_with_prefix(
            scope_id,
            command_hooks.config_change,
            HookEventKey::ConfigChange,
        ),
        subagent_start: build_hooks_with_prefix(
            scope_id,
            command_hooks.subagent_start,
            HookEventKey::SubagentStart,
        ),
        subagent_stop: build_hooks_with_prefix(
            scope_id,
            command_hooks.subagent_stop,
            HookEventKey::SubagentStop,
        ),
        pre_compact: build_hooks_with_prefix(
            scope_id,
            command_hooks.pre_compact,
            HookEventKey::PreCompact,
        ),
        worktree_create: build_hooks_with_prefix(
            scope_id,
            command_hooks.worktree_create,
            HookEventKey::WorktreeCreate,
        ),
        worktree_remove: build_hooks_with_prefix(
            scope_id,
            command_hooks.worktree_remove,
            HookEventKey::WorktreeRemove,
        ),
    }
}

fn build_hooks_with_prefix(
    scope_id: &str,
    configs: Vec<CommandHookConfig>,
    event_key: HookEventKey,
) -> Vec<Hook> {
    configs
        .into_iter()
        .enumerate()
        .filter_map(|(index, config)| hook_from_config(config, event_key, index, Some(scope_id)))
        .collect()
}

async fn execute_command_hook(
    payload: &HookPayload,
    argv: &Arc<Vec<String>>,
    timeout: Option<Duration>,
    event_key: HookEventKey,
) -> HookResult {
    let mut command = match command_from_argv(argv.as_ref()) {
        Some(command) => command,
        None => return HookResult::success(),
    };

    command
        .current_dir(&payload.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return result_with_error(error),
    };

    let payload_json = match serde_json::to_string(payload) {
        Ok(payload_json) => payload_json,
        Err(error) => return result_with_error(io::Error::other(error)),
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(payload_json.as_bytes()).await
        && error.kind() != ErrorKind::BrokenPipe
    {
        return result_with_error(error);
    }

    let output = match wait_for_output(child, timeout).await {
        Ok(output) => output,
        Err(error) => return result_with_error(error),
    };

    result_from_output(event_key, &output)
}

async fn wait_for_output(
    child: tokio::process::Child,
    timeout: Option<Duration>,
) -> io::Result<Output> {
    match timeout {
        Some(duration) => tokio::time::timeout(duration, child.wait_with_output())
            .await
            .map_err(|_| io::Error::new(ErrorKind::TimedOut, "hook command timed out"))?,
        None => child.wait_with_output().await,
    }
}

fn result_from_output(event_key: HookEventKey, output: &Output) -> HookResult {
    let Some(code) = output.status.code() else {
        return HookResult {
            error: Some("hook command terminated by signal".to_string()),
            ..HookResult::success()
        };
    };

    if code == 2 {
        let message = preview_bytes(&output.stderr);
        if event_key.supports_exit_2_block() {
            let reason = if message.is_empty() {
                "hook blocked operation".to_string()
            } else {
                message
            };
            let mut result = HookResult::success();
            result.control = HookResultControl::Block { reason };
            if matches!(event_key, HookEventKey::PermissionRequest) {
                result.permission_decision = Some(HookPermissionDecision::Deny);
            }
            return result;
        }

        return HookResult {
            error: Some(message),
            ..HookResult::success()
        };
    }

    if code != 0 {
        let stderr_preview = preview_bytes(&output.stderr);
        let message = if stderr_preview.is_empty() {
            format!("hook command exited with {}", output.status)
        } else {
            format!(
                "hook command exited with {}: {stderr_preview}",
                output.status
            )
        };
        return HookResult {
            error: Some(message),
            ..HookResult::success()
        };
    }

    let Some(stdout_json) = parse_stdout_json(&output.stdout) else {
        return HookResult::success();
    };

    apply_stdout_json(event_key, stdout_json)
}

fn apply_stdout_json(event_key: HookEventKey, stdout_json: Value) -> HookResult {
    let mut result = HookResult::success();
    let Some(obj) = stdout_json.as_object() else {
        return result;
    };

    if let Some(system_message) = obj
        .get("systemMessage")
        .or_else(|| obj.get("system_message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        result.additional_context.push(system_message.to_string());
    }

    if let Some(additional_context) = obj
        .get("additionalContext")
        .or_else(|| obj.get("additional_context"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        result
            .additional_context
            .push(additional_context.to_string());
    }

    let hook_specific = obj
        .get("hookSpecificOutput")
        .or_else(|| obj.get("hook_specific_output"))
        .and_then(Value::as_object);

    if let Some(hook_specific) = hook_specific
        && let Some(additional_context) = hook_specific
            .get("additionalContext")
            .or_else(|| hook_specific.get("additional_context"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        result
            .additional_context
            .push(additional_context.to_string());
    }

    result.updated_input = obj
        .get("updatedInput")
        .or_else(|| obj.get("updated_input"))
        .cloned()
        .or_else(|| {
            hook_specific.and_then(|hook_specific| {
                hook_specific
                    .get("updatedInput")
                    .or_else(|| hook_specific.get("updated_input"))
                    .cloned()
            })
        });

    apply_decisions(event_key, obj, hook_specific, &mut result);
    result
}

fn apply_decisions(
    event_key: HookEventKey,
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
    result: &mut HookResult,
) {
    if !event_key.supports_output_decisions() {
        return;
    }

    match event_key {
        HookEventKey::PreToolUse => apply_pre_tool_use_decisions(obj, hook_specific, result),
        HookEventKey::PermissionRequest => {
            apply_permission_request_decisions(obj, hook_specific, result)
        }
        _ => apply_block_decision(obj, result),
    }
}

fn apply_pre_tool_use_decisions(
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
    result: &mut HookResult,
) {
    if let Some(permission_decision) = extract_permission_decision(obj, hook_specific) {
        result.permission_decision = Some(permission_decision);
        result.permission_decision_reason = extract_permission_decision_reason(obj, hook_specific);
    }

    let parsed_decision = obj
        .get("decision")
        .and_then(Value::as_str)
        .and_then(parse_decision);
    if matches!(
        parsed_decision,
        Some(ParsedDecision::Deny) | Some(ParsedDecision::Ask)
    ) {
        let reason = decision_reason(obj, hook_specific)
            .unwrap_or_else(|| "hook blocked tool invocation".to_string());
        result.control = HookResultControl::Block { reason };
        return;
    }

    if matches!(
        result.permission_decision,
        Some(HookPermissionDecision::Deny) | Some(HookPermissionDecision::Ask)
    ) {
        let reason = result
            .permission_decision_reason
            .clone()
            .or_else(|| decision_reason(obj, hook_specific))
            .unwrap_or_else(|| "hook blocked tool invocation".to_string());
        result.control = HookResultControl::Block { reason };
    }
}

fn apply_permission_request_decisions(
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
    result: &mut HookResult,
) {
    let permission_decision = extract_permission_decision(obj, hook_specific)
        .or_else(|| extract_permission_behavior(hook_specific))
        .or_else(|| {
            obj.get("decision")
                .and_then(Value::as_str)
                .and_then(parse_permission_decision)
        });
    if let Some(permission_decision) = permission_decision {
        result.permission_decision = Some(permission_decision);
        result.permission_decision_reason = extract_permission_decision_reason(obj, hook_specific);
    }
}

fn apply_block_decision(obj: &serde_json::Map<String, Value>, result: &mut HookResult) {
    let Some(decision) = obj
        .get("decision")
        .and_then(Value::as_str)
        .and_then(parse_decision)
    else {
        return;
    };
    if decision != ParsedDecision::Deny {
        return;
    }
    let reason = obj
        .get("reason")
        .or_else(|| obj.get("stopReason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "hook blocked operation".to_string());
    result.control = HookResultControl::Block { reason };
}

fn extract_permission_decision(
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
) -> Option<HookPermissionDecision> {
    let decision = hook_specific
        .and_then(|obj| permission_decision_str(obj))
        .or_else(|| permission_decision_str(obj))?;
    parse_permission_decision(decision)
}

fn permission_decision_str(obj: &serde_json::Map<String, Value>) -> Option<&str> {
    obj.get("permissionDecision")
        .or_else(|| obj.get("permission_decision"))
        .and_then(Value::as_str)
}

fn extract_permission_behavior(
    hook_specific: Option<&serde_json::Map<String, Value>>,
) -> Option<HookPermissionDecision> {
    let behavior = hook_specific?
        .get("decision")
        .and_then(Value::as_object)?
        .get("behavior")
        .and_then(Value::as_str)?;
    parse_permission_decision(behavior)
}

fn extract_permission_decision_reason(
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    hook_specific
        .and_then(permission_decision_reason_str)
        .or_else(|| permission_decision_reason_str(obj))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn permission_decision_reason_str(obj: &serde_json::Map<String, Value>) -> Option<&str> {
    obj.get("permissionDecisionReason")
        .or_else(|| obj.get("permission_decision_reason"))
        .and_then(Value::as_str)
}

fn decision_reason(
    obj: &serde_json::Map<String, Value>,
    hook_specific: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    obj.get("reason")
        .or_else(|| obj.get("stopReason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            hook_specific
                .and_then(permission_decision_reason_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn parse_permission_decision(decision: &str) -> Option<HookPermissionDecision> {
    match decision.trim().to_lowercase().as_str() {
        "allow" | "approve" | "continue" => Some(HookPermissionDecision::Allow),
        "deny" | "block" | "abort" => Some(HookPermissionDecision::Deny),
        "ask" => Some(HookPermissionDecision::Ask),
        _ => None,
    }
}

fn parse_decision(decision: &str) -> Option<ParsedDecision> {
    match decision.trim().to_lowercase().as_str() {
        "allow" | "approve" | "continue" => Some(ParsedDecision::Allow),
        "block" | "deny" | "abort" => Some(ParsedDecision::Deny),
        "ask" => Some(ParsedDecision::Ask),
        _ => None,
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

fn parse_stdout_json(stdout: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            return Some(value);
        }
    }

    None
}

fn result_with_error(error: io::Error) -> HookResult {
    HookResult {
        error: Some(error.to_string()),
        ..HookResult::success()
    }
}

fn command_from_argv(argv: &[String]) -> Option<Command> {
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
    use std::path::Path;
    use std::path::PathBuf;

    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[cfg(windows)]
    fn exit_command(code: i32) -> Vec<String> {
        vec![
            "cmd".to_string(),
            "/C".to_string(),
            format!("exit /B {code}"),
        ]
    }

    #[cfg(not(windows))]
    fn exit_command(code: i32) -> Vec<String> {
        vec!["sh".to_string(), "-c".to_string(), format!("exit {code}")]
    }

    #[cfg(windows)]
    fn echo_command() -> Vec<String> {
        vec!["cmd".to_string(), "/C".to_string(), "echo hi".to_string()]
    }

    #[cfg(not(windows))]
    fn echo_command() -> Vec<String> {
        vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()]
    }

    #[cfg(windows)]
    fn exit_with_stderr_command(code: i32, stderr: &str) -> Vec<String> {
        vec![
            "cmd".to_string(),
            "/C".to_string(),
            format!("echo {stderr} 1>&2 & exit /B {code}"),
        ]
    }

    #[cfg(not(windows))]
    fn exit_with_stderr_command(code: i32, stderr: &str) -> Vec<String> {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo {stderr} 1>&2; exit {code}"),
        ]
    }

    fn payload(cwd: &Path, hook_event: HookEvent) -> HookPayload {
        HookPayload {
            session_id: ThreadId::new(),
            transcript_path: None,
            cwd: cwd.to_path_buf(),
            permission_mode: "never".to_string(),
            hook_event,
        }
    }

    #[tokio::test]
    async fn exit_2_blocks_for_blockable_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: exit_command(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Block { .. }
        ));
    }

    #[tokio::test]
    async fn blocking_hook_does_not_stop_other_hooks_for_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![
                    CommandHookConfig {
                        command: exit_command(2),
                        ..Default::default()
                    },
                    CommandHookConfig {
                        command: echo_command(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Block { .. }
        ));
        assert!(matches!(
            outcomes[1].result.control,
            HookResultControl::Continue
        ));
    }

    #[tokio::test]
    async fn identical_hooks_are_deduplicated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![
                    CommandHookConfig {
                        command: echo_command(),
                        ..Default::default()
                    },
                    CommandHookConfig {
                        command: echo_command(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
    }

    #[tokio::test]
    async fn matching_hooks_run_in_parallel() {
        use std::time::Duration;

        struct BarrierExecutor {
            barrier: tokio::sync::Barrier,
        }

        impl NonCommandHookExecutor for BarrierExecutor {
            fn execute_prompt(
                self: Arc<Self>,
                _payload: HookPayload,
                _prompt: String,
                _model: Option<String>,
                _timeout: Option<Duration>,
            ) -> Pin<Box<dyn Future<Output = HookResult> + Send>> {
                Box::pin(async move {
                    self.barrier.wait().await;
                    HookResult::success()
                })
            }

            fn execute_agent(
                self: Arc<Self>,
                _payload: HookPayload,
                _prompt: String,
                _model: Option<String>,
                _timeout: Option<Duration>,
            ) -> Pin<Box<dyn Future<Output = HookResult> + Send>> {
                Box::pin(async move {
                    self.barrier.wait().await;
                    HookResult::success()
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let mut hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![
                    CommandHookConfig {
                        handler_type: HookHandlerType::Prompt,
                        prompt: Some("p1".to_string()),
                        ..Default::default()
                    },
                    CommandHookConfig {
                        handler_type: HookHandlerType::Prompt,
                        prompt: Some("p2".to_string()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        });
        hooks.set_non_command_executor(Arc::new(BarrierExecutor {
            barrier: tokio::sync::Barrier::new(2),
        }));

        let outcomes = tokio::time::timeout(
            Duration::from_secs(1),
            hooks.dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            )),
        )
        .await
        .expect("dispatch should not time out");

        assert_eq!(outcomes.len(), 2);
    }

    #[tokio::test]
    async fn exit_2_does_not_block_for_post_tool_use() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                post_tool_use: vec![CommandHookConfig {
                    command: exit_command(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PostToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_response: json!({"ok": true}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Continue
        ));
        assert!(outcomes[0].result.error.is_some());
    }

    #[tokio::test]
    async fn exit_2_does_not_block_for_worktree_create() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                worktree_create: vec![CommandHookConfig {
                    command: exit_command(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::WorktreeCreate {
                    repo_path: PathBuf::from("/repo"),
                    worktree_path: PathBuf::from("/repo/wt"),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Continue
        ));
        assert!(outcomes[0].result.error.is_some());
    }

    #[tokio::test]
    async fn exit_2_does_not_block_for_worktree_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                worktree_remove: vec![CommandHookConfig {
                    command: exit_command(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::WorktreeRemove {
                    repo_path: PathBuf::from("/repo"),
                    worktree_path: PathBuf::from("/repo/wt"),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Continue
        ));
        assert!(outcomes[0].result.error.is_some());
    }

    #[tokio::test]
    async fn exit_2_blocks_teammate_idle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                teammate_idle: vec![CommandHookConfig {
                    command: exit_with_stderr_command(2, "keep working"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::TeammateIdle {
                    teammate_name: "planner".to_string(),
                    team_name: "my-project".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0].result.control,
            HookResultControl::Block { reason } if reason == "keep working"
        ));
    }

    #[tokio::test]
    async fn exit_2_denies_permission_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                permission_request: vec![CommandHookConfig {
                    command: exit_command(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PermissionRequest {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                    permission_suggestions: None,
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Block { .. }
        ));
        assert_eq!(
            outcomes[0].result.permission_decision,
            Some(HookPermissionDecision::Deny)
        );
    }

    #[tokio::test]
    async fn once_hooks_run_only_once_per_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                session_start: vec![CommandHookConfig {
                    command: echo_command(),
                    once: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let hook_event = HookEvent::SessionStart {
            source: "cli".to_string(),
            model: "gpt-5".to_string(),
            agent_type: None,
        };

        let first = hooks
            .dispatch(payload(dir.path(), hook_event.clone()))
            .await;
        let second = hooks.dispatch(payload(dir.path(), hook_event)).await;

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 0);
    }

    #[tokio::test]
    async fn once_hooks_do_not_collide_across_events_with_same_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                session_start: vec![CommandHookConfig {
                    name: Some("shared".to_string()),
                    command: echo_command(),
                    once: true,
                    ..Default::default()
                }],
                user_prompt_submit: vec![CommandHookConfig {
                    name: Some("shared".to_string()),
                    command: echo_command(),
                    once: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let first = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::SessionStart {
                    source: "cli".to_string(),
                    model: "gpt-5".to_string(),
                    agent_type: None,
                },
            ))
            .await;
        let second = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::UserPromptSubmit {
                    prompt: "hi".to_string(),
                },
            ))
            .await;

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
    }

    #[tokio::test]
    async fn once_hooks_do_not_consume_when_matcher_does_not_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: echo_command(),
                    once: true,
                    matcher: HookMatcherConfig {
                        tool_name: Some("exec".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let does_not_match = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;
        let matches = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "exec".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-2".to_string(),
                },
            ))
            .await;

        assert_eq!(does_not_match.len(), 0);
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn scoped_hooks_can_be_installed_and_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::default();

        hooks.insert_scoped_command_hooks(
            "scope-1".to_string(),
            CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: exit_with_stderr_command(2, "blocked by scoped hook"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );

        let blocked = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(blocked.len(), 1);
        assert!(matches!(
            &blocked[0].result.control,
            HookResultControl::Block { reason } if reason == "blocked by scoped hook"
        ));

        hooks.remove_scoped_hooks("scope-1");

        let allowed = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-2".to_string(),
                },
            ))
            .await;

        assert_eq!(allowed.len(), 0);
    }

    #[tokio::test]
    async fn scoped_hooks_are_deduplicated_across_scopes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::default();

        hooks.insert_scoped_command_hooks(
            "scope-1".to_string(),
            CommandHooksConfig {
                session_start: vec![CommandHookConfig {
                    command: echo_command(),
                    once: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        hooks.insert_scoped_command_hooks(
            "scope-2".to_string(),
            CommandHooksConfig {
                session_start: vec![CommandHookConfig {
                    command: echo_command(),
                    once: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );

        let hook_event = HookEvent::SessionStart {
            source: "cli".to_string(),
            model: "gpt-5".to_string(),
            agent_type: None,
        };

        let first = hooks
            .dispatch(payload(dir.path(), hook_event.clone()))
            .await;
        let second = hooks.dispatch(payload(dir.path(), hook_event)).await;

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 0);
    }

    #[tokio::test]
    async fn invalid_tool_name_regex_surfaces_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: echo_command(),
                    matcher: HookMatcherConfig {
                        tool_name_regex: Some("[".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0]
                .result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("invalid tool_name_regex"),
            "expected invalid regex error, got: {:?}",
            outcomes[0].result.error
        );
    }

    #[tokio::test]
    async fn matcher_tool_name_filters_hooks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![
                    CommandHookConfig {
                        command: echo_command(),
                        matcher: HookMatcherConfig {
                            tool_name: Some("shell".to_string()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    CommandHookConfig {
                        command: echo_command(),
                        matcher: HookMatcherConfig {
                            tool_name: Some("exec".to_string()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
    }

    #[tokio::test]
    async fn user_prompt_submit_does_not_support_matchers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                user_prompt_submit: vec![CommandHookConfig {
                    command: echo_command(),
                    matcher: HookMatcherConfig {
                        prompt_regex: Some("hello".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let matches = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::UserPromptSubmit {
                    prompt: "hello world".to_string(),
                },
            ))
            .await;
        let does_not_match = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::UserPromptSubmit {
                    prompt: "goodbye".to_string(),
                },
            ))
            .await;

        assert_eq!(matches.len(), 1);
        assert_eq!(does_not_match.len(), 1);
    }

    #[tokio::test]
    async fn empty_commands_are_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![
                    CommandHookConfig {
                        command: Vec::new(),
                        ..Default::default()
                    },
                    CommandHookConfig {
                        command: vec!["   ".to_string()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 0);
    }

    #[tokio::test]
    async fn matcher_tool_name_regex_does_not_match_non_tool_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                session_start: vec![CommandHookConfig {
                    command: echo_command(),
                    matcher: HookMatcherConfig {
                        tool_name_regex: Some(".*".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::SessionStart {
                    source: "cli".to_string(),
                    model: "gpt-5".to_string(),
                    agent_type: None,
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 0);
    }

    #[tokio::test]
    async fn non_zero_exit_without_stderr_returns_status_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: exit_command(1),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0]
                .result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("hook command exited with")
        );
    }

    #[tokio::test]
    async fn non_zero_exit_with_stderr_includes_stderr_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: exit_with_stderr_command(1, "blocked by policy"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0]
                .result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("blocked by policy")
        );
    }

    #[test]
    fn parse_stdout_json_returns_none_for_empty_output() {
        assert_eq!(parse_stdout_json(b""), None);
        assert_eq!(parse_stdout_json(b"\n \t\n"), None);
    }

    #[test]
    fn parse_stdout_json_skips_blank_lines_before_json() {
        let value = parse_stdout_json(b"\n\n   \n{\"a\":2}\n").expect("json");
        assert_eq!(value, json!({"a": 2}));
    }

    #[test]
    fn parse_stdout_json_accepts_full_json_output() {
        let value = parse_stdout_json(br#"{"a":1}"#).expect("json");
        assert_eq!(value, json!({"a": 1}));
    }

    #[test]
    fn parse_stdout_json_accepts_first_parseable_json_line() {
        let value = parse_stdout_json(b"not json\n{\"a\":1}\n{\"b\":2}\n").expect("json");
        assert_eq!(value, json!({"a": 1}));
    }

    #[test]
    fn apply_stdout_json_collects_context_and_updated_input() {
        let result = apply_stdout_json(
            HookEventKey::PreToolUse,
            json!({
                "systemMessage": " sys ",
                "additionalContext": " ctx ",
                "hookSpecificOutput": {
                    "additionalContext": " hook ",
                    "updatedInput": {"x": 1},
                    "permissionDecision": "deny",
                    "permissionDecisionReason": " nope ",
                },
            }),
        );

        assert_eq!(
            result.additional_context,
            vec!["sys".to_string(), "ctx".to_string(), "hook".to_string()]
        );
        assert_eq!(result.updated_input, Some(json!({"x": 1})));
        assert_eq!(
            result.permission_decision,
            Some(HookPermissionDecision::Deny)
        );
        assert!(matches!(
            result.control,
            HookResultControl::Block { reason } if reason == "nope"
        ));
    }

    #[test]
    fn apply_stdout_json_parses_permission_request_behavior() {
        let result = apply_stdout_json(
            HookEventKey::PermissionRequest,
            json!({
                "hookSpecificOutput": {
                    "decision": {"behavior": "allow"},
                    "permissionDecisionReason": " ok ",
                },
            }),
        );

        assert_eq!(
            result.permission_decision,
            Some(HookPermissionDecision::Allow)
        );
        assert_eq!(result.permission_decision_reason, Some("ok".to_string()));
        assert!(matches!(result.control, HookResultControl::Continue));
    }

    #[test]
    fn apply_stdout_json_blocks_on_decision_with_reason() {
        let result = apply_stdout_json(
            HookEventKey::UserPromptSubmit,
            json!({
                "decision": "block",
                "reason": "no",
            }),
        );

        assert!(matches!(
            result.control,
            HookResultControl::Block { reason } if reason == "no"
        ));
    }

    #[test]
    fn apply_stdout_json_ignores_decisions_for_post_tool_use() {
        let result = apply_stdout_json(
            HookEventKey::PostToolUse,
            json!({
                "decision": "block",
                "reason": "no",
            }),
        );

        assert!(matches!(result.control, HookResultControl::Continue));
    }

    #[test]
    fn apply_stdout_json_non_object_is_ignored() {
        let result = apply_stdout_json(HookEventKey::PreToolUse, json!(["not", "an", "object"]));
        assert_eq!(result, HookResult::success());
    }

    #[test]
    fn apply_stdout_json_pre_tool_use_ask_blocks_with_reason() {
        let result = apply_stdout_json(
            HookEventKey::PreToolUse,
            json!({
                "decision": "ask",
                "stopReason": "need approval",
            }),
        );

        assert!(matches!(
            result.control,
            HookResultControl::Block { reason } if reason == "need approval"
        ));
    }

    #[test]
    fn apply_stdout_json_permission_request_accepts_top_level_decision() {
        let result = apply_stdout_json(
            HookEventKey::PermissionRequest,
            json!({
                "decision": "ask",
                "permissionDecisionReason": "check with user",
            }),
        );

        assert_eq!(
            result.permission_decision,
            Some(HookPermissionDecision::Ask)
        );
        assert_eq!(
            result.permission_decision_reason,
            Some("check with user".to_string())
        );
        assert!(matches!(result.control, HookResultControl::Continue));
    }

    #[test]
    fn apply_stdout_json_user_prompt_submit_does_not_block_on_ask() {
        let result = apply_stdout_json(
            HookEventKey::UserPromptSubmit,
            json!({
                "decision": "ask",
                "reason": "not used",
            }),
        );

        assert!(matches!(result.control, HookResultControl::Continue));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permission_decision_deny_blocks_pre_tool_use_on_exit_0_stdout_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"no\"}}'"
                .to_string(),
        ];
        let hooks = Hooks::new(HooksConfig {
            command_hooks: CommandHooksConfig {
                pre_tool_use: vec![CommandHookConfig {
                    command: cmd,
                    ..Default::default()
                }],
                ..Default::default()
            },
        });

        let outcomes = hooks
            .dispatch(payload(
                dir.path(),
                HookEvent::PreToolUse {
                    tool_name: "shell".to_string(),
                    tool_input: json!({"command":["echo","hi"]}),
                    tool_use_id: "call-1".to_string(),
                },
            ))
            .await;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].result.permission_decision,
            Some(HookPermissionDecision::Deny)
        );
        assert!(matches!(
            outcomes[0].result.control,
            HookResultControl::Block { .. }
        ));
    }
}
