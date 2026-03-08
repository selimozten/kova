mod arb;
mod config;
mod engine;
mod exchange;
mod util;

use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use tracing::info;
use tracing_subscriber::EnvFilter;

use config::Config;
use exchange::binance::BinanceExchange;
use exchange::paribu::ParibuExchange;
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
    /// Analyze trade journal
    Analyze {
        #[arg(short, long, default_value = "kova-journal.jsonl")]
        journal: PathBuf,
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

            match config.exchange.name.as_str() {
                "binance" => {
                    let exchange = BinanceExchange::new(&config).await?;
                    discover_and_print(&exchange, &start).await?;
                }
                "paribu" => {
                    let exchange = ParibuExchange::new(&config).await?;
                    discover_and_print(&exchange, &start).await?;
                }
                other => anyhow::bail!("unsupported exchange: {other}"),
            }
        }
        Commands::Balances { config: path } => {
            let config = Config::load(&path)?;
            init_tracing("warn");

            match config.exchange.name.as_str() {
                "binance" => {
                    let exchange = BinanceExchange::new(&config).await?;
                    print_balances(&exchange, "binance").await?;
                }
                "paribu" => {
                    let exchange = ParibuExchange::new(&config).await?;
                    print_balances(&exchange, "paribu").await?;
                }
                other => anyhow::bail!("unsupported exchange: {other}"),
            }
        }
        Commands::Analyze { journal } => {
            analyze_journal(&journal)?;
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
        "paribu" => {
            let exchange = ParibuExchange::new(&config).await?;
            engine::run_engine(exchange, config).await
        }
        other => anyhow::bail!("unsupported exchange: {other}"),
    }
}

async fn discover_and_print(exchange: &impl Exchange, start: &str) -> Result<()> {
    let symbols = exchange.get_all_symbols().await?;
    let paths = arb::detector::discover_triangles(&symbols, &[start.to_string()]);
    println!("found {} triangle paths:", paths.len());
    for (i, path) in paths.iter().enumerate() {
        println!("  {:>4}. {}", i + 1, path);
    }
    Ok(())
}

async fn print_balances(exchange: &impl Exchange, name: &str) -> Result<()> {
    let balances = exchange.get_balances().await?;
    println!("balances on {}:", name);
    let mut sorted: Vec<_> = balances.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (asset, amount) in sorted {
        println!("  {:<8} {}", asset, amount);
    }
    Ok(())
}

fn analyze_journal(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("cannot open journal {}: {}", path.display(), e))?;
    let reader = std::io::BufReader::new(file);

    let mut total = 0u64;
    let mut wins = 0u64;
    let mut losses = 0u64;
    let mut dry_runs = 0u64;
    let mut failures = 0u64;
    let mut total_profit = Decimal::ZERO;
    let mut best_profit = Decimal::ZERO;
    let mut worst_profit = Decimal::ZERO;
    let mut path_counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(&line)?;

        total += 1;

        let profit = entry["profit_amount"]
            .as_str()
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(Decimal::ZERO);

        let is_dry = entry["dry_run"].as_bool().unwrap_or(false);
        let outcome = &entry["outcome"];

        if is_dry {
            dry_runs += 1;
        }

        if outcome.is_string() && outcome.as_str() == Some("dry_run") {
            // counted above
        } else if outcome.is_string() && outcome.as_str() == Some("executed") {
            if profit > Decimal::ZERO {
                wins += 1;
            } else {
                losses += 1;
            }
        } else if outcome.is_object() {
            failures += 1;
        }

        total_profit += profit;
        if profit > best_profit {
            best_profit = profit;
        }
        if profit < worst_profit {
            worst_profit = profit;
        }

        if let Some(p) = entry["path"].as_str() {
            *path_counts.entry(p.to_string()).or_default() += 1;
        }
    }

    if total == 0 {
        println!("journal is empty");
        return Ok(());
    }

    let avg_profit = total_profit / Decimal::from(total);
    let win_rate = if wins + losses > 0 {
        format!("{:.1}%", (wins as f64 / (wins + losses) as f64) * 100.0)
    } else {
        "N/A".to_string()
    };

    println!("=== kova journal analysis ===");
    println!("  entries:       {}", total);
    println!("  dry runs:      {}", dry_runs);
    println!("  executed:      {}", wins + losses);
    println!("    wins:        {}", wins);
    println!("    losses:      {}", losses);
    println!("    win rate:    {}", win_rate);
    println!("  failures:      {}", failures);
    println!("  total PnL:     {}", total_profit);
    println!("  avg PnL:       {}", avg_profit);
    println!("  best trade:    {}", best_profit);
    println!("  worst trade:   {}", worst_profit);

    // Top paths
    let mut sorted_paths: Vec<_> = path_counts.into_iter().collect();
    sorted_paths.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n  top paths:");
    for (path, count) in sorted_paths.iter().take(10) {
        println!("    {:>4}x  {}", count, path);
    }

    println!("\ntip: for advanced queries, use DuckDB:");
    println!("  duckdb -c \"SELECT * FROM read_json_auto('{}')\"", path.display());

    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
