use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamCleanupArgs {
    team_id: String,
}

#[derive(Debug, Serialize)]
struct TeamCleanupMemberResult {
    name: String,
    agent_id: String,
    ok: bool,
    status: AgentStatus,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TeamCleanupResult {
    team_id: String,
    removed_from_registry: bool,
    removed_team_config: bool,
    removed_task_dir: bool,
    closed: Vec<TeamCleanupMemberResult>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamCleanupArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    let caller_thread_id = session.conversation_id.to_string();
    if caller_thread_id != config.lead_thread_id {
        return Err(FunctionCallError::RespondToModel(format!(
            "team_cleanup must be run by the lead thread `{}`",
            config.lead_thread_id
        )));
    }

    let mut blocked = Vec::new();
    let mut closed = Vec::with_capacity(config.members.len());
    for member in &config.members {
        let member_id = super::agent_id(&member.agent_id)?;
        let status = session.services.agent_control.get_status(member_id).await;
        let ok = matches!(status, AgentStatus::Shutdown | AgentStatus::NotFound);
        if !ok {
            blocked.push(format!(
                "{} ({}) is {status:?}",
                member.name, member.agent_id
            ));
        }
        closed.push(TeamCleanupMemberResult {
            name: member.name.clone(),
            agent_id: member.agent_id.clone(),
            ok,
            status,
            error: None,
        });
    }
    if !blocked.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "team_cleanup found active teammates; close them first: {}",
            blocked.join(", ")
        )));
    }

    remove_team_record(session.conversation_id, &team_id)?;
    remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await?;

    let content = serde_json::to_string(&TeamCleanupResult {
        team_id,
        removed_from_registry: true,
        removed_team_config: true,
        removed_task_dir: true,
        closed,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_cleanup result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
