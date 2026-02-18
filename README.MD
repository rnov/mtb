# MTB Alpha

Polymarket trading bot targeting 15-minute crypto up/down markets (BTC, ETH, SOL, XRP).

Detects arbitrage when `up_ask + down_ask < 1.0` after fees. Uses real-time prices from Binance, Coinbase, OKX, and Bybit to assess leg risk — if exchanges show rapid price movement, orderbook asks are likely to be canceled before we can fill both sides. Built in Rust on top of the official [Polymarket/rs-clob-client](https://github.com/Polymarket/rs-clob-client) SDK.

## Status

| Phase | Status |
|-------|--------|
| Phase 1: Dry-run arb detection | Complete |
| Phase 2: Live execution + safety | Complete |
| Phase 3: External feeds + arb validation | In progress |

See [PROGRESS.md](PROGRESS.md) for detailed step-by-step status.

## Quick Start

### Prerequisites

- Rust >= 1.88 (`rustup update stable`)
- A `.env` file with secrets (see below)
- `config.toml` is committed with sane defaults

### Setup

```bash
# Clone and build
cargo build

# Create .env with your secrets
cat > .env << 'EOF'
POLYMARKET_PRIVATE_KEY=your_64_char_hex_key_no_0x_prefix
TELEGRAM_BOT_TOKEN=your_telegram_bot_token
EOF
```

### Run

```bash
# Dry-run mode (default) -- watches markets, detects arbs, no real orders
cargo run

# With debug logging (see Binance prices, orderbook updates, latency)
RUST_LOG=mtb=debug cargo run

# Timed dry-run test (default 3 minutes, logs to logs/trades.jsonl)
./scripts/dry-run-test.sh        # 3 minutes
./scripts/dry-run-test.sh 1      # 1 minute
./scripts/dry-run-test.sh 10     # 10 minutes

# Latency benchmark — CLOB REST/WS + all exchange feeds, summary table
cargo run --bin latency_bench
```

To switch to live trading, change `mode = "live"` in `config.toml`. Requires funded Polymarket account with USDC allowances set.

## Configuration

**`config.toml`** -- runtime settings (committed):

| Section | Key fields |
|---------|-----------|
| `mode` | `"dry_run"` or `"live"` |
| `[strategy]` | `min_arb_profit`, `assets`, `max_order_size_usdc`, `max_leg_price`, `min_order_usdc`, `min_fresh_sources`, `max_price_age_ms`, `max_divergence_pct` |
| `[risk]` | `max_exposure_usdc`, `max_daily_loss_usdc`, `max_consecutive_failures`, `feed_disconnect_timeout_secs` |
| `[discovery]` | `poll_interval_secs`, `assets` |
| `[telegram]` | `chat_id` |

**`.env`** -- secrets (never committed):

| Variable | Required | Description |
|----------|----------|-------------|
| `POLYMARKET_PRIVATE_KEY` | Live mode | MetaMask private key (64 hex chars, no `0x` prefix) |
| `TELEGRAM_BOT_TOKEN` | Optional | Telegram bot token from BotFather |

## Telegram Bot

Commands (only responds to authorized `chat_id`):

| Command | Description |
|---------|-------------|
| `/status` | Current risk state, exposure, feed health |
| `/kill` | Manually trigger kill switch (stop trading) |
| `/resume` | Reset kill switch (resume trading) |
| `/help` | List commands |

Automatic alerts: trade filled/failed, execution errors, feed disconnect/reconnect, kill switch triggered, startup notification.

## Architecture

```
Gamma (HTTP 60s poll) --> Market Registry (active MarketPairs)
                               |
           +-------------------+-------------------+
           v                   v                   v
  CLOB WebSocket       Exchange WebSockets    Chainlink RTDS
  (orderbook data)     (direct price feeds)   (resolution price)
           |                   |                   |
           |           +-------+-------+           |
           |           v       v       v           |
           |       Binance Coinbase OKX Bybit       |
           |       bookTkr ticker  tickers tickers |
           |           |       |       |           |
           |           +---+---+---+---+           |
           |               v                       v
           |         PriceState              PriceState
           |         (exchange tier:         (resolution tier:
           |          freshness guard)        Chainlink only)
           |               |                       |
           +-------+-------+-----------+-----------+
                   v
           broadcast channel
                   v
           Main Event Loop (tokio::select!)
                   v
           Strategy Engine (pure, no IO)
           - build MarketSnapshot
           - detect arb (up_ask + down_ask < 1.0)
           - calculate fees precisely
                   v
           Freshness Guard — leg risk filter (pure, no IO)
           - enough fresh exchange sources? (exchanges only, excludes Chainlink)
           - prices stale? (>2s = book likely moving)
           - exchanges agree? (<0.5% divergence = stable book, safe to fill)
                   v
           Risk Manager (sync, RwLock)
           - position limit check
           - exposure cap check
           - kill switch check
           - feed staleness check
           - adaptive backoff check
                   v
           Executor (trait-based)
           - Live: SDK order placement (FOK)
           - DryRun: log to JSON lines
                   v
           Monitor
           - Telegram alert
           - JSON log line
```

## Module Layout

```
src/
  main.rs                 Entry point, event loop
  lib.rs                  Re-exports for tests
  error.rs                Unified error types (thiserror)

  bin/
    latency_bench.rs      Standalone benchmark: CLOB REST/WS + exchange WS gaps + clock deltas

  config/
    mod.rs                Config loading (.env + config.toml)
    types.rs              AppConfig, RunMode, StrategyConfig, RiskConfig, Secrets

  market/
    mod.rs                Re-exports
    discovery.rs          Gamma API: slug-based 15-min market discovery
    types.rs              MarketPair (up/down token IDs, asset, expiry)

  feed/
    mod.rs                Feed manager (multiplexes WS sources)
    orderbook.rs          CLOB WebSocket: BestBidAsk per token
    crypto_price.rs       Binance direct WS: combined bookTicker stream
    coinbase.rs           Coinbase direct WS: Advanced Trade ticker
    okx.rs                OKX direct WS: public tickers channel
    bybit.rs              Bybit direct WS: V5 spot tickers (lastPrice)
    chainlink.rs          Chainlink Data Streams via RTDS (resolution source)
    price_state.rs        Multi-exchange price state + leg-risk assessment
    types.rs              FeedEvent enum (OrderbookUpdate, CryptoPrice, ChainlinkPrice, Disconnected, Reconnected)

  strategy/
    mod.rs                Re-exports
    arb.rs                Pure arb: up_ask + down_ask < 1.0 after fees
    freshness.rs          Freshness guard: assess leg risk via exchange price agreement
    fees.rs               Polymarket dynamic taker fee model (2% rate)
    types.rs              MarketSnapshot, Opportunity, OpportunityKind

  execution/
    mod.rs                Executor trait + ExecutionResult
    live.rs               LiveExecutor: auth, build, sign, batch post (GnosisSafe)
    dry_run.rs            DryRunExecutor: logs to JSON lines

  risk/
    mod.rs                RiskManager: gates every execution
    limits.rs             Position limits, exposure caps, daily loss, failure tracking
    kill_switch.rs        Global halt (auto + manual)

  monitor/
    mod.rs                Re-exports
    telegram.rs           Notifier (alerts) + command handler (/status, /kill, /resume)
    logger.rs             JSON lines trade log with pipeline stage + exchange snapshots
```

## Testing

### Unit Tests (66)

Fast, no network. Run anytime:

```bash
cargo test
```

Covers: fee model (11), arb detection (7 — includes leg price filter), freshness guard (8), risk manager (7), trade logger (3), Binance feed parsing (3), Coinbase feed parsing (2), OKX feed parsing (2), Bybit feed parsing (2), Chainlink symbol mapping (2), PriceState multi-exchange (13 — includes exchange-only tier + snapshot tests), position limits (3).

### Integration Tests (20)

Hit real APIs. Marked `#[ignore]`, require network access:

```bash
# Run all integration tests
cargo test -- --ignored --nocapture
```

Or run specific tests:

```bash
# CLOB REST/WS (no .env needed)
cargo test clob_ws_receives_orderbook_updates -- --ignored --nocapture
cargo test clob_rest_latency -- --ignored --nocapture
cargo test clob_ws_orderbook_latency -- --ignored --nocapture

# Exchange price feeds (no .env needed)
cargo test binance_ws_receives_prices -- --ignored --nocapture
cargo test coinbase_ws_receives_prices -- --ignored --nocapture
cargo test okx_ws_receives_prices -- --ignored --nocapture
cargo test bybit_ws_receives_prices -- --ignored --nocapture

# Network latency diagnostics (run locally and on VPS to compare)
cargo test binance_network_latency -- --ignored --nocapture
cargo test coinbase_network_latency -- --ignored --nocapture
cargo test okx_network_latency -- --ignored --nocapture
cargo test bybit_network_latency -- --ignored --nocapture

# Chainlink resolution feed (via RTDS)
cargo test chainlink_rtds_receives_prices -- --ignored --nocapture

# Polymarket pipeline (no .env needed)
cargo test pipeline_discover_fetch_detect -- --ignored --nocapture
cargo test rest_orderbook_returns_valid_data -- --ignored --nocapture
cargo test geoblock_check -- --ignored --nocapture
cargo test batch_orderbook_fetch -- --ignored --nocapture

# Authenticated (requires POLYMARKET_PRIVATE_KEY in .env)
cargo test auth_clob_authenticate -- --ignored --nocapture
cargo test auth_check_balance_allowance -- --ignored --nocapture
cargo test auth_build_and_sign_order -- --ignored --nocapture
cargo test live_post_unfillable_fok_order -- --ignored --nocapture
```

| Test | Auth | What it does |
|------|------|-------------|
| `clob_ws_receives_orderbook_updates` | No | CLOB WS subscription: validate book structure, measure inter-message gaps |
| `clob_rest_latency` | No | CLOB REST round-trip latency: 20 `order_book()` calls, p50/p95/p99 |
| `clob_ws_orderbook_latency` | No | CLOB WS timing: connect time, first-message time, inter-message gaps |
| `binance_ws_receives_prices` | No | Direct Binance WS, verify bookTicker stream + gap stats |
| `coinbase_ws_receives_prices` | No | Direct Coinbase WS, verify ticker stream + gap stats |
| `okx_ws_receives_prices` | No | Direct OKX WS, verify tickers stream + gap stats |
| `binance_network_latency` | No | Network latency to Binance (50 samples via `@trade` stream `E` field) |
| `coinbase_network_latency` | No | Network latency to Coinbase (50 samples via ticker timestamp) |
| `okx_network_latency` | No | Network latency to OKX (50 samples via server timestamp) |
| `bybit_ws_receives_prices` | No | Direct Bybit WS, verify spot tickers stream + gap stats |
| `bybit_network_latency` | No | Network latency to Bybit (50 samples via publicTrade `T` field) |
| `chainlink_rtds_receives_prices` | No | Chainlink resolution prices via RTDS (15 samples, gap + latency) |
| `pipeline_discover_fetch_detect` | No | Full pipeline: discover markets, fetch orderbook, run arb detection |
| `rest_orderbook_returns_valid_data` | No | Validate orderbook structure and price ranges |
| `geoblock_check` | No | Verify IP is not geo-blocked |
| `batch_orderbook_fetch` | No | Multi-market batch orderbook fetch |
| `auth_clob_authenticate` | Yes | EIP-712 auth with GnosisSafe signature type |
| `auth_check_balance_allowance` | Yes | Check USDC balance and allowances on proxy wallet |
| `auth_build_and_sign_order` | Yes | Build and sign a FOK order (no submission) |
| `live_post_unfillable_fok_order` | Yes | Post FOK at $0.01 (immediately killed, $0 cost) |

### Latency Benchmark

`cargo run --bin latency_bench` measures every data source in one run and prints a unified summary table. Use this to compare local vs VPS latency before deploying.

Covers: CLOB REST round-trip, CLOB WS (connect + first-message + inter-message gaps), Binance/Coinbase/OKX/Bybit WS (connect, first-message, p50/p95 gaps, clock delta vs server timestamp).

### Measured Latencies (local, macOS)

CLOB backend is **AWS eu-west-2 (London)** — confirmed in [Polymarket docs](https://docs.polymarket.com/developers/CLOB/geoblock). Deploy to Amsterdam or Frankfurt for lowest REST latency (~10-20ms expected vs ~63ms local).

| Source | p50 | p95 | Notes |
|--------|-----|-----|-------|
| CLOB REST | ~63ms | ~102ms | Round-trip RTT. Ping to Cloudflare edge ~8ms; remainder is backend hop to London |
| CLOB WS | ~107ms | ~426ms | Inter-message gaps (no server timestamp). First message ~400ms (lazy connect + snapshot) |
| Binance WS | ~31ms | ~32ms | Clock delta vs server `E` field (`@trade` stream) |
| Coinbase WS | ~30ms | — | Clock offset ~-38ms (our clock behind Coinbase's) |
| OKX WS | ~133ms | — | Clock delta vs `data[0].ts`; OKX servers in Asia |
| Bybit WS | ~20ms | ~72ms | Clock offset ~-8ms |
| Chainlink (RTDS) | ~1.2s | ~1.5s | Via Polymarket RTDS relay (sponsored key pending for direct) |

### Lint and Format

```bash
cargo clippy    # Zero warnings policy
cargo fmt       # Standard rustfmt
```

## Wallet Setup

Uses MetaMask with `SignatureType::GnosisSafe` (browser wallet / Type 2). The SDK auto-derives a proxy wallet address via CREATE2 from the EOA signer.

- Funds deposited through Polymarket UI go to the proxy wallet
- CLOB authenticates with EOA private key, trades execute from proxy
- GnosisSafe trades are gasless (Polymarket relays)
- USDC allowances must be set (triggered by first manual trade on Polymarket UI)

## Strategy

See [STRATEGY.md](STRATEGY.md) for the full operational reference:
- Step-by-step strategy logic (discovery → feeds → filters → sizing → execution → risk)
- All 5 arb detection filters with math and examples
- Worked example end-to-end
- Config calibration guide (`scripts/exposure_calculator.py`)
- Decision log (key design choices with rationale)

## Key Design Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Language | Rust | Performance, safety, reliability |
| Async runtime | tokio | Industry standard |
| SDK | rs-clob-client (official) | Only trusted Polymarket Rust client |
| Price feeds | Binance + Coinbase + OKX + Bybit (direct WS) | Leg-risk assessment: detect rapid price movement before filling both sides |
| Resolution source | Chainlink Data Streams (via RTDS) | Resolution oracle — detects danger-zone moments near strike, diagnoses abnormal oracle behavior |
| Source tiering | Exchange (fast) vs Resolution (slow) | Exchanges assess leg risk; Chainlink tracks resolution separately (different latency profiles) |
| Wallet type | GnosisSafe (Type 2) | Gasless, MetaMask compatible |
| Fee model | 2% dynamic taker | Symmetric around 0.5, unit-tested |
| Order type | FOK (fill-or-kill) | No partial fills, no open orders to manage |
| Market discovery | Slug-based | Ephemeral 15-min markets not in Gamma pagination |
