use clap::Parser;
use codex_utils_cli::CliConfigOverrides;
use std::net::IpAddr;
use std::path::PathBuf;

mod server;

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    /// Bind address (default: 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: IpAddr,

    /// Listen port (default: 0, auto-assign).
    #[arg(long, default_value_t = 0)]
    pub port: u16,

    /// Do not open the browser automatically.
    #[arg(long)]
    pub no_open: bool,

    /// Serve Web UI assets from the filesystem (dev mode).
    #[arg(long)]
    pub dev: bool,

    /// Specify a server token (default: random).
    #[arg(long)]
    pub token: Option<String>,
}

pub async fn run_main(cli: Cli, codex_linux_sandbox_exe: Option<PathBuf>) -> anyhow::Result<()> {
    server::run(cli, codex_linux_sandbox_exe).await
}
