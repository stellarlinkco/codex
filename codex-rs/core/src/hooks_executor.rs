use std::sync::Arc;
use std::time::Duration;

use codex_hooks::HookPayload;
use codex_hooks::HookResult;
use codex_hooks::HookResultControl;
use codex_hooks::NonCommandHookExecutor;
use codex_otel::OtelManager;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::user_input::UserInput;
use futures::StreamExt;
use serde::Deserialize;
use tracing::warn;

use crate::agent::AgentControl;
use crate::agent::AgentStatus;
use crate::agent::status::is_final;
use crate::client::ModelClient;
use crate::client_common::Prompt;
use crate::config::Config;
use crate::models_manager::manager::ModelsManager;
use crate::rollout::list::find_thread_path_by_id_str;

const PROMPT_HOOK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const AGENT_HOOK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct HooksNonCommandExecutor {
    pub(crate) model_client: ModelClient,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) otel_manager: OtelManager,
    pub(crate) agent_control: AgentControl,
    pub(crate) config: Arc<Config>,
    pub(crate) default_model: String,
}

#[derive(Debug, Deserialize)]
struct PromptHookDecision {
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
}

impl NonCommandHookExecutor for HooksNonCommandExecutor {
    fn execute_prompt(
        self: Arc<Self>,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = HookResult> + Send>> {
        Box::pin(async move { self.run_prompt_hook(payload, prompt, model, timeout).await })
    }

    fn execute_agent(
        self: Arc<Self>,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = HookResult> + Send>> {
        Box::pin(async move { self.run_agent_hook(payload, prompt, model, timeout).await })
    }
}

impl HooksNonCommandExecutor {
    async fn run_prompt_hook(
        &self,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> HookResult {
        let timeout = timeout.unwrap_or(PROMPT_HOOK_DEFAULT_TIMEOUT);
        let result = tokio::time::timeout(timeout, async {
            let arguments = serde_json::to_string(&payload).unwrap_or_default();
            let rendered_prompt = render_prompt_with_arguments(&prompt, &arguments);
            let model = model.unwrap_or_else(|| self.default_model.clone());
            let mut config = (*self.config).clone();
            config.model = Some(model.clone());
            let model_info = self.models_manager.get_model_info(&model, &config).await;

            let schema = serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "ok": { "type": "boolean" },
                    "reason": { "type": "string" }
                },
                "required": ["ok"]
            });

            let user_msg = ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: rendered_prompt,
                }],
                end_turn: None,
                phase: None,
            };

            let prompt = Prompt {
                input: vec![user_msg],
                tools: Vec::new(),
                parallel_tool_calls: false,
                base_instructions: BaseInstructions {
                    text: "Return JSON only: {\"ok\": true} or {\"ok\": false, \"reason\": \"...\"}. No extra text."
                        .to_string(),
                },
                personality: None,
                output_schema: Some(schema),
            };

            let mut client_session = self.model_client.new_session();
            let mut stream = client_session
                .stream(
                    &prompt,
                    &model_info,
                    &self.otel_manager,
                    None,
                    ReasoningSummaryConfig::None,
                    None,
                )
                .await
                .map_err(|err| format!("prompt hook request failed: {err}"))?;

            let mut out = String::new();
            while let Some(ev) = stream.next().await {
                let ev = ev.map_err(|err| format!("prompt hook stream error: {err}"))?;
                match ev {
                    crate::client_common::ResponseEvent::OutputTextDelta(delta) => out.push_str(&delta),
                    crate::client_common::ResponseEvent::Completed { .. } => break,
                    _ => {}
                }
            }

            let trimmed = out.trim();
            let decision: PromptHookDecision = serde_json::from_str(trimmed)
                .map_err(|err| format!("prompt hook returned invalid JSON: {err}"))?;
            Ok::<_, String>(decision)
        })
        .await;

        match result {
            Ok(Ok(decision)) => decision_to_result(decision),
            Ok(Err(error)) => HookResult {
                error: Some(error),
                ..HookResult::success()
            },
            Err(_) => HookResult {
                error: Some("prompt hook timed out".to_string()),
                ..HookResult::success()
            },
        }
    }

    async fn run_agent_hook(
        &self,
        payload: HookPayload,
        prompt: String,
        model: Option<String>,
        timeout: Option<Duration>,
    ) -> HookResult {
        let timeout = timeout.unwrap_or(AGENT_HOOK_DEFAULT_TIMEOUT);
        let model = model.unwrap_or_else(|| self.default_model.clone());
        let arguments = serde_json::to_string(&payload).unwrap_or_default();
        let rendered_prompt = render_prompt_with_arguments(&prompt, &arguments);
        let full_prompt = format!(
            "You are running an agent hook verifier. You may use tools to verify conditions. \
Return JSON only as the final message: {{\"ok\": true}} or {{\"ok\": false, \"reason\": \"...\"}}.\n\n{rendered_prompt}"
        );

        let mut config = (*self.config).clone();
        config.model = Some(model);
        if let Err(err) = config
            .permissions
            .approval_policy
            .set(crate::protocol::AskForApproval::Never)
        {
            warn!(error = %err, "failed to force never approval policy for agent hook; continuing");
        }

        let items = vec![UserInput::Text {
            text: full_prompt,
            text_elements: Vec::new(),
        }];

        let agent_id = match self
            .agent_control
            .spawn_agent(config.clone(), items, None)
            .await
        {
            Ok(agent_id) => agent_id,
            Err(err) => {
                return HookResult {
                    error: Some(format!("agent hook failed to spawn: {err}")),
                    ..HookResult::success()
                };
            }
        };

        let status = match wait_for_agent_final_status(&self.agent_control, agent_id, timeout).await
        {
            Some(status) => status,
            None => {
                let _ = self.agent_control.shutdown_agent(agent_id).await;
                return HookResult {
                    error: Some("agent hook timed out".to_string()),
                    ..HookResult::success()
                };
            }
        };
        if !is_final(&status) {
            let _ = self.agent_control.shutdown_agent(agent_id).await;
            return HookResult {
                error: Some("agent hook ended unexpectedly".to_string()),
                ..HookResult::success()
            };
        }

        let rollout_path =
            match find_thread_path_by_id_str(config.codex_home.as_path(), &agent_id.to_string())
                .await
            {
                Ok(Some(path)) => path,
                Ok(None) => {
                    let _ = self.agent_control.shutdown_agent(agent_id).await;
                    return HookResult {
                        error: Some("agent hook rollout not found".to_string()),
                        ..HookResult::success()
                    };
                }
                Err(err) => {
                    let _ = self.agent_control.shutdown_agent(agent_id).await;
                    return HookResult {
                        error: Some(format!("agent hook rollout lookup failed: {err}")),
                        ..HookResult::success()
                    };
                }
            };

        let last_message = match last_assistant_message_from_rollout(&rollout_path).await {
            Ok(Some(text)) => text,
            Ok(None) => String::new(),
            Err(err) => {
                let _ = self.agent_control.shutdown_agent(agent_id).await;
                return HookResult {
                    error: Some(format!("agent hook rollout read failed: {err}")),
                    ..HookResult::success()
                };
            }
        };

        let _ = self.agent_control.shutdown_agent(agent_id).await;

        let decision: PromptHookDecision = match serde_json::from_str(last_message.trim()) {
            Ok(decision) => decision,
            Err(err) => {
                return HookResult {
                    error: Some(format!("agent hook returned invalid JSON: {err}")),
                    ..HookResult::success()
                };
            }
        };

        decision_to_result(decision)
    }
}

fn render_prompt_with_arguments(prompt: &str, arguments: &str) -> String {
    if prompt.contains("$ARGUMENTS") {
        prompt.replace("$ARGUMENTS", arguments)
    } else {
        format!("{prompt}\n\n$ARGUMENTS:\n{arguments}")
    }
}

fn decision_to_result(decision: PromptHookDecision) -> HookResult {
    if decision.ok {
        return HookResult::success();
    }
    let reason = decision
        .reason
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or_else(|| "hook blocked operation".to_string());
    let mut result = HookResult::success();
    result.control = HookResultControl::Block { reason };
    result
}

async fn wait_for_agent_final_status(
    agent_control: &AgentControl,
    agent_id: ThreadId,
    timeout: Duration,
) -> Option<AgentStatus> {
    let mut status_rx = agent_control.subscribe_status(agent_id).await.ok()?;
    let wait = async {
        loop {
            let status = status_rx.borrow().clone();
            if is_final(&status) {
                return status;
            }
            if status_rx.changed().await.is_err() {
                return status_rx.borrow().clone();
            }
        }
    };
    tokio::time::timeout(timeout, wait).await.ok()
}

async fn last_assistant_message_from_rollout(
    path: &std::path::Path,
) -> std::io::Result<Option<String>> {
    let raw = tokio::fs::read_to_string(path).await?;
    let mut last = None;
    for line in raw.lines() {
        let Ok(item) = serde_json::from_str::<RolloutItem>(line) else {
            continue;
        };
        match item {
            RolloutItem::ResponseItem(ResponseItem::Message { role, content, .. })
                if role == "assistant" =>
            {
                let mut text = String::new();
                for part in content {
                    match part {
                        ContentItem::InputText { text: part }
                        | ContentItem::OutputText { text: part } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&part);
                        }
                        ContentItem::InputImage { .. } => {}
                    }
                }
                if !text.trim().is_empty() {
                    last = Some(text);
                }
            }
            RolloutItem::Compacted(compacted) => last = Some(compacted.message),
            _ => {}
        }
    }
    Ok(last)
}
