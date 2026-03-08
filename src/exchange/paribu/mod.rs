pub mod auth;
pub mod models;
pub mod rest;
pub mod ws;

use std::collections::HashMap;

use rust_decimal::Decimal;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::exchange::types::*;
use crate::exchange::{Exchange, ExchangeError};

const DEFAULT_BASE_URL: &str = "https://api.paribu.com";
const DEFAULT_WS_URL: &str = "wss://stream.paribu.com";

pub struct ParibuExchange {
    rest: rest::ParibuRest,
    ws_url: String,
    maker_fee: Decimal,
    taker_fee: Decimal,
}

impl ParibuExchange {
    pub async fn new(config: &Config) -> Result<Self, ExchangeError> {
        let base_url = config
            .exchange
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL);
        let ws_url = config
            .exchange
            .ws_url
            .as_deref()
            .unwrap_or(DEFAULT_WS_URL)
            .to_string();

        Ok(Self {
            rest: rest::ParibuRest::new(base_url, &config.exchange.api_key, &config.exchange.api_secret),
            ws_url,
            maker_fee: config.exchange.fees.maker,
            taker_fee: config.exchange.fees.taker,
        })
    }
}

impl Exchange for ParibuExchange {
    fn name(&self) -> &str {
        "paribu"
    }

    async fn subscribe_depth(
        &self,
        symbols: &[String],
        tx: broadcast::Sender<DepthDiff>,
    ) -> Result<(), ExchangeError> {
        ws::subscribe_depth(&self.ws_url, symbols, tx).await
    }

    async fn fetch_order_book_snapshot(
        &self,
        symbol: &str,
        depth: u32,
    ) -> Result<OrderBook, ExchangeError> {
        self.rest.get_depth(symbol, depth).await
    }

    async fn place_order(
        &self,
        request: &OrderRequest,
    ) -> Result<OrderResult, ExchangeError> {
        self.rest.place_order(request).await
    }

    async fn cancel_order(
        &self,
        symbol: &str,
        order_id: &str,
    ) -> Result<(), ExchangeError> {
        self.rest.cancel_order(symbol, order_id).await
    }

    async fn get_balances(&self) -> Result<HashMap<String, Decimal>, ExchangeError> {
        self.rest.get_balances().await
    }

    async fn get_fee_rates(
        &self,
        symbol: &str,
    ) -> Result<(Decimal, Decimal), ExchangeError> {
        self.rest
            .get_fee_rates(symbol, self.maker_fee, self.taker_fee)
            .await
    }

    async fn get_symbol_info(
        &self,
        symbol: &str,
    ) -> Result<SymbolInfo, ExchangeError> {
        self.rest.get_symbol_info(symbol).await
    }

    async fn get_all_symbols(&self) -> Result<Vec<SymbolInfo>, ExchangeError> {
        self.rest.get_markets().await
    }
}
