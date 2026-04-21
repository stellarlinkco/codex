use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use codex_core::config::find_codex_home;
use codex_core::plugins::MarketplaceRemoveRequest;
use codex_core::plugins::remove_marketplace;
use codex_utils_cli::CliConfigOverrides;

#[derive(Debug, Parser)]
pub struct MarketplaceCli {
    #[command(subcommand)]
    subcommand: MarketplaceSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum MarketplaceSubcommand {
    Remove(RemoveMarketplaceArgs),
}

#[derive(Debug, Parser)]
struct RemoveMarketplaceArgs {
    /// Configured marketplace name to remove.
    marketplace_name: String,
}

impl MarketplaceCli {
    pub async fn run(self, config_overrides: &CliConfigOverrides) -> Result<()> {
        let _overrides = config_overrides
            .parse_overrides()
            .map_err(anyhow::Error::msg)?;

        match self.subcommand {
            MarketplaceSubcommand::Remove(args) => run_remove(args).await?,
        }

        Ok(())
    }
}

async fn run_remove(args: RemoveMarketplaceArgs) -> Result<()> {
    let RemoveMarketplaceArgs { marketplace_name } = args;
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    let outcome = remove_marketplace(
        codex_home.to_path_buf(),
        MarketplaceRemoveRequest { marketplace_name },
    )
    .await?;

    println!("Removed marketplace `{}`.", outcome.marketplace_name);
    if let Some(installed_root) = outcome.removed_installed_root {
        println!(
            "Removed installed marketplace root: {}",
            installed_root.as_path().display()
        );
    }

    Ok(())
}
