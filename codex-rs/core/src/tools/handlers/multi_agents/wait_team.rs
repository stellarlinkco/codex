use super::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WaitTeamModeArg {
    Any,
    All,
}

#[derive(Debug, Deserialize)]
struct WaitTeamArgs {
    team_id: String,
    mode: Option<WaitTeamModeArg>,
    timeout_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum WaitTeamMode {
    Any,
    All,
}

#[derive(Debug, Serialize)]
struct WaitTeamTriggeredMember {
    name: String,
    agent_id: String,
}

#[derive(Debug, Serialize)]
struct WaitTeamMemberStatus {
    name: String,
    agent_id: String,
    state: AgentStatus,
}

#[derive(Debug, Serialize)]
struct WaitTeamResult {
    completed: bool,
    mode: WaitTeamMode,
    triggered_member: Option<WaitTeamTriggeredMember>,
    member_statuses: Vec<WaitTeamMemberStatus>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: WaitTeamArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    if team.members.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "team `{team_id}` has no members"
        )));
    }

    let (wait_mode, output_mode) = match args.mode.unwrap_or(WaitTeamModeArg::All) {
        WaitTeamModeArg::Any => (WaitMode::Any, WaitTeamMode::Any),
        WaitTeamModeArg::All => (WaitMode::All, WaitTeamMode::All),
    };
    let timeout_ms = normalize_wait_timeout(args.timeout_ms)?;
    let receiver_thread_ids = team
        .members
        .iter()
        .map(|member| member.agent_id)
        .collect::<Vec<_>>();
    let receiver_names = team_member_names(&team.members);
    let event_call_id = prefixed_team_call_id(TEAM_WAIT_CALL_PREFIX, &call_id);

    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: receiver_thread_ids.clone(),
                receiver_agents: Vec::new(),
                receiver_names: receiver_names.clone(),
                call_id: event_call_id.clone(),
            }
            .into(),
        )
        .await;

    let wait_result =
        match wait_for_agents(session.clone(), &receiver_thread_ids, timeout_ms, wait_mode).await {
            Ok(result) => result,
            Err((id, err)) => {
                let statuses =
                    HashMap::from([(id, session.services.agent_control.get_status(id).await)]);
                session
                    .send_event(
                        &turn,
                        CollabWaitingEndEvent {
                            sender_thread_id: session.conversation_id,
                            call_id: event_call_id,
                            agent_statuses: Vec::new(),
                            statuses,
                            receiver_names: receiver_names.clone(),
                        }
                        .into(),
                    )
                    .await;
                return Err(collab_agent_error(id, err));
            }
        };

    let final_statuses = wait_result
        .statuses
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id: event_call_id,
                agent_statuses: Vec::new(),
                statuses: final_statuses.clone(),
                receiver_names: receiver_names.clone(),
            }
            .into(),
        )
        .await;

    for (agent_id, state) in &wait_result.statuses {
        if !crate::agent::status::is_final(state) {
            continue;
        }
        let Some(member) = team
            .members
            .iter()
            .find(|candidate| candidate.agent_id == *agent_id)
        else {
            continue;
        };
        if let Some(err) =
            dispatch_teammate_idle_hook(session.as_ref(), turn.as_ref(), &team_id, &member.name)
                .await
        {
            return Err(FunctionCallError::RespondToModel(err));
        }
    }

    let mut member_statuses = Vec::with_capacity(team.members.len());
    for member in &team.members {
        let state = match final_statuses.get(&member.agent_id) {
            Some(state) => state.clone(),
            None => {
                session
                    .services
                    .agent_control
                    .get_status(member.agent_id)
                    .await
            }
        };
        member_statuses.push(WaitTeamMemberStatus {
            name: member.name.clone(),
            agent_id: member.agent_id.to_string(),
            state,
        });
    }

    let triggered_member = if wait_mode == WaitMode::Any && !wait_result.statuses.is_empty() {
        let (triggered_id, _) = wait_result.statuses[0];
        team.members
            .iter()
            .find(|member| member.agent_id == triggered_id)
            .map(|member| WaitTeamTriggeredMember {
                name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
            })
    } else {
        None
    };

    let completed = match wait_mode {
        WaitMode::Any => !wait_result.timed_out && !wait_result.statuses.is_empty(),
        WaitMode::All => {
            !wait_result.timed_out
                && member_statuses
                    .iter()
                    .all(|entry| crate::agent::status::is_final(&entry.state))
        }
    };

    let content = serde_json::to_string(&WaitTeamResult {
        completed,
        mode: output_mode,
        triggered_member,
        member_statuses,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize wait_team result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
