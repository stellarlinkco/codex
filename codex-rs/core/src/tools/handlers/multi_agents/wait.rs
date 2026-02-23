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
) -> Result<ToolOutput, FunctionCallError> {
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

    session
        .send_event(
            &turn,
            CollabWaitingBeginEvent {
                sender_thread_id: session.conversation_id,
                receiver_thread_ids: receiver_thread_ids.clone(),
                receiver_agents: Vec::new(),
                receiver_names: HashMap::new(),
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
            let statuses =
                HashMap::from([(id, session.services.agent_control.get_status(id).await)]);
            session
                .send_event(
                    &turn,
                    CollabWaitingEndEvent {
                        sender_thread_id: session.conversation_id,
                        call_id: call_id.clone(),
                        agent_statuses: Vec::new(),
                        statuses,
                        receiver_names: HashMap::new(),
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

    // Final event emission.
    session
        .send_event(
            &turn,
            CollabWaitingEndEvent {
                sender_thread_id: session.conversation_id,
                call_id,
                agent_statuses: Vec::new(),
                statuses: statuses_map,
                receiver_names: HashMap::new(),
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
