use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

mod config;
mod error;
mod execution;
mod feed;
mod market;
mod monitor;
mod risk;
mod strategy;

use config::RunMode;
use execution::{DryRunExecutor, Executor};
use feed::{FeedEvent, OrderbookFeed};
use market::MarketDiscovery;
use monitor::TradeLogger;
use risk::RiskManager;
use strategy::{detect_arb, MarketSnapshot};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration
    let app_config = config::load_config("config.toml")?;
    let _secrets = config::load_secrets().unwrap_or_else(|e| {
        eprintln!("Warning: {e} (ok for dry-run mode)");
        config::Secrets {
            private_key: String::new(),
        }
    });

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(&app_config.logging.filter)
        .with_target(true)
        .init();

    info!(mode = ?app_config.mode, "mtb starting");

    // Initialize trade logger
    let logger = TradeLogger::new(&app_config.logging.trade_log_path)?;

    // Initialize executor based on mode
    let executor: Box<dyn Executor> = match app_config.mode {
        RunMode::DryRun => {
            info!("running in DRY RUN mode — no real orders will be placed");
            Box::new(DryRunExecutor::new(logger))
        }
        RunMode::Live => {
            anyhow::bail!("live mode not yet implemented (Phase 2)");
        }
    };

    // Initialize risk manager
    let risk_manager = RiskManager::new(app_config.risk.clone());

    // Discover markets
    let discovery = MarketDiscovery::new(app_config.discovery.clone())?;
    info!("discovering active 15-minute crypto markets...");

    let markets = discovery.discover().await?;
    if markets.is_empty() {
        warn!("no active markets found — will retry on next poll");
    } else {
        info!(count = markets.len(), "active markets found");
    }

    // Collect all token IDs for orderbook subscription
    let mut all_token_ids = Vec::new();
    for m in &markets {
        all_token_ids.push(m.up_token_id.clone());
        all_token_ids.push(m.down_token_id.clone());
    }

    // Set up broadcast channel for feed events
    let (tx, mut rx) = broadcast::channel::<FeedEvent>(1024);

    // Start orderbook feed
    let orderbook_feed = OrderbookFeed::new(tx);
    if !all_token_ids.is_empty() {
        orderbook_feed.subscribe(all_token_ids)?;
    }

    // Build lookup: token_id -> market index + whether it's up/down
    let mut token_to_market: HashMap<String, (usize, bool)> = HashMap::new();
    for (i, m) in markets.iter().enumerate() {
        token_to_market.insert(m.up_token_id.clone(), (i, true));
        token_to_market.insert(m.down_token_id.clone(), (i, false));
    }

    // Latest best bid/ask per token
    let mut best_asks: HashMap<String, Decimal> = HashMap::new();
    let mut best_bids: HashMap<String, Decimal> = HashMap::new();

    // Discovery refresh interval
    let discovery_interval = Duration::from_secs(app_config.discovery.poll_interval_secs);
    let mut discovery_timer = tokio::time::interval(discovery_interval);
    discovery_timer.tick().await; // consume the immediate first tick

    info!("entering main event loop");

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(FeedEvent::OrderbookUpdate(update)) => {
                        // Update latest prices
                        if let Some(bid) = update.best_bid {
                            best_bids.insert(update.token_id.clone(), bid);
                        }
                        if let Some(ask) = update.best_ask {
                            best_asks.insert(update.token_id.clone(), ask);
                        }

                        // Find which market this token belongs to
                        if let Some(&(market_idx, _is_up)) = token_to_market.get(&update.token_id) {
                            let m = &markets[market_idx];

                            // Build snapshot
                            let snapshot = MarketSnapshot {
                                market: m.clone(),
                                up_best_bid: best_bids.get(&m.up_token_id).copied(),
                                up_best_ask: best_asks.get(&m.up_token_id).copied(),
                                down_best_bid: best_bids.get(&m.down_token_id).copied(),
                                down_best_ask: best_asks.get(&m.down_token_id).copied(),
                                timestamp: Utc::now(),
                            };

                            // Run arb detection
                            if let Some(opportunity) = detect_arb(&snapshot, &app_config.strategy) {
                                // Risk check
                                match risk_manager.check(&opportunity) {
                                    Ok(()) => {
                                        match executor.execute(&opportunity).await {
                                            Ok(result) => {
                                                if result.success {
                                                    risk_manager.record_execution(opportunity.suggested_size_usdc);
                                                    risk_manager.reset_failures();
                                                } else {
                                                    risk_manager.record_failure();
                                                }
                                            }
                                            Err(e) => {
                                                error!(error = %e, "execution failed");
                                                risk_manager.record_failure();
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "risk check rejected opportunity");
                                    }
                                }
                            }
                        }
                    }
                    Ok(FeedEvent::Disconnected { source }) => {
                        warn!(source = %source, "feed disconnected");
                    }
                    Ok(FeedEvent::Reconnected { source }) => {
                        info!(source = %source, "feed reconnected");
                    }
                    Ok(_) => {} // CryptoPrice events (Phase 3)
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(missed = n, "feed receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        error!("feed channel closed — shutting down");
                        break;
                    }
                }
            }
            _ = discovery_timer.tick() => {
                info!("refreshing market discovery...");
                match discovery.discover().await {
                    Ok(new_markets) => {
                        info!(count = new_markets.len(), "market refresh complete");
                        // TODO: update market list, subscribe to new tokens
                    }
                    Err(e) => {
                        warn!(error = %e, "market discovery refresh failed");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received Ctrl+C — shutting down");
                break;
            }
        }
    }

    info!("mtb shutdown complete");
    Ok(())
}
