use super::*;
use std::sync::Arc;

const DEFAULT_INBOX_POP_LIMIT: usize = 50;
const MAX_INBOX_POP_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
struct TeamInboxPopArgs {
    team_id: String,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxMessage {
    id: String,
    created_at: i64,
    from_thread_id: String,
    from_name: Option<String>,
    input_items: Vec<UserInput>,
    prompt: String,
}

impl From<inbox::TeamInboxEntry> for TeamInboxMessage {
    fn from(value: inbox::TeamInboxEntry) -> Self {
        Self {
            id: value.id,
            created_at: value.created_at,
            from_thread_id: value.from_thread_id,
            from_name: value.from_name,
            input_items: value.input_items,
            prompt: value.prompt,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamInboxPopResult {
    team_id: String,
    thread_id: String,
    messages: Vec<TeamInboxMessage>,
    ack_token: String,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamInboxPopArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;

    let config =
        super::read_persisted_team_config(turn.config.codex_home.as_path(), &team_id).await?;
    super::assert_team_member_or_lead(&team_id, &config, session.conversation_id)?;

    let limit = args
        .limit
        .unwrap_or(DEFAULT_INBOX_POP_LIMIT)
        .clamp(1, MAX_INBOX_POP_LIMIT);
    let (entries, ack_token) = inbox::pop_inbox_entries(
        turn.config.codex_home.as_path(),
        &team_id,
        session.conversation_id,
        limit,
    )
    .await?;

    let ack_token = ack_token
        .map(|token| serde_json::to_string(&token))
        .transpose()
        .map_err(|err| FunctionCallError::Fatal(format!("failed to serialize ack_token: {err}")))?
        .unwrap_or_default();

    let content = serde_json::to_string(&TeamInboxPopResult {
        team_id,
        thread_id: session.conversation_id.to_string(),
        messages: entries.into_iter().map(TeamInboxMessage::from).collect(),
        ack_token,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_inbox_pop result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
