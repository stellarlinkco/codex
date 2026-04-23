use crate::external_agent_config_migration::ExternalAgentConfigMigrationOutcome;
use crate::external_agent_config_migration::run_external_agent_config_migration_prompt;
use crate::resume_picker::SessionSelection;
use crate::tui;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::external_agent_config::ExternalAgentConfigDetectOptions;
use codex_core::external_agent_config::ExternalAgentConfigMigrationItem;
use codex_core::external_agent_config::ExternalAgentConfigService;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use toml::Value as TomlValue;

pub const EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS: i64 = 5 * 24 * 60 * 60;

pub(crate) enum ExternalAgentConfigMigrationStartupOutcome {
    Continue { success_message: Option<String> },
    ExitRequested,
}

fn should_show_external_agent_config_migration_prompt(
    session_selection: &SessionSelection,
) -> bool {
    matches!(
        session_selection,
        SessionSelection::StartFresh | SessionSelection::Exit
    )
}

fn external_config_migration_project_key(path: &Path) -> String {
    path.display().to_string()
}

fn is_external_config_migration_scope_hidden(config: &Config, cwd: Option<&Path>) -> bool {
    match cwd {
        Some(cwd) => config
            .notices
            .external_config_migration_prompts
            .projects
            .get(&external_config_migration_project_key(cwd))
            .copied()
            .unwrap_or(false),
        None => config
            .notices
            .external_config_migration_prompts
            .home
            .unwrap_or(false),
    }
}

fn external_config_migration_last_prompted_at(config: &Config, cwd: Option<&Path>) -> Option<i64> {
    match cwd {
        Some(cwd) => config
            .notices
            .external_config_migration_prompts
            .project_last_prompted_at
            .get(&external_config_migration_project_key(cwd))
            .copied(),
        None => {
            config
                .notices
                .external_config_migration_prompts
                .home_last_prompted_at
        }
    }
}

pub fn is_external_config_migration_scope_cooling_down(
    config: &Config,
    cwd: Option<&Path>,
    now_unix_seconds: i64,
) -> bool {
    external_config_migration_last_prompted_at(config, cwd).is_some_and(|last_prompted_at| {
        last_prompted_at.saturating_add(EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS)
            > now_unix_seconds
    })
}

pub fn visible_external_agent_config_migration_items(
    config: &Config,
    items: Vec<ExternalAgentConfigMigrationItem>,
    now_unix_seconds: i64,
) -> Vec<ExternalAgentConfigMigrationItem> {
    items
        .into_iter()
        .filter(|item| {
            !is_external_config_migration_scope_hidden(config, item.cwd.as_deref())
                && !is_external_config_migration_scope_cooling_down(
                    config,
                    item.cwd.as_deref(),
                    now_unix_seconds,
                )
        })
        .collect()
}

pub fn external_agent_config_migration_success_message(
    _items: &[ExternalAgentConfigMigrationItem],
) -> String {
    "External config migration completed successfully.".to_string()
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn persist_external_agent_config_migration_prompt_shown(
    config: &mut Config,
    items: &[ExternalAgentConfigMigrationItem],
    now_unix_seconds: i64,
) -> Result<()> {
    let projects = items
        .iter()
        .filter_map(|item| item.cwd.as_deref())
        .map(external_config_migration_project_key)
        .collect::<BTreeSet<_>>();
    let has_home_item = items.iter().any(|item| item.cwd.is_none());
    if !has_home_item && projects.is_empty() {
        return Ok(());
    }
    let mut builder = ConfigEditsBuilder::new(&config.codex_home);
    if has_home_item {
        builder =
            builder.set_external_config_migration_prompt_home_last_prompted_at(now_unix_seconds);
    }
    for project in &projects {
        builder = builder.set_external_config_migration_prompt_project_last_prompted_at(
            project,
            now_unix_seconds,
        );
    }
    builder
        .apply()
        .await
        .map_err(|err| color_eyre::eyre::eyre!("{err}"))
        .wrap_err("Failed to save external config migration prompt timestamp")?;
    if has_home_item {
        config
            .notices
            .external_config_migration_prompts
            .home_last_prompted_at = Some(now_unix_seconds);
    }
    for project in projects {
        config
            .notices
            .external_config_migration_prompts
            .project_last_prompted_at
            .insert(project, now_unix_seconds);
    }
    Ok(())
}

async fn persist_external_agent_config_migration_prompt_dismissal(
    config: &mut Config,
    items: &[ExternalAgentConfigMigrationItem],
) -> Result<()> {
    let hide_home = items.iter().any(|item| item.cwd.is_none());
    let projects = items
        .iter()
        .filter_map(|item| item.cwd.as_deref())
        .map(external_config_migration_project_key)
        .collect::<BTreeSet<_>>();
    if !hide_home && projects.is_empty() {
        return Ok(());
    }
    let mut builder = ConfigEditsBuilder::new(&config.codex_home);
    if hide_home {
        builder = builder.set_hide_external_config_migration_prompt_home(true);
    }
    for project in &projects {
        builder = builder.set_hide_external_config_migration_prompt_project(project, true);
    }
    builder
        .apply()
        .await
        .map_err(|err| color_eyre::eyre::eyre!("{err}"))
        .wrap_err("Failed to save external config migration prompt preference")?;
    if hide_home {
        config.notices.external_config_migration_prompts.home = Some(true);
    }
    for project in projects {
        config
            .notices
            .external_config_migration_prompts
            .projects
            .insert(project, true);
    }
    Ok(())
}

pub(crate) async fn handle_external_agent_config_migration_prompt_if_needed(
    tui: &mut tui::Tui,
    config: &mut Config,
    cli_kv_overrides: &[(String, TomlValue)],
    harness_overrides: &ConfigOverrides,
    session_selection: &SessionSelection,
) -> Result<ExternalAgentConfigMigrationStartupOutcome> {
    if !should_show_external_agent_config_migration_prompt(session_selection) {
        return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
            success_message: None,
        });
    }

    let service = ExternalAgentConfigService::new(config.codex_home.to_path_buf());
    let now_unix_seconds = unix_seconds_now();
    let detected_items = match service.detect(ExternalAgentConfigDetectOptions {
        include_home: true,
        cwds: Some(vec![config.cwd.to_path_buf()]),
    }) {
        Ok(items) => visible_external_agent_config_migration_items(config, items, now_unix_seconds),
        Err(err) => {
            tracing::warn!(error = %err, cwd = %config.cwd.display(), "failed to detect external agent config migrations; continuing startup");
            return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
                success_message: None,
            });
        }
    };
    if detected_items.is_empty() {
        return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
            success_message: None,
        });
    }
    if let Err(err) = persist_external_agent_config_migration_prompt_shown(
        config,
        &detected_items,
        now_unix_seconds,
    )
    .await
    {
        tracing::warn!(error = %err, cwd = %config.cwd.display(), "failed to persist external config migration prompt timestamp");
    }

    let mut error: Option<String> = None;
    loop {
        match run_external_agent_config_migration_prompt(tui, &detected_items, error.as_deref())
            .await
        {
            ExternalAgentConfigMigrationOutcome::Import => {
                match service.import(detected_items.clone()) {
                    Ok(()) => {
                        *config = ConfigBuilder::default()
                            .codex_home(config.codex_home.to_path_buf())
                            .cli_overrides(cli_kv_overrides.to_vec())
                            .harness_overrides(harness_overrides.clone())
                            .build()
                            .await
                            .wrap_err("Failed to reload config after external agent migration")?;
                        return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
                            success_message: Some(external_agent_config_migration_success_message(
                                &detected_items,
                            )),
                        });
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, cwd = %config.cwd.display(), "failed to import external agent config migration items");
                        error = Some(format!("Migration failed: {err}"));
                    }
                }
            }
            ExternalAgentConfigMigrationOutcome::Skip => {
                return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
                    success_message: None,
                });
            }
            ExternalAgentConfigMigrationOutcome::SkipForever => {
                match persist_external_agent_config_migration_prompt_dismissal(
                    config,
                    &detected_items,
                )
                .await
                {
                    Ok(()) => {
                        return Ok(ExternalAgentConfigMigrationStartupOutcome::Continue {
                            success_message: None,
                        });
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, cwd = %config.cwd.display(), "failed to persist external config migration prompt dismissal");
                        error = Some(format!("Failed to save preference: {err}"));
                    }
                }
            }
            ExternalAgentConfigMigrationOutcome::Exit => {
                return Ok(ExternalAgentConfigMigrationStartupOutcome::ExitRequested);
            }
        }
    }
}
