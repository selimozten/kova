pub mod binance;
pub mod paribu;
pub mod types;

use std::collections::HashMap;

use rust_decimal::Decimal;
use thiserror::Error;
use tokio::sync::broadcast;

use types::{DepthDiff, OrderBook, OrderRequest, OrderResult, SymbolInfo};

#[derive(Error, Debug)]
pub enum ExchangeError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("API error: {code} - {msg}")]
    Api { code: i64, msg: String },

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Auth error: {0}")]
    Auth(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

/// Exchange event for monitoring/logging.
#[derive(Debug, Clone)]
pub enum ExchangeEvent {
    Connected,
    Disconnected,
    Reconnecting,
    DepthUpdate(String),
    OrderPlaced(String),
    Error(String),
}

/// The core exchange trait. Generic over exchange — zero-cost via monomorphization.
/// Uses Rust 1.75+ async fn in traits (RPITIT).
pub trait Exchange: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Subscribe to depth streams for the given symbols.
    /// Depth diffs are sent to the broadcast channel.
    fn subscribe_depth(
        &self,
        symbols: &[String],
        tx: broadcast::Sender<DepthDiff>,
    ) -> impl std::future::Future<Output = Result<(), ExchangeError>> + Send;

    /// Fetch a full order book snapshot.
    fn fetch_order_book_snapshot(
        &self,
        symbol: &str,
        depth: u32,
    ) -> impl std::future::Future<Output = Result<OrderBook, ExchangeError>> + Send;

    /// Place an order.
    fn place_order(
        &self,
        request: &OrderRequest,
    ) -> impl std::future::Future<Output = Result<OrderResult, ExchangeError>> + Send;

    /// Cancel an order.
    fn cancel_order(
        &self,
        symbol: &str,
        order_id: &str,
    ) -> impl std::future::Future<Output = Result<(), ExchangeError>> + Send;

    /// Get all balances.
    fn get_balances(
        &self,
    ) -> impl std::future::Future<Output = Result<HashMap<String, Decimal>, ExchangeError>> + Send;

    /// Get maker/taker fee rates for a symbol.
    fn get_fee_rates(
        &self,
        symbol: &str,
    ) -> impl std::future::Future<Output = Result<(Decimal, Decimal), ExchangeError>> + Send;

    /// Get symbol trading rules/filters.
    fn get_symbol_info(
        &self,
        symbol: &str,
    ) -> impl std::future::Future<Output = Result<SymbolInfo, ExchangeError>> + Send;

    /// Get all tradeable symbols and their info.
    fn get_all_symbols(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<SymbolInfo>, ExchangeError>> + Send;
}
