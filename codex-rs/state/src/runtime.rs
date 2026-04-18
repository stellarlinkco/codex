use crate::AgentJob;
use crate::AgentJobCreateParams;
use crate::AgentJobItem;
use crate::AgentJobItemCreateParams;
use crate::AgentJobItemStatus;
use crate::AgentJobProgress;
use crate::AgentJobStatus;
use crate::LOGS_DB_FILENAME;
use crate::LOGS_DB_VERSION;
use crate::LogEntry;
use crate::LogQuery;
use crate::LogRow;
use crate::STATE_DB_FILENAME;
use crate::STATE_DB_VERSION;
use crate::SortKey;
use crate::ThreadMetadata;
use crate::ThreadMetadataBuilder;
use crate::ThreadsPage;
use crate::apply_rollout_item;
use crate::migrations::LOGS_MIGRATOR;
use crate::migrations::STATE_MIGRATOR;
use crate::model::AgentJobRow;
use crate::model::ThreadRow;
use crate::model::anchor_from_item;
use crate::model::datetime_to_epoch_seconds;
use crate::paths::file_modified_time_utc;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::protocol::RolloutItem;
use log::LevelFilter;
use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::QueryBuilder;
use sqlx::Row;
use sqlx::Sqlite;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use sqlx::migrate::MigrateError;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tracing::warn;

mod agent_jobs;
mod backfill;
mod logs;
mod memories;
#[cfg(test)]
mod test_support;
mod threads;

// "Partition" is the retention bucket we cap at 10 MiB:
// - one bucket per non-null thread_id
// - one bucket per threadless (thread_id IS NULL) non-null process_uuid
// - one bucket for threadless rows with process_uuid IS NULL
const LOG_PARTITION_SIZE_LIMIT_BYTES: i64 = 10 * 1024 * 1024;
const LOG_PARTITION_ROW_LIMIT: i64 = 1_000;

#[derive(Clone)]
pub struct StateRuntime {
    codex_home: PathBuf,
    default_provider: String,
    pool: Arc<sqlx::SqlitePool>,
    logs_pool: Arc<sqlx::SqlitePool>,
}

impl StateRuntime {
    /// Initialize the state runtime using the provided Codex home and default provider.
    ///
    /// This opens (and migrates) the SQLite databases under `codex_home`,
    /// keeping logs in a dedicated file to reduce lock contention with the
    /// rest of the state store.
    pub async fn init(codex_home: PathBuf, default_provider: String) -> anyhow::Result<Arc<Self>> {
        tokio::fs::create_dir_all(&codex_home).await?;
        let current_state_name = state_db_filename();
        let current_logs_name = logs_db_filename();
        remove_legacy_db_files(
            &codex_home,
            current_state_name.as_str(),
            STATE_DB_FILENAME,
            "state",
        )
        .await;
        remove_legacy_db_files(
            &codex_home,
            current_logs_name.as_str(),
            LOGS_DB_FILENAME,
            "logs",
        )
        .await;
        let state_path = state_db_path(codex_home.as_path());
        let logs_path = logs_db_path(codex_home.as_path());
        let pool = match open_sqlite(&state_path, &STATE_MIGRATOR, "state").await {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open state db at {}: {err}", state_path.display());
                return Err(err);
            }
        };
        let logs_pool = match open_sqlite(&logs_path, &LOGS_MIGRATOR, "logs").await {
            Ok(db) => Arc::new(db),
            Err(err) => {
                warn!("failed to open logs db at {}: {err}", logs_path.display());
                return Err(err);
            }
        };
        let runtime = Arc::new(Self {
            pool,
            logs_pool,
            codex_home,
            default_provider,
        });
        Ok(runtime)
    }

    /// Return the configured Codex home directory for this runtime.
    pub fn codex_home(&self) -> &Path {
        self.codex_home.as_path()
    }
}

async fn open_sqlite(
    path: &Path,
    migrator: &'static Migrator,
    db_label: &str,
) -> anyhow::Result<SqlitePool> {
    let pool = connect_sqlite(path).await?;
    match migrator.run(&pool).await {
        Ok(()) => Ok(pool),
        Err(MigrateError::VersionMissing(_) | MigrateError::VersionMismatch(_)) => {
            pool.close().await;
            quarantine_incompatible_db_files(path, db_label).await?;
            let rebuilt_pool = connect_sqlite(path).await?;
            migrator.run(&rebuilt_pool).await?;
            warn!(
                "recreated incompatible {db_label} db at {} after migration drift",
                path.display()
            );
            Ok(rebuilt_pool)
        }
        Err(err) => Err(err.into()),
    }
}

async fn connect_sqlite(path: &Path) -> anyhow::Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(sqlite_connect_options(path))
        .await?;
    Ok(pool)
}

fn sqlite_connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .log_statements(LevelFilter::Off)
}

async fn quarantine_incompatible_db_files(path: &Path, db_label: &str) -> anyhow::Result<()> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        anyhow::bail!("invalid {db_label} db path: {}", path.display());
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let source = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            path.with_file_name(format!("{file_name}{suffix}"))
        };
        if !tokio::fs::try_exists(&source).await.unwrap_or(false) {
            continue;
        }
        let Some(source_name) = source.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let target = source.with_file_name(format!("{source_name}.incompatible-{stamp}"));
        tokio::fs::rename(&source, &target).await?;
        warn!(
            "moved incompatible {db_label} db file {} to {}",
            source.display(),
            target.display()
        );
    }
    Ok(())
}

fn db_filename(base_name: &str, version: u32) -> String {
    format!("{base_name}_{version}.sqlite")
}

pub fn state_db_filename() -> String {
    db_filename(STATE_DB_FILENAME, STATE_DB_VERSION)
}

pub fn state_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(state_db_filename())
}

pub fn logs_db_filename() -> String {
    db_filename(LOGS_DB_FILENAME, LOGS_DB_VERSION)
}

pub fn logs_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(logs_db_filename())
}

async fn remove_legacy_db_files(
    codex_home: &Path,
    current_name: &str,
    base_name: &str,
    db_label: &str,
) {
    let mut entries = match tokio::fs::read_dir(codex_home).await {
        Ok(entries) => entries,
        Err(err) => {
            warn!(
                "failed to read codex_home for {db_label} db cleanup {}: {err}",
                codex_home.display(),
            );
            return;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_type()
            .await
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !should_remove_db_file(file_name.as_ref(), current_name, base_name) {
            continue;
        }

        let legacy_path = entry.path();
        if let Err(err) = tokio::fs::remove_file(&legacy_path).await {
            warn!(
                "failed to remove legacy {db_label} db file {}: {err}",
                legacy_path.display(),
            );
        }
    }
}

fn should_remove_db_file(file_name: &str, current_name: &str, base_name: &str) -> bool {
    let mut normalized_name = file_name;
    for suffix in ["-wal", "-shm", "-journal"] {
        if let Some(stripped) = file_name.strip_suffix(suffix) {
            normalized_name = stripped;
            break;
        }
    }
    if normalized_name == current_name {
        return false;
    }
    let unversioned_name = format!("{base_name}.sqlite");
    if normalized_name == unversioned_name {
        return true;
    }

    let Some(version_with_extension) = normalized_name.strip_prefix(&format!("{base_name}_"))
    else {
        return false;
    };
    let Some(version_suffix) = version_with_extension.strip_suffix(".sqlite") else {
        return false;
    };
    !version_suffix.is_empty() && version_suffix.chars().all(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    fn unique_test_codex_home() -> PathBuf {
        std::env::temp_dir().join(format!("codex-state-runtime-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn init_rebuilds_state_db_when_applied_migration_is_missing() {
        let codex_home = unique_test_codex_home();
        tokio::fs::create_dir_all(&codex_home)
            .await
            .expect("create codex home");
        let state_path = state_db_path(codex_home.as_path());
        let latest_migration_version = STATE_MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .max()
            .expect("state migrator should include at least one migration");
        let future_version = latest_migration_version + 1;

        let pool = open_sqlite(&state_path, &STATE_MIGRATOR, "state")
            .await
            .expect("create initial state db");
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(future_version)
        .bind("future_migration")
        .bind(true)
        .bind(vec![0_u8])
        .bind(0_i64)
        .execute(&pool)
        .await
        .expect("insert incompatible migration row");
        pool.close().await;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("rebuild incompatible state db");
        let versions: Vec<(i64,)> =
            sqlx::query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(runtime.pool.as_ref())
                .await
                .expect("read rebuilt migration table");
        assert_eq!(
            versions.last().map(|row| row.0),
            Some(latest_migration_version)
        );
        assert!(
            !versions.iter().any(|row| row.0 == future_version),
            "expected rebuilt state db to drop incompatible migration record"
        );

        let mut quarantined = Vec::new();
        let mut entries = tokio::fs::read_dir(&codex_home)
            .await
            .expect("read codex home");
        while let Some(entry) = entries.next_entry().await.expect("read dir entry") {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name.starts_with("state_5.sqlite.incompatible-") {
                quarantined.push(file_name.to_string());
            }
        }
        assert_eq!(
            quarantined.len(),
            1,
            "expected exactly one quarantined state db"
        );

        tokio::fs::remove_dir_all(&codex_home)
            .await
            .expect("clean test codex home");
    }
}
