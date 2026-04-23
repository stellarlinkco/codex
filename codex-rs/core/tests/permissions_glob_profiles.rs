#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;

use anyhow::Result;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use tempfile::TempDir;

#[cfg(target_os = "linux")]
fn test_harness_overrides() -> ConfigOverrides {
    ConfigOverrides {
        codex_linux_sandbox_exe: Some(
            codex_utils_cargo_bin::cargo_bin("codex-linux-sandbox")
                .expect("find codex-linux-sandbox"),
        ),
        ..ConfigOverrides::default()
    }
}

#[cfg(not(target_os = "linux"))]
fn test_harness_overrides() -> ConfigOverrides {
    ConfigOverrides::default()
}

async fn load_config_from_home(home: &TempDir, cwd: &TempDir) -> Result<Config> {
    Ok(ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .harness_overrides(test_harness_overrides())
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await?)
}

#[tokio::test(flavor = "current_thread")]
async fn permissions_profile_compiles_project_root_deny_glob_with_scan_depth() -> Result<()> {
    let home = TempDir::new()?;
    let cwd = TempDir::new()?;
    fs::write(cwd.path().join(".git"), "gitdir: nowhere")?;
    fs::write(
        home.path().join("config.toml"),
        r#"default_permissions = "workspace"

[permissions.workspace.filesystem]
":minimal" = "read"
glob_scan_max_depth = 4

[permissions.workspace.filesystem.":project_roots"]
"." = "write"
"**/*.env" = "none"
"#,
    )?;

    let config = load_config_from_home(&home, &cwd).await?;
    let entries = &config.permissions.file_system_sandbox_policy.entries;
    let expected_pattern = format!("{}/**/*.env", cwd.path().display());

    assert_eq!(
        config
            .permissions
            .file_system_sandbox_policy
            .glob_scan_max_depth,
        Some(4)
    );
    assert!(entries.iter().any(|entry| {
        entry.access == FileSystemAccessMode::None
            && matches!(
                &entry.path,
                FileSystemPath::GlobPattern { pattern } if pattern == &expected_pattern
            )
    }));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn permissions_profile_warns_when_recursive_deny_glob_has_no_scan_depth() -> Result<()> {
    let home = TempDir::new()?;
    let cwd = TempDir::new()?;
    fs::write(cwd.path().join(".git"), "gitdir: nowhere")?;
    fs::write(
        home.path().join("config.toml"),
        r#"default_permissions = "workspace"

[permissions.workspace.filesystem]
":minimal" = "read"

[permissions.workspace.filesystem.":project_roots"]
"." = "write"
"**/*.env" = "none"
"#,
    )?;

    let config = load_config_from_home(&home, &cwd).await?;

    assert!(
        config
            .startup_warnings
            .iter()
            .any(|warning| warning.contains("glob_scan_max_depth") && warning.contains("**/*.env")),
        "{:?}",
        config.startup_warnings
    );
    Ok(())
}
