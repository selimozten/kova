use rust_decimal::Decimal;
use serde::Deserialize;

/// Binance API error response.
#[derive(Debug, Deserialize)]
pub struct BinanceApiError {
    pub code: i64,
    pub msg: String,
}

/// Binance exchangeInfo response.
#[derive(Debug, Deserialize)]
pub struct ExchangeInfo {
    pub symbols: Vec<BinanceSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceSymbol {
    pub symbol: String,
    pub status: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub filters: Vec<SymbolFilter>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "filterType")]
pub enum SymbolFilter {
    #[serde(rename = "PRICE_FILTER")]
    #[serde(rename_all = "camelCase")]
    PriceFilter {
        #[serde(with = "rust_decimal::serde::str")]
        tick_size: Decimal,
    },
    #[serde(rename = "LOT_SIZE")]
    #[serde(rename_all = "camelCase")]
    LotSize {
        #[serde(with = "rust_decimal::serde::str")]
        min_qty: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        max_qty: Decimal,
        #[serde(with = "rust_decimal::serde::str")]
        step_size: Decimal,
    },
    #[serde(rename = "NOTIONAL")]
    #[serde(rename_all = "camelCase")]
    Notional {
        #[serde(with = "rust_decimal::serde::str")]
        min_notional: Decimal,
    },
    #[serde(rename = "MIN_NOTIONAL")]
    #[serde(rename_all = "camelCase")]
    MinNotional {
        #[serde(with = "rust_decimal::serde::str")]
        min_notional: Decimal,
    },
    #[serde(other)]
    Other,
}

/// Binance depth snapshot response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepthSnapshot {
    pub last_update_id: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

/// Binance WebSocket depth update.
#[derive(Debug, Deserialize)]
pub struct WsDepthUpdate {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "U")]
    pub first_update_id: u64,
    #[serde(rename = "u")]
    pub last_update_id: u64,
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

/// Binance order response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOrderResponse {
    pub order_id: u64,
    pub symbol: String,
    pub side: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub executed_qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub cummulative_quote_qty: Decimal,
    pub fills: Option<Vec<BinanceFill>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceFill {
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub commission: Decimal,
    pub commission_asset: String,
}

/// Binance account info response.
#[derive(Debug, Deserialize)]
pub struct AccountInfo {
    pub balances: Vec<BalanceInfo>,
}

#[derive(Debug, Deserialize)]
pub struct BalanceInfo {
    pub asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub free: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub locked: Decimal,
}
