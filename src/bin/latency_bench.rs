//! MTB Latency Benchmark
//!
//! Measures end-to-end latency for every data source the bot depends on.
//! Run on local and VPS to compare, then pick deployment target.
//!
//! Usage:
//!   cargo run --bin latency_bench
//!   cargo run --bin latency_bench -- --samples 30   # override WS sample count
//!
//! Output: per-source stats + a summary table you can copy-paste for comparison.

use std::str::FromStr;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::{connect_async, tungstenite::Message};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const REST_SAMPLES: usize = 20;
const WS_SAMPLES: usize = 50;        // exchange feeds tick many times/second — 50 arrives fast
const CLOB_WS_SAMPLES: usize = 5;    // CLOB WS sends snapshot on connect + updates per order event;
                                     // update rate varies (seconds to tens of seconds), so keep small
const WS_TIMEOUT: Duration = Duration::from_secs(30);      // per-message timeout for exchange feeds
const CLOB_WS_TIMEOUT: Duration = Duration::from_secs(60); // per-message timeout for CLOB WS
const CLOB_WS_TOTAL: Duration = Duration::from_secs(90);   // overall wall-clock cap for CLOB WS bench
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// Well-known Polymarket BTC Up/Down token IDs (15-min markets are ephemeral;
// we discover live ones at startup, but keep these as fallback).
// Actual token IDs are discovered dynamically in main().
const CLOB_URL: &str = "https://clob.polymarket.com";

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

struct Stats {
    values: Vec<u128>, // milliseconds
}

impl Stats {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn add(&mut self, ms: u128) {
        self.values.push(ms);
    }

    fn count(&self) -> usize {
        self.values.len()
    }

    fn percentile(&self, p: f64) -> Option<u128> {
        if self.values.is_empty() {
            return None;
        }
        let mut sorted = self.values.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * p / 100.0).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1);
        Some(sorted[idx])
    }

    fn min(&self) -> Option<u128> {
        self.values.iter().min().copied()
    }
    fn max(&self) -> Option<u128> {
        self.values.iter().max().copied()
    }
}

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

struct BenchResult {
    label: String,
    /// For REST: None. For WS: time from call to first byte of WS upgrade.
    connect_ms: Option<u128>,
    /// For REST: p50 round-trip. For WS: time from connect_start to first data msg.
    first_ms: Option<u128>,
    /// Inter-message gaps (WS) or individual RTTs (REST).
    samples: Stats,
    /// Server timestamp minus local clock at receipt. Positive = message is "old".
    /// Only available for exchanges that embed a server-side timestamp.
    clock_delta_ms: Option<f64>,
    error: Option<String>,
}

impl BenchResult {
    fn failed(label: &str, err: String) -> Self {
        Self {
            label: label.to_string(),
            connect_ms: None,
            first_ms: None,
            samples: Stats::new(),
            clock_delta_ms: None,
            error: Some(err),
        }
    }
}

// ---------------------------------------------------------------------------
// CLOB REST benchmark
// ---------------------------------------------------------------------------

async fn bench_clob_rest(up_token_id: &str) -> BenchResult {
    use polymarket_client_sdk::clob::types::request::OrderBookSummaryRequest;
    use polymarket_client_sdk::clob::Client as ClobClient;
    use polymarket_client_sdk::types::U256;

    let label = "CLOB REST  (order_books)";

    let token = match U256::from_str(up_token_id) {
        Ok(t) => t,
        Err(e) => return BenchResult::failed(label, format!("invalid token ID: {e}")),
    };
    let clob = ClobClient::default();
    let mut rtts = Stats::new();

    print!("  {} — {} calls ... ", label, REST_SAMPLES);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    for _ in 0..REST_SAMPLES {
        // Rebuild each time — OrderBookSummaryRequest is not Clone
        let req = OrderBookSummaryRequest::builder().token_id(token).build();
        let t = Instant::now();
        match clob.order_books(&[req]).await {
            Ok(_) => rtts.add(t.elapsed().as_millis()),
            Err(e) => eprintln!("[warn: {e}]"),
        }
    }
    println!("done ({} ok)", rtts.count());

    let p50 = rtts.percentile(50.0);
    BenchResult {
        label: label.to_string(),
        connect_ms: None,
        first_ms: p50,
        samples: rtts,
        clock_delta_ms: None,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// CLOB WebSocket benchmark
// ---------------------------------------------------------------------------

async fn bench_clob_ws(up_token_id: &str) -> BenchResult {
    use polymarket_client_sdk::clob::ws::Client as WsClient;
    use polymarket_client_sdk::types::U256;

    let label = "CLOB WS    (orderbook)";

    let token = match U256::from_str(up_token_id) {
        Ok(t) => t,
        Err(e) => return BenchResult::failed(label, format!("invalid token ID: {e}")),
    };

    print!(
        "  {} — up to {} messages ({}s cap) ... ",
        label,
        CLOB_WS_SAMPLES,
        CLOB_WS_TOTAL.as_secs()
    );
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let connect_start = Instant::now();
    let client = WsClient::default();
    let stream = match client.subscribe_orderbook(vec![token]) {
        Ok(s) => s,
        Err(e) => return BenchResult::failed(label, format!("subscribe: {e}")),
    };
    let mut stream = Box::pin(stream);
    let connect_ms = connect_start.elapsed().as_millis();

    let mut first_ms: Option<u128> = None;
    let mut gaps = Stats::new();
    let mut last_msg = Instant::now();
    let mut count = 0;
    let wall_deadline = tokio::time::Instant::now() + CLOB_WS_TOTAL;

    loop {
        // Honour both the per-message timeout and the overall wall-clock cap.
        let per_msg = tokio::time::timeout(CLOB_WS_TIMEOUT, stream.next());
        let result = tokio::time::timeout_at(wall_deadline, per_msg).await;

        match result {
            // Wall-clock cap expired — report partial results, not a failure.
            Err(_wall) => {
                println!("done ({count} msgs, wall-clock cap reached)");
                break;
            }
            Ok(Err(_per_msg_timeout)) => {
                // No message for CLOB_WS_TIMEOUT — treat as done (market quiet).
                println!("done ({count} msgs, no update for {}s)", CLOB_WS_TIMEOUT.as_secs());
                break;
            }
            Ok(Ok(Some(Ok(_)))) => {
                let now = Instant::now();
                if first_ms.is_none() {
                    first_ms = Some(connect_start.elapsed().as_millis());
                } else {
                    gaps.add(last_msg.elapsed().as_millis());
                }
                last_msg = now;
                count += 1;
                if count >= CLOB_WS_SAMPLES {
                    println!("done ({count} msgs)");
                    break;
                }
            }
            Ok(Ok(Some(Err(e)))) => {
                return BenchResult::failed(label, format!("stream error: {e}"));
            }
            Ok(Ok(None)) => {
                println!("done ({count} msgs, stream closed)");
                break;
            }
        }
    }

    BenchResult {
        label: label.to_string(),
        connect_ms: Some(connect_ms),
        first_ms,
        samples: gaps,
        clock_delta_ms: None,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Generic exchange WebSocket benchmark
// ---------------------------------------------------------------------------

/// Run a WebSocket benchmark against an exchange feed.
///
/// `is_data`  — returns true if this JSON message is a real data push (not a
///              subscription confirmation, ping, or unrecognised frame). Messages
///              where this returns false are silently skipped.
///
/// `server_ts` — returns the server-side timestamp (ms) embedded in the message,
///               or None if the exchange does not include one. None does NOT skip
///               the message; it just means no clock-delta is reported for it.
async fn bench_exchange_ws(
    label: &'static str,
    url: &str,
    subscribe_msg: Option<String>,
    is_data: fn(&Value) -> bool,
    server_ts: fn(&Value) -> Option<i64>,
) -> BenchResult {
    print!("  {} — {} messages ... ", label, WS_SAMPLES);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let connect_start = Instant::now();
    let ws = match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url)).await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return BenchResult::failed(label, format!("connect: {e}")),
        Err(_) => return BenchResult::failed(label, "connect timeout".into()),
    };
    let connect_ms = connect_start.elapsed().as_millis();

    let (mut write, mut read) = ws.split();

    if let Some(msg) = subscribe_msg {
        if write.send(Message::Text(msg.into())).await.is_err() {
            return BenchResult::failed(label, "subscribe failed".into());
        }
    }

    let mut first_ms: Option<u128> = None;
    let mut gaps = Stats::new();
    let mut clock_deltas: Vec<f64> = Vec::new();
    let mut last_msg = Instant::now();
    let mut count = 0;

    loop {
        match tokio::time::timeout(WS_TIMEOUT, read.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let local_ms = Utc::now().timestamp_millis();
                let now = Instant::now();

                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Filter: skip subscription confirmations and unrecognised frames.
                if !is_data(&v) {
                    continue;
                }

                // Clock delta: only if the exchange embeds a server timestamp.
                if let Some(ts) = server_ts(&v) {
                    clock_deltas.push((local_ms - ts) as f64);
                }

                if first_ms.is_none() {
                    first_ms = Some(connect_start.elapsed().as_millis());
                } else {
                    gaps.add(last_msg.elapsed().as_millis());
                }
                last_msg = now;
                count += 1;
                if count >= WS_SAMPLES {
                    break;
                }
            }
            Ok(Some(Ok(_))) => {} // ping/pong/binary — ignore
            Ok(Some(Err(e))) => {
                return BenchResult::failed(label, format!("read error: {e}"));
            }
            Ok(None) => break,
            Err(_) => return BenchResult::failed(label, "message timeout".into()),
        }
    }

    println!("done ({count} msgs)");

    let clock_delta_ms = if clock_deltas.is_empty() {
        None
    } else {
        Some(clock_deltas.iter().sum::<f64>() / clock_deltas.len() as f64)
    };

    BenchResult {
        label: label.to_string(),
        connect_ms: Some(connect_ms),
        first_ms,
        samples: gaps,
        clock_delta_ms,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Per-exchange message filters and timestamp extractors
// ---------------------------------------------------------------------------

// --- Binance bookTicker (combined stream) ---
// Message: {"stream":"btcusdt@bookTicker","data":{"u":…,"s":"BTCUSDT","b":"…","a":"…"}}
// The bookTicker stream has NO event-time field (E only exists in trade/kline streams).
// is_data: check "stream" key present and "data.s" (symbol) exists.
// server_ts: None — no server timestamp available in bookTicker.
fn binance_is_data(v: &Value) -> bool {
    v.get("stream").is_some() && v.get("data").and_then(|d| d.get("s")).is_some()
}
fn binance_server_ts(_: &Value) -> Option<i64> {
    None
}

// --- Coinbase Advanced Trade ticker ---
// Message: {"channel":"ticker","timestamp":"2024-01-01T00:00:00.000Z","events":[…]}
// is_data: channel must be "ticker".
// server_ts: top-level "timestamp" field (ISO 8601). More reliable than events[].tickers[].time.
fn coinbase_is_data(v: &Value) -> bool {
    v.get("channel").and_then(|c| c.as_str()) == Some("ticker")
}
fn coinbase_server_ts(v: &Value) -> Option<i64> {
    let ts_str = v.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(ts_str)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

// --- OKX tickers ---
// Message: {"arg":{"channel":"tickers",…},"data":[{"ts":"1234567890000",…}]}
// is_data: "data" array is non-empty.
// server_ts: data[0].ts (string ms).
fn okx_is_data(v: &Value) -> bool {
    v.get("data")
        .and_then(|d| d.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}
fn okx_server_ts(v: &Value) -> Option<i64> {
    let ts_str = v.get("data")?.as_array()?.first()?.get("ts")?.as_str()?;
    ts_str.parse::<i64>().ok()
}

// --- Bybit tickers ---
// Message: {"topic":"tickers.BTCUSDT","ts":1234567890000,"type":"snapshot","data":{…}}
// is_data: "topic" key present (subscription confirmations have "op"/"ret_msg" instead).
// server_ts: top-level "ts" (integer ms).
fn bybit_is_data(v: &Value) -> bool {
    v.get("topic").is_some()
}
fn bybit_server_ts(v: &Value) -> Option<i64> {
    v.get("ts")?.as_i64()
}

// ---------------------------------------------------------------------------
// Market discovery: get a live BTC token ID for CLOB benchmarks
// ---------------------------------------------------------------------------

async fn discover_btc_token() -> Option<String> {
    use mtb::config::types::DiscoveryConfig;
    use mtb::market::MarketDiscovery;

    let config = DiscoveryConfig {
        poll_interval_secs: 60,
        assets: vec!["BTC".into()],
    };
    let discovery = MarketDiscovery::new(config).ok()?;
    let markets = discovery.discover().await.ok()?;
    markets.into_iter().next().map(|m| m.up_token_id)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_summary(results: &[BenchResult]) {
    let w = 96;
    println!();
    println!("{}", "━".repeat(w));
    println!("  {:<28} {:>8} {:>10} {:>8} {:>8} {:>6} {:>6} {:>5} {:>11}",
        "Source", "connect", "first-msg", "p50-gap", "p95-gap", "min", "max", "n", "clock-delta");
    println!("  {}", "-".repeat(w - 2));

    for r in results {
        if let Some(ref e) = r.error {
            println!("  {:<28} FAILED: {}", r.label, e);
            continue;
        }

        // CLOB WS uses a lazy-connect SDK: subscribe_orderbook() is sync and returns
        // immediately without opening a socket. The real WS handshake happens on the
        // first .next() poll, so connect_ms is always 0 and is not meaningful here.
        let connect = match (r.connect_ms, &r.label) {
            (Some(0), label) if label.contains("CLOB WS") => "lazy".to_string(),
            (v, _) => fmt_ms(v),
        };

        let first = fmt_ms(r.first_ms);
        let p50   = fmt_ms(r.samples.percentile(50.0));
        let p95   = fmt_ms(r.samples.percentile(95.0));
        let n     = r.samples.count();
        let min   = fmt_ms(r.samples.min());
        let max   = fmt_ms(r.samples.max());
        // "-" for clock-delta means no server timestamp available (e.g. Binance bookTicker).
        let delta = r.clock_delta_ms
            .map(|d| format!("{:+.0}ms", d))
            .unwrap_or_else(|| "-".into());

        println!("  {:<28} {:>8} {:>10} {:>8} {:>8} {:>6} {:>6} {:>5} {:>11}",
            r.label, connect, first, p50, p95, min, max, n, delta);
    }

    println!("{}", "━".repeat(w));
    println!();
    println!("  connect     = WS handshake time. 'lazy' = SDK connects on first message (time baked into first-msg).");
    println!("  first-msg   = REST: p50 round-trip  |  WS: call → first data message received");
    println!("  p50/p95-gap = inter-message gap percentiles (WS only, n = number of gap samples)");
    println!("  clock-delta = local_recv_time − server_timestamp (avg). '+' = latency+skew, '-' = clock skew only.");
    println!("              = '-' for Binance: bookTicker has no server timestamp (E field is trade-stream only).");
    println!();
    println!("  Run on VPS and compare: lower connect/first-msg = closer to CLOB/exchange servers.");
}

fn fmt_ms(v: Option<u128>) -> String {
    v.map(|ms| format!("{}ms", ms)).unwrap_or_else(|| "-".into())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              MTB Latency Benchmark                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!("  timestamp  : {}", Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    println!("  clob_url   : {}", CLOB_URL);
    println!(
        "  rest_n: {}   exchange_ws_n: {}   clob_ws_n: {} (cap: {}s)",
        REST_SAMPLES,
        WS_SAMPLES,
        CLOB_WS_SAMPLES,
        CLOB_WS_TOTAL.as_secs()
    );
    println!();

    // Discover a live BTC token ID for CLOB tests
    print!("  Discovering BTC market token IDs ... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let btc_token = discover_btc_token().await;
    match &btc_token {
        Some(id) => println!("found ({}...{})", &id[..8.min(id.len())], &id[id.len().saturating_sub(6)..]),
        None     => println!("FAILED — CLOB tests will be skipped"),
    }
    println!();

    println!("  Running benchmarks:");

    let mut results: Vec<BenchResult> = Vec::new();

    // CLOB REST
    if let Some(ref token) = btc_token {
        results.push(bench_clob_rest(token).await);
    } else {
        results.push(BenchResult::failed("CLOB REST  (order_books)", "no token ID".into()));
    }

    // CLOB WebSocket
    if let Some(ref token) = btc_token {
        results.push(bench_clob_ws(token).await);
    } else {
        results.push(BenchResult::failed("CLOB WS    (orderbook)", "no token ID".into()));
    }

    // Binance — combined stream URL embeds subscription; bookTicker has no server timestamp
    results.push(
        bench_exchange_ws(
            "Binance WS (bookTicker)",
            "wss://stream.binance.com:9443/stream?streams=btcusdt@bookTicker",
            None,
            binance_is_data,
            binance_server_ts,
        )
        .await,
    );

    // Coinbase
    let coinbase_sub = serde_json::json!({
        "type": "subscribe",
        "product_ids": ["BTC-USD"],
        "channel": "ticker"
    })
    .to_string();
    results.push(
        bench_exchange_ws(
            "Coinbase WS (ticker)",
            "wss://advanced-trade-ws.coinbase.com",
            Some(coinbase_sub),
            coinbase_is_data,
            coinbase_server_ts,
        )
        .await,
    );

    // OKX
    let okx_sub = serde_json::json!({
        "op": "subscribe",
        "args": [{"channel": "tickers", "instId": "BTC-USDT"}]
    })
    .to_string();
    results.push(
        bench_exchange_ws(
            "OKX WS     (tickers)",
            "wss://ws.okx.com:8443/ws/v5/public",
            Some(okx_sub),
            okx_is_data,
            okx_server_ts,
        )
        .await,
    );

    // Bybit
    let bybit_sub = serde_json::json!({
        "op": "subscribe",
        "args": ["tickers.BTCUSDT"]
    })
    .to_string();
    results.push(
        bench_exchange_ws(
            "Bybit WS   (tickers)",
            "wss://stream.bybit.com/v5/public/spot",
            Some(bybit_sub),
            bybit_is_data,
            bybit_server_ts,
        )
        .await,
    );

    print_summary(&results);

    Ok(())
}
