use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::models::{ParibuWsDepthData, ParibuWsMessage};
use crate::exchange::types::{DepthDiff, PriceLevel};
use crate::exchange::ExchangeError;

/// Connect to Paribu WebSocket depth stream and forward updates.
pub async fn subscribe_depth(
    ws_url: &str,
    symbols: &[String],
    tx: broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    if symbols.is_empty() {
        return Ok(());
    }

    info!("connecting to Paribu WS: {} streams", symbols.len());

    loop {
        match connect_and_stream(ws_url, symbols, &tx).await {
            Ok(()) => {
                info!("Paribu WebSocket stream ended cleanly");
                break;
            }
            Err(e) => {
                error!("Paribu WebSocket error: {e}, reconnecting in 5s...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }

    Ok(())
}

async fn connect_and_stream(
    ws_url: &str,
    symbols: &[String],
    tx: &broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    let (ws_stream, _) = connect_async(ws_url)
        .await
        .map_err(|e| ExchangeError::WebSocket(format!("connect: {e}")))?;

    info!("Paribu WebSocket connected");

    let (write, mut read) = ws_stream.split();
    let write = Arc::new(Mutex::new(write));

    // Send subscription message
    let channels: Vec<String> = symbols
        .iter()
        .map(|s| format!("orderbook:{}@100ms", s.to_lowercase()))
        .collect();

    let sub_msg = serde_json::json!({
        "method": "subscribe",
        "channels": channels,
        "id": "sub_1"
    });

    {
        let mut w = write.lock().await;
        w.send(Message::Text(sub_msg.to_string()))
            .await
            .map_err(|e| ExchangeError::WebSocket(format!("subscribe: {e}")))?;
    }
    debug!("sent subscription for {} channels", channels.len());

    // Spawn ping task: send ping every 25 seconds
    let ping_write = write.clone();
    let ping_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(25));
        loop {
            interval.tick().await;
            let mut w = ping_write.lock().await;
            if w.send(Message::Ping(vec![])).await.is_err() {
                break;
            }
            debug!("sent ping to Paribu WS");
        }
    });

    // Read loop
    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| ExchangeError::WebSocket(format!("read: {e}")))?;

        match msg {
            Message::Text(text) => {
                if let Err(e) = process_message(&text, tx) {
                    debug!("skipping message: {e}");
                }
            }
            Message::Pong(_) => {
                debug!("received pong");
            }
            Message::Close(_) => {
                warn!("received close frame");
                break;
            }
            _ => {}
        }
    }

    ping_handle.abort();
    Ok(())
}

fn process_message(
    text: &str,
    tx: &broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    let msg: ParibuWsMessage =
        serde_json::from_str(text).map_err(|e| ExchangeError::Parse(format!("ws json: {e}")))?;

    // Extract market from channel: "orderbook:btc_tl@100ms" -> "BTC_TL"
    let market = msg
        .channel
        .strip_prefix("orderbook:")
        .and_then(|s| s.split('@').next())
        .ok_or_else(|| ExchangeError::Parse(format!("bad channel: {}", msg.channel)))?;
    let symbol = market.to_uppercase();

    let depth: ParibuWsDepthData = serde_json::from_value(msg.data)
        .map_err(|e| ExchangeError::Parse(format!("depth data: {e}")))?;

    let diff = DepthDiff {
        symbol,
        bids: parse_levels(&depth.bids)?,
        asks: parse_levels(&depth.asks)?,
        first_update_id: depth.timestamp,
        last_update_id: depth.timestamp,
    };

    let _ = tx.send(diff);
    Ok(())
}

fn parse_levels(raw: &[[String; 2]]) -> Result<Vec<PriceLevel>, ExchangeError> {
    raw.iter()
        .map(|[p, q]| {
            let price: Decimal = p
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("level price: {e}")))?;
            let quantity: Decimal = q
                .parse()
                .map_err(|e| ExchangeError::Parse(format!("level qty: {e}")))?;
            Ok(PriceLevel { price, quantity })
        })
        .collect()
}
