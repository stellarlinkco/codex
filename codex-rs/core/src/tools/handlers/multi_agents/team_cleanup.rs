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
    let original_team = team.clone();
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
    let mut members_to_remove = Vec::new();
    for member in &selected_members {
        let status_before = session
            .services
            .agent_control
            .get_status(member.agent_id)
            .await;
        let close_result = if matches!(status_before, AgentStatus::Shutdown | AgentStatus::NotFound)
        {
            let _ = session
                .services
                .agent_control
                .shutdown_agent(member.agent_id)
                .await;
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
        if closed.last().is_some_and(|result| result.ok) {
            members_to_remove.push(member.name.clone());
        }
    }

    let mut removed_from_registry = false;
    let mut removed_team_config = false;
    let mut removed_task_dir = false;
    let mut persistence_error = None;
    if !members_to_remove.is_empty() {
        let remaining_team =
            remove_members_from_team(session.conversation_id, &team_id, &members_to_remove)?;
        let persistence_result = if let Some(team) = remaining_team.as_ref() {
            persist_team_state(
                turn.config.codex_home.as_path(),
                session.conversation_id,
                &team_id,
                team,
                None,
            )
            .await
        } else {
            remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await
        };
        match persistence_result {
            Ok(()) => {
                removed_from_registry = remaining_team.is_none();
                removed_team_config = removed_from_registry;
                removed_task_dir = removed_from_registry;
            }
            Err(err) => {
                let _ = restore_team_record(session.conversation_id, &team_id, original_team);
                persistence_error = Some(err);
            }
        }
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

    if let Some(err) = persistence_error {
        return Err(err);
    }

    let content = serde_json::to_string(&TeamCleanupResult {
        team_id: team_id.clone(),
        removed_from_registry,
        removed_team_config,
        removed_task_dir,
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
