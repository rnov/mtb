# MTB Alpha

Polymarket trading bot targeting 15-minute crypto up/down markets (BTC, ETH, SOL, XRP).

Detects arbitrage when `up_ask + down_ask < 1.0` after fees, cross-validated against real-time prices from Binance, Coinbase, and OKX to reject phantom arbs. Built in Rust on top of the official [Polymarket/rs-clob-client](https://github.com/Polymarket/rs-clob-client) SDK.

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
```

To switch to live trading, change `mode = "live"` in `config.toml`. Requires funded Polymarket account with USDC allowances set.

## Configuration

**`config.toml`** -- runtime settings (committed):

| Section | Key fields |
|---------|-----------|
| `mode` | `"dry_run"` or `"live"` |
| `[strategy]` | `min_arb_profit`, `assets`, `max_order_size_usdc` |
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
  CLOB WebSocket       Exchange WebSockets    Chainlink (planned)
  (orderbook data)     (direct price feeds)   (resolution price)
           |                   |                   |
           |           +-------+-------+           |
           |           v       v       v           |
           |       Binance  Coinbase  OKX          |
           |       bookTicker ticker  tickers      |
           |           |       |       |           |
           |           +---+---+---+---+           |
           |               v                       |
           |         PriceState                    |
           |         (multi-exchange               |
           |          cross-validation)            |
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
           - cross-validate with PriceState
           - calculate fees precisely
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
    chainlink.rs          Chainlink Data Streams via RTDS (resolution source)
    price_state.rs        Multi-exchange price state + cross-validation
    types.rs              FeedEvent enum (OrderbookUpdate, CryptoPrice, ChainlinkPrice, Disconnected, Reconnected)

  strategy/
    mod.rs                Re-exports
    arb.rs                Pure arb: up_ask + down_ask < 1.0 after fees
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
    logger.rs             Append-only JSON lines trade log
```

## Testing

### Unit Tests (40)

Fast, no network. Run anytime:

```bash
cargo test
```

Covers: fee model (11), arb detection (5), risk manager (7), Binance feed parsing (3), Coinbase feed parsing (2), OKX feed parsing (2), Chainlink symbol mapping (2), PriceState multi-exchange (7), position limits (3).

### Integration Tests (15)

Hit real APIs. Marked `#[ignore]`, require network access:

```bash
# Run all integration tests
cargo test -- --ignored --nocapture
```

Or run specific tests:

```bash
# Exchange price feeds (no .env needed)
cargo test binance_ws_receives_prices -- --ignored --nocapture
cargo test coinbase_ws_receives_prices -- --ignored --nocapture
cargo test okx_ws_receives_prices -- --ignored --nocapture

# Network latency diagnostics (run locally and on VPS to compare)
cargo test binance_network_latency -- --ignored --nocapture
cargo test coinbase_network_latency -- --ignored --nocapture
cargo test okx_network_latency -- --ignored --nocapture

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
| `binance_ws_receives_prices` | No | Direct Binance WS, verify bookTicker stream + gap stats |
| `coinbase_ws_receives_prices` | No | Direct Coinbase WS, verify ticker stream + gap stats |
| `okx_ws_receives_prices` | No | Direct OKX WS, verify tickers stream + gap stats |
| `binance_network_latency` | No | Network latency to Binance (50 samples via @trade stream) |
| `coinbase_network_latency` | No | Network latency to Coinbase (50 samples via ticker timestamp) |
| `okx_network_latency` | No | Network latency to OKX (50 samples via server timestamp) |
| `chainlink_rtds_receives_prices` | No | Chainlink resolution prices via RTDS (15 samples, gap + latency) |
| `pipeline_discover_fetch_detect` | No | Full pipeline: discover markets, fetch orderbook, run arb detection |
| `rest_orderbook_returns_valid_data` | No | Validate orderbook structure and price ranges |
| `geoblock_check` | No | Verify IP is not geo-blocked |
| `batch_orderbook_fetch` | No | Multi-market batch orderbook fetch |
| `auth_clob_authenticate` | Yes | EIP-712 auth with GnosisSafe signature type |
| `auth_check_balance_allowance` | Yes | Check USDC balance and allowances on proxy wallet |
| `auth_build_and_sign_order` | Yes | Build and sign a FOK order (no submission) |
| `live_post_unfillable_fok_order` | Yes | Post FOK at $0.01 (immediately killed, $0 cost) |

### Measured Latencies (local, macOS)

| Source | p50 | p95 | Notes |
|--------|-----|-----|-------|
| Binance | ~31ms | ~32ms | Direct WS, via `@trade` stream `E` field |
| OKX | ~119ms | ~120ms | Direct WS, via server `ts` field |
| Coinbase | ~30ms | — | Direct WS, clock offset noted (negative values) |
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

## Key Design Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Language | Rust | Performance, safety, reliability |
| Async runtime | tokio | Industry standard |
| SDK | rs-clob-client (official) | Only trusted Polymarket Rust client |
| Price feeds | Binance + Coinbase + OKX (direct WS) | Multi-exchange cross-validation, lowest latency |
| Resolution source | Chainlink Data Streams (planned) | Actual resolution price for 15-min markets |
| Wallet type | GnosisSafe (Type 2) | Gasless, MetaMask compatible |
| Fee model | 2% dynamic taker | Symmetric around 0.5, unit-tested |
| Order type | FOK (fill-or-kill) | No partial fills, no open orders to manage |
| Market discovery | Slug-based | Ephemeral 15-min markets not in Gamma pagination |
