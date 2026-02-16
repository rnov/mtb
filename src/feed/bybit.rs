use std::str::FromStr;

use futures::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::feed::types::FeedEvent;

/// Bybit V5 public WebSocket endpoint (spot).
const BYBIT_WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";

/// Connects directly to Bybit WebSocket for real-time spot price data.
/// Uses the tickers channel for last traded price updates.
pub struct BybitFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl BybitFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to Bybit tickers for the given symbols.
    /// Symbols should be like "BTCUSDT", "ETHUSDT".
    pub fn subscribe(&self, symbols: Vec<String>) -> Result<()> {
        if symbols.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let symbols_log = symbols.clone();
        tokio::spawn(async move {
            loop {
                match run_bybit_stream(tx.clone(), &symbols).await {
                    Ok(()) => {
                        warn!("bybit stream ended cleanly — reconnecting in 1s");
                    }
                    Err(e) => {
                        error!(error = %e, "bybit stream failed — reconnecting in 1s");
                    }
                }
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "bybit".into(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(FeedEvent::Reconnected {
                    source: "bybit".into(),
                });
            }
        });

        info!(symbols = ?symbols_log, "subscribed to bybit price feed");
        Ok(())
    }
}

/// Map config asset name to Bybit symbol.
/// e.g. "BTC" -> "BTCUSDT", "ETH" -> "ETHUSDT"
pub fn asset_to_bybit_symbol(asset: &str) -> String {
    format!("{}USDT", asset.to_uppercase())
}

/// Bybit ticker response (spot).
#[derive(serde::Deserialize)]
struct TickerResponse {
    topic: Option<String>,
    /// Server timestamp in milliseconds
    ts: Option<i64>,
    data: Option<TickerData>,
}

/// Bybit spot ticker data payload.
#[derive(serde::Deserialize)]
struct TickerData {
    /// Symbol (e.g. "BTCUSDT")
    symbol: String,
    /// Last traded price
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
}

async fn run_bybit_stream(tx: broadcast::Sender<FeedEvent>, symbols: &[String]) -> Result<()> {
    info!(url = BYBIT_WS_URL, "connecting to bybit WebSocket");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(BYBIT_WS_URL)
        .await
        .map_err(|e| Error::Feed(format!("bybit WS connect failed: {e}")))?;

    info!("bybit WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    // Send subscription message
    // Bybit V5 format: {"op":"subscribe","args":["tickers.BTCUSDT","tickers.ETHUSDT"]}
    let args: Vec<String> = symbols.iter().map(|s| format!("tickers.{s}")).collect();
    let sub_json = serde_json::json!({
        "op": "subscribe",
        "args": args
    })
    .to_string();

    write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            sub_json.into(),
        ))
        .await
        .map_err(|e| Error::Feed(format!("bybit WS send failed: {e}")))?;

    info!(symbols = ?symbols, "bybit subscription sent");

    let mut msg_count: u64 = 0;

    while let Some(msg_result) = read.next().await {
        let recv_at = std::time::Instant::now();

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "bybit WS read error");
                return Err(Error::Feed(format!("bybit WS error: {e}")));
            }
        };

        let text = match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => t,
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                info!("bybit WS received close frame");
                break;
            }
            _ => continue,
        };

        let parsed: TickerResponse = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "failed to parse bybit message");
                continue;
            }
        };

        // Only process ticker topics
        let is_ticker = parsed
            .topic
            .as_ref()
            .is_some_and(|t| t.starts_with("tickers."));
        if !is_ticker {
            continue;
        }

        let Some(data) = parsed.data else {
            continue;
        };

        let Some(ref last_price_str) = data.last_price else {
            continue;
        };

        let price = match Decimal::from_str(last_price_str) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let timestamp = parsed
            .ts
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let symbol = data.symbol;

        let process_us = recv_at.elapsed().as_micros();

        msg_count += 1;
        if msg_count.is_multiple_of(1000) {
            info!(
                msg_count = msg_count,
                process_us = process_us,
                "bybit feed stats"
            );
        }

        debug!(
            symbol = %symbol,
            price = %price,
            process_us = process_us,
            "bybit price update"
        );

        let _ = tx.send(FeedEvent::CryptoPrice {
            exchange: "bybit".into(),
            symbol,
            price,
            timestamp,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_to_symbol_mapping() {
        assert_eq!(asset_to_bybit_symbol("BTC"), "BTCUSDT");
        assert_eq!(asset_to_bybit_symbol("ETH"), "ETHUSDT");
        assert_eq!(asset_to_bybit_symbol("sol"), "SOLUSDT");
    }

    #[test]
    fn parses_ticker_message() {
        let json = r#"{
            "topic": "tickers.BTCUSDT",
            "ts": 1673853746003,
            "type": "snapshot",
            "cs": 2588407389,
            "data": {
                "symbol": "BTCUSDT",
                "lastPrice": "69000.50",
                "highPrice24h": "70000",
                "lowPrice24h": "67000",
                "prevPrice24h": "68000",
                "volume24h": "6780.866843",
                "turnover24h": "141946527.22907118",
                "price24hPcnt": "0.0196",
                "usdIndexPrice": "69010.24"
            }
        }"#;

        let msg: TickerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(msg.topic.unwrap(), "tickers.BTCUSDT");
        assert_eq!(msg.ts.unwrap(), 1673853746003);
        let data = msg.data.unwrap();
        assert_eq!(data.symbol, "BTCUSDT");
        let price = Decimal::from_str(data.last_price.as_ref().unwrap()).unwrap();
        assert_eq!(price, Decimal::from_str("69000.50").unwrap());
    }
}
