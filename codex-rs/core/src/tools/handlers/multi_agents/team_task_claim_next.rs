use super::*;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamTaskClaimNextArgs {
    team_id: String,
    member_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct TeamTaskClaimNextResult {
    team_id: String,
    claimed: bool,
    task: Option<TeamTaskOutput>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskClaimNextArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    let valid_member_agent_ids = team
        .members
        .iter()
        .map(|member| member.agent_id.to_string())
        .collect::<HashSet<_>>();
    let _lock = lock_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
    let target_member = args
        .member_name
        .as_deref()
        .map(|member_name| find_team_member(&team, &team_id, member_name))
        .transpose()?;

    let mut tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id).await?;
    let mut selected_index = None;
    for index in 0..tasks.len() {
        let candidate = &tasks[index];
        if candidate.state != PersistedTaskState::Pending {
            continue;
        }
        if !valid_member_agent_ids.contains(&candidate.assignee.agent_id) {
            continue;
        }
        if let Some(member) = target_member.as_ref()
            && (candidate.assignee.name != member.name
                || candidate.assignee.agent_id != member.agent_id.to_string())
        {
            continue;
        }
        if dependencies_satisfied(candidate, &tasks) {
            selected_index = Some(index);
            break;
        }
    }

    let result = if let Some(index) = selected_index {
        let mut task = tasks.swap_remove(index);
        task.state = PersistedTaskState::Claimed;
        task.updated_at = now_unix_seconds();
        write_team_task(turn.config.codex_home.as_path(), &team_id, &task).await?;
        TeamTaskClaimNextResult {
            team_id,
            claimed: true,
            task: Some(task.into()),
        }
    } else {
        TeamTaskClaimNextResult {
            team_id,
            claimed: false,
            task: None,
        }
    };

    let content = serde_json::to_string(&result).map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize team_task_claim_next result: {err}"
        ))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
