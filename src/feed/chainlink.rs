use futures::StreamExt;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::feed::types::FeedEvent;

/// Connects to Polymarket RTDS for Chainlink Data Streams prices.
/// Chainlink is the resolution source for 15-min crypto markets — knowing
/// its price is knowing the outcome before the market closes.
pub struct ChainlinkFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl ChainlinkFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to Chainlink prices for the given assets.
    /// Assets should be like "BTC", "ETH", "SOL", "XRP".
    pub fn subscribe(&self, assets: Vec<String>) -> crate::error::Result<()> {
        if assets.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let assets_log = assets.clone();
        tokio::spawn(async move {
            loop {
                match run_chainlink_stream(tx.clone(), &assets).await {
                    Ok(()) => {
                        warn!("chainlink RTDS stream ended cleanly — reconnecting in 1s");
                    }
                    Err(e) => {
                        error!(error = %e, "chainlink RTDS stream failed — reconnecting in 1s");
                    }
                }
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "chainlink".into(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(FeedEvent::Reconnected {
                    source: "chainlink".into(),
                });
            }
        });

        info!(assets = ?assets_log, "subscribed to chainlink price feed via RTDS");
        Ok(())
    }
}

/// Map config asset name to Chainlink symbol format.
/// e.g. "BTC" -> "btc/usd", "ETH" -> "eth/usd"
pub fn asset_to_chainlink_symbol(asset: &str) -> String {
    format!("{}/usd", asset.to_lowercase())
}

async fn run_chainlink_stream(
    tx: broadcast::Sender<FeedEvent>,
    assets: &[String],
) -> crate::error::Result<()> {
    use polymarket_client_sdk::rtds::Client as RtdsClient;

    info!("connecting to RTDS for chainlink prices");

    let client = RtdsClient::default();

    // Subscribe to each asset's Chainlink feed
    // The SDK only accepts Option<String> (single symbol), so subscribe to all
    // and filter on our side
    let stream = client
        .subscribe_chainlink_prices(None)
        .map_err(|e| crate::error::Error::Feed(format!("chainlink RTDS subscribe failed: {e}")))?;

    info!(assets = ?assets, "chainlink RTDS subscription active");

    let mut stream = Box::pin(stream);
    let mut msg_count: u64 = 0;

    // Build set of symbols we care about
    let wanted_symbols: Vec<String> = assets
        .iter()
        .map(|a| asset_to_chainlink_symbol(a))
        .collect();

    while let Some(result) = stream.next().await {
        let recv_at = std::time::Instant::now();

        match result {
            Ok(price) => {
                // Filter to only our assets
                if !wanted_symbols.contains(&price.symbol) {
                    continue;
                }

                let process_us = recv_at.elapsed().as_micros();

                msg_count += 1;
                if msg_count.is_multiple_of(100) {
                    info!(
                        msg_count = msg_count,
                        process_us = process_us,
                        "chainlink feed stats"
                    );
                }

                debug!(
                    symbol = %price.symbol,
                    value = %price.value,
                    timestamp = price.timestamp,
                    process_us = process_us,
                    "chainlink price update"
                );

                let _ = tx.send(FeedEvent::ChainlinkPrice {
                    symbol: price.symbol,
                    price: price.value,
                    timestamp: price.timestamp,
                });
            }
            Err(e) => {
                warn!(error = %e, "chainlink RTDS message error");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_to_symbol_mapping() {
        assert_eq!(asset_to_chainlink_symbol("BTC"), "btc/usd");
        assert_eq!(asset_to_chainlink_symbol("ETH"), "eth/usd");
        assert_eq!(asset_to_chainlink_symbol("SOL"), "sol/usd");
        assert_eq!(asset_to_chainlink_symbol("XRP"), "xrp/usd");
    }

    #[test]
    fn asset_to_symbol_case_insensitive() {
        assert_eq!(asset_to_chainlink_symbol("btc"), "btc/usd");
        assert_eq!(asset_to_chainlink_symbol("Eth"), "eth/usd");
    }
}
