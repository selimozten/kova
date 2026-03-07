pub mod executor;
pub mod risk;

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use crate::arb::detector;
use crate::arb::types::ArbitrageOpportunity;
use crate::config::Config;
use crate::exchange::types::{DepthDiff, OrderBook};
use crate::exchange::Exchange;
use crate::util::shutdown::ShutdownSignal;

use executor::Executor;
use risk::RiskManager;

/// Run the full engine: discovery, depth subscription, detection, execution.
pub async fn run_engine<E: Exchange>(exchange: E, config: Config) -> anyhow::Result<()> {
    let shutdown = ShutdownSignal::new();
    let exchange = Arc::new(exchange);

    // Discover or load triangle paths
    let paths = if config.triangles.auto_discover {
        info!("discovering triangle paths from exchange...");
        let symbols = exchange.get_all_symbols().await?;
        let start_assets = if config.triangles.start_assets.is_empty() {
            vec![config.general.start_asset.clone()]
        } else {
            config.triangles.start_assets.clone()
        };
        detector::discover_triangles(&symbols, &start_assets)
    } else {
        // Build paths from explicit config (future: parse config.triangles.paths)
        warn!("explicit paths not yet implemented, using auto-discover");
        let symbols = exchange.get_all_symbols().await?;
        detector::discover_triangles(&symbols, &[config.general.start_asset.clone()])
    };

    if paths.is_empty() {
        anyhow::bail!("no triangle paths found");
    }

    info!("found {} triangle paths", paths.len());

    // Collect all unique symbols
    let mut all_symbols: Vec<String> = paths
        .iter()
        .flat_map(|p| p.legs.iter().map(|l| l.symbol.clone()))
        .collect();
    all_symbols.sort_unstable();
    all_symbols.dedup();
    info!("subscribing to {} symbols", all_symbols.len());

    // Channels
    let (depth_tx, depth_rx) = broadcast::channel::<DepthDiff>(8192);
    let (opp_tx, opp_rx) = mpsc::channel::<ArbitrageOpportunity>(256);

    // Shared order books
    let order_books: Arc<DashMap<String, OrderBook>> = Arc::new(DashMap::new());

    // Risk manager
    let risk_manager = RiskManager::new(&config.risk);

    // Executor
    let executor = Executor::new(
        exchange.clone(),
        risk_manager,
        config.general.dry_run,
        config.risk.cooldown_ms,
    );

    let shutdown_ws = shutdown.clone();
    let shutdown_detect = shutdown.clone();
    let shutdown_exec = shutdown.clone();

    // Task: WebSocket depth subscription
    let ws_exchange = exchange.clone();
    let ws_symbols = all_symbols.clone();
    let ws_handle = tokio::spawn(async move {
        tokio::select! {
            result = ws_exchange.subscribe_depth(&ws_symbols, depth_tx) => {
                if let Err(e) = result {
                    error!("depth subscription error: {e}");
                }
            }
            _ = shutdown_ws.wait() => {
                info!("depth subscription shutting down");
            }
        }
    });

    // Task: Arbitrage detector
    let detect_paths = paths.clone();
    let detect_obs = order_books.clone();
    let detect_handle = tokio::spawn(async move {
        tokio::select! {
            _ = detector::run_detector(
                detect_paths,
                depth_rx,
                opp_tx,
                detect_obs,
                config.general.trade_amount,
                config.exchange.fees.taker,
                config.risk.min_profit_pct,
            ) => {}
            _ = shutdown_detect.wait() => {
                info!("detector shutting down");
            }
        }
    });

    // Task: Executor (consumes opportunities)
    let exec_handle = tokio::spawn(async move {
        tokio::select! {
            _ = executor.run(opp_rx) => {}
            _ = shutdown_exec.wait() => {
                info!("executor shutting down");
            }
        }
    });

    // Wait for ctrl-c
    info!("engine running. press ctrl-c to stop.");
    tokio::signal::ctrl_c().await?;
    info!("shutting down...");
    shutdown.trigger();

    // Wait for tasks
    let _ = tokio::join!(ws_handle, detect_handle, exec_handle);

    info!("engine stopped");
    Ok(())
}
