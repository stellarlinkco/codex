use super::*;
use std::collections::HashMap;
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
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamCleanupArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    let selected_members = team.members.clone();
    let receiver_names = team_member_names(&selected_members);
    let event_call_id = prefixed_team_call_id(TEAM_CLOSE_CALL_PREFIX, &call_id);
    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: selected_members
                    .iter()
                    .map(|member| member.agent_id)
                    .collect(),
                receiver_agents: Vec::new(),
                receiver_names: receiver_names.clone(),
                call_id: event_call_id.clone(),
            }
            .into(),
        )
        .await;

    let mut statuses = HashMap::new();
    let mut closed = Vec::with_capacity(selected_members.len());
    for member in &selected_members {
        let status_before = session
            .services
            .agent_control
            .get_status(member.agent_id)
            .await;
        let close_result = if matches!(status_before, AgentStatus::Shutdown | AgentStatus::NotFound)
        {
            Ok(String::new())
        } else {
            session
                .services
                .agent_control
                .shutdown_agent(member.agent_id)
                .await
        };
        let status_after = session
            .services
            .agent_control
            .get_status(member.agent_id)
            .await;
        let event_status = match (&status_before, &close_result, status_after) {
            (_, Err(_), status_after) => status_after,
            (AgentStatus::NotFound, Ok(_), _) => AgentStatus::NotFound,
            (AgentStatus::Shutdown, Ok(_), _) => AgentStatus::Shutdown,
            (_, Ok(_), AgentStatus::NotFound) => AgentStatus::Shutdown,
            (_, Ok(_), status_after) => status_after,
        };
        statuses.insert(member.agent_id, event_status);

        let cleanup_error =
            cleanup_agent_worktree(session.as_ref(), turn.as_ref(), member.agent_id)
                .await
                .err();
        match (close_result, cleanup_error) {
            (Ok(_), None) => closed.push(TeamCleanupMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: true,
                status: status_before,
                error: None,
            }),
            (Ok(_), Some(cleanup_err)) => closed.push(TeamCleanupMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(cleanup_err),
            }),
            (Err(err), None) => closed.push(TeamCleanupMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(err.to_string()),
            }),
            (Err(err), Some(cleanup_err)) => closed.push(TeamCleanupMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(format!("{err}; {cleanup_err}")),
            }),
        }
    }

    let remaining = remove_members_from_team(
        session.conversation_id,
        &team_id,
        &selected_members
            .iter()
            .map(|member| member.name.clone())
            .collect::<Vec<_>>(),
    )?;
    let removed_from_registry = remaining.is_none();
    if removed_from_registry {
        remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await?;
    }

    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id: event_call_id,
                agent_statuses: Vec::new(),
                statuses,
                receiver_names,
            }
            .into(),
        )
        .await;

    let content = serde_json::to_string(&TeamCleanupResult {
        team_id: team_id.clone(),
        removed_from_registry,
        removed_team_config: removed_from_registry,
        removed_task_dir: removed_from_registry,
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
