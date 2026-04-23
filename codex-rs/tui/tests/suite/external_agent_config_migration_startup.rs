use codex_core::config::ConfigBuilder;
use codex_core::external_agent_config::ExternalAgentConfigMigrationItem;
use codex_core::external_agent_config::ExternalAgentConfigMigrationItemType;
use codex_tui::external_agent_config_migration_startup::EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS;
use codex_tui::external_agent_config_migration_startup::external_agent_config_migration_success_message;
use codex_tui::external_agent_config_migration_startup::is_external_config_migration_scope_cooling_down;
use codex_tui::external_agent_config_migration_startup::visible_external_agent_config_migration_items;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tempfile::tempdir;

#[tokio::test]
async fn visible_external_agent_config_migration_items_omits_hidden_scopes() {
    let codex_home = tempdir().expect("temp codex home");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    config.notices.external_config_migration_prompts.home = Some(true);
    config
        .notices
        .external_config_migration_prompts
        .projects
        .insert("/tmp/project".to_string(), true);

    let visible = visible_external_agent_config_migration_items(
        &config,
        vec![
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::Config,
                description: "home".to_string(),
                cwd: None,
            },
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::AgentsMd,
                description: "project".to_string(),
                cwd: Some(PathBuf::from("/tmp/project")),
            },
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::Skills,
                description: "other project".to_string(),
                cwd: Some(PathBuf::from("/tmp/other")),
            },
        ],
        1_760_000_000,
    );

    assert_eq!(
        visible,
        vec![ExternalAgentConfigMigrationItem {
            item_type: ExternalAgentConfigMigrationItemType::Skills,
            description: "other project".to_string(),
            cwd: Some(PathBuf::from("/tmp/other")),
        }]
    );
}

#[tokio::test]
async fn visible_external_agent_config_migration_items_omits_recently_prompted_scopes() {
    let codex_home = tempdir().expect("temp codex home");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    config
        .notices
        .external_config_migration_prompts
        .home_last_prompted_at = Some(1_760_000_000);
    config
        .notices
        .external_config_migration_prompts
        .project_last_prompted_at
        .insert("/tmp/project".to_string(), 1_760_000_000);

    let visible = visible_external_agent_config_migration_items(
        &config,
        vec![
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::Config,
                description: "home".to_string(),
                cwd: None,
            },
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::AgentsMd,
                description: "project".to_string(),
                cwd: Some(PathBuf::from("/tmp/project")),
            },
            ExternalAgentConfigMigrationItem {
                item_type: ExternalAgentConfigMigrationItemType::Skills,
                description: "other project".to_string(),
                cwd: Some(PathBuf::from("/tmp/other")),
            },
        ],
        1_760_000_000 + EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS - 1,
    );

    assert_eq!(
        visible,
        vec![ExternalAgentConfigMigrationItem {
            item_type: ExternalAgentConfigMigrationItemType::Skills,
            description: "other project".to_string(),
            cwd: Some(PathBuf::from("/tmp/other")),
        }]
    );
}

#[tokio::test]
async fn external_config_migration_scope_cooldown_expires_after_five_days() {
    let codex_home = tempdir().expect("temp codex home");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    config
        .notices
        .external_config_migration_prompts
        .home_last_prompted_at = Some(1_760_000_000);

    assert!(is_external_config_migration_scope_cooling_down(
        &config,
        None,
        1_760_000_000 + EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS - 1,
    ));
    assert!(!is_external_config_migration_scope_cooling_down(
        &config,
        None,
        1_760_000_000 + EXTERNAL_CONFIG_MIGRATION_PROMPT_COOLDOWN_SECS,
    ));
}

#[test]
fn external_agent_config_migration_success_message_is_plain_when_no_plugins_exist() {
    let message = external_agent_config_migration_success_message(&[
        ExternalAgentConfigMigrationItem {
            item_type: ExternalAgentConfigMigrationItemType::Config,
            description: String::new(),
            cwd: None,
        },
        ExternalAgentConfigMigrationItem {
            item_type: ExternalAgentConfigMigrationItemType::Skills,
            description: String::new(),
            cwd: Some(PathBuf::from("/tmp/project")),
        },
    ]);

    assert_eq!(
        message,
        "External config migration completed successfully.".to_string()
    );
}
