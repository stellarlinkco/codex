use super::*;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamTaskClaimArgs {
    team_id: String,
    task_id: String,
}

#[derive(Debug, Serialize)]
struct TeamTaskClaimResult {
    team_id: String,
    claimed: bool,
    task: TeamTaskOutput,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskClaimArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    super::assert_team_member_or_lead(&team_id, &config, session.conversation_id)?;
    let caller_thread_id = session.conversation_id.to_string();
    let is_lead = caller_thread_id == config.lead_thread_id;
    let valid_member_agent_ids = config
        .members
        .iter()
        .map(|member| member.agent_id.clone())
        .collect::<HashSet<_>>();
    let _lock = lock_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
    let mut task =
        read_team_task(turn.config.codex_home.as_path(), &team_id, &args.task_id).await?;
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

    match task.state {
        PersistedTaskState::Pending => {}
        PersistedTaskState::Claimed => {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is already claimed",
                task.id
            )));
        }
        PersistedTaskState::Completed => {
            return Err(FunctionCallError::RespondToModel(format!(
                "task `{}` is already completed",
                task.id
            )));
        }
    }

    let tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
    if !dependencies_satisfied(&task, &tasks) {
        return Err(FunctionCallError::RespondToModel(format!(
            "task `{}` has unresolved dependencies",
            task.id
        )));
    }

    task.state = PersistedTaskState::Claimed;
    task.updated_at = now_unix_seconds();
    write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;

    let content = serde_json::to_string(&TeamTaskClaimResult {
        team_id,
        claimed: true,
        task: task.into(),
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_task_claim result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
