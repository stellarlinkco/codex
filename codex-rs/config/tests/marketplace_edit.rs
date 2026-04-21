use codex_config::MarketplaceConfigUpdate;
use codex_config::RemoveMarketplaceConfigOutcome;
use codex_config::record_user_marketplace;
use codex_config::remove_user_marketplace_config;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn remove_user_marketplace_config_reports_case_mismatch() {
    let codex_home = TempDir::new().unwrap();
    let update = MarketplaceConfigUpdate {
        last_updated: "2026-04-13T00:00:00Z",
        last_revision: None,
        source_type: "git",
        source: "https://github.com/owner/repo.git",
        ref_name: Some("main"),
        sparse_paths: &[],
    };
    record_user_marketplace(codex_home.path(), "debug", &update).unwrap();

    let outcome = remove_user_marketplace_config(codex_home.path(), "Debug").unwrap();

    assert_eq!(
        outcome,
        RemoveMarketplaceConfigOutcome::NameCaseMismatch {
            configured_name: "debug".to_string()
        }
    );
}

#[test]
fn remove_user_marketplace_config_removes_inline_table_entry() {
    let codex_home = TempDir::new().unwrap();
    std::fs::write(
        codex_home.path().join(codex_config::CONFIG_TOML_FILE),
        r#"
marketplaces = {
  debug = { source_type = "git", source = "https://github.com/owner/repo.git" },
  other = { source_type = "local", source = "/tmp/marketplace" },
}
"#,
    )
    .unwrap();

    let outcome = remove_user_marketplace_config(codex_home.path(), "debug").unwrap();

    assert_eq!(outcome, RemoveMarketplaceConfigOutcome::Removed);
    let config: toml::Value = toml::from_str(
        &std::fs::read_to_string(codex_home.path().join(codex_config::CONFIG_TOML_FILE)).unwrap(),
    )
    .unwrap();
    let marketplaces = config
        .get("marketplaces")
        .and_then(toml::Value::as_table)
        .unwrap();
    assert_eq!(marketplaces.len(), 1);
    assert!(marketplaces.contains_key("other"));
}
