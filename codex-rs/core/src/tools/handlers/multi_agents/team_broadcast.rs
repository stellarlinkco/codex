use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct TeamBroadcastArgs {
    team_id: String,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
struct TeamBroadcastSent {
    member_name: String,
    agent_id: String,
    submission_id: String,
    inbox_entry_id: String,
}

#[derive(Debug, Serialize)]
struct TeamBroadcastFailed {
    member_name: String,
    agent_id: String,
    inbox_entry_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct TeamBroadcastResult {
    team_id: String,
    sent: Vec<TeamBroadcastSent>,
    failed: Vec<TeamBroadcastFailed>,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamBroadcastArgs = parse_arguments(&arguments)?;
    let team_id = normalized_team_id(&args.team_id)?;
    let team = get_team_record(session.conversation_id, &team_id)?;
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let mut sent = Vec::new();
    let mut failed = Vec::new();

    for member in &team.members {
        let member_call_id = format!("{call_id}:{}", member.name);
        let inbox_entry_id = match inbox::append_inbox_entry(
            turn.config.codex_home.as_path(),
            &team_id,
            member.agent_id,
            session.conversation_id,
            Some("lead"),
            &input_items,
            &prompt,
        )
        .await
        {
            Ok(entry_id) => entry_id,
            Err(err) => {
                failed.push(TeamBroadcastFailed {
                    member_name: member.name.clone(),
                    agent_id: member.agent_id.to_string(),
                    inbox_entry_id: String::new(),
                    error: err.to_string(),
                });
                continue;
            }
        };

        match send_input_to_member(
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
            Ok(submission_id) => sent.push(TeamBroadcastSent {
                member_name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                submission_id,
                inbox_entry_id,
            }),
            Err(err) => failed.push(TeamBroadcastFailed {
                member_name: member.name.clone(),
                agent_id: member.agent_id.to_string(),
                inbox_entry_id,
                error: err.to_string(),
            }),
        }
    }

    let content = serde_json::to_string(&TeamBroadcastResult {
        team_id,
        sent,
        failed,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team_broadcast result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
