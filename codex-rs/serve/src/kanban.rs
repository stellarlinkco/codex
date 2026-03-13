use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

use codex_protocol::openai_models::ReasoningEffort;

const DEFAULT_KANBAN_FILE_NAME: &str = "kanban.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KanbanConfig {
    pub columns: Vec<KanbanColumn>,
    pub card_positions: HashMap<String, CardPosition>,
    #[serde(default)]
    pub card_settings: HashMap<String, KanbanCardSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KanbanCardSettings {
    pub prompt_prefix: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KanbanColumn {
    pub id: String,
    pub name: String,
    pub position: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardPosition {
    pub column_id: String,
    pub position: u32,
}

impl Default for KanbanConfig {
    fn default() -> Self {
        Self {
            columns: vec![
                KanbanColumn {
                    id: "backlog".to_string(),
                    name: "Backlog".to_string(),
                    position: 0,
                },
                KanbanColumn {
                    id: "in-progress".to_string(),
                    name: "In Progress".to_string(),
                    position: 1,
                },
                KanbanColumn {
                    id: "review".to_string(),
                    name: "Review".to_string(),
                    position: 2,
                },
                KanbanColumn {
                    id: "done".to_string(),
                    name: "Done".to_string(),
                    position: 3,
                },
            ],
            card_positions: HashMap::new(),
            card_settings: HashMap::new(),
        }
    }
}

fn kanban_path_with_name(codex_home: &Path, file_name: &str) -> PathBuf {
    codex_home.join(file_name)
}

pub async fn load_or_default(codex_home: &Path) -> KanbanConfig {
    load_or_default_from(codex_home, DEFAULT_KANBAN_FILE_NAME).await
}

pub async fn load_or_default_from(codex_home: &Path, file_name: &str) -> KanbanConfig {
    let path = kanban_path_with_name(codex_home, file_name);
    let content = match tokio::fs::read(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let cfg = KanbanConfig::default();
            persist_to(codex_home, file_name, &cfg).await;
            return cfg;
        }
        Err(err) => {
            warn!("failed to read {file_name}: {err}");
            return KanbanConfig::default();
        }
    };

    match serde_json::from_slice::<KanbanConfig>(&content) {
        Ok(mut cfg) => {
            if cfg.columns.is_empty() {
                cfg.columns = KanbanConfig::default().columns;
            }
            cfg
        }
        Err(err) => {
            warn!("failed to parse {file_name}, using default: {err}");
            KanbanConfig::default()
        }
    }
}

pub async fn persist(codex_home: &Path, cfg: &KanbanConfig) {
    persist_to(codex_home, DEFAULT_KANBAN_FILE_NAME, cfg).await;
}

pub async fn persist_to(codex_home: &Path, file_name: &str, cfg: &KanbanConfig) {
    if let Err(err) = tokio::fs::create_dir_all(codex_home).await {
        warn!("failed to create codex home dir for {file_name}: {err}");
        return;
    }
    let path = kanban_path_with_name(codex_home, file_name);
    let tmp_path = path.with_extension("json.tmp");
    let mut body = match serde_json::to_vec_pretty(cfg) {
        Ok(body) => body,
        Err(err) => {
            warn!("failed to serialize {file_name}: {err}");
            return;
        }
    };
    body.push(b'\n');

    if let Err(err) = tokio::fs::write(&tmp_path, body).await {
        warn!("failed to write {file_name} tmp: {err}");
        return;
    }

    if let Err(_err) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        if let Err(err) = tokio::fs::rename(&tmp_path, &path).await {
            warn!("failed to persist {file_name}: {err}");
        }
    }
}

impl KanbanConfig {
    pub fn has_column(&self, column_id: &str) -> bool {
        self.columns.iter().any(|c| c.id == column_id)
    }

    pub fn reconcile_sessions(&mut self, session_ids: &HashSet<String>) -> bool {
        let mut changed = false;

        if self.columns.is_empty() {
            self.columns = KanbanConfig::default().columns;
            changed = true;
        }

        let before = self.card_positions.len();
        self.card_positions
            .retain(|session_id, _| session_ids.contains(session_id));
        if self.card_positions.len() != before {
            changed = true;
        }

        let before_settings = self.card_settings.len();
        self.card_settings
            .retain(|session_id, _| session_ids.contains(session_id));
        if self.card_settings.len() != before_settings {
            changed = true;
        }

        for session_id in session_ids {
            changed |= self.ensure_session(session_id);
        }

        changed |= self.normalize_positions();
        changed
    }

    pub fn ensure_session(&mut self, session_id: &str) -> bool {
        if self.card_positions.contains_key(session_id) {
            return false;
        }
        let Some(first_col) = self
            .columns
            .iter()
            .min_by_key(|c| c.position)
            .map(|c| c.id.as_str())
        else {
            return false;
        };

        let next = self
            .card_positions
            .values()
            .filter(|pos| pos.column_id == first_col)
            .map(|pos| pos.position)
            .max()
            .map(|max| max.saturating_add(1))
            .unwrap_or(0);

        self.card_positions.insert(
            session_id.to_string(),
            CardPosition {
                column_id: first_col.to_string(),
                position: next,
            },
        );
        true
    }

    pub fn remove_session(&mut self, session_id: &str) -> bool {
        let removed = self.card_positions.remove(session_id).is_some();
        self.card_settings.remove(session_id);
        removed
    }

    pub fn move_card(&mut self, session_id: &str, column_id: &str, position: u32) -> bool {
        if !self.has_column(column_id) {
            return false;
        }
        let current = self.card_positions.get(session_id).cloned();
        if current
            .as_ref()
            .is_some_and(|pos| pos.column_id == column_id && pos.position == position)
        {
            return false;
        }

        let from_col = current.as_ref().map(|pos| pos.column_id.as_str());

        let mut target_items: Vec<(String, u32)> = self
            .card_positions
            .iter()
            .filter_map(|(id, pos)| {
                if pos.column_id == column_id {
                    Some((id.clone(), pos.position))
                } else {
                    None
                }
            })
            .collect();
        target_items.sort_by_key(|(_, pos)| *pos);
        let mut target_ids: Vec<String> = target_items.into_iter().map(|(id, _)| id).collect();

        target_ids.retain(|id| id != session_id);

        let insert_at = (position as usize).min(target_ids.len());
        target_ids.insert(insert_at, session_id.to_string());

        for (idx, id) in target_ids.iter().enumerate() {
            self.card_positions.insert(
                id.clone(),
                CardPosition {
                    column_id: column_id.to_string(),
                    position: idx as u32,
                },
            );
        }

        if let Some(from_col) = from_col
            && from_col != column_id
        {
            let mut from_items: Vec<(String, u32)> = self
                .card_positions
                .iter()
                .filter_map(|(id, pos)| {
                    if pos.column_id == from_col {
                        Some((id.clone(), pos.position))
                    } else {
                        None
                    }
                })
                .collect();
            from_items.sort_by_key(|(_, pos)| *pos);
            let mut from_ids: Vec<String> = from_items.into_iter().map(|(id, _)| id).collect();
            from_ids.retain(|id| id != session_id);
            for (idx, id) in from_ids.iter().enumerate() {
                self.card_positions.insert(
                    id.clone(),
                    CardPosition {
                        column_id: from_col.to_string(),
                        position: idx as u32,
                    },
                );
            }
        }

        true
    }

    pub fn apply_moves(&mut self, moves: &[(String, CardPosition)]) -> bool {
        let mut changed = false;
        for (session_id, pos) in moves {
            if !self.has_column(&pos.column_id) {
                continue;
            }
            match self.card_positions.get(session_id) {
                Some(existing)
                    if existing.column_id == pos.column_id && existing.position == pos.position => {
                }
                _ => {
                    self.card_positions.insert(session_id.clone(), pos.clone());
                    changed = true;
                }
            }
        }
        changed |= self.normalize_positions();
        changed
    }

    fn normalize_positions(&mut self) -> bool {
        let mut changed = false;
        let mut by_col: HashMap<String, Vec<(String, u32)>> = HashMap::new();
        for (session_id, pos) in &self.card_positions {
            by_col
                .entry(pos.column_id.clone())
                .or_default()
                .push((session_id.clone(), pos.position));
        }

        for (col, mut items) in by_col {
            items.sort_by_key(|(_, p)| *p);
            for (idx, (session_id, old)) in items.into_iter().enumerate() {
                let next = idx as u32;
                if old != next {
                    changed = true;
                }
                self.card_positions.insert(
                    session_id,
                    CardPosition {
                        column_id: col.clone(),
                        position: next,
                    },
                );
            }
        }

        changed
    }
}
