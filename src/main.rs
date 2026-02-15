use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

mod config;
mod error;
mod execution;
mod feed;
mod market;
mod monitor;
mod risk;
mod strategy;

use config::RunMode;
use execution::{DryRunExecutor, Executor, LiveExecutor};
use feed::{
    ChainlinkFeed, CoinbaseFeed, CryptoPriceFeed, FeedEvent, OkxFeed, OrderbookFeed, PriceState,
};
use market::MarketDiscovery;
use monitor::TradeLogger;
use risk::RiskManager;
use strategy::{detect_arb, MarketSnapshot};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration
    let app_config = config::load_config("config.toml")?;
    let secrets = config::load_secrets();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(&app_config.logging.filter)
        .with_target(true)
        .init();

    info!(mode = ?app_config.mode, "mtb starting");

    // Geoblock check — verify we can trade from this IP
    {
        use polymarket_client_sdk::clob::Client as ClobClient;
        let clob = ClobClient::default();
        match clob.check_geoblock().await {
            Ok(response) => {
                if response.blocked {
                    anyhow::bail!(
                        "geo-blocked: trading not available from {} ({})",
                        response.country,
                        response.ip
                    );
                }
                info!(
                    ip = %response.ip,
                    country = %response.country,
                    "geoblock check passed"
                );
            }
            Err(e) => {
                warn!(error = %e, "geoblock check failed — proceeding with caution");
            }
        }
    }

    // Initialize trade logger (owned by main loop, not executors)
    let logger = TradeLogger::new(&app_config.logging.trade_log_path)?;

    // Initialize executor based on mode
    let executor: Box<dyn Executor> = match app_config.mode {
        RunMode::DryRun => {
            info!("running in DRY RUN mode — no real orders will be placed");
            Box::new(DryRunExecutor::new())
        }
        RunMode::Live => {
            if secrets.private_key.is_empty() {
                anyhow::bail!("live mode requires POLYMARKET_PRIVATE_KEY in .env");
            }
            info!("running in LIVE mode — real orders will be placed!");
            Box::new(LiveExecutor::new(
                app_config.clob_url.clone(),
                app_config.chain_id,
                secrets.private_key.clone(),
            ))
        }
    };

    // Initialize risk manager (Arc for sharing with Telegram command handler)
    let risk_manager = Arc::new(RiskManager::new(app_config.risk.clone()));

    // Initialize Telegram notifier (optional — needs bot token + chat_id)
    #[cfg(feature = "telegram")]
    let notifier: Option<monitor::Notifier> =
        match (&secrets.telegram_bot_token, &app_config.telegram) {
            (Some(token), Some(tg_config)) => {
                Some(monitor::Notifier::new(token, tg_config.chat_id))
            }
            _ => {
                info!("telegram notifications disabled (missing token or config)");
                None
            }
        };

    // Start Telegram command handler (background task)
    #[cfg(feature = "telegram")]
    if let Some(ref n) = notifier {
        n.start_command_handler(Arc::clone(&risk_manager));
    }

    // Discover markets
    let discovery = MarketDiscovery::new(app_config.discovery.clone())?;
    info!("discovering active 15-minute crypto markets...");

    let markets = discovery.discover().await?;
    if markets.is_empty() {
        warn!("no active markets found — will retry on next poll");
    } else {
        info!(count = markets.len(), "active markets found");
    }

    // Send startup notification
    #[cfg(feature = "telegram")]
    if let Some(ref n) = notifier {
        let mode_str = match app_config.mode {
            RunMode::DryRun => "Dry Run",
            RunMode::Live => "LIVE",
        };
        n.notify_startup(mode_str, markets.len()).await;
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
    let orderbook_feed = OrderbookFeed::new(tx.clone());
    if !all_token_ids.is_empty() {
        orderbook_feed.subscribe(all_token_ids)?;
    }

    // Start Binance direct price feed
    let binance_symbols: Vec<String> = app_config
        .strategy
        .assets
        .iter()
        .map(|a| feed::crypto_price::asset_to_binance_symbol(a))
        .collect();
    let price_feed = CryptoPriceFeed::new(tx.clone());
    price_feed.subscribe(binance_symbols)?;

    // Start Coinbase direct price feed
    let coinbase_products: Vec<String> = app_config
        .strategy
        .assets
        .iter()
        .map(|a| feed::coinbase::asset_to_coinbase_product(a))
        .collect();
    let coinbase_feed = CoinbaseFeed::new(tx.clone());
    coinbase_feed.subscribe(coinbase_products)?;

    // Start OKX direct price feed
    let okx_instruments: Vec<String> = app_config
        .strategy
        .assets
        .iter()
        .map(|a| feed::okx::asset_to_okx_inst_id(a))
        .collect();
    let okx_feed = OkxFeed::new(tx.clone());
    okx_feed.subscribe(okx_instruments)?;

    // Start Chainlink price feed via RTDS (resolution source)
    let chainlink_feed = ChainlinkFeed::new(tx);
    chainlink_feed.subscribe(app_config.strategy.assets.clone())?;

    // Build lookup: token_id -> market index + whether it's up/down
    let mut token_to_market: HashMap<String, (usize, bool)> = HashMap::new();
    for (i, m) in markets.iter().enumerate() {
        token_to_market.insert(m.up_token_id.clone(), (i, true));
        token_to_market.insert(m.down_token_id.clone(), (i, false));
    }

    // Latest best bid/ask per token
    let mut best_asks: HashMap<String, Decimal> = HashMap::new();
    let mut best_bids: HashMap<String, Decimal> = HashMap::new();

    // Price state: latest Binance prices per symbol
    let mut price_state = PriceState::new();

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
                        let tick_start = std::time::Instant::now();

                        // Record feed activity for staleness tracking
                        risk_manager.record_feed_activity();

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

                            // Run arb detection (pure computation — should be <1μs)
                            let arb_start = std::time::Instant::now();
                            let arb_result = detect_arb(&snapshot, &app_config.strategy);
                            let arb_us = arb_start.elapsed().as_micros();

                            if let Some(opportunity) = arb_result {
                                // Risk check
                                let risk_start = std::time::Instant::now();
                                let risk_result = risk_manager.check(&opportunity);
                                let risk_us = risk_start.elapsed().as_micros();

                                match risk_result {
                                    Ok(()) => {
                                        let exec_start = std::time::Instant::now();
                                        match executor.execute(&opportunity).await {
                                            Ok(result) => {
                                                let exec_ms = exec_start.elapsed().as_millis();
                                                let total_ms = tick_start.elapsed().as_millis();

                                                info!(
                                                    asset = %opportunity.asset,
                                                    arb_us = arb_us,
                                                    risk_us = risk_us,
                                                    exec_ms = exec_ms,
                                                    total_ms = total_ms,
                                                    success = result.success,
                                                    "latency: tick-to-trade"
                                                );

                                                // Log trade with full execution details
                                                if let Err(e) = logger.log_trade(&opportunity, &result) {
                                                    error!(error = %e, "failed to write trade log");
                                                }
                                                // Telegram alert for trades
                                                #[cfg(feature = "telegram")]
                                                if let Some(ref n) = notifier {
                                                    n.notify_trade(&opportunity, &result).await;
                                                }
                                                if result.success {
                                                    risk_manager.record_execution(opportunity.suggested_size_usdc);
                                                    risk_manager.reset_failures();
                                                } else {
                                                    risk_manager.record_failure();
                                                    #[cfg(feature = "telegram")]
                                                    if risk_manager.is_killed() {
                                                        if let Some(ref n) = notifier {
                                                            n.notify_kill_switch("consecutive failures threshold").await;
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let exec_ms = exec_start.elapsed().as_millis();
                                                let total_ms = tick_start.elapsed().as_millis();

                                                error!(
                                                    error = %e,
                                                    exec_ms = exec_ms,
                                                    total_ms = total_ms,
                                                    "execution failed"
                                                );
                                                #[cfg(feature = "telegram")]
                                                if let Some(ref n) = notifier {
                                                    n.notify_execution_error(&opportunity.asset, &e.to_string()).await;
                                                }
                                                risk_manager.record_failure();
                                                #[cfg(feature = "telegram")]
                                                if risk_manager.is_killed() {
                                                    if let Some(ref n) = notifier {
                                                        n.notify_kill_switch("consecutive failures threshold").await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let reason = e.to_string();
                                        let total_us = tick_start.elapsed().as_micros();
                                        warn!(
                                            error = %reason,
                                            arb_us = arb_us,
                                            risk_us = risk_us,
                                            total_us = total_us,
                                            "risk check rejected opportunity"
                                        );
                                        // Log rejected opportunities too (for analysis)
                                        if let Err(le) = logger.log_rejected(&opportunity, &reason) {
                                            error!(error = %le, "failed to write rejection log");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(FeedEvent::Disconnected { source }) => {
                        risk_manager.record_disconnect(&source);
                        #[cfg(feature = "telegram")]
                        if let Some(ref n) = notifier {
                            n.notify_disconnect(&source).await;
                        }
                    }
                    Ok(FeedEvent::Reconnected { source }) => {
                        risk_manager.record_reconnect(&source);
                        #[cfg(feature = "telegram")]
                        if let Some(ref n) = notifier {
                            n.notify_reconnect(&source).await;
                        }
                    }
                    Ok(FeedEvent::CryptoPrice { exchange, symbol, price, timestamp }) => {
                        price_state.update(&exchange, &symbol, price, timestamp);
                        debug!(
                            exchange = %exchange,
                            symbol = %symbol,
                            price = %price,
                            "price update"
                        );
                    }
                    Ok(FeedEvent::ChainlinkPrice { symbol, price, timestamp }) => {
                        price_state.update("chainlink", &symbol, price, timestamp);
                        debug!(
                            symbol = %symbol,
                            price = %price,
                            "chainlink price update"
                        );
                    }
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
