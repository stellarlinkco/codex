use std::sync::Arc;

use anyhow::Result;
use codex_hooks::Hooks;
use codex_hooks::HooksConfig;
use codex_hooks::SessionStartRequest;
use codex_hooks::SessionStartSource;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_toml_hooks_loaded_into_session() -> Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    std::fs::write(home.path().join("config.toml"), "")?;
    std::fs::write(
        home.path().join("hooks.json"),
        serde_json::to_vec_pretty(&json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "echo loaded"
                            }
                        ]
                    }
                ]
            }
        }))?,
    )?;

    let test = test_codex()
        .with_home(Arc::clone(&home))
        .build(&server)
        .await?;
    let hooks = Hooks::new(HooksConfig {
        feature_enabled: true,
        config_layer_stack: Some(test.config.config_layer_stack.clone()),
        ..HooksConfig::default()
    });

    let previews = hooks.preview_session_start(&SessionStartRequest {
        session_id: test.session_configured.session_id,
        cwd: test.config.cwd.clone(),
        transcript_path: None,
        model: test.session_configured.model,
        permission_mode: "default".to_string(),
        source: SessionStartSource::Startup,
    });

    assert_eq!(
        previews.len(),
        1,
        "expected hooks.json session_start hook to be loaded"
    );
    assert_eq!(
        previews[0]
            .source_path
            .file_name()
            .and_then(|name| name.to_str()),
        Some("hooks.json")
    );

    Ok(())
}
