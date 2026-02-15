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
    OrderBookSummaryRequest::builder().token_id(token_id).build()
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
        println!("only {} market(s) found, skipping batch test", markets.len());
        return;
    }

    // Build requests for all up+down tokens
    let requests: Vec<OrderBookSummaryRequest> = markets
        .iter()
        .flat_map(|m| {
            vec![
                book_request(&m.up_token_id),
                book_request(&m.down_token_id),
            ]
        })
        .collect();

    let clob = ClobClient::default();
    let books = clob.order_books(&requests).await.expect("batch fetch should succeed");

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
    let book = client.order_book(&request).await.expect("authed orderbook call");
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

    println!("sign: order signed successfully — signature: {:?}", signed.signature);
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
