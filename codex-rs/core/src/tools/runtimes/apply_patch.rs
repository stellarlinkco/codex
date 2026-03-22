//! Apply Patch runtime: executes verified patches under the orchestrator.
//!
//! Assumes `apply_patch` verification/approval happened upstream. Reuses that
//! decision to avoid re-prompting, builds the self-invocation command for
//! `codex --codex-run-as-apply-patch`, and runs under the current
//! `SandboxAttempt` with a minimal environment.
use crate::exec::ExecToolCallOutput;
use crate::sandboxing::CommandSpec;
use crate::sandboxing::SandboxPermissions;
use crate::sandboxing::execute_env;
use crate::sandboxing::merge_permission_profiles;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::SandboxablePreference;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::approved_write_roots;
use crate::tools::sandboxing::matching_write_roots;
use crate::tools::sandboxing::with_cached_approval;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::CODEX_CORE_APPLY_PATCH_ARG1;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ApplyPatchRequest {
    pub action: ApplyPatchAction,
    pub file_paths: Vec<AbsolutePathBuf>,
    pub changes: std::collections::HashMap<PathBuf, FileChange>,
    pub exec_approval_requirement: ExecApprovalRequirement,
    pub timeout_ms: Option<u64>,
    pub codex_exe: Option<PathBuf>,
}

#[derive(Default)]
pub struct ApplyPatchRuntime;

impl ApplyPatchRuntime {
    pub fn new() -> Self {
        Self
    }

    fn build_command_spec(
        req: &ApplyPatchRequest,
        additional_permissions: Option<PermissionProfile>,
        _codex_home: &std::path::Path,
    ) -> Result<CommandSpec, ToolError> {
        let exe = if let Some(path) = &req.codex_exe {
            path.clone()
        } else {
            #[cfg(target_os = "windows")]
            {
                codex_windows_sandbox::resolve_current_exe_for_launch(_codex_home, "codex.exe")
            }
            #[cfg(not(target_os = "windows"))]
            {
                std::env::current_exe().map_err(|e| {
                    ToolError::Rejected(format!("failed to determine codex exe: {e}"))
                })?
            }
        };
        let program = exe.to_string_lossy().to_string();
        let mut env = HashMap::new();
        let temp_dir = req.action.cwd.to_string_lossy().to_string();
        env.insert("TMPDIR".to_string(), temp_dir.clone());
        env.insert("TMP".to_string(), temp_dir.clone());
        env.insert("TEMP".to_string(), temp_dir);

        Ok(CommandSpec {
            program,
            args: vec![
                CODEX_CORE_APPLY_PATCH_ARG1.to_string(),
                req.action.patch.clone(),
            ],
            cwd: req.action.cwd.clone(),
            expiration: req.timeout_ms.into(),
            capture_policy: crate::exec::ExecCapturePolicy::ShellTool,
            // Pin a writable temp dir inside cwd so sandboxed self-invocation
            // does not depend on host TMPDIR access.
            env,
            sandbox_permissions: if additional_permissions.is_some() {
                SandboxPermissions::WithAdditionalPermissions
            } else {
                SandboxPermissions::UseDefault
            },
            additional_permissions,
            justification: None,
        })
    }

    fn stdout_stream(ctx: &ToolCtx) -> Option<crate::exec::StdoutStream> {
        Some(crate::exec::StdoutStream {
            sub_id: ctx.turn.sub_id.clone(),
            call_id: ctx.call_id.clone(),
            tx_event: ctx.session.get_tx_event(),
        })
    }

    async fn preapproved_additional_permissions(
        session: &crate::codex::Session,
        file_paths: &[AbsolutePathBuf],
    ) -> Option<PermissionProfile> {
        let granted_permissions = merge_permission_profiles(
            session.granted_session_permissions().await.as_ref(),
            session.granted_turn_permissions().await.as_ref(),
        );
        if approved_write_roots(granted_permissions.as_ref())
            .and_then(|roots| matching_write_roots(file_paths.iter(), &roots))
            .is_some()
        {
            let scoped_paths = file_paths.to_vec();
            return Some(PermissionProfile {
                file_system: Some(FileSystemPermissions {
                    read: Some(scoped_paths.clone()),
                    write: Some(scoped_paths),
                }),
                ..Default::default()
            });
        }

        let store = session.services.tool_approvals.lock().await;
        store.matching_write_roots(file_paths.iter())?;
        let scoped_paths = file_paths.to_vec();
        Some(PermissionProfile {
            file_system: Some(FileSystemPermissions {
                read: Some(scoped_paths.clone()),
                write: Some(scoped_paths),
            }),
            ..Default::default()
        })
    }
}

impl Sandboxable for ApplyPatchRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }
    fn escalate_on_failure(&self) -> bool {
        true
    }
}

impl Approvable<ApplyPatchRequest> for ApplyPatchRuntime {
    type ApprovalKey = AbsolutePathBuf;

    fn approval_keys(&self, req: &ApplyPatchRequest) -> Vec<Self::ApprovalKey> {
        req.file_paths.clone()
    }

    fn start_approval_async<'a>(
        &'a mut self,
        req: &'a ApplyPatchRequest,
        ctx: ApprovalCtx<'a>,
    ) -> BoxFuture<'a, ReviewDecision> {
        let session = ctx.session;
        let turn = ctx.turn;
        let call_id = ctx.call_id.to_string();
        let retry_reason = ctx.retry_reason.clone();
        let approval_keys = self.approval_keys(req);
        let changes = req.changes.clone();
        Box::pin(async move {
            if retry_reason.is_none()
                && Self::preapproved_additional_permissions(session.as_ref(), &approval_keys)
                    .await
                    .is_some()
            {
                return ReviewDecision::Approved;
            }

            if let Some(reason) = retry_reason {
                let rx_approve = session
                    .request_patch_approval(
                        turn,
                        call_id,
                        changes.clone(),
                        Some(reason),
                        /*grant_root*/ None,
                    )
                    .await;
                return rx_approve.await.unwrap_or_default();
            }

            let decision = with_cached_approval(
                &session.services,
                "apply_patch",
                approval_keys.clone(),
                || async move {
                    let rx_approve = session
                        .request_patch_approval(
                            turn, call_id, changes, /*reason*/ None, /*grant_root*/ None,
                        )
                        .await;
                    rx_approve.await.unwrap_or_default()
                },
            )
            .await;

            if matches!(decision, ReviewDecision::ApprovedForSession) {
                let mut store = session.services.tool_approvals.lock().await;
                store.approve_write_roots(approval_keys);
            }

            decision
        })
    }

    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::Never => false,
            AskForApproval::Reject(reject_config) => !reject_config.rejects_sandbox_approval(),
            AskForApproval::OnFailure => true,
            AskForApproval::OnRequest => true,
            AskForApproval::Granular(_) => true,
            AskForApproval::UnlessTrusted => true,
        }
    }

    // apply_patch approvals are decided upstream by assess_patch_safety.
    //
    // This override ensures the orchestrator runs the patch approval flow when required instead
    // of falling back to the global exec approval policy.
    fn exec_approval_requirement(
        &self,
        req: &ApplyPatchRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }
}

impl ToolRuntime<ApplyPatchRequest, ExecToolCallOutput> for ApplyPatchRuntime {
    async fn run(
        &mut self,
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<ExecToolCallOutput, ToolError> {
        let additional_permissions =
            Self::preapproved_additional_permissions(ctx.session.as_ref(), &req.file_paths).await;
        let spec =
            Self::build_command_spec(req, additional_permissions, &ctx.turn.config.codex_home)?;
        let env = attempt
            .env_for(spec, /*network*/ None)
            .map_err(|err| ToolError::Codex(err.into()))?;
        let out = execute_env(env, Self::stdout_stream(ctx))
            .await
            .map_err(ToolError::Codex)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex::make_session_and_context;
    use codex_protocol::protocol::RejectConfig;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn wants_no_sandbox_approval_reject_respects_sandbox_flag() {
        let runtime = ApplyPatchRuntime::new();
        assert!(runtime.wants_no_sandbox_approval(AskForApproval::OnRequest));
        assert!(
            !runtime.wants_no_sandbox_approval(AskForApproval::Reject(RejectConfig {
                sandbox_approval: true,
                rules: false,
                mcp_elicitations: false,
            }))
        );
        assert!(
            runtime.wants_no_sandbox_approval(AskForApproval::Reject(RejectConfig {
                sandbox_approval: false,
                rules: false,
                mcp_elicitations: false,
            }))
        );
    }

    #[tokio::test]
    async fn preapproved_additional_permissions_accepts_cached_write_roots() {
        let (session, _turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let tmp = tempdir().expect("tmp");
        let file_path =
            AbsolutePathBuf::try_from(tmp.path().join("cached-write-root.txt")).expect("abs path");

        {
            let mut store = session.services.tool_approvals.lock().await;
            store.approve_write_roots(vec![file_path.clone()]);
        }

        let permissions = ApplyPatchRuntime::preapproved_additional_permissions(
            session.as_ref(),
            std::slice::from_ref(&file_path),
        )
        .await;

        assert_eq!(
            permissions,
            Some(PermissionProfile {
                file_system: Some(FileSystemPermissions {
                    read: Some(vec![file_path.clone()]),
                    write: Some(vec![file_path]),
                }),
                ..Default::default()
            })
        );
    }

    #[tokio::test]
    async fn approved_for_session_marks_cached_write_roots() {
        let (session, turn) = make_session_and_context().await;
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let tmp = tempdir().expect("tmp");
        let file_path =
            AbsolutePathBuf::try_from(tmp.path().join("approved-for-session.txt")).expect("abs");

        {
            let mut store = session.services.tool_approvals.lock().await;
            store.put(file_path.clone(), ReviewDecision::ApprovedForSession);
            assert!(store.matching_write_roots([&file_path]).is_none());
        }

        let req = ApplyPatchRequest {
            action: ApplyPatchAction::new_add_for_test(file_path.as_path(), "content".to_string()),
            file_paths: vec![file_path.clone()],
            changes: HashMap::new(),
            exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
                reason: None,
                proposed_execpolicy_amendment: None,
            },
            timeout_ms: None,
            codex_exe: None,
        };

        let mut runtime = ApplyPatchRuntime::new();
        let decision = runtime
            .start_approval_async(
                &req,
                ApprovalCtx {
                    session: &session,
                    turn: &turn,
                    call_id: "apply-patch-call",
                    retry_reason: None,
                    network_approval_context: None,
                },
            )
            .await;

        assert_eq!(decision, ReviewDecision::ApprovedForSession);
        let store = session.services.tool_approvals.lock().await;
        let expected_path = AbsolutePathBuf::try_from(
            file_path
                .as_path()
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join(file_path.as_path().file_name().unwrap()),
        )
        .unwrap();
        assert_eq!(
            store.matching_write_roots([&file_path]),
            Some(vec![expected_path])
        );
    }
}
