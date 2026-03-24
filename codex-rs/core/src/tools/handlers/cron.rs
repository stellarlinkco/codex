#![allow(dead_code)]

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

use crate::function_tool::FunctionCallError;
use crate::scheduled_tasks::ScheduledTaskInfo;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::models::FunctionCallOutputBody;

pub(crate) const CRON_CREATE_TOOL_NAME: &str = "CronCreate";
pub(crate) const CRON_LIST_TOOL_NAME: &str = "CronList";
pub(crate) const CRON_DELETE_TOOL_NAME: &str = "CronDelete";

#[derive(Default)]
pub struct CronCreateHandler;

#[derive(Default)]
pub struct CronListHandler;

#[derive(Default)]
pub struct CronDeleteHandler;

#[derive(Debug, Deserialize)]
struct CronCreateArgs {
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    run_at: Option<String>,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct CronDeleteArgs {
    id: String,
}

#[derive(Debug, Serialize)]
struct CronDeleteResult {
    deleted: bool,
    task: Option<ScheduledTaskInfo>,
}

#[async_trait]
impl ToolHandler for CronCreateHandler {
    type Output = FunctionToolOutput;
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<FunctionToolOutput, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "CronCreate handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: CronCreateArgs = parse_arguments(&arguments)?;
        let now = Utc::now();
        let task = match (args.schedule.as_deref(), args.run_at.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(FunctionCallError::RespondToModel(
                    "provide either `schedule` or `run_at`, not both".to_string(),
                ));
            }
            (None, None) => {
                return Err(FunctionCallError::RespondToModel(
                    "missing schedule; provide either `schedule` or `run_at`".to_string(),
                ));
            }
            (Some(schedule), None) => {
                session
                    .services
                    .scheduled_tasks
                    .create_cron(schedule, &args.prompt, now)
                    .await
            }
            (None, Some(run_at)) => {
                let run_at = parse_rfc3339(run_at)?;
                session
                    .services
                    .scheduled_tasks
                    .create_once(run_at, &args.prompt, now)
                    .await
            }
        }
        .map_err(FunctionCallError::RespondToModel)?;

        Ok(json_success(json!({ "task": task })))
    }
}

#[async_trait]
impl ToolHandler for CronListHandler {
    type Output = FunctionToolOutput;
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<FunctionToolOutput, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;
        match payload {
            ToolPayload::Function { .. } => {}
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "CronList handler received unsupported payload".to_string(),
                ));
            }
        }

        let tasks = session.services.scheduled_tasks.list().await;
        Ok(json_success(json!({ "tasks": tasks })))
    }
}

#[async_trait]
impl ToolHandler for CronDeleteHandler {
    type Output = FunctionToolOutput;
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<FunctionToolOutput, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "CronDelete handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: CronDeleteArgs = parse_arguments(&arguments)?;
        let task = session.services.scheduled_tasks.delete(&args.id).await;
        let result = CronDeleteResult {
            deleted: task.is_some(),
            task,
        };

        Ok(json_success(json!({ "result": result })))
    }
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, FunctionCallError> {
    DateTime::parse_from_rfc3339(value)
        .map(|datetime| datetime.with_timezone(&Utc))
        .map_err(|err| FunctionCallError::RespondToModel(format!("invalid `run_at`: {err}")))
}

fn json_success(value: serde_json::Value) -> FunctionToolOutput {
    FunctionToolOutput {
        body: FunctionCallOutputBody::Text(value.to_string()),
        success: Some(true),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    use crate::codex::make_session_and_context;
    use crate::tools::context::ToolInvocation;
    use crate::tools::context::ToolPayload;
    use crate::turn_diff_tracker::TurnDiffTracker;

    use super::*;

    fn invocation(
        session: Arc<crate::codex::Session>,
        turn: Arc<crate::codex::TurnContext>,
        tool_name: &str,
        arguments: &str,
    ) -> ToolInvocation {
        ToolInvocation {
            session,
            turn,
            tracker: Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
            call_id: "call-1".to_string(),
            tool_name: tool_name.to_string(),
            tool_namespace: None,
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn cron_handlers_create_list_and_delete_tasks() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let create_handler = CronCreateHandler;
        let list_handler = CronListHandler;
        let delete_handler = CronDeleteHandler;

        let create_output = create_handler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                CRON_CREATE_TOOL_NAME,
                r#"{"schedule":"*/10 * * * *","prompt":"check build status"}"#,
            ))
            .await
            .expect("create output");

        let FunctionToolOutput { body, .. } = create_output;
        let created_json: serde_json::Value =
            serde_json::from_str(body.to_text().as_deref().expect("text output"))
                .expect("valid json");
        let created_id = created_json["task"]["id"]
            .as_str()
            .expect("task id")
            .to_string();

        let list_output = list_handler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                CRON_LIST_TOOL_NAME,
                "{}",
            ))
            .await
            .expect("list output");

        let FunctionToolOutput { body, .. } = list_output;
        let list_json: serde_json::Value =
            serde_json::from_str(body.to_text().as_deref().expect("text output"))
                .expect("valid json");
        assert_eq!(list_json["tasks"].as_array().map(Vec::len), Some(1));

        let delete_output = delete_handler
            .handle(invocation(
                Arc::clone(&session),
                Arc::clone(&turn),
                CRON_DELETE_TOOL_NAME,
                &format!(r#"{{"id":"{created_id}"}}"#),
            ))
            .await
            .expect("delete output");

        let FunctionToolOutput { body, .. } = delete_output;
        let delete_json: serde_json::Value =
            serde_json::from_str(body.to_text().as_deref().expect("text output"))
                .expect("valid json");
        assert_eq!(delete_json["result"]["deleted"].as_bool(), Some(true));
    }
}
