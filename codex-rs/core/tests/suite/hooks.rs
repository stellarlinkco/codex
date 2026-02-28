use anyhow::Result;
use core_test_support::fs_wait;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_toml_hooks_loaded_into_session() -> Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    let marker_path = home.path().join("session_start.marker");
    let marker_path = marker_path.to_string_lossy();

    let config_toml = if cfg!(windows) {
        format!(
            "[hooks]\n\n[[hooks.session_start]]\ncommand = ['cmd', '/C', 'echo loaded>>\"{marker_path}\"']\n"
        )
    } else {
        format!(
            "[hooks]\n\n[[hooks.session_start]]\ncommand = ['sh', '-c', 'echo loaded >> \"{marker_path}\"']\n"
        )
    };
    std::fs::write(home.path().join("config.toml"), config_toml)?;

    let mut builder = test_codex().with_home(Arc::clone(&home));
    builder.build(&server).await?;

    fs_wait::wait_for_path_exists(
        home.path().join("session_start.marker"),
        Duration::from_secs(2),
    )
    .await?;
    let contents = std::fs::read_to_string(home.path().join("session_start.marker"))?;
    assert!(contents.contains("loaded"));
    Ok(())
}
