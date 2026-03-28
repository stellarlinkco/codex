use serde::Deserialize;
use serde::Serialize;
use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub const CODEX_TELEGRAM_BOT_TOKEN_ENV: &str = "CODEX_TELEGRAM_BOT_TOKEN";
pub const CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV: &str = "CODEX_TELEGRAM_ALLOWED_CHAT_IDS";
pub const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";
pub const TELEGRAM_MAX_TEXT_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBotConfig {
    pub bot_token: String,
    pub allowed_chat_ids: Vec<i64>,
}

impl TelegramBotConfig {
    pub fn from_env() -> Result<Option<Self>, TelegramBotConfigError> {
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    pub fn from_env_with<F>(mut lookup: F) -> Result<Option<Self>, TelegramBotConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let Some(bot_token) = lookup(CODEX_TELEGRAM_BOT_TOKEN_ENV)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        let raw_chat_ids = lookup(CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV)
            .ok_or(TelegramBotConfigError::MissingAllowedChatIds)?;
        let allowed_chat_ids = parse_allowed_chat_ids(&raw_chat_ids)?;

        Ok(Some(Self {
            bot_token,
            allowed_chat_ids,
        }))
    }

    pub fn is_chat_allowed(&self, chat_id: i64) -> bool {
        self.allowed_chat_ids.binary_search(&chat_id).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramBotConfigError {
    MissingAllowedChatIds,
    EmptyAllowedChatIds,
    InvalidAllowedChatId(String),
}

impl fmt::Display for TelegramBotConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAllowedChatIds => write!(
                f,
                "{CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV} is required when Telegram bot support is enabled"
            ),
            Self::EmptyAllowedChatIds => write!(
                f,
                "{CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV} must contain at least one chat id"
            ),
            Self::InvalidAllowedChatId(value) => write!(
                f,
                "invalid Telegram chat id `{value}` in {CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV}"
            ),
        }
    }
}

impl StdError for TelegramBotConfigError {}

fn parse_allowed_chat_ids(raw: &str) -> Result<Vec<i64>, TelegramBotConfigError> {
    let mut ids = raw
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<i64>()
                .map_err(|_| TelegramBotConfigError::InvalidAllowedChatId(part.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    ids.sort_unstable();
    ids.dedup();

    if ids.is_empty() {
        return Err(TelegramBotConfigError::EmptyAllowedChatIds);
    }

    Ok(ids)
}

#[derive(Debug, Clone)]
pub struct TelegramBotClient {
    http: reqwest::Client,
    bot_token: String,
    api_base_url: String,
}

impl TelegramBotClient {
    pub fn with_client(
        http: reqwest::Client,
        bot_token: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> Self {
        Self {
            http,
            bot_token: bot_token.into(),
            api_base_url: api_base_url.into(),
        }
    }

    pub fn method_url(&self, method: &str) -> String {
        let base_url = self.api_base_url.trim_end_matches('/');
        format!("{base_url}/bot{}/{}", self.bot_token, method)
    }

    pub async fn get_updates(
        &self,
        request: &GetUpdatesRequest,
    ) -> Result<Vec<TelegramUpdate>, TelegramApiError> {
        self.post("getUpdates", request).await
    }

    pub async fn send_message(
        &self,
        request: &SendMessageRequest,
    ) -> Result<TelegramMessage, TelegramApiError> {
        self.post("sendMessage", request).await
    }

    pub async fn edit_message_text(
        &self,
        request: &EditMessageTextRequest,
    ) -> Result<EditMessageTextResult, TelegramApiError> {
        self.post("editMessageText", request).await
    }

    pub async fn answer_callback_query(
        &self,
        request: &AnswerCallbackQueryRequest,
    ) -> Result<bool, TelegramApiError> {
        self.post("answerCallbackQuery", request).await
    }

    async fn post<Request, Response>(
        &self,
        method: &str,
        request: &Request,
    ) -> Result<Response, TelegramApiError>
    where
        Request: Serialize + ?Sized,
        Response: for<'de> Deserialize<'de>,
    {
        let response = self
            .http
            .post(self.method_url(method))
            .json(request)
            .send()
            .await
            .map_err(TelegramApiError::Transport)?;
        let status = response.status();
        let body = response.text().await.map_err(TelegramApiError::Transport)?;
        let envelope: TelegramEnvelope<Response> =
            serde_json::from_str(&body).map_err(|source| TelegramApiError::Decode {
                method: method.to_string(),
                status,
                body: body.clone(),
                source,
            })?;

        if envelope.ok {
            return envelope
                .result
                .ok_or_else(|| TelegramApiError::MissingResult {
                    method: method.to_string(),
                    status,
                    body,
                });
        }

        Err(TelegramApiError::Api {
            method: method.to_string(),
            status,
            error_code: envelope.error_code,
            description: envelope
                .description
                .unwrap_or_else(|| "Telegram API request failed".to_string()),
            parameters: envelope.parameters,
        })
    }
}

#[derive(Debug)]
pub enum TelegramApiError {
    Transport(reqwest::Error),
    Decode {
        method: String,
        status: reqwest::StatusCode,
        body: String,
        source: serde_json::Error,
    },
    MissingResult {
        method: String,
        status: reqwest::StatusCode,
        body: String,
    },
    Api {
        method: String,
        status: reqwest::StatusCode,
        error_code: Option<u16>,
        description: String,
        parameters: Option<TelegramResponseParameters>,
    },
}

impl fmt::Display for TelegramApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "Telegram transport error: {err}"),
            Self::Decode {
                method,
                status,
                body,
                ..
            } => write!(
                f,
                "failed to decode Telegram response for {method} ({status}): {body}"
            ),
            Self::MissingResult {
                method,
                status,
                body,
            } => write!(
                f,
                "Telegram response for {method} ({status}) did not include a result: {body}"
            ),
            Self::Api {
                method,
                status,
                error_code,
                description,
                parameters,
            } => match error_code {
                Some(error_code) => write!(
                    f,
                    "Telegram API {method} failed with {status} / {error_code}: {description}{}",
                    format_retry_hint(parameters.as_ref())
                ),
                None => write!(
                    f,
                    "Telegram API {method} failed with {status}: {description}{}",
                    format_retry_hint(parameters.as_ref())
                ),
            },
        }
    }
}

fn format_retry_hint(parameters: Option<&TelegramResponseParameters>) -> String {
    let Some(parameters) = parameters else {
        return String::new();
    };
    let mut hints = Vec::new();
    if let Some(retry_after) = parameters.retry_after {
        hints.push(format!(" retry_after={retry_after}s"));
    }
    if let Some(chat_id) = parameters.migrate_to_chat_id {
        hints.push(format!(" migrate_to_chat_id={chat_id}"));
    }
    hints.concat()
}

impl StdError for TelegramApiError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Transport(err) => Some(err),
            Self::Decode { source, .. } => Some(source),
            Self::MissingResult { .. } | Self::Api { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TelegramPollingWorker {
    options: TelegramPollingOptions,
}

impl Default for TelegramPollingWorker {
    fn default() -> Self {
        Self::new(TelegramPollingOptions::default())
    }
}

impl TelegramPollingWorker {
    pub fn new(options: TelegramPollingOptions) -> Self {
        Self { options }
    }

    pub async fn run<Handler, HandlerFuture, HandlerError>(
        &self,
        client: TelegramBotClient,
        _config: TelegramBotConfig,
        shutdown: &AtomicBool,
        mut handler: Handler,
    ) -> Result<(), TelegramPollingError<HandlerError>>
    where
        Handler: FnMut(TelegramBotClient, TelegramUpdate) -> HandlerFuture,
        HandlerFuture: Future<Output = Result<TelegramPollControl, HandlerError>>,
    {
        let mut offset = None;

        while !shutdown.load(Ordering::Relaxed) {
            let request = self.options.build_request(offset);
            let updates = client
                .get_updates(&request)
                .await
                .map_err(TelegramPollingError::Api)?;

            if updates.is_empty() {
                tokio::time::sleep(self.options.idle_backoff).await;
                continue;
            }

            for update in updates {
                offset = Some(update.update_id + 1);

                let control = handler(client.clone(), update)
                    .await
                    .map_err(TelegramPollingError::Handler)?;
                if control == TelegramPollControl::Stop {
                    return Ok(());
                }

                if shutdown.load(Ordering::Relaxed) {
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramPollingOptions {
    pub limit: Option<u8>,
    pub timeout_seconds: u16,
    pub idle_backoff: Duration,
    pub allowed_updates: Vec<String>,
}

impl Default for TelegramPollingOptions {
    fn default() -> Self {
        Self {
            limit: Some(100),
            timeout_seconds: 30,
            idle_backoff: Duration::from_secs(1),
            allowed_updates: vec!["message".to_string(), "callback_query".to_string()],
        }
    }
}

impl TelegramPollingOptions {
    pub fn build_request(&self, offset: Option<i64>) -> GetUpdatesRequest {
        GetUpdatesRequest {
            offset,
            limit: self.limit,
            timeout: Some(self.timeout_seconds),
            allowed_updates: (!self.allowed_updates.is_empty())
                .then(|| self.allowed_updates.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramPollControl {
    Continue,
    Stop,
}

#[derive(Debug)]
pub enum TelegramPollingError<HandlerError> {
    Api(TelegramApiError),
    Handler(HandlerError),
}

impl<HandlerError> fmt::Display for TelegramPollingError<HandlerError>
where
    HandlerError: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(err) => write!(f, "{err}"),
            Self::Handler(err) => write!(f, "Telegram update handler failed: {err}"),
        }
    }
}

impl<HandlerError> StdError for TelegramPollingError<HandlerError>
where
    HandlerError: StdError + 'static,
{
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Api(err) => Some(err),
            Self::Handler(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelegramEnvelope<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    pub error_code: Option<u16>,
    pub parameters: Option<TelegramResponseParameters>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramResponseParameters {
    pub migrate_to_chat_id: Option<i64>,
    pub retry_after: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetUpdatesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_updates: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendMessageRequest {
    pub chat_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditMessageTextRequest {
    pub chat_id: i64,
    pub message_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnswerCallbackQueryRequest {
    pub callback_query_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_alert: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_time: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
    pub callback_query: Option<TelegramCallbackQuery>,
}

impl TelegramUpdate {
    pub fn chat_id(&self) -> Option<i64> {
        self.message
            .as_ref()
            .map(TelegramMessage::chat_id)
            .or_else(|| self.edited_message.as_ref().map(TelegramMessage::chat_id))
            .or_else(|| {
                self.callback_query
                    .as_ref()
                    .and_then(TelegramCallbackQuery::chat_id)
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
    pub language_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub date: i64,
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

impl TelegramMessage {
    pub fn chat_id(&self) -> i64 {
        self.chat.id
    }

    pub fn reference(&self) -> TelegramMessageRef {
        TelegramMessageRef {
            chat_id: self.chat.id,
            message_id: self.message_id,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramMessageRef {
    pub chat_id: i64,
    pub message_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramCallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    pub message: Option<TelegramMessage>,
    pub inline_message_id: Option<String>,
    pub chat_instance: Option<String>,
    pub data: Option<String>,
}

impl TelegramCallbackQuery {
    pub fn chat_id(&self) -> Option<i64> {
        self.message.as_ref().map(TelegramMessage::chat_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum EditMessageTextResult {
    Message(TelegramMessage),
    Acknowledged(bool),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

impl InlineKeyboardMarkup {
    pub fn single_row(buttons: Vec<InlineKeyboardButton>) -> Self {
        Self {
            inline_keyboard: vec![buttons],
        }
    }

    pub fn rows(rows: Vec<Vec<InlineKeyboardButton>>) -> Self {
        Self {
            inline_keyboard: rows,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InlineKeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl InlineKeyboardButton {
    pub fn callback(text: impl Into<String>, payload: &TelegramCallbackPayload) -> Self {
        Self {
            text: text.into(),
            callback_data: Some(payload.encode()),
            url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramCallbackPayload {
    #[serde(rename = "a")]
    pub action: String,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(rename = "v", skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl TelegramCallbackPayload {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("callback payload serialization should not fail")
    }

    pub fn decode(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingMessageState {
    InProgress,
    Complete,
}

impl StreamingMessageState {
    fn label(self) -> &'static str {
        match self {
            Self::InProgress => "Status: running",
            Self::Complete => "Status: complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTelegramText {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditableStreamingMessage {
    pub title: Option<String>,
    pub body: String,
    pub footer: Option<String>,
}

impl EditableStreamingMessage {
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            title: None,
            body: body.into(),
            footer: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn render(&self, state: StreamingMessageState) -> RenderedTelegramText {
        render_streaming_message(
            self.title.as_deref(),
            &self.body,
            self.footer.as_deref(),
            state,
        )
    }
}

pub fn render_streaming_message(
    title: Option<&str>,
    body: &str,
    footer: Option<&str>,
    state: StreamingMessageState,
) -> RenderedTelegramText {
    let mut sections = Vec::new();

    if let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) {
        sections.push(title.to_string());
    }

    let body = body.trim_end();
    if !body.is_empty() {
        sections.push(body.to_string());
    }

    if let Some(footer) = footer.map(str::trim).filter(|value| !value.is_empty()) {
        sections.push(footer.to_string());
    }

    sections.push(state.label().to_string());

    truncate_telegram_text(&sections.join("\n\n"))
}

pub fn truncate_telegram_text(text: &str) -> RenderedTelegramText {
    let text_len = text.chars().count();
    if text_len <= TELEGRAM_MAX_TEXT_CHARS {
        return RenderedTelegramText {
            text: text.to_string(),
            truncated: false,
        };
    }

    let suffix = "\n\n[truncated]";
    let keep = TELEGRAM_MAX_TEXT_CHARS.saturating_sub(suffix.chars().count());
    let mut truncated = text.chars().take(keep).collect::<String>();
    truncated.push_str(suffix);

    RenderedTelegramText {
        text: truncated,
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parses_config_from_env_map() {
        let vars = HashMap::from([
            (
                CODEX_TELEGRAM_BOT_TOKEN_ENV.to_string(),
                "bot-token".to_string(),
            ),
            (
                CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV.to_string(),
                "42, -100123 42".to_string(),
            ),
        ]);

        let config = TelegramBotConfig::from_env_with(|key| vars.get(key).cloned())
            .expect("config should parse")
            .expect("config should be enabled");

        assert_eq!(
            config,
            TelegramBotConfig {
                bot_token: "bot-token".to_string(),
                allowed_chat_ids: vec![-100123, 42],
            }
        );
    }

    #[test]
    fn missing_token_disables_config() {
        let vars = HashMap::from([(
            CODEX_TELEGRAM_ALLOWED_CHAT_IDS_ENV.to_string(),
            "42".to_string(),
        )]);

        let config = TelegramBotConfig::from_env_with(|key| vars.get(key).cloned())
            .expect("missing token should not error");

        assert_eq!(config, None);
    }

    #[test]
    fn callback_payload_roundtrip() {
        let payload = TelegramCallbackPayload {
            action: "approve".to_string(),
            scope: Some("session-1:req-2".to_string()),
            value: Some("allow".to_string()),
        };

        let encoded = payload.encode();
        let decoded = TelegramCallbackPayload::decode(&encoded).expect("payload should decode");

        assert_eq!(decoded, payload);
    }

    #[test]
    fn render_streaming_message_truncates_long_text() {
        let body = "x".repeat(TELEGRAM_MAX_TEXT_CHARS + 32);
        let rendered = render_streaming_message(None, &body, None, StreamingMessageState::Complete);

        assert!(rendered.truncated);
        assert_eq!(rendered.text.chars().count(), TELEGRAM_MAX_TEXT_CHARS);
    }
}
