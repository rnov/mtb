use std::str::FromStr;

use futures::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::feed::types::FeedEvent;

/// Coinbase Advanced Trade WebSocket endpoint.
const COINBASE_WS_URL: &str = "wss://advanced-trade-ws.coinbase.com";

/// Connects directly to Coinbase WebSocket for real-time price data.
/// Uses the ticker channel for best bid/ask updates.
pub struct CoinbaseFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl CoinbaseFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to Coinbase ticker for the given product IDs.
    /// Product IDs should be like "BTC-USD", "ETH-USD".
    pub fn subscribe(&self, product_ids: Vec<String>) -> Result<()> {
        if product_ids.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let products_log = product_ids.clone();
        tokio::spawn(async move {
            loop {
                match run_coinbase_stream(tx.clone(), &product_ids).await {
                    Ok(()) => {
                        warn!("coinbase stream ended cleanly — reconnecting in 1s");
                    }
                    Err(e) => {
                        error!(error = %e, "coinbase stream failed — reconnecting in 1s");
                    }
                }
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "coinbase".into(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(FeedEvent::Reconnected {
                    source: "coinbase".into(),
                });
            }
        });

        info!(products = ?products_log, "subscribed to coinbase price feed");
        Ok(())
    }
}

/// Map config asset name to Coinbase product ID.
/// e.g. "BTC" -> "BTC-USD", "ETH" -> "ETH-USD"
pub fn asset_to_coinbase_product(asset: &str) -> String {
    format!("{}-USD", asset.to_uppercase())
}

/// Coinbase ticker subscription message.
#[derive(serde::Serialize)]
struct SubscribeMsg {
    #[serde(rename = "type")]
    msg_type: String,
    product_ids: Vec<String>,
    channel: String,
}

/// Coinbase ticker response (top-level).
#[derive(serde::Deserialize)]
struct TickerResponse {
    channel: String,
    #[allow(dead_code)]
    timestamp: Option<String>,
    events: Option<Vec<TickerEvent>>,
}

/// Coinbase ticker event.
#[derive(serde::Deserialize)]
struct TickerEvent {
    tickers: Option<Vec<TickerData>>,
}

/// Coinbase ticker data payload.
#[derive(serde::Deserialize)]
struct TickerData {
    product_id: String,
    best_bid: Option<String>,
    best_ask: Option<String>,
}

async fn run_coinbase_stream(
    tx: broadcast::Sender<FeedEvent>,
    product_ids: &[String],
) -> Result<()> {
    info!(url = COINBASE_WS_URL, "connecting to coinbase WebSocket");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(COINBASE_WS_URL)
        .await
        .map_err(|e| Error::Feed(format!("coinbase WS connect failed: {e}")))?;

    info!("coinbase WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    // Send subscription message
    let sub_msg = SubscribeMsg {
        msg_type: "subscribe".into(),
        product_ids: product_ids.to_vec(),
        channel: "ticker".into(),
    };
    let sub_json = serde_json::to_string(&sub_msg)
        .map_err(|e| Error::Feed(format!("failed to serialize subscribe msg: {e}")))?;
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            sub_json.into(),
        ))
        .await
        .map_err(|e| Error::Feed(format!("coinbase WS send failed: {e}")))?;

    info!(products = ?product_ids, "coinbase subscription sent");

    let mut msg_count: u64 = 0;

    while let Some(msg_result) = read.next().await {
        let recv_at = std::time::Instant::now();

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "coinbase WS read error");
                return Err(Error::Feed(format!("coinbase WS error: {e}")));
            }
        };

        let text = match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => t,
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                info!("coinbase WS received close frame");
                break;
            }
            _ => continue,
        };

        let parsed: TickerResponse = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "failed to parse coinbase message");
                continue;
            }
        };

        if parsed.channel != "ticker" {
            continue;
        }

        let Some(events) = parsed.events else {
            continue;
        };

        for event in &events {
            let Some(tickers) = &event.tickers else {
                continue;
            };

            for ticker in tickers {
                let bid = ticker
                    .best_bid
                    .as_deref()
                    .and_then(|s| Decimal::from_str(s).ok());
                let ask = ticker
                    .best_ask
                    .as_deref()
                    .and_then(|s| Decimal::from_str(s).ok());

                let (Some(bid), Some(ask)) = (bid, ask) else {
                    continue;
                };

                let mid = (bid + ask) / Decimal::TWO;
                let timestamp = chrono::Utc::now().timestamp_millis();
                let symbol = ticker.product_id.clone();

                let process_us = recv_at.elapsed().as_micros();

                msg_count += 1;
                if msg_count.is_multiple_of(1000) {
                    info!(
                        msg_count = msg_count,
                        process_us = process_us,
                        "coinbase feed stats"
                    );
                }

                debug!(
                    symbol = %symbol,
                    bid = %bid,
                    ask = %ask,
                    mid = %mid,
                    process_us = process_us,
                    "coinbase price update"
                );

                let _ = tx.send(FeedEvent::CryptoPrice {
                    exchange: "coinbase".into(),
                    symbol,
                    price: mid,
                    timestamp,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_to_product_mapping() {
        assert_eq!(asset_to_coinbase_product("BTC"), "BTC-USD");
        assert_eq!(asset_to_coinbase_product("ETH"), "ETH-USD");
        assert_eq!(asset_to_coinbase_product("sol"), "SOL-USD");
    }

    #[test]
    fn parses_ticker_message() {
        let json = r#"{
            "channel": "ticker",
            "client_id": "",
            "timestamp": "2023-02-09T20:30:37.167359596Z",
            "sequence_num": 0,
            "events": [{
                "type": "snapshot",
                "tickers": [{
                    "type": "ticker",
                    "product_id": "BTC-USD",
                    "price": "69000.00",
                    "best_bid": "68999.50",
                    "best_ask": "69000.50",
                    "best_bid_quantity": "1.5",
                    "best_ask_quantity": "2.0"
                }]
            }]
        }"#;

        let msg: TickerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(msg.channel, "ticker");
        let event = &msg.events.unwrap()[0];
        let ticker = &event.tickers.as_ref().unwrap()[0];
        assert_eq!(ticker.product_id, "BTC-USD");
        let bid = Decimal::from_str(ticker.best_bid.as_ref().unwrap()).unwrap();
        let ask = Decimal::from_str(ticker.best_ask.as_ref().unwrap()).unwrap();
        assert!(bid > Decimal::ZERO);
        assert!(ask > bid);
    }
}
