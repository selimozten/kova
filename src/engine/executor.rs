use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::arb::types::ArbitrageOpportunity;
use crate::exchange::types::{OrderRequest, OrderSide, OrderStatus, OrderType, SymbolInfo};
use crate::exchange::Exchange;
use crate::util::decimal::round_to_step;

use super::journal::{TradeJournal, TradeOutcome};
use super::risk::RiskManager;

pub struct Executor<E: Exchange> {
    exchange: Arc<E>,
    risk: Arc<RiskManager>,
    journal: Option<Arc<TradeJournal>>,
    dry_run: bool,
    cooldown_ms: u64,
    max_opportunity_age_ms: u64,
    symbol_info: Arc<DashMap<String, SymbolInfo>>,
}

impl<E: Exchange> Executor<E> {
    pub fn new(
        exchange: Arc<E>,
        risk: Arc<RiskManager>,
        journal: Option<Arc<TradeJournal>>,
        dry_run: bool,
        cooldown_ms: u64,
        max_opportunity_age_ms: u64,
        symbol_info: Arc<DashMap<String, SymbolInfo>>,
    ) -> Self {
        Self {
            exchange,
            risk,
            journal,
            dry_run,
            cooldown_ms,
            max_opportunity_age_ms,
            symbol_info,
        }
    }

    pub async fn run(&self, mut rx: mpsc::Receiver<ArbitrageOpportunity>) {
        while let Some(opp) = rx.recv().await {
            self.handle_opportunity(opp).await;
        }
        info!("executor: opportunity channel closed");
    }

    async fn handle_opportunity(&self, opp: ArbitrageOpportunity) {
        // Reject stale opportunities (>2s old by the time we process them)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis() as u64;
        let age_ms = now_ms.saturating_sub(opp.detected_at_ms);
        if age_ms > self.max_opportunity_age_ms {
            debug!("dropping stale opportunity ({}ms old): {}", age_ms, opp.path);
            return;
        }

        // Pre-trade risk checks
        if !self.risk.pre_trade_check(&opp) {
            return;
        }

        info!("executing: {}", opp);

        if self.dry_run {
            info!("[DRY RUN] would execute 3 legs, expected profit: {} ({:.4}%)",
                opp.profit_amount, opp.profit_pct);
            self.risk.record_trade(opp.profit_amount);
            if let Some(j) = &self.journal {
                j.record(&opp, TradeOutcome::DryRun, true);
            }
            return;
        }

        // Verify we have enough balance for the first leg
        match self.exchange.get_balances().await {
            Ok(balances) => {
                let start_asset = &opp.path.start_asset;
                let available = balances.get(start_asset).copied().unwrap_or(Decimal::ZERO);
                if available < opp.start_amount {
                    warn!(
                        "insufficient balance: have {} {}, need {}",
                        available, start_asset, opp.start_amount
                    );
                    return;
                }
            }
            Err(e) => {
                warn!("failed to check balance, proceeding: {}", e);
            }
        }

        // Execute 3 legs sequentially
        let result = self.execute_legs(&opp).await;

        match result {
            Ok(actual_profit) => {
                info!("trade completed, actual profit: {}", actual_profit);
                self.risk.record_trade(actual_profit);
                if let Some(j) = &self.journal {
                    j.record(&opp, TradeOutcome::Executed, false);
                }
            }
            Err((failed_leg, held_asset, held_amount, msg)) => {
                error!("trade failed at leg {}: {}", failed_leg + 1, msg);
                if let Some(j) = &self.journal {
                    j.record(&opp, TradeOutcome::Failed {
                        leg: failed_leg + 1,
                        reason: msg.clone(),
                    }, false);
                }
                if failed_leg > 0 && held_amount > Decimal::ZERO {
                    self.attempt_unwind(&opp, failed_leg, &held_asset, held_amount).await;
                }
                self.risk.record_trade(-opp.start_amount * Decimal::new(1, 2));
            }
        }

        // Cooldown
        if self.cooldown_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.cooldown_ms)).await;
        }
    }

    /// Execute legs sequentially. On failure returns (failed_leg_index, held_asset, held_amount, error_msg).
    async fn execute_legs(
        &self,
        opp: &ArbitrageOpportunity,
    ) -> Result<Decimal, (usize, String, Decimal, String)> {
        let mut current_amount = opp.start_amount;
        // Track what asset we're currently holding for unwind purposes
        let mut current_asset = opp.path.start_asset.clone();

        for (i, (leg, path_leg)) in opp.legs.iter().zip(opp.path.legs.iter()).enumerate() {
            info!(
                "leg {}: {} {} qty={} (input={})",
                i + 1,
                leg.side,
                leg.symbol,
                leg.quantity,
                current_amount,
            );

            let quantity = if let Some(info) = self.symbol_info.get(&leg.symbol) {
                let rounded = round_to_step(leg.quantity, info.step_size);
                if rounded < info.min_qty {
                    return Err((
                        i,
                        current_asset,
                        current_amount,
                        format!("qty {} below min {}", rounded, info.min_qty),
                    ));
                }
                if rounded > info.max_qty {
                    return Err((
                        i,
                        current_asset,
                        current_amount,
                        format!("qty {} exceeds max {}", rounded, info.max_qty),
                    ));
                }
                if info.min_notional > Decimal::ZERO {
                    let notional = leg.effective_price * rounded;
                    if notional < info.min_notional {
                        return Err((
                            i,
                            current_asset,
                            current_amount,
                            format!("notional {} below min {}", notional, info.min_notional),
                        ));
                    }
                }
                rounded
            } else {
                leg.quantity
            };

            let client_oid = format!(
                "kova_{}_{}",
                chrono::Utc::now().format("%Y%m%d%H%M%S%3f"),
                i + 1,
            );

            let request = OrderRequest {
                symbol: leg.symbol.clone(),
                side: leg.side,
                order_type: OrderType::Market,
                quantity,
                price: None,
                client_order_id: Some(client_oid),
            };

            let result = match self.exchange.place_order(&request).await {
                Ok(r) => r,
                Err(e) => {
                    return Err((
                        i,
                        current_asset,
                        current_amount,
                        format!("{}: {}", leg.symbol, e),
                    ));
                }
            };

            if result.status == OrderStatus::Rejected {
                return Err((
                    i,
                    current_asset,
                    current_amount,
                    format!("{} rejected", leg.symbol),
                ));
            }

            if result.executed_quantity.is_zero() {
                return Err((
                    i,
                    current_asset,
                    current_amount,
                    format!("{} zero fill", leg.symbol),
                ));
            }

            if result.status != OrderStatus::Filled {
                warn!(
                    "leg {} partially filled: {:?}, executed_qty={}",
                    i + 1,
                    result.status,
                    result.executed_quantity
                );
            }

            // Update current amount and asset for next leg
            match leg.side {
                OrderSide::Buy => {
                    current_amount = result.executed_quantity;
                    current_asset = path_leg.base_asset.clone();
                }
                OrderSide::Sell => {
                    current_amount = result.executed_quote_quantity;
                    current_asset = path_leg.quote_asset.clone();
                }
            }

            info!(
                "leg {} filled: qty={}, quote_qty={}, price={}",
                i + 1,
                result.executed_quantity,
                result.executed_quote_quantity,
                result.effective_price,
            );
        }

        let actual_profit = current_amount - opp.start_amount;
        Ok(actual_profit)
    }

    /// Attempt to sell the held asset back to the start asset when a leg fails mid-triangle.
    async fn attempt_unwind(
        &self,
        opp: &ArbitrageOpportunity,
        failed_leg: usize,
        held_asset: &str,
        held_amount: Decimal,
    ) {
        warn!(
            "attempting unwind: sell {} {} back to {}",
            held_amount, held_asset, opp.path.start_asset
        );

        // Try to find a direct pair to convert back
        // Look through the path legs we already traversed for a reverse route
        for leg in &opp.path.legs[..failed_leg] {
            let can_reverse = match leg.side {
                OrderSide::Buy if leg.base_asset == held_asset => true,
                OrderSide::Sell if leg.quote_asset == held_asset => true,
                _ => false,
            };

            if can_reverse {
                let (reverse_side, reverse_qty) = match leg.side {
                    OrderSide::Buy => (OrderSide::Sell, held_amount),
                    OrderSide::Sell => (OrderSide::Buy, held_amount),
                };

                let quantity = if let Some(info) = self.symbol_info.get(&leg.symbol) {
                    round_to_step(reverse_qty, info.step_size)
                } else {
                    reverse_qty
                };

                let request = OrderRequest {
                    symbol: leg.symbol.clone(),
                    side: reverse_side,
                    order_type: OrderType::Market,
                    quantity,
                    price: None,
                    client_order_id: Some(format!(
                        "kova_unwind_{}",
                        chrono::Utc::now().format("%Y%m%d%H%M%S%3f"),
                    )),
                };

                match self.exchange.place_order(&request).await {
                    Ok(result) => {
                        info!(
                            "unwind {} {}: filled qty={}, price={}",
                            reverse_side, leg.symbol, result.executed_quantity, result.effective_price
                        );
                        return;
                    }
                    Err(e) => {
                        error!("unwind failed for {}: {}", leg.symbol, e);
                    }
                }
            }
        }

        error!(
            "could not unwind {} {} — manual intervention required",
            held_amount, held_asset
        );
    }
}
