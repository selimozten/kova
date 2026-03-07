use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::arb::types::ArbitrageOpportunity;
use crate::config::RiskConfig;

pub struct RiskManager {
    min_profit_pct: Decimal,
    max_trade_size: Decimal,
    max_session_loss: Decimal,
    max_consecutive_losses: u32,
    session_pnl: Mutex<Decimal>,
    consecutive_losses: AtomicU32,
    total_trades: AtomicU32,
    kill_switch: std::sync::atomic::AtomicBool,
}

impl RiskManager {
    pub fn new(config: &RiskConfig) -> Self {
        Self {
            min_profit_pct: config.min_profit_pct,
            max_trade_size: config.max_trade_size,
            max_session_loss: config.max_session_loss,
            max_consecutive_losses: config.max_consecutive_losses,
            session_pnl: Mutex::new(Decimal::ZERO),
            consecutive_losses: AtomicU32::new(0),
            total_trades: AtomicU32::new(0),
            kill_switch: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Check if a trade should be executed.
    pub fn pre_trade_check(&self, opp: &ArbitrageOpportunity) -> bool {
        if self.kill_switch.load(Ordering::Relaxed) {
            warn!("kill switch active, rejecting trade");
            return false;
        }

        if opp.profit_pct < self.min_profit_pct {
            return false;
        }

        if opp.start_amount > self.max_trade_size {
            warn!(
                "trade size {} exceeds max {}",
                opp.start_amount, self.max_trade_size
            );
            return false;
        }

        true
    }

    /// Record a completed trade's PnL.
    pub fn record_trade(&self, pnl: Decimal) {
        self.total_trades.fetch_add(1, Ordering::Relaxed);

        let mut session = self.session_pnl.lock().unwrap();
        *session += pnl;

        if pnl < Decimal::ZERO {
            let losses = self.consecutive_losses.fetch_add(1, Ordering::Relaxed) + 1;
            if losses >= self.max_consecutive_losses {
                warn!("kill switch triggered: {} consecutive losses", losses);
                self.kill_switch.store(true, Ordering::Relaxed);
            }
        } else {
            self.consecutive_losses.store(0, Ordering::Relaxed);
        }

        if *session < -self.max_session_loss {
            warn!(
                "kill switch triggered: session loss {} exceeds max {}",
                session, self.max_session_loss
            );
            self.kill_switch.store(true, Ordering::Relaxed);
        }

        info!(
            "trade recorded: pnl={}, session_pnl={}, trades={}",
            pnl,
            *session,
            self.total_trades.load(Ordering::Relaxed),
        );
    }

    pub fn session_pnl(&self) -> Decimal {
        *self.session_pnl.lock().unwrap()
    }

    pub fn total_trades(&self) -> u32 {
        self.total_trades.load(Ordering::Relaxed)
    }

    pub fn is_killed(&self) -> bool {
        self.kill_switch.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arb::types::*;
    use crate::exchange::types::OrderSide;
    use rust_decimal_macros::dec;

    fn test_config() -> RiskConfig {
        RiskConfig {
            min_profit_pct: dec!(0.1),
            max_trade_size: dec!(500),
            max_session_loss: dec!(50),
            max_consecutive_losses: 3,
            slippage_buffer_bps: dec!(2),
            cooldown_ms: 0,
        }
    }

    fn test_opp(profit_pct: Decimal, start_amount: Decimal) -> ArbitrageOpportunity {
        let leg = LegDetail {
            symbol: "BTCUSDT".into(),
            side: OrderSide::Buy,
            quantity: dec!(0.01),
            effective_price: dec!(50000),
            fee: dec!(0.5),
            input_amount: dec!(500),
            output_amount: dec!(499.5),
        };
        ArbitrageOpportunity {
            path: TrianglePath {
                start_asset: "USDT".into(),
                legs: [
                    TradeLeg {
                        symbol: "BTCUSDT".into(),
                        side: OrderSide::Buy,
                        base_asset: "BTC".into(),
                        quote_asset: "USDT".into(),
                    },
                    TradeLeg {
                        symbol: "ETHBTC".into(),
                        side: OrderSide::Sell,
                        base_asset: "ETH".into(),
                        quote_asset: "BTC".into(),
                    },
                    TradeLeg {
                        symbol: "ETHUSDT".into(),
                        side: OrderSide::Sell,
                        base_asset: "ETH".into(),
                        quote_asset: "USDT".into(),
                    },
                ],
            },
            legs: [leg.clone(), leg.clone(), leg],
            start_amount,
            end_amount: start_amount + (start_amount * profit_pct / dec!(100)),
            profit_amount: start_amount * profit_pct / dec!(100),
            profit_pct,
        }
    }

    #[test]
    fn test_pre_trade_check_passes() {
        let rm = RiskManager::new(&test_config());
        let opp = test_opp(dec!(0.2), dec!(100));
        assert!(rm.pre_trade_check(&opp));
    }

    #[test]
    fn test_pre_trade_check_low_profit() {
        let rm = RiskManager::new(&test_config());
        let opp = test_opp(dec!(0.05), dec!(100));
        assert!(!rm.pre_trade_check(&opp));
    }

    #[test]
    fn test_pre_trade_check_too_large() {
        let rm = RiskManager::new(&test_config());
        let opp = test_opp(dec!(0.5), dec!(1000));
        assert!(!rm.pre_trade_check(&opp));
    }

    #[test]
    fn test_kill_switch_consecutive_losses() {
        let rm = RiskManager::new(&test_config());
        rm.record_trade(dec!(-1));
        rm.record_trade(dec!(-1));
        assert!(!rm.is_killed());
        rm.record_trade(dec!(-1));
        assert!(rm.is_killed());

        let opp = test_opp(dec!(1.0), dec!(100));
        assert!(!rm.pre_trade_check(&opp));
    }

    #[test]
    fn test_kill_switch_session_loss() {
        let rm = RiskManager::new(&test_config());
        rm.record_trade(dec!(-51));
        assert!(rm.is_killed());
    }

    #[test]
    fn test_consecutive_losses_reset() {
        let rm = RiskManager::new(&test_config());
        rm.record_trade(dec!(-1));
        rm.record_trade(dec!(-1));
        rm.record_trade(dec!(5)); // win resets counter
        rm.record_trade(dec!(-1));
        rm.record_trade(dec!(-1));
        assert!(!rm.is_killed());
    }
}
