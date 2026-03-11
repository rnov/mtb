//! MTB Execution Path Benchmark
//!
//! Runs two patterns back-to-back and compares them:
//!   OLD — re-auth every execution, sequential build, sequential sign
//!   NEW — auth once at startup, concurrent build, concurrent sign
//!
//! Uses guaranteed-rejected sub-$1 orders — costs nothing, no real trades placed.
//!
//! Usage:
//!   POLYMARKET_PRIVATE_KEY=xxx cargo run --bin exec_bench

use std::str::FromStr;
use std::time::Instant;

use chrono::Utc;
use rust_decimal::Decimal;

use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer as _;
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::types::U256;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const SAMPLES: usize = 5;
const CLOB_URL: &str = "https://clob.polymarket.com";

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

struct Stats {
    values: Vec<u128>,
}

impl Stats {
    fn new() -> Self { Self { values: Vec::new() } }
    fn add(&mut self, ms: u128) { self.values.push(ms); }

    fn percentile(&self, p: f64) -> Option<u128> {
        if self.values.is_empty() { return None; }
        let mut sorted = self.values.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * p / 100.0).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1);
        Some(sorted[idx])
    }

    fn min(&self) -> Option<u128> { self.values.iter().min().copied() }
    fn max(&self) -> Option<u128> { self.values.iter().max().copied() }
}

fn fmt(v: Option<u128>) -> String {
    v.map(|ms| format!("{}ms", ms)).unwrap_or_else(|| "-".into())
}

fn fmt_delta(new: Option<u128>, old: Option<u128>) -> String {
    match (new, old) {
        (Some(n), Some(o)) if o > 0 => {
            let saved = o as i64 - n as i64;
            if saved > 0 {
                format!("-{}ms ({:.0}% faster)", saved, saved as f64 / o as f64 * 100.0)
            } else {
                format!("+{}ms", -saved)
            }
        }
        _ => "-".into(),
    }
}

// ---------------------------------------------------------------------------
// Market discovery
// ---------------------------------------------------------------------------

struct TokenPair {
    up_token_id: String,
    down_token_id: String,
}

async fn discover_btc_tokens() -> Option<TokenPair> {
    use mtb::config::types::DiscoveryConfig;
    use mtb::market::MarketDiscovery;

    let config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(config).ok()?;
    let markets = discovery.discover().await.ok()?;
    let market = markets.into_iter().next()?;
    Some(TokenPair {
        up_token_id: market.up_token_id,
        down_token_id: market.down_token_id,
    })
}

// ---------------------------------------------------------------------------
// Sample struct
// ---------------------------------------------------------------------------

struct Sample {
    auth_ms:  u128,
    build_ms: u128,
    sign_ms:  u128,
    post_ms:  u128,
    total_ms: u128,
}

struct BenchStats {
    auth:  Stats,
    build: Stats,
    sign:  Stats,
    post:  Stats,
    total: Stats,
}

impl BenchStats {
    fn new() -> Self {
        Self {
            auth:  Stats::new(),
            build: Stats::new(),
            sign:  Stats::new(),
            post:  Stats::new(),
            total: Stats::new(),
        }
    }
    fn add(&mut self, s: &Sample) {
        self.auth.add(s.auth_ms);
        self.build.add(s.build_ms);
        self.sign.add(s.sign_ms);
        self.post.add(s.post_ms);
        self.total.add(s.total_ms);
    }
}

// ---------------------------------------------------------------------------
// OLD pattern — re-auth every execution, sequential build + sign
// ---------------------------------------------------------------------------

async fn run_old(
    private_key: &str,
    up_token_id: &str,
    down_token_id: &str,
) -> anyhow::Result<Sample> {
    let key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer = PrivateKeySigner::from_str(key)?.with_chain_id(Some(137u64));

    let total_start = Instant::now();

    // Auth on every call
    let t = Instant::now();
    let client = Client::new(CLOB_URL, Config::default())?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await?;
    let auth_ms = t.elapsed().as_millis();

    let up_token   = U256::from_str(up_token_id)?;
    let down_token = U256::from_str(down_token_id)?;

    // Build sequentially
    let t = Instant::now();
    let up_order = client
        .limit_order().token_id(up_token)
        .price(Decimal::new(1, 2)).size(Decimal::ONE)
        .side(Side::Buy).order_type(OrderType::FOK)
        .build().await?;
    let down_order = client
        .limit_order().token_id(down_token)
        .price(Decimal::new(1, 2)).size(Decimal::ONE)
        .side(Side::Buy).order_type(OrderType::FOK)
        .build().await?;
    let build_ms = t.elapsed().as_millis();

    // Sign sequentially
    let t = Instant::now();
    let signed_up   = client.sign(&signer, up_order).await?;
    let signed_down = client.sign(&signer, down_order).await?;
    let sign_ms = t.elapsed().as_millis();

    // Post concurrently
    let t = Instant::now();
    let _ = tokio::join!(
        client.post_order(signed_up),
        client.post_order(signed_down),
    );
    let post_ms = t.elapsed().as_millis();

    Ok(Sample { auth_ms, build_ms, sign_ms, post_ms, total_ms: total_start.elapsed().as_millis() })
}

// ---------------------------------------------------------------------------
// NEW pattern — auth once, concurrent build + sign
// ---------------------------------------------------------------------------

async fn run_new(
    client: &polymarket_client_sdk::clob::Client<
        polymarket_client_sdk::auth::state::Authenticated<polymarket_client_sdk::auth::Normal>
    >,
    signer: &PrivateKeySigner,
    up_token_id: &str,
    down_token_id: &str,
) -> anyhow::Result<Sample> {
    let total_start = Instant::now();
    let auth_ms = 0; // auth already done at startup

    let up_token   = U256::from_str(up_token_id)?;
    let down_token = U256::from_str(down_token_id)?;

    // Build concurrently
    let t = Instant::now();
    let (up_res, down_res) = tokio::join!(
        client.limit_order().token_id(up_token)
            .price(Decimal::new(1, 2)).size(Decimal::ONE)
            .side(Side::Buy).order_type(OrderType::FOK).build(),
        client.limit_order().token_id(down_token)
            .price(Decimal::new(1, 2)).size(Decimal::ONE)
            .side(Side::Buy).order_type(OrderType::FOK).build(),
    );
    let up_order   = up_res?;
    let down_order = down_res?;
    let build_ms = t.elapsed().as_millis();

    // Sign concurrently
    let t = Instant::now();
    let (sup_res, sdown_res) = tokio::join!(
        client.sign(signer, up_order),
        client.sign(signer, down_order),
    );
    let signed_up   = sup_res?;
    let signed_down = sdown_res?;
    let sign_ms = t.elapsed().as_millis();

    // Post concurrently
    let t = Instant::now();
    let _ = tokio::join!(
        client.post_order(signed_up),
        client.post_order(signed_down),
    );
    let post_ms = t.elapsed().as_millis();

    Ok(Sample { auth_ms, build_ms, sign_ms, post_ms, total_ms: total_start.elapsed().as_millis() })
}

// ---------------------------------------------------------------------------
// Print summary table
// ---------------------------------------------------------------------------

fn print_table(label: &str, s: &BenchStats) {
    let w = 70;
    println!("  ── {} ──", label);
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "Step", "p50", "p95", "min", "max");
    println!("  {}", "-".repeat(w - 2));
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "auth",
        fmt(s.auth.percentile(50.0)),  fmt(s.auth.percentile(95.0)),  fmt(s.auth.min()),  fmt(s.auth.max()));
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "build",
        fmt(s.build.percentile(50.0)), fmt(s.build.percentile(95.0)), fmt(s.build.min()), fmt(s.build.max()));
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "sign",
        fmt(s.sign.percentile(50.0)),  fmt(s.sign.percentile(95.0)),  fmt(s.sign.min()),  fmt(s.sign.max()));
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "post",
        fmt(s.post.percentile(50.0)),  fmt(s.post.percentile(95.0)),  fmt(s.post.min()),  fmt(s.post.max()));
    println!("  {}", "-".repeat(w - 2));
    println!("  {:<12} {:>8} {:>8} {:>8} {:>8}", "TOTAL",
        fmt(s.total.percentile(50.0)), fmt(s.total.percentile(95.0)), fmt(s.total.min()), fmt(s.total.max()));
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
        .expect("POLYMARKET_PRIVATE_KEY not set");

    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           MTB Execution Path Benchmark                      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!("  timestamp : {}", Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    println!("  samples   : {} per pattern", SAMPLES);
    println!("  note      : sub-$1 orders — rejected before matching engine, no cost");
    println!();

    // Discover tokens
    print!("  Discovering BTC token pair ... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let tokens = discover_btc_tokens().await
        .expect("Failed to discover BTC market tokens");
    println!("found");
    println!();

    // ── OLD pattern ──────────────────────────────────────────────────────────
    println!("  [OLD] re-auth every call, sequential build + sign");
    println!("  {:>4}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "#", "auth", "build", "sign", "post", "total");
    println!("  {}", "-".repeat(52));

    let mut old = BenchStats::new();
    for i in 1..=SAMPLES {
        match run_old(&private_key, &tokens.up_token_id, &tokens.down_token_id).await {
            Ok(s) => {
                println!("  {:>4}  {:>7}ms  {:>7}ms  {:>7}ms  {:>7}ms  {:>7}ms",
                    i, s.auth_ms, s.build_ms, s.sign_ms, s.post_ms, s.total_ms);
                old.add(&s);
            }
            Err(e) => println!("  {:>4}  ERROR: {}", i, e),
        }
    }

    // ── NEW pattern ──────────────────────────────────────────────────────────
    println!();
    println!("  [NEW] auth once at startup, concurrent build + sign");

    let key = private_key.strip_prefix("0x").unwrap_or(&private_key);
    let signer = PrivateKeySigner::from_str(key)?.with_chain_id(Some(137u64));
    let t = Instant::now();
    let client = Client::new(CLOB_URL, Config::default())?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await?;
    println!("  startup auth: {}ms (one-time)", t.elapsed().as_millis());
    println!();

    println!("  {:>4}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "#", "auth", "build", "sign", "post", "total");
    println!("  {}", "-".repeat(52));

    let mut new = BenchStats::new();
    for i in 1..=SAMPLES {
        match run_new(&client, &signer, &tokens.up_token_id, &tokens.down_token_id).await {
            Ok(s) => {
                println!("  {:>4}  {:>7}ms  {:>7}ms  {:>7}ms  {:>7}ms  {:>7}ms",
                    i, s.auth_ms, s.build_ms, s.sign_ms, s.post_ms, s.total_ms);
                new.add(&s);
            }
            Err(e) => println!("  {:>4}  ERROR: {}", i, e),
        }
    }

    // ── Summary ──────────────────────────────────────────────────────────────
    let w = 70;
    println!();
    println!("{}", "━".repeat(w));
    print_table("OLD — re-auth + sequential", &old);
    println!();
    print_table("NEW — startup auth + concurrent", &new);
    println!();
    println!("  ── Improvement (p50) ──");
    println!("  {:<12} {}", "auth",  fmt_delta(new.auth.percentile(50.0),  old.auth.percentile(50.0)));
    println!("  {:<12} {}", "build", fmt_delta(new.build.percentile(50.0), old.build.percentile(50.0)));
    println!("  {:<12} {}", "sign",  fmt_delta(new.sign.percentile(50.0),  old.sign.percentile(50.0)));
    println!("  {:<12} {}", "post",  fmt_delta(new.post.percentile(50.0),  old.post.percentile(50.0)));
    println!("  {}", "-".repeat(w - 2));
    println!("  {:<12} {}", "TOTAL", fmt_delta(new.total.percentile(50.0), old.total.percentile(50.0)));
    println!("{}", "━".repeat(w));
    println!();

    Ok(())
}
