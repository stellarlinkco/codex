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
    let _ = get_team_record(session.conversation_id, &team_id)?;
    let (task_id, task_title, assignee_name) = {
        let _lock = lock_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
        let task =
            read_team_task(turn.config.codex_home.as_path(), &team_id, &args.task_id).await?;
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
        let mut task =
            read_team_task(turn.config.codex_home.as_path(), &team_id, &args.task_id).await?;
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
