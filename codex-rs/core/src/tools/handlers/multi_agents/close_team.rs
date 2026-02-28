use super::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct CloseTeamArgs {
    team_id: String,
    members: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct CloseTeamMemberResult {
    name: String,
    agent_id: String,
    ok: bool,
    status: AgentStatus,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CloseTeamResult {
    team_id: String,
    closed: Vec<CloseTeamMemberResult>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: CloseTeamArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    if team.members.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "team `{team_id}` has no members"
        )));
    }
    let original_team = team.clone();

    let selected_names = match args.members {
        Some(names) => {
            if names.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "members must be non-empty when provided".to_string(),
                ));
            }
            let mut selected = HashSet::new();
            for name in names {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "member name must be non-empty".to_string(),
                    ));
                }
                selected.insert(name);
            }
            selected
        }
        None => team
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect(),
    };

    let selected_members = team
        .members
        .iter()
        .filter(|member| selected_names.contains(&member.name))
        .cloned()
        .collect::<Vec<_>>();
    if selected_members.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "no matching team members found".to_string(),
        ));
    }

    let event_call_id = prefixed_team_call_id(TEAM_CLOSE_CALL_PREFIX, &call_id);
    let receiver_names = team_member_names(&selected_members);
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
            (Ok(_), None) => closed.push(CloseTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: true,
                status: status_before,
                error: None,
            }),
            (Ok(_), Some(cleanup_err)) => closed.push(CloseTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(cleanup_err),
            }),
            (Err(err), None) => closed.push(CloseTeamMemberResult {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                ok: false,
                status: status_before,
                error: Some(format!("{err}")),
            }),
            (Err(err), Some(cleanup_err)) => closed.push(CloseTeamMemberResult {
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
            let empty_team = TeamRecord {
                members: Vec::new(),
                created_at: original_team.created_at,
            };
            persist_team_state(
                turn.config.codex_home.as_path(),
                session.conversation_id,
                &team_id,
                &empty_team,
                None,
            )
            .await
        };
        if let Err(err) = persistence_result {
            let _ = restore_team_record(session.conversation_id, &team_id, original_team);
            persistence_error = Some(err);
        }
    };

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

    let content = serde_json::to_string(&CloseTeamResult { team_id, closed }).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize close_team result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
