#[cfg(unix)]
use anyhow::Context as _;
#[cfg(unix)]
use serde::Deserialize;
#[cfg(unix)]
use serde::Serialize;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use uuid::Uuid;

#[cfg(unix)]
pub(crate) const ZSH_EXEC_BRIDGE_WRAPPER_SOCKET_ENV_VAR: &str =
    "CODEX_ZSH_EXEC_BRIDGE_WRAPPER_SOCKET";
pub(crate) const ZSH_EXEC_WRAPPER_MODE_ENV_VAR: &str = "CODEX_ZSH_EXEC_WRAPPER_MODE";
#[cfg(unix)]
pub(crate) const EXEC_WRAPPER_ENV_VAR: &str = "EXEC_WRAPPER";

#[cfg(unix)]
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WrapperIpcRequest {
    ExecRequest {
        request_id: String,
        file: String,
        argv: Vec<String>,
        cwd: String,
    },
}

#[cfg(unix)]
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WrapperIpcResponse {
    ExecResponse {
        request_id: String,
        action: WrapperExecAction,
        reason: Option<String>,
    },
}

#[cfg(unix)]
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WrapperExecAction {
    Run,
    Deny,
}

pub fn maybe_run_zsh_exec_wrapper_mode() -> anyhow::Result<bool> {
    if std::env::var_os(ZSH_EXEC_WRAPPER_MODE_ENV_VAR).is_none() {
        return Ok(false);
    }

    run_exec_wrapper_mode()?;
    Ok(true)
}

fn run_exec_wrapper_mode() -> anyhow::Result<()> {
    #[cfg(not(unix))]
    {
        anyhow::bail!("zsh exec wrapper mode is only supported on unix");
    }

    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream as StdUnixStream;

        let args: Vec<String> = std::env::args().collect();
        if args.len() < 2 {
            anyhow::bail!("exec wrapper mode requires target executable path");
        }
        let file = args[1].clone();
        let argv = if args.len() > 2 {
            args[2..].to_vec()
        } else {
            vec![file.clone()]
        };
        let cwd = std::env::current_dir()
            .context("resolve wrapper cwd")?
            .to_string_lossy()
            .to_string();
        let socket_path = std::env::var(ZSH_EXEC_BRIDGE_WRAPPER_SOCKET_ENV_VAR)
            .context("missing wrapper socket path env var")?;

        let request_id = Uuid::new_v4().to_string();
        let request = WrapperIpcRequest::ExecRequest {
            request_id: request_id.clone(),
            file: file.clone(),
            argv: argv.clone(),
            cwd,
        };

        let mut stream = StdUnixStream::connect(&socket_path)
            .with_context(|| format!("connect to wrapper socket at {socket_path}"))?;
        let encoded = serde_json::to_string(&request).context("serialize wrapper request")?;
        stream
            .write_all(encoded.as_bytes())
            .context("write wrapper request")?;
        stream
            .write_all(b"\n")
            .context("write wrapper request newline")?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context("shutdown wrapper write")?;

        let mut response_buf = String::new();
        stream
            .read_to_string(&mut response_buf)
            .context("read wrapper response")?;
        let response: WrapperIpcResponse =
            serde_json::from_str(response_buf.trim()).context("parse wrapper response")?;

        let (response_request_id, action, reason) = match response {
            WrapperIpcResponse::ExecResponse {
                request_id,
                action,
                reason,
            } => (request_id, action, reason),
        };
        if response_request_id != request_id {
            anyhow::bail!(
                "wrapper response request_id mismatch: expected {request_id}, got {response_request_id}"
            );
        }

        if action == WrapperExecAction::Deny {
            if let Some(reason) = reason {
                tracing::warn!("execution denied: {reason}");
            } else {
                tracing::warn!("execution denied");
            }
            std::process::exit(1);
        }

        let mut command = std::process::Command::new(&file);
        if argv.len() > 1 {
            command.args(&argv[1..]);
        }
        command.env_remove(ZSH_EXEC_WRAPPER_MODE_ENV_VAR);
        command.env_remove(ZSH_EXEC_BRIDGE_WRAPPER_SOCKET_ENV_VAR);
        command.env_remove(EXEC_WRAPPER_ENV_VAR);
        let status = command.status().context("spawn wrapped executable")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
