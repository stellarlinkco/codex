use anthropic_sdk::Anthropic;
use anthropic_sdk::ClientOptions;
use anthropic_sdk::Error as AnthropicError;
use anthropic_sdk::HttpApiError;
use anthropic_sdk::types::messages::MessageCreateParams;
use anthropic_sdk::types::messages::MessageDeltaUsage;
use anthropic_sdk::types::messages::MessageParam;
use anthropic_sdk::types::messages::RawContentBlockDelta;
use anthropic_sdk::types::messages::RawMessageStreamEvent;
use codex_api::common::ResponseEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::TokenUsage;
use futures::StreamExt;
use jsonschema::JSONSchema;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use crate::client_common::Prompt;
use crate::client_common::ResponseStream;
use crate::client_common::tools::ToolSpec;
use crate::error::CodexErr;
use crate::error::EnvVarError;
use crate::error::Result;
use crate::error::RetryLimitReachedError;
use crate::error::UnexpectedResponseError;
use crate::model_provider_info::ModelProviderInfo;

const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 8_192;
const TOOL_INPUT_FIELD: &str = "input";
const ANTHROPIC_AUTH_TOKEN_ENV_VAR: &str = "ANTHROPIC_AUTH_TOKEN";
const ANTHROPIC_OUTPUT_SCHEMA_INSTRUCTIONS: &str =
    "Respond with JSON only. It must strictly match this schema:";
const ANTHROPIC_SCHEMA_REPAIR_MAX_RETRIES: usize = 2;
const ANTHROPIC_SCHEMA_REPAIR_PREVIOUS_OUTPUT_MAX_BYTES: usize = 8_192;
const ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES: usize = 65_536;

pub(crate) async fn stream_anthropic(
    provider: &ModelProviderInfo,
    prompt: &Prompt,
    model_info: &ModelInfo,
) -> Result<ResponseStream> {
    let client = create_anthropic_client(provider)?;
    let prompt = prompt.clone();
    let model_info = model_info.clone();

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(1600);
    tokio::spawn(async move {
        let result = if prompt.output_schema.is_some() {
            match stream_anthropic_inner(&client, prompt, model_info).await {
                Ok(events) => send_events(&tx_event, events).await,
                Err(err) => Err(err),
            }
        } else {
            stream_anthropic_once_to_channel(&client, &prompt, &model_info, &tx_event).await
        };

        if let Err(err) = result {
            let _ = tx_event.send(Err(err)).await;
        }
    });

    Ok(ResponseStream { rx_event })
}

async fn stream_anthropic_inner(
    client: &Anthropic,
    mut prompt: Prompt,
    model_info: ModelInfo,
) -> Result<Vec<ResponseEvent>> {
    let Some(output_schema) = prompt.output_schema.clone() else {
        return stream_anthropic_once(client, &prompt, &model_info).await;
    };

    for attempt in 0..=ANTHROPIC_SCHEMA_REPAIR_MAX_RETRIES {
        let mut events = stream_anthropic_once(client, &prompt, &model_info).await?;
        let Some(assistant_output) = last_assistant_output_text(&events) else {
            return Ok(events);
        };

        match validate_assistant_output_against_schema(&output_schema, &assistant_output) {
            Ok(()) => return Ok(events),
            Err(validation_error) => {
                if let Some(normalized_json) =
                    extract_schema_matching_json(&output_schema, &assistant_output)
                {
                    replace_last_assistant_message(&mut events, &normalized_json);
                    return Ok(events);
                }

                if attempt == ANTHROPIC_SCHEMA_REPAIR_MAX_RETRIES {
                    return Err(CodexErr::InvalidRequest(format!(
                        "anthropic output_schema validation failed after retries: {validation_error}"
                    )));
                }

                append_output_schema_retry_message(
                    &mut prompt,
                    &validation_error,
                    &assistant_output,
                );
            }
        }
    }

    Err(CodexErr::InvalidRequest(
        "anthropic output_schema validation failed after retries".to_string(),
    ))
}

async fn stream_anthropic_once_to_channel(
    client: &Anthropic,
    prompt: &Prompt,
    model_info: &ModelInfo,
    tx_event: &mpsc::Sender<Result<ResponseEvent>>,
) -> Result<()> {
    let request = build_anthropic_request(prompt, model_info)?;
    let mut stream = client
        .messages
        .create_stream(request, None)
        .await
        .map_err(map_anthropic_error)?;

    let mut state = AnthropicStreamState::new(&prompt.tools);
    while let Some(event_result) = stream.next().await {
        match event_result {
            Ok(event) => state.handle_event(event, tx_event).await?,
            Err(err) => return Err(map_anthropic_error(err)),
        }
    }

    state.finish(tx_event).await?;
    Ok(())
}

async fn stream_anthropic_once(
    client: &Anthropic,
    prompt: &Prompt,
    model_info: &ModelInfo,
) -> Result<Vec<ResponseEvent>> {
    let request = build_anthropic_request(prompt, model_info)?;
    let mut stream = client
        .messages
        .create_stream(request, None)
        .await
        .map_err(map_anthropic_error)?;

    let mut state = AnthropicStreamState::new(&prompt.tools);
    let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(1600);
    let mut events = Vec::new();

    while let Some(event_result) = stream.next().await {
        match event_result {
            Ok(event) => state.handle_event(event, &tx_event).await?,
            Err(err) => return Err(map_anthropic_error(err)),
        }
        drain_buffered_events(&mut rx_event, &mut events)?;
    }

    state.finish(&tx_event).await?;
    drop(tx_event);

    while let Some(event) = rx_event.recv().await {
        events.push(event?);
    }

    Ok(events)
}

fn drain_buffered_events(
    rx_event: &mut mpsc::Receiver<Result<ResponseEvent>>,
    events: &mut Vec<ResponseEvent>,
) -> Result<()> {
    loop {
        match rx_event.try_recv() {
            Ok(event) => events.push(event?),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

fn append_output_schema_retry_message(
    prompt: &mut Prompt,
    validation_error: &str,
    previous_output: &str,
) {
    let (previous_output, truncated) =
        if previous_output.len() > ANTHROPIC_SCHEMA_REPAIR_PREVIOUS_OUTPUT_MAX_BYTES {
            let mut end = ANTHROPIC_SCHEMA_REPAIR_PREVIOUS_OUTPUT_MAX_BYTES;
            while !previous_output.is_char_boundary(end) {
                end -= 1;
            }
            (&previous_output[..end], true)
        } else {
            (previous_output, false)
        };
    let truncated_note = if truncated { "\n\n[truncated]" } else { "" };

    let retry_text = format!(
        "Your previous answer did not satisfy the required JSON Schema.\nValidation error: {validation_error}\nPrevious output:\n{previous_output}{truncated_note}\n\nReturn JSON only, strictly matching the schema."
    );
    prompt.input.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText { text: retry_text }],
        end_turn: None,
        phase: None,
    });
}

fn last_assistant_output_text(events: &[ResponseEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. })
            if role == "assistant" =>
        {
            let text = content
                .iter()
                .filter_map(|item| match item {
                    ContentItem::OutputText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            Some(text)
        }
        _ => None,
    })
}

fn create_anthropic_client(provider: &ModelProviderInfo) -> Result<Anthropic> {
    let (api_key, auth_token) = select_anthropic_credentials(provider)?;
    if api_key.is_none() && auth_token.is_none() {
        let env_key_hint = provider
            .env_key
            .as_deref()
            .map(|name| format!("`{name}`"))
            .unwrap_or_else(|| "`env_key`".to_string());
        return Err(CodexErr::InvalidRequest(format!(
            "anthropic provider requires credentials via {env_key_hint}, `experimental_bearer_token`, or environment variable `{ANTHROPIC_AUTH_TOKEN_ENV_VAR}`"
        )));
    }

    let max_retries = provider.request_max_retries().min(u64::from(u32::MAX)) as u32;
    let options = ClientOptions {
        api_key,
        auth_token,
        base_url: provider.base_url.clone(),
        timeout: Some(provider.stream_idle_timeout()),
        max_retries: Some(max_retries),
        default_headers: provider_headers(provider),
    };
    Anthropic::new(options).map_err(map_anthropic_error)
}

fn select_anthropic_credentials(
    provider: &ModelProviderInfo,
) -> Result<(Option<String>, Option<String>)> {
    let provider_key = provider.api_key();
    let fallback_auth_token = anthropic_auth_token_from_env();
    resolve_anthropic_credentials(
        provider.env_key.as_deref(),
        provider_key,
        provider.experimental_bearer_token.clone(),
        fallback_auth_token,
    )
}

fn anthropic_auth_token_from_env() -> Option<String> {
    std::env::var(ANTHROPIC_AUTH_TOKEN_ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn resolve_anthropic_credentials(
    env_key_name: Option<&str>,
    provider_key: Result<Option<String>>,
    experimental_bearer_token: Option<String>,
    fallback_auth_token: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let has_auth_token_fallback =
        experimental_bearer_token.is_some() || fallback_auth_token.is_some();

    let provider_key = match provider_key {
        Ok(provider_key) => provider_key,
        Err(err) => match err {
            CodexErr::EnvVar(EnvVarError { .. }) if has_auth_token_fallback => None,
            other => return Err(other),
        },
    };

    if env_key_name == Some(ANTHROPIC_AUTH_TOKEN_ENV_VAR) {
        let auth_token = provider_key
            .or(experimental_bearer_token)
            .or(fallback_auth_token);
        return Ok((None, auth_token));
    }

    if provider_key.is_some() {
        return Ok((provider_key, None));
    }

    let auth_token = experimental_bearer_token.or(fallback_auth_token);
    Ok((None, auth_token))
}

fn provider_headers(provider: &ModelProviderInfo) -> HeaderMap {
    let mut headers = HeaderMap::new();
    insert_static_headers(&mut headers, provider.http_headers.as_ref());
    insert_env_headers(&mut headers, provider.env_http_headers.as_ref());
    headers
}

fn insert_static_headers(headers: &mut HeaderMap, source: Option<&HashMap<String, String>>) {
    let Some(source) = source else {
        return;
    };

    for (name, value) in source {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::try_from(value.as_str()),
        ) {
            headers.insert(name, value);
        }
    }
}

fn insert_env_headers(headers: &mut HeaderMap, source: Option<&HashMap<String, String>>) {
    let Some(source) = source else {
        return;
    };

    for (header_name, env_var) in source {
        if let Ok(env_value) = std::env::var(env_var)
            && !env_value.trim().is_empty()
            && let (Ok(name), Ok(value)) = (
                HeaderName::try_from(header_name.as_str()),
                HeaderValue::try_from(env_value.as_str()),
            )
        {
            headers.insert(name, value);
        }
    }
}

fn build_anthropic_request(prompt: &Prompt, model_info: &ModelInfo) -> Result<MessageCreateParams> {
    let mut messages = Vec::<MessageParam>::new();
    let mut system_segments = vec![prompt.base_instructions.text.clone()];
    if let Some(output_schema) = &prompt.output_schema {
        system_segments.push(anthropic_output_schema_instruction(output_schema));
    }

    for item in prompt.get_formatted_input() {
        match item {
            ResponseItem::Message { role, content, .. } => {
                if matches!(role.as_str(), "system" | "developer") {
                    let text = content_text(&content);
                    if !text.trim().is_empty() {
                        system_segments.push(text);
                    }
                    continue;
                }

                if matches!(role.as_str(), "user" | "assistant") {
                    let blocks = content_blocks(&content);
                    if blocks.is_empty() {
                        continue;
                    }
                    messages.push(MessageParam {
                        role,
                        content: anthropic_sdk::types::messages::MessageContent::Blocks(blocks),
                    });
                }
            }
            ResponseItem::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            } => {
                messages.push(anthropic_tool_use_message(
                    name,
                    call_id,
                    parse_json_object_or_wrapped(&arguments),
                ));
            }
            ResponseItem::CustomToolCall {
                name,
                call_id,
                input,
                ..
            } => {
                messages.push(anthropic_tool_use_message(
                    name,
                    call_id,
                    parse_json_object_or_wrapped(&input),
                ));
            }
            ResponseItem::LocalShellCall {
                call_id,
                id,
                action,
                ..
            } => {
                let call_id = call_id
                    .or(id)
                    .unwrap_or_else(|| "local_shell_call".to_string());
                messages.push(anthropic_tool_use_message(
                    "local_shell".to_string(),
                    call_id,
                    local_shell_input(&action),
                ));
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                messages.push(anthropic_tool_result_message(
                    call_id,
                    anthropic_tool_result_text(&output),
                    output.success == Some(false),
                ));
            }
            ResponseItem::CustomToolCallOutput { call_id, output } => {
                messages.push(anthropic_tool_result_message(call_id, output, false));
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        messages.push(MessageParam::user(String::new()));
    }

    let mut extra = BTreeMap::<String, Value>::new();
    let system = system_segments
        .into_iter()
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<String>>()
        .join("\n\n");
    if !system.is_empty() {
        extra.insert("system".to_string(), Value::String(system));
    }

    let tools = tool_specs_to_anthropic_tools(&prompt.tools);
    if !tools.is_empty() {
        extra.insert("tools".to_string(), Value::Array(tools));
        if !prompt.parallel_tool_calls {
            extra.insert(
                "tool_choice".to_string(),
                json!({
                    "type": "auto",
                    "disable_parallel_tool_use": true,
                }),
            );
        }
    }

    Ok(MessageCreateParams {
        model: model_info.slug.clone(),
        max_tokens: ANTHROPIC_DEFAULT_MAX_TOKENS,
        messages,
        stream: Some(true),
        extra,
    })
}

fn anthropic_output_schema_instruction(output_schema: &Value) -> String {
    let schema = serde_json::to_string(output_schema).unwrap_or_else(|_| output_schema.to_string());
    format!("{ANTHROPIC_OUTPUT_SCHEMA_INSTRUCTIONS} {schema}")
}

fn parse_base64_data_url(image_url: &str) -> Option<(&str, &str)> {
    let rest = image_url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if data.trim().is_empty() {
        return None;
    }

    let mut parts = meta.split(';');
    let media_type = parts.next()?.trim();
    if media_type.is_empty() {
        return None;
    }
    let is_base64 = parts.any(|part| part.trim().eq_ignore_ascii_case("base64"));
    if !is_base64 {
        return None;
    }

    Some((media_type, data))
}

fn content_text(content: &[ContentItem]) -> String {
    content
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text.clone(),
            ContentItem::InputImage { image_url } if parse_base64_data_url(image_url).is_some() => {
                "[image: data-url]".to_string()
            }
            ContentItem::InputImage { image_url } => format!("[image: {image_url}]"),
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn content_blocks(content: &[ContentItem]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text }
                if !text.is_empty() =>
            {
                blocks.push(json!({
                    "type": "text",
                    "text": text,
                }));
            }
            ContentItem::InputImage { image_url } => {
                if let Some((media_type, data)) = parse_base64_data_url(image_url) {
                    blocks.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data,
                        },
                    }));
                } else if image_url.starts_with("data:") {
                    blocks.push(json!({
                        "type": "text",
                        "text": "[image: data-url]",
                    }));
                } else {
                    blocks.push(json!({
                        "type": "text",
                        "text": format!("[image: {image_url}]"),
                    }));
                }
            }
            _ => {}
        }
    }
    blocks
}

fn anthropic_tool_use_message(name: String, call_id: String, input: Value) -> MessageParam {
    MessageParam {
        role: "assistant".to_string(),
        content: anthropic_sdk::types::messages::MessageContent::Blocks(vec![json!({
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": input,
        })]),
    }
}

fn anthropic_tool_result_message(call_id: String, output: String, is_error: bool) -> MessageParam {
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": output,
    });
    if is_error && let Some(object) = block.as_object_mut() {
        object.insert("is_error".to_string(), Value::Bool(true));
    }

    MessageParam {
        role: "user".to_string(),
        content: anthropic_sdk::types::messages::MessageContent::Blocks(vec![block]),
    }
}

fn anthropic_tool_result_text(output: &FunctionCallOutputPayload) -> String {
    output.body.to_text().unwrap_or_else(|| output.to_string())
}

fn local_shell_input(action: &LocalShellAction) -> Value {
    match action {
        LocalShellAction::Exec(exec) => json!({
            "command": exec.command,
            "workdir": exec.working_directory,
            "timeout_ms": exec.timeout_ms,
        }),
    }
}

fn parse_json_object_or_wrapped(input: &str) -> Value {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(Value::Null) => Value::Object(Map::new()),
        Ok(other) => {
            let mut object = Map::new();
            object.insert(TOOL_INPUT_FIELD.to_string(), other);
            Value::Object(object)
        }
        Err(_) => {
            let mut object = Map::new();
            object.insert(
                TOOL_INPUT_FIELD.to_string(),
                Value::String(input.to_string()),
            );
            Value::Object(object)
        }
    }
}

fn tool_specs_to_anthropic_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .filter_map(tool_spec_to_anthropic_tool)
        .collect()
}

fn tool_spec_to_anthropic_tool(spec: &ToolSpec) -> Option<Value> {
    match spec {
        ToolSpec::Function(function_tool) => {
            let input_schema = serde_json::to_value(&function_tool.parameters).ok()?;
            Some(json!({
                "name": function_tool.name,
                "description": function_tool.description,
                "input_schema": input_schema,
            }))
        }
        ToolSpec::Freeform(tool) => Some(json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": {
                "type": "object",
                "properties": {
                    TOOL_INPUT_FIELD: {
                        "type": "string",
                        "description": "Raw freeform tool input.",
                    }
                },
                "required": [TOOL_INPUT_FIELD],
                "additionalProperties": false,
            },
        })),
        ToolSpec::LocalShell {} => Some(json!({
            "name": "local_shell",
            "description": "Runs a local shell command and returns its output.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "array", "items": { "type": "string" } },
                    "workdir": { "type": "string" },
                    "timeout_ms": { "type": "number" },
                    "sandbox_permissions": { "type": "string" },
                    "justification": { "type": "string" },
                    "prefix_rule": { "type": "array", "items": { "type": "string" } },
                },
                "required": ["command"],
                "additionalProperties": false,
            },
        })),
        ToolSpec::WebSearch { .. } => Some(json!({
            "name": "web_search",
            "description": "Searches the web for public information.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "external_web_access": { "type": "boolean" },
                },
                "additionalProperties": false,
            },
        })),
    }
}

#[derive(Default)]
struct AnthropicStreamState {
    response_id: Option<String>,
    message_id: Option<String>,
    message_item_started: bool,
    reasoning_item_started: bool,
    reasoning_item_done: bool,
    completed: bool,
    text_blocks: BTreeMap<usize, String>,
    reasoning_blocks: BTreeMap<usize, String>,
    tool_blocks: BTreeMap<usize, ToolUseState>,
    reasoning_summary_started: HashSet<usize>,
    usage: Option<MessageDeltaUsage>,
    stop_reason: Option<String>,
    freeform_tool_names: HashSet<String>,
}

#[derive(Default)]
struct ToolUseState {
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    partial_json: String,
}

impl AnthropicStreamState {
    fn new(tool_specs: &[ToolSpec]) -> Self {
        let freeform_tool_names = tool_specs
            .iter()
            .filter_map(|tool| match tool {
                ToolSpec::Freeform(tool) => Some(tool.name.clone()),
                _ => None,
            })
            .collect::<HashSet<String>>();
        Self {
            freeform_tool_names,
            ..Self::default()
        }
    }

    async fn handle_event(
        &mut self,
        event: RawMessageStreamEvent,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        match event {
            RawMessageStreamEvent::MessageStart { message } => {
                self.response_id = Some(message.id.clone());
                self.message_id = Some(message.id);
                self.message_item_started = false;
                self.reasoning_item_started = false;
                self.reasoning_item_done = false;
                self.completed = false;
                self.text_blocks.clear();
                self.reasoning_blocks.clear();
                self.tool_blocks.clear();
                self.reasoning_summary_started.clear();
                self.usage = None;
                self.stop_reason = None;
                send_event(tx_event, ResponseEvent::Created).await?;
            }
            RawMessageStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.handle_content_block_start(index, content_block),
            RawMessageStreamEvent::ContentBlockDelta { index, delta } => {
                self.handle_content_block_delta(index, delta, tx_event)
                    .await?
            }
            RawMessageStreamEvent::MessageDelta { delta, usage } => {
                self.usage = Some(usage);
                self.stop_reason = delta.stop_reason;
            }
            RawMessageStreamEvent::ContentBlockStop { .. } => {}
            RawMessageStreamEvent::MessageStop => {
                self.finish(tx_event).await?;
            }
        }
        Ok(())
    }

    fn handle_content_block_start(&mut self, index: usize, content_block: Value) {
        let Some(content_type) = content_block.get("type").and_then(Value::as_str) else {
            return;
        };

        match content_type {
            "text" => {
                let text = content_block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.text_blocks.insert(index, text);
            }
            "tool_use" => {
                let entry = self.tool_blocks.entry(index).or_default();
                if let Some(id) = content_block.get("id").and_then(Value::as_str) {
                    entry.id = Some(id.to_string());
                }
                if let Some(name) = content_block.get("name").and_then(Value::as_str) {
                    entry.name = Some(name.to_string());
                }
                if let Some(input) = content_block.get("input") {
                    entry.input = Some(input.clone());
                }
            }
            _ => {}
        }
    }

    async fn handle_content_block_delta(
        &mut self,
        index: usize,
        delta: RawContentBlockDelta,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        match delta {
            RawContentBlockDelta::TextDelta { text } => {
                self.ensure_message_started(tx_event).await?;
                self.text_blocks.entry(index).or_default().push_str(&text);
                send_event(tx_event, ResponseEvent::OutputTextDelta(text)).await?;
            }
            RawContentBlockDelta::ThinkingDelta { thinking } => {
                self.handle_reasoning_delta(index, thinking, tx_event)
                    .await?;
            }
            RawContentBlockDelta::InputJsonDelta { partial_json } => {
                self.tool_blocks
                    .entry(index)
                    .or_default()
                    .partial_json
                    .push_str(&partial_json);
            }
            RawContentBlockDelta::SignatureDelta { .. }
            | RawContentBlockDelta::CitationsDelta { .. }
            | RawContentBlockDelta::Unknown => {}
        }
        Ok(())
    }

    async fn handle_reasoning_delta(
        &mut self,
        index: usize,
        thinking: String,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if !self.reasoning_item_started {
            self.reasoning_item_started = true;
            self.reasoning_item_done = false;
            send_event(
                tx_event,
                ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                    id: self.message_id.clone().unwrap_or_default(),
                    summary: Vec::new(),
                    content: None,
                    encrypted_content: None,
                }),
            )
            .await?;
        }

        if self.reasoning_summary_started.insert(index) {
            send_event(
                tx_event,
                ResponseEvent::ReasoningSummaryPartAdded {
                    summary_index: index as i64,
                },
            )
            .await?;
        }

        self.reasoning_blocks
            .entry(index)
            .or_default()
            .push_str(&thinking);
        send_event(
            tx_event,
            ResponseEvent::ReasoningSummaryDelta {
                delta: thinking,
                summary_index: index as i64,
            },
        )
        .await
    }

    async fn finish_reasoning_if_needed(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if !self.reasoning_item_started || self.reasoning_item_done {
            return Ok(());
        }
        self.reasoning_item_done = true;

        let text = self
            .reasoning_blocks
            .values()
            .map(String::as_str)
            .collect::<String>();
        let summary = if text.is_empty() {
            Vec::new()
        } else {
            vec![ReasoningItemReasoningSummary::SummaryText { text: text.clone() }]
        };
        let content = if text.is_empty() {
            None
        } else {
            Some(vec![ReasoningItemContent::ReasoningText { text }])
        };
        send_event(
            tx_event,
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                id: self.message_id.clone().unwrap_or_default(),
                summary,
                content,
                encrypted_content: None,
            }),
        )
        .await
    }

    async fn ensure_message_started(
        &mut self,
        tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    ) -> Result<()> {
        if self.message_item_started {
            return Ok(());
        }

        self.finish_reasoning_if_needed(tx_event).await?;
        self.message_item_started = true;
        send_event(
            tx_event,
            ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: self.message_id.clone(),
                role: "assistant".to_string(),
                content: Vec::new(),
                end_turn: None,
                phase: None,
            }),
        )
        .await
    }

    async fn finish(&mut self, tx_event: &mpsc::Sender<Result<ResponseEvent>>) -> Result<()> {
        if self.completed {
            return Ok(());
        }
        self.completed = true;

        self.finish_reasoning_if_needed(tx_event).await?;
        if !self.message_item_started && !self.text_blocks.is_empty() {
            self.ensure_message_started(tx_event).await?;
        }

        if self.message_item_started {
            let text = self
                .text_blocks
                .values()
                .map(String::as_str)
                .collect::<String>();
            send_event(
                tx_event,
                ResponseEvent::OutputItemDone(ResponseItem::Message {
                    id: self.message_id.clone(),
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText { text }],
                    end_turn: self.message_end_turn(),
                    phase: None,
                }),
            )
            .await?;
        }

        for (index, tool) in &self.tool_blocks {
            if let Some(item) = tool_use_to_response_item(*index, tool, &self.freeform_tool_names) {
                send_event(tx_event, ResponseEvent::OutputItemDone(item)).await?;
            }
        }

        let response_id = self
            .response_id
            .clone()
            .unwrap_or_else(|| "anthropic-response".to_string());
        send_event(
            tx_event,
            ResponseEvent::Completed {
                response_id,
                token_usage: self.token_usage(),
                can_append: false,
            },
        )
        .await
    }

    fn message_end_turn(&self) -> Option<bool> {
        match self.stop_reason.as_deref() {
            Some("tool_use") => Some(false),
            Some("end_turn") => Some(true),
            _ => None,
        }
    }

    fn token_usage(&self) -> Option<TokenUsage> {
        self.usage.as_ref().map(|usage| {
            let input_tokens = usage.input_tokens.unwrap_or_default() as i64;
            let output_tokens = usage.output_tokens as i64;
            TokenUsage {
                input_tokens,
                cached_input_tokens: usage.cache_read_input_tokens.unwrap_or_default() as i64,
                output_tokens,
                reasoning_output_tokens: 0,
                total_tokens: input_tokens + output_tokens,
            }
        })
    }
}

fn tool_use_to_response_item(
    index: usize,
    tool: &ToolUseState,
    freeform_tool_names: &HashSet<String>,
) -> Option<ResponseItem> {
    let name = tool
        .name
        .clone()
        .unwrap_or_else(|| format!("anthropic_tool_missing_name_{index}"));
    let call_id = tool
        .id
        .clone()
        .unwrap_or_else(|| format!("anthropic_tool_{index}"));
    let input = tool_input_value(tool);
    if freeform_tool_names.contains(&name) {
        let text = match input {
            Value::Object(mut object) => match object.remove(TOOL_INPUT_FIELD) {
                Some(Value::String(text)) => text,
                Some(value) => value.to_string(),
                None => Value::Object(object).to_string(),
            },
            value => value.to_string(),
        };
        return Some(ResponseItem::CustomToolCall {
            id: None,
            status: None,
            call_id,
            name,
            input: text,
        });
    }

    let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    Some(ResponseItem::FunctionCall {
        id: None,
        name,
        arguments,
        call_id,
    })
}

fn tool_input_value(tool: &ToolUseState) -> Value {
    let start_input = tool
        .input
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !tool.partial_json.is_empty() {
        if let Ok(Value::Object(partial_object)) = serde_json::from_str::<Value>(&tool.partial_json)
        {
            return merge_object_values(start_input, Value::Object(partial_object));
        }
        let mut object = match start_input {
            Value::Object(object) => object,
            value => {
                let mut object = Map::new();
                object.insert(TOOL_INPUT_FIELD.to_string(), value);
                object
            }
        };
        object.insert(
            "raw_partial_json".to_string(),
            Value::String(tool.partial_json.clone()),
        );
        return Value::Object(object);
    }

    start_input
}

fn merge_object_values(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base), Value::Object(overlay)) => {
            base.extend(overlay);
            Value::Object(base)
        }
        (_, overlay) => overlay,
    }
}

fn validate_assistant_output_against_schema(
    schema: &Value,
    assistant_output: &str,
) -> std::result::Result<(), String> {
    let parsed_output = serde_json::from_str::<Value>(assistant_output)
        .map_err(|err| format!("assistant output is not valid JSON: {err}"))?;
    let compiled_schema =
        JSONSchema::compile(schema).map_err(|err| format!("invalid output schema: {err}"))?;
    if let Err(errors) = compiled_schema.validate(&parsed_output) {
        let details = errors
            .take(3)
            .map(|err| err.to_string())
            .collect::<Vec<_>>();
        let details = if details.is_empty() {
            "no details".to_string()
        } else {
            details.join("; ")
        };
        return Err(format!("assistant output does not match schema: {details}"));
    }
    Ok(())
}

fn find_schema_matching_json_in_output(
    compiled_schema: &JSONSchema,
    assistant_output: &str,
    offset: usize,
) -> Option<(usize, String)> {
    let bytes = assistant_output.as_bytes();
    let mut stack = Vec::<usize>::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut best: Option<(usize, String)> = None;

    for end in 0..bytes.len() {
        let byte = bytes[end];
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        if byte == b'"' {
            in_string = true;
            continue;
        }

        if byte == b'{' {
            stack.push(end);
            continue;
        }

        if byte == b'}' {
            let Some(start) = stack.pop() else {
                continue;
            };

            let candidate = &assistant_output[start..=end];
            if let Ok(value) = serde_json::from_str::<Value>(candidate)
                && compiled_schema.is_valid(&value)
                && let Ok(serialized) = serde_json::to_string(&value)
            {
                let start_index = offset + start;
                match &best {
                    Some((best_start, _)) if *best_start <= start_index => {}
                    _ => best = Some((start_index, serialized)),
                }
            }
        }
    }

    best
}

fn extract_schema_matching_json(schema: &Value, assistant_output: &str) -> Option<String> {
    let compiled_schema = JSONSchema::compile(schema).ok()?;
    if let Ok(value) = serde_json::from_str::<Value>(assistant_output)
        && compiled_schema.is_valid(&value)
    {
        return serde_json::to_string(&value).ok();
    }

    if assistant_output.len() <= ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES {
        return find_schema_matching_json_in_output(&compiled_schema, assistant_output, 0)
            .map(|(_, candidate)| candidate);
    }

    let mut suffix_start = assistant_output
        .len()
        .saturating_sub(ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES);
    while !assistant_output.is_char_boundary(suffix_start) {
        suffix_start += 1;
    }
    let suffix = &assistant_output[suffix_start..];
    let mut best = find_schema_matching_json_in_output(&compiled_schema, suffix, suffix_start);

    let mut prefix_end = ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES.min(assistant_output.len());
    while !assistant_output.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    let prefix = &assistant_output[..prefix_end];
    if let Some(candidate) = find_schema_matching_json_in_output(&compiled_schema, prefix, 0) {
        match &best {
            Some((best_start, _)) if *best_start <= candidate.0 => {}
            _ => best = Some(candidate),
        }
    }

    best.map(|(_, candidate)| candidate)
}

fn replace_last_assistant_message(events: &mut Vec<ResponseEvent>, normalized_json: &str) {
    let Some(last_done_index) = events.iter().rposition(|event| {
        matches!(
            event,
            ResponseEvent::OutputItemDone(ResponseItem::Message { role, .. }) if role == "assistant"
        )
    }) else {
        return;
    };

    if let ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) =
        &mut events[last_done_index]
    {
        *content = vec![ContentItem::OutputText {
            text: normalized_json.to_string(),
        }];
    }

    let delta_start = events
        .iter()
        .take(last_done_index)
        .rposition(|event| {
            matches!(
                event,
                ResponseEvent::OutputItemAdded(ResponseItem::Message { role, .. }) if role == "assistant"
            )
        })
        .map_or(0, |index| index + 1);

    let delta_indices = (delta_start..last_done_index)
        .filter(|&index| matches!(events[index], ResponseEvent::OutputTextDelta(_)))
        .collect::<Vec<_>>();
    if let Some(first_index) = delta_indices.first().copied() {
        events[first_index] = ResponseEvent::OutputTextDelta(normalized_json.to_string());
        for index in delta_indices.into_iter().skip(1).rev() {
            events.remove(index);
        }
    }
}

async fn send_event(
    tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    event: ResponseEvent,
) -> Result<()> {
    tx_event
        .send(Ok(event))
        .await
        .map_err(|_| CodexErr::TurnAborted)
}

async fn send_events(
    tx_event: &mpsc::Sender<Result<ResponseEvent>>,
    events: Vec<ResponseEvent>,
) -> Result<()> {
    for event in events {
        send_event(tx_event, event).await?;
    }
    Ok(())
}

fn map_anthropic_error(err: AnthropicError) -> CodexErr {
    match err {
        AnthropicError::AuthMissing => CodexErr::InvalidRequest(
            "anthropic authentication missing: configure `env_key` or `experimental_bearer_token`"
                .to_string(),
        ),
        AnthropicError::Http(http_error) => map_anthropic_http_error(http_error),
        AnthropicError::Timeout => CodexErr::Timeout,
        AnthropicError::Transport(source) => {
            if source.is_timeout() {
                CodexErr::Timeout
            } else {
                CodexErr::Stream(source.to_string(), None)
            }
        }
        AnthropicError::Json(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::Url(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::InvalidHeaderValue(error) => CodexErr::Stream(error.to_string(), None),
        AnthropicError::InvalidSse(message)
        | AnthropicError::InvalidJsonl(message)
        | AnthropicError::Internal(message) => CodexErr::Stream(message, None),
        AnthropicError::Aborted => CodexErr::TurnAborted,
    }
}

fn map_anthropic_http_error(err: HttpApiError) -> CodexErr {
    match err {
        HttpApiError::BadRequest(api_error) => {
            let message = api_error
                .message
                .unwrap_or_else(|| "bad request".to_string());
            CodexErr::InvalidRequest(message)
        }
        HttpApiError::RateLimit(api_error) => CodexErr::RetryLimit(RetryLimitReachedError {
            status: StatusCode::TOO_MANY_REQUESTS,
            request_id: api_error.request_id,
        }),
        HttpApiError::InternalServer(_) => CodexErr::InternalServerError,
        HttpApiError::Authentication(api_error) => {
            map_to_unexpected_status(StatusCode::UNAUTHORIZED, api_error)
        }
        HttpApiError::PermissionDenied(api_error) => {
            map_to_unexpected_status(StatusCode::FORBIDDEN, api_error)
        }
        HttpApiError::NotFound(api_error) => {
            map_to_unexpected_status(StatusCode::NOT_FOUND, api_error)
        }
        HttpApiError::Conflict(api_error) => {
            map_to_unexpected_status(StatusCode::CONFLICT, api_error)
        }
        HttpApiError::UnprocessableEntity(api_error) => {
            map_to_unexpected_status(StatusCode::UNPROCESSABLE_ENTITY, api_error)
        }
        HttpApiError::Other(api_error) => {
            let status = api_error
                .status
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            map_to_unexpected_status(status, api_error)
        }
    }
}

fn map_to_unexpected_status(status: StatusCode, api_error: anthropic_sdk::ApiError) -> CodexErr {
    let body = api_error
        .message
        .or_else(|| api_error.body.as_ref().map(Value::to_string))
        .unwrap_or_else(|| "unknown error".to_string());
    CodexErr::UnexpectedStatus(UnexpectedResponseError {
        status,
        body,
        url: None,
        cf_ray: None,
        request_id: api_error.request_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_common::tools::FreeformTool;
    use crate::client_common::tools::FreeformToolFormat;
    use crate::client_common::tools::ResponsesApiTool;
    use crate::tools::spec::AdditionalProperties;
    use crate::tools::spec::JsonSchema;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use tokio::sync::mpsc::error::TryRecvError;

    fn missing_env_error(var: &str) -> CodexErr {
        CodexErr::EnvVar(EnvVarError {
            var: var.to_string(),
            instructions: None,
        })
    }

    fn test_model_info() -> ModelInfo {
        serde_json::from_value::<ModelInfo>(json!({
            "slug": "claude-test",
            "display_name": "claude-test",
            "description": "desc",
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort":"medium","description":"medium"}],
            "shell_type": "shell_command",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 1,
            "upgrade": null,
            "base_instructions": "base",
            "model_messages": null,
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode":"bytes","limit":10000},
            "supports_parallel_tool_calls": true,
            "context_window": 200000,
            "auto_compact_token_limit": null,
            "experimental_supported_tools": []
        }))
        .expect("deserialize model info")
    }

    #[test]
    fn resolve_credentials_accepts_auth_token_env_fallback() {
        let (api_key, auth_token) = resolve_anthropic_credentials(
            Some("ANTHROPIC_API_KEY"),
            Ok(None),
            None,
            Some("auth-token".to_string()),
        )
        .expect("resolve credentials");

        assert_eq!(api_key, None);
        assert_eq!(auth_token, Some("auth-token".to_string()));
    }

    #[test]
    fn resolve_credentials_uses_auth_token_when_env_key_points_to_auth_token() {
        let (api_key, auth_token) = resolve_anthropic_credentials(
            Some(ANTHROPIC_AUTH_TOKEN_ENV_VAR),
            Ok(Some("session-token".to_string())),
            Some("config-token".to_string()),
            Some("env-token".to_string()),
        )
        .expect("resolve credentials");

        assert_eq!(api_key, None);
        assert_eq!(auth_token, Some("session-token".to_string()));
    }

    #[test]
    fn resolve_credentials_does_not_mix_api_key_and_auth_token() {
        let (api_key, auth_token) = resolve_anthropic_credentials(
            Some("ANTHROPIC_API_KEY"),
            Ok(Some("api-key".to_string())),
            Some("config-token".to_string()),
            Some("env-token".to_string()),
        )
        .expect("resolve credentials");

        assert_eq!(api_key, Some("api-key".to_string()));
        assert_eq!(auth_token, None);
    }

    #[test]
    fn resolve_credentials_falls_back_to_bearer_when_api_key_missing() {
        let (api_key, auth_token) = resolve_anthropic_credentials(
            Some("ANTHROPIC_API_KEY"),
            Err(missing_env_error("ANTHROPIC_API_KEY")),
            Some("config-token".to_string()),
            None,
        )
        .expect("resolve credentials");

        assert_eq!(api_key, None);
        assert_eq!(auth_token, Some("config-token".to_string()));
    }

    #[test]
    fn resolve_credentials_preserves_non_env_errors() {
        let err = resolve_anthropic_credentials(
            Some("ANTHROPIC_API_KEY"),
            Err(CodexErr::InvalidRequest("bad provider".to_string())),
            Some("config-token".to_string()),
            None,
        )
        .expect_err("non-env error should be returned");

        assert!(matches!(err, CodexErr::InvalidRequest(_)));
    }

    #[test]
    fn builds_anthropic_request_with_tool_and_tool_results() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "hello".to_string(),
                    }],
                    end_turn: None,
                    phase: None,
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    arguments: r#"{"command":["pwd"]}"#.to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
            ],
            tools: vec![
                ToolSpec::Function(ResponsesApiTool {
                    name: "shell".to_string(),
                    description: "Run shell".to_string(),
                    strict: false,
                    parameters: JsonSchema::Object {
                        properties: BTreeMap::from([(
                            "command".to_string(),
                            JsonSchema::Array {
                                items: Box::new(JsonSchema::String { description: None }),
                                description: None,
                            },
                        )]),
                        required: Some(vec!["command".to_string()]),
                        additional_properties: Some(AdditionalProperties::Boolean(false)),
                    },
                }),
                ToolSpec::Freeform(FreeformTool {
                    name: "apply_patch".to_string(),
                    description: "Patch files".to_string(),
                    format: FreeformToolFormat {
                        r#type: "grammar".to_string(),
                        syntax: "lark".to_string(),
                        definition: "dummy".to_string(),
                    },
                }),
            ],
            parallel_tool_calls: true,
            base_instructions: codex_protocol::models::BaseInstructions {
                text: "be concise".to_string(),
            },
            personality: None,
            output_schema: None,
        };
        let model_info = test_model_info();

        let request = build_anthropic_request(&prompt, &model_info).expect("build request");
        assert_eq!(request.model, "claude-test");
        assert_eq!(request.stream, Some(true));
        assert_eq!(request.messages.len(), 3);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(request.messages[1].role, "assistant");
        assert_eq!(request.messages[2].role, "user");

        let Some(Value::Array(tools)) = request.extra.get("tools") else {
            panic!("tools should be serialized");
        };
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].get("name").and_then(Value::as_str), Some("shell"));
        assert_eq!(
            tools[1].get("name").and_then(Value::as_str),
            Some("apply_patch")
        );
        assert_eq!(
            request.extra.get("system").and_then(Value::as_str),
            Some("be concise"),
        );
    }

    #[test]
    fn builds_anthropic_request_includes_output_schema_instruction() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hi".to_string(),
                }],
                end_turn: None,
                phase: None,
            }],
            tools: Vec::new(),
            parallel_tool_calls: true,
            base_instructions: codex_protocol::models::BaseInstructions {
                text: "be concise".to_string(),
            },
            personality: None,
            output_schema: Some(schema.clone()),
        };
        let model_info = test_model_info();

        let request = build_anthropic_request(&prompt, &model_info).expect("build request");
        let system = request
            .extra
            .get("system")
            .and_then(Value::as_str)
            .expect("system should be present");
        assert!(system.contains("be concise"));
        assert!(system.contains(ANTHROPIC_OUTPUT_SCHEMA_INSTRUCTIONS));
        let serialized_schema = serde_json::to_string(&schema).expect("serialize schema");
        assert!(system.contains(&serialized_schema));
    }

    #[test]
    fn builds_anthropic_request_maps_base64_input_image_to_image_block() {
        let prompt = Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputImage {
                        image_url: "data:image/png;base64,AAA".to_string(),
                    },
                    ContentItem::InputText {
                        text: "hi".to_string(),
                    },
                ],
                end_turn: None,
                phase: None,
            }],
            tools: Vec::new(),
            parallel_tool_calls: true,
            base_instructions: codex_protocol::models::BaseInstructions {
                text: "be concise".to_string(),
            },
            personality: None,
            output_schema: None,
        };
        let model_info = test_model_info();

        let request = build_anthropic_request(&prompt, &model_info).expect("build request");
        assert_eq!(request.messages.len(), 1);

        let anthropic_sdk::types::messages::MessageContent::Blocks(blocks) =
            &request.messages[0].content
        else {
            panic!("expected message blocks");
        };

        assert_eq!(
            blocks[0],
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "AAA",
                },
            })
        );
        assert_eq!(
            blocks[1],
            json!({
                "type": "text",
                "text": "hi",
            })
        );
    }

    #[test]
    fn append_output_schema_retry_message_truncates_previous_output() {
        let mut prompt = Prompt {
            input: Vec::new(),
            tools: Vec::new(),
            parallel_tool_calls: true,
            base_instructions: codex_protocol::models::BaseInstructions {
                text: "be concise".to_string(),
            },
            personality: None,
            output_schema: None,
        };

        let previous_output = "x".repeat(ANTHROPIC_SCHEMA_REPAIR_PREVIOUS_OUTPUT_MAX_BYTES + 10);
        append_output_schema_retry_message(&mut prompt, "bad output", &previous_output);

        let Some(ResponseItem::Message { content, .. }) = prompt.input.last() else {
            panic!("expected retry message");
        };
        let [ContentItem::InputText { text }] = content.as_slice() else {
            panic!("expected retry text content");
        };

        assert!(text.contains("Validation error: bad output"));
        assert!(text.contains("[truncated]"));
        assert!(!text.contains(&previous_output));
    }

    #[tokio::test]
    async fn streams_message_then_tool_call_events_in_order() {
        let mut state = AnthropicStreamState::default();
        let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(32);

        state
            .handle_event(
                RawMessageStreamEvent::MessageStart {
                    message: anthropic_sdk::types::messages::Message {
                        id: "resp_1".to_string(),
                        ..Default::default()
                    },
                },
                &tx_event,
            )
            .await
            .expect("message start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: json!({"type":"text","text":""}),
                },
                &tx_event,
            )
            .await
            .expect("content block start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: RawContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("text delta");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 1,
                    content_block: json!({
                        "type":"tool_use",
                        "id":"call_1",
                        "name":"shell",
                        "input":{"command":["pwd"]}
                    }),
                },
                &tx_event,
            )
            .await
            .expect("tool block start");
        state
            .handle_event(
                RawMessageStreamEvent::MessageDelta {
                    delta: anthropic_sdk::types::messages::MessageDelta {
                        container: None,
                        stop_reason: Some("tool_use".to_string()),
                        stop_sequence: None,
                    },
                    usage: MessageDeltaUsage {
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: Some(2),
                        input_tokens: Some(7),
                        output_tokens: 5,
                        server_tool_use: None,
                        extra: BTreeMap::new(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("message delta");
        state
            .handle_event(RawMessageStreamEvent::MessageStop, &tx_event)
            .await
            .expect("message stop");

        let mut events = Vec::new();
        loop {
            match rx_event.try_recv() {
                Ok(event) => events.push(event.expect("stream event")),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert_eq!(events.len(), 6);
        assert!(matches!(events[0], ResponseEvent::Created));
        assert!(matches!(events[1], ResponseEvent::OutputItemAdded(_)));
        match &events[2] {
            ResponseEvent::OutputTextDelta(delta) => assert_eq!(delta, "Hello"),
            other => panic!("unexpected text delta event: {other:?}"),
        }

        match &events[3] {
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                content, end_turn, ..
            }) => {
                assert_eq!(
                    content,
                    &vec![ContentItem::OutputText {
                        text: "Hello".to_string(),
                    }]
                );
                assert_eq!(*end_turn, Some(false));
            }
            other => panic!("unexpected message done event: {other:?}"),
        }

        match &events[4] {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) => {
                assert_eq!(name, "shell");
                assert_eq!(call_id, "call_1");
                assert_eq!(arguments, r#"{"command":["pwd"]}"#);
            }
            other => panic!("unexpected function call event: {other:?}"),
        }

        match &events[5] {
            ResponseEvent::Completed {
                response_id,
                token_usage: Some(token_usage),
                can_append,
            } => {
                assert_eq!(response_id, "resp_1");
                assert_eq!(token_usage.input_tokens, 7);
                assert_eq!(token_usage.cached_input_tokens, 2);
                assert_eq!(token_usage.output_tokens, 5);
                assert_eq!(token_usage.total_tokens, 12);
                assert!(!can_append);
            }
            other => panic!("unexpected completion event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn merges_input_json_delta_for_tool_arguments() {
        let mut state = AnthropicStreamState::default();
        let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(16);

        state
            .handle_event(
                RawMessageStreamEvent::MessageStart {
                    message: anthropic_sdk::types::messages::Message {
                        id: "resp_2".to_string(),
                        ..Default::default()
                    },
                },
                &tx_event,
            )
            .await
            .expect("message start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: json!({
                        "type":"tool_use",
                        "id":"call_2",
                        "name":"apply_patch"
                    }),
                },
                &tx_event,
            )
            .await
            .expect("tool start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: RawContentBlockDelta::InputJsonDelta {
                        partial_json: r#"{"input":"*** Begin Patch\n"#.to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("tool delta 1");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: RawContentBlockDelta::InputJsonDelta {
                        partial_json: r#"*** End Patch"}"#.to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("tool delta 2");
        state
            .handle_event(RawMessageStreamEvent::MessageStop, &tx_event)
            .await
            .expect("message stop");

        let mut function_call_arguments = None;
        loop {
            match rx_event.try_recv() {
                Ok(Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    arguments,
                    ..
                }))) => {
                    function_call_arguments = Some(arguments);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert_eq!(
            function_call_arguments,
            Some(r#"{"input":"*** Begin Patch\n*** End Patch"}"#.to_string())
        );
    }

    #[tokio::test]
    async fn emits_reasoning_events_for_thinking_then_message() {
        let mut state = AnthropicStreamState::default();
        let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(32);

        state
            .handle_event(
                RawMessageStreamEvent::MessageStart {
                    message: anthropic_sdk::types::messages::Message {
                        id: "resp_reasoning".to_string(),
                        ..Default::default()
                    },
                },
                &tx_event,
            )
            .await
            .expect("message start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: RawContentBlockDelta::ThinkingDelta {
                        thinking: "first-thought".to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("thinking delta");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 1,
                    content_block: json!({"type":"text","text":""}),
                },
                &tx_event,
            )
            .await
            .expect("text start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 1,
                    delta: RawContentBlockDelta::TextDelta {
                        text: "final answer".to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("text delta");
        state
            .handle_event(
                RawMessageStreamEvent::MessageDelta {
                    delta: anthropic_sdk::types::messages::MessageDelta {
                        container: None,
                        stop_reason: Some("end_turn".to_string()),
                        stop_sequence: None,
                    },
                    usage: MessageDeltaUsage {
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: Some(3),
                        input_tokens: Some(8),
                        output_tokens: 6,
                        server_tool_use: None,
                        extra: BTreeMap::new(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("message delta");
        state
            .handle_event(RawMessageStreamEvent::MessageStop, &tx_event)
            .await
            .expect("message stop");

        let mut events = Vec::new();
        loop {
            match rx_event.try_recv() {
                Ok(event) => events.push(event.expect("stream event")),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert_eq!(events.len(), 9);
        assert!(matches!(events[0], ResponseEvent::Created));
        assert!(matches!(
            events[1],
            ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. })
        ));
        assert!(matches!(
            events[2],
            ResponseEvent::ReasoningSummaryPartAdded { summary_index: 0 }
        ));
        match &events[3] {
            ResponseEvent::ReasoningSummaryDelta {
                delta,
                summary_index,
            } => {
                assert_eq!(delta, "first-thought");
                assert_eq!(*summary_index, 0);
            }
            other => panic!("unexpected reasoning delta event: {other:?}"),
        }
        match &events[4] {
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                summary, content, ..
            }) => {
                assert_eq!(
                    summary,
                    &vec![ReasoningItemReasoningSummary::SummaryText {
                        text: "first-thought".to_string()
                    }]
                );
                assert_eq!(
                    content,
                    &Some(vec![ReasoningItemContent::ReasoningText {
                        text: "first-thought".to_string()
                    }])
                );
            }
            other => panic!("unexpected reasoning done event: {other:?}"),
        }
        assert!(matches!(events[5], ResponseEvent::OutputItemAdded(_)));
        match &events[6] {
            ResponseEvent::OutputTextDelta(delta) => assert_eq!(delta, "final answer"),
            other => panic!("unexpected text delta event: {other:?}"),
        }
        match &events[7] {
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                content, end_turn, ..
            }) => {
                assert_eq!(
                    content,
                    &vec![ContentItem::OutputText {
                        text: "final answer".to_string()
                    }]
                );
                assert_eq!(*end_turn, Some(true));
            }
            other => panic!("unexpected message done event: {other:?}"),
        }
        assert!(matches!(
            events[8],
            ResponseEvent::Completed {
                response_id: _,
                token_usage: Some(_),
                can_append: false
            }
        ));
    }

    #[tokio::test]
    async fn emits_message_done_when_start_block_contains_text_without_delta() {
        let mut state = AnthropicStreamState::default();
        let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(16);

        state
            .handle_event(
                RawMessageStreamEvent::MessageStart {
                    message: anthropic_sdk::types::messages::Message {
                        id: "resp_3".to_string(),
                        ..Default::default()
                    },
                },
                &tx_event,
            )
            .await
            .expect("message start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: json!({"type":"text","text":"Hello from start"}),
                },
                &tx_event,
            )
            .await
            .expect("text block start");
        state
            .handle_event(
                RawMessageStreamEvent::MessageDelta {
                    delta: anthropic_sdk::types::messages::MessageDelta {
                        container: None,
                        stop_reason: Some("end_turn".to_string()),
                        stop_sequence: None,
                    },
                    usage: MessageDeltaUsage {
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: Some(1),
                        input_tokens: Some(3),
                        output_tokens: 2,
                        server_tool_use: None,
                        extra: BTreeMap::new(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("message delta");
        state
            .handle_event(RawMessageStreamEvent::MessageStop, &tx_event)
            .await
            .expect("message stop");

        let mut events = Vec::new();
        loop {
            match rx_event.try_recv() {
                Ok(event) => events.push(event.expect("stream event")),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], ResponseEvent::Created));
        assert!(matches!(events[1], ResponseEvent::OutputItemAdded(_)));
        match &events[2] {
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                content, end_turn, ..
            }) => {
                assert_eq!(
                    content,
                    &vec![ContentItem::OutputText {
                        text: "Hello from start".to_string()
                    }]
                );
                assert_eq!(*end_turn, Some(true));
            }
            other => panic!("unexpected message done event: {other:?}"),
        }
        match &events[3] {
            ResponseEvent::Completed {
                response_id,
                token_usage: Some(token_usage),
                can_append,
            } => {
                assert_eq!(response_id, "resp_3");
                assert_eq!(token_usage.input_tokens, 3);
                assert_eq!(token_usage.cached_input_tokens, 1);
                assert_eq!(token_usage.output_tokens, 2);
                assert_eq!(token_usage.total_tokens, 5);
                assert!(!can_append);
            }
            other => panic!("unexpected completion event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn merges_tool_start_fields_when_json_delta_arrives_first() {
        let mut state = AnthropicStreamState::default();
        let (tx_event, mut rx_event) = mpsc::channel::<Result<ResponseEvent>>(16);

        state
            .handle_event(
                RawMessageStreamEvent::MessageStart {
                    message: anthropic_sdk::types::messages::Message {
                        id: "resp_4".to_string(),
                        ..Default::default()
                    },
                },
                &tx_event,
            )
            .await
            .expect("message start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: RawContentBlockDelta::InputJsonDelta {
                        partial_json: r#"{"command":["pwd"]}"#.to_string(),
                    },
                },
                &tx_event,
            )
            .await
            .expect("tool delta before start");
        state
            .handle_event(
                RawMessageStreamEvent::ContentBlockStart {
                    index: 0,
                    content_block: json!({
                        "type":"tool_use",
                        "id":"call_early",
                        "name":"shell",
                        "input":{"command":["noop"]}
                    }),
                },
                &tx_event,
            )
            .await
            .expect("tool start");
        state
            .handle_event(RawMessageStreamEvent::MessageStop, &tx_event)
            .await
            .expect("message stop");

        let mut function_call_arguments = None;
        let mut function_call_name = None;
        let mut function_call_id = None;
        loop {
            match rx_event.try_recv() {
                Ok(Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    arguments,
                    name,
                    call_id,
                    ..
                }))) => {
                    function_call_arguments = Some(arguments);
                    function_call_name = Some(name);
                    function_call_id = Some(call_id);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert_eq!(function_call_name, Some("shell".to_string()));
        assert_eq!(function_call_id, Some("call_early".to_string()));
        assert_eq!(
            function_call_arguments,
            Some(r#"{"command":["pwd"]}"#.to_string())
        );
    }

    #[test]
    fn validate_assistant_output_against_schema_rejects_non_json_text() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let result = validate_assistant_output_against_schema(&schema, "plain-text");
        let error = result.expect_err("should fail when output is not valid JSON");
        assert!(error.contains("not valid JSON"));
    }

    #[test]
    fn validate_assistant_output_against_schema_rejects_shape_mismatch() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let result = validate_assistant_output_against_schema(&schema, r#"{"wrong":"value"}"#);
        let error = result.expect_err("should fail when schema does not match");
        assert!(error.contains("does not match schema"));
    }

    #[test]
    fn extract_schema_matching_json_recovers_json_from_mixed_text() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let recovered = extract_schema_matching_json(
            &schema,
            "<think>analysis</think>\n\n{\"answer\":\"HELLO\"}",
        );
        assert_eq!(recovered, Some(r#"{"answer":"HELLO"}"#.to_string()));
    }

    #[test]
    fn extract_schema_matching_json_returns_none_when_no_match() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let recovered = extract_schema_matching_json(&schema, "no json here");
        assert_eq!(recovered, None);
    }

    #[test]
    fn extract_schema_matching_json_prefers_schema_valid_candidate() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let recovered = extract_schema_matching_json(
            &schema,
            r#"prefix {"wrong":"value"} middle {"answer":"ok"}"#,
        );
        assert_eq!(recovered, Some(r#"{"answer":"ok"}"#.to_string()));
    }

    #[test]
    fn extract_schema_matching_json_scans_limited_window() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let assistant_output = format!(
            "{}{{\"answer\":\"ok\"}}",
            "x".repeat(ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES + 100)
        );
        let recovered = extract_schema_matching_json(&schema, &assistant_output);
        assert_eq!(recovered, Some(r#"{"answer":"ok"}"#.to_string()));
    }

    #[test]
    fn extract_schema_matching_json_prefers_earlier_match_across_windows() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        });

        let assistant_output = format!(
            "{{\"answer\":\"first\"}}{}{{\"answer\":\"second\"}}",
            "x".repeat(ANTHROPIC_SCHEMA_REPAIR_SCAN_MAX_BYTES + 100)
        );
        let recovered = extract_schema_matching_json(&schema, &assistant_output);
        assert_eq!(recovered, Some(r#"{"answer":"first"}"#.to_string()));
    }

    #[test]
    fn replace_last_assistant_message_updates_last_assistant_only() {
        let mut events = vec![
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "first".to_string(),
                }],
                end_turn: Some(true),
                phase: None,
            }),
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                arguments: "{}".to_string(),
                call_id: "call_1".to_string(),
            }),
            ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: Some("msg_2".to_string()),
                role: "assistant".to_string(),
                content: Vec::new(),
                end_turn: None,
                phase: None,
            }),
            ResponseEvent::OutputTextDelta("<think>analysis</think>".to_string()),
            ResponseEvent::OutputTextDelta(r#"{"answer":"second"}"#.to_string()),
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "second".to_string(),
                }],
                end_turn: Some(true),
                phase: None,
            }),
        ];

        replace_last_assistant_message(&mut events, r#"{"answer":"normalized"}"#);

        match &events[0] {
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) => {
                assert_eq!(
                    content,
                    &vec![ContentItem::OutputText {
                        text: "first".to_string()
                    }]
                );
            }
            other => panic!("unexpected first event: {other:?}"),
        }
        match &events[3] {
            ResponseEvent::OutputTextDelta(delta) => {
                assert_eq!(delta, r#"{"answer":"normalized"}"#);
            }
            other => panic!("unexpected normalized delta event: {other:?}"),
        }
        match &events[4] {
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) => {
                assert_eq!(
                    content,
                    &vec![ContentItem::OutputText {
                        text: r#"{"answer":"normalized"}"#.to_string()
                    }]
                );
            }
            other => panic!("unexpected last event: {other:?}"),
        }
    }

    #[test]
    fn tool_input_value_preserves_start_input_when_partial_json_invalid() {
        let tool = ToolUseState {
            id: None,
            name: None,
            input: Some(json!({"command":["pwd"]})),
            partial_json: r#"{"command":"#.to_string(),
        };

        let value = tool_input_value(&tool);
        assert_eq!(
            value,
            json!({
                "command": ["pwd"],
                "raw_partial_json": r#"{"command":"#
            })
        );
    }

    #[test]
    fn tool_input_value_merges_start_input_with_partial_json_object() {
        let tool = ToolUseState {
            id: None,
            name: None,
            input: Some(json!({"command":["pwd"],"cwd":"/tmp"})),
            partial_json: r#"{"command":["ls"]}"#.to_string(),
        };

        let value = tool_input_value(&tool);
        assert_eq!(
            value,
            json!({
                "command": ["ls"],
                "cwd": "/tmp"
            })
        );
    }

    #[test]
    fn tool_use_to_response_item_uses_fallback_name_when_missing() {
        let tool = ToolUseState {
            id: Some("call_missing_name".to_string()),
            name: None,
            input: Some(json!({"command":["pwd"]})),
            partial_json: String::new(),
        };

        let item =
            tool_use_to_response_item(3, &tool, &HashSet::new()).expect("tool call should exist");
        assert_eq!(
            item,
            ResponseItem::FunctionCall {
                id: None,
                name: "anthropic_tool_missing_name_3".to_string(),
                arguments: r#"{"command":["pwd"]}"#.to_string(),
                call_id: "call_missing_name".to_string()
            }
        );
    }

    #[test]
    fn tool_use_to_response_item_emits_custom_tool_call_for_freeform_tool() {
        let tool = ToolUseState {
            id: Some("call_patch".to_string()),
            name: Some("apply_patch".to_string()),
            input: Some(json!({"input":"*** Begin Patch\n*** End Patch"})),
            partial_json: String::new(),
        };

        let freeform_tool_names = HashSet::from_iter(["apply_patch".to_string()]);
        let item = tool_use_to_response_item(0, &tool, &freeform_tool_names)
            .expect("custom tool call should exist");
        assert_eq!(
            item,
            ResponseItem::CustomToolCall {
                id: None,
                status: None,
                call_id: "call_patch".to_string(),
                name: "apply_patch".to_string(),
                input: "*** Begin Patch\n*** End Patch".to_string(),
            }
        );
    }

    #[test]
    fn anthropic_tool_result_text_uses_text_projection_for_content_items() {
        let output = FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::InputText {
                text: "line 1".to_string(),
            },
            FunctionCallOutputContentItem::InputImage {
                image_url: "data:image/png;base64,AAA".to_string(),
            },
            FunctionCallOutputContentItem::InputText {
                text: "line 2".to_string(),
            },
        ]);
        assert_eq!(anthropic_tool_result_text(&output), "line 1\nline 2");
    }
}
