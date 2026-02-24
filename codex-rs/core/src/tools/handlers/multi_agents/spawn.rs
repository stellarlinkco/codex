use super::*;
use crate::agent::role::apply_role_to_config;

use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    agent_type: Option<String>,
    model_provider: Option<String>,
    model: Option<String>,
    #[serde(default)]
    worktree: bool,
    #[serde(default, alias = "backendground")]
    background: bool,
}

#[derive(Debug, Serialize)]
struct SpawnAgentResult {
    agent_id: String,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnAgentArgs = parse_arguments(&arguments)?;
    let role_name = args
        .agent_type
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());
    let model_provider = optional_non_empty(&args.model_provider, "model_provider")?;
    let model = optional_non_empty(&args.model, "model")?;
    let use_worktree = args.worktree;
    let _background = args.background;
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let session_source = turn.session_source.clone();
    let child_depth = next_thread_spawn_depth(&session_source);
    if exceeds_thread_spawn_depth_limit(child_depth, turn.config.agent_max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }
    session
        .send_event(
            &turn,
            CollabAgentSpawnBeginEvent {
                call_id: call_id.clone(),
                sender_thread_id: session.conversation_id,
                prompt: prompt.clone(),
            }
            .into(),
        )
        .await;
    let mut config = build_agent_spawn_config(
        &session.get_base_instructions().await,
        turn.as_ref(),
        child_depth,
    )?;
    apply_role_to_config(&mut config, role_name)
        .await
        .map_err(FunctionCallError::RespondToModel)?;
    apply_member_model_overrides(&mut config, model_provider, model)?;
    apply_spawn_agent_overrides(&mut config, child_depth);
    let worktree_lease = if use_worktree {
        match create_agent_worktree(&session, &turn).await {
            Ok(lease) => {
                config.cwd = lease.worktree_path.clone();
                Some(lease)
            }
            Err(err) => {
                session
                    .send_event(
                        &turn,
                        CollabAgentSpawnEndEvent {
                            call_id,
                            sender_thread_id: session.conversation_id,
                            new_thread_id: None,
                            new_agent_nickname: None,
                            new_agent_role: None,
                            prompt,
                            status: AgentStatus::NotFound,
                        }
                        .into(),
                    )
                    .await;
                return Err(err);
            }
        }
    } else {
        None
    };
    let result = session
        .services
        .agent_control
        .spawn_agent_thread(
            config,
            Some(thread_spawn_source(session.conversation_id, child_depth)),
        )
        .await
        .map_err(collab_spawn_error);

    let (agent_id, notification_source) = match result {
        Ok((agent_id, notification_source)) => (agent_id, notification_source),
        Err(err) => {
            if let Some(lease) = worktree_lease {
                let _ = remove_worktree_lease(&session, &turn, lease).await;
            }
            session
                .send_event(
                    &turn,
                    CollabAgentSpawnEndEvent {
                        call_id,
                        sender_thread_id: session.conversation_id,
                        new_thread_id: None,
                        new_agent_nickname: None,
                        new_agent_role: None,
                        prompt,
                        status: AgentStatus::NotFound,
                    }
                    .into(),
                )
                .await;
            return Err(err);
        }
    };

    let hook_context = dispatch_subagent_start_hook(
        session.as_ref(),
        turn.as_ref(),
        agent_id,
        role_name.unwrap_or("default"),
    )
    .await;
    if !hook_context.is_empty() {
        let injected = hook_context.join("\n\n");
        if let Err(err) = session
            .services
            .agent_control
            .inject_developer_message_without_turn(agent_id, injected)
            .await
        {
            warn!("failed to inject subagent_start hook context: {err}");
        }
    }

    if let Err(err) = session
        .services
        .agent_control
        .send_spawn_input(agent_id, input_items, notification_source)
        .await
    {
        if let Some(lease) = worktree_lease {
            let _ = remove_worktree_lease(&session, &turn, lease).await;
        }
        let _ = session
            .services
            .agent_control
            .shutdown_agent(agent_id)
            .await;
        session
            .send_event(
                &turn,
                CollabAgentSpawnEndEvent {
                    call_id,
                    sender_thread_id: session.conversation_id,
                    new_thread_id: None,
                    new_agent_nickname: None,
                    new_agent_role: None,
                    prompt,
                    status: AgentStatus::NotFound,
                }
                .into(),
            )
            .await;
        return Err(collab_spawn_error(err));
    }

    if let Some(lease) = worktree_lease {
        register_worktree_lease(agent_id, lease);
    }

    let status = session.services.agent_control.get_status(agent_id).await;
    session
        .send_event(
            &turn,
            CollabAgentSpawnEndEvent {
                call_id,
                sender_thread_id: session.conversation_id,
                new_thread_id: Some(agent_id),
                new_agent_nickname: None,
                new_agent_role: None,
                prompt,
                status,
            }
            .into(),
        )
        .await;

    let content = serde_json::to_string(&SpawnAgentResult {
        agent_id: agent_id.to_string(),
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize spawn_agent result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
