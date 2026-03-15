use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

use codex_protocol::openai_models::ReasoningEffort;

const WORKSPACES_DIR_NAME: &str = "workspaces";
const WORKSPACES_INDEX_FILE_NAME: &str = "index.json";
const WORKSPACE_FILE_NAME: &str = "workspace.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    pub repo_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub repos: Vec<RepoRef>,
    pub board: BoardConfig,
    pub default_exec: ExecConfig,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub full_name: String,
    pub color: String,
    pub short_label: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardConfig {
    pub columns: Vec<Column>,
    pub swimlane_mode: SwimLaneMode,
    pub wip_limits: HashMap<String, u8>,
    pub filters: BoardFilters,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub id: String,
    pub name: String,
    pub position: u32,
    pub auto_trigger: Option<AutoTrigger>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoTrigger {
    StartExecution,
    CloseIssue,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SwimLaneMode {
    ByEpic,
    ByRepo,
    ByAssignee,
    None,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardFilters {
    pub repos: Option<Vec<String>>,
    pub epics: Option<Vec<IssueRef>>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    pub repo: String,
    pub number: u64,
}

/// Unset = inherit from parent. Stored as-is; resolution happens at execution time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecConfig {
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub sandbox: Option<SandboxMode>,
    pub system_prompt: Option<String>,
    pub prompt: Option<String>,
    pub timeout_minutes: Option<u32>,
    pub auto_pr: Option<bool>,
    pub auto_test: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            columns: vec![
                Column {
                    id: "backlog".to_string(),
                    name: "Backlog".to_string(),
                    position: 0,
                    auto_trigger: None,
                },
                Column {
                    id: "running".to_string(),
                    name: "Running".to_string(),
                    position: 1,
                    auto_trigger: Some(AutoTrigger::StartExecution),
                },
                Column {
                    id: "testing".to_string(),
                    name: "Testing".to_string(),
                    position: 2,
                    auto_trigger: None,
                },
                Column {
                    id: "review".to_string(),
                    name: "Review".to_string(),
                    position: 3,
                    auto_trigger: None,
                },
                Column {
                    id: "done".to_string(),
                    name: "Done".to_string(),
                    position: 4,
                    auto_trigger: Some(AutoTrigger::CloseIssue),
                },
            ],
            swimlane_mode: SwimLaneMode::ByEpic,
            wip_limits: HashMap::new(),
            filters: BoardFilters::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceStore {
    index: Vec<WorkspaceSummary>,
    workspaces: HashMap<String, Workspace>,
}

#[derive(Clone, Debug)]
pub struct RepoInput {
    pub full_name: String,
    pub color: Option<String>,
    pub short_label: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CreateWorkspaceInput {
    pub name: String,
    pub repos: Vec<RepoInput>,
    pub board: Option<BoardConfig>,
    pub default_exec: Option<ExecConfig>,
    pub now_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct UpdateWorkspaceInput {
    pub name: Option<String>,
    pub repos: Option<Vec<RepoInput>>,
    pub board: Option<BoardConfig>,
    pub default_exec: Option<ExecConfig>,
    pub now_ms: u64,
}

impl WorkspaceStore {
    pub async fn load_or_default(codex_home: &Path) -> Self {
        let root = workspaces_root(codex_home);
        let index_path = root.join(WORKSPACES_INDEX_FILE_NAME);
        let content = match tokio::fs::read(&index_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn!("failed to read workspaces index: {err}");
                return Self::default();
            }
        };

        let index = match serde_json::from_slice::<Vec<WorkspaceSummary>>(&content) {
            Ok(index) => index,
            Err(err) => {
                warn!("failed to parse workspaces index, using empty: {err}");
                return Self::default();
            }
        };

        let mut kept_index = Vec::new();
        let mut workspaces = HashMap::new();
        for summary in &index {
            let path = workspace_dir(&root, &summary.id).join(WORKSPACE_FILE_NAME);
            let content = match tokio::fs::read(&path).await {
                Ok(content) => content,
                Err(err) => {
                    warn!("failed to read workspace {}: {err}", summary.id);
                    continue;
                }
            };
            match serde_json::from_slice::<Workspace>(&content) {
                Ok(ws) => {
                    kept_index.push(WorkspaceSummary {
                        id: ws.id.clone(),
                        name: ws.name.clone(),
                        repo_count: ws.repos.len() as u32,
                    });
                    workspaces.insert(ws.id.clone(), ws);
                }
                Err(err) => warn!("failed to parse workspace {}: {err}", summary.id),
            }
        }

        Self {
            index: kept_index,
            workspaces,
        }
    }

    pub fn list(&self) -> Vec<WorkspaceSummary> {
        self.index.clone()
    }

    pub fn get(&self, id: &str) -> Option<Workspace> {
        self.workspaces.get(id).cloned()
    }

    pub async fn create(
        &mut self,
        codex_home: &Path,
        input: CreateWorkspaceInput,
    ) -> anyhow::Result<Workspace> {
        let root = workspaces_root(codex_home);
        let id = uuid::Uuid::new_v4().to_string();
        let repos = normalize_repos(input.repos);
        let ws = Workspace {
            id: id.clone(),
            name: input.name,
            repos,
            board: input.board.unwrap_or_default(),
            default_exec: input.default_exec.unwrap_or_default(),
            created_at_ms: input.now_ms,
            updated_at_ms: input.now_ms,
        };

        persist_workspace(&root, &ws).await?;
        self.workspaces.insert(id.clone(), ws.clone());
        self.index.push(WorkspaceSummary {
            id: id.clone(),
            name: ws.name.clone(),
            repo_count: ws.repos.len() as u32,
        });
        persist_index(&root, &self.index).await?;

        Ok(ws)
    }

    pub async fn update(
        &mut self,
        codex_home: &Path,
        id: &str,
        input: UpdateWorkspaceInput,
    ) -> anyhow::Result<Option<Workspace>> {
        let root = workspaces_root(codex_home);
        let Some(mut ws) = self.workspaces.get(id).cloned() else {
            return Ok(None);
        };

        if let Some(name) = input.name {
            ws.name = name;
        }
        if let Some(repos) = input.repos {
            ws.repos = normalize_repos(repos);
        }
        if let Some(board) = input.board {
            ws.board = board;
        }
        if let Some(default_exec) = input.default_exec {
            ws.default_exec = default_exec;
        }
        ws.updated_at_ms = input.now_ms;

        persist_workspace(&root, &ws).await?;
        self.workspaces.insert(id.to_string(), ws.clone());

        if let Some(summary) = self.index.iter_mut().find(|s| s.id == id) {
            summary.name = ws.name.clone();
            summary.repo_count = ws.repos.len() as u32;
        }
        persist_index(&root, &self.index).await?;

        Ok(Some(ws))
    }

    pub async fn delete(&mut self, codex_home: &Path, id: &str) -> anyhow::Result<bool> {
        let root = workspaces_root(codex_home);
        if !self.workspaces.contains_key(id) {
            return Ok(false);
        }

        self.workspaces.remove(id);
        self.index.retain(|s| s.id != id);
        persist_index(&root, &self.index).await?;

        let dir = workspace_dir(&root, id);
        if let Err(err) = tokio::fs::remove_dir_all(&dir).await {
            warn!(
                "failed to remove workspace directory {}: {err}",
                dir.display()
            );
        }
        Ok(true)
    }
}

fn workspaces_root(codex_home: &Path) -> PathBuf {
    codex_home.join(WORKSPACES_DIR_NAME)
}

fn workspace_dir(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

async fn persist_workspace(root: &Path, ws: &Workspace) -> anyhow::Result<()> {
    let dir = workspace_dir(root, &ws.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create workspace dir {}", dir.display()))?;
    let path = dir.join(WORKSPACE_FILE_NAME);
    write_json_atomic(&path, ws).await
}

async fn persist_index(root: &Path, index: &[WorkspaceSummary]) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(root)
        .await
        .with_context(|| format!("create workspaces root {}", root.display()))?;
    let path = root.join(WORKSPACES_INDEX_FILE_NAME);
    write_json_atomic(&path, index).await
}

async fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> anyhow::Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    let mut body = serde_json::to_vec_pretty(value).context("serialize json")?;
    body.push(b'\n');
    tokio::fs::write(&tmp_path, body)
        .await
        .with_context(|| format!("write tmp {}", tmp_path.display()))?;

    if let Err(_err) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(path).await;
        tokio::fs::rename(&tmp_path, path)
            .await
            .with_context(|| format!("rename tmp {} -> {}", tmp_path.display(), path.display()))?;
    }
    Ok(())
}

fn normalize_repos(repos: Vec<RepoInput>) -> Vec<RepoRef> {
    repos
        .into_iter()
        .map(|repo| {
            let full_name = repo.full_name.trim().to_string();
            let color = repo.color.unwrap_or_else(|| assign_color(&full_name));
            let short_label = repo
                .short_label
                .unwrap_or_else(|| short_label_from_full_name(&full_name));
            let default_branch = repo.default_branch.unwrap_or_else(|| "main".to_string());
            RepoRef {
                full_name,
                color,
                short_label,
                default_branch,
            }
        })
        .collect()
}

fn assign_color(full_name: &str) -> String {
    // Stable palette; deterministic on repo name.
    const COLORS: [&str; 10] = [
        "#3B82F6", "#22C55E", "#F97316", "#A855F7", "#06B6D4", "#EF4444", "#EAB308", "#14B8A6",
        "#6366F1", "#EC4899",
    ];
    let mut hasher = DefaultHasher::new();
    full_name.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % COLORS.len();
    COLORS[idx].to_string()
}

fn short_label_from_full_name(full_name: &str) -> String {
    let repo = full_name.split('/').nth(1).unwrap_or(full_name);
    let mut out = String::new();
    for c in repo.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
        if out.len() >= 3 {
            break;
        }
    }
    if out.is_empty() {
        "repo".to_string()
    } else {
        out
    }
}
