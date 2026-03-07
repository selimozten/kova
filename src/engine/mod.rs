pub mod executor;
pub mod risk;

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use crate::arb::detector;
use crate::arb::types::ArbitrageOpportunity;
use crate::config::Config;
use crate::exchange::types::{DepthDiff, OrderBook, SymbolInfo};
use crate::exchange::Exchange;
use crate::util::shutdown::ShutdownSignal;

use executor::Executor;
use risk::RiskManager;

/// Run the full engine: discovery, depth subscription, detection, execution.
pub async fn run_engine<E: Exchange>(exchange: E, config: Config) -> anyhow::Result<()> {
    let shutdown = ShutdownSignal::new();
    let exchange = Arc::new(exchange);

    // Fetch all exchange symbols
    info!("fetching exchange symbol info...");
    let all_symbol_info = exchange.get_all_symbols().await?;

    // Build symbol info lookup map
    let symbol_info_map: Arc<DashMap<String, SymbolInfo>> = Arc::new(DashMap::new());
    for info in &all_symbol_info {
        symbol_info_map.insert(info.symbol.clone(), info.clone());
    }

    // Discover or load triangle paths
    let paths = if config.triangles.auto_discover {
        let start_assets = if config.triangles.start_assets.is_empty() {
            vec![config.general.start_asset.clone()]
        } else {
            config.triangles.start_assets.clone()
        };
        detector::discover_triangles(&all_symbol_info, &start_assets)
    } else {
        warn!("explicit paths not yet implemented, using auto-discover");
        detector::discover_triangles(&all_symbol_info, &[config.general.start_asset.clone()])
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

    // Fetch initial order book snapshots
    let order_books: Arc<DashMap<String, OrderBook>> = Arc::new(DashMap::new());
    info!("fetching initial order book snapshots...");
    for symbol in &all_symbols {
        match exchange.fetch_order_book_snapshot(symbol, 20).await {
            Ok(ob) => {
                order_books.insert(symbol.clone(), ob);
            }
            Err(e) => {
                warn!("failed to fetch snapshot for {}: {}", symbol, e);
            }
        }
    }
    info!("initialized {} order books", order_books.len());

    // Channels
    let (depth_tx, depth_rx) = broadcast::channel::<DepthDiff>(8192);
    let (opp_tx, opp_rx) = mpsc::channel::<ArbitrageOpportunity>(256);

    // Risk manager (shared)
    let risk_manager = Arc::new(RiskManager::new(&config.risk));

    // Executor
    let executor = Executor::new(
        exchange.clone(),
        risk_manager.clone(),
        config.general.dry_run,
        config.risk.cooldown_ms,
        symbol_info_map,
    );

    let stats_interval = config.monitoring.stats_interval_secs;
    let shutdown_ws = shutdown.clone();
    let shutdown_detect = shutdown.clone();
    let shutdown_exec = shutdown.clone();
    let shutdown_stats = shutdown.clone();

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
                config.risk.slippage_buffer_bps,
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

    // Task: Periodic stats
    let stats_risk = risk_manager.clone();
    let stats_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(stats_interval));
        interval.tick().await; // skip first immediate tick
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let (pnl, total, wins) = stats_risk.stats_snapshot();
                    let losses = total - wins;
                    info!(
                        "[STATS] trades={} wins={} losses={} pnl={}",
                        total, wins, losses, pnl
                    );
                }
                _ = shutdown_stats.wait() => break,
            }
        }
    });

    // Wait for ctrl-c
    info!("engine running. press ctrl-c to stop.");
    tokio::signal::ctrl_c().await?;
    info!("shutting down...");
    shutdown.trigger();

    // Wait for tasks
    let _ = tokio::join!(ws_handle, detect_handle, exec_handle, stats_handle);

    info!("engine stopped");
    Ok(())
}
