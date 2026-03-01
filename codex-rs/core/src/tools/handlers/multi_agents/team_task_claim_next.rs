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
    let member_name = optional_non_empty(&args.member_name, "member_name")?;
    let target_member = if is_lead {
        member_name
            .map(|member_name| {
                config
                    .members
                    .iter()
                    .find(|member| member.name == member_name)
                    .cloned()
                    .ok_or_else(|| {
                        FunctionCallError::RespondToModel(format!(
                            "member `{member_name}` not found in team `{team_id}`"
                        ))
                    })
            })
            .transpose()?
    } else {
        let caller_member = config
            .members
            .iter()
            .find(|member| member.agent_id == caller_thread_id)
            .cloned()
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "thread `{}` is not a member of team `{team_id}`",
                    session.conversation_id
                ))
            })?;
        if member_name.is_some_and(|member_name| member_name != caller_member.name) {
            return Err(FunctionCallError::RespondToModel(format!(
                "member_name must be `{}` when invoked by this teammate",
                caller_member.name
            )));
        }
        Some(caller_member)
    };

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
                || candidate.assignee.agent_id != member.agent_id)
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
