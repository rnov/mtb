use std::str::FromStr;

use futures::StreamExt;
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::feed::types::FeedEvent;

/// Binance WebSocket combined stream endpoint.
const BINANCE_WS_BASE: &str = "wss://stream.binance.com:9443/stream";

/// Connects directly to Binance WebSocket for real-time price data.
/// Uses the bookTicker stream for lowest-latency best bid/ask updates.
pub struct CryptoPriceFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl CryptoPriceFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to Binance bookTicker for the given symbols.
    /// Symbols should be lowercase pairs like "btcusdt", "ethusdt".
    /// Spawns a background task that streams into the broadcast channel.
    pub fn subscribe(&self, symbols: Vec<String>) -> Result<()> {
        if symbols.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let symbols_log = symbols.clone();
        tokio::spawn(async move {
            loop {
                match run_binance_stream(tx.clone(), &symbols).await {
                    Ok(()) => {
                        warn!("binance stream ended cleanly — reconnecting in 1s");
                    }
                    Err(e) => {
                        error!(error = %e, "binance stream failed — reconnecting in 1s");
                    }
                }
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "binance".into(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(FeedEvent::Reconnected {
                    source: "binance".into(),
                });
            }
        });

        info!(symbols = ?symbols_log, "subscribed to binance price feed");
        Ok(())
    }
}

/// Map config asset name to Binance symbol.
/// e.g. "BTC" -> "btcusdt", "ETH" -> "ethusdt"
pub fn asset_to_binance_symbol(asset: &str) -> String {
    format!("{}usdt", asset.to_lowercase())
}

/// Build the combined stream URL for multiple symbols.
/// Format: wss://stream.binance.com:9443/stream?streams=btcusdt@bookTicker/ethusdt@bookTicker
fn build_stream_url(symbols: &[String]) -> String {
    let streams: Vec<String> = symbols.iter().map(|s| format!("{s}@bookTicker")).collect();
    format!("{BINANCE_WS_BASE}?streams={}", streams.join("/"))
}

/// Binance combined stream wrapper message.
#[derive(serde::Deserialize)]
struct CombinedStreamMsg {
    #[allow(dead_code)]
    stream: String,
    data: BookTickerData,
}

/// Binance bookTicker payload.
/// Docs: https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams#individual-symbol-book-ticker-streams
#[derive(serde::Deserialize)]
struct BookTickerData {
    /// Symbol (uppercase, e.g. "BTCUSDT")
    s: String,
    /// Best bid price
    b: String,
    /// Best ask price
    a: String,
}

async fn run_binance_stream(
    tx: broadcast::Sender<FeedEvent>,
    symbols: &[String],
) -> Result<()> {
    let url = build_stream_url(symbols);

    info!(url = %url, "connecting to binance WebSocket");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| Error::Feed(format!("binance WS connect failed: {e}")))?;

    info!("binance WebSocket connected");

    let (_, mut read) = ws_stream.split();
    let mut msg_count: u64 = 0;

    while let Some(msg_result) = read.next().await {
        let recv_at = std::time::Instant::now();

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "binance WS read error");
                return Err(Error::Feed(format!("binance WS error: {e}")));
            }
        };

        // Only process text messages (Binance sends JSON as text)
        let text = match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => t,
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                info!("binance WS received close frame");
                break;
            }
            _ => continue,
        };

        let parsed: CombinedStreamMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "failed to parse binance message");
                continue;
            }
        };

        let data = &parsed.data;

        // Parse bid/ask as Decimal for precision
        let bid = match Decimal::from_str(&data.b) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let ask = match Decimal::from_str(&data.a) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Mid price as the reference price
        let mid = (bid + ask) / Decimal::TWO;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let symbol = data.s.to_lowercase();

        // Measure processing time (parse + broadcast)
        let process_us = recv_at.elapsed().as_micros();

        msg_count += 1;
        // Log processing latency every 1000th message (avoid spamming)
        if msg_count.is_multiple_of(1000) {
            info!(
                msg_count = msg_count,
                process_us = process_us,
                "binance feed stats"
            );
        }

        debug!(
            symbol = %symbol,
            bid = %bid,
            ask = %ask,
            mid = %mid,
            process_us = process_us,
            "binance price update"
        );

        let _ = tx.send(FeedEvent::CryptoPrice {
            exchange: "binance".into(),
            symbol,
            price: mid,
            timestamp,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_correct_stream_url() {
        let symbols = vec!["btcusdt".into(), "ethusdt".into()];
        let url = build_stream_url(&symbols);
        assert_eq!(
            url,
            "wss://stream.binance.com:9443/stream?streams=btcusdt@bookTicker/ethusdt@bookTicker"
        );
    }

    #[test]
    fn asset_to_symbol_mapping() {
        assert_eq!(asset_to_binance_symbol("BTC"), "btcusdt");
        assert_eq!(asset_to_binance_symbol("ETH"), "ethusdt");
        assert_eq!(asset_to_binance_symbol("Sol"), "solusdt");
    }

    #[test]
    fn parses_book_ticker_message() {
        let json = r#"{
            "stream": "btcusdt@bookTicker",
            "data": {
                "u": 400900217,
                "s": "BTCUSDT",
                "b": "25351.90000000",
                "B": "31.21000000",
                "a": "25365.20000000",
                "A": "40.66000000"
            }
        }"#;

        let msg: CombinedStreamMsg = serde_json::from_str(json).unwrap();
        assert_eq!(msg.data.s, "BTCUSDT");
        let bid = Decimal::from_str(&msg.data.b).unwrap();
        let ask = Decimal::from_str(&msg.data.a).unwrap();
        assert!(bid > Decimal::ZERO);
        assert!(ask > bid);
    }
}
