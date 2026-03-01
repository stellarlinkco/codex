use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamTaskCompleteArgs {
    team_id: String,
    task_id: String,
}

#[derive(Debug, Serialize)]
struct TeamTaskCompleteResult {
    team_id: String,
    completed: bool,
    task: TeamTaskOutput,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskCompleteArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let task_id = required_path_segment(&args.task_id, "task_id")?;
    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    super::assert_team_member_or_lead(&team_id, &config, session.conversation_id)?;
    let caller_thread_id = session.conversation_id.to_string();
    let is_lead = caller_thread_id == config.lead_thread_id;
    let valid_member_agent_ids = config
        .members
        .iter()
        .map(|member| member.agent_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let _completion_lock = {
        let tasks_dir = team_tasks_dir(turn.config.codex_home.as_path(), &team_id);
        tokio::fs::create_dir_all(&tasks_dir)
            .await
            .map_err(|err| team_persistence_error("create team tasks directory", &team_id, err))?;
        let lock_path = tasks_dir.join(format!("{task_id}.complete.lock"));
        locks::lock_file_exclusive(&lock_path)
            .await
            .map_err(|err| team_persistence_error("lock team task completion", &team_id, err))?
    };
    let (task_id, task_title, assignee_name) = {
        let _lock = lock_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
        let task = read_team_task(turn.config.codex_home.as_path(), &team_id, task_id).await?;
        if !valid_member_agent_ids.contains(&task.assignee.agent_id) {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is assigned to a removed team member",
                task.id
            )));
        }
        if !is_lead && task.assignee.agent_id != caller_thread_id {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is assigned to another teammate",
                task.id
            )));
        }
        if task.state == PersistedTaskState::Completed {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is already completed",
                task.id
            )));
        }
        (task.id, task.title, task.assignee.name)
    };

    if let Some(err) = dispatch_task_completed_hook(
        session.as_ref(),
        turn.as_ref(),
        &team_id,
        &task_id,
        &task_title,
        Some(&assignee_name),
    )
    .await
    {
        return Err(FunctionCallError::RespondToModel(err));
    }

    let task = {
        let _lock = lock_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
        let mut task = read_team_task(turn.config.codex_home.as_path(), &team_id, &task_id).await?;
        if !valid_member_agent_ids.contains(&task.assignee.agent_id) {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is assigned to a removed team member",
                task.id
            )));
        }
        if !is_lead && task.assignee.agent_id != caller_thread_id {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is assigned to another teammate",
                task.id
            )));
        }
        if task.state == PersistedTaskState::Completed {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is already completed",
                task.id
            )));
        }
        task.state = PersistedTaskState::Completed;
        task.updated_at = now_unix_seconds();
        write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;
        task
    };

    let content = serde_json::to_string(&TeamTaskCompleteResult {
        team_id,
        completed: true,
        task: task.into(),
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize team_task_complete result: {err}"
        ))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
