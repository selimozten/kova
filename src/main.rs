mod arb;
mod config;
mod engine;
mod exchange;
mod util;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

use config::Config;
use exchange::binance::BinanceExchange;
use exchange::Exchange;

#[derive(Parser)]
#[command(name = "kova", about = "Triangular arbitrage engine", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the arbitrage engine
    Run {
        #[arg(short, long, default_value = "kova.toml")]
        config: PathBuf,
    },
    /// Detect opportunities without executing trades
    DryRun {
        #[arg(short, long, default_value = "kova.toml")]
        config: PathBuf,
    },
    /// Validate configuration file
    CheckConfig {
        #[arg(short, long, default_value = "kova.toml")]
        config: PathBuf,
    },
    /// Discover all valid triangle paths
    Discover {
        #[arg(short, long, default_value = "kova.toml")]
        config: PathBuf,
        #[arg(long)]
        start_asset: Option<String>,
    },
    /// Show account balances
    Balances {
        #[arg(short, long, default_value = "kova.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { config: path } => {
            let config = Config::load(&path)?;
            init_tracing(&config.general.log_level);
            info!("starting kova engine");
            run_with_exchange(config).await?;
        }
        Commands::DryRun { config: path } => {
            let mut config = Config::load(&path)?;
            config.general.dry_run = true;
            init_tracing(&config.general.log_level);
            info!("starting kova engine (dry run)");
            run_with_exchange(config).await?;
        }
        Commands::CheckConfig { config: path } => {
            let config = Config::load(&path)?;
            println!("config OK");
            println!("  exchange: {}", config.exchange.name);
            println!("  start_asset: {}", config.general.start_asset);
            println!("  trade_amount: {}", config.general.trade_amount);
            println!("  dry_run: {}", config.general.dry_run);
            println!("  auto_discover: {}", config.triangles.auto_discover);
            println!("  min_profit: {}%", config.risk.min_profit_pct);
        }
        Commands::Discover {
            config: path,
            start_asset,
        } => {
            let config = Config::load(&path)?;
            init_tracing(&config.general.log_level);

            let start = start_asset.unwrap_or(config.general.start_asset.clone());
            println!("discovering triangles for {} on {}...", start, config.exchange.name);

            let exchange = create_exchange(&config).await?;
            let symbols = exchange.get_all_symbols().await?;
            let paths = arb::detector::discover_triangles(&symbols, &[start]);

            println!("found {} triangle paths:", paths.len());
            for (i, path) in paths.iter().enumerate() {
                println!("  {:>4}. {}", i + 1, path);
            }
        }
        Commands::Balances { config: path } => {
            let config = Config::load(&path)?;
            init_tracing("warn");

            let exchange = create_exchange(&config).await?;
            let balances = exchange.get_balances().await?;

            println!("balances on {}:", config.exchange.name);
            let mut sorted: Vec<_> = balances.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            for (asset, amount) in sorted {
                println!("  {:<8} {}", asset, amount);
            }
        }
    }

    Ok(())
}

async fn run_with_exchange(config: Config) -> Result<()> {
    match config.exchange.name.as_str() {
        "binance" => {
            let exchange = BinanceExchange::new(&config).await?;
            engine::run_engine(exchange, config).await
        }
        other => anyhow::bail!("unsupported exchange: {other}"),
    }
}

async fn create_exchange(config: &Config) -> Result<BinanceExchange> {
    match config.exchange.name.as_str() {
        "binance" => Ok(BinanceExchange::new(config).await?),
        other => anyhow::bail!("unsupported exchange: {other}"),
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
