# PROGRESS.md - Project Status

## Current Phase
**Phase 2: Live Execution + Safety** — COMPLETE (all 10 steps)

## Phase 1: Foundation (Dry-Run Arb Detection) — COMPLETE
- [x] Rust project scaffolded (`cargo init`)
- [x] CLAUDE.md, AGENT.md, PROGRESS.md created
- [x] Research gathered and verified against real sources
- [x] Identified official SDK: `Polymarket/rs-clob-client` as foundation
- [x] Defined core strategy: 15-min crypto arb on BTC/ETH/SOL/XRP markets
- [x] Architecture plan designed and approved
- [x] Rust toolchain updated 1.82 → 1.93.1 (SDK requires ≥1.88)
- [x] Cargo config: `net.git-fetch-with-cli = true` for SSH auth
- [x] `.deps/rs-clob-client/` cloned locally for source reference
- [x] All Phase 1 dependencies in Cargo.toml
- [x] `error.rs` — unified error types (thiserror)
- [x] `config/` — AppConfig, RunMode, StrategyConfig, RiskConfig, Secrets; loads config.toml + .env
- [x] `market/` — Slug-based discovery via Gamma API (`{asset}-updown-15m-{epoch}`)
- [x] `feed/` — OrderbookFeed via CLOB WebSocket; FeedEvent broadcast channel
- [x] `strategy/fees.rs` — Polymarket dynamic taker fee model (2% rate, symmetric around 0.5)
- [x] `strategy/arb.rs` — Pure arb detection (up_ask + down_ask < 1.0 after fees)
- [x] `execution/` — Executor trait + DryRunExecutor (logs to JSON lines)
- [x] `risk/` — RiskManager with position limits, exposure caps, drawdown, kill switch
- [x] `monitor/logger.rs` — Append-only JSON lines trade log
- [x] `main.rs` — Full event loop: config → discover → WS subscribe → arb detect → risk gate → execute
- [x] `config.toml` — Sane defaults for dry-run mode (all 4 assets)
- [x] 18 unit tests across fees, arb, and risk modules — all passing
- [x] `cargo fmt` / `cargo clippy` / `cargo test` — all clean, zero warnings
- [x] Live dry-run verified: discovers 4 markets, streams orderbook data, runs arb detection in real-time

## Phase 2: Live Execution + Safety — COMPLETE
- [x] **Step 1: SDK Authentication** — `LocalSigner` from private key + `ClobClient` EIP-712 auth flow, `SignatureType::GnosisSafe` for MetaMask proxy wallet
- [x] **Step 2: Order Building** — `Opportunity` → two FOK limit orders (UP + DOWN) with correct token_id/price/size
- [x] **Step 3: LiveExecutor** — Full `Executor` trait impl: auth → build → sign → batch post → response handling
- [x] **Step 4: Execution Error Handling** — 3 retries with exponential backoff (200ms/400ms/800ms), 10s timeout, partial fill detection (4-way match on UP/DOWN success), transient vs permanent error classification
- [x] **Step 5: Feed Disconnect Halt** — `record_feed_activity()` on every orderbook update, `record_disconnect()`/`record_reconnect()` on WS events, dual check in `risk.check()`: explicit disconnect + staleness timeout (no data >5s). 3 unit tests.
- [x] **Step 6: Adaptive Backoff** — Exponential backoff (1s/2s/4s/8s/16s/30s cap) between executions after consecutive failures. Backoff gate in `risk.check()`, resets on success. 3 unit tests.
- [x] **Step 7: Trade Logger Enhancements** — `TradeRecord` struct with opportunity + outcome. `log_trade()` and `log_rejected()` in main loop. All trades (success/fail) and risk-rejected opportunities logged to JSON lines.
- [x] **Step 8: Telegram Bot — Alerts** — `Notifier` struct with alert methods: trade filled/failed, execution error, disconnect/reconnect, kill switch, startup. Feature-gated behind `#[cfg(feature = "telegram")]`.
- [x] **Step 9: Telegram Bot — Commands** — `/status`, `/kill`, `/resume`, `/help`. teloxide `Dispatcher` in background task. Only responds to authorized `chat_id`. `RiskManager` wrapped in `Arc` for sharing.
- [x] **Step 10: Integration Testing** — 8 integration tests: 4 unauthenticated (pipeline, orderbook, geoblock, batch fetch) + 4 authenticated (CLOB auth, balance check, build+sign, live FOK post). All verified against real Polymarket APIs.

### Phase 2 Test Coverage
- 24 unit tests passing (fees, arb, risk: feed disconnect, staleness, adaptive backoff, backoff scaling, backoff clear, feed activity reset, position limits, daily loss, failure tracking)
- 8 live integration tests (`#[ignore]`): market discovery, full pipeline, REST orderbook, geoblock, batch fetch, CLOB auth, build+sign order, live FOK order post
- Authenticated tests verified: EIP-712 auth, $9 USDC balance visible, USDC allowances set, order submission pipeline working
- `cargo fmt` / `cargo clippy` / `cargo test` — all clean, zero warnings

### Phase 3 Test Coverage (cumulative)
- 27 unit tests passing (+3: Binance URL builder, asset-to-symbol mapping, bookTicker JSON parsing)
- 10 live integration tests (+2: `binance_ws_receives_prices`, `binance_network_latency`)
- Binance latency measured: ~30ms from local machine. Run `binance_network_latency` on VPS to compare.
- `cargo fmt` / `cargo clippy` / `cargo test` — all clean, zero warnings

## Architecture Notes
- **Wallet**: MetaMask EOA private key + `SignatureType::GnosisSafe` (type 2). SDK auto-derives proxy wallet address via CREATE2. Funds deposited through Polymarket UI go to proxy wallet. CLOB authenticates with EOA key, trades from proxy.
- `.deps/rs-clob-client/` is a local clone for reading source only; not used by build
- SDK dependency is pulled from GitHub via git in `Cargo.toml`
- `.gitignore` excludes `/target`, `.deps/`, `.env`, `.claude`
- Market discovery uses slug-based lookup: `GET /events/slug/{asset}-updown-15m-{epoch}`
- Outcomes are "Up"/"Down" (not "Yes"/"No")
- Markets are ephemeral — new slug every 15 minutes, aligned to ET

## Phase 3: External Feeds + Directional Strategy — IN PROGRESS
- [x] **Step 0: Latency Instrumentation** — Timing spans across the hot path: tick-to-trade in `main.rs` (arb_us, risk_us, exec_ms, total_ms), execution breakdown in `live.rs` (auth_ms, build_ms, sign_ms, post_ms). All logged as structured tracing fields.
- [x] **Step 1: Binance Direct Price Feed** — Direct WebSocket to `wss://stream.binance.com:9443` (NOT via Polymarket RTDS relay). Combined `bookTicker` stream for BTC/ETH/SOL/XRP. Auto-reconnect on disconnect. Processing latency logged every 1000 ticks. Network latency test via `@trade` stream (30ms from local, run on VPS to compare). 3 unit tests + 2 integration tests.
- [ ] Step 2: Price state manager
- [ ] Step 3: Directional signal
- [ ] Step 4: Directional strategy
- [ ] Step 5: Strategy selector
- [ ] Step 6: Backtesting / dry-run validation
- [ ] Step 7: Chainlink Data Streams (optional)
- [ ] Step 8: EOA implementation
- [ ] Step 9: Deployment prep
- [ ] Step 10: Production deployment

## Next: Phase 3 Details

### Feed Strategy
Polymarket's 15-min crypto markets resolve via **Chainlink Data Streams** which aggregate
from multiple exchanges. To detect edges before Polymarket odds adjust, we need external
price feeds that **lead** Chainlink:

**Primary (must-have):**
- Binance WS (spot/perp) — highest volume, primary price leader. Already supported via
  SDK's RTDS module (`subscribe_crypto_prices`). Symbols: btcusdt, ethusdt, solusdt, xrpusdt.
- Chainlink Data Streams — direct low-latency feed for resolution price estimates.
  Requires sponsored API key (request via Polymarket/Chainlink).

**Secondary (confirmation/momentum):**
- Bybit — fast during volatility, strong perps/futures
- OKX — high volume on major pairs, leads Asian hours
- Coinbase/Kraken — US-regulated, frequently in Chainlink aggregations

**Cross-venue (Phase 4):**
- Kalshi — CFTC-regulated fiat prediction market, main cross-arb rival for overlapping events

### Implementation Plan
- [ ] `feed/crypto_price.rs` — RTDS Binance price feed (SDK built-in)
- [ ] `feed/chainlink.rs` — Direct Chainlink Data Streams (sponsored key)
- [ ] `strategy/directional.rs` — Directional signal: predict Chainlink resolution from
  leading exchange prices, bet Up/Down when Polymarket odds lag
- [ ] `monitor/metrics.rs` — Prometheus endpoint

## Key Decisions
| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust | Performance, safety, reliability |
| Async runtime | tokio | Industry standard for Rust async |
| SDK | rs-clob-client (official) | Only trusted Polymarket Rust client |
| Assets | BTC, ETH, SOL, XRP | All 4 live 15-min markets on Polymarket |
| Discovery | Slug-based (`{asset}-updown-15m-{epoch}`) | Ephemeral markets not in Gamma pagination |
| Fee model | 2% dynamic taker fee | Symmetric around 0.5, validated with unit tests |
| Primary price signal | Binance (via RTDS) | Highest volume, leads Chainlink aggregation |

## Deployment
- **VPS**: Amsterdam (Hetzner to start, ~€10/month). Best latency tradeoff: ~5-10ms to
  Polymarket CLOB (London), ~10-20ms to Polygon RPC (Frankfurt), ~100-150ms to Binance (Tokyo).
- **Private RPC**: Required for Phase 2 (Alchemy/QuickNode EU, ~$50/month). Non-negotiable
  for both speed and reliability of order submission.
- **Latency budget**: Edges are milliseconds, not seconds. Exchange price moves propagate to
  Chainlink → Polymarket in ~100-200ms. Our total tick-to-trade must be competitive:
  Binance WS (~100ms) + processing (<1ms) + CLOB submit (~10ms) ≈ 110ms.
- **Geo-compliance**: Netherlands is not geo-blocked. Add geoblock check at bot startup.

## Risk Register
| Risk | Mitigation |
|------|------------|
| Fee modeling errors | Precise fee calc with unit tests against known values |
| Arb edges too competitive | Validate with dry-run before risking capital |
| API rate limits | Respect CLOB rate limits, implement backoff |
| Chainlink latency | Monitor Binance as leading indicator; get sponsored Chainlink key |
| MEV on Polygon | Use private relay / Flashbots-equivalent for order submission (research needed) |
| Edge window is ms not s | Optimize every hop; private RPC; benchmark real pings before going live |
