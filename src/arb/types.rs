use rust_decimal::Decimal;

use crate::exchange::types::OrderSide;

/// A triangle path: three symbols forming a cycle back to the start asset.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrianglePath {
    pub legs: [TradeLeg; 3],
    pub start_asset: String,
}

impl std::fmt::Display for TrianglePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} -> {} {} -> {} {}",
            self.legs[0].symbol,
            self.legs[0].side,
            self.legs[1].symbol,
            self.legs[1].side,
            self.legs[2].symbol,
            self.legs[2].side,
        )
    }
}

/// A single leg of a triangle trade.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TradeLeg {
    pub symbol: String,
    pub side: OrderSide,
    pub base_asset: String,
    pub quote_asset: String,
}

/// A detected arbitrage opportunity with calculated profit.
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub path: TrianglePath,
    pub legs: [LegDetail; 3],
    pub start_amount: Decimal,
    pub end_amount: Decimal,
    pub profit_amount: Decimal,
    pub profit_pct: Decimal,
    /// When this opportunity was detected (epoch ms).
    pub detected_at_ms: u64,
}

impl std::fmt::Display for ArbitrageOpportunity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:.4}%] {} | {:.6} {} -> {:.6} {} (profit: {:.6})",
            self.profit_pct,
            self.path,
            self.start_amount,
            self.path.start_asset,
            self.end_amount,
            self.path.start_asset,
            self.profit_amount,
        )
    }
}

/// Details for one leg of an evaluated opportunity.
#[derive(Debug, Clone)]
pub struct LegDetail {
    pub symbol: String,
    pub side: OrderSide,
    pub quantity: Decimal,
    pub effective_price: Decimal,
    pub fee: Decimal,
    pub input_amount: Decimal,
    pub output_amount: Decimal,
}
