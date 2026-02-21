#![cfg(not(target_os = "windows"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use codex_core::features::Feature;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn text_from_content_items(items: &[Value]) -> Option<String> {
    let text_segments = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("input_text") => item.get("text").and_then(Value::as_str),
            Some(_) | None => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    if text_segments.is_empty() {
        None
    } else {
        Some(text_segments.join("\n"))
    }
}

fn text_from_output_value(output: &Value) -> Option<String> {
    match output {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => text_from_content_items(items),
        Value::Object(obj) => {
            if let Some(text) = obj.get("content").and_then(Value::as_str) {
                return Some(text.to_string());
            }
            if let Some(text) = obj
                .get("content")
                .and_then(Value::as_array)
                .and_then(|items| text_from_content_items(items))
            {
                return Some(text);
            }
            obj.get("content_items")
                .and_then(Value::as_array)
                .and_then(|items| text_from_content_items(items))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn is_lead_request(request: &wiremock::Request) -> bool {
    request.headers.get("x-openai-subagent").is_none()
}

fn parse_tool_output_json(mock: &ResponseMock, call_id: &str) -> Result<Option<Value>> {
    if let Some(output) = mock.function_call_output_text(call_id) {
        let parsed: Value = serde_json::from_str(&output).context("tool output should be JSON")?;
        return Ok(Some(parsed));
    }

    for request in mock.requests() {
        for item in request.input() {
            let is_target_call = matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            ) && item.get("call_id").and_then(Value::as_str) == Some(call_id);
            if !is_target_call {
                continue;
            }

            if let Some(output_text) = item.get("output").and_then(text_from_output_value) {
                let parsed: Value =
                    serde_json::from_str(&output_text).context("tool output should be JSON")?;
                return Ok(Some(parsed));
            }
        }
    }

    Ok(None)
}

fn captured_call_outputs(mock: &ResponseMock) -> Vec<String> {
    mock.requests()
        .iter()
        .flat_map(|request| request.input().into_iter())
        .filter_map(|item| {
            let output_type = item.get("type").and_then(Value::as_str);
            if !matches!(
                output_type,
                Some("function_call_output" | "custom_tool_call_output")
            ) {
                return None;
            }
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("<missing-call-id>");
            Some(format!("{}:{call_id}", output_type.unwrap_or_default()))
        })
        .collect()
}

async fn tool_output_json(mock: &ResponseMock, call_id: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(parsed) = parse_tool_output_json(mock, call_id)? {
            return Ok(parsed);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "call output missing for {call_id}; captured {} requests; outputs={:?}",
                mock.requests().len(),
                captured_call_outputs(mock)
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[test]
fn text_from_output_value_supports_string_output() {
    let item = json!({
        "type": "function_call_output",
        "output": "{\"ok\":true}"
    });
    assert_eq!(
        item.get("output").and_then(text_from_output_value),
        Some("{\"ok\":true}".to_string())
    );
}

#[test]
fn text_from_output_value_supports_content_item_arrays() {
    let item = json!({
        "type": "function_call_output",
        "output": [
            {"type": "input_text", "text": "{\"ok\":"},
            {"type": "input_image", "image_url": "data:image/png;base64,AAA"},
            {"type": "input_text", "text": "true}"}
        ]
    });
    assert_eq!(
        item.get("output").and_then(text_from_output_value),
        Some("{\"ok\":\ntrue}".to_string())
    );
}

#[test]
fn text_from_output_value_supports_legacy_object_output() {
    let item = json!({
        "type": "function_call_output",
        "output": {"content": "{\"ok\":true}"}
    });
    assert_eq!(
        item.get("output").and_then(text_from_output_value),
        Some("{\"ok\":true}".to_string())
    );
}

fn first_task_id_for_assignee(tasks_dir: &Path, assignee: &str) -> Result<String> {
    for entry in std::fs::read_dir(tasks_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(entry.path())?;
        let task: Value = serde_json::from_str(&raw)?;
        if task
            .get("assignee")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            == Some(assignee)
        {
            let task_id = task
                .get("id")
                .and_then(Value::as_str)
                .context("task id missing")?;
            return Ok(task_id.to_string());
        }
    }
    anyhow::bail!("task for assignee `{assignee}` not found");
}

fn git(path: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(path)
        .status()
        .with_context(|| format!("failed to run git {args:?}"))?;
    if status.success() {
        return Ok(());
    }
    bail!("git {args:?} exited with {status}");
}

fn init_git_repo(path: &Path) -> Result<()> {
    git(path, &["init", "--initial-branch=main"])?;
    git(path, &["config", "user.name", "Codex Tests"])?;
    git(path, &["config", "user.email", "codex-tests@example.com"])?;
    std::fs::write(path.join("README.md"), "seed\n")?;
    git(path, &["add", "README.md"])?;
    git(path, &["commit", "-m", "seed"])?;
    Ok(())
}

fn list_worktree_paths(codex_home: &Path) -> Result<Vec<PathBuf>> {
    let root = codex_home.join("worktrees");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut worktrees = Vec::new();
    for owner_entry in std::fs::read_dir(root)? {
        let owner_entry = owner_entry?;
        if !owner_entry.file_type()?.is_dir() {
            continue;
        }
        for worktree_entry in std::fs::read_dir(owner_entry.path())? {
            let worktree_entry = worktree_entry?;
            if worktree_entry.file_type()?.is_dir() {
                worktrees.push(worktree_entry.path());
            }
        }
    }
    worktrees.sort();
    Ok(worktrees)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_teams_tool_flow_persists_and_cleans_up() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.features.enable(Feature::Collab);
    });
    let test = builder.build(&server).await?;

    let team_id = "e2e-team";
    let spawn_call_id = "call-spawn-team";
    let spawn_args = json!({
        "team_id": team_id,
        "members": [
            {"name": "planner", "task": "Plan the work", "agent_type": "architect"},
            {"name": "worker", "task": "Implement the plan", "agent_type": "develop"}
        ]
    })
    .to_string();
    let spawn_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-spawn-1"),
                ev_function_call(spawn_call_id, "spawn_team", &spawn_args),
                ev_completed("resp-spawn-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-spawn-1", "spawned"),
                ev_completed("resp-spawn-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("spawn an agent team").await?;

    let spawn_output = tool_output_json(&spawn_mock, spawn_call_id).await?;
    assert_eq!(spawn_output["team_id"].as_str(), Some(team_id));
    assert_eq!(spawn_output["members"].as_array().map(Vec::len), Some(2));

    let team_config_path = test
        .codex_home_path()
        .join("teams")
        .join(team_id)
        .join("config.json");
    assert_eq!(team_config_path.exists(), true);

    let team_tasks_dir = test.codex_home_path().join("tasks").join(team_id);
    assert_eq!(team_tasks_dir.exists(), true);
    let planner_task_id = first_task_id_for_assignee(&team_tasks_dir, "planner")?;

    let claim_call_id = "call-claim-task";
    let claim_args = json!({
        "team_id": team_id,
        "task_id": planner_task_id.clone()
    })
    .to_string();
    let claim_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-claim-1"),
                ev_function_call(claim_call_id, "team_task_claim", &claim_args),
                ev_completed("resp-claim-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-claim-1", "claimed"),
                ev_completed("resp-claim-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("claim planner task").await?;

    let claim_output = tool_output_json(&claim_mock, claim_call_id).await?;
    assert_eq!(claim_output["claimed"].as_bool(), Some(true));
    assert_eq!(claim_output["task"]["state"].as_str(), Some("claimed"));

    let complete_call_id = "call-complete-task";
    let complete_args = json!({
        "team_id": team_id,
        "task_id": planner_task_id
    })
    .to_string();
    let complete_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-complete-1"),
                ev_function_call(complete_call_id, "team_task_complete", &complete_args),
                ev_completed("resp-complete-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-complete-1", "completed"),
                ev_completed("resp-complete-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("complete planner task").await?;

    let complete_output = tool_output_json(&complete_mock, complete_call_id).await?;
    assert_eq!(complete_output["completed"].as_bool(), Some(true));
    assert_eq!(complete_output["task"]["state"].as_str(), Some("completed"));

    let cleanup_call_id = "call-cleanup-team";
    let cleanup_args = json!({ "team_id": team_id }).to_string();
    let cleanup_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-cleanup-1"),
                ev_function_call(cleanup_call_id, "team_cleanup", &cleanup_args),
                ev_completed("resp-cleanup-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-cleanup-1", "cleaned"),
                ev_completed("resp-cleanup-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("cleanup team").await?;

    let cleanup_output = tool_output_json(&cleanup_mock, cleanup_call_id).await?;
    assert_eq!(cleanup_output["team_id"].as_str(), Some(team_id));
    assert_eq!(
        cleanup_output["removed_from_registry"].as_bool(),
        Some(true)
    );
    assert_eq!(cleanup_output["removed_team_config"].as_bool(), Some(true));
    assert_eq!(cleanup_output["removed_task_dir"].as_bool(), Some(true));
    assert_eq!(cleanup_output["closed"].as_array().map(Vec::len), Some(2));

    assert_eq!(std::fs::metadata(team_config_path).is_err(), true);
    assert_eq!(std::fs::metadata(team_tasks_dir).is_err(), true);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_teams_message_and_broadcast_flow() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.features.enable(Feature::Collab);
    });
    let test = builder.build(&server).await?;

    let team_id = "e2e-team-message";
    let team_config_path = test
        .codex_home_path()
        .join("teams")
        .join(team_id)
        .join("config.json");
    let team_tasks_dir = test.codex_home_path().join("tasks").join(team_id);

    let spawn_call_id = "call-msg-spawn-team";
    let spawn_args = json!({
        "team_id": team_id,
        "members": [
            {"name": "planner", "task": "Plan the work", "agent_type": "architect"},
            {"name": "worker", "task": "Implement the plan", "agent_type": "develop"}
        ]
    })
    .to_string();
    let spawn_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-msg-spawn-1"),
                ev_function_call(spawn_call_id, "spawn_team", &spawn_args),
                ev_completed("resp-msg-spawn-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-msg-spawn-1", "spawned"),
                ev_completed("resp-msg-spawn-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("spawn message team").await?;

    let spawn_output = tool_output_json(&spawn_mock, spawn_call_id).await?;
    assert_eq!(spawn_output["team_id"].as_str(), Some(team_id));
    assert_eq!(spawn_output["members"].as_array().map(Vec::len), Some(2));
    assert_eq!(team_config_path.exists(), true);
    assert_eq!(team_tasks_dir.exists(), true);

    let message_call_id = "call-team-message";
    let message_args = json!({
        "team_id": team_id,
        "member_name": "planner",
        "message": "Please provide a short plan.",
        "interrupt": false
    })
    .to_string();
    let message_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-team-message-1"),
                ev_function_call(message_call_id, "team_message", &message_args),
                ev_completed("resp-team-message-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-team-message-1", "sent"),
                ev_completed("resp-team-message-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("message planner").await?;

    let message_output = tool_output_json(&message_mock, message_call_id).await?;
    assert_eq!(message_output["team_id"].as_str(), Some(team_id));
    assert_eq!(message_output["member_name"].as_str(), Some("planner"));
    assert_eq!(
        message_output["submission_id"].as_str().map(str::is_empty),
        Some(false)
    );

    let broadcast_call_id = "call-team-broadcast";
    let broadcast_args = json!({
        "team_id": team_id,
        "message": "Status update in one sentence.",
        "interrupt": false
    })
    .to_string();
    let broadcast_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-team-broadcast-1"),
                ev_function_call(broadcast_call_id, "team_broadcast", &broadcast_args),
                ev_completed("resp-team-broadcast-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-team-broadcast-1", "broadcast"),
                ev_completed("resp-team-broadcast-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("broadcast to team").await?;

    let broadcast_output = tool_output_json(&broadcast_mock, broadcast_call_id).await?;
    let sent_count = broadcast_output["sent"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let failed_count = broadcast_output["failed"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(sent_count + failed_count, 2);

    let cleanup_call_id = "call-msg-cleanup-team";
    let cleanup_args = json!({ "team_id": team_id }).to_string();
    let cleanup_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-msg-cleanup-1"),
                ev_function_call(cleanup_call_id, "team_cleanup", &cleanup_args),
                ev_completed("resp-msg-cleanup-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-msg-cleanup-1", "cleaned"),
                ev_completed("resp-msg-cleanup-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("cleanup message team").await?;

    let cleanup_output = tool_output_json(&cleanup_mock, cleanup_call_id).await?;
    assert_eq!(cleanup_output["team_id"].as_str(), Some(team_id));
    assert_eq!(
        cleanup_output["removed_from_registry"].as_bool(),
        Some(true)
    );
    assert_eq!(cleanup_output["removed_team_config"].as_bool(), Some(true));
    assert_eq!(cleanup_output["removed_task_dir"].as_bool(), Some(true));
    assert_eq!(cleanup_output["closed"].as_array().map(Vec::len), Some(2));
    assert_eq!(std::fs::metadata(team_config_path).is_err(), true);
    assert_eq!(std::fs::metadata(team_tasks_dir).is_err(), true);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_agent_worktree_create_and_close_cleanup() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.features.enable(Feature::Collab);
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.cwd_path())?;

    let spawn_call_id = "call-worktree-spawn-agent";
    let spawn_args = json!({
        "message": "work in isolated checkout",
        "worktree": true
    })
    .to_string();
    let spawn_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-worktree-spawn-1"),
                ev_function_call(spawn_call_id, "spawn_agent", &spawn_args),
                ev_completed("resp-worktree-spawn-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-worktree-spawn-1", "spawned"),
                ev_completed("resp-worktree-spawn-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("spawn worktree agent").await?;

    let spawn_output = tool_output_json(&spawn_mock, spawn_call_id).await?;
    let agent_id = spawn_output["agent_id"]
        .as_str()
        .context("agent id missing")?
        .to_string();
    let worktrees = list_worktree_paths(test.codex_home_path())?;
    assert_eq!(worktrees.len(), 1);
    let worktree_path = worktrees
        .first()
        .cloned()
        .context("worktree path missing after spawn")?;
    assert_eq!(worktree_path.exists(), true);
    assert_ne!(worktree_path, test.cwd_path());

    let close_call_id = "call-worktree-close-agent";
    let close_args = json!({ "id": agent_id }).to_string();
    let close_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-worktree-close-1"),
                ev_function_call(close_call_id, "close_agent", &close_args),
                ev_completed("resp-worktree-close-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-worktree-close-1", "closed"),
                ev_completed("resp-worktree-close-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("close worktree agent").await?;

    let close_output = tool_output_json(&close_mock, close_call_id).await?;
    assert_eq!(close_output["status"].is_string(), true);
    assert_eq!(std::fs::metadata(worktree_path).is_err(), true);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_team_worktree_members_create_and_cleanup() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.features.enable(Feature::Collab);
    });
    let test = builder.build(&server).await?;
    init_git_repo(test.cwd_path())?;

    let team_id = "e2e-team-worktree";
    let spawn_call_id = "call-worktree-spawn-team";
    let spawn_args = json!({
        "team_id": team_id,
        "members": [
            {
                "name": "planner",
                "task": "Plan the work",
                "agent_type": "architect",
                "worktree": true
            },
            {
                "name": "worker",
                "task": "Implement the plan",
                "agent_type": "develop",
                "worktree": true
            }
        ]
    })
    .to_string();
    let spawn_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-worktree-team-spawn-1"),
                ev_function_call(spawn_call_id, "spawn_team", &spawn_args),
                ev_completed("resp-worktree-team-spawn-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-worktree-team-spawn-1", "spawned"),
                ev_completed("resp-worktree-team-spawn-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("spawn worktree team").await?;

    let spawn_output = tool_output_json(&spawn_mock, spawn_call_id).await?;
    assert_eq!(spawn_output["team_id"].as_str(), Some(team_id));
    assert_eq!(spawn_output["members"].as_array().map(Vec::len), Some(2));
    let worktree_paths = list_worktree_paths(test.codex_home_path())?;
    assert_eq!(worktree_paths.len(), 2);
    for worktree_path in &worktree_paths {
        assert_eq!(worktree_path.exists(), true);
        assert_ne!(worktree_path, test.cwd_path());
    }

    let cleanup_call_id = "call-worktree-team-cleanup";
    let cleanup_args = json!({ "team_id": team_id }).to_string();
    let cleanup_mock = mount_sse_sequence_match(
        &server,
        is_lead_request,
        vec![
            sse(vec![
                ev_response_created("resp-worktree-team-cleanup-1"),
                ev_function_call(cleanup_call_id, "team_cleanup", &cleanup_args),
                ev_completed("resp-worktree-team-cleanup-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-worktree-team-cleanup-1", "cleaned"),
                ev_completed("resp-worktree-team-cleanup-2"),
            ]),
        ],
    )
    .await;
    test.submit_turn("cleanup worktree team").await?;

    let cleanup_output = tool_output_json(&cleanup_mock, cleanup_call_id).await?;
    assert_eq!(cleanup_output["team_id"].as_str(), Some(team_id));
    assert_eq!(
        cleanup_output["removed_from_registry"].as_bool(),
        Some(true)
    );
    let closed_members = cleanup_output["closed"]
        .as_array()
        .context("closed members missing")?;
    assert_eq!(closed_members.len(), 2);
    for member in closed_members {
        assert_eq!(member["ok"].as_bool(), Some(true));
    }
    for worktree_path in worktree_paths {
        assert_eq!(std::fs::metadata(worktree_path).is_err(), true);
    }

    Ok(())
}
