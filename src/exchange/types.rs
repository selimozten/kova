use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single price level in the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub quantity: Decimal,
}

/// Local order book maintained from WebSocket depth updates.
#[derive(Debug, Clone)]
pub struct OrderBook {
    pub symbol: String,
    /// Bids sorted by price descending (best bid first).
    pub bids: BTreeMap<Decimal, Decimal>,
    /// Asks sorted by price ascending (best ask first).
    pub asks: BTreeMap<Decimal, Decimal>,
    pub last_update_id: u64,
    /// Timestamp (ms) of the last update applied.
    pub updated_at_ms: u64,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_update_id: 0,
            updated_at_ms: 0,
        }
    }

    /// Returns the age of this order book in milliseconds.
    pub fn age_ms(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis() as u64;
        now.saturating_sub(self.updated_at_ms)
    }

    /// Returns true if this order book has not been updated recently.
    pub fn is_stale(&self, max_age_ms: u64) -> bool {
        self.updated_at_ms == 0 || self.age_ms() > max_age_ms
    }

    /// Best bid price (highest).
    pub fn best_bid(&self) -> Option<&Decimal> {
        self.bids.keys().next_back()
    }

    /// Best ask price (lowest).
    pub fn best_ask(&self) -> Option<&Decimal> {
        self.asks.keys().next()
    }

    /// Apply a depth diff: update or remove price levels.
    pub fn apply_diff(&mut self, diff: &DepthDiff) {
        for level in &diff.bids {
            if level.quantity.is_zero() {
                self.bids.remove(&level.price);
            } else {
                self.bids.insert(level.price, level.quantity);
            }
        }
        for level in &diff.asks {
            if level.quantity.is_zero() {
                self.asks.remove(&level.price);
            } else {
                self.asks.insert(level.price, level.quantity);
            }
        }
        if diff.last_update_id > self.last_update_id {
            self.last_update_id = diff.last_update_id;
        }
        self.updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis() as u64;
    }

    /// Walk the ask side to compute the effective price and max fillable quantity
    /// for a buy of `target_quantity` base asset.
    pub fn walk_asks(&self, target_quantity: Decimal) -> Option<(Decimal, Decimal)> {
        let mut remaining = target_quantity;
        let mut total_cost = Decimal::ZERO;

        for (&price, &qty) in &self.asks {
            if remaining.is_zero() {
                break;
            }
            let fill = remaining.min(qty);
            total_cost += fill * price;
            remaining -= fill;
        }

        if remaining > Decimal::ZERO {
            return None; // Not enough liquidity
        }

        let effective_price = total_cost / target_quantity;
        Some((effective_price, target_quantity))
    }

    /// Walk the bid side to compute the effective price and max fillable quantity
    /// for a sell of `target_quantity` base asset.
    pub fn walk_bids(&self, target_quantity: Decimal) -> Option<(Decimal, Decimal)> {
        let mut remaining = target_quantity;
        let mut total_proceeds = Decimal::ZERO;

        for (&price, &qty) in self.bids.iter().rev() {
            if remaining.is_zero() {
                break;
            }
            let fill = remaining.min(qty);
            total_proceeds += fill * price;
            remaining -= fill;
        }

        if remaining > Decimal::ZERO {
            return None;
        }

        let effective_price = total_proceeds / target_quantity;
        Some((effective_price, target_quantity))
    }
}

/// Depth update received from the WebSocket stream.
#[derive(Debug, Clone)]
pub struct DepthDiff {
    pub symbol: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub first_update_id: u64,
    pub last_update_id: u64,
}

/// Request to place an order.
#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Decimal,
    /// Only for limit orders.
    pub price: Option<Decimal>,
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Market => write!(f, "MARKET"),
            OrderType::Limit => write!(f, "LIMIT"),
        }
    }
}

/// Result of a placed order.
#[derive(Debug, Clone)]
pub struct OrderResult {
    pub order_id: String,
    pub symbol: String,
    pub side: OrderSide,
    pub status: OrderStatus,
    pub executed_quantity: Decimal,
    pub executed_quote_quantity: Decimal,
    pub effective_price: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
    New,
}

/// Exchange symbol metadata used for validation and rounding.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub status: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_notional: Decimal,
    pub min_qty: Decimal,
    pub max_qty: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_order_book_apply_diff() {
        let mut ob = OrderBook::new("BTCUSDT".into());
        let diff = DepthDiff {
            symbol: "BTCUSDT".into(),
            bids: vec![
                PriceLevel { price: dec!(50000), quantity: dec!(1.0) },
                PriceLevel { price: dec!(49999), quantity: dec!(2.0) },
            ],
            asks: vec![
                PriceLevel { price: dec!(50001), quantity: dec!(1.5) },
                PriceLevel { price: dec!(50002), quantity: dec!(0.5) },
            ],
            first_update_id: 1,
            last_update_id: 1,
        };
        ob.apply_diff(&diff);

        assert_eq!(ob.best_bid(), Some(&dec!(50000)));
        assert_eq!(ob.best_ask(), Some(&dec!(50001)));

        // Remove a level
        let diff2 = DepthDiff {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel { price: dec!(50000), quantity: dec!(0) }],
            asks: vec![],
            first_update_id: 2,
            last_update_id: 2,
        };
        ob.apply_diff(&diff2);
        assert_eq!(ob.best_bid(), Some(&dec!(49999)));
    }

    #[test]
    fn test_walk_asks() {
        let mut ob = OrderBook::new("BTCUSDT".into());
        ob.asks.insert(dec!(100), dec!(1.0));
        ob.asks.insert(dec!(101), dec!(2.0));
        ob.asks.insert(dec!(102), dec!(3.0));

        // Buy 2.0: should fill 1.0 @ 100 + 1.0 @ 101 = 201 total, effective 100.5
        let (eff_price, qty) = ob.walk_asks(dec!(2.0)).unwrap();
        assert_eq!(qty, dec!(2.0));
        assert_eq!(eff_price, dec!(100.5));

        // Buy 10.0: not enough liquidity
        assert!(ob.walk_asks(dec!(10.0)).is_none());
    }

    #[test]
    fn test_staleness() {
        let ob = OrderBook::new("TEST".into());
        assert!(ob.is_stale(1000)); // never updated = stale

        let mut ob2 = OrderBook::new("TEST".into());
        let diff = DepthDiff {
            symbol: "TEST".into(),
            bids: vec![PriceLevel { price: dec!(100), quantity: dec!(1.0) }],
            asks: vec![],
            first_update_id: 1,
            last_update_id: 1,
        };
        ob2.apply_diff(&diff);
        assert!(!ob2.is_stale(1000)); // just updated
    }

    #[test]
    fn test_walk_bids() {
        let mut ob = OrderBook::new("BTCUSDT".into());
        ob.bids.insert(dec!(100), dec!(1.0));
        ob.bids.insert(dec!(99), dec!(2.0));
        ob.bids.insert(dec!(98), dec!(3.0));

        // Sell 2.0: should fill 1.0 @ 100 + 1.0 @ 99 = 199 total, effective 99.5
        let (eff_price, qty) = ob.walk_bids(dec!(2.0)).unwrap();
        assert_eq!(qty, dec!(2.0));
        assert_eq!(eff_price, dec!(99.5));
    }
}
