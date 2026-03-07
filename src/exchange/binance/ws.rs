use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::models::WsDepthUpdate;
use crate::exchange::types::{DepthDiff, PriceLevel};
use crate::exchange::ExchangeError;

/// Connect to Binance combined depth WebSocket stream and forward updates.
pub async fn subscribe_depth(
    ws_url: &str,
    symbols: &[String],
    tx: broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    if symbols.is_empty() {
        return Ok(());
    }

    // Build combined stream URL: wss://stream.binance.com:9443/stream?streams=btcusdt@depth/ethusdt@depth
    let streams: Vec<String> = symbols
        .iter()
        .map(|s| format!("{}@depth@100ms", s.to_lowercase()))
        .collect();
    let url = format!("{}/stream?streams={}", ws_url, streams.join("/"));

    info!("connecting to Binance WS: {} streams", symbols.len());

    loop {
        match connect_and_stream(&url, &tx).await {
            Ok(()) => {
                info!("WebSocket stream ended cleanly");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {e}, reconnecting in 5s...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }

    Ok(())
}

async fn connect_and_stream(
    url: &str,
    tx: &broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    let (ws_stream, _) = connect_async(url)
        .await
        .map_err(|e| ExchangeError::WebSocket(format!("connect: {e}")))?;

    info!("WebSocket connected");

    let (mut _write, mut read) = ws_stream.split();

    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| ExchangeError::WebSocket(format!("read: {e}")))?;

        match msg {
            Message::Text(text) => {
                if let Err(e) = process_message(&text, tx) {
                    debug!("skipping message: {e}");
                }
            }
            Message::Ping(data) => {
                debug!("received ping, sending pong");
                let _ = _write.send(Message::Pong(data)).await;
            }
            Message::Close(_) => {
                warn!("received close frame");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Combined stream wrapper: {"stream":"btcusdt@depth","data":{...}}
#[derive(serde::Deserialize)]
struct CombinedStream {
    data: serde_json::Value,
}

fn process_message(
    text: &str,
    tx: &broadcast::Sender<DepthDiff>,
) -> Result<(), ExchangeError> {
    // Try combined stream format first
    let data = if let Ok(combined) = serde_json::from_str::<CombinedStream>(text) {
        combined.data
    } else {
        serde_json::from_str(text).map_err(|e| ExchangeError::Parse(format!("ws json: {e}")))?
    };

    let update: WsDepthUpdate =
        serde_json::from_value(data).map_err(|e| ExchangeError::Parse(format!("depth: {e}")))?;

    let diff = DepthDiff {
        symbol: update.symbol,
        bids: parse_levels(&update.bids)?,
        asks: parse_levels(&update.asks)?,
        first_update_id: update.first_update_id,
        last_update_id: update.last_update_id,
    };

    // Ignore send errors (no receivers).
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
