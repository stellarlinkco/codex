use super::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct WaitArgs {
    ids: Vec<String>,
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
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: WaitArgs = parse_arguments(&arguments)?;
    if args.ids.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "ids must be non-empty".to_owned(),
        ));
    }
    let receiver_thread_ids = args
        .ids
        .iter()
        .map(|id| agent_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    let timeout_ms = normalize_wait_timeout(args.timeout_ms)?;

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

    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: receiver_thread_ids.clone(),
                receiver_agents,
                call_id: call_id.clone(),
            }
            .into(),
        )
        .await;

    let wait_result = match wait_for_agents(
        session.clone(),
        &receiver_thread_ids,
        timeout_ms,
        WaitMode::Any,
    )
    .await
    {
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
                        call_id: call_id.clone(),
                        agent_statuses,
                        statuses,
                    }
                    .into(),
                )
                .await;
            return Err(collab_agent_error(id, err));
        }
    };

    // Convert payload.
    let statuses_map = wait_result
        .statuses
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let result = WaitResult {
        status: statuses_map.clone(),
        timed_out: wait_result.timed_out,
    };

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

    // Final event emission.
    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id,
                agent_statuses,
                statuses: statuses_map,
            }
            .into(),
        )
        .await;

    let content = serde_json::to_string(&result).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize wait result: {err}"))
    })?;

    Ok(FunctionToolOutput::from_text(
        content, /*success*/ None,
    ))
}
