#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use codex_config::MarketplaceConfigUpdate;
use codex_config::record_user_marketplace;
use codex_core::plugins::MarketplaceRemoveRequest;
use codex_core::plugins::marketplace_install_root;
use codex_core::plugins::remove_marketplace;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn configured_marketplace_update() -> MarketplaceConfigUpdate<'static> {
    MarketplaceConfigUpdate {
        last_updated: "2026-04-13T00:00:00Z",
        last_revision: None,
        source_type: "git",
        source: "https://github.com/owner/repo.git",
        ref_name: Some("main"),
        sparse_paths: &[],
    }
}

fn write_installed_marketplace(codex_home: &std::path::Path, marketplace_name: &str) -> Result<()> {
    let root = marketplace_install_root(codex_home).join(marketplace_name);
    std::fs::create_dir_all(root.join(".agents/plugins"))?;
    std::fs::write(root.join(".agents/plugins/marketplace.json"), "{}")?;
    std::fs::write(root.join("marker.txt"), "installed")?;
    Ok(())
}

#[tokio::test]
async fn remove_marketplace_deletes_config_and_installed_root() -> Result<()> {
    let codex_home = TempDir::new()?;
    record_user_marketplace(codex_home.path(), "debug", &configured_marketplace_update())?;
    write_installed_marketplace(codex_home.path(), "debug")?;

    let outcome = remove_marketplace(
        codex_home.path().to_path_buf(),
        MarketplaceRemoveRequest {
            marketplace_name: "debug".to_string(),
        },
    )
    .await?;

    assert_eq!(outcome.marketplace_name, "debug");
    assert_eq!(
        outcome.removed_installed_root,
        Some(
            marketplace_install_root(codex_home.path())
                .join("debug")
                .try_into()?
        )
    );
    let config = std::fs::read_to_string(codex_home.path().join(codex_config::CONFIG_TOML_FILE))?;
    assert!(!config.contains("[marketplaces.debug]"));
    assert!(
        !marketplace_install_root(codex_home.path())
            .join("debug")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn remove_marketplace_rejects_unknown_marketplace() -> Result<()> {
    let codex_home = TempDir::new()?;

    let err = remove_marketplace(
        codex_home.path().to_path_buf(),
        MarketplaceRemoveRequest {
            marketplace_name: "debug".to_string(),
        },
    )
    .await
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        "marketplace `debug` is not configured or installed"
    );
    Ok(())
}

#[tokio::test]
async fn remove_marketplace_rejects_case_mismatched_marketplace_name() -> Result<()> {
    let codex_home = TempDir::new()?;
    record_user_marketplace(codex_home.path(), "debug", &configured_marketplace_update())?;
    write_installed_marketplace(codex_home.path(), "debug")?;

    let err = remove_marketplace(
        codex_home.path().to_path_buf(),
        MarketplaceRemoveRequest {
            marketplace_name: "Debug".to_string(),
        },
    )
    .await
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        "marketplace `Debug` does not match configured marketplace `debug` exactly"
    );
    let config = std::fs::read_to_string(codex_home.path().join(codex_config::CONFIG_TOML_FILE))?;
    assert!(config.contains("[marketplaces.debug]"));
    assert!(
        marketplace_install_root(codex_home.path())
            .join("debug")
            .exists()
    );
    Ok(())
}
