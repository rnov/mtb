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
```

To switch to live trading, change `mode = "live"` in `config.toml`. Requires funded Polymarket account with USDC allowances set.

## Configuration

**`config.toml`** -- runtime settings (committed):

| Section | Key fields |
|---------|-----------|
| `mode` | `"dry_run"` or `"live"` |
| `[strategy]` | `min_arb_profit`, `assets`, `max_order_size_usdc`, `min_fresh_sources`, `max_price_age_ms`, `max_divergence_pct` |
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

### Unit Tests (61)

Fast, no network. Run anytime:

```bash
cargo test
```

Covers: fee model (11), arb detection (7 — includes leg price filter), freshness guard (8), risk manager (7), trade logger (3), Binance feed parsing (3), Coinbase feed parsing (2), OKX feed parsing (2), Bybit feed parsing (2), Chainlink symbol mapping (2), PriceState multi-exchange (13 — includes exchange-only tier + snapshot tests), position limits (3).

### Integration Tests (17)

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
| `binance_ws_receives_prices` | No | Direct Binance WS, verify bookTicker stream + gap stats |
| `coinbase_ws_receives_prices` | No | Direct Coinbase WS, verify ticker stream + gap stats |
| `okx_ws_receives_prices` | No | Direct OKX WS, verify tickers stream + gap stats |
| `binance_network_latency` | No | Network latency to Binance (50 samples via @trade stream) |
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

### Measured Latencies (local, macOS)

| Source | p50 | p95 | Notes |
|--------|-----|-----|-------|
| Binance | ~31ms | ~32ms | Direct WS, via `@trade` stream `E` field |
| OKX | ~119ms | ~120ms | Direct WS, via server `ts` field |
| Coinbase | ~30ms | — | Direct WS, clock offset noted (negative values) |
| Bybit | ~20ms | ~72ms | Direct WS via publicTrade `T` field, clock offset ~-11ms |
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

## Arb Detection Strategy

Detection moves from **theoretical** to **executable** in four layers:

### Layer 1 — Price check (always active)
```
up_ask + down_ask < 1 − fees
```
Basic parity condition. Necessary but not sufficient — doesn't say whether the book can actually fill.

### Layer 2 — Min-notional filter (Step 8a)
The CLOB rejects any order below $1. Minimum shares needed per leg:
```
min_shares_up   = ceil(1 / up_ask)
min_shares_down = ceil(1 / down_ask)
units           = floor(budget / combined_cost)

skip if units < max(min_shares_up, min_shares_down)
```
Example: up=$0.30 → need ≥ 4 units (4 × $0.30 = $1.20). With budget $3 and combined ~$0.96 → 3 units available < 4 required → skip.

### Layer 3 — Executable size check (Step 8c, requires 8b)
The ask price might exist but with only 1 share available. We need enough depth on both sides:
```
executable_up   = shares available at best_ask_up
executable_down = shares available at best_ask_down
size_cap = min(executable_up, executable_down)
size_min = max(min_shares_up, min_shares_down)

skip if size_cap < size_min
```
Prevents sending FOK orders into books too thin to fill.

### Layer 4 — VWAP arb preservation (Step 8d, requires 8b)
If we need to scale across multiple price levels, the average fill price (VWAP) may exceed the best ask and erase the edge:
```
vwap_up(size_cap) + vwap_down(size_cap) < 1 − fees − buffer

skip if this fails
```
Ensures the arb edge survives at the actual executable size.

### Why this matters
| Check | Problem it solves |
|-------|------------------|
| Price check | Confirms arb exists in theory |
| Min-notional | Avoids CLOB $1/order rejections |
| Executable size | Avoids FOK kills from thin books |
| VWAP | Avoids eroding edge by scaling into depth |

## Execution Semantics

### Order Flow

Each arb opportunity submits two **FOK (fill-or-kill)** orders concurrently via individual `POST /order` calls (`tokio::join!`):
1. Buy Up token at the ask price
2. Buy Down token at the ask price

Both orders use **equal share counts** (units = budget / combined_cost) so every Up token is paired with a Down token. The CLOB processes them independently — they are **not atomic**.

### Order Status

The CLOB returns a `status` field per order. `success: true` only means the order was accepted, not filled.

| Status | Meaning | Action |
|--------|---------|--------|
| `Matched` | Filled — tokens exchanged, USDC spent | Only true "success" |
| `Canceled` | FOK killed — no matching liquidity at our price | Normal, no cost, book moved |
| `Unmatched` | Could not match | Normal, no cost |
| `Live` | Order resting on book | **Unexpected for FOK** — warns, counts as failure |
| `Delayed` | Order queued by CLOB | Unexpected — warns, counts as failure |

### Outcome Matrix

| Up | Down | Result |
|----|------|--------|
| Matched | Matched | Arb filled — guaranteed profit at resolution |
| Matched | Not matched | **Partial fill** — kill switch triggers immediately, operator must review |
| Not matched | Matched | **Partial fill** — kill switch triggers immediately, operator must review |
| Not matched | Not matched | No fill — normal, no cost, bot continues |

### Safety Guards

| Guard | Config | Effect |
|-------|--------|--------|
| Max leg price | `max_leg_price = 0.90` | Rejects arbs where either side > $0.90 — limits directional exposure on partial fill |
| Partial fill kill switch | Automatic | Halts all trading on first partial fill — `/resume` to restart |
| Exposure cap | `max_exposure_usdc` | Cumulative USDC spent limit per session |
| Consecutive failures | `max_consecutive_failures` | Kill switch after N failed orders in a row |
| Feed disconnect | `feed_disconnect_timeout_secs` | Halts trading when orderbook WS disconnects |
| Freshness guard | `min_fresh_sources`, `max_price_age_ms`, `max_divergence_pct` | Skips arbs when exchange prices are stale or divergent (high leg risk) |

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
