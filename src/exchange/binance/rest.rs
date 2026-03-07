use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use rust_decimal::Decimal;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::auth;
use super::models::*;
use crate::exchange::types::*;
use crate::exchange::ExchangeError;

pub struct BinanceRest {
    client: Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    rate_limiter: Arc<Semaphore>,
}

impl BinanceRest {
    pub fn new(base_url: &str, api_key: &str, api_secret: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            rate_limiter: Arc::new(Semaphore::new(5)),
        }
    }

    async fn acquire_rate_limit(&self) -> tokio::sync::OwnedSemaphorePermit {
        let permit = self.rate_limiter.clone().acquire_owned().await.unwrap();
        let limiter = self.rate_limiter.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            drop(limiter);
        });
        permit
    }

    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis() as u64
    }

    fn sign_query(&self, query: &str) -> String {
        let signature = auth::sign(query, &self.api_secret);
        format!("{query}&signature={signature}")
    }

    pub async fn get_exchange_info(&self) -> Result<Vec<SymbolInfo>, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let url = format!("{}/api/v3/exchangeInfo", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let info: ExchangeInfo = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("exchangeInfo: {e}")))?;

        let mut symbols = Vec::new();
        for s in info.symbols {
            if s.status != "TRADING" {
                continue;
            }
            let mut tick_size = Decimal::ZERO;
            let mut step_size = Decimal::ZERO;
            let mut min_notional = Decimal::ZERO;
            let mut min_qty = Decimal::ZERO;
            let mut max_qty = Decimal::new(i64::MAX, 0);

            for filter in &s.filters {
                match filter {
                    SymbolFilter::PriceFilter { tick_size: ts } => tick_size = *ts,
                    SymbolFilter::LotSize {
                        min_qty: mq,
                        max_qty: xq,
                        step_size: ss,
                    } => {
                        min_qty = *mq;
                        max_qty = *xq;
                        step_size = *ss;
                    }
                    SymbolFilter::Notional { min_notional: mn }
                    | SymbolFilter::MinNotional { min_notional: mn } => {
                        min_notional = *mn;
                    }
                    SymbolFilter::Other => {}
                }
            }

            symbols.push(SymbolInfo {
                symbol: s.symbol,
                base_asset: s.base_asset,
                quote_asset: s.quote_asset,
                status: s.status,
                tick_size,
                step_size,
                min_notional,
                min_qty,
                max_qty,
            });
        }

        debug!("loaded {} trading symbols", symbols.len());
        Ok(symbols)
    }

    pub async fn get_depth(
        &self,
        symbol: &str,
        limit: u32,
    ) -> Result<OrderBook, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let url = format!(
            "{}/api/v3/depth?symbol={}&limit={}",
            self.base_url, symbol, limit
        );
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let snapshot: DepthSnapshot = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("depth: {e}")))?;

        let mut ob = OrderBook::new(symbol.to_string());
        ob.last_update_id = snapshot.last_update_id;

        for [price_str, qty_str] in &snapshot.bids {
            let price: Decimal = price_str
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("bid price: {e}")))?;
            let qty: Decimal = qty_str
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("bid qty: {e}")))?;
            ob.bids.insert(price, qty);
        }
        for [price_str, qty_str] in &snapshot.asks {
            let price: Decimal = price_str
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("ask price: {e}")))?;
            let qty: Decimal = qty_str
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("ask qty: {e}")))?;
            ob.asks.insert(price, qty);
        }

        Ok(ob)
    }

    pub async fn place_order(
        &self,
        request: &OrderRequest,
    ) -> Result<OrderResult, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let mut query = format!(
            "symbol={}&side={}&type={}&quantity={}&timestamp={}",
            request.symbol,
            request.side,
            request.order_type,
            request.quantity,
            Self::timestamp_ms(),
        );

        if let Some(price) = request.price {
            query.push_str(&format!("&price={price}&timeInForce=GTC"));
        }

        if let Some(ref coid) = request.client_order_id {
            query.push_str(&format!("&newClientOrderId={coid}"));
        }

        let signed = self.sign_query(&query);
        let url = format!("{}/api/v3/order?{}", self.base_url, signed);

        let resp = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceApiError>(&text) {
                return Err(ExchangeError::Api {
                    code: err.code,
                    msg: err.msg,
                });
            }
            return Err(ExchangeError::Other(format!("HTTP {status}: {text}")));
        }

        let order: BinanceOrderResponse = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("order response: {e}")))?;

        let effective_price = if order.executed_qty > Decimal::ZERO {
            order.cummulative_quote_qty / order.executed_qty
        } else {
            Decimal::ZERO
        };

        let order_status = match order.status.as_str() {
            "FILLED" => OrderStatus::Filled,
            "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "CANCELED" | "CANCELLED" => OrderStatus::Cancelled,
            "REJECTED" | "EXPIRED" => OrderStatus::Rejected,
            _ => OrderStatus::New,
        };

        Ok(OrderResult {
            order_id: order.order_id.to_string(),
            symbol: order.symbol,
            side: request.side,
            status: order_status,
            executed_quantity: order.executed_qty,
            executed_quote_quantity: order.cummulative_quote_qty,
            effective_price,
        })
    }

    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: &str,
    ) -> Result<(), ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let query = format!(
            "symbol={}&orderId={}&timestamp={}",
            symbol,
            order_id,
            Self::timestamp_ms(),
        );
        let signed = self.sign_query(&query);
        let url = format!("{}/api/v3/order?{}", self.base_url, signed);

        let resp = self
            .client
            .delete(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            if let Ok(err) = serde_json::from_str::<BinanceApiError>(&text) {
                return Err(ExchangeError::Api {
                    code: err.code,
                    msg: err.msg,
                });
            }
            return Err(ExchangeError::Other(format!("cancel HTTP {status}: {text}")));
        }

        Ok(())
    }

    pub async fn get_balances(&self) -> Result<HashMap<String, Decimal>, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let query = format!("timestamp={}", Self::timestamp_ms());
        let signed = self.sign_query(&query);
        let url = format!("{}/api/v3/account?{}", self.base_url, signed);

        let resp = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceApiError>(&text) {
                return Err(ExchangeError::Api {
                    code: err.code,
                    msg: err.msg,
                });
            }
            return Err(ExchangeError::Other(format!("account HTTP {status}: {text}")));
        }

        let account: AccountInfo = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("account: {e}")))?;

        let mut balances = HashMap::new();
        for b in account.balances {
            let total = b.free + b.locked;
            if total > Decimal::ZERO {
                balances.insert(b.asset, total);
            }
        }

        Ok(balances)
    }

    pub async fn get_symbol_info(
        &self,
        symbol: &str,
    ) -> Result<SymbolInfo, ExchangeError> {
        let all = self.get_exchange_info().await?;
        all.into_iter()
            .find(|s| s.symbol == symbol)
            .ok_or_else(|| ExchangeError::NotFound(format!("symbol {symbol} not found")))
    }

    pub async fn get_fee_rates(
        &self,
        _symbol: &str,
        default_maker: Decimal,
        default_taker: Decimal,
    ) -> Result<(Decimal, Decimal), ExchangeError> {
        // Binance has a tradeFee endpoint but it requires special permissions.
        // Fall back to config-provided rates.
        warn!("using configured fee rates (tradeFee endpoint not called)");
        Ok((default_maker, default_taker))
    }
}
