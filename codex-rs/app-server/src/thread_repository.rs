use crate::codex_message_processor::build_thread_from_snapshot;
use crate::codex_message_processor::read_rollout_items_from_rollout;
use crate::codex_message_processor::read_summary_from_rollout;
use crate::codex_message_processor::read_summary_from_state_db_by_thread_id;
use crate::codex_message_processor::read_summary_from_state_db_context_by_thread_id;
use crate::codex_message_processor::summary_from_thread_list_item;
use crate::codex_message_processor::summary_to_thread;
use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use crate::filters::compute_source_filters;
use crate::filters::source_kind_matches;
use crate::thread_status::ThreadWatchManager;
use crate::thread_status::resolve_thread_status;
use async_trait::async_trait;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadSourceKind;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_core::ThreadManager;
use codex_core::ThreadSortKey as CoreThreadSortKey;
use codex_core::config::Config;
use codex_core::find_archived_thread_path_by_id_str;
use codex_core::find_thread_name_by_id;
use codex_core::find_thread_names_by_ids;
use codex_core::find_thread_path_by_id_str;
use codex_core::parse_cursor;
use codex_core::path_utils;
use codex_core::rollout_date_parts;
use codex_core::state_db::get_state_db;
use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::FileTimes;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::warn;

pub(crate) const THREAD_LIST_DEFAULT_LIMIT: usize = 25;
pub(crate) const THREAD_LIST_MAX_LIMIT: usize = 100;

pub(crate) struct ThreadListFilters {
    pub(crate) model_providers: Option<Vec<String>>,
    pub(crate) source_kinds: Option<Vec<ThreadSourceKind>>,
    pub(crate) archived: bool,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) search_term: Option<String>,
}

pub(crate) struct ThreadListQuery {
    pub(crate) requested_page_size: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) sort_key: CoreThreadSortKey,
    pub(crate) filters: ThreadListFilters,
}

pub(crate) struct ThreadListPage {
    pub(crate) data: Vec<Thread>,
    pub(crate) next_cursor: Option<String>,
}

pub(crate) struct ThreadTurnsPage {
    pub(crate) data: Vec<Turn>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) backwards_cursor: Option<String>,
}

#[async_trait]
pub(crate) trait ThreadRepository: Send + Sync {
    async fn find_rollout_path(&self, thread_id: ThreadId) -> Result<PathBuf, JSONRPCErrorError>;
    async fn list(&self, query: ThreadListQuery) -> Result<ThreadListPage, JSONRPCErrorError>;
    async fn read(&self, thread_id: &str, include_turns: bool)
    -> Result<Thread, JSONRPCErrorError>;
    async fn list_turns(
        &self,
        thread_id: &str,
        cursor: Option<String>,
        backwards_cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<ThreadTurnsPage, JSONRPCErrorError>;
    async fn unarchive(&self, thread_id: ThreadId) -> Result<Thread, JSONRPCErrorError>;
}

#[derive(Clone)]
pub(crate) struct LocalThreadRepository {
    config: Arc<Config>,
    thread_manager: Arc<ThreadManager>,
    thread_watch_manager: ThreadWatchManager,
}

impl LocalThreadRepository {
    pub(crate) fn new(
        config: Arc<Config>,
        thread_manager: Arc<ThreadManager>,
        thread_watch_manager: ThreadWatchManager,
    ) -> Self {
        Self {
            config,
            thread_manager,
            thread_watch_manager,
        }
    }
}

#[async_trait]
impl ThreadRepository for LocalThreadRepository {
    async fn find_rollout_path(&self, thread_id: ThreadId) -> Result<PathBuf, JSONRPCErrorError> {
        match find_thread_path_by_id_str(&self.config.codex_home, &thread_id.to_string()).await {
            Ok(Some(path)) => Ok(path),
            Ok(None) => Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!("no rollout found for thread id {thread_id}"),
                data: None,
            }),
            Err(err) => Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!("failed to locate thread id {thread_id}: {err}"),
                data: None,
            }),
        }
    }

    async fn list(&self, query: ThreadListQuery) -> Result<ThreadListPage, JSONRPCErrorError> {
        let ThreadListQuery {
            requested_page_size,
            cursor,
            sort_key,
            filters,
        } = query;
        let ThreadListFilters {
            model_providers,
            source_kinds,
            archived,
            cwd,
            search_term,
        } = filters;
        let mut cursor_obj = match cursor.as_ref() {
            Some(cursor_str) => {
                Some(parse_cursor(cursor_str).ok_or_else(|| JSONRPCErrorError {
                    code: INVALID_REQUEST_ERROR_CODE,
                    message: format!("invalid cursor: {cursor_str}"),
                    data: None,
                })?)
            }
            None => None,
        };
        let mut last_cursor = cursor_obj.clone();
        let mut remaining = requested_page_size;
        let mut items = Vec::with_capacity(requested_page_size);
        let mut next_cursor = None;

        let model_provider_filter = match model_providers {
            Some(providers) if providers.is_empty() => None,
            Some(providers) => Some(providers),
            None => Some(vec![self.config.model_provider_id.clone()]),
        };
        let fallback_provider = self.config.model_provider_id.clone();
        let (allowed_sources_vec, source_kind_filter) = compute_source_filters(source_kinds);
        let allowed_sources = allowed_sources_vec.as_slice();
        let state_db_ctx = get_state_db(&self.config).await;

        while remaining > 0 {
            let page_size = remaining.min(THREAD_LIST_MAX_LIMIT);
            let page = if archived {
                codex_core::RolloutRecorder::list_archived_threads(
                    &self.config,
                    page_size,
                    cursor_obj.as_ref(),
                    sort_key,
                    allowed_sources,
                    model_provider_filter.as_deref(),
                    fallback_provider.as_str(),
                    search_term.as_deref(),
                )
                .await
            } else {
                codex_core::RolloutRecorder::list_threads(
                    &self.config,
                    page_size,
                    cursor_obj.as_ref(),
                    sort_key,
                    allowed_sources,
                    model_provider_filter.as_deref(),
                    fallback_provider.as_str(),
                    search_term.as_deref(),
                )
                .await
            }
            .map_err(|err| JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("failed to list threads: {err}"),
                data: None,
            })?;

            let mut filtered = Vec::with_capacity(page.items.len());
            for item in page.items {
                let Some(summary) = summary_from_thread_list_item(
                    item,
                    fallback_provider.as_str(),
                    state_db_ctx.as_ref(),
                )
                .await
                else {
                    continue;
                };
                if source_kind_filter
                    .as_ref()
                    .is_none_or(|filter| source_kind_matches(&summary.source, filter))
                    && cwd.as_ref().is_none_or(|expected_cwd| {
                        path_utils::paths_match_after_normalization(&summary.cwd, expected_cwd)
                    })
                {
                    filtered.push(summary);
                    if filtered.len() >= remaining {
                        break;
                    }
                }
            }

            items.extend(filtered);
            remaining = requested_page_size.saturating_sub(items.len());

            let next_cursor_value = page.next_cursor.clone();
            next_cursor = next_cursor_value
                .as_ref()
                .and_then(|next| serde_json::to_value(next).ok())
                .and_then(|value| value.as_str().map(str::to_owned));
            if remaining == 0 {
                break;
            }

            match next_cursor_value {
                Some(cursor_val) if remaining > 0 => {
                    if last_cursor.as_ref() == Some(&cursor_val) {
                        next_cursor = None;
                        break;
                    }
                    last_cursor = Some(cursor_val.clone());
                    cursor_obj = Some(cursor_val);
                }
                _ => break,
            }
        }

        let mut threads = Vec::with_capacity(items.len());
        let mut thread_ids = HashSet::with_capacity(items.len());
        let mut status_ids = Vec::with_capacity(items.len());
        for summary in items {
            let conversation_id = summary.conversation_id;
            thread_ids.insert(conversation_id);

            let thread = summary_to_thread(summary);
            status_ids.push(thread.id.clone());
            threads.push((conversation_id, thread));
        }

        let names = match find_thread_names_by_ids(&self.config.codex_home, &thread_ids).await {
            Ok(names) => names,
            Err(err) => {
                warn!("Failed to read thread names: {err}");
                HashMap::new()
            }
        };

        let statuses = self
            .thread_watch_manager
            .loaded_statuses_for_threads(status_ids)
            .await;

        let data = threads
            .into_iter()
            .map(|(conversation_id, mut thread)| {
                thread.name = names.get(&conversation_id).cloned();
                if let Some(status) = statuses.get(&thread.id) {
                    thread.status = status.clone();
                }
                thread
            })
            .collect();

        Ok(ThreadListPage { data, next_cursor })
    }

    async fn read(
        &self,
        thread_id: &str,
        include_turns: bool,
    ) -> Result<Thread, JSONRPCErrorError> {
        let thread_uuid = ThreadId::from_string(thread_id).map_err(|err| JSONRPCErrorError {
            code: INVALID_REQUEST_ERROR_CODE,
            message: format!("invalid thread id: {err}"),
            data: None,
        })?;

        let loaded_thread = self.thread_manager.get_thread(thread_uuid).await.ok();
        let loaded_thread_state_db = loaded_thread.as_ref().and_then(|thread| thread.state_db());
        let db_summary = if let Some(state_db_ctx) = loaded_thread_state_db.as_ref() {
            read_summary_from_state_db_context_by_thread_id(Some(state_db_ctx), thread_uuid).await
        } else {
            read_summary_from_state_db_by_thread_id(&self.config, thread_uuid).await
        };
        let mut rollout_path = db_summary.as_ref().map(|summary| summary.path.clone());
        if rollout_path.is_none() || include_turns {
            rollout_path =
                match find_thread_path_by_id_str(&self.config.codex_home, &thread_uuid.to_string())
                    .await
                {
                    Ok(Some(path)) => Some(path),
                    Ok(None) => {
                        if include_turns {
                            None
                        } else {
                            rollout_path
                        }
                    }
                    Err(err) => {
                        return Err(JSONRPCErrorError {
                            code: INVALID_REQUEST_ERROR_CODE,
                            message: format!("failed to locate thread id {thread_uuid}: {err}"),
                            data: None,
                        });
                    }
                };
        }

        if include_turns && rollout_path.is_none() && db_summary.is_some() {
            return Err(JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("failed to locate rollout for thread {thread_uuid}"),
                data: None,
            });
        }

        let mut thread = if let Some(summary) = db_summary {
            summary_to_thread(summary)
        } else if let Some(rollout_path) = rollout_path.as_ref() {
            let fallback_provider = self.config.model_provider_id.as_str();
            read_summary_from_rollout(rollout_path, fallback_provider)
                .await
                .map(summary_to_thread)
                .map_err(|err| JSONRPCErrorError {
                    code: INTERNAL_ERROR_CODE,
                    message: format!(
                        "failed to load rollout `{}` for thread {thread_uuid}: {err}",
                        rollout_path.display()
                    ),
                    data: None,
                })?
        } else {
            let Some(thread) = loaded_thread else {
                return Err(JSONRPCErrorError {
                    code: INVALID_REQUEST_ERROR_CODE,
                    message: format!("thread not loaded: {thread_uuid}"),
                    data: None,
                });
            };
            let config_snapshot = thread.config_snapshot().await;
            let loaded_rollout_path = thread.rollout_path();
            if include_turns && loaded_rollout_path.is_none() {
                return Err(JSONRPCErrorError {
                    code: INVALID_REQUEST_ERROR_CODE,
                    message: "ephemeral threads do not support includeTurns".to_string(),
                    data: None,
                });
            }
            if include_turns {
                rollout_path = loaded_rollout_path.clone();
            }
            build_thread_from_snapshot(thread_uuid, &config_snapshot, loaded_rollout_path)
        };

        if include_turns && let Some(rollout_path) = rollout_path.as_ref() {
            match read_rollout_items_from_rollout(rollout_path).await {
                Ok(items) => {
                    thread.turns = build_turns_from_rollout_items(&items);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(JSONRPCErrorError {
                        code: INVALID_REQUEST_ERROR_CODE,
                        message: format!(
                            "thread {thread_uuid} is not materialized yet; includeTurns is unavailable before first user message"
                        ),
                        data: None,
                    });
                }
                Err(err) => {
                    return Err(JSONRPCErrorError {
                        code: INTERNAL_ERROR_CODE,
                        message: format!(
                            "failed to load rollout `{}` for thread {thread_uuid}: {err}",
                            rollout_path.display()
                        ),
                        data: None,
                    });
                }
            }
        }

        match find_thread_name_by_id(&self.config.codex_home, &thread_uuid).await {
            Ok(name) => {
                thread.name = name;
            }
            Err(err) => {
                warn!("Failed to read thread name for {thread_uuid}: {err}");
            }
        }
        thread.status = resolve_thread_status(
            self.thread_watch_manager
                .loaded_status_for_thread(&thread.id)
                .await,
            false,
        );
        Ok(thread)
    }

    async fn list_turns(
        &self,
        thread_id: &str,
        cursor: Option<String>,
        backwards_cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<ThreadTurnsPage, JSONRPCErrorError> {
        if cursor.is_some() && backwards_cursor.is_some() {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: "cursor and backwardsCursor are mutually exclusive".to_string(),
                data: None,
            });
        }

        let thread = self.read(thread_id, true).await?;
        let total = thread.turns.len();
        let limit = limit.unwrap_or(THREAD_LIST_DEFAULT_LIMIT as u32).max(1) as usize;
        let limit = limit.min(THREAD_LIST_MAX_LIMIT);
        let start_cursor = cursor.or(backwards_cursor);
        let start = match start_cursor {
            Some(value) => value.parse::<usize>().map_err(|_| JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!("invalid cursor: {value}"),
                data: None,
            })?,
            None => 0,
        };

        if start > total {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!("cursor {start} exceeds total turns {total}"),
                data: None,
            });
        }

        let end = start.saturating_add(limit).min(total);
        let data = thread.turns[start..end].to_vec();
        let next_cursor = (end < total).then(|| end.to_string());
        let backwards_cursor = (start > 0).then(|| start.saturating_sub(limit).to_string());

        Ok(ThreadTurnsPage {
            data,
            next_cursor,
            backwards_cursor,
        })
    }

    async fn unarchive(&self, thread_id: ThreadId) -> Result<Thread, JSONRPCErrorError> {
        let archived_path = match find_archived_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
        )
        .await
        {
            Ok(Some(path)) => path,
            Ok(None) => {
                return Err(JSONRPCErrorError {
                    code: INVALID_REQUEST_ERROR_CODE,
                    message: format!("no archived rollout found for thread id {thread_id}"),
                    data: None,
                });
            }
            Err(err) => {
                return Err(JSONRPCErrorError {
                    code: INVALID_REQUEST_ERROR_CODE,
                    message: format!("failed to locate archived thread id {thread_id}: {err}"),
                    data: None,
                });
            }
        };

        let rollout_path_display = archived_path.display().to_string();
        let fallback_provider = self.config.model_provider_id.clone();
        let state_db_ctx = get_state_db(&self.config).await;
        let archived_folder = self
            .config
            .codex_home
            .join(codex_core::ARCHIVED_SESSIONS_SUBDIR);

        let canonical_archived_dir =
            tokio::fs::canonicalize(&archived_folder)
                .await
                .map_err(|err| JSONRPCErrorError {
                    code: INTERNAL_ERROR_CODE,
                    message: format!(
                        "failed to unarchive thread: unable to resolve archived directory: {err}"
                    ),
                    data: None,
                })?;
        let canonical_rollout_path = tokio::fs::canonicalize(&archived_path).await;
        let canonical_rollout_path = if let Ok(path) = canonical_rollout_path
            && path.starts_with(&canonical_archived_dir)
        {
            path
        } else {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!(
                    "rollout path `{rollout_path_display}` must be in archived directory"
                ),
                data: None,
            });
        };

        let required_suffix = format!("{thread_id}.jsonl");
        let Some(file_name) = canonical_rollout_path.file_name().map(OsStr::to_owned) else {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!("rollout path `{rollout_path_display}` missing file name"),
                data: None,
            });
        };
        if !file_name
            .to_string_lossy()
            .ends_with(required_suffix.as_str())
        {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!(
                    "rollout path `{rollout_path_display}` does not match thread id {thread_id}"
                ),
                data: None,
            });
        }

        let Some((year, month, day)) = rollout_date_parts(&file_name) else {
            return Err(JSONRPCErrorError {
                code: INVALID_REQUEST_ERROR_CODE,
                message: format!(
                    "rollout path `{rollout_path_display}` missing filename timestamp"
                ),
                data: None,
            });
        };

        let sessions_folder = self.config.codex_home.join(codex_core::SESSIONS_SUBDIR);
        let dest_dir = sessions_folder.join(year).join(month).join(day);
        let restored_path = dest_dir.join(&file_name);
        tokio::fs::create_dir_all(&dest_dir)
            .await
            .map_err(|err| JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("failed to unarchive thread: {err}"),
                data: None,
            })?;
        tokio::fs::rename(&canonical_rollout_path, &restored_path)
            .await
            .map_err(|err| JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: format!("failed to unarchive thread: {err}"),
                data: None,
            })?;
        tokio::task::spawn_blocking({
            let restored_path = restored_path.clone();
            move || -> std::io::Result<()> {
                let times = FileTimes::new().set_modified(SystemTime::now());
                OpenOptions::new()
                    .append(true)
                    .open(&restored_path)?
                    .set_times(times)?;
                Ok(())
            }
        })
        .await
        .map_err(|err| JSONRPCErrorError {
            code: INTERNAL_ERROR_CODE,
            message: format!("failed to update unarchived thread timestamp: {err}"),
            data: None,
        })?
        .map_err(|err| JSONRPCErrorError {
            code: INTERNAL_ERROR_CODE,
            message: format!("failed to update unarchived thread timestamp: {err}"),
            data: None,
        })?;
        if let Some(ctx) = state_db_ctx {
            let _ = ctx
                .mark_unarchived(thread_id, restored_path.as_path())
                .await;
        }
        let summary =
            read_summary_from_rollout(restored_path.as_path(), fallback_provider.as_str())
                .await
                .map_err(|err| JSONRPCErrorError {
                    code: INTERNAL_ERROR_CODE,
                    message: format!("failed to read unarchived thread: {err}"),
                    data: None,
                })?;
        let mut thread = summary_to_thread(summary);
        thread.status = resolve_thread_status(
            self.thread_watch_manager
                .loaded_status_for_thread(&thread.id)
                .await,
            false,
        );
        match find_thread_name_by_id(&self.config.codex_home, &thread_id).await {
            Ok(name) => {
                thread.name = name;
            }
            Err(err) => {
                warn!("Failed to read thread name for {thread_id}: {err}");
            }
        }
        Ok(thread)
    }
}
