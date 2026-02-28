use super::*;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamTaskListArgs {
    team_id: String,
}

#[derive(Debug, Serialize)]
struct TeamTaskListResult {
    team_id: String,
    tasks: Vec<TeamTaskOutput>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamTaskListArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    super::assert_team_member_or_lead(&team_id, &config, session.conversation_id)?;
    let valid_member_agent_ids = config
        .members
        .iter()
        .map(|member| member.agent_id.clone())
        .collect::<HashSet<_>>();

    let tasks = read_team_tasks(turn.config.codex_home.as_path(), &team_id)
        .await?
        .into_iter()
        .filter(|task| valid_member_agent_ids.contains(&task.assignee.agent_id))
        .map(TeamTaskOutput::from)
        .collect::<Vec<_>>();

    let content = serde_json::to_string(&TeamTaskListResult { team_id, tasks }).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_task_list result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
