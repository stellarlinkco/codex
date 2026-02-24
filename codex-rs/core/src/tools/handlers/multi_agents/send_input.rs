use super::*;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct SendInputArgs {
    id: String,
    message: Option<String>,
    items: Option<Vec<UserInput>>,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
struct SendInputResult {
    submission_id: String,
}

pub async fn handle(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: SendInputArgs = parse_arguments(&arguments)?;
    let receiver_thread_id = agent_id(&args.id)?;
    let input_items = parse_collab_input(args.message, args.items)?;
    let prompt = input_preview(&input_items);
    let submission_id = send_input_to_member(
        &session,
        &turn,
        call_id,
        receiver_thread_id,
        input_items,
        prompt,
        args.interrupt,
    )
    .await?;

    let content = serde_json::to_string(&SendInputResult { submission_id }).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_input result: {err}"))
    })?;

    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(content),
        success: Some(true),
    })
}
