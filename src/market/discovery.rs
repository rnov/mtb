use chrono::{Timelike, Utc};
use polymarket_client_sdk::gamma::types::request::EventBySlugRequest;
use polymarket_client_sdk::gamma::Client as GammaClient;
use tracing::{debug, info, warn};

use crate::config::types::DiscoveryConfig;
use crate::error::{Error, Result};
use crate::market::types::MarketPair;

/// Asset slugs for 15-minute crypto up/down markets.
const ASSET_SLUGS: &[(&str, &str)] = &[
    ("BTC", "btc"),
    ("ETH", "eth"),
    ("SOL", "sol"),
    ("XRP", "xrp"),
];

/// Discovers active 15-minute crypto up/down markets from the Gamma API.
///
/// These markets are ephemeral — a new event is created every 15 minutes with a
/// slug like `btc-updown-15m-<epoch_seconds>`, where the epoch is the start time
/// of the 15-minute window aligned to ET.
pub struct MarketDiscovery {
    client: GammaClient,
    config: DiscoveryConfig,
}

impl MarketDiscovery {
    pub fn new(config: DiscoveryConfig) -> Result<Self> {
        let client = GammaClient::default();
        Ok(Self { client, config })
    }

    /// Fetch currently active 15-minute markets for all configured assets.
    pub async fn discover(&self) -> Result<Vec<MarketPair>> {
        let epoch = current_15min_epoch();
        let mut pairs = Vec::new();

        for &(asset, slug_prefix) in ASSET_SLUGS {
            // Only discover assets the user configured
            if !self
                .config
                .assets
                .iter()
                .any(|a| a.eq_ignore_ascii_case(asset))
            {
                continue;
            }

            let slug = format!("{slug_prefix}-updown-15m-{epoch}");
            debug!(slug = %slug, asset = %asset, "fetching market by slug");

            match self.fetch_by_slug(&slug, asset).await {
                Ok(Some(pair)) => {
                    info!(
                        market_id = %pair.market_id,
                        asset = %pair.asset,
                        question = %pair.question,
                        "discovered market"
                    );
                    pairs.push(pair);
                }
                Ok(None) => {
                    debug!(slug = %slug, "no active market found for slug");
                }
                Err(e) => {
                    warn!(slug = %slug, error = %e, "failed to fetch market");
                }
            }
        }

        info!(count = pairs.len(), "market discovery complete");
        Ok(pairs)
    }

    /// Fetch a single event by slug and extract the market pair.
    async fn fetch_by_slug(&self, slug: &str, asset: &str) -> Result<Option<MarketPair>> {
        let request = EventBySlugRequest::builder().slug(slug).build();

        let event =
            self.client.event_by_slug(&request).await.map_err(|e| {
                Error::Discovery(format!("gamma event_by_slug failed for {slug}: {e}"))
            })?;

        let markets = match &event.markets {
            Some(m) => m,
            None => return Ok(None),
        };

        for market in markets {
            let active = market.active.unwrap_or(false);
            let closed = market.closed.unwrap_or(true);
            let accepting = market.accepting_orders.unwrap_or(false);

            if !active || closed || !accepting {
                debug!(slug = %slug, "market not active/accepting");
                continue;
            }

            let token_ids = match &market.clob_token_ids {
                Some(ids) if ids.len() == 2 => ids,
                _ => {
                    debug!(slug = %slug, "expected 2 token IDs");
                    continue;
                }
            };

            let outcomes = match &market.outcomes {
                Some(o) if o.len() == 2 => o,
                _ => {
                    warn!(slug = %slug, "expected 2 outcomes");
                    continue;
                }
            };

            // Match token IDs to Up/Down by outcome name — never assume array order
            let (up_token_id, down_token_id) = if outcomes[0] == "Up" && outcomes[1] == "Down" {
                (token_ids[0].to_string(), token_ids[1].to_string())
            } else if outcomes[0] == "Down" && outcomes[1] == "Up" {
                (token_ids[1].to_string(), token_ids[0].to_string())
            } else {
                warn!(
                    slug = %slug,
                    outcomes = ?outcomes,
                    "unexpected outcome names (expected Up/Down)"
                );
                continue;
            };

            info!(
                slug = %slug,
                outcomes = ?outcomes,
                up_token = %up_token_id,
                down_token = %down_token_id,
                "token-to-outcome mapping"
            );

            return Ok(Some(MarketPair {
                market_id: market.id.clone(),
                question: market.question.clone().unwrap_or_default(),
                asset: asset.to_string(),
                slug: slug.to_string(),
                up_token_id,
                down_token_id,
                end_date: event.end_date,
                liquidity: market.liquidity,
                neg_risk: market.neg_risk.unwrap_or(false),
            }));
        }

        Ok(None)
    }
}

/// Compute the Unix epoch (seconds) for the current 15-minute window start.
/// Aligned to 15-minute boundaries in UTC.
fn current_15min_epoch() -> i64 {
    let now = Utc::now();
    let minute = (now.minute() / 15) * 15;
    let aligned = now
        .with_minute(minute)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    aligned.timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live integration test: discovers markets from real Gamma API.
    /// Run with: cargo test -- --ignored discovery
    #[tokio::test]
    #[ignore]
    async fn live_discover_markets() {
        let config = DiscoveryConfig {
            poll_interval_secs: 60,
            assets: vec!["BTC".into(), "ETH".into(), "SOL".into(), "XRP".into()],
        };
        let discovery = MarketDiscovery::new(config).unwrap();
        let markets = discovery.discover().await.unwrap();

        println!("discovered {} markets:", markets.len());
        for m in &markets {
            println!(
                "  {} | {} | up={} | down={} | neg_risk={}\n    question: {}",
                m.asset,
                m.market_id,
                &m.up_token_id[..20],
                &m.down_token_id[..20],
                m.neg_risk,
                m.question
            );
        }

        // Should find at least 1 market (markets run 24/7)
        assert!(!markets.is_empty(), "expected at least one active market");

        // Each market should have valid token IDs
        for m in &markets {
            assert!(!m.up_token_id.is_empty());
            assert!(!m.down_token_id.is_empty());
            assert_ne!(m.up_token_id, m.down_token_id);
        }
    }
}
