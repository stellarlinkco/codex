use super::*;
use crate::agent::next_thread_spawn_depth;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct ResumeAgentArgs {
    id: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ResumeAgentResult {
    pub(super) status: AgentStatus,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ResumeAgentArgs = parse_arguments(&arguments)?;
    let receiver_thread_id = agent_id(&args.id)?;
    let child_depth = next_thread_spawn_depth(&turn.session_source);
    if exceeds_thread_spawn_depth_limit(child_depth, turn.config.agent_max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }

    let (receiver_agent_nickname, receiver_agent_role) = session
        .services
        .agent_control
        .get_agent_nickname_and_role(receiver_thread_id)
        .await
        .unwrap_or((None, None));
    session
        .send_event(
            &turn,
            CollabResumeBeginEvent {
                call_id: call_id.clone(),
                sender_thread_id: session.conversation_id,
                receiver_thread_id,
                receiver_agent_nickname,
                receiver_agent_role,
            }
            .into(),
        )
        .await;

    let mut status = session
        .services
        .agent_control
        .get_status(receiver_thread_id)
        .await;
    let error = if matches!(status, AgentStatus::NotFound) {
        // If the thread is no longer active, attempt to restore it from rollout.
        match try_resume_closed_agent(&session, &turn, receiver_thread_id, child_depth).await {
            Ok(resumed_status) => {
                status = resumed_status;
                None
            }
            Err(err) => {
                status = session
                    .services
                    .agent_control
                    .get_status(receiver_thread_id)
                    .await;
                Some(err)
            }
        }
    } else {
        None
    };

    let (receiver_agent_nickname, receiver_agent_role) = session
        .services
        .agent_control
        .get_agent_nickname_and_role(receiver_thread_id)
        .await
        .unwrap_or((None, None));
    session
        .send_event(
            &turn,
            CollabResumeEndEvent {
                call_id,
                sender_thread_id: session.conversation_id,
                receiver_thread_id,
                receiver_agent_nickname,
                receiver_agent_role,
                status: status.clone(),
            }
            .into(),
        )
        .await;

    if let Some(err) = error {
        return Err(err);
    }

    let content = serde_json::to_string(&ResumeAgentResult { status }).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize resume_agent result: {err}"))
    })?;

    Ok(FunctionToolOutput::from_text(content, Some(true)))
}

async fn try_resume_closed_agent(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    receiver_thread_id: ThreadId,
    child_depth: i32,
) -> Result<AgentStatus, FunctionCallError> {
    let resume_result = session
        .services
        .agent_control
        .resume_agent_from_rollout(
            build_agent_resume_config(turn.as_ref(), child_depth)?,
            receiver_thread_id,
            thread_spawn_source(session.conversation_id, child_depth),
        )
        .await;
    let resumed_thread_id = match resume_result {
        Ok(thread_id) => Ok(thread_id),
        Err(err @ CodexErr::AgentLimitReached { .. }) => {
            if reap_finished_agents_for_slots(session.as_ref(), turn.as_ref(), /*slots*/ 1).await
                == 0
            {
                Err(err)
            } else {
                session
                    .services
                    .agent_control
                    .resume_agent_from_rollout(
                        build_agent_resume_config(turn.as_ref(), child_depth)?,
                        receiver_thread_id,
                        thread_spawn_source(session.conversation_id, child_depth),
                    )
                    .await
            }
        }
        Err(err) => Err(err),
    }
    .map_err(|err| collab_agent_error(receiver_thread_id, err))?;

    Ok(session
        .services
        .agent_control
        .get_status(resumed_thread_id)
        .await)
}
