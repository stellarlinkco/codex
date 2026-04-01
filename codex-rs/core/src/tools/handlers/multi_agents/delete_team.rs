use super::*;
use std::collections::HashMap;
use std::sync::Arc;

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct DeleteTeamArgs {
    team_id: String,
    #[serde(default = "default_true")]
    cleanup: bool,
}

#[derive(Debug, Serialize)]
struct DeleteTeamMemberResult {
    name: String,
    agent_id: String,
    ok: bool,
    status: AgentStatus,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeleteTeamResult {
    team_id: String,
    removed_from_registry: bool,
    removed_team_config: bool,
    removed_task_dir: bool,
    closed: Vec<DeleteTeamMemberResult>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: DeleteTeamArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;

    if let Some(active_team_id) = find_team_for_member(session.conversation_id)? {
        return Err(FunctionCallError::RespondToModel(format!(
            "delete_team is disabled for agent team teammates (team `{active_team_id}`). Ask the team lead to delete teams."
        )));
    }

    let existing_team = get_team_record(session.conversation_id, &team_id).ok();
    let persisted_config = if existing_team.is_some() {
        read_persisted_team_config(turn.config.codex_home.as_path(), &team_id)
            .await
            .ok()
    } else {
        Some(read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?)
    };
    if let Some(config) = persisted_config.as_ref()
        && session.conversation_id.to_string() != config.lead_thread_id
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "delete_team must be run by the lead thread `{}`",
            config.lead_thread_id
        )));
    }

    let original_team = existing_team.clone();
    let members = match existing_team {
        Some(team) => team.members,
        None => {
            let config = persisted_config.as_ref().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!("team `{team_id}` not found"))
            })?;
            config
                .members
                .iter()
                .map(|member| {
                    Ok(TeamMember {
                        name: member.name.clone(),
                        agent_id: agent_id(&member.agent_id)?,
                        agent_type: member.agent_type.clone(),
                    })
                })
                .collect::<Result<Vec<_>, FunctionCallError>>()?
        }
    };

    let event_call_id = prefixed_team_call_id(TEAM_CLOSE_CALL_PREFIX, &call_id);
    let receiver_agents = team_member_refs(&members);
    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: members.iter().map(|member| member.agent_id).collect(),
                receiver_agents: receiver_agents.clone(),
                call_id: event_call_id.clone(),
            }
            .into(),
        )
        .await;

    let mut statuses = HashMap::new();
    let mut closed = Vec::with_capacity(members.len());
    for member in &members {
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
                .map_err(|err| format!("{err}"))
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
            (Ok(_), None) => closed.push(DeleteTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: true,
                status: status_before,
                error: None,
            }),
            (Ok(_), Some(cleanup_err)) => closed.push(DeleteTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(cleanup_err),
            }),
            (Err(err), None) => closed.push(DeleteTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(err),
            }),
            (Err(err), Some(cleanup_err)) => closed.push(DeleteTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(format!("{err}; {cleanup_err}")),
            }),
        }
    }

    remove_team_record(session.conversation_id, &team_id)?;
    if args.cleanup
        && let Err(err) = remove_team_persistence(turn.config.codex_home.as_path(), &team_id).await
    {
        if let Some(original_team) = original_team {
            let _ = restore_team_record(session.conversation_id, &team_id, original_team);
        }
        return Err(err);
    }

    let agent_statuses = team_member_status_entries(&members, &statuses);
    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id: event_call_id,
                agent_statuses,
                statuses,
            }
            .into(),
        )
        .await;

    let content = serde_json::to_string(&DeleteTeamResult {
        team_id,
        removed_from_registry: true,
        removed_team_config: args.cleanup,
        removed_task_dir: args.cleanup,
        closed,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize delete_team result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
