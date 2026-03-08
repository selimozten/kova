use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Client;
use rust_decimal::Decimal;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::auth;
use super::models::*;
use crate::exchange::types::*;
use crate::exchange::ExchangeError;

pub struct ParibuRest {
    client: Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    rate_limiter: Arc<Semaphore>,
}

impl ParibuRest {
    pub fn new(base_url: &str, api_key: &str, api_secret: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            rate_limiter: Arc::new(Semaphore::new(15)),
        }
    }

    /// Convert internal symbol (BTC_TL) to Paribu native format (btc_tl).
    fn to_paribu_market(symbol: &str) -> String {
        symbol.to_lowercase()
    }

    /// Convert Paribu native format (btc_tl) to internal symbol (BTC_TL).
    fn from_paribu_market(market: &str) -> String {
        market.to_uppercase()
    }

    async fn acquire_rate_limit(&self) -> tokio::sync::OwnedSemaphorePermit {
        let permit = self.rate_limiter.clone().acquire_owned().await.unwrap();
        let limiter = self.rate_limiter.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            drop(limiter);
        });
        permit
    }

    /// Build a signed request: signs query params + body, adds Authorization + X-Signature headers.
    fn sign_data(&self, query: &str, body: &str) -> String {
        let data = if body.is_empty() {
            query.to_string()
        } else if query.is_empty() {
            body.to_string()
        } else {
            format!("{}{}", query, body)
        };
        auth::sign(&data, &self.api_secret)
    }

    fn check_api_error(status: reqwest::StatusCode, text: &str) -> Result<(), ExchangeError> {
        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<ParibuApiError>(text) {
                return Err(ExchangeError::Api {
                    code: err.code,
                    msg: err.message,
                });
            }
            return Err(ExchangeError::Other(format!("HTTP {status}: {text}")));
        }
        Ok(())
    }

    pub async fn get_markets(&self) -> Result<Vec<SymbolInfo>, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let url = format!("{}/markets", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let markets: Vec<ParibuMarket> = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("markets: {e}")))?;

        let mut symbols = Vec::new();
        for m in markets {
            if m.status != "active" {
                continue;
            }
            symbols.push(SymbolInfo {
                symbol: Self::from_paribu_market(&m.market),
                base_asset: m.base_currency.to_uppercase(),
                quote_asset: m.quote_currency.to_uppercase(),
                status: m.status,
                tick_size: m.tick_size,
                step_size: m.step_size,
                min_notional: m.min_total,
                min_qty: m.min_amount,
                max_qty: m.max_amount,
            });
        }

        debug!("loaded {} active paribu markets", symbols.len());
        Ok(symbols)
    }

    pub async fn get_depth(
        &self,
        symbol: &str,
        limit: u32,
    ) -> Result<OrderBook, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let market = Self::to_paribu_market(symbol);
        let url = format!(
            "{}/orderbook?market={}&depth={}",
            self.base_url, market, limit
        );
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        let snapshot: ParibuOrderBook = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("orderbook: {e}")))?;

        let mut ob = OrderBook::new(symbol.to_string());
        ob.last_update_id = snapshot.timestamp;

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
        let market = Self::to_paribu_market(&request.symbol);

        let side = match request.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let order_type = match request.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
        };

        let mut body = serde_json::json!({
            "market": market,
            "type": side,
            "order_type": order_type,
            "amount": request.quantity.to_string(),
        });

        if let Some(price) = request.price {
            body["price"] = serde_json::Value::String(price.to_string());
        }

        let body_str = body.to_string();
        let signature = self.sign_data("", &body_str);
        let url = format!("{}/order", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", &self.api_key)
            .header("X-Signature", &signature)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        Self::check_api_error(status, &text)?;

        let order: ParibuOrderResponse = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("order response: {e}")))?;

        let executed_qty = order.amount - order.remaining_amount;
        let effective_price = if executed_qty > Decimal::ZERO {
            order.average
        } else {
            Decimal::ZERO
        };
        let executed_quote_qty = executed_qty * effective_price;

        let order_status = match order.status.as_str() {
            "filled" => OrderStatus::Filled,
            "partially_filled" => OrderStatus::PartiallyFilled,
            "cancelled" | "canceled" => OrderStatus::Cancelled,
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::New,
        };

        Ok(OrderResult {
            order_id: order.uid,
            symbol: Self::from_paribu_market(&order.market),
            side: request.side,
            status: order_status,
            executed_quantity: executed_qty,
            executed_quote_quantity: executed_quote_qty,
            effective_price,
        })
    }

    pub async fn cancel_order(
        &self,
        _symbol: &str,
        order_id: &str,
    ) -> Result<(), ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let signature = self.sign_data(&format!("orderId={}", order_id), "");
        let url = format!("{}/order/{}", self.base_url, order_id);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", &self.api_key)
            .header("X-Signature", &signature)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            Self::check_api_error(status, &text)?;
        }

        Ok(())
    }

    pub async fn get_balances(&self) -> Result<HashMap<String, Decimal>, ExchangeError> {
        let _permit = self.acquire_rate_limit().await;
        let signature = self.sign_data("", "");
        let url = format!("{}/user/assets", self.base_url);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", &self.api_key)
            .header("X-Signature", &signature)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        Self::check_api_error(status, &text)?;

        let assets: Vec<ParibuBalance> = serde_json::from_str(&text)
            .map_err(|e| ExchangeError::Parse(format!("balances: {e}")))?;

        let mut balances = HashMap::new();
        for b in assets {
            if b.available > Decimal::ZERO {
                balances.insert(b.currency.to_uppercase(), b.available);
            }
        }

        Ok(balances)
    }

    pub async fn get_symbol_info(
        &self,
        symbol: &str,
    ) -> Result<SymbolInfo, ExchangeError> {
        let all = self.get_markets().await?;
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
        warn!("using configured fee rates for paribu");
        Ok((default_maker, default_taker))
    }
}
