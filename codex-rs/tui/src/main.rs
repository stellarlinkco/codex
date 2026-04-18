use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_core::util;
use codex_tui::AppExitInfo;
use codex_tui::Cli;
use codex_tui::ExitReason;
use codex_tui::run_main;
use codex_utils_cli::CliConfigOverrides;
use supports_color::Stream;

fn format_exit_messages(exit_info: AppExitInfo, color_enabled: bool) -> Vec<String> {
    let AppExitInfo {
        token_usage,
        thread_id,
        thread_name,
        ..
    } = exit_info;

    let mut lines = Vec::new();
    if !token_usage.is_zero() {
        lines.push(codex_protocol::protocol::FinalOutput::from(token_usage).to_string());
    }

    if let Some(resume_cmd) = util::resume_command(thread_name.as_deref(), thread_id) {
        let command = if color_enabled {
            format!("\u{1b}[36m{resume_cmd}\u{1b}[39m")
        } else {
            resume_cmd
        };
        lines.push(format!("To continue this session, run {command}"));
    }

    lines
}

#[derive(Parser, Debug)]
struct TopCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    #[clap(flatten)]
    inner: Cli,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let top_cli = TopCli::parse();
        let mut inner = top_cli.inner;
        inner
            .config_overrides
            .raw_overrides
            .splice(0..0, top_cli.config_overrides.raw_overrides);
        let exit_info = run_main(inner, arg0_paths).await?;
        match exit_info.exit_reason {
            ExitReason::Fatal(message) => {
                eprintln!("ERROR: {message}");
                std::process::exit(1);
            }
            ExitReason::UserRequested => {}
        }

        let color_enabled = supports_color::on(Stream::Stdout).is_some();
        for line in format_exit_messages(exit_info, color_enabled) {
            println!("{line}");
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::format_exit_messages;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::TokenUsage;
    use codex_tui::AppExitInfo;
    use codex_tui::ExitReason;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_exit_messages_includes_resume_hint() {
        let thread_id = ThreadId::new();
        let exit_info = AppExitInfo {
            token_usage: TokenUsage {
                input_tokens: 120,
                cached_input_tokens: 0,
                output_tokens: 30,
                reasoning_output_tokens: 0,
                total_tokens: 150,
            },
            thread_id: Some(thread_id),
            thread_name: Some("demo-thread".to_string()),
            update_action: None,
            exit_reason: ExitReason::UserRequested,
        };

        let messages = format_exit_messages(exit_info, false);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1],
            "To continue this session, run codex resume demo-thread"
        );
    }

    #[test]
    fn format_exit_messages_omits_usage_when_zero() {
        let exit_info = AppExitInfo {
            token_usage: TokenUsage::default(),
            thread_id: Some(ThreadId::new()),
            thread_name: Some("demo-thread".to_string()),
            update_action: None,
            exit_reason: ExitReason::UserRequested,
        };

        let messages = format_exit_messages(exit_info, false);
        assert_eq!(
            messages,
            vec!["To continue this session, run codex resume demo-thread"]
        );
    }
}
