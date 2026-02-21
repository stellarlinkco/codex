use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::client_common::tools::ToolSpec;
use crate::features::Feature;
use crate::function_tool::FunctionCallError;
use crate::memories::usage::emit_metric_for_tool_read;
use crate::protocol::SandboxPolicy;
use crate::sandbox_tags::sandbox_tag;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use async_trait::async_trait;
use codex_hooks::HookEvent;
use codex_hooks::HookPayload;
use codex_hooks::HookResultControl;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::AskForApproval;
use codex_utils_readiness::Readiness;
use serde_json::Value;
use serde_json::json;
use tracing::warn;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Function,
    Mcp,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn kind(&self) -> ToolKind;

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            (self.kind(), payload),
            (ToolKind::Function, ToolPayload::Function { .. })
                | (ToolKind::Mcp, ToolPayload::Mcp { .. })
        )
    }

    /// Returns `true` if the [ToolInvocation] *might* mutate the environment of the
    /// user (through file system, OS operations, ...).
    /// This function must remains defensive and return `true` if a doubt exist on the
    /// exact effect of a ToolInvocation.
    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    /// Perform the actual [ToolInvocation] and returns a [ToolOutput] containing
    /// the final output to return to the model.
    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError>;
}

pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new(handlers: HashMap<String, Arc<dyn ToolHandler>>) -> Self {
        Self { handlers }
    }

    pub fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).map(Arc::clone)
    }

    // TODO(jif) for dynamic tools.
    // pub fn register(&mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) {
    //     let name = name.into();
    //     if self.handlers.insert(name.clone(), handler).is_some() {
    //         warn!("overwriting handler for tool {name}");
    //     }
    // }

    pub async fn dispatch(
        &self,
        mut invocation: ToolInvocation,
    ) -> Result<ResponseInputItem, FunctionCallError> {
        let tool_name = invocation.tool_name.clone();
        let call_id_owned = invocation.call_id.clone();
        let otel = invocation.turn.otel_manager.clone();
        let initial_log_payload = invocation.payload.log_payload();
        let metric_tags = [
            (
                "sandbox",
                sandbox_tag(
                    &invocation.turn.sandbox_policy,
                    invocation.turn.windows_sandbox_level,
                    invocation
                        .turn
                        .features
                        .enabled(Feature::UseLinuxSandboxBwrap),
                ),
            ),
            (
                "sandbox_policy",
                sandbox_policy_tag(&invocation.turn.sandbox_policy),
            ),
        ];
        let (mcp_server, mcp_server_origin) = match &invocation.payload {
            ToolPayload::Mcp { server, .. } => {
                let manager = invocation
                    .session
                    .services
                    .mcp_connection_manager
                    .read()
                    .await;
                let origin = manager.server_origin(server).map(str::to_owned);
                (Some(server.clone()), origin)
            }
            _ => (None, None),
        };
        let mcp_server_ref = mcp_server.as_deref();
        let mcp_server_origin_ref = mcp_server_origin.as_deref();

        let handler = match self.handler(tool_name.as_ref()) {
            Some(handler) => handler,
            None => {
                let message =
                    unsupported_tool_call_message(&invocation.payload, tool_name.as_ref());
                otel.tool_result_with_tags(
                    tool_name.as_ref(),
                    &call_id_owned,
                    initial_log_payload.as_ref(),
                    Duration::ZERO,
                    false,
                    &message,
                    &metric_tags,
                    mcp_server_ref,
                    mcp_server_origin_ref,
                );
                return Err(FunctionCallError::RespondToModel(message));
            }
        };

        if !handler.matches_kind(&invocation.payload) {
            let message = format!("tool {tool_name} invoked with incompatible payload");
            otel.tool_result_with_tags(
                tool_name.as_ref(),
                &call_id_owned,
                initial_log_payload.as_ref(),
                Duration::ZERO,
                false,
                &message,
                &metric_tags,
                mcp_server_ref,
                mcp_server_origin_ref,
            );
            return Err(FunctionCallError::Fatal(message));
        }

        let is_mutating = handler.is_mutating(&invocation).await;
        if let Some(err) = dispatch_pre_tool_use_hook(PreToolUseHookDispatch {
            invocation: &mut invocation,
        })
        .await
        {
            return Err(err);
        }

        let payload_for_response = invocation.payload.clone();
        let log_payload = payload_for_response.log_payload();

        let output_cell = tokio::sync::Mutex::new(None);
        let invocation_for_tool = invocation.clone();

        let started = Instant::now();
        let result = otel
            .log_tool_result_with_tags(
                tool_name.as_ref(),
                &call_id_owned,
                log_payload.as_ref(),
                &metric_tags,
                mcp_server_ref,
                mcp_server_origin_ref,
                || {
                    let handler = handler.clone();
                    let output_cell = &output_cell;
                    async move {
                        if is_mutating {
                            tracing::trace!("waiting for tool gate");
                            invocation_for_tool.turn.tool_call_gate.wait_ready().await;
                            tracing::trace!("tool gate released");
                        }
                        match handler.handle(invocation_for_tool).await {
                            Ok(output) => {
                                let preview = output.log_preview();
                                let success = output.success_for_logging();
                                let mut guard = output_cell.lock().await;
                                *guard = Some(output);
                                Ok((preview, success))
                            }
                            Err(err) => Err(err),
                        }
                    }
                },
            )
            .await;
        let duration = started.elapsed();
        let (output_preview, success) = match &result {
            Ok((preview, success)) => (preview.clone(), *success),
            Err(err) => (err.to_string(), false),
        };
        emit_metric_for_tool_read(&invocation, success).await;
        dispatch_post_tool_use_hook(PostToolUseHookDispatch {
            invocation: &invocation,
            output_preview: output_preview.clone(),
            success,
            executed: true,
            duration,
            mutating: is_mutating,
        })
        .await;

        if let Err(err) = &result {
            dispatch_post_tool_use_failure_hook(PostToolUseFailureHookDispatch {
                invocation: &invocation,
                error: err.to_string(),
            })
            .await;
        }

        match result {
            Ok(_) => {
                let mut guard = output_cell.lock().await;
                let output = guard.take().ok_or_else(|| {
                    FunctionCallError::Fatal("tool produced no output".to_string())
                })?;
                Ok(output.into_response(&call_id_owned, &payload_for_response))
            }
            Err(err) => Err(err),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfiguredToolSpec {
    pub spec: ToolSpec,
    pub supports_parallel_tool_calls: bool,
}

impl ConfiguredToolSpec {
    pub fn new(spec: ToolSpec, supports_parallel_tool_calls: bool) -> Self {
        Self {
            spec,
            supports_parallel_tool_calls,
        }
    }
}

pub struct ToolRegistryBuilder {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: Vec<ConfiguredToolSpec>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            specs: Vec::new(),
        }
    }

    pub fn push_spec(&mut self, spec: ToolSpec) {
        self.push_spec_with_parallel_support(spec, false);
    }

    pub fn push_spec_with_parallel_support(
        &mut self,
        spec: ToolSpec,
        supports_parallel_tool_calls: bool,
    ) {
        self.specs
            .push(ConfiguredToolSpec::new(spec, supports_parallel_tool_calls));
    }

    pub fn register_handler(&mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) {
        let name = name.into();
        if self
            .handlers
            .insert(name.clone(), handler.clone())
            .is_some()
        {
            warn!("overwriting handler for tool {name}");
        }
    }

    // TODO(jif) for dynamic tools.
    // pub fn register_many<I>(&mut self, names: I, handler: Arc<dyn ToolHandler>)
    // where
    //     I: IntoIterator,
    //     I::Item: Into<String>,
    // {
    //     for name in names {
    //         let name = name.into();
    //         if self
    //             .handlers
    //             .insert(name.clone(), handler.clone())
    //             .is_some()
    //         {
    //             warn!("overwriting handler for tool {name}");
    //         }
    //     }
    // }

    pub fn build(self) -> (Vec<ConfiguredToolSpec>, ToolRegistry) {
        let registry = ToolRegistry::new(self.handlers);
        (self.specs, registry)
    }
}

fn unsupported_tool_call_message(payload: &ToolPayload, tool_name: &str) -> String {
    match payload {
        ToolPayload::Custom { .. } => format!("unsupported custom tool call: {tool_name}"),
        _ => format!("unsupported call: {tool_name}"),
    }
}

fn sandbox_policy_tag(policy: &SandboxPolicy) -> &'static str {
    match policy {
        SandboxPolicy::ReadOnly { .. } => "read-only",
        SandboxPolicy::WorkspaceWrite { .. } => "workspace-write",
        SandboxPolicy::DangerFullAccess => "danger-full-access",
        SandboxPolicy::ExternalSandbox { .. } => "external-sandbox",
    }
}

fn approval_policy_for_hooks(policy: AskForApproval) -> &'static str {
    match policy {
        AskForApproval::UnlessTrusted => "untrusted",
        AskForApproval::OnFailure => "on-failure",
        AskForApproval::OnRequest => "on-request",
        AskForApproval::Reject(_) => "reject",
        AskForApproval::Never => "never",
    }
}

fn hook_tool_input(payload: &ToolPayload) -> Value {
    match payload {
        ToolPayload::Function { arguments } => {
            serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.clone()))
        }
        ToolPayload::Custom { input } => Value::String(input.clone()),
        ToolPayload::LocalShell { params } => json!({
            "command": params.command.clone(),
            "workdir": params.workdir.clone(),
            "timeout_ms": params.timeout_ms,
            "sandbox_permissions": params.sandbox_permissions,
            "prefix_rule": params.prefix_rule.clone(),
            "justification": params.justification.clone(),
        }),
        ToolPayload::Mcp {
            server,
            tool,
            raw_arguments,
        } => {
            let arguments = serde_json::from_str(raw_arguments)
                .unwrap_or_else(|_| Value::String(raw_arguments.clone()));
            json!({
                "server": server,
                "tool": tool,
                "arguments": arguments,
            })
        }
    }
}

fn apply_updated_tool_input(payload: &mut ToolPayload, updated_input: Value) -> Result<(), String> {
    match payload {
        ToolPayload::Function { arguments } => match updated_input {
            Value::String(text) => {
                *arguments = text;
                Ok(())
            }
            value => serde_json::to_string(&value)
                .map(|text| {
                    *arguments = text;
                })
                .map_err(|err| format!("failed to serialize updatedInput: {err}")),
        },
        ToolPayload::Custom { input } => match updated_input {
            Value::String(text) => {
                *input = text;
                Ok(())
            }
            value => serde_json::to_string(&value)
                .map(|text| {
                    *input = text;
                })
                .map_err(|err| format!("failed to serialize updatedInput: {err}")),
        },
        ToolPayload::LocalShell { params } => serde_json::from_value(updated_input)
            .map(|updated: codex_protocol::models::ShellToolCallParams| {
                *params = updated;
            })
            .map_err(|err| format!("invalid updatedInput for shell params: {err}")),
        ToolPayload::Mcp { raw_arguments, .. } => match updated_input {
            Value::String(text) => {
                *raw_arguments = text;
                Ok(())
            }
            value => serde_json::to_string(&value)
                .map(|text| {
                    *raw_arguments = text;
                })
                .map_err(|err| format!("failed to serialize updatedInput: {err}")),
        },
    }
}

struct PreToolUseHookDispatch<'a> {
    invocation: &'a mut ToolInvocation,
}

async fn dispatch_pre_tool_use_hook(
    dispatch: PreToolUseHookDispatch<'_>,
) -> Option<FunctionCallError> {
    let PreToolUseHookDispatch { invocation } = dispatch;
    let session = invocation.session.as_ref();
    let turn = invocation.turn.as_ref();
    let tool_input = hook_tool_input(&invocation.payload);
    let hook_outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::PreToolUse {
                tool_name: invocation.tool_name.clone(),
                tool_input,
                tool_use_id: invocation.call_id.clone(),
            },
        })
        .await;

    let mut additional_context = Vec::new();
    let mut updated_input = None;
    let mut blocked = None;

    for hook_outcome in hook_outcomes {
        let hook_name = hook_outcome.hook_name;
        let result = hook_outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                call_id = %invocation.call_id,
                tool_name = %invocation.tool_name,
                hook_name = %hook_name,
                error,
                "pre_tool_use hook failed; continuing"
            );
        }

        additional_context.extend(result.additional_context);
        if let Some(value) = result.updated_input {
            updated_input = Some(value);
        }

        if let HookResultControl::Block { reason } = result.control {
            blocked = Some((hook_name, reason));
            break;
        }
    }

    session.record_hook_context(turn, &additional_context).await;

    if let Some(updated_input) = updated_input {
        if let Err(error) = apply_updated_tool_input(&mut invocation.payload, updated_input) {
            warn!(
                call_id = %invocation.call_id,
                tool_name = %invocation.tool_name,
                error,
                "pre_tool_use hook produced an invalid updatedInput; ignoring"
            );
        }
    }

    if let Some((hook_name, reason)) = blocked {
        return Some(FunctionCallError::RespondToModel(format!(
            "pre_tool_use hook '{hook_name}' blocked tool '{tool_name}': {reason}",
            tool_name = invocation.tool_name
        )));
    }

    None
}

struct PostToolUseHookDispatch<'a> {
    invocation: &'a ToolInvocation,
    output_preview: String,
    success: bool,
    executed: bool,
    duration: Duration,
    mutating: bool,
}

async fn dispatch_post_tool_use_hook(dispatch: PostToolUseHookDispatch<'_>) {
    let PostToolUseHookDispatch { invocation, .. } = dispatch;
    let session = invocation.session.as_ref();
    let turn = invocation.turn.as_ref();
    let tool_input = hook_tool_input(&invocation.payload);
    let tool_response = json!({
        "executed": dispatch.executed,
        "success": dispatch.success,
        "duration_ms": u64::try_from(dispatch.duration.as_millis()).unwrap_or(u64::MAX),
        "mutating": dispatch.mutating,
        "output_preview": dispatch.output_preview,
    });
    let hook_outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::PostToolUse {
                tool_name: invocation.tool_name.clone(),
                tool_input,
                tool_response,
                tool_use_id: invocation.call_id.clone(),
            },
        })
        .await;

    let mut additional_context = Vec::new();
    for hook_outcome in hook_outcomes {
        let hook_name = hook_outcome.hook_name;
        let result = hook_outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                call_id = %invocation.call_id,
                tool_name = %invocation.tool_name,
                hook_name = %hook_name,
                error,
                "post_tool_use hook failed; continuing"
            );
        }

        if let HookResultControl::Block { reason } = result.control {
            warn!(
                call_id = %invocation.call_id,
                tool_name = %invocation.tool_name,
                hook_name = %hook_name,
                reason,
                "post_tool_use hook returned a blocking decision; ignoring"
            );
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
}

struct PostToolUseFailureHookDispatch<'a> {
    invocation: &'a ToolInvocation,
    error: String,
}

async fn dispatch_post_tool_use_failure_hook(dispatch: PostToolUseFailureHookDispatch<'_>) {
    let PostToolUseFailureHookDispatch { invocation, error } = dispatch;
    let session = invocation.session.as_ref();
    let turn = invocation.turn.as_ref();
    let tool_input = hook_tool_input(&invocation.payload);
    let hook_outcomes = session
        .hooks()
        .dispatch(HookPayload {
            session_id: session.conversation_id,
            transcript_path: session.transcript_path().await,
            cwd: turn.cwd.clone(),
            permission_mode: approval_policy_for_hooks(turn.approval_policy).to_string(),
            hook_event: HookEvent::PostToolUseFailure {
                tool_name: invocation.tool_name.clone(),
                tool_input,
                tool_use_id: invocation.call_id.clone(),
                error,
                is_interrupt: None,
            },
        })
        .await;

    let mut additional_context = Vec::new();
    for hook_outcome in hook_outcomes {
        let hook_name = hook_outcome.hook_name;
        let result = hook_outcome.result;

        if let Some(error) = result.error.as_deref() {
            warn!(
                call_id = %invocation.call_id,
                tool_name = %invocation.tool_name,
                hook_name = %hook_name,
                error,
                "post_tool_use_failure hook failed; continuing"
            );
        }

        additional_context.extend(result.additional_context);
    }

    session.record_hook_context(turn, &additional_context).await;
}

#[cfg(test)]
mod tests {
    use codex_protocol::models::ShellToolCallParams;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::ToolPayload;
    use super::apply_updated_tool_input;

    #[test]
    fn apply_updated_tool_input_updates_function_arguments() {
        let mut payload = ToolPayload::Function {
            arguments: "{\"a\":1}".to_string(),
        };

        apply_updated_tool_input(&mut payload, json!({"b": 2})).expect("updatedInput");

        let ToolPayload::Function { arguments } = payload else {
            panic!("expected function payload");
        };
        assert_eq!(arguments, "{\"b\":2}".to_string());
    }

    #[test]
    fn apply_updated_tool_input_updates_shell_params() {
        let mut payload = ToolPayload::LocalShell {
            params: ShellToolCallParams {
                command: vec!["echo".to_string(), "hi".to_string()],
                workdir: None,
                timeout_ms: None,
                sandbox_permissions: None,
                prefix_rule: None,
                justification: None,
            },
        };

        apply_updated_tool_input(
            &mut payload,
            json!({"command": ["ls", "-la"], "timeout": 5}),
        )
        .expect("updatedInput");

        let ToolPayload::LocalShell { params } = payload else {
            panic!("expected local shell payload");
        };
        assert_eq!(
            params,
            ShellToolCallParams {
                command: vec!["ls".to_string(), "-la".to_string()],
                workdir: None,
                timeout_ms: Some(5),
                sandbox_permissions: None,
                prefix_rule: None,
                justification: None,
            }
        );
    }

    #[test]
    fn apply_updated_tool_input_rejects_invalid_shell_params() {
        let mut payload = ToolPayload::LocalShell {
            params: ShellToolCallParams {
                command: vec!["echo".to_string(), "hi".to_string()],
                workdir: None,
                timeout_ms: None,
                sandbox_permissions: None,
                prefix_rule: None,
                justification: None,
            },
        };

        let err = apply_updated_tool_input(&mut payload, json!("nope")).expect_err("invalid");
        assert!(err.contains("invalid updatedInput for shell params"));
    }
}
