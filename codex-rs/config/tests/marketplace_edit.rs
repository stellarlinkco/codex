use codex_config::MarketplaceConfigUpdate;
use codex_config::RemoveMarketplaceConfigOutcome;
use codex_config::record_user_marketplace;
use codex_config::remove_user_marketplace_config;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let unique = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos(),
            Err(err) => panic!("system clock should be after unix epoch: {err}"),
        };
        let path = std::env::temp_dir().join(format!(
            "codex-config-marketplace-edit-{}-{unique}",
            std::process::id()
        ));
        if let Err(err) = std::fs::create_dir_all(&path) {
            panic!("failed to create temp dir {}: {err}", path.display());
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn remove_user_marketplace_config_reports_case_mismatch() {
    let codex_home = TestDir::new();
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
    let codex_home = TestDir::new();
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
