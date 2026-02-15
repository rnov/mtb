//! Integration tests that hit real Polymarket APIs.
//!
//! These tests are `#[ignore]` by default. Run with:
//!   cargo test -- --ignored
//!
//! Unauthenticated tests require only network access.
//! Authenticated tests (auth_*, live_*) also require POLYMARKET_PRIVATE_KEY in .env.

use std::str::FromStr;

use alloy::signers::local::LocalSigner;
use alloy::signers::Signer as _;
use chrono::Utc;
use polymarket_client_sdk::clob::types::request::OrderBookSummaryRequest;
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client as ClobClient, Config};
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use mtb::config::types::{DiscoveryConfig, StrategyConfig};
use mtb::market::MarketDiscovery;
use mtb::strategy::{detect_arb, MarketSnapshot};

/// Load private key from .env. Strips `0x` prefix if present. Panics if not set.
fn load_private_key() -> String {
    let _ = dotenvy::dotenv();
    let key = std::env::var("POLYMARKET_PRIVATE_KEY")
        .expect("POLYMARKET_PRIVATE_KEY must be set in .env for auth tests");
    key.strip_prefix("0x").unwrap_or(&key).to_string()
}

const CLOB_URL: &str = "https://clob.polymarket.com";
const POLYGON_CHAIN_ID: u64 = 137;

/// Helper: build an OrderBookSummaryRequest from a token ID string.
fn book_request(token_id_str: &str) -> OrderBookSummaryRequest {
    let token_id = U256::from_str(token_id_str).expect("valid token ID");
    OrderBookSummaryRequest::builder()
        .token_id(token_id)
        .build()
}

/// Full pipeline: discover markets → fetch orderbook via REST → run arb detection.
/// This exercises the entire data path without placing any orders.
#[tokio::test]
#[ignore]
async fn pipeline_discover_fetch_detect() {
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into(), "ETH".into(), "SOL".into(), "XRP".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();

    assert!(!markets.is_empty(), "expected at least one active market");
    println!("discovered {} markets", markets.len());

    // Fetch orderbooks for the first market via REST (no auth needed)
    let market = &markets[0];
    let clob = ClobClient::default();

    let requests = [
        book_request(&market.up_token_id),
        book_request(&market.down_token_id),
    ];

    let books = clob
        .order_books(&requests)
        .await
        .expect("order_books REST call should succeed");

    assert_eq!(books.len(), 2, "expected 2 orderbook responses");

    let up_book = &books[0];
    let down_book = &books[1];

    println!(
        "  {} up book: {} bids, {} asks",
        market.asset,
        up_book.bids.len(),
        up_book.asks.len()
    );
    println!(
        "  {} down book: {} bids, {} asks",
        market.asset,
        down_book.bids.len(),
        down_book.asks.len()
    );

    // Extract best ask from each book
    let up_best_ask = up_book.asks.first().map(|a| a.price);
    let down_best_ask = down_book.asks.first().map(|a| a.price);
    let up_best_bid = up_book.bids.first().map(|b| b.price);
    let down_best_bid = down_book.bids.first().map(|b| b.price);

    println!(
        "  up: bid={:?} ask={:?} | down: bid={:?} ask={:?}",
        up_best_bid, up_best_ask, down_best_bid, down_best_ask
    );

    // Build a snapshot and run arb detection
    let snapshot = MarketSnapshot {
        market: market.clone(),
        up_best_bid,
        up_best_ask,
        down_best_bid,
        down_best_ask,
        timestamp: Utc::now(),
    };

    let strategy_config = StrategyConfig {
        min_arb_profit: dec!(0.001),
        assets: vec!["BTC".into(), "ETH".into(), "SOL".into(), "XRP".into()],
        max_order_size_usdc: dec!(100),
    };

    match detect_arb(&snapshot, &strategy_config) {
        Some(opp) => {
            println!(
                "  ARB FOUND: profit/unit={} expected_pnl=${} ({})",
                opp.profit_per_unit, opp.expected_profit_usdc, opp.asset
            );
            // Validate opportunity fields
            assert!(opp.profit_per_unit > Decimal::ZERO);
            assert!(opp.expected_profit_usdc > Decimal::ZERO);
            assert_eq!(opp.up_token_id, market.up_token_id);
            assert_eq!(opp.down_token_id, market.down_token_id);
        }
        None => {
            println!("  no arb (combined ask after fees >= 1.0) — this is normal");
        }
    }
}

/// Verify the CLOB REST orderbook endpoint returns valid data for a known token.
/// Uses the first discovered market's up token.
#[tokio::test]
#[ignore]
async fn rest_orderbook_returns_valid_data() {
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();
    assert!(!markets.is_empty(), "need at least one BTC market");

    let market = &markets[0];
    let clob = ClobClient::default();

    let request = book_request(&market.up_token_id);
    let book = clob
        .order_book(&request)
        .await
        .expect("order_book REST call should succeed");

    // Basic structural validation
    println!(
        "orderbook: {} bids, {} asks, tick_size={:?}, min_order_size={}, neg_risk={}",
        book.bids.len(),
        book.asks.len(),
        book.tick_size,
        book.min_order_size,
        book.neg_risk,
    );

    // All prices should be in (0, 1) range with positive size
    for level in book.bids.iter().chain(book.asks.iter()) {
        assert!(level.price > Decimal::ZERO, "price should be > 0");
        assert!(level.price < Decimal::ONE, "price should be < 1");
        assert!(level.size > Decimal::ZERO, "size should be > 0");
    }

    // Best bid (max) should be <= best ask (min) — no crossed book
    if let (Some(best_bid), Some(best_ask)) = (
        book.bids.iter().map(|b| b.price).max(),
        book.asks.iter().map(|a| a.price).min(),
    ) {
        assert!(
            best_bid <= best_ask,
            "book should not be crossed: best_bid={} > best_ask={}",
            best_bid,
            best_ask
        );
        println!("  spread: best_bid={} best_ask={}", best_bid, best_ask);
    }
}

/// Verify geoblock check works (no auth needed).
#[tokio::test]
#[ignore]
async fn geoblock_check() {
    let clob = ClobClient::default();
    let response = clob
        .check_geoblock()
        .await
        .expect("geoblock check should succeed");

    println!(
        "geoblock: blocked={}, ip={}, country={}",
        response.blocked, response.ip, response.country
    );

    // We don't assert blocked=false because that depends on the runner's location.
    // Just verify we got a valid response.
    assert!(!response.ip.is_empty(), "should have an IP address");
    assert!(!response.country.is_empty(), "should have a country");
}

/// Multi-market orderbook fetch — batch endpoint.
#[tokio::test]
#[ignore]
async fn batch_orderbook_fetch() {
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into(), "ETH".into(), "SOL".into(), "XRP".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();

    if markets.len() < 2 {
        println!(
            "only {} market(s) found, skipping batch test",
            markets.len()
        );
        return;
    }

    // Build requests for all up+down tokens
    let requests: Vec<OrderBookSummaryRequest> = markets
        .iter()
        .flat_map(|m| vec![book_request(&m.up_token_id), book_request(&m.down_token_id)])
        .collect();

    let clob = ClobClient::default();
    let books = clob
        .order_books(&requests)
        .await
        .expect("batch fetch should succeed");

    assert_eq!(
        books.len(),
        markets.len() * 2,
        "should get one book per token"
    );

    // Print summary
    for (i, market) in markets.iter().enumerate() {
        let up_book = &books[i * 2];
        let down_book = &books[i * 2 + 1];
        let up_ask = up_book.asks.first().map(|a| a.price);
        let down_ask = down_book.asks.first().map(|a| a.price);
        let combined = match (up_ask, down_ask) {
            (Some(u), Some(d)) => format!("{}", u + d),
            _ => "incomplete".into(),
        };
        println!(
            "  {} | up_ask={:?} down_ask={:?} combined={}",
            market.asset, up_ask, down_ask, combined
        );
    }
}

// =============================================================================
// Authenticated tests — require POLYMARKET_PRIVATE_KEY in .env
// =============================================================================

/// Diagnostic: check USDC balance and allowance on the CLOB.
/// Uses GnosisSafe signature type (browser wallet / MetaMask login).
#[tokio::test]
#[ignore]
async fn auth_check_balance_allowance() {
    use polymarket_client_sdk::clob::types::request::BalanceAllowanceRequest;
    use polymarket_client_sdk::clob::types::AssetType;

    let private_key = load_private_key();
    let signer = LocalSigner::from_str(&private_key)
        .expect("valid private key")
        .with_chain_id(Some(POLYGON_CHAIN_ID));

    let client = ClobClient::new(CLOB_URL, Config::default())
        .expect("create CLOB client")
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .expect("auth should succeed");

    let request = BalanceAllowanceRequest::builder()
        .asset_type(AssetType::Collateral)
        .signature_type(SignatureType::GnosisSafe)
        .build();
    let response = client
        .balance_allowance(request)
        .await
        .expect("balance_allowance call should succeed");

    println!("balance: {}", response.balance);
    println!("allowances: {:?}", response.allowances);
}

/// Test 1: Verify CLOB authentication works with our private key.
/// Uses GnosisSafe signature type (MetaMask browser wallet).
#[tokio::test]
#[ignore]
async fn auth_clob_authenticate() {
    let private_key = load_private_key();

    let signer = LocalSigner::from_str(&private_key)
        .expect("valid private key")
        .with_chain_id(Some(POLYGON_CHAIN_ID));

    let client = ClobClient::new(CLOB_URL, Config::default())
        .expect("create CLOB client")
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .expect("EIP-712 authentication should succeed");

    println!("auth: CLOB authentication successful (GnosisSafe)");

    // Use the authenticated client to read an orderbook (proves the client works post-auth)
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();
    assert!(!markets.is_empty());

    let request = book_request(&markets[0].up_token_id);
    let book = client
        .order_book(&request)
        .await
        .expect("authed orderbook call");
    println!(
        "auth: fetched orderbook post-auth — {} bids, {} asks",
        book.bids.len(),
        book.asks.len()
    );
}

/// Test 2: Build and sign an order without posting it.
/// Verifies the full order construction pipeline works with real market data.
#[tokio::test]
#[ignore]
async fn auth_build_and_sign_order() {
    let private_key = load_private_key();

    let signer = LocalSigner::from_str(&private_key)
        .expect("valid private key")
        .with_chain_id(Some(POLYGON_CHAIN_ID));

    let client = ClobClient::new(CLOB_URL, Config::default())
        .expect("create CLOB client")
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .expect("authentication should succeed");

    // Discover a real market for valid token IDs
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();
    assert!(!markets.is_empty());
    let market = &markets[0];

    let token_id = U256::from_str(&market.up_token_id).unwrap();

    // Build a FOK buy order at $0.01 (intentionally low — would never fill)
    let order = client
        .limit_order()
        .token_id(token_id)
        .price(dec!(0.01))
        .size(dec!(100.0)) // 100 shares at $0.01 = $1 notional
        .side(Side::Buy)
        .order_type(OrderType::FOK)
        .build()
        .await
        .expect("order build should succeed");

    println!("build: order built successfully — {:?}", order.order_type);

    // Sign the order (does NOT post it)
    let signed = client
        .sign(&signer, order)
        .await
        .expect("order signing should succeed");

    println!(
        "sign: order signed successfully — signature: {:?}",
        signed.signature
    );
    // If we got here without error, the signing pipeline works
}

/// Test 3: Post a single FOK order at an impossible price ($0.01).
/// The order will be immediately rejected (FOK at $0.01 won't fill).
/// This verifies the full submission pipeline end-to-end. Cost: $0.
#[tokio::test]
#[ignore]
async fn live_post_unfillable_fok_order() {
    let private_key = load_private_key();

    let signer = LocalSigner::from_str(&private_key)
        .expect("valid private key")
        .with_chain_id(Some(POLYGON_CHAIN_ID));

    let client = ClobClient::new(CLOB_URL, Config::default())
        .expect("create CLOB client")
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .expect("authentication should succeed");

    // Discover a real market
    let discovery_config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(discovery_config).unwrap();
    let markets = discovery.discover().await.unwrap();
    assert!(!markets.is_empty());
    let market = &markets[0];

    let token_id = U256::from_str(&market.up_token_id).unwrap();

    // Build a FOK buy at $0.01 — no seller will fill at this price
    let order = client
        .limit_order()
        .token_id(token_id)
        .price(dec!(0.01))
        .size(dec!(100.0))
        .side(Side::Buy)
        .order_type(OrderType::FOK)
        .build()
        .await
        .expect("order build should succeed");

    let signed = client
        .sign(&signer, order)
        .await
        .expect("order signing should succeed");

    // Post the order — FOK at $0.01 will not fill, so no cost.
    // The CLOB returns a 400 for FOK rejections, which the SDK surfaces as an error.
    // This is the expected behavior — the order reached the matching engine and was killed.
    match client.post_order(signed).await {
        Ok(response) => {
            println!(
                "post: order_id={}, success={}, error={:?}",
                response.order_id, response.success, response.error_msg
            );
        }
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("fully filled")
                    || err_msg.contains("FOK")
                    || err_msg.contains("killed"),
                "unexpected error (not a FOK rejection): {err_msg}"
            );
            println!("post: FOK correctly rejected — {err_msg}");
        }
    }

    println!("post: full order submission pipeline verified");
}

// =============================================================================
// Exchange WebSocket tests (Binance, Coinbase, OKX)
// =============================================================================

/// Helper: run a feed gap/price test against any exchange.
/// Subscribes, collects `count` CryptoPrice events, prints gap stats.
async fn run_exchange_gap_test(
    exchange_name: &str,
    expected_symbol: &str,
    tx: tokio::sync::broadcast::Sender<mtb::feed::FeedEvent>,
    count: usize,
    timeout_secs: u64,
) {
    use mtb::feed::FeedEvent;

    let mut rx = tx.subscribe();
    let mut received = 0u32;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let connect_start = std::time::Instant::now();
    let mut last_recv = connect_start;
    let mut gaps_ms = Vec::new();

    loop {
        tokio::select! {
            result = rx.recv() => {
                let now = std::time::Instant::now();
                let gap_ms = now.duration_since(last_recv).as_millis();
                last_recv = now;
                match result {
                    Ok(FeedEvent::CryptoPrice { exchange, symbol, price, .. }) => {
                        if exchange != exchange_name {
                            continue;
                        }
                        received += 1;
                        if received == 1 {
                            println!("{exchange_name}: first msg in {gap_ms}ms (connect + TLS)");
                        } else {
                            gaps_ms.push(gap_ms);
                        }
                        println!("  [{received:>2}] {symbol} = ${price} (gap: {gap_ms}ms)");
                        assert_eq!(symbol, expected_symbol);
                        assert!(price > rust_decimal_macros::dec!(0));
                        if received >= count as u32 {
                            break;
                        }
                    }
                    Ok(_) => {} // disconnect/reconnect events
                    Err(e) => panic!("channel error: {e}"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                panic!("{exchange_name}: timed out after {timeout_secs}s (got {received}/{count})");
            }
        }
    }

    if !gaps_ms.is_empty() {
        let min = gaps_ms.iter().min().unwrap();
        let max = gaps_ms.iter().max().unwrap();
        let avg = gaps_ms.iter().sum::<u128>() / gaps_ms.len() as u128;
        println!("\n{exchange_name}: {received} updates | gap min={min}ms avg={avg}ms max={max}ms");
    }
}

/// Binance: verify direct WS bookTicker delivers BTC prices, measure gaps.
#[tokio::test]
#[ignore]
async fn binance_ws_receives_prices() {
    use tokio::sync::broadcast;

    use mtb::feed::crypto_price::{asset_to_binance_symbol, CryptoPriceFeed};
    use mtb::feed::FeedEvent;

    let (tx, _) = broadcast::channel::<FeedEvent>(256);
    let feed = CryptoPriceFeed::new(tx.clone());
    feed.subscribe(vec![asset_to_binance_symbol("BTC")])
        .unwrap();

    run_exchange_gap_test("binance", "btcusdt", tx, 15, 10).await;
}

/// Coinbase: verify direct WS ticker delivers BTC prices, measure gaps.
#[tokio::test]
#[ignore]
async fn coinbase_ws_receives_prices() {
    use tokio::sync::broadcast;

    use mtb::feed::coinbase::{asset_to_coinbase_product, CoinbaseFeed};
    use mtb::feed::FeedEvent;

    let (tx, _) = broadcast::channel::<FeedEvent>(256);
    let feed = CoinbaseFeed::new(tx.clone());
    feed.subscribe(vec![asset_to_coinbase_product("BTC")])
        .unwrap();

    run_exchange_gap_test("coinbase", "BTC-USD", tx, 15, 15).await;
}

/// OKX: verify direct WS tickers delivers BTC prices, measure gaps.
#[tokio::test]
#[ignore]
async fn okx_ws_receives_prices() {
    use tokio::sync::broadcast;

    use mtb::feed::okx::{asset_to_okx_inst_id, OkxFeed};
    use mtb::feed::FeedEvent;

    let (tx, _) = broadcast::channel::<FeedEvent>(256);
    let feed = OkxFeed::new(tx.clone());
    feed.subscribe(vec![asset_to_okx_inst_id("BTC")]).unwrap();

    run_exchange_gap_test("okx", "BTC-USDT", tx, 15, 15).await;
}

/// Measure real network latency to Binance using the @trade stream.
/// Compares Binance's server-side event time (E) against our local clock.
/// Run locally and on VPS to compare: cargo test binance_network_latency -- --ignored --nocapture
/// NOTE: requires NTP-synced clock for accurate results.
#[tokio::test]
#[ignore]
async fn binance_network_latency() {
    use futures::StreamExt;

    let url = "wss://stream.binance.com:9443/ws/btcusdt@trade";
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (_, mut read) = ws.split();

    let mut latencies_ms = Vec::new();
    let samples = 50;

    println!("binance latency test: collecting {samples} trade messages...\n");

    while latencies_ms.len() < samples {
        if let Some(Ok(msg)) = read.next().await {
            let local_now = chrono::Utc::now().timestamp_millis();
            let text = match msg.into_text() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let parsed: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_time = match parsed.get("E").and_then(|v| v.as_i64()) {
                Some(t) => t,
                None => continue,
            };
            let price = parsed.get("p").and_then(|v| v.as_str()).unwrap_or("?");

            let latency = local_now - event_time;
            latencies_ms.push(latency);

            if latencies_ms.len() <= 5 || latencies_ms.len() == samples {
                println!(
                    "  [{:>2}] price=${price} binance_time={event_time} local_now={local_now} latency={latency}ms",
                    latencies_ms.len()
                );
            } else if latencies_ms.len() == 6 {
                println!("  ...");
            }
        }
    }

    print_latency_stats("Binance", &mut latencies_ms);
}

/// Measure real network latency to OKX using the tickers channel.
/// OKX provides server timestamp (ts) in the ticker data.
/// Run: cargo test okx_network_latency -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn okx_network_latency() {
    use futures::{SinkExt, StreamExt};

    let url = "wss://ws.okx.com:8443/ws/v5/public";
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (mut write, mut read) = ws.split();

    // Subscribe to BTC-USDT tickers
    let sub = r#"{"op":"subscribe","args":[{"channel":"tickers","instId":"BTC-USDT"}]}"#;
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(sub.into()))
        .await
        .unwrap();

    let mut latencies_ms = Vec::new();
    let samples = 50;

    println!("okx latency test: collecting {samples} ticker messages...\n");

    while latencies_ms.len() < samples {
        if let Some(Ok(msg)) = read.next().await {
            let local_now = chrono::Utc::now().timestamp_millis();
            let text = match msg.into_text() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let parsed: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // OKX ticker: {"arg":{"channel":"tickers",...},"data":[{"ts":"1700000000000",...}]}
            let ts = parsed
                .get("data")
                .and_then(|d| d.get(0))
                .and_then(|d| d.get("ts"))
                .and_then(|t| t.as_str())
                .and_then(|s| s.parse::<i64>().ok());

            let Some(event_time) = ts else {
                continue;
            };

            let price = parsed
                .get("data")
                .and_then(|d| d.get(0))
                .and_then(|d| d.get("bidPx"))
                .and_then(|p| p.as_str())
                .unwrap_or("?");

            let latency = local_now - event_time;
            latencies_ms.push(latency);

            if latencies_ms.len() <= 5 || latencies_ms.len() == samples {
                println!(
                    "  [{:>2}] price=${price} okx_time={event_time} local_now={local_now} latency={latency}ms",
                    latencies_ms.len()
                );
            } else if latencies_ms.len() == 6 {
                println!("  ...");
            }
        }
    }

    print_latency_stats("OKX", &mut latencies_ms);
}

/// Measure real network latency to Coinbase using the ticker channel.
/// Coinbase provides server timestamp in the message-level `timestamp` field (ISO 8601).
/// Run: cargo test coinbase_network_latency -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn coinbase_network_latency() {
    use futures::{SinkExt, StreamExt};

    let url = "wss://advanced-trade-ws.coinbase.com";
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (mut write, mut read) = ws.split();

    // Subscribe to BTC-USD ticker
    let sub = r#"{"type":"subscribe","product_ids":["BTC-USD"],"channel":"ticker"}"#;
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(sub.into()))
        .await
        .unwrap();

    let mut latencies_ms = Vec::new();
    let samples = 50;

    println!("coinbase latency test: collecting {samples} ticker messages...\n");

    while latencies_ms.len() < samples {
        if let Some(Ok(msg)) = read.next().await {
            let local_now = chrono::Utc::now().timestamp_millis();
            let text = match msg.into_text() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let parsed: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Coinbase ticker: {"channel":"ticker","timestamp":"2023-02-09T20:30:37.167Z",...}
            let channel = parsed.get("channel").and_then(|c| c.as_str());
            if channel != Some("ticker") {
                continue;
            }

            let ts_str = match parsed.get("timestamp").and_then(|t| t.as_str()) {
                Some(s) => s,
                None => continue,
            };

            // Parse ISO 8601 timestamp to epoch millis
            let event_time = match chrono::DateTime::parse_from_rfc3339(ts_str) {
                Ok(dt) => dt.timestamp_millis(),
                Err(_) => {
                    // Try alternate format without timezone offset
                    match chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%dT%H:%M:%S%.fZ") {
                        Ok(ndt) => ndt.and_utc().timestamp_millis(),
                        Err(_) => continue,
                    }
                }
            };

            // Get price from nested events
            let price = parsed
                .get("events")
                .and_then(|e| e.get(0))
                .and_then(|e| e.get("tickers"))
                .and_then(|t| t.get(0))
                .and_then(|t| t.get("price"))
                .and_then(|p| p.as_str())
                .unwrap_or("?");

            let latency = local_now - event_time;
            latencies_ms.push(latency);

            if latencies_ms.len() <= 5 || latencies_ms.len() == samples {
                println!(
                    "  [{:>2}] price=${price} coinbase_time={event_time} local_now={local_now} latency={latency}ms",
                    latencies_ms.len()
                );
            } else if latencies_ms.len() == 6 {
                println!("  ...");
            }
        }
    }

    print_latency_stats("Coinbase", &mut latencies_ms);
}

/// Chainlink RTDS: verify we receive resolution prices.
/// Uses the SDK's RTDS client to subscribe to Chainlink Data Streams prices.
#[tokio::test]
#[ignore]
async fn chainlink_rtds_receives_prices() {
    use futures::StreamExt;
    use polymarket_client_sdk::rtds::Client as RtdsClient;

    println!("connecting to Polymarket RTDS for Chainlink prices...");

    let client = RtdsClient::default();

    let stream = client
        .subscribe_chainlink_prices(None)
        .expect("failed to subscribe to chainlink prices");

    let mut stream = Box::pin(stream);
    let mut count = 0;
    let samples = 15;
    let timeout_duration = std::time::Duration::from_secs(30);
    let mut prev_time = std::time::Instant::now();
    let start = std::time::Instant::now();

    println!("waiting for Chainlink price updates...\n");

    while count < samples {
        match tokio::time::timeout(timeout_duration, stream.next()).await {
            Ok(Some(Ok(price))) => {
                let now = std::time::Instant::now();
                let gap_ms = now.duration_since(prev_time).as_millis();
                prev_time = now;

                let local_now = chrono::Utc::now().timestamp_millis();
                let latency = local_now - price.timestamp;

                count += 1;
                println!(
                    "  [{:>2}] {} = ${} (gap: {}ms, latency: {}ms, ts: {})",
                    count, price.symbol, price.value, gap_ms, latency, price.timestamp
                );
            }
            Ok(Some(Err(e))) => {
                println!("  error: {e}");
            }
            Ok(None) => {
                println!("  stream ended");
                break;
            }
            Err(_) => {
                println!("  timeout after {}s", timeout_duration.as_secs());
                break;
            }
        }
    }

    let elapsed = start.elapsed();
    println!(
        "\n--- Chainlink RTDS ({count} prices in {:.1}s) ---",
        elapsed.as_secs_f64()
    );

    // Chainlink updates less frequently than exchange feeds (deviation-based + heartbeat)
    // We should get at least a few prices within 30s
    assert!(
        count >= 3,
        "expected at least 3 Chainlink prices, got {count}"
    );
}

/// Print latency percentile stats.
fn print_latency_stats(name: &str, latencies_ms: &mut Vec<i64>) {
    latencies_ms.sort();
    let samples = latencies_ms.len();
    let min = latencies_ms.first().unwrap();
    let max = latencies_ms.last().unwrap();
    let avg = latencies_ms.iter().sum::<i64>() / samples as i64;
    let p50 = latencies_ms[samples / 2];
    let p95 = latencies_ms[(samples as f64 * 0.95) as usize];
    let p99 = latencies_ms[(samples as f64 * 0.99) as usize];

    println!("\n--- {name} Network Latency ({samples} samples) ---");
    println!("  min:  {min}ms");
    println!("  avg:  {avg}ms");
    println!("  p50:  {p50}ms");
    println!("  p95:  {p95}ms");
    println!("  p99:  {p99}ms");
    println!("  max:  {max}ms");
    println!(
        "\nNOTE: accuracy depends on NTP clock sync. Negative values = clock ahead of exchange."
    );
}
