use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamInboxAckArgs {
    team_id: String,
    ack_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxAckResult {
    team_id: String,
    thread_id: String,
    acked: bool,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamInboxAckArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;

    if args.ack_token.trim().is_empty() {
        let content = serde_json::to_string(&TeamInboxAckResult {
            team_id,
            thread_id: session.conversation_id.to_string(),
            acked: false,
        })
        .map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize team_inbox_ack result: {err}"))
        })?;
        return Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(content),
            success: Some(true),
        });
    }

    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    super::assert_team_member_or_lead(&team_id, &config, session.conversation_id)?;

    let token: inbox::TeamInboxAckToken = serde_json::from_str(&args.ack_token)
        .map_err(|err| FunctionCallError::RespondToModel(format!("ack_token is invalid: {err}")))?;
    if token.team_id != team_id {
        return Err(FunctionCallError::RespondToModel(
            "ack_token team_id mismatch".to_string(),
        ));
    }
    if token.thread_id != session.conversation_id.to_string() {
        return Err(FunctionCallError::RespondToModel(
            "ack_token thread_id mismatch".to_string(),
        ));
    }

    inbox::ack_inbox(turn.config.codex_home.as_path(), &token).await?;

    let content = serde_json::to_string(&TeamInboxAckResult {
        team_id,
        thread_id: session.conversation_id.to_string(),
        acked: true,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_inbox_ack result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
