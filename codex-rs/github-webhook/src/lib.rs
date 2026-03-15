use anyhow::Context;
use anyhow::Result;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_core::config::types::GithubWebhookAuthModeToml;
use codex_core::config::types::GithubWebhookEventsToml;
use codex_core::config::types::GithubWebhookSourceToml;
use codex_core::config::types::GithubWebhookToml;
use hmac::Hmac;
use hmac::Mac;
use jsonwebtoken::Algorithm;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header as JwtHeader;
use reqwest::header::ACCEPT;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap as ReqwestHeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue as ReqwestHeaderValue;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;

const DEFAULT_WEBHOOK_SECRET_ENV: &str = "GITHUB_WEBHOOK_SECRET";
const DEFAULT_GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
const DEFAULT_GITHUB_APP_ID_ENV: &str = "GITHUB_APP_ID";
const DEFAULT_GITHUB_APP_PRIVATE_KEY_ENV: &str = "GITHUB_APP_PRIVATE_KEY";
const DEFAULT_COMMAND_PREFIX: &str = "/codex";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_CONTEXT_FILENAME: &str = ".codex_github_context.md";
const REPO_MANAGED_MARKER_FILENAME: &str = ".codex_github_managed";
const REPO_LAST_USED_FILENAME: &str = ".codex_github_last_used";
const DEFAULT_DELIVERY_TTL_DAYS: u64 = 7;
const DEFAULT_REPO_TTL_DAYS: u64 = 0;
const GITHUB_API_PER_PAGE: usize = 100;
const GITHUB_API_MAX_PAGES: usize = 100;
const MAX_WEBHOOK_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_CONCURRENCY: usize = 2;
const GITHUB_API_TIMEOUT: Duration = Duration::from_secs(20);
const GC_INTERVAL: Duration = Duration::from_secs(60 * 60);
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CODEX_EXEC_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const GITHUB_APP_JWT_BACKDATE_SECS: u64 = 60;
const GITHUB_APP_JWT_LIFETIME_SECS: u64 = 9 * 60;
const ACKNOWLEDGMENT_MESSAGE: &str = "codex github received this request and is working on it.";
const ACKNOWLEDGMENT_REACTION: &str = "eyes";

type HmacSha256 = Hmac<Sha256>;

fn ttl_from_days(days: u64) -> Option<Duration> {
    if days == 0 {
        return None;
    }
    let secs = days.saturating_mul(24 * 60 * 60);
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
std::thread_local! {
    static TEST_GIT_COMMAND_TIMEOUT_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn git_command_timeout() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_GIT_COMMAND_TIMEOUT_MS.with(std::cell::Cell::get);
        if ms != 0 {
            return Duration::from_millis(ms);
        }
    }
    GIT_COMMAND_TIMEOUT
}

#[cfg(test)]
struct TestGitCommandTimeoutGuard;

#[cfg(test)]
impl TestGitCommandTimeoutGuard {
    fn set(timeout: Duration) -> Self {
        let ms = timeout.as_millis().try_into().unwrap_or(u64::MAX);
        TEST_GIT_COMMAND_TIMEOUT_MS.with(|slot| slot.set(ms));
        Self
    }
}

#[cfg(test)]
impl Drop for TestGitCommandTimeoutGuard {
    fn drop(&mut self) {
        TEST_GIT_COMMAND_TIMEOUT_MS.with(|slot| slot.set(0));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MinPermission {
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl MinPermission {
    fn allows(self, actual: &str) -> bool {
        let Some(actual_rank) = permission_rank(actual) else {
            return false;
        };
        let required_rank = match self {
            MinPermission::Read => 1,
            MinPermission::Triage => 2,
            MinPermission::Write => 3,
            MinPermission::Maintain => 4,
            MinPermission::Admin => 5,
        };
        actual_rank >= required_rank
    }
}

fn permission_rank(permission: &str) -> Option<u8> {
    match permission {
        "read" => Some(1),
        "triage" => Some(2),
        "write" => Some(3),
        "maintain" => Some(4),
        "admin" => Some(5),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WebhookSource {
    Repo,
    Organization,
    GithubApp,
}

impl WebhookSource {
    fn from_headers_and_payload(headers: &HeaderMap, payload: &Value) -> Self {
        if payload
            .get("installation")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_u64)
            .is_some()
        {
            return Self::GithubApp;
        }
        let target_type = header_string(headers, "X-GitHub-Hook-Installation-Target-Type")
            .map(|s| s.to_ascii_lowercase());
        match target_type.as_deref() {
            Some("organization") => Self::Organization,
            Some("integration") => Self::GithubApp,
            _ => Self::Repo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GithubEvent {
    IssueComment,
    Issues,
    PullRequest,
    PullRequestReviewComment,
    PullRequestReview,
    Push,
}

impl GithubEvent {
    fn from_name(event: &str) -> Option<Self> {
        match event {
            "issue_comment" => Some(Self::IssueComment),
            "issues" => Some(Self::Issues),
            "pull_request" => Some(Self::PullRequest),
            "pull_request_review_comment" => Some(Self::PullRequestReviewComment),
            "pull_request_review" => Some(Self::PullRequestReview),
            "push" => Some(Self::Push),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnabledEvents {
    issue_comment: bool,
    issues: bool,
    pull_request: bool,
    pull_request_review: bool,
    pull_request_review_comment: bool,
    push: bool,
}

impl EnabledEvents {
    fn legacy_default() -> Self {
        Self {
            issue_comment: true,
            issues: false,
            pull_request: false,
            pull_request_review: true,
            pull_request_review_comment: true,
            push: false,
        }
    }

    fn expanded_default() -> Self {
        Self {
            issue_comment: true,
            issues: true,
            pull_request: true,
            pull_request_review: true,
            pull_request_review_comment: true,
            push: true,
        }
    }

    fn allows(self, event: GithubEvent) -> bool {
        match event {
            GithubEvent::IssueComment => self.issue_comment,
            GithubEvent::Issues => self.issues,
            GithubEvent::PullRequest => self.pull_request,
            GithubEvent::PullRequestReview => self.pull_request_review,
            GithubEvent::PullRequestReviewComment => self.pull_request_review_comment,
            GithubEvent::Push => self.push,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RepoKey {
    owner: String,
    repo: String,
}

#[derive(Debug, Clone)]
struct GithubAppCredentials {
    app_id: u64,
    private_key: Arc<String>,
}

#[derive(Clone)]
struct GithubAuthConfig {
    mode: GithubWebhookAuthModeToml,
    static_token: Arc<String>,
    app: Option<Arc<GithubAppCredentials>>,
}

impl GithubAuthConfig {
    fn static_token(&self) -> Option<&str> {
        (!self.static_token.is_empty()).then_some(self.static_token.as_str())
    }
}

#[derive(Clone)]
struct AppState {
    secret: Arc<Vec<u8>>,
    github_api_base_url: Arc<String>,
    github_auth: Arc<GithubAuthConfig>,
    allow_repos: Arc<HashSet<String>>,
    enabled_sources: Arc<HashSet<WebhookSource>>,
    enabled_events: EnabledEvents,
    min_permission: MinPermission,
    command_prefix: Arc<String>,
    repo_root: Arc<PathBuf>,
    codex_bin: Arc<PathBuf>,
    codex_config_overrides: Arc<Vec<String>>,
    delivery_markers_dir: Arc<PathBuf>,
    thread_state_dir: Arc<PathBuf>,
    delivery_ttl: Option<Duration>,
    repo_ttl: Option<Duration>,
    concurrency_limit: Arc<Semaphore>,
    work_locks: Arc<Mutex<HashMap<WorkKey, Arc<Mutex<()>>>>>,
    repo_locks: Arc<Mutex<HashMap<RepoKey, Arc<Mutex<()>>>>>,
}

#[derive(Clone)]
pub struct GithubWebhook {
    state: Arc<AppState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubLabelSummary {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoWorkItem {
    pub repo: String,
    pub kind: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub updated_at: String,
    pub labels: Vec<GithubLabelSummary>,
    pub comments: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubCodexJobOutput {
    pub thread_id: Option<String>,
    pub last_message: String,
}

#[derive(Debug, Clone, Default)]
pub struct GithubCodexRunOverrides {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubWorkItemDetail {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub updated_at: String,
    pub body: String,
}

impl GithubWebhook {
    pub fn try_from_config(
        codex_home: &Path,
        github_webhook: Option<&GithubWebhookToml>,
        codex_bin: PathBuf,
        mut codex_config_overrides: Vec<String>,
    ) -> Result<Option<Self>> {
        let Some(github_webhook) = github_webhook else {
            return Ok(None);
        };
        if github_webhook.enabled != Some(true) {
            return Ok(None);
        }

        let has_github_webhook_config = true;
        let min_permission = match github_webhook.min_permission.as_deref() {
            Some(value) => parse_min_permission_str(value)?,
            None => MinPermission::Triage,
        };
        let auth_mode = github_webhook
            .auth_mode
            .unwrap_or(GithubWebhookAuthModeToml::Auto);

        let webhook_secret_env = github_webhook
            .webhook_secret_env
            .clone()
            .unwrap_or_else(|| DEFAULT_WEBHOOK_SECRET_ENV.to_string());
        let github_token_env = github_webhook
            .github_token_env
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_TOKEN_ENV.to_string());
        let github_app_id_env = github_webhook
            .github_app_id_env
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_APP_ID_ENV.to_string());
        let github_app_private_key_env = github_webhook
            .github_app_private_key_env
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_APP_PRIVATE_KEY_ENV.to_string());

        let secret = read_env_required(&webhook_secret_env, "GitHub webhook secret")?;
        let token = read_env_optional(&github_token_env)?.unwrap_or_default();
        let github_app = load_github_app_credentials(
            &github_app_id_env,
            &github_app_private_key_env,
            auth_mode,
        )?;
        if auth_mode == GithubWebhookAuthModeToml::Token && token.is_empty() {
            anyhow::bail!("GitHub token not set: missing env {github_token_env}");
        }
        if auth_mode == GithubWebhookAuthModeToml::Auto && token.is_empty() && github_app.is_none()
        {
            anyhow::bail!(
                "codex serve github webhook requires either env {github_token_env} or GitHub App envs {github_app_id_env} and {github_app_private_key_env}"
            );
        }

        let enabled_sources = resolve_enabled_sources(github_webhook.sources.clone());
        let enabled_events =
            resolve_enabled_events(github_webhook.events.clone(), has_github_webhook_config);

        let allow_repo = github_webhook.allow_repos.clone().unwrap_or_default();
        let allow_repos = normalize_allowlist(&allow_repo);

        let command_prefix = github_webhook
            .command_prefix
            .clone()
            .unwrap_or_else(|| DEFAULT_COMMAND_PREFIX.to_string());

        let delivery_ttl = ttl_from_days(
            github_webhook
                .delivery_ttl_days
                .unwrap_or(DEFAULT_DELIVERY_TTL_DAYS),
        );
        let repo_ttl = ttl_from_days(
            github_webhook
                .repo_ttl_days
                .unwrap_or(DEFAULT_REPO_TTL_DAYS),
        );

        codex_config_overrides.push("approval_policy=\"never\"".to_string());
        codex_config_overrides.push("sandbox_mode=\"workspace-write\"".to_string());

        let repo_root = codex_home.join("github-repos");
        let delivery_markers_dir = codex_home.join("github").join("deliveries");
        let thread_state_dir = codex_home.join("github").join("threads");

        let state = AppState {
            secret: Arc::new(secret.into_bytes()),
            github_api_base_url: Arc::new(GITHUB_API_BASE_URL.to_string()),
            github_auth: Arc::new(GithubAuthConfig {
                mode: auth_mode,
                static_token: Arc::new(token),
                app: github_app,
            }),
            allow_repos: Arc::new(allow_repos),
            enabled_sources: Arc::new(enabled_sources),
            enabled_events,
            min_permission,
            command_prefix: Arc::new(command_prefix),
            repo_root: Arc::new(repo_root),
            codex_bin: Arc::new(codex_bin),
            codex_config_overrides: Arc::new(codex_config_overrides),
            delivery_markers_dir: Arc::new(delivery_markers_dir),
            thread_state_dir: Arc::new(thread_state_dir),
            delivery_ttl,
            repo_ttl,
            concurrency_limit: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENCY)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        Ok(Some(Self {
            state: Arc::new(state),
        }))
    }

    pub fn spawn_gc_loop_if_needed(&self) {
        if self.state.delivery_ttl.is_some() || self.state.repo_ttl.is_some() {
            tokio::spawn(gc_loop(self.state.as_ref().clone()));
        }
    }

    pub async fn handle_webhook(&self, headers: HeaderMap, body: Bytes) -> Response {
        handle_webhook_inner(self.state.clone(), headers, body).await
    }

    pub async fn list_repo_work_items(
        &self,
        repo_full_name: &str,
    ) -> Result<Vec<GithubRepoWorkItem>> {
        let repo_full_name = repo_full_name.trim();
        if repo_full_name.is_empty() {
            return Ok(Vec::new());
        }
        let (owner, repo) = split_owner_repo(repo_full_name)?;
        let dummy = WorkItem {
            repo_full_name: repo_full_name.to_string(),
            sender_login: owner.to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: owner.to_string(),
                repo: repo.to_string(),
                kind: WorkKind::Issue,
                number: 0,
            },
            prompt: String::new(),
            display_target: String::new(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::None,
        };

        let access = resolve_github_access(self.state.as_ref(), &dummy).await?;
        let url_base = format!(
            "{}/repos/{owner}/{repo}/issues?state=all",
            access.github.base_url
        );
        let raw = access.github.list_paginated(url_base).await?;
        let mut out = Vec::with_capacity(raw.len());
        for v in raw {
            match parse_repo_work_item(repo_full_name, v) {
                Ok(item) => out.push(item),
                Err(err) => {
                    eprintln!("failed to parse GitHub work item in {repo_full_name}: {err:#}")
                }
            }
        }
        Ok(out)
    }

    pub async fn fetch_work_item_detail(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<GithubWorkItemDetail> {
        let repo_full_name = repo_full_name.trim();
        if repo_full_name.is_empty() {
            anyhow::bail!("missing repo");
        }
        let (owner, repo) = split_owner_repo(repo_full_name)?;
        let dummy = WorkItem {
            repo_full_name: repo_full_name.to_string(),
            sender_login: owner.to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: owner.to_string(),
                repo: repo.to_string(),
                kind: WorkKind::Issue,
                number,
            },
            prompt: String::new(),
            display_target: format!("#{number}"),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::None,
        };

        let access = resolve_github_access(self.state.as_ref(), &dummy).await?;
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/{number}",
            access.github.base_url
        );
        let v = access.github.get_json_value(url).await?;
        let title = v.get("title").and_then(Value::as_str).unwrap_or_default();
        let state = v.get("state").and_then(Value::as_str).unwrap_or_default();
        let url = v
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let updated_at = v
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = v.get("body").and_then(Value::as_str).unwrap_or_default();
        Ok(GithubWorkItemDetail {
            repo: repo_full_name.to_string(),
            number,
            title: title.to_string(),
            state: state.to_string(),
            url: url.to_string(),
            updated_at: updated_at.to_string(),
            body: body.to_string(),
        })
    }

    pub async fn set_work_item_state(
        &self,
        repo_full_name: &str,
        number: u64,
        state: &str,
    ) -> Result<()> {
        let repo_full_name = repo_full_name.trim();
        if repo_full_name.is_empty() {
            anyhow::bail!("missing repo");
        }
        let state = match state.trim().to_ascii_lowercase().as_str() {
            "open" => "open",
            "closed" => "closed",
            other => anyhow::bail!("invalid issue state: {other}"),
        };
        let (owner, repo) = split_owner_repo(repo_full_name)?;
        let dummy = WorkItem {
            repo_full_name: repo_full_name.to_string(),
            sender_login: owner.to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: owner.to_string(),
                repo: repo.to_string(),
                kind: WorkKind::Issue,
                number,
            },
            prompt: String::new(),
            display_target: format!("#{number}"),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::None,
        };

        let access = resolve_github_access(self.state.as_ref(), &dummy).await?;
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/{number}",
            access.github.base_url
        );
        access
            .github
            .patch_json(url, serde_json::json!({ "state": state }))
            .await
    }

    pub async fn run_codex_for_work_item(
        &self,
        repo_full_name: &str,
        kind: &str,
        number: u64,
        prompt: String,
        overrides: GithubCodexRunOverrides,
        log_path: Option<std::path::PathBuf>,
    ) -> Result<GithubCodexJobOutput> {
        let (owner, repo) = split_owner_repo(repo_full_name)?;
        let kind = match kind.trim().to_ascii_lowercase().as_str() {
            "issue" => WorkKind::Issue,
            "pull" => WorkKind::Pull,
            other => anyhow::bail!("invalid work kind: {other}"),
        };

        let permit = self
            .state
            .concurrency_limit
            .clone()
            .try_acquire_owned()
            .context("busy")?;

        let response_target = match kind {
            WorkKind::Issue => ResponseTarget::IssueComment {
                issue_number: number,
            },
            WorkKind::Pull => ResponseTarget::PullRequestReview {
                pull_number: number,
            },
            WorkKind::Push => ResponseTarget::None,
        };
        let item = WorkItem {
            repo_full_name: repo_full_name.to_string(),
            sender_login: owner.to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: owner.to_string(),
                repo: repo.to_string(),
                kind,
                number,
            },
            prompt,
            display_target: format!("#{number}"),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target,
        };

        let state = self.state.as_ref().clone();
        let work_lock = {
            let mut locks = state.work_locks.lock().await;
            locks
                .entry(item.work.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = work_lock.lock_owned().await;

        let mut override_args = Vec::new();
        if let Some(model) = overrides
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if model.contains('\"') || model.contains('\n') || model.contains('\r') {
                anyhow::bail!("invalid model override");
            }
            override_args.push(format!("model=\"{model}\""));
        }
        if let Some(effort) = overrides
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if effort.contains('\"') || effort.contains('\n') || effort.contains('\r') {
                anyhow::bail!("invalid reasoning effort override");
            }
            override_args.push(format!("model_reasoning_effort=\"{effort}\""));
        }

        let output =
            run_work_item_inner_with_output(&state, &item, &override_args, log_path.as_deref())
                .await?;
        drop(permit);
        Ok(GithubCodexJobOutput {
            thread_id: output.thread_id,
            last_message: output.last_message,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkKey {
    owner: String,
    repo: String,
    kind: WorkKind,
    number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WorkKind {
    Issue,
    Pull,
    Push,
}

impl WorkKind {
    fn dir_name(self) -> &'static str {
        match self {
            WorkKind::Issue => "issues",
            WorkKind::Pull => "pulls",
            WorkKind::Push => "pushes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            WorkKind::Issue => "issue",
            WorkKind::Pull => "pull",
            WorkKind::Push => "push",
        }
    }
}

#[derive(Debug, Clone)]
struct WorkItem {
    repo_full_name: String,
    sender_login: String,
    source: WebhookSource,
    installation_id: Option<u64>,
    work: WorkKey,
    prompt: String,
    display_target: String,
    push_ref: Option<String>,
    push_after: Option<String>,
    ack_target: AckTarget,
    response_target: ResponseTarget,
}

#[derive(Debug, Clone)]
enum ResponseTarget {
    None,
    IssueComment { issue_number: u64 },
    ReviewCommentReply { comment_id: u64 },
    PullRequestReview { pull_number: u64 },
}

#[derive(Debug, Clone, Copy)]
enum AckTarget {
    None,
    IssueComment { comment_id: u64 },
    ReviewComment { comment_id: u64 },
}

#[derive(Debug)]
struct ResolvedGithubAccess {
    token: String,
    github: GithubApi,
}

#[derive(Serialize)]
struct GithubAppJwtClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[derive(Debug)]
struct GithubApi {
    client: reqwest::Client,
    base_url: String,
}

impl GithubApi {
    fn new_with_base_url(token: String, base_url: String) -> Result<Self> {
        Self::new_with_optional_token(base_url, Some(token))
    }

    fn new_with_optional_token(base_url: String, token: Option<String>) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let mut headers = ReqwestHeaderMap::new();
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            let auth = format!("Bearer {token}");
            headers.insert(
                AUTHORIZATION,
                ReqwestHeaderValue::from_str(&auth).context("invalid GitHub token")?,
            );
        }
        headers.insert(
            ACCEPT,
            ReqwestHeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, ReqwestHeaderValue::from_static("codex-cli"));
        headers.insert(
            HeaderName::from_static("x-github-api-version"),
            ReqwestHeaderValue::from_static(GITHUB_API_VERSION),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(GITHUB_API_TIMEOUT)
            .build()
            .context("failed to build GitHub HTTP client")?;

        Ok(Self { client, base_url })
    }

    async fn repo_permission(&self, owner: &str, repo: &str, user: &str) -> Result<Option<String>> {
        let url = format!(
            "{}/repos/{owner}/{repo}/collaborators/{user}/permission",
            self.base_url
        );
        let res = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to query collaborator permission")?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            anyhow::bail!("GitHub permission API failed ({status}): {body}");
        }
        let v: Value = serde_json::from_str(&body).context("invalid GitHub permission JSON")?;
        let permission = v
            .get("permission")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if permission.is_empty() {
            anyhow::bail!("GitHub permission API returned empty permission: {body}");
        }
        Ok(Some(permission.to_string()))
    }

    async fn repo_default_branch(&self, owner: &str, repo: &str) -> Result<String> {
        let url = format!("{}/repos/{owner}/{repo}", self.base_url);
        let res = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to query repository metadata")?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub repo API failed ({status}): {body}");
        }
        let v: Value = serde_json::from_str(&body).context("invalid GitHub repo JSON")?;
        let default_branch = v
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if default_branch.is_empty() {
            anyhow::bail!("GitHub repo API returned empty default_branch: {body}");
        }
        Ok(default_branch.to_string())
    }

    async fn repo_installation_id(&self, owner: &str, repo: &str) -> Result<u64> {
        let url = format!("{}/repos/{owner}/{repo}/installation", self.base_url);
        let response = self.get_json_value(url).await?;
        response
            .get("id")
            .and_then(Value::as_u64)
            .context("GitHub repo installation API returned empty id")
    }

    async fn org_installation_id(&self, org: &str) -> Result<u64> {
        let url = format!("{}/orgs/{org}/installation", self.base_url);
        let response = self.get_json_value(url).await?;
        response
            .get("id")
            .and_then(Value::as_u64)
            .context("GitHub org installation API returned empty id")
    }

    async fn create_installation_token(&self, installation_id: u64) -> Result<String> {
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.base_url
        );
        let response = self.post_json_value(url, serde_json::json!({})).await?;
        response
            .get("token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .context("GitHub installation token API returned empty token")
    }

    async fn get_json_value(&self, url: String) -> Result<Value> {
        let res = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to call GitHub API")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub API failed ({status}): {text}");
        }
        serde_json::from_str(&text).context("invalid GitHub JSON")
    }

    async fn get_json_vec(&self, url: String) -> Result<Vec<Value>> {
        let res = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to call GitHub API")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub API failed ({status}): {text}");
        }
        serde_json::from_str(&text).context("invalid GitHub JSON")
    }

    async fn list_paginated(&self, url_base: String) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let sep = if url_base.contains('?') { '&' } else { '?' };
        for page in 1..=GITHUB_API_MAX_PAGES {
            let url = format!("{url_base}{sep}per_page={GITHUB_API_PER_PAGE}&page={page}");
            let batch = self.get_json_vec(url).await?;
            let n = batch.len();
            out.extend(batch);
            if n < GITHUB_API_PER_PAGE {
                return Ok(out);
            }
        }
        anyhow::bail!("GitHub API pagination exceeded max pages ({GITHUB_API_MAX_PAGES})");
    }

    async fn post_issue_comment(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/{issue_number}/comments",
            self.base_url
        );
        self.post_json(url, serde_json::json!({ "body": body }))
            .await
    }

    async fn post_review_comment_reply(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/pulls/comments/{comment_id}/replies",
            self.base_url
        );
        self.post_json(url, serde_json::json!({ "body": body }))
            .await
    }

    async fn post_issue_comment_reaction(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
        content: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions",
            self.base_url
        );
        self.post_json(url, serde_json::json!({ "content": content }))
            .await
    }

    async fn post_review_comment_reaction(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
        content: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions",
            self.base_url
        );
        self.post_json(url, serde_json::json!({ "content": content }))
            .await
    }

    async fn create_pr_review(
        &self,
        owner: &str,
        repo: &str,
        pull_number: u64,
        body: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/pulls/{pull_number}/reviews",
            self.base_url
        );
        self.post_json(
            url,
            serde_json::json!({
                "body": body,
                "event": "COMMENT"
            }),
        )
        .await
    }

    async fn post_json(&self, url: String, body: Value) -> Result<()> {
        self.post_json_value(url, body).await?;
        Ok(())
    }

    async fn patch_json(&self, url: String, body: Value) -> Result<()> {
        self.patch_json_value(url, body).await?;
        Ok(())
    }

    async fn post_json_value(&self, url: String, body: Value) -> Result<Value> {
        let res = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .context("failed to call GitHub API")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub API failed ({status}): {text}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).context("invalid GitHub JSON")
    }

    async fn patch_json_value(&self, url: String, body: Value) -> Result<Value> {
        let res = self
            .client
            .patch(url)
            .json(&body)
            .send()
            .await
            .context("failed to call GitHub API")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("GitHub API failed ({status}): {text}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).context("invalid GitHub JSON")
    }
}

fn parse_min_permission_str(raw: &str) -> Result<MinPermission> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "read" => Ok(MinPermission::Read),
        "triage" => Ok(MinPermission::Triage),
        "write" => Ok(MinPermission::Write),
        "maintain" => Ok(MinPermission::Maintain),
        "admin" => Ok(MinPermission::Admin),
        other => anyhow::bail!("invalid github_webhook.min_permission: {other}"),
    }
}

fn resolve_enabled_sources(
    configured: Option<Vec<GithubWebhookSourceToml>>,
) -> HashSet<WebhookSource> {
    let mut enabled_sources = HashSet::new();
    for source in configured.unwrap_or_else(|| {
        vec![
            GithubWebhookSourceToml::Repo,
            GithubWebhookSourceToml::Organization,
            GithubWebhookSourceToml::GithubApp,
        ]
    }) {
        let mapped = match source {
            GithubWebhookSourceToml::Repo => WebhookSource::Repo,
            GithubWebhookSourceToml::Organization => WebhookSource::Organization,
            GithubWebhookSourceToml::GithubApp => WebhookSource::GithubApp,
        };
        enabled_sources.insert(mapped);
    }
    enabled_sources
}

fn resolve_enabled_events(
    configured: Option<GithubWebhookEventsToml>,
    has_github_webhook_config: bool,
) -> EnabledEvents {
    let mut enabled_events = if has_github_webhook_config {
        EnabledEvents::expanded_default()
    } else {
        EnabledEvents::legacy_default()
    };
    if let Some(configured) = configured {
        if let Some(value) = configured.issue_comment {
            enabled_events.issue_comment = value;
        }
        if let Some(value) = configured.issues {
            enabled_events.issues = value;
        }
        if let Some(value) = configured.pull_request {
            enabled_events.pull_request = value;
        }
        if let Some(value) = configured.pull_request_review {
            enabled_events.pull_request_review = value;
        }
        if let Some(value) = configured.pull_request_review_comment {
            enabled_events.pull_request_review_comment = value;
        }
        if let Some(value) = configured.push {
            enabled_events.push = value;
        }
    }
    enabled_events
}

fn read_env_optional(env_var: &str) -> Result<Option<String>> {
    match std::env::var(env_var) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("environment variable {env_var} is not valid UTF-8")
        }
    }
}

fn normalize_github_app_private_key(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.contains("BEGIN") {
        return Ok(trimmed.replace("\\n", "\n"));
    }
    let decoded = BASE64_STANDARD
        .decode(trimmed)
        .context("failed to decode GitHub App private key as base64")?;
    String::from_utf8(decoded).context("GitHub App private key base64 payload is not valid UTF-8")
}

fn load_github_app_credentials(
    app_id_env: &str,
    private_key_env: &str,
    auth_mode: GithubWebhookAuthModeToml,
) -> Result<Option<Arc<GithubAppCredentials>>> {
    let app_id = read_env_optional(app_id_env)?;
    let private_key = read_env_optional(private_key_env)?;
    match (app_id, private_key) {
        (None, None) => {
            if auth_mode == GithubWebhookAuthModeToml::GithubApp {
                anyhow::bail!("GitHub App auth requires envs {app_id_env} and {private_key_env}");
            }
            Ok(None)
        }
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("GitHub App auth requires both envs {app_id_env} and {private_key_env}")
        }
        (Some(app_id), Some(private_key)) => Ok(Some(Arc::new(GithubAppCredentials {
            app_id: app_id
                .parse::<u64>()
                .with_context(|| format!("invalid GitHub App ID in env {app_id_env}"))?,
            private_key: Arc::new(normalize_github_app_private_key(&private_key)?),
        }))),
    }
}

async fn gc_loop(state: AppState) {
    loop {
        if let Some(ttl) = state.delivery_ttl
            && let Err(err) = gc_delivery_markers(state.delivery_markers_dir.as_ref(), ttl).await
        {
            eprintln!("delivery gc failed: {err:#}");
        }
        if let Some(ttl) = state.repo_ttl
            && let Err(err) = gc_repo_caches(&state, ttl).await
        {
            eprintln!("repo gc failed: {err:#}");
        }
        tokio::time::sleep(GC_INTERVAL).await;
    }
}

async fn gc_delivery_markers(dir: &Path, ttl: Duration) -> Result<()> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", dir.display())),
    };
    let now = SystemTime::now();
    while let Some(entry) = rd
        .next_entry()
        .await
        .with_context(|| format!("failed to read {}", dir.display()))?
    {
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("marker") {
            continue;
        }
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let age = match now.duration_since(modified) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if age < ttl {
            continue;
        }
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(())
}

async fn gc_repo_caches(state: &AppState, ttl: Duration) -> Result<()> {
    let root = state.repo_root.as_ref();
    let mut owners = match tokio::fs::read_dir(root).await {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", root.display())),
    };

    while let Some(owner_entry) = owners
        .next_entry()
        .await
        .with_context(|| format!("failed to read {}", root.display()))?
    {
        let owner_ft = match owner_entry.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !owner_ft.is_dir() {
            continue;
        }
        let owner_name = owner_entry.file_name();
        let Some(owner) = owner_name.to_str() else {
            continue;
        };
        let owner_dir = owner_entry.path();
        let mut repos = match tokio::fs::read_dir(&owner_dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Some(repo_entry) = repos.next_entry().await? {
            let repo_ft = match repo_entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !repo_ft.is_dir() {
                continue;
            }
            let repo_name = repo_entry.file_name();
            let Some(repo) = repo_name.to_str() else {
                continue;
            };
            gc_repo_cache_if_stale(state, owner, repo, ttl).await?;
        }
    }

    Ok(())
}

async fn gc_repo_cache_if_stale(
    state: &AppState,
    owner: &str,
    repo: &str,
    ttl: Duration,
) -> Result<()> {
    let repo_dir = state.repo_root.join(owner).join(repo);
    if !repo_dir.join(REPO_MANAGED_MARKER_FILENAME).exists() {
        return Ok(());
    }

    let Some(last_used) = read_repo_last_used(&repo_dir).await? else {
        return Ok(());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now < last_used {
        return Ok(());
    }
    let age = Duration::from_secs(now - last_used);
    if age < ttl {
        return Ok(());
    }

    if !repo_worktrees_are_empty(&repo_dir).await? {
        return Ok(());
    }

    let repo_lock = repo_lock_for(state, owner, repo).await;
    let _guard = repo_lock.lock().await;

    if !repo_dir.join(REPO_MANAGED_MARKER_FILENAME).exists() {
        return Ok(());
    }
    let Some(last_used) = read_repo_last_used(&repo_dir).await? else {
        return Ok(());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now < last_used {
        return Ok(());
    }
    let age = Duration::from_secs(now - last_used);
    if age < ttl {
        return Ok(());
    }
    if !repo_worktrees_are_empty(&repo_dir).await? {
        return Ok(());
    }

    tokio::fs::remove_dir_all(&repo_dir)
        .await
        .with_context(|| format!("failed to delete {}", repo_dir.display()))?;
    Ok(())
}

async fn repo_worktrees_are_empty(repo_dir: &Path) -> Result<bool> {
    let issues_ok = dir_is_empty(&repo_dir.join("issues")).await?;
    let pulls_ok = dir_is_empty(&repo_dir.join("pulls")).await?;
    let pushes_ok = dir_is_empty(&repo_dir.join("pushes")).await?;
    Ok(issues_ok && pulls_ok && pushes_ok)
}

async fn dir_is_empty(path: &Path) -> Result<bool> {
    let mut rd = match tokio::fs::read_dir(path).await {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    Ok(rd.next_entry().await?.is_none())
}

async fn read_repo_last_used(repo_dir: &Path) -> Result<Option<u64>> {
    let path = repo_dir.join(REPO_LAST_USED_FILENAME);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed.parse::<u64>() {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}

async fn handle_webhook_inner(state: Arc<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > MAX_WEBHOOK_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "payload too large").into_response();
    }
    let Some(event_name) = header_string(&headers, "X-GitHub-Event") else {
        return (StatusCode::BAD_REQUEST, "missing X-GitHub-Event").into_response();
    };
    let Some(delivery_id) = header_string(&headers, "X-GitHub-Delivery") else {
        return (StatusCode::BAD_REQUEST, "missing X-GitHub-Delivery").into_response();
    };
    let signature = header_value(&headers, "X-Hub-Signature-256");
    if !verify_github_signature(&state.secret, &body, signature) {
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };

    let source = WebhookSource::from_headers_and_payload(&headers, &payload);
    if !state.enabled_sources.contains(&source) {
        return (StatusCode::ACCEPTED, "ignored").into_response();
    }

    let Some(event) = GithubEvent::from_name(&event_name) else {
        return (StatusCode::ACCEPTED, "ignored").into_response();
    };
    if !state.enabled_events.allows(event) {
        return (StatusCode::ACCEPTED, "ignored").into_response();
    }

    let work_item =
        match parse_work_item_with_source(event, source, &payload, &state.command_prefix) {
            Ok(Some(item)) => item,
            Ok(None) => return (StatusCode::ACCEPTED, "ignored").into_response(),
            Err(err) => {
                eprintln!("payload parse failed: {err:#}");
                return (StatusCode::BAD_REQUEST, "invalid payload").into_response();
            }
        };

    if !repo_allowed(&state.allow_repos, &work_item.repo_full_name) {
        return (StatusCode::ACCEPTED, "ignored").into_response();
    }

    let permit = match state.concurrency_limit.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            if let Err(err) =
                post_failure(&state, &work_item, "codex github is busy; try again later").await
            {
                eprintln!("failed to post busy notification: {err:#}");
            }
            return (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response();
        }
    };

    match sender_allowed(&state, &work_item).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::ACCEPTED, "ignored").into_response(),
        Err(err) => {
            eprintln!("sender permission check failed: {err:#}");
            if let Err(post_err) = post_failure(
                &state,
                &work_item,
                "codex github could not verify sender permissions",
            )
            .await
            {
                eprintln!("failed to post permission-check notification: {post_err:#}");
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, "permission check failed").into_response();
        }
    }

    match claim_delivery(&state.delivery_markers_dir, &delivery_id).await {
        Ok(false) => return (StatusCode::ACCEPTED, "duplicate delivery").into_response(),
        Ok(true) => {}
        Err(err) => {
            eprintln!("delivery claim failed: {err:#}");
            if let Err(post_err) = post_failure(
                &state,
                &work_item,
                "codex github could not claim this delivery",
            )
            .await
            {
                eprintln!("failed to post delivery-claim notification: {post_err:#}");
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to claim delivery",
            )
                .into_response();
        }
    }

    if let Err(err) = post_ack(&state, &work_item).await {
        eprintln!("failed to post ack: {err:#}");
    }

    tokio::spawn(process_work_item(state.as_ref().clone(), work_item, permit));
    (StatusCode::ACCEPTED, "queued").into_response()
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    headers.get(name)
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn read_env_required(env_var: &str, label: &str) -> Result<String> {
    let value = std::env::var(env_var)
        .with_context(|| format!("{label} not set: missing env {env_var}"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} is empty: env {env_var}");
    }
    Ok(trimmed.to_string())
}

fn normalize_allowlist(items: &[String]) -> HashSet<String> {
    items
        .iter()
        .map(|s| normalize_repo_full_name(s))
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalize_repo_full_name(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn repo_allowed(allow_repos: &HashSet<String>, repo_full_name: &str) -> bool {
    if allow_repos.is_empty() {
        return true;
    }
    allow_repos.contains(&normalize_repo_full_name(repo_full_name))
}

fn parse_repo_work_item(repo_full_name: &str, payload: Value) -> Result<GithubRepoWorkItem> {
    let number = payload
        .get("number")
        .and_then(Value::as_u64)
        .context("missing number")?;
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = payload
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let updated_at = payload
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let comments = payload.get("comments").and_then(Value::as_u64).unwrap_or(0);

    let kind = if payload.get("pull_request").is_some() {
        "pull".to_string()
    } else {
        "issue".to_string()
    };

    let labels = payload
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| {
                    let name = label.get("name")?.as_str()?;
                    let color = label.get("color")?.as_str()?;
                    Some(GithubLabelSummary {
                        name: name.to_string(),
                        color: color.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(GithubRepoWorkItem {
        repo: repo_full_name.to_string(),
        kind,
        number,
        title,
        state,
        url,
        updated_at,
        labels,
        comments,
    })
}

fn generate_github_app_jwt(credentials: &GithubAppCredentials) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let claims = GithubAppJwtClaims {
        iat: now.saturating_sub(GITHUB_APP_JWT_BACKDATE_SECS),
        exp: now.saturating_add(GITHUB_APP_JWT_LIFETIME_SECS),
        iss: credentials.app_id.to_string(),
    };
    let encoding_key = EncodingKey::from_rsa_pem(credentials.private_key.as_bytes())
        .context("failed to parse GitHub App private key")?;
    jsonwebtoken::encode(&JwtHeader::new(Algorithm::RS256), &claims, &encoding_key)
        .context("failed to sign GitHub App JWT")
}

async fn resolve_github_app_access(
    state: &AppState,
    item: &WorkItem,
) -> Result<ResolvedGithubAccess> {
    let credentials = state
        .github_auth
        .app
        .as_ref()
        .context("GitHub App credentials are not configured")?;
    let jwt = generate_github_app_jwt(credentials)?;
    let app_github = GithubApi::new_with_base_url(jwt, state.github_api_base_url.as_ref().clone())?;
    let installation_id = match item.installation_id {
        Some(value) => value,
        None => match item.source {
            WebhookSource::Organization => {
                match app_github.org_installation_id(&item.work.owner).await {
                    Ok(value) => value,
                    Err(_) => {
                        app_github
                            .repo_installation_id(&item.work.owner, &item.work.repo)
                            .await?
                    }
                }
            }
            WebhookSource::Repo | WebhookSource::GithubApp => {
                app_github
                    .repo_installation_id(&item.work.owner, &item.work.repo)
                    .await?
            }
        },
    };
    let token = app_github
        .create_installation_token(installation_id)
        .await?;
    let github =
        GithubApi::new_with_base_url(token.clone(), state.github_api_base_url.as_ref().clone())?;
    Ok(ResolvedGithubAccess { token, github })
}

async fn resolve_github_access(state: &AppState, item: &WorkItem) -> Result<ResolvedGithubAccess> {
    match state.github_auth.mode {
        GithubWebhookAuthModeToml::Token => {
            let token = state
                .github_auth
                .static_token()
                .context("GitHub token mode requires a configured static token")?
                .to_string();
            let github = GithubApi::new_with_base_url(
                token.clone(),
                state.github_api_base_url.as_ref().clone(),
            )?;
            Ok(ResolvedGithubAccess { token, github })
        }
        GithubWebhookAuthModeToml::GithubApp => resolve_github_app_access(state, item).await,
        GithubWebhookAuthModeToml::Auto => {
            if state.github_auth.app.is_some() {
                match resolve_github_app_access(state, item).await {
                    Ok(access) => return Ok(access),
                    Err(err) if state.github_auth.static_token().is_some() => {
                        eprintln!(
                            "GitHub App auth failed for {}: {err:#}; falling back to static token",
                            item.repo_full_name
                        );
                    }
                    Err(err) => return Err(err),
                }
            }
            let token = state
                .github_auth
                .static_token()
                .context(
                    "GitHub auto auth requires either a static token or GitHub App credentials",
                )?
                .to_string();
            let github = GithubApi::new_with_base_url(
                token.clone(),
                state.github_api_base_url.as_ref().clone(),
            )?;
            Ok(ResolvedGithubAccess { token, github })
        }
    }
}

async fn sender_allowed(state: &AppState, item: &WorkItem) -> Result<bool> {
    let owner = item.work.owner.as_str();
    let repo = item.work.repo.as_str();
    let sender = item.sender_login.as_str();
    if sender.eq_ignore_ascii_case(owner) {
        return Ok(true);
    }
    let access = resolve_github_access(state, item).await?;
    let permission = access
        .github
        .repo_permission(owner, repo, sender)
        .await
        .with_context(|| format!("permission API failed for {owner}/{repo} {sender}"))?;
    let Some(permission) = permission else {
        eprintln!("sender {sender} not allowed on {owner}/{repo} (permission API returned 404)");
        return Ok(false);
    };
    if state.min_permission.allows(&permission) {
        Ok(true)
    } else {
        eprintln!(
            "sender {sender} has {permission} but requires {:?}",
            state.min_permission
        );
        Ok(false)
    }
}

fn verify_github_signature(secret: &[u8], body: &[u8], signature: Option<&HeaderValue>) -> bool {
    let Some(signature) = signature.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(hex) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let Some(sig_bytes) = decode_hex(hex) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut it = s.as_bytes().iter().copied();
    while let (Some(hi), Some(lo)) = (it.next(), it.next()) {
        let hi = from_hex_digit(hi)?;
        let lo = from_hex_digit(lo)?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

async fn claim_delivery(dir: &Path, delivery_id: &str) -> Result<bool> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to create delivery markers dir {}", dir.display()))?;
    let marker_name = sanitize_filename_component(delivery_id);
    let marker = dir.join(format!("{marker_name}.marker"));
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .await
    {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err).context("failed to create delivery marker file"),
    }
}

fn sanitize_filename_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(ch),
            _ => out.push('_'),
        }
    }
    if out.is_empty() { "_".to_string() } else { out }
}

fn installation_id_from_payload(payload: &Value) -> Option<u64> {
    payload
        .get("installation")
        .and_then(|v| v.get("id"))
        .and_then(Value::as_u64)
}

fn extract_sender_login(payload: &Value) -> Result<&str> {
    payload
        .get("sender")
        .and_then(|v| v.get("login"))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("pusher")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
        })
        .context("missing sender.login")
}

fn hash_work_number(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn display_target_for_push(ref_name: &str, after: &str) -> String {
    let short_sha: String = after.chars().take(7).collect();
    if short_sha.is_empty() {
        ref_name.to_string()
    } else {
        format!("{ref_name}@{short_sha}")
    }
}

#[derive(Clone, Copy)]
struct ParseContext<'a> {
    owner: &'a str,
    repo: &'a str,
    repo_full_name: &'a str,
    sender_login: &'a str,
    source: WebhookSource,
    installation_id: Option<u64>,
    command_prefix: &'a str,
}

fn parse_work_item_with_source(
    event: GithubEvent,
    source: WebhookSource,
    payload: &Value,
    command_prefix: &str,
) -> Result<Option<WorkItem>> {
    let repo_full_name = payload
        .get("repository")
        .and_then(|v| v.get("full_name"))
        .and_then(Value::as_str)
        .context("missing repository.full_name")?;
    let (owner, repo) = split_owner_repo(repo_full_name)?;
    let sender_login = extract_sender_login(payload)?;
    let ctx = ParseContext {
        owner,
        repo,
        repo_full_name,
        sender_login,
        source,
        installation_id: installation_id_from_payload(payload),
        command_prefix,
    };

    match event {
        GithubEvent::IssueComment => parse_issue_comment(ctx, payload),
        GithubEvent::Issues => parse_issue_event(ctx, payload),
        GithubEvent::PullRequest => parse_pull_request_event(ctx, payload),
        GithubEvent::PullRequestReviewComment => parse_review_comment(ctx, payload),
        GithubEvent::PullRequestReview => parse_review(ctx, payload),
        GithubEvent::Push => parse_push_event(ctx, payload),
    }
}

fn parse_issue_comment(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "created" | "edited") {
        return Ok(None);
    }

    let issue = payload.get("issue").context("missing issue")?;
    let issue_number = issue
        .get("number")
        .and_then(Value::as_u64)
        .context("missing issue.number")?;
    let comment = payload.get("comment").context("missing comment")?;
    let body = comment
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_command(body, ctx.command_prefix) else {
        return Ok(None);
    };

    let is_pr = issue.get("pull_request").is_some();
    let work_kind = if is_pr {
        WorkKind::Pull
    } else {
        WorkKind::Issue
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: work_kind,
            number: issue_number,
        },
        prompt,
        display_target: format!("#{issue_number}"),
        push_ref: None,
        push_after: None,
        ack_target: comment
            .get("id")
            .and_then(Value::as_u64)
            .map_or(AckTarget::None, |comment_id| AckTarget::IssueComment {
                comment_id,
            }),
        response_target: ResponseTarget::IssueComment { issue_number },
    }))
}

fn parse_issue_event(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "opened" | "edited" | "reopened") {
        return Ok(None);
    }
    let issue = payload.get("issue").context("missing issue")?;
    let issue_number = issue
        .get("number")
        .and_then(Value::as_u64)
        .context("missing issue.number")?;
    let body = issue
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_command(body, ctx.command_prefix) else {
        return Ok(None);
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: WorkKind::Issue,
            number: issue_number,
        },
        prompt,
        display_target: format!("#{issue_number}"),
        push_ref: None,
        push_after: None,
        ack_target: AckTarget::None,
        response_target: ResponseTarget::IssueComment { issue_number },
    }))
}

fn parse_pull_request_event(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "opened" | "edited" | "reopened" | "synchronize") {
        return Ok(None);
    }
    let pull_request = payload
        .get("pull_request")
        .context("missing pull_request")?;
    let pull_number = pull_request
        .get("number")
        .and_then(Value::as_u64)
        .or_else(|| payload.get("number").and_then(Value::as_u64))
        .context("missing pull_request.number")?;
    let body = pull_request
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_command(body, ctx.command_prefix) else {
        return Ok(None);
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: WorkKind::Pull,
            number: pull_number,
        },
        prompt,
        display_target: format!("#{pull_number}"),
        push_ref: None,
        push_after: None,
        ack_target: AckTarget::None,
        response_target: ResponseTarget::IssueComment {
            issue_number: pull_number,
        },
    }))
}

fn parse_review_comment(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "created" | "edited") {
        return Ok(None);
    }

    let pull_number = payload
        .get("pull_request")
        .and_then(|v| v.get("number"))
        .and_then(Value::as_u64)
        .context("missing pull_request.number")?;
    let comment = payload.get("comment").context("missing comment")?;
    let comment_id = comment
        .get("id")
        .and_then(Value::as_u64)
        .context("missing comment.id")?;
    let body = comment
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_command(body, ctx.command_prefix) else {
        return Ok(None);
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: WorkKind::Pull,
            number: pull_number,
        },
        prompt,
        display_target: format!("#{pull_number}"),
        push_ref: None,
        push_after: None,
        ack_target: AckTarget::ReviewComment { comment_id },
        response_target: ResponseTarget::ReviewCommentReply { comment_id },
    }))
}

fn parse_review(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "submitted" | "edited") {
        return Ok(None);
    }

    let pull_number = payload
        .get("pull_request")
        .and_then(|v| v.get("number"))
        .and_then(Value::as_u64)
        .context("missing pull_request.number")?;
    let review = payload.get("review").context("missing review")?;
    let body = review
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_review_commands(body, ctx.command_prefix) else {
        return Ok(None);
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: WorkKind::Pull,
            number: pull_number,
        },
        prompt,
        display_target: format!("#{pull_number}"),
        push_ref: None,
        push_after: None,
        ack_target: AckTarget::None,
        response_target: ResponseTarget::PullRequestReview { pull_number },
    }))
}

fn parse_push_event(ctx: ParseContext<'_>, payload: &Value) -> Result<Option<WorkItem>> {
    if payload.get("deleted").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let ref_name = payload
        .get("ref")
        .and_then(Value::as_str)
        .context("missing ref")?;
    let branch_name = match ref_name.strip_prefix("refs/heads/") {
        Some(value) if !value.is_empty() => value,
        _ => return Ok(None),
    };
    let after = payload
        .get("after")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if after.is_empty() || after.chars().all(|ch| ch == '0') {
        return Ok(None);
    }
    let head_commit = match payload.get("head_commit") {
        Some(value) => value,
        None => return Ok(None),
    };
    let message = head_commit
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(prompt) = extract_command(message, ctx.command_prefix) else {
        return Ok(None);
    };

    Ok(Some(WorkItem {
        repo_full_name: ctx.repo_full_name.to_string(),
        sender_login: ctx.sender_login.to_string(),
        source: ctx.source,
        installation_id: ctx.installation_id,
        work: WorkKey {
            owner: ctx.owner.to_string(),
            repo: ctx.repo.to_string(),
            kind: WorkKind::Push,
            number: hash_work_number(ref_name),
        },
        prompt,
        display_target: display_target_for_push(branch_name, after),
        push_ref: Some(branch_name.to_string()),
        push_after: Some(after.to_string()),
        ack_target: AckTarget::None,
        response_target: ResponseTarget::None,
    }))
}

#[cfg(test)]
fn parse_work_item(event: &str, payload: &Value, command_prefix: &str) -> Result<Option<WorkItem>> {
    let Some(event) = GithubEvent::from_name(event) else {
        return Ok(None);
    };
    let source = if installation_id_from_payload(payload).is_some() {
        WebhookSource::GithubApp
    } else if payload.get("organization").is_some() {
        WebhookSource::Organization
    } else {
        WebhookSource::Repo
    };
    parse_work_item_with_source(event, source, payload, command_prefix)
}

fn split_owner_repo(full_name: &str) -> Result<(&str, &str)> {
    let mut it = full_name.split('/');
    let owner = it.next().unwrap_or_default();
    let repo = it.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || it.next().is_some() {
        anyhow::bail!("invalid repository.full_name: {full_name}");
    }
    if !is_safe_repo_component(owner) || !is_safe_repo_component(repo) {
        anyhow::bail!("invalid repository.full_name: {full_name}");
    }
    Ok((owner, repo))
}

fn is_safe_repo_component(s: &str) -> bool {
    if matches!(s, "." | "..") {
        return false;
    }
    s.chars()
        .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.'))
}

fn extract_command(body: &str, prefix: &str) -> Option<String> {
    strip_prefix_prompt(body, prefix).ok().flatten()
}

fn extract_review_commands(body: &str, prefix: &str) -> Option<String> {
    strip_prefix_lines(body, prefix)
}

fn strip_prefix_prompt(body: &str, prefix: &str) -> Result<Option<String>> {
    let body = body.trim_start();
    let Some(rest) = strip_prefix_with_boundary(body, prefix) else {
        return Ok(None);
    };
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Ok(None);
    }
    Ok(Some(rest.to_string()))
}

fn strip_prefix_lines(body: &str, prefix: &str) -> Option<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim_start();
        let Some(rest) = strip_prefix_with_boundary(line, prefix) else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.is_empty() {
            continue;
        }
        out.push(rest.to_string());
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

fn strip_prefix_with_boundary<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if !s.starts_with(prefix) {
        return None;
    }
    let rest = &s[prefix.len()..];
    if rest.is_empty() {
        return Some(rest);
    }
    rest.chars().next().filter(|c| c.is_whitespace())?;
    Some(rest)
}

async fn process_work_item(
    state: AppState,
    item: WorkItem,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let work_lock = {
        let mut locks = state.work_locks.lock().await;
        locks
            .entry(item.work.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = work_lock.lock_owned().await;
    if let Err(err) = process_work_item_inner(&state, &item).await {
        eprintln!("work item failed: {err:#}");
        let _ = post_failure(&state, &item, &format!("{err:#}")).await;
    }
}

async fn process_work_item_inner(state: &AppState, item: &WorkItem) -> Result<()> {
    let _ = run_work_item_inner_with_output(state, item, &[], None).await?;
    Ok(())
}

async fn run_work_item_inner_with_output(
    state: &AppState,
    item: &WorkItem,
    codex_config_overrides: &[String],
    log_path: Option<&Path>,
) -> Result<CodexOutput> {
    let access = resolve_github_access(state, item).await?;
    let work_dir = worktree_path(state, &item.work);
    ensure_repo_and_worktree(state, item, &access.github, &access.token, &work_dir).await?;
    let output = run_codex_in_worktree_with_github(
        state,
        item,
        &access.github,
        &work_dir,
        codex_config_overrides,
        log_path,
    )
    .await?;
    if let Some(thread_id) = output.thread_id.as_deref() {
        write_thread_id(state, &item.work, thread_id).await?;
    }
    post_success_with_github(&access.github, item, &output.last_message).await?;
    Ok(output)
}

fn worktree_path(state: &AppState, key: &WorkKey) -> PathBuf {
    state
        .repo_root
        .join(&key.owner)
        .join(&key.repo)
        .join(key.kind.dir_name())
        .join(key.number.to_string())
}

fn clone_path(state: &AppState, key: &WorkKey) -> PathBuf {
    state
        .repo_root
        .join(&key.owner)
        .join(&key.repo)
        .join("repo")
}

async fn repo_lock_for(state: &AppState, owner: &str, repo: &str) -> Arc<Mutex<()>> {
    let mut locks = state.repo_locks.lock().await;
    locks
        .entry(RepoKey {
            owner: owner.to_string(),
            repo: repo.to_string(),
        })
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn touch_repo_markers(state: &AppState, key: &WorkKey) -> Result<()> {
    let repo_parent = state.repo_root.join(&key.owner).join(&key.repo);
    tokio::fs::create_dir_all(&repo_parent)
        .await
        .with_context(|| format!("failed to create {}", repo_parent.display()))?;

    let managed = repo_parent.join(REPO_MANAGED_MARKER_FILENAME);
    let _ = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&managed)
        .await;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last_used = repo_parent.join(REPO_LAST_USED_FILENAME);
    tokio::fs::write(&last_used, format!("{now}\n"))
        .await
        .with_context(|| format!("failed to write {}", last_used.display()))?;
    Ok(())
}

async fn ensure_repo_and_worktree(
    state: &AppState,
    item: &WorkItem,
    github: &GithubApi,
    github_token: &str,
    work_dir: &Path,
) -> Result<()> {
    let key = &item.work;
    let repo_dir = clone_path(state, key);
    let repo_lock = repo_lock_for(state, &key.owner, &key.repo).await;
    let _repo_guard = repo_lock.lock().await;

    touch_repo_markers(state, key).await?;
    ensure_clone_with_token(key, &repo_dir, github_token).await?;
    ensure_worktree_with_access(item, github, github_token, &repo_dir, work_dir).await?;
    Ok(())
}

async fn ensure_clone_with_token(key: &WorkKey, repo_dir: &Path, github_token: &str) -> Result<()> {
    if repo_dir.join(".git").exists() {
        run_git(
            repo_dir,
            git_args(&["fetch", "--prune", "origin"]),
            github_token,
        )
        .await?;
        return Ok(());
    }

    let parent = repo_dir.parent().context("repo dir must have parent")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create {}", parent.display()))?;

    let owner = &key.owner;
    let repo = &key.repo;
    let repo_spec = format!("{owner}/{repo}");
    let mut cmd = tokio::process::Command::new("gh");
    cmd.kill_on_drop(true);
    cmd.current_dir(parent)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_TOKEN", github_token)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args(["repo", "clone"])
        .arg(repo_spec)
        .arg(repo_dir)
        .arg("--")
        .arg("--filter=blob:none");
    let gh_result =
        command_output_with_timeout(&mut cmd, parent, git_command_timeout(), "gh repo clone").await;

    match gh_result {
        Ok(TimedCommandOutput::Completed(output)) if output.status.success() => return Ok(()),
        Ok(TimedCommandOutput::Completed(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("gh repo clone failed for {owner}/{repo}: {stderr}");
        }
        Ok(TimedCommandOutput::TimedOut) => {
            eprintln!("gh repo clone timed out in {}", parent.display())
        }
        Err(err) => eprintln!("gh repo clone failed for {owner}/{repo}: {err:#}"),
    }

    let url = github_clone_url(owner, repo);
    let mut args = git_args(&["clone", "--filter=blob:none"]);
    args.push(url);
    args.push(repo_dir.display().to_string());
    run_git(parent, args, github_token)
        .await
        .context("git clone failed")?;

    Ok(())
}

fn github_clone_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}.git")
}

async fn ensure_worktree_with_access(
    item: &WorkItem,
    github: &GithubApi,
    github_token: &str,
    repo_dir: &Path,
    work_dir: &Path,
) -> Result<()> {
    let key = &item.work;
    match tokio::fs::metadata(work_dir).await {
        Ok(meta) => {
            if !meta.is_dir() {
                anyhow::bail!(
                    "existing worktree path is not a directory at {}",
                    work_dir.display()
                );
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat {}", work_dir.display()));
        }
    }

    if work_dir.exists() {
        let dot_git = work_dir.join(".git");
        let dot_git_meta = tokio::fs::metadata(&dot_git)
            .await
            .with_context(|| format!("existing worktree missing {}", dot_git.display()))?;
        if !dot_git_meta.is_file() {
            anyhow::bail!(
                "existing worktree has invalid .git marker at {}",
                dot_git.display()
            );
        }
        let dot_git_contents = tokio::fs::read_to_string(&dot_git)
            .await
            .with_context(|| format!("failed to read {}", dot_git.display()))?;
        let gitdir_raw = dot_git_contents
            .trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .context("worktree .git file missing gitdir")?;
        let gitdir = tokio::fs::canonicalize(gitdir_raw)
            .await
            .with_context(|| format!("failed to resolve gitdir {gitdir_raw}"))?;
        let expected = tokio::fs::canonicalize(repo_dir.join(".git").join("worktrees"))
            .await
            .with_context(|| {
                format!("failed to resolve worktrees dir for {}", repo_dir.display())
            })?;
        if !gitdir.starts_with(&expected) {
            anyhow::bail!(
                "existing worktree gitdir {} is not under {}",
                gitdir.display(),
                expected.display()
            );
        }
        return Ok(());
    }

    let branch = format!("codex/github/{}-{}", key.kind.label(), key.number);

    match key.kind {
        WorkKind::Issue => {
            let default_branch = github.repo_default_branch(&key.owner, &key.repo).await?;
            let base = format!("origin/{default_branch}");
            let mut args = git_args(&["worktree", "add", "-B"]);
            args.push(branch);
            args.push(work_dir.display().to_string());
            args.push(base);
            run_git(repo_dir, args, github_token).await?;
        }
        WorkKind::Pull => {
            let refspec = format!("pull/{}/head:{}", key.number, branch);
            run_git(
                repo_dir,
                vec!["fetch".to_string(), "origin".to_string(), refspec],
                github_token,
            )
            .await?;
            let mut args = git_args(&["worktree", "add"]);
            args.push(work_dir.display().to_string());
            args.push(branch);
            run_git(repo_dir, args, github_token).await?;
        }
        WorkKind::Push => {
            let push_ref = item
                .push_ref
                .as_deref()
                .context("missing push ref for push work item")?;
            run_git(
                repo_dir,
                vec![
                    "fetch".to_string(),
                    "origin".to_string(),
                    push_ref.to_string(),
                ],
                github_token,
            )
            .await?;
            let base = format!("origin/{push_ref}");
            let mut args = git_args(&["worktree", "add", "-B"]);
            args.push(branch);
            args.push(work_dir.display().to_string());
            args.push(base);
            run_git(repo_dir, args, github_token).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
async fn ensure_clone(state: &AppState, key: &WorkKey, repo_dir: &Path) -> Result<()> {
    let github_token = state
        .github_auth
        .static_token()
        .context("test/helper ensure_clone requires a static GitHub token")?;
    ensure_clone_with_token(key, repo_dir, github_token).await
}

#[cfg(test)]
async fn ensure_worktree(
    state: &AppState,
    key: &WorkKey,
    repo_dir: &Path,
    work_dir: &Path,
) -> Result<()> {
    let github_token = state
        .github_auth
        .static_token()
        .context("test/helper ensure_worktree requires a static GitHub token")?;
    let github = GithubApi::new_with_base_url(
        github_token.to_string(),
        state.github_api_base_url.as_ref().clone(),
    )?;
    let item = WorkItem {
        repo_full_name: format!("{}/{}", key.owner, key.repo),
        sender_login: "tester".to_string(),
        source: WebhookSource::Repo,
        installation_id: None,
        work: key.clone(),
        prompt: "test".to_string(),
        display_target: format!("#{}", key.number),
        push_ref: None,
        push_after: None,
        ack_target: AckTarget::None,
        response_target: ResponseTarget::IssueComment {
            issue_number: key.number,
        },
    };
    ensure_worktree_with_access(&item, &github, github_token, repo_dir, work_dir).await
}

fn git_args(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

fn github_git_auth_header(token: &str) -> String {
    let encoded = BASE64_STANDARD.encode(format!("x-access-token:{token}"));
    format!("Authorization: basic {encoded}")
}

fn git_needs_auth(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("clone" | "fetch"))
}

enum TimedCommandOutput {
    Completed(std::process::Output),
    TimedOut,
}

async fn command_output_with_timeout(
    cmd: &mut tokio::process::Command,
    cwd: &Path,
    timeout: Duration,
    label: &str,
) -> Result<TimedCommandOutput> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to run {label} in {}", cwd.display()))?;
    let mut stdout = child.stdout.take().context("child stdout missing")?;
    let mut stderr = child.stderr.take().context("child stderr missing")?;
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => {
            status.with_context(|| format!("failed to run {label} in {}", cwd.display()))?
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Ok(TimedCommandOutput::TimedOut);
        }
    };

    let stdout = stdout_task.await.context("failed to join stdout task")??;
    let stderr = stderr_task.await.context("failed to join stderr task")??;
    Ok(TimedCommandOutput::Completed(std::process::Output {
        status,
        stdout,
        stderr,
    }))
}

async fn run_git(cwd: &Path, args: Vec<String>, github_token: &str) -> Result<()> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.kill_on_drop(true);
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    if git_needs_auth(&args) {
        cmd.arg("-c").arg(format!(
            "http.extraHeader={}",
            github_git_auth_header(github_token)
        ));
    }
    cmd.args(args);
    let output =
        match command_output_with_timeout(&mut cmd, cwd, GIT_COMMAND_TIMEOUT, "git").await? {
            TimedCommandOutput::Completed(output) => output,
            TimedCommandOutput::TimedOut => anyhow::bail!("git timed out in {}", cwd.display()),
        };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git failed: {stderr}");
    }
}

#[derive(Debug)]
struct CodexOutput {
    thread_id: Option<String>,
    last_message: String,
}

struct FetchedGithubContext {
    title: String,
    markdown: String,
}

async fn fetch_github_context_for_item(
    github: &GithubApi,
    item: &WorkItem,
) -> Result<FetchedGithubContext> {
    match item.work.kind {
        WorkKind::Issue => fetch_issue_context(github, &item.work).await,
        WorkKind::Pull => fetch_pull_context(github, &item.work).await,
        WorkKind::Push => fetch_push_context(github, item).await,
    }
}

async fn fetch_push_context(github: &GithubApi, item: &WorkItem) -> Result<FetchedGithubContext> {
    let owner = item.work.owner.as_str();
    let repo = item.work.repo.as_str();
    let after = item
        .push_after
        .as_deref()
        .context("missing push sha for push work item")?;
    let commit_url = format!("{}/repos/{owner}/{repo}/commits/{after}", github.base_url);
    let commit = github.get_json_value(commit_url).await?;
    Ok(format_push_context(item, &commit))
}

#[cfg(test)]
async fn fetch_github_context(github: &GithubApi, key: &WorkKey) -> Result<FetchedGithubContext> {
    match key.kind {
        WorkKind::Issue => fetch_issue_context(github, key).await,
        WorkKind::Pull => fetch_pull_context(github, key).await,
        WorkKind::Push => anyhow::bail!("fetch_github_context requires WorkItem for push events"),
    }
}

async fn fetch_issue_context(github: &GithubApi, key: &WorkKey) -> Result<FetchedGithubContext> {
    let owner = key.owner.as_str();
    let repo = key.repo.as_str();
    let number = key.number;
    let issue_url = format!("{}/repos/{owner}/{repo}/issues/{number}", github.base_url);
    let issue = github.get_json_value(issue_url).await?;

    let comments_url = format!(
        "{}/repos/{owner}/{repo}/issues/{number}/comments",
        github.base_url
    );
    let comments = github.list_paginated(comments_url).await?;

    Ok(format_issue_context(key, &issue, &comments))
}

async fn fetch_pull_context(github: &GithubApi, key: &WorkKey) -> Result<FetchedGithubContext> {
    let owner = key.owner.as_str();
    let repo = key.repo.as_str();
    let number = key.number;

    let pr_url = format!("{}/repos/{owner}/{repo}/pulls/{number}", github.base_url);
    let pr = github.get_json_value(pr_url).await?;

    let issue_comments_url = format!(
        "{}/repos/{owner}/{repo}/issues/{number}/comments",
        github.base_url
    );
    let issue_comments = github.list_paginated(issue_comments_url).await?;

    let review_comments_url = format!(
        "{}/repos/{owner}/{repo}/pulls/{number}/comments",
        github.base_url
    );
    let review_comments = github.list_paginated(review_comments_url).await?;

    let reviews_url = format!(
        "{}/repos/{owner}/{repo}/pulls/{number}/reviews",
        github.base_url
    );
    let reviews = github.list_paginated(reviews_url).await?;

    Ok(format_pull_context(
        key,
        &pr,
        &issue_comments,
        &review_comments,
        &reviews,
    ))
}

fn format_issue_context(key: &WorkKey, issue: &Value, comments: &[Value]) -> FetchedGithubContext {
    let title = json_str(issue, "title").to_string();
    let url = json_str(issue, "html_url");
    let author = json_user_login(issue);
    let state = json_str(issue, "state");
    let created_at = json_str(issue, "created_at");
    let updated_at = json_str(issue, "updated_at");
    let body = json_str(issue, "body");

    let mut out = String::new();
    out.push_str(&format!(
        "# Issue {}/{}#{number}\n\n",
        key.owner,
        key.repo,
        number = key.number
    ));
    if !title.is_empty() {
        out.push_str(&format!("Title: {title}\n"));
    }
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !state.is_empty() {
        out.push_str(&format!("State: {state}\n"));
    }
    if !created_at.is_empty() {
        out.push_str(&format!("Created: {created_at}\n"));
    }
    if !updated_at.is_empty() {
        out.push_str(&format!("Updated: {updated_at}\n"));
    }

    out.push_str("\n## Body\n\n");
    out.push_str(body);
    out.push('\n');

    out.push_str(&format!("\n## Comments ({})\n\n", comments.len()));
    for comment in comments {
        format_issue_comment(&mut out, comment);
    }

    FetchedGithubContext {
        title,
        markdown: out,
    }
}

fn format_pull_context(
    key: &WorkKey,
    pr: &Value,
    issue_comments: &[Value],
    review_comments: &[Value],
    reviews: &[Value],
) -> FetchedGithubContext {
    let title = json_str(pr, "title").to_string();
    let url = json_str(pr, "html_url");
    let author = json_user_login(pr);
    let state = json_str(pr, "state");
    let created_at = json_str(pr, "created_at");
    let updated_at = json_str(pr, "updated_at");
    let body = json_str(pr, "body");
    let base_ref = pr
        .get("base")
        .and_then(|v| v.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let head_ref = pr
        .get("head")
        .and_then(|v| v.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!(
        "# Pull Request {}/{}#{number}\n\n",
        key.owner,
        key.repo,
        number = key.number
    ));
    if !title.is_empty() {
        out.push_str(&format!("Title: {title}\n"));
    }
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !state.is_empty() {
        out.push_str(&format!("State: {state}\n"));
    }
    if !created_at.is_empty() {
        out.push_str(&format!("Created: {created_at}\n"));
    }
    if !updated_at.is_empty() {
        out.push_str(&format!("Updated: {updated_at}\n"));
    }
    if !base_ref.is_empty() || !head_ref.is_empty() {
        out.push_str(&format!("Branch: {head_ref} -> {base_ref}\n"));
    }

    out.push_str("\n## Body\n\n");
    out.push_str(body);
    out.push('\n');

    out.push_str(&format!(
        "\n## Issue comments ({})\n\n",
        issue_comments.len()
    ));
    for comment in issue_comments {
        format_issue_comment(&mut out, comment);
    }

    out.push_str(&format!(
        "\n## Review comments ({})\n\n",
        review_comments.len()
    ));
    for comment in review_comments {
        format_review_comment(&mut out, comment);
    }

    out.push_str(&format!("\n## Reviews ({})\n\n", reviews.len()));
    for review in reviews {
        format_review(&mut out, review);
    }

    FetchedGithubContext {
        title,
        markdown: out,
    }
}

fn format_push_context(item: &WorkItem, commit: &Value) -> FetchedGithubContext {
    let title = commit
        .get("commit")
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let url = json_str(commit, "html_url");
    let author = json_user_login(commit);
    let committed_at = commit
        .get("commit")
        .and_then(|v| v.get("author"))
        .and_then(|v| v.get("date"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = commit
        .get("commit")
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sha = item.push_after.as_deref().unwrap_or_default();
    let ref_name = item.push_ref.as_deref().unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!(
        "# Push {} ref={} sha={}\n\n",
        item.repo_full_name, ref_name, sha
    ));
    if !title.is_empty() {
        out.push_str(&format!("Title: {title}\n"));
    }
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !committed_at.is_empty() {
        out.push_str(&format!("Committed: {committed_at}\n"));
    }

    out.push_str("\n## Commit message\n\n");
    out.push_str(message);
    out.push('\n');

    FetchedGithubContext {
        title,
        markdown: out,
    }
}

fn format_issue_comment(out: &mut String, comment: &Value) {
    let id = comment
        .get("id")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let url = json_str(comment, "html_url");
    let author = json_user_login(comment);
    let created_at = json_str(comment, "created_at");
    let updated_at = json_str(comment, "updated_at");
    let body = json_str(comment, "body");

    out.push_str(&format!("---\nComment id: {id}\n"));
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !created_at.is_empty() {
        out.push_str(&format!("Created: {created_at}\n"));
    }
    if !updated_at.is_empty() {
        out.push_str(&format!("Updated: {updated_at}\n"));
    }
    out.push('\n');
    out.push_str(body);
    out.push_str("\n\n");
}

fn format_review_comment(out: &mut String, comment: &Value) {
    let id = comment
        .get("id")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let url = json_str(comment, "html_url");
    let author = json_user_login(comment);
    let created_at = json_str(comment, "created_at");
    let updated_at = json_str(comment, "updated_at");
    let body = json_str(comment, "body");
    let path = json_str(comment, "path");
    let line = comment
        .get("line")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    out.push_str(&format!("---\nReview comment id: {id}\n"));
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !created_at.is_empty() {
        out.push_str(&format!("Created: {created_at}\n"));
    }
    if !updated_at.is_empty() {
        out.push_str(&format!("Updated: {updated_at}\n"));
    }
    if !path.is_empty() {
        out.push_str(&format!("Path: {path}\n"));
    }
    if line != 0 {
        out.push_str(&format!("Line: {line}\n"));
    }
    out.push('\n');
    out.push_str(body);
    out.push_str("\n\n");
}

fn format_review(out: &mut String, review: &Value) {
    let id = review.get("id").and_then(Value::as_u64).unwrap_or_default();
    let url = json_str(review, "html_url");
    let author = json_user_login(review);
    let state = json_str(review, "state");
    let submitted_at = json_str(review, "submitted_at");
    let body = json_str(review, "body");

    out.push_str(&format!("---\nReview id: {id}\n"));
    if !url.is_empty() {
        out.push_str(&format!("URL: {url}\n"));
    }
    if !author.is_empty() {
        out.push_str(&format!("Author: @{author}\n"));
    }
    if !state.is_empty() {
        out.push_str(&format!("State: {state}\n"));
    }
    if !submitted_at.is_empty() {
        out.push_str(&format!("Submitted: {submitted_at}\n"));
    }
    out.push('\n');
    out.push_str(body);
    out.push_str("\n\n");
}

fn json_str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn json_user_login(v: &Value) -> &str {
    v.get("user")
        .and_then(|u| u.get("login"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

async fn run_codex_in_worktree_with_github(
    state: &AppState,
    item: &WorkItem,
    github: &GithubApi,
    work_dir: &Path,
    codex_config_overrides: &[String],
    log_path: Option<&Path>,
) -> Result<CodexOutput> {
    let thread_id = read_thread_id(state, &item.work).await?;

    let tempdir = tempfile::tempdir().context("failed to create temp dir")?;
    let last_message_path = tempdir.path().join("last_message.txt");
    let context_path = work_dir.join(GITHUB_CONTEXT_FILENAME);
    let (context_title, context_note) = match fetch_github_context_for_item(github, item).await {
        Ok(ctx) => {
            if let Err(err) = tokio::fs::write(&context_path, ctx.markdown).await {
                eprintln!("failed to write {}: {err:#}", context_path.display());
                (
                    ctx.title,
                    "Context: (failed to write context file)
"
                    .to_string(),
                )
            } else {
                (
                    ctx.title,
                    format!(
                        "Context: {GITHUB_CONTEXT_FILENAME}
"
                    ),
                )
            }
        }
        Err(err) => {
            eprintln!("failed to fetch GitHub context: {err:#}");
            let _ = tokio::fs::write(
                &context_path,
                format!(
                    "# GitHub context fetch failed

{err:#}
"
                ),
            )
            .await;
            (
                String::new(),
                format!(
                    "Context: {GITHUB_CONTEXT_FILENAME} (fetch failed)
"
                ),
            )
        }
    };

    let title_line = if context_title.is_empty() {
        String::new()
    } else {
        format!(
            "Title: {context_title}
"
        )
    };
    let subject = match item.work.kind {
        WorkKind::Push => format!("{} {}", item.repo_full_name, item.display_target),
        WorkKind::Issue | WorkKind::Pull => {
            format!("{}{}", item.repo_full_name, item.display_target)
        }
    };
    let prompt = format!(
        "GitHub {kind} event for {subject} from @{sender}.
{title_line}{context_note}
Command:
{command}

Read {GITHUB_CONTEXT_FILENAME} first, then do the command.",
        kind = item.work.kind.label(),
        sender = item.sender_login.as_str(),
        command = item.prompt.as_str()
    );

    let mut cmd = tokio::process::Command::new(state.codex_bin.as_ref());
    for ov in state.codex_config_overrides.iter() {
        cmd.arg("-c").arg(ov);
    }
    for ov in codex_config_overrides {
        cmd.arg("-c").arg(ov);
    }
    cmd.arg("exec")
        .arg("--json")
        .arg("-C")
        .arg(work_dir)
        .arg("-o")
        .arg(&last_message_path)
        .arg("--skip-git-repo-check");

    if let Some(thread_id) = &thread_id {
        cmd.arg("resume").arg(thread_id);
    }

    cmd.arg(prompt);

    cmd.kill_on_drop(true);
    let log_file = if let Some(path) = log_path {
        if let Some(parent) = path.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                eprintln!("failed to create log dir {}: {err:#}", parent.display());
                None
            } else {
                match tokio::fs::File::create(path).await {
                    Ok(file) => Some(Arc::new(Mutex::new(file))),
                    Err(err) => {
                        eprintln!("failed to create log {}: {err:#}", path.display());
                        None
                    }
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(if log_file.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::inherit()
        })
        .spawn()
        .context("failed to spawn codex exec")?;

    let stdout = child.stdout.take().context("missing stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let stderr = child.stderr.take();
    let mut stderr_reader = stderr.map(|stderr| tokio::io::BufReader::new(stderr).lines());
    let mut observed_thread_id: Option<String> = None;
    let mut stdout_closed = false;
    let mut stderr_closed = stderr_reader.is_none();
    let mut exit_status = None;
    let deadline = tokio::time::sleep(CODEX_EXEC_TIMEOUT);
    tokio::pin!(deadline);

    while exit_status.is_none() || !stdout_closed || !stderr_closed {
        tokio::select! {
            _ = &mut deadline => {
                let _ = child.kill().await;
                anyhow::bail!("codex exec timed out");
            }
            line = reader.next_line(), if !stdout_closed => {
                let line = line.context("failed to read codex exec stdout")?;
                match line {
                    Some(line) => {
                        if let Some(file) = log_file.as_ref() {
                            let mut file = file.lock().await;
                            if let Err(err) = file.write_all(format!("[stdout] {line}\n").as_bytes()).await {
                                eprintln!("failed to write codex log stdout: {err:#}");
                            }
                        }
                        if observed_thread_id.is_none()
                            && let Ok(v) = serde_json::from_str::<Value>(&line)
                                && v.get("type").and_then(Value::as_str) == Some("thread.started")
                                && let Some(id) = v.get("thread_id").and_then(Value::as_str)
                            {
                                observed_thread_id = Some(id.to_string());
                            }
                    }
                    None => stdout_closed = true,
                }
            }
            line = async {
                if let Some(stderr_reader) = stderr_reader.as_mut() {
                    stderr_reader.next_line().await
                } else {
                    Ok(None)
                }
            }, if !stderr_closed => {
                let line = line.context("failed to read codex exec stderr")?;
                match line {
                    Some(line) => {
                        if let Some(file) = log_file.as_ref() {
                            let mut file = file.lock().await;
                            if let Err(err) = file.write_all(format!("[stderr] {line}\n").as_bytes()).await {
                                eprintln!("failed to write codex log stderr: {err:#}");
                            }
                        }
                    }
                    None => stderr_closed = true,
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                exit_status = Some(status.context("failed to wait for codex exec")?);
            }
        }
    }

    let status = exit_status.context("missing codex exec exit status")?;
    if !status.success() {
        anyhow::bail!("codex exec failed ({status})");
    }

    let last_message = tokio::fs::read_to_string(&last_message_path)
        .await
        .with_context(|| format!("failed to read {}", last_message_path.display()))?;
    if last_message.trim().is_empty() {
        anyhow::bail!(
            "codex exec did not write output to {}",
            last_message_path.display()
        );
    }
    Ok(CodexOutput {
        thread_id: observed_thread_id,
        last_message: truncate_for_github(&last_message),
    })
}

#[cfg(test)]
async fn run_codex_in_worktree(
    state: &AppState,
    item: &WorkItem,
    work_dir: &Path,
) -> Result<CodexOutput> {
    let access = resolve_github_access(state, item).await?;
    run_codex_in_worktree_with_github(state, item, &access.github, work_dir, &[], None).await
}

fn truncate_for_github(s: &str) -> String {
    const LIMIT: usize = 60_000;
    if s.len() <= LIMIT {
        return s.to_string();
    }
    let mut end = LIMIT;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[truncated]", &s[..end])
}

async fn read_thread_id(state: &AppState, key: &WorkKey) -> Result<Option<String>> {
    let path = thread_id_path(state, key);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to read thread id file"),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

async fn write_thread_id(state: &AppState, key: &WorkKey, thread_id: &str) -> Result<()> {
    let path = thread_id_path(state, key);
    let dir = path.parent().context("thread id path must have parent")?;
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    tokio::fs::write(&path, thread_id)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn thread_id_path(state: &AppState, key: &WorkKey) -> PathBuf {
    state
        .thread_state_dir
        .join(&key.owner)
        .join(&key.repo)
        .join(key.kind.dir_name())
        .join(format!("{}.txt", key.number))
}

async fn post_success_with_github(
    github: &GithubApi,
    item: &WorkItem,
    message: &str,
) -> Result<()> {
    let owner = item.work.owner.as_str();
    let repo = item.work.repo.as_str();
    match item.response_target {
        ResponseTarget::None => {}
        ResponseTarget::IssueComment { issue_number } => {
            github
                .post_issue_comment(owner, repo, issue_number, message)
                .await?;
        }
        ResponseTarget::ReviewCommentReply { comment_id } => {
            github
                .post_review_comment_reply(owner, repo, comment_id, message)
                .await?;
        }
        ResponseTarget::PullRequestReview { pull_number } => {
            github
                .create_pr_review(owner, repo, pull_number, message)
                .await?;
        }
    }
    Ok(())
}

async fn post_ack_with_github(github: &GithubApi, item: &WorkItem) -> Result<()> {
    let owner = item.work.owner.as_str();
    let repo = item.work.repo.as_str();
    match item.ack_target {
        AckTarget::IssueComment { comment_id } => {
            if github
                .post_issue_comment_reaction(owner, repo, comment_id, ACKNOWLEDGMENT_REACTION)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        AckTarget::ReviewComment { comment_id } => {
            if github
                .post_review_comment_reaction(owner, repo, comment_id, ACKNOWLEDGMENT_REACTION)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        AckTarget::None => {}
    }
    post_success_with_github(github, item, ACKNOWLEDGMENT_MESSAGE).await
}

async fn post_ack(state: &AppState, item: &WorkItem) -> Result<()> {
    if matches!(item.response_target, ResponseTarget::None) {
        return Ok(());
    }
    let access = resolve_github_access(state, item).await?;
    post_ack_with_github(&access.github, item).await
}

async fn post_failure(state: &AppState, item: &WorkItem, err: &str) -> Result<()> {
    if matches!(item.response_target, ResponseTarget::None) {
        return Ok(());
    }
    let access = resolve_github_access(state, item).await?;
    let body = truncate_for_github(&format!(
        "codex github failed:

{err}"
    ));
    post_success_with_github(&access.github, item, &body).await
}

#[cfg(test)]
async fn post_success(state: &AppState, item: &WorkItem, message: &str) -> Result<()> {
    if matches!(item.response_target, ResponseTarget::None) {
        return Ok(());
    }
    let access = resolve_github_access(state, item).await?;
    post_success_with_github(&access.github, item, message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::Query;
    use axum::routing::get;
    use axum::routing::post;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::net::TcpListener;

    #[cfg(unix)]
    static ENV_MUTEX: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

    #[cfg(unix)]
    struct PathGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(unix)]
    impl PathGuard {
        fn prepend(dir: &Path) -> Self {
            let lock = ENV_MUTEX
                .get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os("PATH");
            let mut next = std::ffi::OsString::new();
            next.push(dir.as_os_str());
            if let Some(prev) = previous.as_ref() {
                next.push(":");
                next.push(prev);
            }
            // set_var/remove_var are unsafe because mutating the process env is racy; we serialize
            // all env mutations in tests via ENV_MUTEX.
            unsafe {
                std::env::set_var("PATH", next);
            }
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for PathGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous.as_ref() {
                    Some(prev) => std::env::set_var("PATH", prev),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    #[cfg(unix)]
    struct EnvSnapshot {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Vec<(String, Option<std::ffi::OsString>)>,
    }

    #[cfg(unix)]
    impl EnvSnapshot {
        fn set(vars: &[(&str, &str)]) -> Self {
            let lock = ENV_MUTEX
                .get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut previous = Vec::with_capacity(vars.len());
            for (k, v) in vars {
                previous.push(((*k).to_string(), std::env::var_os(k)));
                unsafe {
                    std::env::set_var(k, v);
                }
            }
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, prev) in self.previous.iter() {
                unsafe {
                    match prev {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    fn write_exe(path: &Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, contents).expect("write script");
        let mut perms = std::fs::metadata(path)
            .expect("script metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod");
    }

    fn test_github_auth(token: &str) -> Arc<GithubAuthConfig> {
        Arc::new(GithubAuthConfig {
            mode: GithubWebhookAuthModeToml::Token,
            static_token: Arc::new(token.to_string()),
            app: None,
        })
    }

    fn test_enabled_sources() -> Arc<HashSet<WebhookSource>> {
        Arc::new(HashSet::from([
            WebhookSource::Repo,
            WebhookSource::Organization,
            WebhookSource::GithubApp,
        ]))
    }

    fn test_state(temp: &tempfile::TempDir) -> AppState {
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .expect("create github api");
        AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new(DEFAULT_COMMAND_PREFIX.to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn spawn_test_server(
        listener: TcpListener,
        app: Router,
    ) -> tokio::task::JoinHandle<std::io::Result<()>> {
        tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
            Ok(())
        })
    }

    #[cfg(unix)]
    #[test]
    fn env_snapshot_restores_existing_variable() {
        unsafe {
            std::env::set_var("CODEX_GITHUB_TEST_ENV", "before");
        }
        {
            let _snapshot = EnvSnapshot::set(&[("CODEX_GITHUB_TEST_ENV", "after")]);
            assert_eq!(std::env::var("CODEX_GITHUB_TEST_ENV").unwrap(), "after");
        }
        assert_eq!(std::env::var("CODEX_GITHUB_TEST_ENV").unwrap(), "before");
        unsafe {
            std::env::remove_var("CODEX_GITHUB_TEST_ENV");
        }
    }

    #[cfg(unix)]
    #[test]
    fn path_guard_restores_missing_path() {
        let lock = ENV_MUTEX
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("PATH");
        }
        drop(lock);

        let temp = tempfile::tempdir().unwrap();
        drop(PathGuard::prepend(temp.path()));
        assert_eq!(std::env::var_os("PATH"), None);

        if let Some(previous) = previous {
            let lock = ENV_MUTEX
                .get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            unsafe {
                std::env::set_var("PATH", previous);
            }
            drop(lock);
        }
    }

    #[test]
    fn decode_hex_rejects_odd_length() {
        assert_eq!(decode_hex("0"), None);
    }

    #[test]
    fn decode_hex_rejects_invalid_chars() {
        assert_eq!(decode_hex("xx"), None);
    }

    #[test]
    fn decode_hex_accepts_uppercase() {
        assert_eq!(decode_hex("0A").as_deref(), Some(&[0x0a_u8][..]));
    }

    #[test]
    fn verify_github_signature_accepts_valid_signature() {
        let secret = b"sekrit";
        let body = b"hello world";

        let header = signature_header(secret, body);
        assert!(verify_github_signature(secret, body, Some(&header)));
    }

    #[test]
    fn verify_github_signature_rejects_wrong_prefix() {
        let secret = b"sekrit";
        let body = b"hello world";
        let header = HeaderValue::from_static("sha1=deadbeef");
        assert!(!verify_github_signature(secret, body, Some(&header)));
    }

    #[test]
    fn verify_github_signature_rejects_invalid_hex() {
        let secret = b"sekrit";
        let body = b"hello world";
        let header = HeaderValue::from_static("sha256=not-hex");
        assert!(!verify_github_signature(secret, body, Some(&header)));
    }

    #[test]
    fn github_clone_url_uses_standard_https() {
        assert_eq!(github_clone_url("o", "r"), "https://github.com/o/r.git");
    }

    #[test]
    fn github_git_auth_header_uses_basic_auth() {
        assert_eq!(
            github_git_auth_header("t"),
            "Authorization: basic eC1hY2Nlc3MtdG9rZW46dA=="
        );
    }

    #[test]
    fn ttl_from_days_disables_on_zero() {
        assert_eq!(ttl_from_days(0), None);
        assert_eq!(ttl_from_days(1), Some(Duration::from_secs(24 * 60 * 60)));
    }

    #[test]
    fn min_permission_allows_rejects_unknown_permission() {
        assert_eq!(MinPermission::Read.allows("nope"), false);
        assert_eq!(permission_rank("nope"), None);
    }

    #[test]
    fn work_kind_dir_name_and_label_are_distinct() {
        assert_eq!(WorkKind::Issue.dir_name(), "issues");
        assert_eq!(WorkKind::Pull.dir_name(), "pulls");
        assert_eq!(WorkKind::Issue.label(), "issue");
        assert_eq!(WorkKind::Pull.label(), "pull");
    }

    #[test]
    fn github_api_new_rejects_invalid_token_header_value() {
        let err =
            GithubApi::new_with_base_url("bad\ntoken".to_string(), GITHUB_API_BASE_URL.to_string())
                .err()
                .unwrap();
        assert!(format!("{err:#}").contains("invalid GitHub token"));
    }

    #[tokio::test]
    async fn repo_permission_errors_on_empty_permission() {
        let app = Router::new().route(
            "/repos/o/r/collaborators/u/permission",
            get(|| async { axum::Json(json!({ "permission": "" })) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let err = github.repo_permission("o", "r", "u").await.unwrap_err();
        assert!(format!("{err:#}").contains("empty permission"));

        server.abort();
    }

    #[tokio::test]
    async fn repo_default_branch_errors_on_non_success() {
        let app = Router::new().route(
            "/repos/o/r",
            get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "no") }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let err = github.repo_default_branch("o", "r").await.unwrap_err();
        assert!(format!("{err:#}").contains("repo API failed"));

        server.abort();
    }

    #[tokio::test]
    async fn repo_default_branch_errors_on_empty_default_branch() {
        let app = Router::new().route(
            "/repos/o/r",
            get(|| async { axum::Json(json!({ "default_branch": "" })) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let err = github.repo_default_branch("o", "r").await.unwrap_err();
        assert!(format!("{err:#}").contains("empty default_branch"));

        server.abort();
    }

    #[tokio::test]
    async fn get_json_vec_errors_on_non_success() {
        let app = Router::new().route("/v", get(|| async { (StatusCode::BAD_REQUEST, "no") }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let url = format!("http://{addr}/v");
        let err = github.get_json_vec(url).await.unwrap_err();
        assert!(format!("{err:#}").contains("GitHub API failed"));

        server.abort();
    }

    #[tokio::test]
    async fn list_paginated_errors_when_exceeding_max_pages() {
        let app = Router::new().route(
            "/v",
            get(|| async { axum::Json(vec![Value::Null; GITHUB_API_PER_PAGE]) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let url_base = format!("http://{addr}/v");
        let err = github.list_paginated(url_base).await.unwrap_err();
        assert!(format!("{err:#}").contains("pagination exceeded max pages"));

        server.abort();
    }

    #[tokio::test]
    async fn post_issue_comment_errors_on_non_success() {
        let app = Router::new().route(
            "/repos/o/r/issues/1/comments",
            post(|| async { (StatusCode::BAD_REQUEST, "no") }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let err = github
            .post_issue_comment("o", "r", 1, "hi")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("GitHub API failed"));

        server.abort();
    }

    #[test]
    fn parse_issue_comment_requires_prefix() {
        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 32 },
            "comment": { "body": "hello" }
        });
        let item = parse_work_item("issue_comment", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_issue_comment_ignores_unknown_action() {
        let payload = serde_json::json!({
            "action": "deleted",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 32 },
            "comment": { "body": "/codex hi" }
        });
        let item = parse_work_item("issue_comment", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn command_prefix_requires_boundary() {
        assert_eq!(strip_prefix_prompt("/codexx hi", "/codex").unwrap(), None);
        assert_eq!(
            strip_prefix_prompt(" /codex hi", "/codex").unwrap(),
            Some("hi".to_string())
        );
    }

    #[test]
    fn strip_prefix_prompt_rejects_empty_prompt() {
        assert_eq!(strip_prefix_prompt("/codex   ", "/codex").unwrap(), None);
    }

    #[test]
    fn review_prefix_requires_boundary() {
        assert_eq!(
            strip_prefix_lines("/codexx no\n/codex yes", "/codex"),
            Some("yes".to_string())
        );
    }

    #[test]
    fn strip_prefix_lines_skips_empty_and_returns_none() {
        assert_eq!(strip_prefix_lines("/codex\n/codex   \n", "/codex"), None);
    }

    #[test]
    fn strip_prefix_with_boundary_allows_exact_prefix() {
        assert_eq!(strip_prefix_with_boundary("/codex", "/codex"), Some(""));
    }

    #[test]
    fn parse_issue_comment_on_pr_maps_to_pull_worktree() {
        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 32, "pull_request": { "url": "x" } },
            "comment": { "body": "/codex fix it" }
        });
        let item = parse_work_item("issue_comment", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.work.kind, WorkKind::Pull);
        assert_eq!(item.work.number, 32);
        assert_eq!(item.prompt, "fix it");
    }

    #[test]
    fn parse_review_comment_ignores_unknown_action() {
        let payload = serde_json::json!({
            "action": "deleted",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "comment": { "id": 123, "body": "/codex reply please" }
        });
        let item = parse_work_item("pull_request_review_comment", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_review_comment_requires_prefix() {
        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "comment": { "id": 123, "body": "hello" }
        });
        let item = parse_work_item("pull_request_review_comment", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_review_body_scans_multiple_lines() {
        let payload = serde_json::json!({
            "action": "submitted",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "review": { "body": "hi\n/codex do a\n/codex do b\nbye" }
        });
        let item = parse_work_item("pull_request_review", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.prompt, "do a\ndo b");
    }

    #[test]
    fn parse_review_ignores_unknown_action() {
        let payload = serde_json::json!({
            "action": "dismissed",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "review": { "body": "/codex hi" }
        });
        let item = parse_work_item("pull_request_review", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_review_requires_prefix() {
        let payload = serde_json::json!({
            "action": "submitted",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "review": { "body": "hello" }
        });
        let item = parse_work_item("pull_request_review", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_review_comment_extracts_ids() {
        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "pull_request": { "number": 7 },
            "comment": { "id": 123, "body": "/codex reply please" }
        });
        let item = parse_work_item("pull_request_review_comment", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.work.kind, WorkKind::Pull);
        assert_eq!(item.work.number, 7);
        assert_eq!(item.prompt, "reply please");
        match item.response_target {
            ResponseTarget::ReviewCommentReply { comment_id } => assert_eq!(comment_id, 123),
            other => panic!("unexpected response target: {other:?}"),
        }
    }

    #[test]
    fn webhook_source_detection_prefers_payload_installation_then_header() {
        let payload = serde_json::json!({});
        let headers = HeaderMap::new();
        assert_eq!(
            WebhookSource::from_headers_and_payload(&headers, &payload),
            WebhookSource::Repo
        );

        let mut org_headers = HeaderMap::new();
        org_headers.insert(
            "X-GitHub-Hook-Installation-Target-Type",
            HeaderValue::from_static("organization"),
        );
        assert_eq!(
            WebhookSource::from_headers_and_payload(&org_headers, &payload),
            WebhookSource::Organization
        );

        let mut integration_headers = HeaderMap::new();
        integration_headers.insert(
            "X-GitHub-Hook-Installation-Target-Type",
            HeaderValue::from_static("integration"),
        );
        assert_eq!(
            WebhookSource::from_headers_and_payload(&integration_headers, &payload),
            WebhookSource::GithubApp
        );

        let app_payload = serde_json::json!({
            "installation": { "id": 42 },
            "organization": { "login": "o" }
        });
        assert_eq!(
            WebhookSource::from_headers_and_payload(&org_headers, &app_payload),
            WebhookSource::GithubApp
        );
    }

    #[test]
    fn resolve_enabled_events_keeps_legacy_defaults_without_config() {
        let legacy = resolve_enabled_events(None, false);
        assert_eq!(legacy.issue_comment, true);
        assert_eq!(legacy.issues, false);
        assert_eq!(legacy.pull_request, false);
        assert_eq!(legacy.pull_request_review, true);
        assert_eq!(legacy.pull_request_review_comment, true);
        assert_eq!(legacy.push, false);

        let expanded = resolve_enabled_events(None, true);
        assert_eq!(expanded.issue_comment, true);
        assert_eq!(expanded.issues, true);
        assert_eq!(expanded.pull_request, true);
        assert_eq!(expanded.pull_request_review, true);
        assert_eq!(expanded.pull_request_review_comment, true);
        assert_eq!(expanded.push, true);
    }

    #[test]
    fn parse_issue_event_extracts_prompt_and_installation() {
        let payload = serde_json::json!({
            "action": "opened",
            "installation": { "id": 77 },
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 9, "body": "/codex investigate" }
        });
        let item = parse_work_item("issues", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.source, WebhookSource::GithubApp);
        assert_eq!(item.installation_id, Some(77));
        assert_eq!(
            item.work,
            WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 9,
            }
        );
        assert_eq!(item.prompt, "investigate");
        match item.response_target {
            ResponseTarget::IssueComment { issue_number } => assert_eq!(issue_number, 9),
            other => panic!("unexpected response target: {other:?}"),
        }
    }

    #[test]
    fn parse_pull_request_event_extracts_prompt_from_body() {
        let payload = serde_json::json!({
            "action": "synchronize",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "number": 17,
            "pull_request": {
                "number": 17,
                "body": "/codex run regression"
            }
        });
        let item = parse_work_item("pull_request", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.source, WebhookSource::Repo);
        assert_eq!(
            item.work,
            WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Pull,
                number: 17,
            }
        );
        assert_eq!(item.prompt, "run regression");
        assert_eq!(item.display_target, "#17");
    }

    #[test]
    fn parse_push_event_extracts_branch_and_sha_without_reply_target() {
        let payload = serde_json::json!({
            "repository": { "full_name": "o/r" },
            "organization": { "login": "o" },
            "sender": { "login": "u" },
            "ref": "refs/heads/main",
            "after": "abcdef1234567890",
            "deleted": false,
            "head_commit": { "message": "/codex inspect crash" }
        });
        let item = parse_work_item("push", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.source, WebhookSource::Organization);
        assert_eq!(item.work.kind, WorkKind::Push);
        assert_eq!(item.prompt, "inspect crash");
        assert_eq!(item.display_target, "main@abcdef1");
        assert_eq!(item.push_ref.as_deref(), Some("main"));
        assert_eq!(item.push_after.as_deref(), Some("abcdef1234567890"));
        assert!(matches!(item.response_target, ResponseTarget::None));
    }

    #[test]
    fn parse_push_event_ignores_deleted_push() {
        let payload = serde_json::json!({
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "ref": "refs/heads/main",
            "after": "abcdef1234567890",
            "deleted": true,
            "head_commit": { "message": "/codex inspect crash" }
        });
        let item = parse_work_item("unknown_event", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn parse_issue_event_uses_issue_body_prefix() {
        let payload = serde_json::json!({
            "action": "opened",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 11, "body": "/codex investigate" }
        });
        let item = parse_work_item("issues", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.work.kind, WorkKind::Issue);
        assert_eq!(item.work.number, 11);
        assert_eq!(item.prompt, "investigate");
        assert_eq!(item.display_target, "#11");
        match item.response_target {
            ResponseTarget::IssueComment { issue_number } => assert_eq!(issue_number, 11),
            other => panic!("unexpected response target: {other:?}"),
        }
    }

    #[test]
    fn parse_pull_request_event_uses_pr_body_prefix() {
        let payload = serde_json::json!({
            "action": "edited",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "number": 23,
            "pull_request": { "number": 23, "body": "/codex refresh this" }
        });
        let item = parse_work_item("pull_request", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.work.kind, WorkKind::Pull);
        assert_eq!(item.work.number, 23);
        assert_eq!(item.prompt, "refresh this");
        assert_eq!(item.display_target, "#23");
        match item.response_target {
            ResponseTarget::IssueComment { issue_number } => assert_eq!(issue_number, 23),
            other => panic!("unexpected response target: {other:?}"),
        }
    }

    #[test]
    fn parse_push_event_extracts_branch_and_disables_reply_target() {
        let payload = serde_json::json!({
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "ref": "refs/heads/main",
            "after": "0123456789abcdef0123456789abcdef01234567",
            "head_commit": { "message": "/codex inspect push" }
        });
        let item = parse_work_item("push", &payload, "/codex")
            .unwrap()
            .unwrap();
        assert_eq!(item.work.kind, WorkKind::Push);
        assert_eq!(item.prompt, "inspect push");
        assert_eq!(item.push_ref.as_deref(), Some("main"));
        assert_eq!(
            item.push_after.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert!(matches!(item.response_target, ResponseTarget::None));
    }

    #[test]
    fn parse_work_item_ignores_unknown_event_without_repo_fields() {
        let payload = serde_json::json!({ "some": "thing" });
        let item = parse_work_item("unknown_event", &payload, "/codex").unwrap();
        assert_eq!(item.is_none(), true);
    }

    #[test]
    fn min_permission_ranking_is_monotonic() {
        assert_eq!(MinPermission::Write.allows("read"), false);
        assert_eq!(MinPermission::Write.allows("write"), true);
        assert_eq!(MinPermission::Write.allows("maintain"), true);
    }

    #[test]
    fn min_permission_allows_covers_variants() {
        assert_eq!(MinPermission::Read.allows("read"), true);
        assert_eq!(MinPermission::Maintain.allows("admin"), true);
        assert_eq!(MinPermission::Admin.allows("admin"), true);
    }

    #[test]
    fn repo_allowed_normalizes_case() {
        let allow = normalize_allowlist(&["O/R".to_string()]);
        assert_eq!(repo_allowed(&allow, "o/r"), true);
        assert_eq!(repo_allowed(&allow, "O/R"), true);
        assert_eq!(repo_allowed(&allow, "x/y"), false);
    }

    #[test]
    fn split_owner_repo_rejects_unsafe_components() {
        assert!(split_owner_repo("../x").is_err());
        assert!(split_owner_repo("x/..").is_err());
        assert!(split_owner_repo("x/y").is_ok());
    }

    #[tokio::test]
    async fn claim_delivery_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(claim_delivery(dir.path(), "abc").await.unwrap(), true);
        assert_eq!(claim_delivery(dir.path(), "abc").await.unwrap(), false);
    }

    #[tokio::test]
    async fn claim_delivery_errors_when_marker_path_is_directory() {
        let dir = tempfile::tempdir().unwrap();
        let marker_root = dir.path().join("markers");
        tokio::fs::write(&marker_root, "not-a-dir").await.unwrap();
        let err = claim_delivery(&marker_root, "x").await.unwrap_err();
        assert!(format!("{err:#}").contains("delivery markers dir"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claim_delivery_surfaces_marker_create_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let marker_root = dir.path().join("markers");
        tokio::fs::create_dir_all(&marker_root).await.unwrap();
        let mut perms = std::fs::metadata(&marker_root).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&marker_root, perms).unwrap();

        let err = claim_delivery(&marker_root, "x").await.unwrap_err();
        assert!(format!("{err:#}").contains("failed to create delivery marker file"));
    }

    #[tokio::test]
    async fn resolve_github_access_token_mode_uses_configured_base_url() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new("http://example.test".to_string()),
            github_auth: Arc::new(GithubAuthConfig {
                mode: GithubWebhookAuthModeToml::Token,
                static_token: Arc::new("static-token".to_string()),
                app: None,
            }),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "hi".to_string(),
            display_target: "#1".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let access = resolve_github_access(&state, &item).await.unwrap();
        assert_eq!(access.token, "static-token");
        assert_eq!(access.github.base_url, "http://example.test");
    }

    #[tokio::test]
    async fn resolve_github_access_auto_prefers_github_app_before_static_token() {
        let app = Router::new()
            .route(
                "/repos/o/r/installation",
                get(|| async { (StatusCode::OK, axum::Json(serde_json::json!({ "id": 42 }))) }),
            )
            .route(
                "/app/installations/42/access_tokens",
                post(|| async {
                    (
                        StatusCode::CREATED,
                        axum::Json(serde_json::json!({ "token": "installation-token" })),
                    )
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(format!("http://{addr}")),
            github_auth: Arc::new(GithubAuthConfig {
                mode: GithubWebhookAuthModeToml::Auto,
                static_token: Arc::new("static-token".to_string()),
                app: Some(Arc::new(GithubAppCredentials {
                    app_id: 123,
                    private_key: Arc::new(
                        r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCwUHhCZWtS4iDx
G5g0CPK46rHMmY/qQj/urDzJ1tAp3bkRWrCBZRBmZgqnJXw47aX87BpiI/8TIsch
U3qMijsGeMgV6K/YPACkb5NHyUEdRr0xrje50HE6R7WwBNRqx+Xe+y5dn9UFuk+t
oSxvSOHHIP+Eq9V/pOG/sr9Tf8w1qPt7S4d9I/dfMtGjiJienJiYjZ8iF5f40LwO
1SIM6fygQzXmUky3qIqBmgXpX1LC90R0AG//SoN542GkA+IW/2DGTz1dQalKGuWi
cwo+JTP8Af9KDn7xntnUyNmDsGPhbE3CgsypQa7AstFiW9JLjOgZoa/axCKQ/nNd
uG8kwWdhAgMBAAECggEADNQ8HeOsip481KJkAaRFigY8u/UQA1UA+yRB85NLeAJj
vxMk3PNuPS8vVwBroKZNIcFJvaqeGqW5DNAazotX1mWXNH3m/qJGDzD1q99lWySc
q1bgg+caeAnD+vvs+1ySebY35lTVDprDBbCgr7PDRuacJQqOSUCFcxoF4STfRmen
a+Z+XKIWfKnEzDAR92+ZOkgJS0kkNP/9TONeOvWTiOvxFfAf80xNBWffH+GrVB5r
CeWE8DJOWC4o74FlblZ4gVvDWbGAJX/pyBSlGoHSbHh+vGzWnNwwHWnFwfYAzlap
3M2Lk6k5UHo2lKaCumI1q6vanqYGFi7R1nS31dun+QKBgQDlRR+JQmCFYNDPTQdH
4sQP+TGHuiXGNQba0lNqhO6op8AeSFcrTq+OXKSoplDOTO9BvrcRlCQsDgLqYASG
LptTqAhhbUULEX8oXCsobCFmfqPbSVRAdjctirh+3TY0xaRF+Lsnzrgp/pWYyL33
+8ULS4Ff+5Hc20T+PX2qrBRPiQKBgQDE3s/ggi37XyDU9PmE/rufZdRhXZm15SSO
d/I9S3MhpFKyDZDXekJEVPU6fOEFrU5crfEx0nxLYijko2xtvQ87WzUZO9gKdIsp
pKlgT/m6KT3/4KyNdE8pKP5k7oVc47kmKyeT0x6MH1Cxx4goTp3OAhWj2qAhlSuD
6h6bnRTLGQKBgQCm3Bzsl7uJtwGhrez7m4WYHoO2xXqSe6tGfMarAp5zbss6/uk6
IqVQVgqcl5a93m5PCg9QouGEkpn6m/EO+0Kequ+WgKE8QfqqlBHw9GmGn+p/QSop
VCAqbAiEhFjcJW++YR1NBn0wSxHzRT5FCh7JbqV1BrGM7KSU6InaOiz6CQKBgQCr
cFjYaqT+RS4DJT3xCh97RKL5ExibJOt7wYpKxFyDTGTTNysN6iKw/Mb84ujWF8Co
xrTGrUSeJOH1kTcILV6JUvjfe5S8LhdN8V2qSJrw+Z9LJ208Va/l6RP38xph9NE0
Itp5SZ1NaqvL1TWF3EhhsMEFiopuFEfrvUJgQx9raQKBgHxjxj5xAHKiMWToavRN
KagatZeUpmvfZ8Ov6vZ9Y0Fks32cMykAbxidS/+nIEEwz3Xzk19NM/ujYgwhpml1
L0TEzz1ofIhomRKm+ThdWBno4czkXsSdPHDtc3RPxbdfP2ZYIgZBJEKdCj97bv64
gM6+LiULCYzYqcuiuKsJk6lL
-----END PRIVATE KEY-----"#
                            .to_string(),
                    ),
                })),
            }),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "hi".to_string(),
            display_target: "#1".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let access = resolve_github_access(&state, &item).await.unwrap();
        assert_eq!(access.token, "installation-token");
        assert_eq!(access.github.base_url, format!("http://{addr}"));

        server.abort();
    }

    #[tokio::test]
    async fn repo_lock_for_reuses_lock_per_repo() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let a = repo_lock_for(&state, "o", "r").await;
        let b = repo_lock_for(&state, "o", "r").await;
        let c = repo_lock_for(&state, "o", "x").await;
        assert_eq!(Arc::ptr_eq(&a, &b), true);
        assert_eq!(Arc::ptr_eq(&a, &c), false);
    }

    #[tokio::test]
    async fn touch_repo_markers_creates_files() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        touch_repo_markers(&state, &key).await.unwrap();
        let repo_parent = state.repo_root.join("o").join("r");
        assert_eq!(
            repo_parent.join(REPO_MANAGED_MARKER_FILENAME).exists(),
            true
        );
        let last_used = tokio::fs::read_to_string(repo_parent.join(REPO_LAST_USED_FILENAME))
            .await
            .unwrap();
        assert!(last_used.trim().parse::<u64>().is_ok());
    }

    #[tokio::test]
    async fn read_repo_last_used_parses_or_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();

        assert_eq!(read_repo_last_used(&repo_dir).await.unwrap(), None);

        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "\n")
            .await
            .unwrap();
        assert_eq!(read_repo_last_used(&repo_dir).await.unwrap(), None);

        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "nope\n")
            .await
            .unwrap();
        assert_eq!(read_repo_last_used(&repo_dir).await.unwrap(), None);

        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "123\n")
            .await
            .unwrap();
        assert_eq!(read_repo_last_used(&repo_dir).await.unwrap(), Some(123));
    }

    #[tokio::test]
    async fn dir_is_empty_treats_missing_as_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            dir_is_empty(&temp.path().join("missing")).await.unwrap(),
            true
        );

        let dir = temp.path().join("d");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        assert_eq!(dir_is_empty(&dir).await.unwrap(), true);

        tokio::fs::write(dir.join("x"), "1").await.unwrap();
        assert_eq!(dir_is_empty(&dir).await.unwrap(), false);
    }

    #[tokio::test]
    async fn gc_delivery_markers_skips_non_markers_and_deletes_markers() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(temp.path()).await.unwrap();
        tokio::fs::write(temp.path().join("a.marker"), "x")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("b.txt"), "x")
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp.path().join("c.marker"))
            .await
            .unwrap();

        gc_delivery_markers(temp.path(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(temp.path().join("a.marker").exists(), false);
        assert_eq!(temp.path().join("b.txt").exists(), true);
        assert_eq!(temp.path().join("c.marker").exists(), true);
    }

    #[tokio::test]
    async fn gc_delivery_markers_returns_ok_when_dir_missing() {
        let temp = tempfile::tempdir().unwrap();
        gc_delivery_markers(&temp.path().join("missing"), Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn gc_delivery_markers_errors_when_path_is_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("f");
        tokio::fs::write(&file, "x").await.unwrap();
        let err = gc_delivery_markers(&file, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to read"));
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_deletes_managed_repo_without_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "0\n")
            .await
            .unwrap();

        gc_repo_cache_if_stale(&state, "o", "r", Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(repo_dir.exists(), false);
    }

    #[tokio::test]
    async fn gc_repo_cache_skips_when_worktrees_present() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(repo_dir.join("issues").join("1"))
            .await
            .unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "0\n")
            .await
            .unwrap();

        gc_repo_cache_if_stale(&state, "o", "r", Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_caches_scans_repo_root() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        tokio::fs::create_dir_all(state.repo_root.as_ref())
            .await
            .unwrap();
        tokio::fs::write(state.repo_root.join("junk"), "x")
            .await
            .unwrap();

        let stale = state.repo_root.join("o").join("stale");
        tokio::fs::create_dir_all(&stale).await.unwrap();
        tokio::fs::write(stale.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(stale.join(REPO_LAST_USED_FILENAME), "0\n")
            .await
            .unwrap();

        let unmanaged = state.repo_root.join("o").join("keep");
        tokio::fs::create_dir_all(&unmanaged).await.unwrap();
        tokio::fs::write(unmanaged.join(REPO_LAST_USED_FILENAME), "0\n")
            .await
            .unwrap();

        gc_repo_caches(&state, Duration::ZERO).await.unwrap();
        assert_eq!(stale.exists(), false);
        assert_eq!(unmanaged.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_caches_skips_repo_files() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let good_owner = state.repo_root.join("good");
        tokio::fs::create_dir_all(&good_owner).await.unwrap();
        tokio::fs::write(good_owner.join("repo-file"), "x")
            .await
            .unwrap();

        gc_repo_caches(&state, Duration::ZERO).await.unwrap();
        assert_eq!(good_owner.join("repo-file").exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_caches_returns_ok_when_root_missing() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state(&temp);
        state.repo_root = Arc::new(temp.path().join("missing"));
        gc_repo_caches(&state, Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn gc_repo_caches_errors_when_root_is_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state(&temp);
        let root = temp.path().join("root-file");
        tokio::fs::write(&root, "x").await.unwrap();
        state.repo_root = Arc::new(root);
        let err = gc_repo_caches(&state, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to read"));
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_returns_ok_when_last_used_missing() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();

        gc_repo_cache_if_stale(&state, "o", "r", Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_returns_ok_when_last_used_is_future() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            format!("{}\n", now + 60),
        )
        .await
        .unwrap();

        gc_repo_cache_if_stale(&state, "o", "r", Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_returns_ok_when_recent() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), format!("{now}\n"))
            .await
            .unwrap();

        gc_repo_cache_if_stale(&state, "o", "r", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn dir_is_empty_errors_when_path_is_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("f");
        tokio::fs::write(&file, "x").await.unwrap();
        let err = dir_is_empty(&file).await.unwrap_err();
        assert!(format!("{err:#}").contains("failed to read"));
    }

    #[tokio::test]
    async fn read_repo_last_used_errors_when_path_is_directory() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(REPO_LAST_USED_FILENAME))
            .await
            .unwrap();
        let err = read_repo_last_used(&repo_dir).await.unwrap_err();
        assert!(format!("{err:#}").contains("failed to read"));
    }

    #[tokio::test]
    async fn gc_loop_runs_once_then_sleeps() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state(&temp);
        state.delivery_ttl = Some(Duration::ZERO);
        state.repo_ttl = Some(Duration::ZERO);
        tokio::fs::create_dir_all(state.delivery_markers_dir.as_ref())
            .await
            .unwrap();
        tokio::fs::write(state.delivery_markers_dir.join("d.marker"), "x")
            .await
            .unwrap();
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(repo_dir.join(REPO_LAST_USED_FILENAME), "0\n")
            .await
            .unwrap();

        let _ = tokio::time::timeout(Duration::from_millis(50), gc_loop(state)).await;
        assert_eq!(
            temp.path().join("deliveries").join("d.marker").exists(),
            false
        );
        assert_eq!(
            temp.path().join("repos").join("o").join("r").exists(),
            false
        );
    }

    #[tokio::test]
    async fn gc_delivery_markers_skips_recent_marker() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("a.marker"), "x")
            .await
            .unwrap();

        gc_delivery_markers(temp.path(), Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(temp.path().join("a.marker").exists(), true);
    }

    #[tokio::test]
    async fn gc_loop_logs_gc_errors_and_keeps_running() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state(&temp);
        let deliveries = temp.path().join("deliveries-file");
        let repos = temp.path().join("repos-file");
        tokio::fs::write(&deliveries, "x").await.unwrap();
        tokio::fs::write(&repos, "x").await.unwrap();
        state.delivery_markers_dir = Arc::new(deliveries);
        state.repo_root = Arc::new(repos);
        state.delivery_ttl = Some(Duration::ZERO);
        state.repo_ttl = Some(Duration::ZERO);

        let _ = tokio::time::timeout(Duration::from_millis(50), gc_loop(state)).await;
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_rechecks_marker_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            "0
",
        )
        .await
        .unwrap();

        let repo_lock = repo_lock_for(&state, "o", "r").await;
        let guard = repo_lock.lock().await;
        let state_for_task = state.clone();
        let task = tokio::spawn(async move {
            gc_repo_cache_if_stale(&state_for_task, "o", "r", Duration::ZERO).await
        });
        tokio::task::yield_now().await;
        tokio::fs::remove_file(repo_dir.join(REPO_MANAGED_MARKER_FILENAME))
            .await
            .unwrap();
        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_rechecks_last_used_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            "0
",
        )
        .await
        .unwrap();

        let repo_lock = repo_lock_for(&state, "o", "r").await;
        let guard = repo_lock.lock().await;
        let state_for_task = state.clone();
        let task = tokio::spawn(async move {
            gc_repo_cache_if_stale(&state_for_task, "o", "r", Duration::ZERO).await
        });
        tokio::task::yield_now().await;
        tokio::fs::remove_file(repo_dir.join(REPO_LAST_USED_FILENAME))
            .await
            .unwrap();
        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_rechecks_future_last_used_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            "0
",
        )
        .await
        .unwrap();

        let repo_lock = repo_lock_for(&state, "o", "r").await;
        let guard = repo_lock.lock().await;
        let state_for_task = state.clone();
        let task = tokio::spawn(async move {
            gc_repo_cache_if_stale(&state_for_task, "o", "r", Duration::ZERO).await
        });
        tokio::task::yield_now().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            format!(
                "{}
",
                now + 60
            ),
        )
        .await
        .unwrap();
        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_rechecks_recent_last_used_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            "0
",
        )
        .await
        .unwrap();

        let repo_lock = repo_lock_for(&state, "o", "r").await;
        let guard = repo_lock.lock().await;
        let state_for_task = state.clone();
        let task = tokio::spawn(async move {
            gc_repo_cache_if_stale(&state_for_task, "o", "r", Duration::from_secs(60)).await
        });
        tokio::task::yield_now().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            format!(
                "{now}
"
            ),
        )
        .await
        .unwrap();
        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn gc_repo_cache_if_stale_rechecks_worktrees_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = state.repo_root.join("o").join("r");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::write(repo_dir.join(REPO_MANAGED_MARKER_FILENAME), "")
            .await
            .unwrap();
        tokio::fs::write(
            repo_dir.join(REPO_LAST_USED_FILENAME),
            "0
",
        )
        .await
        .unwrap();

        let repo_lock = repo_lock_for(&state, "o", "r").await;
        let guard = repo_lock.lock().await;
        let state_for_task = state.clone();
        let task = tokio::spawn(async move {
            gc_repo_cache_if_stale(&state_for_task, "o", "r", Duration::ZERO).await
        });
        tokio::task::yield_now().await;
        tokio::fs::create_dir_all(repo_dir.join("issues").join("1"))
            .await
            .unwrap();
        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repo_dir.exists(), true);
    }

    #[tokio::test]
    async fn handle_webhook_rejects_payload_too_large() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let body = vec![0_u8; MAX_WEBHOOK_BYTES + 1];
        let res = handle_webhook_inner(state, HeaderMap::new(), Bytes::from(body)).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn handle_webhook_requires_event_header() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let res = handle_webhook_inner(state, HeaderMap::new(), Bytes::from_static(b"{}")).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_webhook_requires_delivery_header() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        let res = handle_webhook_inner(state, headers, Bytes::from_static(b"{}")).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_webhook_rejects_missing_signature() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        let res = handle_webhook_inner(state, headers, Bytes::from_static(b"{}")).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn handle_webhook_rejects_invalid_json() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let body = Bytes::from_static(b"not-json");
        let header = signature_header(b"sekrit", body.as_ref());

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state, headers, body).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_webhook_rejects_invalid_payload() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let payload = json!({
            "action": "created",
            "repository": { "full_name": "invalid" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = Bytes::from(serde_json::to_vec(&payload).unwrap());
        let header = signature_header(b"sekrit", body.as_ref());

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state, headers, body).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_webhook_ignores_when_comment_missing_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(&temp));
        let payload = json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "body": "hello" }
        });
        let body = Bytes::from(serde_json::to_vec(&payload).unwrap());
        let header = signature_header(b"sekrit", body.as_ref());

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state, headers, body).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn handle_webhook_ignores_when_repo_not_allowed() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state(&temp);
        state.allow_repos = Arc::new(HashSet::from([normalize_repo_full_name("x/y")]));
        let state = Arc::new(state);

        let payload = json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "o" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = Bytes::from(serde_json::to_vec(&payload).unwrap());
        let header = signature_header(b"sekrit", body.as_ref());

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state, headers, body).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn handle_webhook_returns_500_when_permission_api_errors() {
        let app = Router::new().route(
            "/repos/o/r/collaborators/u/permission",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(2)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        });

        let payload = json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let header = signature_header(b"sekrit", &body);

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state.clone(), headers, Bytes::from(body)).await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        server.abort();
    }

    #[tokio::test]
    async fn handle_webhook_returns_500_when_delivery_claim_fails() {
        let temp = tempfile::tempdir().unwrap();
        let delivery_markers_dir = temp.path().join("deliveries");
        tokio::fs::write(&delivery_markers_dir, "not a dir")
            .await
            .unwrap();

        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(delivery_markers_dir),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(2)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let payload = json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "o" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let header = signature_header(b"sekrit", &body);

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(Arc::new(state), headers, Bytes::from(body)).await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn handle_webhook_returns_busy_before_permission_check() {
        let posted_body = Arc::new(Mutex::new(String::new()));
        let app = Router::new().route(
            "/repos/o/r/issues/1/comments",
            post({
                let posted_body = Arc::clone(&posted_body);
                move |axum::Json(v): axum::Json<Value>| {
                    let posted_body = Arc::clone(&posted_body);
                    async move {
                        *posted_body.lock().await = v
                            .get("body")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        StatusCode::CREATED
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);
        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(0)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let header = signature_header(b"sekrit", &body);

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(Arc::new(state), headers, Bytes::from(body)).await;
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(posted_body.lock().await.contains("busy"));

        server.abort();
    }

    #[tokio::test]
    async fn handle_webhook_ignores_when_sender_is_not_collaborator() {
        let app = Router::new().route(
            "/repos/o/r/collaborators/u/permission",
            get(|| async { StatusCode::NOT_FOUND }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(2)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let payload = serde_json::json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "body": "/codex hi" }
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let header = signature_header(b"sekrit", &body);

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(Arc::new(state), headers, Bytes::from(body)).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED);

        server.abort();
    }

    #[tokio::test]
    async fn sender_allowed_allows_repo_owner_without_permission_api() {
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "o".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "hi".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        assert_eq!(sender_allowed(&state, &item).await.unwrap(), true);
    }

    #[tokio::test]
    async fn sender_allowed_rejects_when_permission_too_low() {
        let app = Router::new().route(
            "/repos/o/r/collaborators/u/permission",
            get(|| async {
                (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({ "permission": "read" })),
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "hi".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        assert_eq!(sender_allowed(&state, &item).await.unwrap(), false);

        server.abort();
    }

    #[tokio::test]
    async fn fetch_issue_context_fetches_title_body_and_all_comments() {
        let app = Router::new()
            .route(
                "/repos/o/r/issues/1",
                get(|| async {
                    axum::Json(json!({
                        "title": "Issue title",
                        "body": "Issue body",
                        "html_url": "https://example.invalid/o/r/issues/1",
                        "user": { "login": "alice" },
                        "state": "open",
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z"
                    }))
                }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    let page: usize = q
                        .get("page")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(1);
                    let per_page: usize = q
                        .get("per_page")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or_default();
                    assert_eq!(per_page, GITHUB_API_PER_PAGE);

                    let (start, count) = if page == 1 {
                        (0, GITHUB_API_PER_PAGE)
                    } else if page == 2 {
                        (GITHUB_API_PER_PAGE, GITHUB_API_PER_PAGE)
                    } else {
                        (GITHUB_API_PER_PAGE * 2, 1)
                    };
                    let comments: Vec<Value> = (0..count)
                        .map(|i| {
                            let id = (start + i) as u64;
                            json!({
                                "id": id,
                                "html_url": format!("https://example.invalid/o/r/issues/1#issuecomment-{id}"),
                                "user": { "login": format!("u{id}") },
                                "created_at": "2026-03-05T00:00:00Z",
                                "updated_at": "2026-03-05T00:00:00Z",
                                "body": format!("comment-body-{id}"),
                            })
                        })
                        .collect();
                    axum::Json(comments)
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let ctx = fetch_issue_context(&github, &key).await.unwrap();
        assert_eq!(ctx.title, "Issue title");
        assert!(ctx.markdown.contains("Issue body"));
        assert!(ctx.markdown.contains("comment-body-0"));
        assert!(
            ctx.markdown
                .contains(&format!("comment-body-{GITHUB_API_PER_PAGE}"))
        );

        server.abort();
    }

    #[tokio::test]
    async fn fetch_pull_context_includes_body_and_comment_types() {
        let app = Router::new()
            .route(
                "/repos/o/r/pulls/7",
                get(|| async {
                    axum::Json(json!({
                        "title": "PR title",
                        "body": "PR body",
                        "html_url": "https://example.invalid/o/r/pull/7",
                        "user": { "login": "alice" },
                        "state": "open",
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z",
                        "base": { "ref": "main" },
                        "head": { "ref": "feature" }
                    }))
                }),
            )
            .route(
                "/repos/o/r/issues/7/comments",
                get(|| async {
                    axum::Json(vec![json!({
                        "id": 1,
                        "html_url": "https://example.invalid/o/r/pull/7#issuecomment-1",
                        "user": { "login": "bob" },
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z",
                        "body": "issue comment body"
                    })])
                }),
            )
            .route(
                "/repos/o/r/pulls/7/comments",
                get(|| async {
                    axum::Json(vec![json!({
                        "id": 2,
                        "html_url": "https://example.invalid/o/r/pull/7#discussion_r2",
                        "user": { "login": "carol" },
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z",
                        "path": "src/lib.rs",
                        "line": 10,
                        "body": "review comment body"
                    })])
                }),
            )
            .route(
                "/repos/o/r/pulls/7/reviews",
                get(|| async {
                    axum::Json(vec![json!({
                        "id": 3,
                        "html_url": "https://example.invalid/o/r/pull/7#pullrequestreview-3",
                        "user": { "login": "dave" },
                        "state": "APPROVED",
                        "submitted_at": "2026-03-05T00:00:00Z",
                        "body": "review body"
                    })])
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Pull,
            number: 7,
        };
        let ctx = fetch_github_context(&github, &key).await.unwrap();
        assert_eq!(ctx.title, "PR title");
        assert!(ctx.markdown.contains("PR body"));
        assert!(ctx.markdown.contains("issue comment body"));
        assert!(ctx.markdown.contains("review comment body"));
        assert!(ctx.markdown.contains("Path: src/lib.rs"));
        assert!(ctx.markdown.contains("review body"));
        assert!(ctx.markdown.contains("State: APPROVED"));

        server.abort();
    }

    #[test]
    fn format_context_skips_empty_metadata_fields() {
        let issue_key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let issue = json!({ "body": "issue body" });
        let issue_ctx = format_issue_context(&issue_key, &issue, &[]);
        assert_eq!(issue_ctx.title, "");
        assert_eq!(issue_ctx.markdown.contains("Title:"), false);

        let pr_key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Pull,
            number: 7,
        };
        let pr = json!({ "body": "pr body", "base": {}, "head": {} });
        let pr_ctx = format_pull_context(&pr_key, &pr, &[], &[], &[]);
        assert_eq!(pr_ctx.title, "");
        assert_eq!(pr_ctx.markdown.contains("Title:"), false);
        assert_eq!(pr_ctx.markdown.contains("Branch:"), false);

        let mut out = String::new();
        format_issue_comment(&mut out, &json!({ "id": 1, "body": "c" }));
        assert_eq!(out.contains("URL:"), false);
        assert_eq!(out.contains("Author:"), false);

        out.clear();
        format_review_comment(&mut out, &json!({ "id": 2, "body": "c" }));
        assert_eq!(out.contains("URL:"), false);
        assert_eq!(out.contains("Path:"), false);
        assert_eq!(out.contains("Line:"), false);

        out.clear();
        format_review(&mut out, &json!({ "id": 3, "body": "c" }));
        assert_eq!(out.contains("URL:"), false);
        assert_eq!(out.contains("State:"), false);
    }

    #[test]
    fn sanitize_filename_component_replaces_unsafe_chars_and_handles_empty() {
        assert_eq!(sanitize_filename_component(""), "_");
        assert_eq!(sanitize_filename_component("a/b"), "a_b");
    }

    #[tokio::test]
    async fn ensure_worktree_accepts_existing_valid_marker() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        let git_worktrees = repo_dir.join(".git").join("worktrees");
        let gitdir = git_worktrees.join("wt");
        tokio::fs::create_dir_all(&gitdir).await.unwrap();

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::write(
            work_dir.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .await
        .unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ensure_worktree_rejects_directory_git_marker() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(".git").join("worktrees"))
            .await
            .unwrap();

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(work_dir.join(".git"))
            .await
            .unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("invalid .git marker"));
    }

    #[tokio::test]
    async fn ensure_worktree_rejects_git_marker_missing_gitdir() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(".git").join("worktrees"))
            .await
            .unwrap();

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::write(work_dir.join(".git"), "nope\n")
            .await
            .unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("missing gitdir"));
    }

    #[tokio::test]
    async fn ensure_worktree_rejects_gitdir_outside_expected() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(".git").join("worktrees"))
            .await
            .unwrap();

        let other = temp.path().join("other");
        tokio::fs::create_dir_all(&other).await.unwrap();

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::write(
            work_dir.join(".git"),
            format!("gitdir: {}\n", other.display()),
        )
        .await
        .unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("is not under"));
    }

    #[tokio::test]
    async fn ensure_worktree_rejects_non_directory_work_dir() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        let work_dir = temp.path().join("work");
        tokio::fs::write(&work_dir, "x").await.unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("not a directory"));
    }

    #[tokio::test]
    async fn ensure_worktree_errors_when_repo_worktrees_dir_missing() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let repo_dir = temp.path().join("repo");
        let work_dir = temp.path().join("work");
        let gitdir = temp.path().join("gitdir");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::create_dir_all(&gitdir).await.unwrap();
        tokio::fs::write(
            work_dir.join(".git"),
            format!(
                "gitdir: {}
",
                gitdir.display()
            ),
        )
        .await
        .unwrap();

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to resolve worktrees dir"));
    }

    #[tokio::test]
    async fn ensure_worktree_issue_errors_when_default_branch_api_fails() {
        let app = Router::new().route(
            "/repos/o/r",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        let work_dir = temp.path().join("work");

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("repo API failed"));

        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_worktree_pull_fetches_and_creates_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        let work_dir = temp.path().join("work");
        let log = temp.path().join("git.log");

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\necho \"$*\" >> \"{log}\"\ncase \" $* \" in\n  *\" worktree add \"*) mkdir -p \"{work_dir}\" ;;\nesac\n",
                log = log.display(),
                work_dir = work_dir.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Pull,
            number: 7,
        };
        ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap();

        assert_eq!(work_dir.exists(), true);
        let logged = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(logged.contains("fetch origin pull/7/head:codex/github/pull-7"));
        assert!(logged.contains("worktree add"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_worktree_pull_errors_when_fetch_fails() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        let work_dir = temp.path().join("work");

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(bin_dir.join("git").as_path(), "#!/bin/sh\nexit 1\n");
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Pull,
            number: 7,
        };
        let err = ensure_worktree(&state, &key, &repo_dir, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("git failed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_prefers_gh_when_available() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        let parent = repo_dir.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("gh").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\nmkdir -p \"{repo_dir}/.git\"\nexit 0\n",
                repo_dir = repo_dir.display()
            ),
        );
        write_exe(bin_dir.join("git").as_path(), "#!/bin/sh\nexit 1\n");
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        ensure_clone(&state, &key, &repo_dir).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_fetches_when_repo_exists() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(".git"))
            .await
            .unwrap();
        let log = temp.path().join("git.log");

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\necho \"$*\" >> \"{log}\"\nexit 0\n",
                log = log.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        ensure_clone(&state, &key, &repo_dir).await.unwrap();
        let logged = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(logged.contains("fetch --prune origin"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_errors_when_fetch_fails() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        tokio::fs::create_dir_all(repo_dir.join(".git"))
            .await
            .unwrap();

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("git").as_path(),
            "#!/bin/sh\nset -eu\necho \"no\" 1>&2\nexit 1\n",
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let err = ensure_clone(&state, &key, &repo_dir).await.unwrap_err();
        assert!(format!("{err:#}").contains("git failed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_falls_back_when_gh_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        let parent = repo_dir.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\ncase \" $* \" in\n  *\" clone \"*) /bin/mkdir -p \"{repo_dir}/.git\" ;;\nesac\n",
                repo_dir = repo_dir.display()
            ),
        );
        let _env = EnvSnapshot::set(&[("PATH", bin_dir.to_str().unwrap())]);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        ensure_clone(&state, &key, &repo_dir).await.unwrap();
        assert_eq!(repo_dir.join(".git").exists(), true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_falls_back_when_gh_is_not_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);

        let repo_dir = temp.path().join("repo");
        let parent = repo_dir.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let gh_path = bin_dir.join("gh");
        std::fs::write(&gh_path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&gh_path, perms).unwrap();

        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\ncase \" $* \" in\n  *\" clone \"*) mkdir -p \"{repo_dir}/.git\" ;;\nesac\n",
                repo_dir = repo_dir.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        ensure_clone(&state, &key, &repo_dir).await.unwrap();
        assert_eq!(repo_dir.join(".git").exists(), true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_clone_times_out_then_falls_back() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let _timeout_guard = TestGitCommandTimeoutGuard::set(Duration::from_millis(100));

        let repo_dir = temp.path().join("repo");
        let parent = repo_dir.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(bin_dir.join("gh").as_path(), "#!/bin/sh\nexec sleep 5\n");
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\ncase \" $* \" in\n  *\" clone \"*) mkdir -p \"{repo_dir}/.git\" ;;\nesac\n",
                repo_dir = repo_dir.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };

        ensure_clone(&state, &key, &repo_dir).await.unwrap();
        assert_eq!(repo_dir.join(".git").exists(), true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_git_surfaces_command_failure() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("err.txt");
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\necho \"no\" > \"{log}\"\necho \"no\" 1>&2\nexit 1\n",
                log = log.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let err = run_git(temp.path(), vec!["status".to_string()], "t")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("git failed"));
    }

    #[tokio::test]
    async fn post_success_supports_all_response_targets() {
        let issue_body = Arc::new(tokio::sync::Mutex::new(String::new()));
        let reply_body = Arc::new(tokio::sync::Mutex::new(String::new()));
        let review_payload = Arc::new(tokio::sync::Mutex::new(Value::Null));

        let app = {
            let issue_body = Arc::clone(&issue_body);
            let reply_body = Arc::clone(&reply_body);
            let review_payload = Arc::clone(&review_payload);
            Router::new()
                .route(
                    "/repos/o/r/issues/1/comments",
                    post(move |axum::Json(v): axum::Json<Value>| {
                        let issue_body = Arc::clone(&issue_body);
                        async move {
                            *issue_body.lock().await = v
                                .get("body")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            StatusCode::CREATED
                        }
                    }),
                )
                .route(
                    "/repos/o/r/pulls/comments/123/replies",
                    post(move |axum::Json(v): axum::Json<Value>| {
                        let reply_body = Arc::clone(&reply_body);
                        async move {
                            *reply_body.lock().await = v
                                .get("body")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            StatusCode::CREATED
                        }
                    }),
                )
                .route(
                    "/repos/o/r/pulls/7/reviews",
                    post(move |axum::Json(v): axum::Json<Value>| {
                        let review_payload = Arc::clone(&review_payload);
                        async move {
                            *review_payload.lock().await = v;
                            StatusCode::CREATED
                        }
                    }),
                )
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let issue_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };
        post_success(&state, &issue_item, "m1").await.unwrap();
        assert_eq!(issue_body.lock().await.as_str(), "m1");

        let reply_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Pull,
                number: 7,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::ReviewCommentReply { comment_id: 123 },
        };
        post_success(&state, &reply_item, "m2").await.unwrap();
        assert_eq!(reply_body.lock().await.as_str(), "m2");

        let review_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Pull,
                number: 7,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::PullRequestReview { pull_number: 7 },
        };
        post_success(&state, &review_item, "m3").await.unwrap();

        let posted = review_payload.lock().await.clone();
        assert_eq!(posted.get("body").and_then(Value::as_str), Some("m3"));
        assert_eq!(posted.get("event").and_then(Value::as_str), Some("COMMENT"));

        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_work_item_posts_failure_comment() {
        let posted_body = Arc::new(Mutex::new(String::new()));
        let app = Router::new().route(
            "/repos/o/r/issues/1/comments",
            post({
                let posted_body = Arc::clone(&posted_body);
                move |axum::Json(v): axum::Json<Value>| {
                    let posted_body = Arc::clone(&posted_body);
                    async move {
                        *posted_body.lock().await = v
                            .get("body")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        StatusCode::CREATED
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repos");
        let repo_dir = repo_root.join("o").join("r").join("repo");
        let work_dir = repo_root.join("o").join("r").join("issues").join("1");

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(bin_dir.join("gh").as_path(), "#!/bin/sh\nexit 1\n");
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\nmkdir -p \"{repo_dir}\"\ncase \" $* \" in\n  *\" clone \"*) mkdir -p \"{repo_dir}/.git\" ;;\n  *\" worktree add \"*) mkdir -p \"{work_dir}\" ; printf 'gitdir: {repo_dir}/.git/worktrees/issue-1\\n' > \"{work_dir}/.git\" ;;\nesac\n",
                repo_dir = repo_dir.display(),
                work_dir = work_dir.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(repo_root),
            codex_bin: Arc::new(temp.path().join("missing-codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do the thing".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let permit = state
            .concurrency_limit
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        process_work_item(state, item, permit).await;
        assert!(posted_body.lock().await.contains("codex github failed"));

        server.abort();
    }

    #[tokio::test]
    async fn post_ack_reacts_to_issue_comment_when_available() {
        let reaction_calls = Arc::new(AtomicUsize::new(0));
        let comment_calls = Arc::new(AtomicUsize::new(0));
        let reaction_body = Arc::new(Mutex::new(String::new()));
        let app = Router::new()
            .route(
                "/repos/o/r/issues/comments/99/reactions",
                post({
                    let reaction_calls = Arc::clone(&reaction_calls);
                    let reaction_body = Arc::clone(&reaction_body);
                    move |axum::Json(v): axum::Json<Value>| {
                        let reaction_calls = Arc::clone(&reaction_calls);
                        let reaction_body = Arc::clone(&reaction_body);
                        async move {
                            reaction_calls.fetch_add(1, Ordering::SeqCst);
                            *reaction_body.lock().await = v
                                .get("content")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            StatusCode::CREATED
                        }
                    }
                }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                post({
                    let comment_calls = Arc::clone(&comment_calls);
                    move || {
                        let comment_calls = Arc::clone(&comment_calls);
                        async move {
                            comment_calls.fetch_add(1, Ordering::SeqCst);
                            StatusCode::CREATED
                        }
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::IssueComment { comment_id: 99 },
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        post_ack(&state, &item).await.unwrap();
        assert_eq!(reaction_calls.load(Ordering::SeqCst), 1);
        assert_eq!(comment_calls.load(Ordering::SeqCst), 0);
        assert_eq!(reaction_body.lock().await.as_str(), ACKNOWLEDGMENT_REACTION);

        server.abort();
    }

    #[tokio::test]
    async fn post_ack_falls_back_to_comment_when_reaction_fails() {
        let comment_body = Arc::new(Mutex::new(String::new()));
        let app = Router::new()
            .route(
                "/repos/o/r/issues/comments/99/reactions",
                post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "no") }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                post({
                    let comment_body = Arc::clone(&comment_body);
                    move |axum::Json(v): axum::Json<Value>| {
                        let comment_body = Arc::clone(&comment_body);
                        async move {
                            *comment_body.lock().await = v
                                .get("body")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            StatusCode::CREATED
                        }
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::IssueComment { comment_id: 99 },
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        post_ack(&state, &item).await.unwrap();
        assert_eq!(comment_body.lock().await.as_str(), ACKNOWLEDGMENT_MESSAGE);

        server.abort();
    }

    #[tokio::test]
    async fn post_success_errors_when_github_api_fails() {
        let app = Router::new()
            .route(
                "/repos/o/r/issues/1/comments",
                post(|| async { (StatusCode::BAD_REQUEST, "no") }),
            )
            .route(
                "/repos/o/r/pulls/comments/123/replies",
                post(|| async { (StatusCode::BAD_REQUEST, "no") }),
            )
            .route(
                "/repos/o/r/pulls/7/reviews",
                post(|| async { (StatusCode::BAD_REQUEST, "no") }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(PathBuf::from("codex")),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let issue_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };
        assert!(post_success(&state, &issue_item, "m1").await.is_err());

        let reply_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Pull,
                number: 7,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::ReviewCommentReply { comment_id: 123 },
        };
        assert!(post_success(&state, &reply_item, "m2").await.is_err());

        let review_item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "u".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Pull,
                number: 7,
            },
            prompt: "x".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::PullRequestReview { pull_number: 7 },
        };
        assert!(post_success(&state, &review_item, "m3").await.is_err());

        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_writes_context_file() {
        use std::os::unix::fs::PermissionsExt;

        let app = Router::new()
            .route(
                "/repos/o/r/issues/1",
                get(|| async {
                    axum::Json(json!({
                        "title": "Issue title",
                        "body": "Issue body",
                        "html_url": "https://example.invalid/o/r/issues/1",
                        "user": { "login": "alice" },
                        "state": "open",
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z"
                    }))
                }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                get(|| async { axum::Json(Vec::<Value>::new()) }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let prompt_log = temp.path().join("prompt.txt");
        let codex_path = temp.path().join("codex");
        std::fs::write(
            &codex_path,
            format!(
                "#!/bin/sh\nset -eu\nlast=\"\"\nfor arg in \"$@\"; do last=\"$arg\"; done\necho \"$last\" > \"{}\"\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"{{\\\"type\\\":\\\"thread.started\\\",\\\"thread_id\\\":\\\"thr-1\\\"}}\"\nprintf %s \"ok\" > \"$out\"\n",
                prompt_log.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&codex_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&codex_path, perms).unwrap();

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do the thing".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let output = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap();
        assert_eq!(output.thread_id.as_deref(), Some("thr-1"));
        assert_eq!(output.last_message, "ok");

        let context = tokio::fs::read_to_string(work_dir.join(GITHUB_CONTEXT_FILENAME))
            .await
            .unwrap();
        assert!(context.contains("Issue title"));
        assert!(context.contains("Issue body"));

        let prompt = tokio::fs::read_to_string(&prompt_log).await.unwrap();
        assert!(prompt.contains("Title: Issue title"));
        assert!(prompt.contains(&format!("Context: {GITHUB_CONTEXT_FILENAME}")));
        assert!(prompt.contains("do the thing"));

        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_notes_context_write_failure_and_passes_overrides() {
        let app = Router::new()
            .route(
                "/repos/o/r/issues/1",
                get(|| async {
                    axum::Json(json!({
                        "title": "Issue title",
                        "body": "Issue body",
                        "html_url": "https://example.invalid/o/r/issues/1",
                        "user": { "login": "alice" },
                        "state": "open",
                        "created_at": "2026-03-05T00:00:00Z",
                        "updated_at": "2026-03-05T00:00:00Z"
                    }))
                }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                get(|| async { axum::Json(Vec::<Value>::new()) }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();

        let args_log = temp.path().join("args.txt");
        let codex_path = temp.path().join("codex");
        write_exe(
            &codex_path,
            &format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > \"{args_log}\"\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"{{\\\"type\\\":\\\"thread.started\\\",\\\"thread_id\\\":\\\"thr-1\\\"}}\"\necho \"hello\"\nprintf %s \"ok\" > \"$out\"\n",
                args_log = args_log.display()
            ),
        );

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::create_dir_all(work_dir.join(GITHUB_CONTEXT_FILENAME))
            .await
            .unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(vec!["k=v".to_string()]),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do the thing".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let output = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap();
        assert_eq!(output.last_message, "ok");

        let args = tokio::fs::read_to_string(&args_log).await.unwrap();
        assert!(args.contains("-c\nk=v\n"));
        assert!(args.contains("failed to write context file"));

        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_errors_on_empty_output_file() {
        let app = Router::new()
            .route(
                "/repos/o/r/issues/1",
                get(|| async { axum::Json(json!({ "title": "", "body": "" })) }),
            )
            .route(
                "/repos/o/r/issues/1/comments",
                get(|| async { axum::Json(Vec::<Value>::new()) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();

        let codex_path = temp.path().join("codex");
        write_exe(
            &codex_path,
            "#!/bin/sh\nset -eu\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"{\\\"type\\\":\\\"thread.started\\\",\\\"thread_id\\\":\\\"thr-1\\\"}\"\n: > \"$out\"\n",
        );

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let err = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("did not write output"));

        server.abort();
    }

    #[test]
    fn truncate_for_github_truncates_and_preserves_utf8() {
        const LIMIT: usize = 60_000;
        let mut s = "a".repeat(LIMIT - 1);
        s.push('🦀');
        s.push_str("tail");
        let out = truncate_for_github(&s);
        assert!(out.contains("[truncated]"));
        assert!(out.is_char_boundary(out.len()));
        assert!(out.len() <= LIMIT + 20);
    }

    #[tokio::test]
    async fn read_thread_id_returns_none_when_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let path = thread_id_path(&state, &key);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, "\n").await.unwrap();
        assert_eq!(read_thread_id(&state, &key).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_thread_id_errors_when_path_is_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(&temp);
        let key = WorkKey {
            owner: "o".to_string(),
            repo: "r".to_string(),
            kind: WorkKind::Issue,
            number: 1,
        };
        let path = thread_id_path(&state, &key);
        tokio::fs::create_dir_all(&path).await.unwrap();
        let err = read_thread_id(&state, &key).await.unwrap_err();
        assert!(format!("{err:#}").contains("thread id file"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_handles_context_fetch_failure_and_resume() {
        let app = Router::new().route(
            "/repos/o/r/issues/1",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();

        let args_log = temp.path().join("args.txt");
        let codex_path = temp.path().join("codex");
        write_exe(
            &codex_path,
            &format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > \"{args_log}\"\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"{{\\\"type\\\":\\\"thread.started\\\",\\\"thread_id\\\":\\\"thr-1\\\"}}\"\nprintf %s \"ok\" > \"$out\"\n",
                args_log = args_log.display()
            ),
        );

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        write_thread_id(&state, &item.work, "thr-old")
            .await
            .unwrap();
        let output = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap();
        assert_eq!(output.thread_id.as_deref(), Some("thr-1"));
        assert_eq!(output.last_message, "ok");

        let context = tokio::fs::read_to_string(work_dir.join(GITHUB_CONTEXT_FILENAME))
            .await
            .unwrap();
        assert!(context.contains("context fetch failed"));

        let args = tokio::fs::read_to_string(&args_log).await.unwrap();
        assert!(args.contains("resume"));
        assert!(args.contains("thr-old"));
        assert!(args.contains("fetch failed"));

        server.abort();
    }

    #[tokio::test]
    async fn run_codex_in_worktree_errors_when_codex_spawn_fails() {
        let temp = tempfile::tempdir().unwrap();
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();
        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let missing = temp.path().join("missing-codex");
        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(missing),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let err = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to spawn codex exec"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_errors_when_codex_exits_nonzero() {
        let temp = tempfile::tempdir().unwrap();
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();

        let codex_path = temp.path().join("codex");
        write_exe(&codex_path, "#!/bin/sh\nexit 1\n");
        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let err = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("codex exec failed"));
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn run_codex_in_worktree_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();

        let codex_path = temp.path().join("codex");
        write_exe(&codex_path, "#!/bin/sh\nexec tail -f /dev/null\n");
        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let handle =
            tokio::spawn(async move { run_codex_in_worktree(&state, &item, &work_dir).await });
        tokio::task::yield_now().await;
        tokio::time::advance(CODEX_EXEC_TIMEOUT + Duration::from_millis(1)).await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_codex_in_worktree_ignores_non_thread_started_stdout() {
        let github =
            GithubApi::new_with_base_url("t".to_string(), "http://example.invalid".to_string())
                .unwrap();
        let temp = tempfile::tempdir().unwrap();

        let codex_path = temp.path().join("codex");
        write_exe(
            &codex_path,
            "#!/bin/sh\nset -eu\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"hello\"\nprintf %s \"ok\" > \"$out\"\n",
        );

        let work_dir = temp.path().join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let state = AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(temp.path().join("repos")),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(1)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        let item = WorkItem {
            repo_full_name: "o/r".to_string(),
            sender_login: "bob".to_string(),
            work: WorkKey {
                owner: "o".to_string(),
                repo: "r".to_string(),
                kind: WorkKind::Issue,
                number: 1,
            },
            prompt: "do it".to_string(),
            source: WebhookSource::Repo,
            installation_id: None,
            display_target: "test".to_string(),
            push_ref: None,
            push_after: None,
            ack_target: AckTarget::None,
            response_target: ResponseTarget::IssueComment { issue_number: 1 },
        };

        let output = run_codex_in_worktree(&state, &item, &work_dir)
            .await
            .unwrap();
        assert_eq!(output.thread_id, None);
        assert_eq!(output.last_message, "ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn handle_webhook_processes_issue_comment_end_to_end() {
        let post_calls = Arc::new(AtomicUsize::new(0));
        let reaction_calls = Arc::new(AtomicUsize::new(0));
        let posted_body = Arc::new(tokio::sync::Mutex::new(String::new()));
        let app = {
            let post_calls = Arc::clone(&post_calls);
            let reaction_calls = Arc::clone(&reaction_calls);
            let posted_body = Arc::clone(&posted_body);
            Router::new()
                .route(
                    "/repos/o/r/collaborators/u/permission",
                    get(|| async { axum::Json(json!({ "permission": "admin" })) }),
                )
                .route(
                    "/repos/o/r",
                    get(|| async { axum::Json(json!({ "default_branch": "master" })) }),
                )
                .route(
                    "/repos/o/r/issues/1",
                    get(|| async {
                        axum::Json(json!({
                            "title": "Issue title",
                            "body": "Issue body",
                            "html_url": "https://example.invalid/o/r/issues/1",
                            "user": { "login": "alice" },
                            "state": "open",
                            "created_at": "2026-03-05T00:00:00Z",
                            "updated_at": "2026-03-05T00:00:00Z"
                        }))
                    }),
                )
                .route(
                    "/repos/o/r/issues/1/comments",
                    get(|| async { axum::Json(Vec::<Value>::new()) }).post(
                        move |axum::Json(v): axum::Json<Value>| {
                            let post_calls = Arc::clone(&post_calls);
                            let posted_body = Arc::clone(&posted_body);
                            async move {
                                post_calls.fetch_add(1, Ordering::SeqCst);
                                let body = v
                                    .get("body")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                *posted_body.lock().await = body;
                                StatusCode::CREATED
                            }
                        },
                    ),
                )
                .route(
                    "/repos/o/r/issues/comments/99/reactions",
                    post(move |axum::Json(v): axum::Json<Value>| {
                        let reaction_calls = Arc::clone(&reaction_calls);
                        async move {
                            reaction_calls.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(
                                v.get("content").and_then(Value::as_str),
                                Some(ACKNOWLEDGMENT_REACTION)
                            );
                            StatusCode::CREATED
                        }
                    }),
                )
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_test_server(listener, app);

        let github =
            GithubApi::new_with_base_url("t".to_string(), format!("http://{addr}")).unwrap();
        let temp = tempfile::tempdir().unwrap();

        let repo_root = temp.path().join("repos");
        let repo_dir = repo_root.join("o").join("r").join("repo");
        let work_dir = repo_root.join("o").join("r").join("issues").join("1");

        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        write_exe(bin_dir.join("gh").as_path(), "#!/bin/sh\nexit 1\n");
        write_exe(
            bin_dir.join("git").as_path(),
            &format!(
                "#!/bin/sh\nset -eu\nmkdir -p \"{repo_dir}\"\ncase \" $* \" in\n  *\" clone \"*) mkdir -p \"{repo_dir}/.git\" ;;\n  *\" worktree add \"*) mkdir -p \"{work_dir}\" ;;\nesac\n",
                repo_dir = repo_dir.display(),
                work_dir = work_dir.display()
            ),
        );
        let _path_guard = PathGuard::prepend(&bin_dir);

        let codex_path = temp.path().join("codex");
        write_exe(
            &codex_path,
            "#!/bin/sh\nset -eu\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n  shift\ndone\necho \"{\\\"type\\\":\\\"thread.started\\\",\\\"thread_id\\\":\\\"thr-1\\\"}\"\nprintf %s \"ok\" > \"$out\"\n",
        );

        let state = Arc::new(AppState {
            secret: Arc::new(b"sekrit".to_vec()),
            github_api_base_url: Arc::new(github.base_url.clone()),
            github_auth: test_github_auth("t"),
            allow_repos: Arc::new(HashSet::new()),
            enabled_sources: test_enabled_sources(),
            enabled_events: EnabledEvents::expanded_default(),
            min_permission: MinPermission::Triage,
            command_prefix: Arc::new("/codex".to_string()),
            repo_root: Arc::new(repo_root),
            codex_bin: Arc::new(codex_path),
            codex_config_overrides: Arc::new(Vec::new()),
            delivery_markers_dir: Arc::new(temp.path().join("deliveries")),
            thread_state_dir: Arc::new(temp.path().join("threads")),
            delivery_ttl: None,
            repo_ttl: None,
            concurrency_limit: Arc::new(Semaphore::new(2)),
            work_locks: Arc::new(Mutex::new(HashMap::new())),
            repo_locks: Arc::new(Mutex::new(HashMap::new())),
        });

        let payload = json!({
            "action": "created",
            "repository": { "full_name": "o/r" },
            "sender": { "login": "u" },
            "issue": { "number": 1 },
            "comment": { "id": 99, "body": "/codex do the thing" }
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let header = signature_header(b"sekrit", &body);

        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);

        let res = handle_webhook_inner(state.clone(), headers, Bytes::from(body.clone())).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED);

        let header = signature_header(b"sekrit", &body);
        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", HeaderValue::from_static("issue_comment"));
        headers.insert("X-GitHub-Delivery", HeaderValue::from_static("d1"));
        headers.insert("X-Hub-Signature-256", header);
        let dup = handle_webhook_inner(state.clone(), headers, Bytes::from(body)).await;
        assert_eq!(dup.status(), StatusCode::ACCEPTED);

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if post_calls.load(Ordering::SeqCst) == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(post_calls.load(Ordering::SeqCst), 1);
        assert_eq!(reaction_calls.load(Ordering::SeqCst), 1);
        assert_eq!(posted_body.lock().await.as_str(), "ok");

        let context = tokio::fs::read_to_string(work_dir.join(GITHUB_CONTEXT_FILENAME))
            .await
            .unwrap();
        assert!(context.contains("Issue title"));
        assert!(context.contains("Issue body"));

        let thread_id_path = state
            .thread_state_dir
            .join("o")
            .join("r")
            .join("issues")
            .join("1.txt");
        assert_eq!(
            tokio::fs::read_to_string(&thread_id_path)
                .await
                .unwrap()
                .trim(),
            "thr-1"
        );

        let marker_path = state.delivery_markers_dir.join("d1.marker");
        assert_eq!(marker_path.exists(), true);

        server.abort();
    }

    fn signature_header(secret: &[u8], body: &[u8]) -> HeaderValue {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        let sig = mac.finalize().into_bytes();
        let header = format!("sha256={}", bytes_to_lower_hex(&sig));
        HeaderValue::from_str(&header).unwrap()
    }

    fn bytes_to_lower_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(hex_digit((b >> 4) & 0xf));
            out.push(hex_digit(b & 0xf));
        }
        out
    }

    fn hex_digit(n: u8) -> char {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        char::from(HEX[n as usize])
    }
}
