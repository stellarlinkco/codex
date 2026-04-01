use super::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WaitModeArg {
    Any,
    All,
}

#[derive(Debug, Deserialize)]
struct WaitArgs {
    ids: Option<Vec<String>>,
    team_id: Option<String>,
    mode: Option<WaitModeArg>,
    timeout_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct WaitResult {
    status: HashMap<ThreadId, AgentStatus>,
    timed_out: bool,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: WaitArgs = parse_arguments(&arguments)?;
    let wait_mode = match args.mode {
        Some(WaitModeArg::Any) => WaitMode::Any,
        Some(WaitModeArg::All) => WaitMode::All,
        None if args.team_id.is_some() => WaitMode::All,
        None => WaitMode::Any,
    };

    let (receiver_thread_ids, event_call_id, receiver_agents_from_team, team_id) =
        if let Some(team_id) = args.team_id.as_deref() {
            if args.ids.is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "ids must not be provided when team_id is set".to_string(),
                ));
            }
            let team_id = normalized_team_id(team_id)?;
            let team = get_team_record(session.conversation_id, &team_id)?;
            if team.members.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "team `{team_id}` has no members"
                )));
            }
            (
                team.members.iter().map(|member| member.agent_id).collect(),
                prefixed_team_call_id(TEAM_WAIT_CALL_PREFIX, &call_id),
                team_member_refs(&team.members),
                Some(team_id),
            )
        } else {
            let Some(ids) = args.ids.as_ref() else {
                return Err(FunctionCallError::RespondToModel(
                    "ids must be non-empty".to_owned(),
                ));
            };
            if ids.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "ids must be non-empty".to_owned(),
                ));
            }
            let receiver_thread_ids = ids
                .iter()
                .map(|id| agent_id(id))
                .collect::<Result<Vec<_>, _>>()?;
            (receiver_thread_ids, call_id.clone(), Vec::new(), None)
        };

    let timeout_ms = normalize_wait_timeout(args.timeout_ms)?;

    let receiver_agents = if !receiver_agents_from_team.is_empty() {
        receiver_agents_from_team
    } else {
        let mut receiver_agents = Vec::with_capacity(receiver_thread_ids.len());
        for receiver_thread_id in &receiver_thread_ids {
            let (agent_nickname, agent_role) = session
                .services
                .agent_control
                .get_agent_nickname_and_role(*receiver_thread_id)
                .await
                .unwrap_or((None, None));
            receiver_agents.push(CollabAgentRef {
                thread_id: *receiver_thread_id,
                agent_nickname,
                agent_role,
            });
        }
        receiver_agents
    };

    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: receiver_thread_ids.clone(),
                receiver_agents,
                call_id: event_call_id.clone(),
            }
            .into(),
        )
        .await;

    let wait_result =
        match wait_for_agents(session.clone(), &receiver_thread_ids, timeout_ms, wait_mode).await {
            Ok(result) => result,
            Err((id, err)) => {
                let status = session.services.agent_control.get_status(id).await;
                let (agent_nickname, agent_role) = session
                    .services
                    .agent_control
                    .get_agent_nickname_and_role(id)
                    .await
                    .unwrap_or((None, None));
                let statuses = HashMap::from([(id, status.clone())]);
                let agent_statuses = vec![CollabAgentStatusEntry {
                    thread_id: id,
                    agent_nickname,
                    agent_role,
                    status,
                }];
                session
                    .send_event(
                        &turn,
                        CollabWaitingEndEvent {
                            sender_thread_id: session.conversation_id,
                            call_id: event_call_id.clone(),
                            agent_statuses,
                            statuses,
                        }
                        .into(),
                    )
                    .await;
                return Err(collab_agent_error(id, err));
            }
        };

    let statuses_map = wait_result
        .statuses
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let (reported_statuses, agent_statuses) = if let Some(team_id) = team_id.as_deref() {
        let team = get_team_record(session.conversation_id, team_id)?;
        let mut reported_statuses = statuses_map.clone();
        for member in &team.members {
            if reported_statuses.contains_key(&member.agent_id) {
                continue;
            }
            let status = session
                .services
                .agent_control
                .get_status(member.agent_id)
                .await;
            reported_statuses.insert(member.agent_id, status);
        }

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
                dispatch_teammate_idle_hook(session.as_ref(), turn.as_ref(), team_id, &member.name)
                    .await
            {
                return Err(FunctionCallError::RespondToModel(err));
            }
        }

        (
            reported_statuses.clone(),
            team_member_status_entries(&team.members, &reported_statuses),
        )
    } else {
        let mut agent_statuses = Vec::with_capacity(statuses_map.len());
        for receiver_thread_id in &receiver_thread_ids {
            let Some(status) = statuses_map.get(receiver_thread_id) else {
                continue;
            };
            let (agent_nickname, agent_role) = session
                .services
                .agent_control
                .get_agent_nickname_and_role(*receiver_thread_id)
                .await
                .unwrap_or((None, None));
            agent_statuses.push(CollabAgentStatusEntry {
                thread_id: *receiver_thread_id,
                agent_nickname,
                agent_role,
                status: status.clone(),
            });
        }
        (statuses_map.clone(), agent_statuses)
    };

    let result = WaitResult {
        status: reported_statuses.clone(),
        timed_out: wait_result.timed_out,
    };

    // Final event emission.
    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id: event_call_id,
                agent_statuses,
                statuses: reported_statuses,
            }
            .into(),
        )
        .await;

    let content = serde_json::to_string(&result).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize wait result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: None,
    })
}
