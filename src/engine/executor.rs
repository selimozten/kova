use std::sync::Arc;

use dashmap::DashMap;
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::arb::types::ArbitrageOpportunity;
use crate::exchange::types::{OrderRequest, OrderSide, OrderStatus, OrderType, SymbolInfo};
use crate::exchange::Exchange;
use crate::util::decimal::round_to_step;

use super::risk::RiskManager;

pub struct Executor<E: Exchange> {
    exchange: Arc<E>,
    risk: Arc<RiskManager>,
    dry_run: bool,
    cooldown_ms: u64,
    symbol_info: Arc<DashMap<String, SymbolInfo>>,
}

impl<E: Exchange> Executor<E> {
    pub fn new(
        exchange: Arc<E>,
        risk: Arc<RiskManager>,
        dry_run: bool,
        cooldown_ms: u64,
        symbol_info: Arc<DashMap<String, SymbolInfo>>,
    ) -> Self {
        Self {
            exchange,
            risk,
            dry_run,
            cooldown_ms,
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
        // Pre-trade risk checks
        if !self.risk.pre_trade_check(&opp) {
            return;
        }

        info!("executing: {}", opp);

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

        if self.dry_run {
            info!("[DRY RUN] would execute 3 legs, expected profit: {} ({:.4}%)",
                opp.profit_amount, opp.profit_pct);
            self.risk.record_trade(opp.profit_amount);
            return;
        }

        // Execute 3 legs sequentially
        let result = self.execute_legs(&opp).await;

        match result {
            Ok(actual_profit) => {
                info!("trade completed, actual profit: {}", actual_profit);
                self.risk.record_trade(actual_profit);
            }
            Err(e) => {
                error!("trade failed at leg: {e}");
                // Record as loss for risk tracking
                self.risk.record_trade(-opp.start_amount * Decimal::new(1, 2));
            }
        }

        // Cooldown
        if self.cooldown_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.cooldown_ms)).await;
        }
    }

    async fn execute_legs(&self, opp: &ArbitrageOpportunity) -> Result<Decimal, String> {
        let mut current_amount = opp.start_amount;

        for (i, leg) in opp.legs.iter().enumerate() {
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
                    return Err(format!(
                        "leg {} qty {} below min {}",
                        i + 1, rounded, info.min_qty
                    ));
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

            let result = self
                .exchange
                .place_order(&request)
                .await
                .map_err(|e| format!("leg {} ({}): {}", i + 1, leg.symbol, e))?;

            if result.status != OrderStatus::Filled {
                warn!(
                    "leg {} not fully filled: {:?}, executed_qty={}",
                    i + 1,
                    result.status,
                    result.executed_quantity
                );

                if result.status == OrderStatus::Rejected {
                    return Err(format!("leg {} rejected", i + 1));
                }

                // Partial fill: could attempt unwind, but for MVP just continue
                if result.executed_quantity.is_zero() {
                    return Err(format!("leg {} zero fill", i + 1));
                }
            }

            // Update current amount for next leg
            match leg.side {
                OrderSide::Buy => {
                    current_amount = result.executed_quantity;
                }
                OrderSide::Sell => {
                    current_amount = result.executed_quote_quantity;
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
}
