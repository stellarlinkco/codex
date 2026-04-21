use codex_config::RemoveMarketplaceConfigOutcome;
use codex_config::remove_user_marketplace_config;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::plugins::marketplace_install_root;

use super::store::validate_plugin_segment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceRemoveRequest {
    pub marketplace_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceRemoveOutcome {
    pub marketplace_name: String,
    pub removed_installed_root: Option<AbsolutePathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceRemoveError {
    #[error("{0}")]
    InvalidRequest(String),

    #[error("{0}")]
    Internal(String),
}

pub async fn remove_marketplace(
    codex_home: PathBuf,
    request: MarketplaceRemoveRequest,
) -> Result<MarketplaceRemoveOutcome, MarketplaceRemoveError> {
    tokio::task::spawn_blocking(move || remove_marketplace_sync(codex_home.as_path(), request))
        .await
        .map_err(|err| {
            MarketplaceRemoveError::Internal(format!("failed to remove marketplace: {err}"))
        })?
}

fn remove_marketplace_sync(
    codex_home: &Path,
    request: MarketplaceRemoveRequest,
) -> Result<MarketplaceRemoveOutcome, MarketplaceRemoveError> {
    let marketplace_name = request.marketplace_name;
    validate_plugin_segment(&marketplace_name, "marketplace name")
        .map_err(MarketplaceRemoveError::InvalidRequest)?;

    let installed_root = marketplace_install_root(codex_home).join(&marketplace_name);
    let config_outcome =
        remove_user_marketplace_config(codex_home, &marketplace_name).map_err(|err| {
            MarketplaceRemoveError::Internal(format!(
                "failed to remove marketplace '{marketplace_name}' from user config.toml: {err}"
            ))
        })?;

    if let RemoveMarketplaceConfigOutcome::NameCaseMismatch { configured_name } = &config_outcome {
        return Err(MarketplaceRemoveError::InvalidRequest(format!(
            "marketplace `{marketplace_name}` does not match configured marketplace `{configured_name}` exactly"
        )));
    }

    let removed_config = config_outcome == RemoveMarketplaceConfigOutcome::Removed;
    let removed_installed_root = remove_marketplace_root(&installed_root)?;
    if removed_installed_root.is_none() && !removed_config {
        return Err(MarketplaceRemoveError::InvalidRequest(format!(
            "marketplace `{marketplace_name}` is not configured or installed"
        )));
    }

    Ok(MarketplaceRemoveOutcome {
        marketplace_name,
        removed_installed_root,
    })
}

fn remove_marketplace_root(root: &Path) -> Result<Option<AbsolutePathBuf>, MarketplaceRemoveError> {
    if !root.exists() {
        return Ok(None);
    }

    let removed_root = AbsolutePathBuf::try_from(root.to_path_buf()).map_err(|err| {
        MarketplaceRemoveError::Internal(format!(
            "failed to resolve installed marketplace root {}: {err}",
            root.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(root).map_err(|err| {
        MarketplaceRemoveError::Internal(format!(
            "failed to inspect installed marketplace root {}: {err}",
            root.display()
        ))
    })?;
    let result = if metadata.is_dir() {
        fs::remove_dir_all(root)
    } else {
        fs::remove_file(root)
    };
    result.map_err(|err| {
        MarketplaceRemoveError::Internal(format!(
            "failed to remove installed marketplace root {}: {err}",
            root.display()
        ))
    })?;

    Ok(Some(removed_root))
}
