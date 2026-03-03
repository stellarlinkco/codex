/// The current Codex CLI version as embedded at compile time.
pub const CODEX_CLI_VERSION: &str = match option_env!("CODEX_CLI_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
