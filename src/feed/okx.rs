use std::str::FromStr;

use futures::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::feed::types::FeedEvent;

/// OKX public WebSocket endpoint.
const OKX_WS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

/// Connects directly to OKX WebSocket for real-time price data.
/// Uses the tickers channel for best bid/ask updates.
pub struct OkxFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl OkxFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to OKX tickers for the given instrument IDs.
    /// Instrument IDs should be like "BTC-USDT", "ETH-USDT".
    pub fn subscribe(&self, inst_ids: Vec<String>) -> Result<()> {
        if inst_ids.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let ids_log = inst_ids.clone();
        tokio::spawn(async move {
            loop {
                match run_okx_stream(tx.clone(), &inst_ids).await {
                    Ok(()) => {
                        warn!("okx stream ended cleanly — reconnecting in 1s");
                    }
                    Err(e) => {
                        error!(error = %e, "okx stream failed — reconnecting in 1s");
                    }
                }
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "okx".into(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(FeedEvent::Reconnected {
                    source: "okx".into(),
                });
            }
        });

        info!(instruments = ?ids_log, "subscribed to okx price feed");
        Ok(())
    }
}

/// Map config asset name to OKX instrument ID.
/// e.g. "BTC" -> "BTC-USDT", "ETH" -> "ETH-USDT"
pub fn asset_to_okx_inst_id(asset: &str) -> String {
    format!("{}-USDT", asset.to_uppercase())
}

/// OKX subscription message.
#[derive(serde::Serialize)]
struct SubscribeMsg {
    op: String,
    args: Vec<SubscribeArg>,
}

#[derive(serde::Serialize)]
struct SubscribeArg {
    channel: String,
    #[serde(rename = "instId")]
    inst_id: String,
}

/// OKX ticker response.
#[derive(serde::Deserialize)]
struct TickerResponse {
    arg: Option<TickerArg>,
    data: Option<Vec<TickerData>>,
}

#[derive(serde::Deserialize)]
struct TickerArg {
    channel: String,
    #[allow(dead_code)]
    #[serde(rename = "instId")]
    inst_id: String,
}

/// OKX ticker data payload.
#[derive(serde::Deserialize)]
struct TickerData {
    /// Instrument ID (e.g. "BTC-USDT")
    #[serde(rename = "instId")]
    inst_id: String,
    /// Best bid price
    #[serde(rename = "bidPx")]
    bid_px: String,
    /// Best ask price
    #[serde(rename = "askPx")]
    ask_px: String,
    /// Timestamp in milliseconds
    ts: String,
}

async fn run_okx_stream(
    tx: broadcast::Sender<FeedEvent>,
    inst_ids: &[String],
) -> Result<()> {
    info!(url = OKX_WS_URL, "connecting to okx WebSocket");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(OKX_WS_URL)
        .await
        .map_err(|e| Error::Feed(format!("okx WS connect failed: {e}")))?;

    info!("okx WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    // Send subscription message
    let sub_msg = SubscribeMsg {
        op: "subscribe".into(),
        args: inst_ids
            .iter()
            .map(|id| SubscribeArg {
                channel: "tickers".into(),
                inst_id: id.clone(),
            })
            .collect(),
    };
    let sub_json = serde_json::to_string(&sub_msg)
        .map_err(|e| Error::Feed(format!("failed to serialize subscribe msg: {e}")))?;
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(sub_json.into()))
        .await
        .map_err(|e| Error::Feed(format!("okx WS send failed: {e}")))?;

    info!(instruments = ?inst_ids, "okx subscription sent");

    let mut msg_count: u64 = 0;

    while let Some(msg_result) = read.next().await {
        let recv_at = std::time::Instant::now();

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "okx WS read error");
                return Err(Error::Feed(format!("okx WS error: {e}")));
            }
        };

        let text = match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => t,
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                info!("okx WS received close frame");
                break;
            }
            _ => continue,
        };

        let parsed: TickerResponse = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "failed to parse okx message");
                continue;
            }
        };

        // Only process ticker channel data
        let is_ticker = parsed
            .arg
            .as_ref()
            .is_some_and(|a| a.channel == "tickers");
        if !is_ticker {
            continue;
        }

        let Some(data_list) = parsed.data else {
            continue;
        };

        for data in &data_list {
            let bid = match Decimal::from_str(&data.bid_px) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let ask = match Decimal::from_str(&data.ask_px) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let mid = (bid + ask) / Decimal::TWO;
            // OKX provides timestamp in ms
            let timestamp = data.ts.parse::<i64>().unwrap_or_else(|_| {
                chrono::Utc::now().timestamp_millis()
            });
            let symbol = data.inst_id.clone();

            let process_us = recv_at.elapsed().as_micros();

            msg_count += 1;
            if msg_count.is_multiple_of(1000) {
                info!(
                    msg_count = msg_count,
                    process_us = process_us,
                    "okx feed stats"
                );
            }

            debug!(
                symbol = %symbol,
                bid = %bid,
                ask = %ask,
                mid = %mid,
                process_us = process_us,
                "okx price update"
            );

            let _ = tx.send(FeedEvent::CryptoPrice {
                exchange: "okx".into(),
                symbol,
                price: mid,
                timestamp,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_to_inst_id_mapping() {
        assert_eq!(asset_to_okx_inst_id("BTC"), "BTC-USDT");
        assert_eq!(asset_to_okx_inst_id("ETH"), "ETH-USDT");
        assert_eq!(asset_to_okx_inst_id("sol"), "SOL-USDT");
    }

    #[test]
    fn parses_ticker_message() {
        let json = r#"{
            "arg": {
                "channel": "tickers",
                "instId": "BTC-USDT"
            },
            "data": [{
                "instType": "SPOT",
                "instId": "BTC-USDT",
                "last": "69000.00",
                "lastSz": "0.1",
                "askPx": "69000.50",
                "askSz": "11",
                "bidPx": "68999.50",
                "bidSz": "5",
                "open24h": "68000",
                "high24h": "70000",
                "low24h": "67000",
                "volCcy24h": "2222",
                "vol24h": "2222",
                "ts": "1700000000000"
            }]
        }"#;

        let msg: TickerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(msg.arg.unwrap().channel, "tickers");
        let data = &msg.data.unwrap()[0];
        assert_eq!(data.inst_id, "BTC-USDT");
        let bid = Decimal::from_str(&data.bid_px).unwrap();
        let ask = Decimal::from_str(&data.ask_px).unwrap();
        assert!(bid > Decimal::ZERO);
        assert!(ask > bid);
        assert_eq!(data.ts, "1700000000000");
    }
}
