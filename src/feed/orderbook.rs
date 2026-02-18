use std::str::FromStr;

use futures::StreamExt;
use rust_decimal::Decimal;
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::types::U256;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};
use crate::feed::types::{BestBidAsk, FeedEvent, MAX_ASK_DEPTH};

/// Subscribes to CLOB WebSocket orderbook data and publishes BestBidAsk updates.
pub struct OrderbookFeed {
    tx: broadcast::Sender<FeedEvent>,
}

impl OrderbookFeed {
    pub fn new(tx: broadcast::Sender<FeedEvent>) -> Self {
        Self { tx }
    }

    /// Subscribe to orderbook updates for the given token IDs.
    /// This spawns a background task that streams updates into the broadcast channel.
    pub fn subscribe(&self, token_ids: Vec<String>) -> Result<()> {
        if token_ids.is_empty() {
            return Ok(());
        }

        let tx = self.tx.clone();
        let asset_ids: Vec<U256> = token_ids
            .iter()
            .map(|id| U256::from_str(id))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Feed(format!("invalid token ID: {e}")))?;

        let token_ids_for_task = token_ids.clone();
        tokio::spawn(async move {
            if let Err(e) = run_orderbook_stream(tx, asset_ids, &token_ids_for_task).await {
                error!(error = %e, "orderbook feed failed");
            }
        });

        info!(count = token_ids.len(), "subscribed to orderbook feed");
        Ok(())
    }
}

async fn run_orderbook_stream(
    tx: broadcast::Sender<FeedEvent>,
    asset_ids: Vec<U256>,
    _token_ids: &[String],
) -> Result<()> {
    let client = WsClient::default();

    let stream = client
        .subscribe_orderbook(asset_ids)
        .map_err(|e| Error::Feed(format!("orderbook subscription failed: {e}")))?;

    let mut stream = Box::pin(stream);

    info!("orderbook WebSocket stream started");

    while let Some(result) = stream.next().await {
        match result {
            Ok(book) => {
                let asset_id_str = book.asset_id.to_string();

                // Extract best bid (highest price in bids).
                // The CLOB WS sorts bids ascending (lowest first), so best bid is last.
                let best_bid = book.bids.last().map(|level| level.price);
                // Extract best ask (lowest price in asks).
                // The CLOB WS sorts asks descending (highest first), so best ask is last.
                let best_ask = book.asks.last().map(|level| level.price);

                // Collect ask-side depth levels sorted ascending (cheapest first).
                // WS gives asks descending, so we reverse. Cap at MAX_ASK_DEPTH.
                let ask_levels: Vec<(Decimal, Decimal)> = book
                    .asks
                    .iter()
                    .rev()
                    .take(MAX_ASK_DEPTH)
                    .map(|level| (level.price, level.size))
                    .collect();

                let update = BestBidAsk {
                    token_id: asset_id_str.clone(),
                    best_bid,
                    best_ask,
                    ask_levels,
                    timestamp: book.timestamp,
                };

                debug!(
                    token_id = %asset_id_str,
                    best_bid = ?best_bid,
                    best_ask = ?best_ask,
                    "orderbook update"
                );

                let _ = tx.send(FeedEvent::OrderbookUpdate(update));
            }
            Err(e) => {
                warn!(error = %e, "orderbook stream error");
                let _ = tx.send(FeedEvent::Disconnected {
                    source: "orderbook".into(),
                });
            }
        }
    }

    warn!("orderbook stream ended");
    let _ = tx.send(FeedEvent::Disconnected {
        source: "orderbook".into(),
    });
    Ok(())
}
