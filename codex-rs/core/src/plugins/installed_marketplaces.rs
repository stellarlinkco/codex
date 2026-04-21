use std::path::Path;
use std::path::PathBuf;

pub const INSTALLED_MARKETPLACES_DIR: &str = ".tmp/marketplaces";

pub fn marketplace_install_root(codex_home: &Path) -> PathBuf {
    codex_home.join(INSTALLED_MARKETPLACES_DIR)
}
