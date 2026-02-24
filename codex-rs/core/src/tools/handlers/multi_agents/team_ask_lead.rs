use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamAskLeadArgs {
    team_id: String,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
struct TeamAskLeadResult {
    team_id: String,
    lead_thread_id: String,
    submission_id: String,
    delivered: bool,
    inbox_entry_id: String,
    error: Option<String>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamAskLeadArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;

    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    let sender_thread_id = session.conversation_id.to_string();
    if sender_thread_id == config.lead_thread_id {
        return Err(FunctionCallError::RespondToModel(
            "team_ask_lead cannot be called by the lead".to_string(),
        ));
    }

    let sender_name = config
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
    let inbox_entry_id = inbox::append_inbox_entry(
        turn.config.codex_home.as_path(),
        &team_id,
        lead_thread_id,
        session.conversation_id,
        Some(sender_name),
        &input_items,
        &prompt,
    )
    .await?;

    let delivery = send_input_to_member(
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

    let content = serde_json::to_string(&TeamAskLeadResult {
        team_id,
        lead_thread_id: lead_thread_id.to_string(),
        submission_id,
        delivered,
        inbox_entry_id,
        error,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_ask_lead result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
