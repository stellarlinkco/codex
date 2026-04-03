use super::*;
use crate::telegram_bot::AnswerCallbackQueryRequest;
use crate::telegram_bot::EditMessageTextRequest;
use crate::telegram_bot::EditableStreamingMessage;
use crate::telegram_bot::InlineKeyboardButton;
use crate::telegram_bot::InlineKeyboardMarkup;
use crate::telegram_bot::SendMessageRequest;
use crate::telegram_bot::StreamingMessageState;
use crate::telegram_bot::TelegramBotClient;
use crate::telegram_bot::TelegramBotConfig;
use crate::telegram_bot::TelegramCallbackPayload;
use crate::telegram_bot::TelegramMessage;
use crate::telegram_bot::TelegramMessageRef;
use crate::telegram_bot::TelegramPollControl;
use crate::telegram_bot::TelegramPollingOptions;
use crate::telegram_bot::TelegramPollingWorker;
use anyhow::Context;
use axum::Json;
use axum::body::to_bytes;
use axum::extract::Path;
use axum::extract::State;
use serde_json::Map as JsonMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::time::Instant;

const TELEGRAM_API_BASE_URL_ENV: &str = "CODEX_TELEGRAM_API_BASE_URL";
const TELEGRAM_POLL_TIMEOUT_SECS_ENV: &str = "CODEX_TELEGRAM_POLL_TIMEOUT_SECS";
const TELEGRAM_EDIT_THROTTLE_MS_ENV: &str = "CODEX_TELEGRAM_EDIT_THROTTLE_MS";
const TELEGRAM_WATCH_TRANSCRIPT_LIMIT: usize = 8;
const TELEGRAM_IDLE_POLL_RETRY: Duration = Duration::from_secs(1);
const TELEGRAM_FLUSH_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn spawn(state: AppState) {
    let config = match TelegramWorkerConfig::from_env() {
        Ok(Some(config)) => config,
        Ok(None) => return,
        Err(err) => {
            warn!(error = %err, "telegram bot disabled due to invalid configuration");
            return;
        }
    };

    let client = TelegramBotClient::with_client(
        codex_core::default_client::build_reqwest_client(),
        config.bot.bot_token.clone(),
        config.api_base_url.clone(),
    );
    let worker = Arc::new(TelegramWorker::new(state, client, config));

    let polling_worker = Arc::clone(&worker);
    tokio::spawn(async move {
        polling_worker.poll_updates().await;
    });

    let event_worker = Arc::clone(&worker);
    tokio::spawn(async move {
        event_worker.forward_events().await;
    });

    let flush_worker = Arc::clone(&worker);
    tokio::spawn(async move {
        flush_worker.flush_stream_updates().await;
    });
}

#[derive(Clone)]
struct TelegramWorkerConfig {
    bot: TelegramBotConfig,
    api_base_url: String,
    poll_timeout_seconds: u16,
    edit_throttle: Duration,
}

impl TelegramWorkerConfig {
    fn from_env() -> Result<Option<Self>, String> {
        let Some(bot) = TelegramBotConfig::from_env().map_err(|err| err.to_string())? else {
            return Ok(None);
        };

        let api_base_url = std::env::var(TELEGRAM_API_BASE_URL_ENV)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| crate::telegram_bot::TELEGRAM_API_BASE_URL.to_string());
        let poll_timeout_seconds = parse_env_u16(TELEGRAM_POLL_TIMEOUT_SECS_ENV, 30)?;
        let edit_throttle_ms = parse_env_u64(TELEGRAM_EDIT_THROTTLE_MS_ENV, 1200)?;

        Ok(Some(Self {
            bot,
            api_base_url,
            poll_timeout_seconds,
            edit_throttle: Duration::from_millis(edit_throttle_ms),
        }))
    }
}

fn parse_env_u16(key: &str, default: u16) -> Result<u16, String> {
    match std::env::var(key) {
        Ok(value) => value
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("invalid integer for {key}")),
        Err(_) => Ok(default),
    }
}

fn parse_env_u64(key: &str, default: u64) -> Result<u64, String> {
    match std::env::var(key) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("invalid integer for {key}")),
        Err(_) => Ok(default),
    }
}

struct TelegramWorker {
    state: AppState,
    client: TelegramBotClient,
    config: TelegramWorkerConfig,
    chats: Mutex<HashMap<i64, ChatState>>,
    rejected_chats: Mutex<HashSet<i64>>,
}

impl TelegramWorker {
    fn new(state: AppState, client: TelegramBotClient, config: TelegramWorkerConfig) -> Self {
        Self {
            state,
            client,
            config,
            chats: Mutex::new(HashMap::new()),
            rejected_chats: Mutex::new(HashSet::new()),
        }
    }

    async fn poll_updates(self: Arc<Self>) {
        let shutdown = AtomicBool::new(false);
        let polling = TelegramPollingWorker::new(TelegramPollingOptions {
            timeout_seconds: self.config.poll_timeout_seconds,
            idle_backoff: TELEGRAM_IDLE_POLL_RETRY,
            ..Default::default()
        });

        loop {
            let worker = Arc::clone(&self);
            let result: Result<(), crate::telegram_bot::TelegramPollingError<anyhow::Error>> =
                polling
                    .run(
                        self.client.clone(),
                        self.config.bot.clone(),
                        &shutdown,
                        move |_client, update| {
                            let worker = Arc::clone(&worker);
                            async move {
                                worker.handle_update(update).await?;
                                Ok::<TelegramPollControl, anyhow::Error>(
                                    TelegramPollControl::Continue,
                                )
                            }
                        },
                    )
                    .await;
            if let Err(err) = result {
                warn!(error = %err, "telegram polling loop failed");
                tokio::time::sleep(TELEGRAM_IDLE_POLL_RETRY).await;
                continue;
            }
            break;
        }
    }

    async fn handle_update(
        &self,
        update: crate::telegram_bot::TelegramUpdate,
    ) -> anyhow::Result<()> {
        if let Some(chat_id) = update.chat_id()
            && !self.config.bot.is_chat_allowed(chat_id)
        {
            self.reject_chat(chat_id).await?;
            return Ok(());
        }
        if let Some(callback) = update.callback_query {
            self.handle_callback(callback).await?;
            return Ok(());
        }
        let Some(message) = update.message else {
            return Ok(());
        };
        self.handle_message(message).await
    }

    async fn reject_chat(&self, chat_id: i64) -> anyhow::Result<()> {
        let mut rejected = self.rejected_chats.lock().await;
        if !rejected.insert(chat_id) {
            return Ok(());
        }
        drop(rejected);
        self.send_text(chat_id, "This bot is not enabled for this chat.", None)
            .await?;
        Ok(())
    }

    async fn handle_message(&self, message: TelegramMessage) -> anyhow::Result<()> {
        let Some(text) = message.text.as_deref().map(str::trim) else {
            return Ok(());
        };
        let chat_id = message.chat_id();

        if text.starts_with("/start") || text.starts_with("/projects") {
            self.show_projects(chat_id, None).await?;
            return Ok(());
        }
        if text.starts_with("/refresh") {
            if let Some(session_id) = self.watched_session_id(chat_id).await {
                self.show_session(chat_id, &session_id, None).await?;
            } else {
                self.send_text(chat_id, "No watched session. Use /projects first.", None)
                    .await?;
            }
            return Ok(());
        }
        if text.starts_with("/continue") {
            if let Some(session_id) = self.watched_session_id(chat_id).await {
                self.resume_session(&session_id).await?;
                self.show_session(chat_id, &session_id, None).await?;
            } else {
                self.send_text(chat_id, "No watched session. Use /projects first.", None)
                    .await?;
            }
            return Ok(());
        }
        if text.starts_with("/stop") {
            if let Some(session_id) = self.watched_session_id(chat_id).await {
                self.abort_session(&session_id).await?;
                self.show_session(chat_id, &session_id, None).await?;
            } else {
                self.send_text(chat_id, "No watched session. Use /projects first.", None)
                    .await?;
            }
            return Ok(());
        }

        if self.has_pending_answer(chat_id).await {
            self.submit_pending_answer(chat_id, text).await?;
            return Ok(());
        }

        if let Some(session_id) = self.watched_session_id(chat_id).await {
            let local_id = format!("telegram:{chat_id}:{}", message.message_id);
            self.post_message(&session_id, text.to_string(), Some(local_id))
                .await?;
            return Ok(());
        }

        self.send_text(
            chat_id,
            "Open a session with /projects before sending a message.",
            None,
        )
        .await?;
        Ok(())
    }

    async fn handle_callback(
        &self,
        callback: crate::telegram_bot::TelegramCallbackQuery,
    ) -> anyhow::Result<()> {
        let Some(chat_id) = callback.chat_id() else {
            return Ok(());
        };
        let Some(data) = callback.data.as_deref() else {
            return Ok(());
        };
        let payload = TelegramCallbackPayload::decode(data)
            .with_context(|| format!("decode telegram callback payload `{data}`"))?;
        let reference = callback.message.as_ref().map(TelegramMessage::reference);

        match payload.action.as_str() {
            "projects" => {
                self.show_projects(chat_id, reference).await?;
            }
            "project" => {
                let Some(index) = payload
                    .value
                    .as_deref()
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    self.answer_callback(&callback.id, Some("Project menu expired"))
                        .await?;
                    return Ok(());
                };
                let Some(project_key) = self.project_key(chat_id, index).await else {
                    self.answer_callback(&callback.id, Some("Project menu expired"))
                        .await?;
                    return Ok(());
                };
                self.show_project_sessions(chat_id, &project_key, reference)
                    .await?;
            }
            "session" => {
                let Some(session_id) = payload.scope.as_deref() else {
                    self.answer_callback(&callback.id, Some("Missing session id"))
                        .await?;
                    return Ok(());
                };
                self.show_session(chat_id, session_id, reference).await?;
            }
            "refresh" => {
                let Some(session_id) = payload.scope.as_deref() else {
                    self.answer_callback(&callback.id, Some("Missing session id"))
                        .await?;
                    return Ok(());
                };
                self.show_session(chat_id, session_id, reference).await?;
            }
            "continue" => {
                let Some(session_id) = payload.scope.as_deref() else {
                    self.answer_callback(&callback.id, Some("Missing session id"))
                        .await?;
                    return Ok(());
                };
                self.resume_session(session_id).await?;
                self.show_session(chat_id, session_id, reference).await?;
            }
            "stop" => {
                let Some(session_id) = payload.scope.as_deref() else {
                    self.answer_callback(&callback.id, Some("Missing session id"))
                        .await?;
                    return Ok(());
                };
                self.abort_session(session_id).await?;
                self.show_session(chat_id, session_id, reference).await?;
            }
            "approve" | "approve_session" | "deny" | "answer" => {
                let Some(req_id) = payload.scope.as_deref() else {
                    self.answer_callback(&callback.id, Some("Missing request id"))
                        .await?;
                    return Ok(());
                };
                let Some(session_id) = self.watched_session_id(chat_id).await else {
                    self.answer_callback(&callback.id, Some("No watched session"))
                        .await?;
                    return Ok(());
                };
                match payload.action.as_str() {
                    "approve" => {
                        self.approve_request(&session_id, req_id, false).await?;
                        self.show_session(chat_id, &session_id, reference).await?;
                    }
                    "approve_session" => {
                        self.approve_request(&session_id, req_id, true).await?;
                        self.show_session(chat_id, &session_id, reference).await?;
                    }
                    "deny" => {
                        self.deny_request(&session_id, req_id).await?;
                        self.show_session(chat_id, &session_id, reference).await?;
                    }
                    "answer" => {
                        self.start_answer_request(chat_id, &session_id, req_id)
                            .await?;
                    }
                    _ => {}
                }
            }
            _ => {
                self.answer_callback(&callback.id, Some("Unsupported action"))
                    .await?;
                return Ok(());
            }
        }

        self.answer_callback(&callback.id, None).await?;
        Ok(())
    }

    async fn show_projects(
        &self,
        chat_id: i64,
        existing_message: Option<TelegramMessageRef>,
    ) -> anyhow::Result<()> {
        let projects = collect_project_summaries(&self.state).await;
        let text = render_projects_text(&projects);
        let buttons = projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                vec![InlineKeyboardButton::callback(
                    project_button_label(project),
                    &TelegramCallbackPayload {
                        action: "project".to_string(),
                        scope: None,
                        value: Some(index.to_string()),
                    },
                )]
            })
            .collect::<Vec<_>>();
        {
            let mut chats = self.chats.lock().await;
            let chat = chats.entry(chat_id).or_default();
            chat.project_keys = projects
                .iter()
                .map(|project| project.project_key.clone())
                .collect();
        }
        self.present_text(
            chat_id,
            existing_message,
            text,
            Some(InlineKeyboardMarkup::rows(buttons)),
        )
        .await?;
        Ok(())
    }

    async fn show_project_sessions(
        &self,
        chat_id: i64,
        project_key: &str,
        existing_message: Option<TelegramMessageRef>,
    ) -> anyhow::Result<()> {
        let sessions = collect_session_summaries(&self.state)
            .await
            .into_iter()
            .filter(|session| session.project_key == project_key)
            .collect::<Vec<_>>();
        let text = render_project_sessions_text(project_key, &sessions);
        let mut rows = sessions
            .iter()
            .map(|session| {
                vec![InlineKeyboardButton::callback(
                    session_button_label(session),
                    &TelegramCallbackPayload {
                        action: "session".to_string(),
                        scope: Some(session.id.clone()),
                        value: None,
                    },
                )]
            })
            .collect::<Vec<_>>();
        rows.push(vec![InlineKeyboardButton::callback(
            "Back to projects",
            &TelegramCallbackPayload {
                action: "projects".to_string(),
                scope: None,
                value: None,
            },
        )]);
        self.present_text(
            chat_id,
            existing_message,
            text,
            Some(InlineKeyboardMarkup::rows(rows)),
        )
        .await?;
        Ok(())
    }

    async fn show_session(
        &self,
        chat_id: i64,
        session_id: &str,
        existing_message: Option<TelegramMessageRef>,
    ) -> anyhow::Result<()> {
        let Some(session) = load_session_detail(&self.state, session_id).await? else {
            self.send_text(chat_id, "Session not found.", None).await?;
            return Ok(());
        };
        let messages =
            load_recent_messages(&self.state, session_id, TELEGRAM_WATCH_TRANSCRIPT_LIMIT).await;
        let pending_requests = pending_request_entries(&session);
        let text = render_session_text(&session, &messages, &pending_requests);
        let markup = session_action_markup(&session, &pending_requests);
        {
            let mut chats = self.chats.lock().await;
            let chat = chats.entry(chat_id).or_default();
            let title = session_title(&session);
            let watched_changed = chat.watched_session_id.as_deref() != Some(session_id);
            chat.watched_session_id = Some(session_id.to_string());
            chat.watched_session_title = Some(title);
            chat.pending_answer = None;
            chat.notified_requests = pending_requests
                .iter()
                .map(|(req_id, _)| req_id.clone())
                .collect::<HashSet<_>>();
            if watched_changed {
                chat.streaming = None;
            }
        }
        self.present_text(chat_id, existing_message, text, Some(markup))
            .await?;
        Ok(())
    }

    async fn start_answer_request(
        &self,
        chat_id: i64,
        session_id: &str,
        req_id: &str,
    ) -> anyhow::Result<()> {
        let Some(session) = load_session_detail(&self.state, session_id).await? else {
            self.send_text(chat_id, "Session not found.", None).await?;
            return Ok(());
        };
        let pending_requests = pending_request_entries(&session);
        let Some((_, request)) = pending_requests.into_iter().find(|(id, _)| id == req_id) else {
            self.send_text(chat_id, "Request is no longer pending.", None)
                .await?;
            return Ok(());
        };
        let Some(questions) = request_questions(&request) else {
            self.send_text(chat_id, "That request does not expect an answer.", None)
                .await?;
            return Ok(());
        };
        {
            let mut chats = self.chats.lock().await;
            let chat = chats.entry(chat_id).or_default();
            chat.pending_answer = Some(PendingAnswerState {
                session_id: session_id.to_string(),
                request_id: req_id.to_string(),
                questions: questions.clone(),
            });
        }
        self.send_text(chat_id, render_answer_prompt(&questions), None)
            .await?;
        Ok(())
    }

    async fn submit_pending_answer(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        let pending = {
            let chats = self.chats.lock().await;
            chats
                .get(&chat_id)
                .and_then(|chat| chat.pending_answer.clone())
        };
        let Some(pending) = pending else {
            return Ok(());
        };
        let answers = parse_user_input_answers(text, &pending.questions)?;
        self.answer_request(&pending.session_id, &pending.request_id, answers)
            .await?;
        {
            let mut chats = self.chats.lock().await;
            if let Some(chat) = chats.get_mut(&chat_id) {
                chat.pending_answer = None;
            }
        }
        self.send_text(chat_id, "Answer submitted.", None).await?;
        self.show_session(chat_id, &pending.session_id, None)
            .await?;
        Ok(())
    }

    async fn answer_request(
        &self,
        session_id: &str,
        req_id: &str,
        answers: JsonValue,
    ) -> anyhow::Result<()> {
        let response = handle_approve_permission(
            State(self.state.clone()),
            Path((session_id.to_string(), req_id.to_string())),
            Json(ApprovePermissionRequest {
                mode: None,
                allow_tools: None,
                decision: Some("approved".to_string()),
                answers: Some(answers),
            }),
        )
        .await;
        ensure_ok_response(response).await
    }

    async fn approve_request(
        &self,
        session_id: &str,
        req_id: &str,
        session_scope: bool,
    ) -> anyhow::Result<()> {
        let decision = if session_scope {
            Some("approved_for_session".to_string())
        } else {
            Some("approved".to_string())
        };
        let response = handle_approve_permission(
            State(self.state.clone()),
            Path((session_id.to_string(), req_id.to_string())),
            Json(ApprovePermissionRequest {
                mode: None,
                allow_tools: None,
                decision,
                answers: None,
            }),
        )
        .await;
        ensure_ok_response(response).await
    }

    async fn deny_request(&self, session_id: &str, req_id: &str) -> anyhow::Result<()> {
        let response = handle_deny_permission(
            State(self.state.clone()),
            Path((session_id.to_string(), req_id.to_string())),
            Json(DenyPermissionRequest {
                decision: Some("denied".to_string()),
            }),
        )
        .await;
        ensure_ok_response(response).await
    }

    async fn post_message(
        &self,
        session_id: &str,
        text: String,
        local_id: Option<String>,
    ) -> anyhow::Result<()> {
        let response = handle_post_message(
            State(self.state.clone()),
            Path(session_id.to_string()),
            Json(MessagePostRequest {
                text,
                local_id,
                attachments: None,
            }),
        )
        .await;
        ensure_ok_response(response).await
    }

    async fn resume_session(&self, session_id: &str) -> anyhow::Result<()> {
        let response =
            handle_resume_session(State(self.state.clone()), Path(session_id.to_string())).await;
        ensure_ok_response(response).await
    }

    async fn abort_session(&self, session_id: &str) -> anyhow::Result<()> {
        let response =
            handle_abort_session(State(self.state.clone()), Path(session_id.to_string())).await;
        ensure_ok_response(response).await
    }

    async fn send_text(
        &self,
        chat_id: i64,
        text: impl AsRef<str>,
        reply_markup: Option<InlineKeyboardMarkup>,
    ) -> anyhow::Result<TelegramMessageRef> {
        let rendered = crate::telegram_bot::truncate_telegram_text(text.as_ref());
        let message = self
            .client
            .send_message(&SendMessageRequest {
                chat_id,
                text: rendered.text,
                parse_mode: None,
                disable_notification: None,
                reply_markup,
            })
            .await
            .context("send Telegram message")?;
        Ok(message.reference())
    }

    async fn edit_text(
        &self,
        reference: TelegramMessageRef,
        text: impl AsRef<str>,
        reply_markup: Option<InlineKeyboardMarkup>,
    ) -> anyhow::Result<()> {
        let rendered = crate::telegram_bot::truncate_telegram_text(text.as_ref());
        self.client
            .edit_message_text(&EditMessageTextRequest {
                chat_id: reference.chat_id,
                message_id: reference.message_id,
                text: rendered.text,
                parse_mode: None,
                reply_markup,
            })
            .await
            .context("edit Telegram message")?;
        Ok(())
    }

    async fn present_text(
        &self,
        chat_id: i64,
        existing_message: Option<TelegramMessageRef>,
        text: String,
        reply_markup: Option<InlineKeyboardMarkup>,
    ) -> anyhow::Result<()> {
        if let Some(reference) = existing_message {
            self.edit_text(reference, text, reply_markup).await
        } else {
            self.send_text(chat_id, text, reply_markup)
                .await
                .map(|_| ())
        }
    }

    async fn answer_callback(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> anyhow::Result<()> {
        self.client
            .answer_callback_query(&AnswerCallbackQueryRequest {
                callback_query_id: callback_query_id.to_string(),
                text: text.map(ToOwned::to_owned),
                show_alert: None,
                cache_time: Some(0),
            })
            .await
            .context("answer Telegram callback")?;
        Ok(())
    }

    async fn watched_session_id(&self, chat_id: i64) -> Option<String> {
        self.chats
            .lock()
            .await
            .get(&chat_id)
            .and_then(|chat| chat.watched_session_id.clone())
    }

    async fn project_key(&self, chat_id: i64, index: usize) -> Option<String> {
        self.chats
            .lock()
            .await
            .get(&chat_id)
            .and_then(|chat| chat.project_keys.get(index).cloned())
    }

    async fn has_pending_answer(&self, chat_id: i64) -> bool {
        self.chats
            .lock()
            .await
            .get(&chat_id)
            .and_then(|chat| chat.pending_answer.as_ref())
            .is_some()
    }

    async fn forward_events(self: Arc<Self>) {
        let mut rx = self.state.events_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(err) = self.handle_sync_event(event).await {
                        warn!(error = %err, "telegram event forwarding failed");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    async fn handle_sync_event(&self, event: SyncEvent) -> anyhow::Result<()> {
        let Some(session_id) = sync_event_session_id(&event).map(ToOwned::to_owned) else {
            return Ok(());
        };
        let watched_chats = self.watched_chats_for_session(&session_id).await;
        for chat_id in watched_chats {
            match &event {
                SyncEvent::MessageDelta { event, .. } => {
                    if let Some(delta) = assistant_delta_text(event) {
                        self.append_stream_delta(chat_id, &session_id, delta)
                            .await?;
                    }
                }
                SyncEvent::MessageReceived { message, .. } => {
                    self.handle_message_received(chat_id, &session_id, message)
                        .await?;
                }
                SyncEvent::MessageFinalized { .. } => {
                    if let Some(text) = latest_agent_message_text(&self.state, &session_id).await {
                        self.finalize_stream(chat_id, &session_id, Some(text))
                            .await?;
                    }
                }
                SyncEvent::SessionUpdated { .. } => {
                    self.send_pending_request_cards(chat_id, &session_id)
                        .await?;
                }
                SyncEvent::SessionRemoved { .. } => {
                    self.clear_watched_session(chat_id, &session_id).await;
                    self.send_text(chat_id, "Watched session was removed.", None)
                        .await?;
                }
                SyncEvent::SessionLiveDetached { .. } => {
                    self.send_text(
                        chat_id,
                        "Watched live session detached. Stored transcript remains available.",
                        None,
                    )
                    .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn watched_chats_for_session(&self, session_id: &str) -> Vec<i64> {
        self.chats
            .lock()
            .await
            .iter()
            .filter_map(|(chat_id, chat)| {
                (chat.watched_session_id.as_deref() == Some(session_id)).then_some(*chat_id)
            })
            .collect()
    }

    async fn append_stream_delta(
        &self,
        chat_id: i64,
        session_id: &str,
        delta: &str,
    ) -> anyhow::Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        let mut send_new = None;
        let mut edit_existing = None;
        {
            let mut chats = self.chats.lock().await;
            let Some(chat) = chats.get_mut(&chat_id) else {
                return Ok(());
            };
            if chat.watched_session_id.as_deref() != Some(session_id) {
                return Ok(());
            }
            let title = chat
                .watched_session_title
                .clone()
                .unwrap_or_else(|| session_id.to_string());
            let stream = chat.streaming.get_or_insert_with(|| StreamingBuffer {
                message_ref: None,
                body: String::new(),
                title,
                last_sent_text: String::new(),
                last_edit_at: None,
                dirty: false,
            });
            stream.body.push_str(delta);
            let rendered = stream.render(StreamingMessageState::InProgress);
            if let Some(reference) = stream.message_ref {
                if stream
                    .last_edit_at
                    .is_some_and(|at| at.elapsed() >= self.config.edit_throttle)
                {
                    stream.last_sent_text = rendered.text.clone();
                    stream.last_edit_at = Some(Instant::now());
                    stream.dirty = false;
                    edit_existing = Some((reference, rendered.text));
                } else {
                    stream.dirty = true;
                }
            } else {
                stream.last_sent_text = rendered.text.clone();
                stream.last_edit_at = Some(Instant::now());
                stream.dirty = false;
                send_new = Some(rendered.text);
            }
        }

        if let Some(text) = send_new {
            let reference = self.send_text(chat_id, text, None).await?;
            let mut chats = self.chats.lock().await;
            if let Some(chat) = chats.get_mut(&chat_id)
                && let Some(stream) = chat.streaming.as_mut()
                && stream.message_ref.is_none()
                && chat.watched_session_id.as_deref() == Some(session_id)
            {
                stream.message_ref = Some(reference);
            }
        }
        if let Some((reference, text)) = edit_existing {
            self.edit_text(reference, text, None).await?;
        }
        Ok(())
    }

    async fn handle_message_received(
        &self,
        chat_id: i64,
        session_id: &str,
        message: &WebDecryptedMessage,
    ) -> anyhow::Result<()> {
        let Some(role) = message_role(message) else {
            return Ok(());
        };
        match role {
            "agent" => {
                if let Some(text) = message_text(message) {
                    self.finalize_stream(chat_id, session_id, Some(text))
                        .await?;
                }
            }
            "user" => {
                if message
                    .local_id
                    .as_deref()
                    .is_some_and(|local_id| local_id.starts_with("telegram:"))
                {
                    return Ok(());
                }
                if let Some(text) = message_text(message) {
                    self.send_text(chat_id, format!("User:\n\n{text}"), None)
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn finalize_stream(
        &self,
        chat_id: i64,
        session_id: &str,
        final_text: Option<String>,
    ) -> anyhow::Result<()> {
        let mut send_new = None;
        let mut edit_existing = None;
        {
            let mut chats = self.chats.lock().await;
            let Some(chat) = chats.get_mut(&chat_id) else {
                return Ok(());
            };
            if chat.watched_session_id.as_deref() != Some(session_id) {
                return Ok(());
            }
            if let Some(stream) = chat.streaming.take() {
                let final_body = final_text.unwrap_or(stream.body);
                let rendered = EditableStreamingMessage::new(final_body)
                    .with_title(stream.title)
                    .render(StreamingMessageState::Complete);
                if let Some(reference) = stream.message_ref {
                    if rendered.text != stream.last_sent_text {
                        edit_existing = Some((reference, rendered.text));
                    }
                } else {
                    send_new = Some(rendered.text);
                }
            } else if let Some(final_text) = final_text {
                send_new = Some(final_text);
            }
        }
        if let Some((reference, text)) = edit_existing {
            self.edit_text(reference, text, None).await?;
        } else if let Some(text) = send_new {
            self.send_text(chat_id, text, None).await?;
        }
        Ok(())
    }

    async fn flush_stream_updates(self: Arc<Self>) {
        let mut interval = tokio::time::interval(TELEGRAM_FLUSH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) = self.flush_stream_updates_once().await {
                warn!(error = %err, "telegram throttled edit failed");
            }
        }
    }

    async fn flush_stream_updates_once(&self) -> anyhow::Result<()> {
        let pending = {
            let mut chats = self.chats.lock().await;
            let mut edits = Vec::new();
            for chat in chats.values_mut() {
                let Some(stream) = chat.streaming.as_mut() else {
                    continue;
                };
                let Some(reference) = stream.message_ref else {
                    continue;
                };
                if !stream.dirty {
                    continue;
                }
                if stream
                    .last_edit_at.is_none_or(|at| at.elapsed() < self.config.edit_throttle)
                {
                    continue;
                }
                let rendered = stream.render(StreamingMessageState::InProgress);
                stream.last_sent_text = rendered.text.clone();
                stream.last_edit_at = Some(Instant::now());
                stream.dirty = false;
                edits.push((reference, rendered.text));
            }
            edits
        };
        for (reference, text) in pending {
            self.edit_text(reference, text, None).await?;
        }
        Ok(())
    }

    async fn send_pending_request_cards(
        &self,
        chat_id: i64,
        session_id: &str,
    ) -> anyhow::Result<()> {
        let Some(session) = load_session_detail(&self.state, session_id).await? else {
            return Ok(());
        };
        let pending = pending_request_entries(&session);
        let mut new_requests = Vec::new();
        {
            let mut chats = self.chats.lock().await;
            let Some(chat) = chats.get_mut(&chat_id) else {
                return Ok(());
            };
            let current_ids = pending
                .iter()
                .map(|(req_id, _)| req_id.clone())
                .collect::<HashSet<_>>();
            for (req_id, request) in &pending {
                if !chat.notified_requests.contains(req_id) {
                    chat.notified_requests.insert(req_id.clone());
                    new_requests.push((req_id.clone(), request.clone()));
                }
            }
            chat.notified_requests
                .retain(|req_id| current_ids.contains(req_id));
        }
        for (req_id, request) in new_requests {
            self.send_text(
                chat_id,
                render_request_card_text(&session, &req_id, &request),
                Some(request_action_markup(&req_id, &request)),
            )
            .await?;
        }
        Ok(())
    }

    async fn clear_watched_session(&self, chat_id: i64, session_id: &str) {
        let mut chats = self.chats.lock().await;
        if let Some(chat) = chats.get_mut(&chat_id)
            && chat.watched_session_id.as_deref() == Some(session_id)
        {
            chat.watched_session_id = None;
            chat.watched_session_title = None;
            chat.pending_answer = None;
            chat.streaming = None;
            chat.notified_requests.clear();
        }
    }
}

#[derive(Clone, Default)]
struct PendingAnswerState {
    session_id: String,
    request_id: String,
    questions: Vec<codex_protocol::request_user_input::RequestUserInputQuestion>,
}

#[derive(Default)]
struct ChatState {
    watched_session_id: Option<String>,
    watched_session_title: Option<String>,
    project_keys: Vec<String>,
    pending_answer: Option<PendingAnswerState>,
    streaming: Option<StreamingBuffer>,
    notified_requests: HashSet<String>,
}

struct StreamingBuffer {
    message_ref: Option<TelegramMessageRef>,
    body: String,
    title: String,
    last_sent_text: String,
    last_edit_at: Option<Instant>,
    dirty: bool,
}

impl StreamingBuffer {
    fn render(&self, state: StreamingMessageState) -> crate::telegram_bot::RenderedTelegramText {
        EditableStreamingMessage::new(self.body.clone())
            .with_title(self.title.clone())
            .render(state)
    }
}

async fn collect_project_summaries(state: &AppState) -> Vec<ProjectSummary> {
    let sessions = collect_session_summaries(state).await;
    let mut projects: HashMap<String, ProjectSummary> = HashMap::new();

    for session in sessions {
        let Some(metadata) = session.metadata else {
            continue;
        };
        let entry = projects
            .entry(session.project_key.clone())
            .or_insert_with(|| ProjectSummary {
                project_key: session.project_key.clone(),
                path: metadata.path.clone(),
                active_sessions: 0,
                total_sessions: 0,
                updated_at: session.updated_at,
            });
        entry.total_sessions = entry.total_sessions.saturating_add(1);
        if session.active {
            entry.active_sessions = entry.active_sessions.saturating_add(1);
        }
        entry.updated_at = entry.updated_at.max(session.updated_at);
    }

    let mut projects: Vec<ProjectSummary> = projects.into_values().collect();
    projects.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.project_key.cmp(&b.project_key))
    });
    projects
}

async fn load_session_detail(state: &AppState, id: &str) -> anyhow::Result<Option<Session>> {
    if let Some(session) = state.sessions.read().await.get(id).cloned() {
        return Ok(Some(build_session_json(&session).await));
    }

    if let Ok(thread_id) = ThreadId::from_string(id)
        && let Some(registration) =
            live_runtime::load_live_registration_for_thread(&state.config.codex_home, &thread_id)
                .await
    {
        return Ok(Some(
            build_live_window_session(state, id, &registration).await,
        ));
    }

    let stored = match load_stored_session_snapshot(state, id, false)
        .await
        .map_err(anyhow::Error::msg)?
    {
        Some(stored) => stored,
        None => return Ok(None),
    };

    Ok(Some(Session {
        id: id.to_string(),
        namespace: "local".to_string(),
        seq: stored.seq,
        created_at: stored.created_at,
        updated_at: stored.updated_at,
        active: false,
        active_at: stored.updated_at,
        backing: SessionBacking::Stored,
        live_state: SessionLiveState::Stopped,
        project_key: session_project_key(&stored.cwd),
        runtime_owner: None,
        window_id: None,
        controller_count: 0,
        metadata: Some(Metadata {
            path: stored.cwd.display().to_string(),
            host: "local".to_string(),
            name: stored.name,
            machine_id: Some("local".to_string()),
            tools: None,
            flavor: Some("codex".to_string()),
            summary: None,
        }),
        metadata_version: 0,
        agent_state: None,
        agent_state_version: 0,
        thinking: false,
        thinking_at: stored.updated_at,
        permission_mode: Some("default".to_string()),
        model_mode: Some("default".to_string()),
    }))
}

async fn load_recent_messages(
    state: &AppState,
    session_id: &str,
    limit: usize,
) -> Vec<WebDecryptedMessage> {
    let mut all_messages =
        if let Some(session) = state.sessions.read().await.get(session_id).cloned() {
            session.state.read().await.messages.clone()
        } else {
            load_messages_from_rollout(state, session_id)
                .await
                .unwrap_or_default()
        };
    all_messages.sort_by(|a, b| a.seq.unwrap_or(0).cmp(&b.seq.unwrap_or(0)));
    if all_messages.len() <= limit {
        return all_messages;
    }
    all_messages[all_messages.len() - limit..].to_vec()
}

async fn latest_agent_message_text(state: &AppState, session_id: &str) -> Option<String> {
    load_recent_messages(state, session_id, 12)
        .await
        .into_iter()
        .rev()
        .find(|message| message_role(message) == Some("agent"))
        .and_then(|message| message_text(&message))
}

fn pending_request_entries(session: &Session) -> Vec<(String, WebAgentRequest)> {
    let mut pending = session
        .agent_state
        .as_ref()
        .and_then(|agent_state| agent_state.requests.clone())
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    pending.sort_by(|(left, _), (right, _)| left.cmp(right));
    pending
}

fn request_questions(
    request: &WebAgentRequest,
) -> Option<Vec<codex_protocol::request_user_input::RequestUserInputQuestion>> {
    if request.tool != "request_user_input" {
        return None;
    }
    request
        .arguments
        .get("questions")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn sync_event_session_id(event: &SyncEvent) -> Option<&str> {
    match event {
        SyncEvent::SessionAdded { session_id, .. }
        | SyncEvent::SessionUpdated { session_id, .. }
        | SyncEvent::SessionRemoved { session_id }
        | SyncEvent::MessageReceived { session_id, .. }
        | SyncEvent::MessageDelta { session_id, .. }
        | SyncEvent::MessageFinalized { session_id, .. }
        | SyncEvent::SessionLiveAttached { session_id }
        | SyncEvent::SessionLiveDetached { session_id } => Some(session_id.as_str()),
        _ => None,
    }
}

fn assistant_delta_text(event: &Event) -> Option<&str> {
    match &event.msg {
        EventMsg::AgentMessageContentDelta(delta) => Some(delta.delta.as_str()),
        _ => None,
    }
}

fn message_role(message: &WebDecryptedMessage) -> Option<&str> {
    message.content.get("role").and_then(JsonValue::as_str)
}

fn message_text(message: &WebDecryptedMessage) -> Option<String> {
    match message_role(message) {
        Some("user") => message
            .content
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
        Some("agent") => message
            .content
            .get("content")
            .and_then(|content| content.get("data"))
            .and_then(|data| data.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn session_title(session: &Session) -> String {
    session
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Session {}", short_id(&session.id)))
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn project_button_label(project: &ProjectSummary) -> String {
    let name = std::path::Path::new(&project.path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&project.path);
    format!(
        "{name} [{} / {}]",
        project.active_sessions, project.total_sessions
    )
}

fn session_button_label(session: &SessionSummary) -> String {
    let name = session
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| short_id(&session.id));
    format!("{} {}", backing_label(session.backing), name)
}

fn backing_label(backing: SessionBacking) -> &'static str {
    match backing {
        SessionBacking::LiveWindow => "[live]",
        SessionBacking::Headless => "[headless]",
        SessionBacking::Stored => "[stored]",
    }
}

fn live_state_label(live_state: SessionLiveState) -> &'static str {
    match live_state {
        SessionLiveState::Idle => "idle",
        SessionLiveState::Generating => "generating",
        SessionLiveState::WaitingApproval => "waitingApproval",
        SessionLiveState::WaitingUserInput => "waitingUserInput",
        SessionLiveState::Stopped => "stopped",
        SessionLiveState::Unavailable => "unavailable",
    }
}

fn render_projects_text(projects: &[ProjectSummary]) -> String {
    if projects.is_empty() {
        return "No projects found.".to_string();
    }
    let mut lines = vec!["Projects".to_string(), String::new()];
    for project in projects {
        lines.push(format!(
            "- {}\n  path: {}\n  sessions: {} active / {} total",
            project_button_label(project),
            project.path,
            project.active_sessions,
            project.total_sessions,
        ));
    }
    lines.join("\n")
}

fn render_project_sessions_text(project_key: &str, sessions: &[SessionSummary]) -> String {
    let mut lines = vec![format!("Project\n\n{project_key}")];
    if sessions.is_empty() {
        lines.push("\nNo sessions found for this project.".to_string());
        return lines.join("");
    }
    for session in sessions {
        lines.push(format!(
            "\n{} {}\n  state: {}\n  pending: {}",
            backing_label(session.backing),
            session
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.name.clone())
                .unwrap_or_else(|| short_id(&session.id).to_string()),
            live_state_label(session.live_state),
            session.pending_requests_count,
        ));
    }
    lines.join("\n")
}

fn render_session_text(
    session: &Session,
    messages: &[WebDecryptedMessage],
    pending_requests: &[(String, WebAgentRequest)],
) -> String {
    let mut lines = vec![session_title(session), String::new()];
    lines.push(format!("id: {}", session.id));
    lines.push(format!("backing: {}", backing_label(session.backing)));
    lines.push(format!("state: {}", live_state_label(session.live_state)));
    if let Some(owner) = session.runtime_owner {
        lines.push(format!("owner: {}", owner_name(owner)));
    }
    if let Some(metadata) = session.metadata.as_ref() {
        lines.push(format!("path: {}", metadata.path));
    }
    if !pending_requests.is_empty() {
        lines.push(String::new());
        lines.push("pending requests:".to_string());
        for (req_id, request) in pending_requests {
            lines.push(format!(
                "- {} ({})",
                summarize_request(request),
                short_id(req_id)
            ));
        }
    }
    if !messages.is_empty() {
        lines.push(String::new());
        lines.push("recent transcript:".to_string());
        for message in messages {
            if let Some(text) = message_text(message) {
                let role = message_role(message).unwrap_or("message");
                lines.push(format!("- {role}: {}", trim_inline_text(&text, 320)));
            }
        }
    }
    lines.join("\n")
}

fn trim_inline_text(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_len.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn session_action_markup(
    session: &Session,
    pending_requests: &[(String, WebAgentRequest)],
) -> InlineKeyboardMarkup {
    let mut rows = vec![vec![InlineKeyboardButton::callback(
        "Refresh",
        &TelegramCallbackPayload {
            action: "refresh".to_string(),
            scope: Some(session.id.clone()),
            value: None,
        },
    )]];
    if session.backing == SessionBacking::Stored || session.live_state == SessionLiveState::Stopped
    {
        rows.push(vec![InlineKeyboardButton::callback(
            "Continue",
            &TelegramCallbackPayload {
                action: "continue".to_string(),
                scope: Some(session.id.clone()),
                value: None,
            },
        )]);
    } else {
        rows.push(vec![InlineKeyboardButton::callback(
            "Stop",
            &TelegramCallbackPayload {
                action: "stop".to_string(),
                scope: Some(session.id.clone()),
                value: None,
            },
        )]);
    }

    for (req_id, request) in pending_requests {
        rows.extend(request_action_markup(req_id, request).inline_keyboard);
    }
    InlineKeyboardMarkup::rows(rows)
}

fn request_action_markup(req_id: &str, request: &WebAgentRequest) -> InlineKeyboardMarkup {
    match request.tool.as_str() {
        "request_user_input" => {
            InlineKeyboardMarkup::single_row(vec![InlineKeyboardButton::callback(
                "Answer",
                &TelegramCallbackPayload {
                    action: "answer".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            )])
        }
        "permissions" => InlineKeyboardMarkup::rows(vec![vec![
            InlineKeyboardButton::callback(
                "Approve",
                &TelegramCallbackPayload {
                    action: "approve".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            ),
            InlineKeyboardButton::callback(
                "Approve Session",
                &TelegramCallbackPayload {
                    action: "approve_session".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            ),
            InlineKeyboardButton::callback(
                "Deny",
                &TelegramCallbackPayload {
                    action: "deny".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            ),
        ]]),
        _ => InlineKeyboardMarkup::single_row(vec![
            InlineKeyboardButton::callback(
                "Approve",
                &TelegramCallbackPayload {
                    action: "approve".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            ),
            InlineKeyboardButton::callback(
                "Deny",
                &TelegramCallbackPayload {
                    action: "deny".to_string(),
                    scope: Some(req_id.to_string()),
                    value: None,
                },
            ),
        ]),
    }
}

fn summarize_request(request: &WebAgentRequest) -> String {
    match request.tool.as_str() {
        "shell" => request
            .arguments
            .get("command")
            .and_then(JsonValue::as_str)
            .map(|command| format!("shell: {}", trim_inline_text(command, 120)))
            .unwrap_or_else(|| "shell approval".to_string()),
        "apply_patch" => "apply_patch approval".to_string(),
        "permissions" => "permissions request".to_string(),
        "request_user_input" => request
            .arguments
            .get("questions")
            .and_then(JsonValue::as_array)
            .and_then(|questions| questions.first())
            .and_then(|question| question.get("header"))
            .and_then(JsonValue::as_str)
            .map(|header| format!("user input: {header}"))
            .unwrap_or_else(|| "user input request".to_string()),
        other => other.to_string(),
    }
}

fn render_request_card_text(session: &Session, req_id: &str, request: &WebAgentRequest) -> String {
    format!(
        "{}\n\nNew pending request {}\n{}",
        session_title(session),
        short_id(req_id),
        summarize_request(request),
    )
}

fn render_answer_prompt(
    questions: &[codex_protocol::request_user_input::RequestUserInputQuestion],
) -> String {
    let mut lines = vec!["Reply with your answer in chat.".to_string(), String::new()];
    if questions.len() == 1 {
        let question = &questions[0];
        lines.push(format!("{} ({})", question.header, question.id));
        lines.push(question.question.clone());
        if let Some(options) = question.options.as_ref() {
            lines.push(String::new());
            lines.push("Options:".to_string());
            for option in options {
                lines.push(format!("- {}", option.label));
            }
        }
        return lines.join("\n");
    }

    lines.push("Use one `id=value` line per question.".to_string());
    for question in questions {
        lines.push(String::new());
        lines.push(format!("{} ({})", question.header, question.id));
        lines.push(question.question.clone());
    }
    lines.join("\n")
}

fn parse_user_input_answers(
    text: &str,
    questions: &[codex_protocol::request_user_input::RequestUserInputQuestion],
) -> anyhow::Result<JsonValue> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("answer cannot be empty");
    }

    let mut answers = JsonMap::new();
    if questions.len() == 1 && !trimmed.contains('=') {
        answers.insert(
            questions[0].id.clone(),
            serde_json::json!({ "answers": [trimmed] }),
        );
        return Ok(JsonValue::Object(answers));
    }

    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some((key, value)) = line.split_once('=') else {
            anyhow::bail!("expected `id=value` lines for multi-question input");
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            anyhow::bail!("each answer line must include both id and value");
        }
        let values = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if values.is_empty() {
            anyhow::bail!("answer for `{key}` cannot be empty");
        }
        answers.insert(key.to_string(), serde_json::json!({ "answers": values }));
    }

    for question in questions {
        if !answers.contains_key(&question.id) {
            anyhow::bail!("missing answer for `{}`", question.id);
        }
    }

    Ok(JsonValue::Object(answers))
}

async fn ensure_ok_response(response: Response) -> anyhow::Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .context("read error body")?;
    let parsed = serde_json::from_slice::<JsonValue>(&body).ok();
    let error = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(JsonValue::as_str)
        .unwrap_or("request_failed");
    anyhow::bail!("{status}: {error}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use codex_core::AuthManager;
    use codex_core::ThreadManager;
    use codex_core::config::Config;
    use codex_core::config::ConfigOverrides;
    use codex_core::models_manager::collaboration_mode_presets::CollaborationModesConfig;
    use codex_protocol::request_user_input::RequestUserInputQuestion;
    use codex_protocol::request_user_input::RequestUserInputQuestionOption;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum MockTelegramCall {
        SendMessage {
            chat_id: i64,
            text: String,
        },
        EditMessageText {
            chat_id: i64,
            message_id: i64,
            text: String,
        },
        AnswerCallback {
            callback_query_id: String,
            text: Option<String>,
        },
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    async fn test_app_state() -> AppState {
        codex_core::test_support::set_thread_manager_test_mode(true);
        let codex_home = temp_dir("telegram-worker-home");
        let base_overrides = ConfigOverrides {
            cwd: Some(codex_home.clone()),
            ..Default::default()
        };
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            Vec::new(),
            base_overrides.clone(),
        )
        .await
        .expect("load config");
        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );
        let thread_manager = Arc::new(ThreadManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            config.model_catalog.clone(),
            CollaborationModesConfig::default(),
        ));
        let (events_tx, _) = broadcast::channel(64);
        let kanban = crate::kanban::load_or_default(&config.codex_home).await;
        let workspaces =
            crate::workspace::WorkspaceStore::load_or_default(&config.codex_home).await;

        AppState {
            token: Arc::new("test-token".to_string()),
            static_dir: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            base_overrides,
            auth_manager,
            thread_manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kanban: Arc::new(RwLock::new(kanban)),
            workspaces: Arc::new(RwLock::new(workspaces)),
            github_webhook: None,
            github_repos: Arc::new(RwLock::new(Vec::new())),
            github_work_items: Arc::new(RwLock::new(GithubWorkItemsSnapshot::default())),
            github_kanban: Arc::new(RwLock::new(crate::kanban::KanbanConfig::default())),
            github_jobs: Arc::new(RwLock::new(HashMap::new())),
            github_sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            workspace_kanban_locks: Arc::new(RwLock::new(HashMap::new())),
            events_tx,
        }
    }

    async fn spawn_mock_telegram_api(
        bot_token: &str,
    ) -> anyhow::Result<(String, Arc<Mutex<Vec<MockTelegramCall>>>)> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let send_calls = Arc::clone(&calls);
        let edit_calls = Arc::clone(&calls);
        let answer_calls = Arc::clone(&calls);
        let bot_path = format!("/bot{bot_token}");
        let send_route = format!("{bot_path}/sendMessage");
        let edit_route = format!("{bot_path}/editMessageText");
        let answer_route = format!("{bot_path}/answerCallbackQuery");

        let app = Router::new()
            .route(
                send_route.as_str(),
                post(move |Json(body): Json<SendMessageRequest>| {
                    let send_calls = Arc::clone(&send_calls);
                    async move {
                        send_calls.lock().await.push(MockTelegramCall::SendMessage {
                            chat_id: body.chat_id,
                            text: body.text.clone(),
                        });
                        Json(serde_json::json!({
                            "ok": true,
                            "result": {
                                "message_id": 1,
                                "date": 0,
                                "chat": {
                                    "id": body.chat_id,
                                    "type": "private",
                                    "title": null,
                                    "username": null,
                                    "first_name": null,
                                    "last_name": null
                                },
                                "from": null,
                                "text": body.text,
                                "reply_markup": body.reply_markup
                            }
                        }))
                    }
                }),
            )
            .route(
                edit_route.as_str(),
                post(move |Json(body): Json<EditMessageTextRequest>| {
                    let edit_calls = Arc::clone(&edit_calls);
                    async move {
                        edit_calls
                            .lock()
                            .await
                            .push(MockTelegramCall::EditMessageText {
                                chat_id: body.chat_id,
                                message_id: body.message_id,
                                text: body.text.clone(),
                            });
                        Json(serde_json::json!({ "ok": true, "result": true }))
                    }
                }),
            )
            .route(
                answer_route.as_str(),
                post(move |Json(body): Json<AnswerCallbackQueryRequest>| {
                    let answer_calls = Arc::clone(&answer_calls);
                    async move {
                        answer_calls
                            .lock()
                            .await
                            .push(MockTelegramCall::AnswerCallback {
                                callback_query_id: body.callback_query_id.clone(),
                                text: body.text.clone(),
                            });
                        Json(serde_json::json!({ "ok": true, "result": true }))
                    }
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });
        Ok((format!("http://{addr}"), calls))
    }

    async fn test_worker(
        allowed_chat_ids: Vec<i64>,
        edit_throttle: Duration,
    ) -> anyhow::Result<(TelegramWorker, Arc<Mutex<Vec<MockTelegramCall>>>, String)> {
        let state = test_app_state().await;
        let bot_token = "bot-token";
        let (api_base_url, calls) = spawn_mock_telegram_api(bot_token).await?;
        let worker = TelegramWorker::new(
            state,
            TelegramBotClient::with_client(
                codex_core::default_client::build_reqwest_client(),
                bot_token,
                api_base_url.clone(),
            ),
            TelegramWorkerConfig {
                bot: TelegramBotConfig {
                    bot_token: bot_token.to_string(),
                    allowed_chat_ids,
                },
                api_base_url: api_base_url.clone(),
                poll_timeout_seconds: 30,
                edit_throttle,
            },
        );
        Ok((worker, calls, api_base_url))
    }

    async fn insert_pending_session(
        worker: &TelegramWorker,
        session_id: &str,
        request_id: &str,
        request: WebAgentRequest,
    ) {
        let thread_id = ThreadId::from_string(session_id).expect("valid session id");
        let now = now_ms();
        let mut requests = HashMap::new();
        requests.insert(request_id.to_string(), request);
        let session = Arc::new(ActiveSession {
            thread_id,
            thread: None,
            live_registration: None,
            lease_created_at: None,
            lease_heartbeat_stop: Arc::new(AtomicBool::new(false)),
            rollout_path: None,
            state: RwLock::new(SessionState {
                name: Some("Telegram Test".to_string()),
                cwd: PathBuf::from("/tmp/telegram-test"),
                model: "gpt-5".to_string(),
                reasoning_effort: None,
                created_at: now,
                updated_at: now,
                active: true,
                active_at: now,
                thinking: false,
                thinking_at: now,
                permission_mode: "default".to_string(),
                model_mode: "default".to_string(),
                metadata_version: 0,
                agent_state_version: 0,
                agent_state: WebAgentState {
                    controlled_by_user: Some(true),
                    requests: Some(requests),
                    completed_requests: None,
                },
                backing: SessionBacking::Headless,
                live_state: SessionLiveState::WaitingApproval,
                runtime_owner: RuntimeOwner::Serve,
                window_id: None,
                controller_count: 1,
                next_seq: 1,
                messages: Vec::new(),
            }),
        });
        worker
            .state
            .sessions
            .write()
            .await
            .insert(session_id.to_string(), session);
    }

    #[test]
    fn multi_question_answers_require_key_value_pairs() {
        let questions = vec![
            RequestUserInputQuestion {
                id: "plan".to_string(),
                header: "Plan".to_string(),
                question: "Choose a plan".to_string(),
                is_other: false,
                is_secret: false,
                options: Some(vec![RequestUserInputQuestionOption {
                    label: "A".to_string(),
                    description: "opt A".to_string(),
                }]),
            },
            RequestUserInputQuestion {
                id: "note".to_string(),
                header: "Note".to_string(),
                question: "Add a note".to_string(),
                is_other: true,
                is_secret: false,
                options: None,
            },
        ];

        let parsed = parse_user_input_answers("plan=A\nnote=ship it", &questions).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "plan": { "answers": ["A"] },
                "note": { "answers": ["ship it"] },
            })
        );
    }

    #[test]
    fn single_question_accepts_freeform_text() {
        let questions = vec![RequestUserInputQuestion {
            id: "reason".to_string(),
            header: "Reason".to_string(),
            question: "Why?".to_string(),
            is_other: true,
            is_secret: false,
            options: None,
        }];

        let parsed = parse_user_input_answers("because it is needed", &questions).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "reason": { "answers": ["because it is needed"] },
            })
        );
    }

    #[test]
    fn request_markup_uses_answer_for_user_input() {
        let request = WebAgentRequest {
            tool: "request_user_input".to_string(),
            arguments: serde_json::json!({
                "kind": "request_user_input",
                "questions": [{"id": "reason", "header": "Reason", "question": "Why?"}],
            }),
            created_at: None,
        };

        let markup = request_action_markup("req-1", &request);
        assert_eq!(markup.inline_keyboard.len(), 1);
        assert_eq!(markup.inline_keyboard[0].len(), 1);
        assert_eq!(markup.inline_keyboard[0][0].text, "Answer");
    }

    #[test]
    fn assistant_delta_text_ignores_reasoning_chunks() {
        let event = Event {
            id: "evt-1".to_string(),
            msg: EventMsg::ReasoningContentDelta(
                codex_protocol::protocol::ReasoningContentDeltaEvent {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    item_id: "item-1".to_string(),
                    delta: "thinking".to_string(),
                    summary_index: 0,
                },
            ),
        };
        assert_eq!(assistant_delta_text(&event), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_rejects_non_allowlisted_chat_once() -> anyhow::Result<()> {
        let (worker, calls, _) = test_worker(vec![42], Duration::from_millis(50)).await?;
        let update = crate::telegram_bot::TelegramUpdate {
            update_id: 1,
            message: Some(TelegramMessage {
                message_id: 10,
                date: 0,
                chat: crate::telegram_bot::TelegramChat {
                    id: 7,
                    kind: "private".to_string(),
                    title: None,
                    username: None,
                    first_name: None,
                    last_name: None,
                },
                from: None,
                text: Some("/projects".to_string()),
                reply_markup: None,
            }),
            edited_message: None,
            callback_query: None,
        };

        worker.handle_update(update.clone()).await?;
        worker.handle_update(update).await?;

        let calls = calls.lock().await.clone();
        assert_eq!(
            calls,
            vec![MockTelegramCall::SendMessage {
                chat_id: 7,
                text: "This bot is not enabled for this chat.".to_string(),
            }]
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn watched_session_pushes_only_to_matching_chat() -> anyhow::Result<()> {
        let (worker, calls, _) = test_worker(vec![42, 43], Duration::from_millis(50)).await?;
        {
            let mut chats = worker.chats.lock().await;
            chats.insert(
                42,
                ChatState {
                    watched_session_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
                    watched_session_title: Some("Session A".to_string()),
                    ..Default::default()
                },
            );
            chats.insert(
                43,
                ChatState {
                    watched_session_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
                    watched_session_title: Some("Session B".to_string()),
                    ..Default::default()
                },
            );
        }

        worker
            .handle_sync_event(SyncEvent::MessageDelta {
                session_id: "11111111-1111-1111-1111-111111111111".to_string(),
                event: Event {
                    id: "evt-1".to_string(),
                    msg: EventMsg::AgentMessageContentDelta(
                        codex_protocol::protocol::AgentMessageContentDeltaEvent {
                            thread_id: "thread-a".to_string(),
                            turn_id: "turn-1".to_string(),
                            item_id: "item-1".to_string(),
                            delta: "hello".to_string(),
                        },
                    ),
                },
            })
            .await?;

        let calls = calls.lock().await.clone();
        assert_eq!(
            calls,
            vec![MockTelegramCall::SendMessage {
                chat_id: 42,
                text: "Session A\n\nhello\n\nStatus: running".to_string(),
            }]
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_edits_are_throttled_until_flush_window() -> anyhow::Result<()> {
        let (worker, calls, _) = test_worker(vec![42], Duration::from_millis(20)).await?;
        {
            let mut chats = worker.chats.lock().await;
            chats.insert(
                42,
                ChatState {
                    watched_session_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
                    watched_session_title: Some("Session A".to_string()),
                    ..Default::default()
                },
            );
        }

        worker
            .append_stream_delta(42, "11111111-1111-1111-1111-111111111111", "hello")
            .await?;
        worker
            .append_stream_delta(42, "11111111-1111-1111-1111-111111111111", " world")
            .await?;
        worker.flush_stream_updates_once().await?;
        assert_eq!(calls.lock().await.len(), 1);

        tokio::time::sleep(Duration::from_millis(25)).await;
        worker.flush_stream_updates_once().await?;

        let calls = calls.lock().await.clone();
        assert_eq!(
            calls,
            vec![
                MockTelegramCall::SendMessage {
                    chat_id: 42,
                    text: "Session A\n\nhello\n\nStatus: running".to_string(),
                },
                MockTelegramCall::EditMessageText {
                    chat_id: 42,
                    message_id: 1,
                    text: "Session A\n\nhello world\n\nStatus: running".to_string(),
                },
            ]
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn approve_request_maps_to_completed_request_state() -> anyhow::Result<()> {
        let (worker, _calls, _) = test_worker(vec![42], Duration::from_millis(20)).await?;
        let session_id = "11111111-1111-1111-1111-111111111111";
        insert_pending_session(
            &worker,
            session_id,
            "req-1",
            WebAgentRequest {
                tool: "permissions".to_string(),
                arguments: serde_json::json!({
                    "kind": "permissions",
                    "callId": "call-1",
                    "turnId": "turn-1",
                    "permissions": []
                }),
                created_at: Some(now_ms()),
            },
        )
        .await;

        worker.approve_request(session_id, "req-1", true).await?;

        let session = worker
            .state
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .expect("session");
        let state = session.state.read().await;
        assert_eq!(
            state
                .agent_state
                .requests
                .as_ref()
                .map(std::collections::HashMap::len),
            Some(0)
        );
        let completed = state
            .agent_state
            .completed_requests
            .as_ref()
            .and_then(|requests| requests.get("req-1"))
            .expect("completed request");
        assert_eq!(completed.status, "approved");
        assert_eq!(completed.decision.as_deref(), Some("approved_for_session"));
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn answer_request_maps_answers_into_completed_state() -> anyhow::Result<()> {
        let (worker, _calls, _) = test_worker(vec![42], Duration::from_millis(20)).await?;
        let session_id = "22222222-2222-2222-2222-222222222222";
        insert_pending_session(
            &worker,
            session_id,
            "req-2",
            WebAgentRequest {
                tool: "request_user_input".to_string(),
                arguments: serde_json::json!({
                    "kind": "request_user_input",
                    "turnId": "turn-2",
                    "questions": [{
                        "id": "reason",
                        "header": "Reason",
                        "question": "Why?"
                    }]
                }),
                created_at: Some(now_ms()),
            },
        )
        .await;

        worker
            .answer_request(
                session_id,
                "req-2",
                serde_json::json!({
                    "reason": { "answers": ["ship it"] }
                }),
            )
            .await?;

        let session = worker
            .state
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .expect("session");
        let state = session.state.read().await;
        let completed = state
            .agent_state
            .completed_requests
            .as_ref()
            .and_then(|requests| requests.get("req-2"))
            .expect("completed request");
        assert_eq!(completed.status, "approved");
        assert_eq!(
            completed.answers.clone().unwrap_or(JsonValue::Null),
            serde_json::json!({
                "reason": { "answers": ["ship it"] }
            })
        );
        Ok(())
    }
}
