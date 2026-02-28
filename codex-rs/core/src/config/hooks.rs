use crate::config_loader::ConfigLayerStack;
use crate::config_loader::ConfigLayerStackOrdering;
use codex_hooks::CommandHookConfig;
use codex_hooks::CommandHooksConfig;
use codex_hooks::HookHandlerType;
use codex_hooks::HookMatcherConfig;
use serde::Deserialize;
use std::io;
use toml::Value as TomlValue;

#[derive(Deserialize)]
#[serde(untagged)]
enum HookCommandToml {
    Shell(String),
    Argv(Vec<String>),
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HookMatcherToml {
    tool_name: Option<String>,
    tool_name_regex: Option<String>,
    prompt_regex: Option<String>,
    matcher: Option<String>,
}

#[derive(Deserialize)]
struct HookEntryToml {
    #[serde(default)]
    name: Option<String>,
    command: HookCommandToml,
    #[serde(default, rename = "async")]
    async_: bool,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    status_message: Option<String>,
    #[serde(default)]
    once: bool,
    #[serde(default)]
    matcher: HookMatcherToml,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HooksToml {
    session_start: Vec<HookEntryToml>,
    session_end: Vec<HookEntryToml>,
    user_prompt_submit: Vec<HookEntryToml>,
    pre_tool_use: Vec<HookEntryToml>,
    permission_request: Vec<HookEntryToml>,
    notification: Vec<HookEntryToml>,
    post_tool_use: Vec<HookEntryToml>,
    post_tool_use_failure: Vec<HookEntryToml>,
    stop: Vec<HookEntryToml>,
    teammate_idle: Vec<HookEntryToml>,
    task_completed: Vec<HookEntryToml>,
    config_change: Vec<HookEntryToml>,
    subagent_start: Vec<HookEntryToml>,
    subagent_stop: Vec<HookEntryToml>,
    pre_compact: Vec<HookEntryToml>,
    worktree_create: Vec<HookEntryToml>,
    worktree_remove: Vec<HookEntryToml>,
}

#[derive(Deserialize, Default)]
struct HooksLayerToml {
    hooks: Option<HooksToml>,
}

pub(crate) fn command_hooks_from_layer_stack(
    config_layer_stack: &ConfigLayerStack,
) -> io::Result<CommandHooksConfig> {
    let mut hooks = CommandHooksConfig::default();
    for layer in
        config_layer_stack.get_layers(ConfigLayerStackOrdering::LowestPrecedenceFirst, false)
    {
        let Some(layer_hooks) = parse_layer_hooks(&layer.config, &layer.name)? else {
            continue;
        };
        extend_command_hooks(&mut hooks, layer_hooks);
    }
    Ok(hooks)
}

fn parse_layer_hooks(
    config: &TomlValue,
    layer_name: &impl std::fmt::Debug,
) -> io::Result<Option<HooksToml>> {
    let parsed: HooksLayerToml = config.clone().try_into().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse hooks config for {layer_name:?}: {err}"),
        )
    })?;
    Ok(parsed.hooks)
}

fn extend_command_hooks(dst: &mut CommandHooksConfig, src: HooksToml) {
    dst.session_start
        .extend(src.session_start.into_iter().map(command_hook_from_entry));
    dst.session_end
        .extend(src.session_end.into_iter().map(command_hook_from_entry));
    dst.user_prompt_submit.extend(
        src.user_prompt_submit
            .into_iter()
            .map(command_hook_from_entry),
    );
    dst.pre_tool_use
        .extend(src.pre_tool_use.into_iter().map(command_hook_from_entry));
    dst.permission_request.extend(
        src.permission_request
            .into_iter()
            .map(command_hook_from_entry),
    );
    dst.notification
        .extend(src.notification.into_iter().map(command_hook_from_entry));
    dst.post_tool_use
        .extend(src.post_tool_use.into_iter().map(command_hook_from_entry));
    dst.post_tool_use_failure.extend(
        src.post_tool_use_failure
            .into_iter()
            .map(command_hook_from_entry),
    );
    dst.stop
        .extend(src.stop.into_iter().map(command_hook_from_entry));
    dst.teammate_idle
        .extend(src.teammate_idle.into_iter().map(command_hook_from_entry));
    dst.task_completed
        .extend(src.task_completed.into_iter().map(command_hook_from_entry));
    dst.config_change
        .extend(src.config_change.into_iter().map(command_hook_from_entry));
    dst.subagent_start
        .extend(src.subagent_start.into_iter().map(command_hook_from_entry));
    dst.subagent_stop
        .extend(src.subagent_stop.into_iter().map(command_hook_from_entry));
    dst.pre_compact
        .extend(src.pre_compact.into_iter().map(command_hook_from_entry));
    dst.worktree_create
        .extend(src.worktree_create.into_iter().map(command_hook_from_entry));
    dst.worktree_remove
        .extend(src.worktree_remove.into_iter().map(command_hook_from_entry));
}

fn command_hook_from_entry(entry: HookEntryToml) -> CommandHookConfig {
    CommandHookConfig {
        name: entry.name,
        handler_type: HookHandlerType::Command,
        command: command_argv(entry.command),
        async_: entry.async_,
        timeout: entry.timeout,
        status_message: entry.status_message,
        once: entry.once,
        matcher: matcher_from_toml(entry.matcher),
        prompt: None,
        model: None,
    }
}

fn matcher_from_toml(toml: HookMatcherToml) -> HookMatcherConfig {
    HookMatcherConfig {
        tool_name: toml.tool_name,
        tool_name_regex: toml.tool_name_regex,
        prompt_regex: toml.prompt_regex,
        matcher: toml.matcher,
    }
}

fn command_argv(command: HookCommandToml) -> Vec<String> {
    match command {
        HookCommandToml::Shell(command) => {
            let command = command.trim();
            if command.is_empty() {
                Vec::new()
            } else {
                shell_command_argv(command)
            }
        }
        HookCommandToml::Argv(argv) => argv,
    }
}

#[cfg(windows)]
fn shell_command_argv(command: &str) -> Vec<String> {
    vec!["cmd".to_string(), "/C".to_string(), command.to_string()]
}

#[cfg(not(windows))]
fn shell_command_argv(command: &str) -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), command.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_loader::ConfigLayerEntry;
    use crate::config_loader::ConfigLayerStack;
    use crate::config_loader::ConfigRequirements;
    use crate::config_loader::ConfigRequirementsToml;
    use codex_app_server_protocol::ConfigLayerSource;
    use core_test_support::test_absolute_path;
    use pretty_assertions::assert_eq;

    fn layer(name: ConfigLayerSource, toml: &str) -> ConfigLayerEntry {
        ConfigLayerEntry::new(name, toml::from_str::<TomlValue>(toml).expect("parse toml"))
    }

    #[test]
    fn command_hooks_append_across_layers() {
        let user_file = test_absolute_path("/tmp/codex-user/config.toml");
        let project_folder = test_absolute_path("/tmp/codex-project/.codex");
        let stack = ConfigLayerStack::new(
            vec![
                layer(
                    ConfigLayerSource::User { file: user_file },
                    r#"
[hooks]

[[hooks.pre_tool_use]]
command = ["echo", "u-pre"]

[hooks.pre_tool_use.matcher]
tool_name_regex = "^shell$"

[[hooks.stop]]
command = ["echo", "u-stop"]
"#,
                ),
                layer(
                    ConfigLayerSource::Project {
                        dot_codex_folder: project_folder,
                    },
                    r#"
[hooks]

[[hooks.stop]]
command = "echo p-stop"
"#,
                ),
            ],
            ConfigRequirements::default(),
            ConfigRequirementsToml::default(),
        )
        .expect("layer stack");

        let hooks = command_hooks_from_layer_stack(&stack).expect("hooks config");
        assert_eq!(hooks.pre_tool_use.len(), 1);
        assert_eq!(
            hooks.pre_tool_use[0].matcher.tool_name_regex.as_deref(),
            Some("^shell$")
        );
        assert_eq!(hooks.stop.len(), 2);
        assert_eq!(
            hooks.stop[0].command,
            vec!["echo".to_string(), "u-stop".to_string()]
        );

        #[cfg(windows)]
        assert_eq!(
            hooks.stop[1].command,
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo p-stop".to_string()
            ]
        );
        #[cfg(not(windows))]
        assert_eq!(
            hooks.stop[1].command,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo p-stop".to_string()
            ]
        );
    }
}
