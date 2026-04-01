use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct SendMessageArgs {
    to: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    items: Option<Vec<UserInput>>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    broadcast: bool,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
struct SendMessageDirectResult {
    submission_id: String,
}

#[derive(Debug, Serialize)]
struct SendMessageTeamMemberResult {
    team_id: String,
    member_name: String,
    agent_id: String,
    submission_id: String,
    delivered: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageAskLeadResult {
    team_id: String,
    lead_thread_id: String,
    submission_id: String,
    delivered: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageBroadcastSent {
    member_name: String,
    agent_id: String,
    submission_id: String,
}

#[derive(Debug, Serialize)]
struct SendMessageBroadcastFailed {
    member_name: String,
    agent_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct SendMessageBroadcastResult {
    team_id: String,
    sent: Vec<SendMessageBroadcastSent>,
    failed: Vec<SendMessageBroadcastFailed>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "route", rename_all = "snake_case")]
enum SendMessageResult {
    Direct(SendMessageDirectResult),
    TeamMember(SendMessageTeamMemberResult),
    AskLead(SendMessageAskLeadResult),
    Broadcast(SendMessageBroadcastResult),
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: SendMessageArgs = parse_arguments(&arguments)?;

    if args.broadcast {
        let team_id = args.team_id.clone().ok_or_else(|| {
            FunctionCallError::RespondToModel("team_id is required for broadcast".to_string())
        })?;
        return broadcast_to_team(session, turn, call_id, &team_id, args).await;
    }

    if let Some(team_id) = args.team_id.clone() {
        if args.to == "lead" {
            return ask_lead(session, turn, call_id, &team_id, args).await;
        }
        return message_team_member(session, turn, call_id, &team_id, args).await;
    }

    direct_send(session, turn, call_id, args).await
}

async fn direct_send(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    args: SendMessageArgs,
) -> Result<ToolOutput, FunctionCallError> {
    let receiver_thread_id = agent_id(&args.to)?;
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let submission_id = send_message_to_member(
        &session,
        &turn,
        call_id,
        receiver_thread_id,
        input_items,
        prompt,
        args.interrupt,
    )
    .await?;

    let content = serde_json::to_string(&SendMessageResult::Direct(SendMessageDirectResult {
        submission_id,
    }))
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_message result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}

async fn message_team_member(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    team_id: &str,
    args: SendMessageArgs,
) -> Result<ToolOutput, FunctionCallError> {
    let team_id = normalized_team_id(team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    let member = find_team_member(&team, &team_id, &args.to)?;

    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let delivery = send_message_to_member(
        &session,
        &turn,
        call_id,
        member.agent_id,
        input_items,
        prompt,
        args.interrupt,
    )
    .await;

    let (delivered, submission_id, error) = match delivery {
        Ok(submission_id) => (true, submission_id, None),
        Err(err) => (false, String::new(), Some(err.to_string())),
    };

    let content = serde_json::to_string(&SendMessageResult::TeamMember(
        SendMessageTeamMemberResult {
            team_id,
            member_name: member.name,
            agent_id: member.agent_id.to_string(),
            submission_id,
            delivered,
            error,
        },
    ))
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_message result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}

async fn broadcast_to_team(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    team_id: &str,
    args: SendMessageArgs,
) -> Result<ToolOutput, FunctionCallError> {
    let team_id = normalized_team_id(team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let mut sent = Vec::new();
    let mut failed = Vec::new();

    for member in &team.members {
        let member_call_id = format!("{call_id}:{}", member.name);
        match send_message_to_member(
            &session,
            &turn,
            member_call_id,
            member.agent_id,
            input_items.clone(),
            prompt.clone(),
            args.interrupt,
        )
        .await
        {
            Ok(submission_id) => sent.push(SendMessageBroadcastSent {
                member_name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                submission_id,
            }),
            Err(err) => failed.push(SendMessageBroadcastFailed {
                member_name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                error: err.to_string(),
            }),
        }
    }

    let content =
        serde_json::to_string(&SendMessageResult::Broadcast(SendMessageBroadcastResult {
            team_id,
            sent,
            failed,
        }))
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize send_message result: {err}"))
        })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}

async fn ask_lead(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    team_id: &str,
    args: SendMessageArgs,
) -> Result<ToolOutput, FunctionCallError> {
    let team_id = normalized_team_id(team_id)?;

    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    let sender_thread_id = session.conversation_id.to_string();
    if sender_thread_id == config.lead_thread_id {
        return Err(FunctionCallError::RespondToModel(
            "send_message cannot be called by the lead when to=lead".to_string(),
        ));
    }

    let _sender_name = config
        .members
        .iter()
        .find(|member| member.agent_id == sender_thread_id)
        .map(|member| member.name.as_str())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "thread `{}` is not a member of team `{team_id}`",
                session.conversation_id
            ))
        })?;
    let lead_thread_id = agent_id(&config.lead_thread_id)?;

    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let delivery = send_message_to_member(
        &session,
        &turn,
        call_id,
        lead_thread_id,
        input_items,
        prompt,
        args.interrupt,
    )
    .await;

    let (delivered, submission_id, error) = match delivery {
        Ok(submission_id) => (true, submission_id, None),
        Err(err) => (false, String::new(), Some(err.to_string())),
    };

    let content = serde_json::to_string(&SendMessageResult::AskLead(SendMessageAskLeadResult {
        team_id,
        lead_thread_id: lead_thread_id.to_string(),
        submission_id,
        delivered,
        error,
    }))
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_message result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
