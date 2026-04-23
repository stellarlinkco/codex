use codex_config::CONFIG_TOML_FILE;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::edit::apply_blocking;
use pretty_assertions::assert_eq;
use tempfile::tempdir;
use toml::Value as TomlValue;

#[test]
fn blocking_set_hide_external_config_migration_prompt_home_preserves_nested_table() {
    let tmp = tempdir().expect("tmpdir");
    let codex_home = tmp.path();
    std::fs::write(
        codex_home.join(CONFIG_TOML_FILE),
        r#"[notice]
existing = "value"
"#,
    )
    .expect("seed");

    apply_blocking(
        codex_home,
        None,
        &[ConfigEdit::SetNoticeHideExternalConfigMigrationPromptHome(
            true,
        )],
    )
    .expect("persist");

    let contents = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).expect("read config");
    let expected = r#"[notice]
existing = "value"

[notice.external_config_migration_prompts]
home = true
"#;
    assert_eq!(contents, expected);
}

#[test]
fn blocking_set_external_config_migration_project_timestamp_preserves_nested_table() {
    let tmp = tempdir().expect("tmpdir");
    let codex_home = tmp.path();
    std::fs::write(
        codex_home.join(CONFIG_TOML_FILE),
        r#"[notice]
existing = "value"
"#,
    )
    .expect("seed");

    apply_blocking(
        codex_home,
        None,
        &[
            ConfigEdit::SetNoticeExternalConfigMigrationPromptProjectLastPromptedAt(
                "/tmp/project".to_string(),
                1_760_000_000,
            ),
        ],
    )
    .expect("persist");

    let contents = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).expect("read config");
    let expected = r#"[notice]
existing = "value"

[notice.external_config_migration_prompts.project_last_prompted_at]
"/tmp/project" = 1760000000
"#;
    assert_eq!(contents, expected);
}

#[tokio::test]
async fn async_builder_set_external_config_migration_prompt_home_timestamp_persists() {
    let tmp = tempdir().expect("tmpdir");
    let codex_home = tmp.path().to_path_buf();

    ConfigEditsBuilder::new(&codex_home)
        .set_external_config_migration_prompt_home_last_prompted_at(1_760_000_000)
        .apply()
        .await
        .expect("persist");

    let raw = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).expect("read config");
    let timestamp = toml::from_str::<TomlValue>(&raw)
        .expect("parse config")
        .get("notice")
        .and_then(TomlValue::as_table)
        .and_then(|tbl| tbl.get("external_config_migration_prompts"))
        .and_then(TomlValue::as_table)
        .and_then(|tbl| tbl.get("home_last_prompted_at"))
        .and_then(TomlValue::as_integer);
    assert_eq!(timestamp, Some(1_760_000_000));
}
