use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, trace};

use crate::exchange::types::{DepthDiff, OrderBook, OrderSide};
use super::calculator;
use super::types::{ArbitrageOpportunity, TradeLeg, TrianglePath};
use crate::exchange::types::SymbolInfo;

/// Discover all valid triangle paths from exchange symbol info.
pub fn discover_triangles(symbols: &[SymbolInfo], start_assets: &[String]) -> Vec<TrianglePath> {
    // Build adjacency: asset -> [(symbol, other_asset, is_base)]
    // If we hold asset X and symbol has base=A, quote=X: we can buy A (Buy side)
    // If we hold asset X and symbol has base=X, quote=Q: we can sell X for Q (Sell side)
    let mut adj: HashMap<String, Vec<(String, String, OrderSide, String, String)>> = HashMap::new();

    for s in symbols {
        // If I hold quote_asset, I can buy base_asset
        adj.entry(s.quote_asset.clone())
            .or_default()
            .push((
                s.symbol.clone(),
                s.base_asset.clone(),
                OrderSide::Buy,
                s.base_asset.clone(),
                s.quote_asset.clone(),
            ));
        // If I hold base_asset, I can sell for quote_asset
        adj.entry(s.base_asset.clone())
            .or_default()
            .push((
                s.symbol.clone(),
                s.quote_asset.clone(),
                OrderSide::Sell,
                s.base_asset.clone(),
                s.quote_asset.clone(),
            ));
    }

    let mut paths = Vec::new();

    for start in start_assets {
        let Some(first_edges) = adj.get(start) else {
            continue;
        };

        for (sym1, asset1, side1, base1, quote1) in first_edges {
            if asset1 == start {
                continue;
            }
            let Some(second_edges) = adj.get(asset1) else {
                continue;
            };

            for (sym2, asset2, side2, base2, quote2) in second_edges {
                if asset2 == start || asset2 == asset1 {
                    // asset2 == start means 2-hop cycle (not triangle)
                    // asset2 == asset1 means we went back
                    continue;
                }
                let Some(third_edges) = adj.get(asset2) else {
                    continue;
                };

                for (sym3, asset3, side3, base3, quote3) in third_edges {
                    if asset3 != start {
                        continue;
                    }
                    // Found a triangle: start -> asset1 -> asset2 -> start
                    paths.push(TrianglePath {
                        start_asset: start.clone(),
                        legs: [
                            TradeLeg {
                                symbol: sym1.clone(),
                                side: *side1,
                                base_asset: base1.clone(),
                                quote_asset: quote1.clone(),
                            },
                            TradeLeg {
                                symbol: sym2.clone(),
                                side: *side2,
                                base_asset: base2.clone(),
                                quote_asset: quote2.clone(),
                            },
                            TradeLeg {
                                symbol: sym3.clone(),
                                side: *side3,
                                base_asset: base3.clone(),
                                quote_asset: quote3.clone(),
                            },
                        ],
                    });
                }
            }
        }
    }

    // Deduplicate: same 3 symbols in any order = same triangle
    let before = paths.len();
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| {
        let mut key: Vec<&str> = p.legs.iter().map(|l| l.symbol.as_str()).collect();
        key.sort();
        seen.insert(key.join(","))
    });

    info!(
        "discovered {} triangle paths ({} duplicates removed)",
        paths.len(),
        before - paths.len()
    );
    paths
}

/// Build reverse index: symbol -> list of triangle indices that use it.
pub fn build_symbol_index(paths: &[TrianglePath]) -> HashMap<String, Vec<usize>> {
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        for leg in &path.legs {
            index.entry(leg.symbol.clone()).or_default().push(i);
        }
    }
    // Deduplicate
    for indices in index.values_mut() {
        indices.sort_unstable();
        indices.dedup();
    }
    index
}

/// Run the arbitrage detector loop.
/// Listens for depth updates, maintains local order books, and evaluates triangles.
pub async fn run_detector(
    paths: Vec<TrianglePath>,
    mut depth_rx: broadcast::Receiver<DepthDiff>,
    opp_tx: mpsc::Sender<ArbitrageOpportunity>,
    order_books: Arc<DashMap<String, OrderBook>>,
    start_amount: Decimal,
    taker_fee: Decimal,
    min_profit_pct: Decimal,
    slippage_bps: Decimal,
    evaluated_counter: Arc<AtomicU64>,
    opportunity_counter: Arc<AtomicU64>,
) {
    let symbol_index = build_symbol_index(&paths);

    info!(
        "detector running: {} paths, min_profit={}%",
        paths.len(),
        min_profit_pct
    );

    loop {
        let diff = match depth_rx.recv().await {
            Ok(d) => d,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                debug!("detector lagged {n} messages");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("depth channel closed, detector stopping");
                break;
            }
        };

        // Update local order book
        {
            let mut ob = order_books
                .entry(diff.symbol.clone())
                .or_insert_with(|| OrderBook::new(diff.symbol.clone()));
            ob.apply_diff(&diff);
        }

        // Evaluate affected triangles
        let Some(affected) = symbol_index.get(&diff.symbol) else {
            continue;
        };

        for &idx in affected {
            let path = &paths[idx];
            let ob_clone = order_books.clone();
            evaluated_counter.fetch_add(1, Ordering::Relaxed);

            let opp = calculator::evaluate_triangle(
                path,
                &|sym| ob_clone.get(sym).map(|r| r.value().clone()),
                start_amount,
                taker_fee,
                slippage_bps,
            );

            if let Some(opp) = opp {
                if opp.profit_pct >= min_profit_pct {
                    opportunity_counter.fetch_add(1, Ordering::Relaxed);
                    trace!("{}", opp);
                    if opp_tx.send(opp).await.is_err() {
                        info!("opportunity channel closed, detector stopping");
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_symbols() -> Vec<SymbolInfo> {
        vec![
            SymbolInfo {
                symbol: "BTCUSDT".into(),
                base_asset: "BTC".into(),
                quote_asset: "USDT".into(),
                status: "TRADING".into(),
                tick_size: Decimal::ZERO,
                step_size: Decimal::ZERO,
                min_notional: Decimal::ZERO,
                min_qty: Decimal::ZERO,
                max_qty: Decimal::new(i64::MAX, 0),
            },
            SymbolInfo {
                symbol: "ETHBTC".into(),
                base_asset: "ETH".into(),
                quote_asset: "BTC".into(),
                status: "TRADING".into(),
                tick_size: Decimal::ZERO,
                step_size: Decimal::ZERO,
                min_notional: Decimal::ZERO,
                min_qty: Decimal::ZERO,
                max_qty: Decimal::new(i64::MAX, 0),
            },
            SymbolInfo {
                symbol: "ETHUSDT".into(),
                base_asset: "ETH".into(),
                quote_asset: "USDT".into(),
                status: "TRADING".into(),
                tick_size: Decimal::ZERO,
                step_size: Decimal::ZERO,
                min_notional: Decimal::ZERO,
                min_qty: Decimal::ZERO,
                max_qty: Decimal::new(i64::MAX, 0),
            },
        ]
    }

    #[test]
    fn test_discover_triangles() {
        let symbols = test_symbols();
        let paths = discover_triangles(&symbols, &["USDT".into()]);
        // Should find paths like USDT->BTC->ETH->USDT and USDT->ETH->BTC->USDT
        assert!(!paths.is_empty());
        for path in &paths {
            assert_eq!(path.legs.len(), 3);
            assert_eq!(path.start_asset, "USDT");
        }
    }

    #[test]
    fn test_build_symbol_index() {
        let symbols = test_symbols();
        let paths = discover_triangles(&symbols, &["USDT".into()]);
        let index = build_symbol_index(&paths);

        // Each symbol should appear in the index
        assert!(index.contains_key("BTCUSDT"));
        assert!(index.contains_key("ETHBTC"));
        assert!(index.contains_key("ETHUSDT"));
    }
}
