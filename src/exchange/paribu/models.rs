use rust_decimal::Decimal;
use serde::Deserialize;

/// Paribu API error response.
#[derive(Debug, Deserialize)]
pub struct ParibuApiError {
    #[serde(default)]
    pub error: bool,
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

/// Paribu market info from GET /markets.
#[derive(Debug, Deserialize)]
pub struct ParibuMarket {
    pub market: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub tick_size: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub step_size: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_amount: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub max_amount: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_total: Decimal,
}

/// Paribu order book snapshot.
#[derive(Debug, Deserialize)]
pub struct ParibuOrderBook {
    pub timestamp: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

/// Paribu order response.
#[derive(Debug, Deserialize)]
pub struct ParibuOrderResponse {
    pub uid: String,
    pub market: String,
    #[serde(rename = "type")]
    pub trade: String,
    pub status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub remaining_amount: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub average: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub total: Decimal,
}

/// Paribu balance info.
#[derive(Debug, Deserialize)]
pub struct ParibuBalance {
    pub currency: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub total: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub available: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub blocked_in_orders: Decimal,
}

/// Paribu WebSocket message envelope.
#[derive(Debug, Deserialize)]
pub struct ParibuWsMessage {
    pub channel: String,
    pub data: serde_json::Value,
}

/// Paribu WebSocket depth data payload.
#[derive(Debug, Deserialize)]
pub struct ParibuWsDepthData {
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_paribu_api_error() {
        let json = r#"{"error": true, "code": 1001, "message": "Invalid market"}"#;
        let err: ParibuApiError = serde_json::from_str(json).unwrap();
        assert!(err.error);
        assert_eq!(err.code, 1001);
        assert_eq!(err.message, "Invalid market");
    }

    #[test]
    fn test_parse_paribu_market() {
        let json = r#"{
            "market": "btc_tl",
            "base_currency": "btc",
            "quote_currency": "tl",
            "status": "active",
            "tick_size": "0.01",
            "step_size": "0.00001",
            "min_amount": "0.0001",
            "max_amount": "100.0",
            "min_total": "10.0"
        }"#;
        let market: ParibuMarket = serde_json::from_str(json).unwrap();
        assert_eq!(market.market, "btc_tl");
        assert_eq!(market.base_currency, "btc");
        assert_eq!(market.quote_currency, "tl");
    }

    #[test]
    fn test_parse_paribu_orderbook() {
        let json = r#"{
            "timestamp": 1700000000,
            "bids": [["50000.00", "1.5"], ["49999.00", "2.0"]],
            "asks": [["50001.00", "0.5"], ["50002.00", "1.0"]]
        }"#;
        let ob: ParibuOrderBook = serde_json::from_str(json).unwrap();
        assert_eq!(ob.bids.len(), 2);
        assert_eq!(ob.asks.len(), 2);
        assert_eq!(ob.timestamp, 1700000000);
    }

    #[test]
    fn test_parse_paribu_order_response() {
        let json = r#"{
            "uid": "order-123",
            "market": "btc_tl",
            "type": "buy",
            "status": "filled",
            "remaining_amount": "0.0",
            "average": "50000.00",
            "amount": "1.0",
            "total": "50000.00"
        }"#;
        let order: ParibuOrderResponse = serde_json::from_str(json).unwrap();
        assert_eq!(order.uid, "order-123");
        assert_eq!(order.trade, "buy");
        assert_eq!(order.remaining_amount, rust_decimal_macros::dec!(0.0));
    }

    #[test]
    fn test_parse_paribu_balance() {
        let json = r#"{
            "currency": "btc",
            "total": "1.5",
            "available": "1.0",
            "blocked_in_orders": "0.5"
        }"#;
        let bal: ParibuBalance = serde_json::from_str(json).unwrap();
        assert_eq!(bal.currency, "btc");
        assert_eq!(bal.available, rust_decimal_macros::dec!(1.0));
    }

    #[test]
    fn test_parse_paribu_ws_message() {
        let json = r#"{
            "channel": "orderbook:btc_tl@100ms",
            "data": {
                "bids": [["50000.00", "1.0"]],
                "asks": [["50001.00", "0.5"]],
                "timestamp": 1700000000
            }
        }"#;
        let msg: ParibuWsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.channel, "orderbook:btc_tl@100ms");

        let depth: ParibuWsDepthData = serde_json::from_value(msg.data).unwrap();
        assert_eq!(depth.bids.len(), 1);
        assert_eq!(depth.timestamp, 1700000000);
    }
}
