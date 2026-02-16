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
- 50 unit tests passing (+26 from Phase 2: feeds parsing/mapping, PriceState, freshness guard)
- 17 live integration tests (+9: exchange gap/latency tests for 4 exchanges + Chainlink RTDS)
- Network latency (local): Binance ~31ms p50, OKX ~119ms p50, Coinbase ~30ms (clock offset noted), Chainlink RTDS ~1-1.5s (relay overhead)
- `cargo fmt` / `cargo clippy` / `cargo test` — all clean, zero warnings

## Architecture Notes
- **Wallet**: MetaMask EOA private key + `SignatureType::GnosisSafe` (type 2). SDK auto-derives proxy wallet address via CREATE2. Funds deposited through Polymarket UI go to proxy wallet. CLOB authenticates with EOA key, trades from proxy.
- `.deps/rs-clob-client/` is a local clone for reading source only; not used by build
- SDK dependency is pulled from GitHub via git in `Cargo.toml`
- `.gitignore` excludes `/target`, `.deps/`, `.env`, `.claude`
- Market discovery uses slug-based lookup: `GET /events/slug/{asset}-updown-15m-{epoch}`
- Outcomes are "Up"/"Down" (not "Yes"/"No")
- Markets are ephemeral — new slug every 15 minutes, aligned to ET

## Phase 3: External Feeds + Arb Validation — IN PROGRESS
- [x] **Step 0: Latency Instrumentation** — Timing spans across the hot path: tick-to-trade in `main.rs` (arb_us, risk_us, exec_ms, total_ms), execution breakdown in `live.rs` (auth_ms, build_ms, sign_ms, post_ms). All logged as structured tracing fields.
- [x] **Step 1: Binance Direct Price Feed** — Direct WebSocket to `wss://stream.binance.com:9443` (NOT via Polymarket RTDS relay). Combined `bookTicker` stream for BTC/ETH/SOL/XRP. Auto-reconnect on disconnect. Processing latency logged every 1000 ticks. Network latency test via `@trade` stream (30ms from local, run on VPS to compare). 3 unit tests + 2 integration tests.
- [x] **Step 2: Price State Manager** — Multi-exchange `PriceState` keyed by `exchange:symbol`. Tracks latest price, staleness, freshness per exchange+asset. Cross-exchange validation via `prices_agree()` for phantom arb rejection. 7 unit tests.
- [x] **Step 3: Multi-Exchange Feeds** — Added Coinbase (`advanced-trade-ws.coinbase.com` ticker) and OKX (`ws.okx.com:8443` tickers) direct WebSocket feeds. Same auto-reconnect pattern as Binance. `FeedEvent::CryptoPrice` now includes `exchange` field. Gap + network latency tests for all 3 exchanges. Measured latency: Binance ~31ms p50, OKX ~119ms p50, Coinbase ~30ms (clock offset). 4 unit tests + 4 integration tests.
- [x] **Step 4: Chainlink Data Streams (via RTDS)** — Resolution source feed using SDK's `subscribe_chainlink_prices()` via `wss://ws-live-data.polymarket.com`. Streams real-time Chainlink oracle prices for BTC/ETH/SOL/XRP. Added as `"chainlink"` source in `PriceState` for cross-validation. Measured latency: ~1-1.5s via RTDS relay. Sponsored Chainlink API key application pending (https://pm-ds-request.streams.chain.link/) for lower latency direct access. 2 unit tests + 1 integration test.
- [x] **Step 5: Arb Freshness Guard** — `validate_freshness()` between arb detection and risk check. 3 checks: sufficient fresh exchange sources (default 2), price staleness (<2s), cross-exchange price agreement (<0.5% divergence). Config-driven thresholds in `StrategyConfig`. Rejected arbs logged to trade log. 8 unit tests.
- [x] **Step 6: Bybit Direct WS Feed** — Direct WebSocket to `wss://stream.bybit.com/v5/public/spot` for spot tickers. Uses `lastPrice` (spot tickers are snapshot+delta, update on price change only). Added as `"bybit"` source in `PriceState` (5 sources per asset now). Latency test via `publicTrade` stream `T` field (clock offset ~-11ms, similar to Coinbase). Gap avg ~762ms (quiet periods), faster during volatility. 2 unit tests + 2 integration tests.
- [ ] Step 7: Volatility tracker — rolling window, dynamic arb confidence thresholds
- [ ] Step 8: Multi-market prioritization — score markets by arb likelihood
- [ ] Step 9: Backtesting / dry-run validation — log all arbs with multi-exchange context
- [ ] Step 10: EOA implementation
- [ ] Step 11: Deployment prep + production

### Feed Strategy
Polymarket's 15-min crypto markets resolve via **Chainlink Data Streams** which aggregate
from multiple exchanges. More price sources = higher confidence that a detected arb is
real (not phantom from stale orderbooks). If all exchanges agree the price is stable,
the Polymarket orderbook is trustworthy. If they show a spike, the arb is likely phantom.

**Primary (implemented):**
- Binance WS (direct bookTicker) — highest volume, primary price leader

**Validation sources (implemented):**
- Coinbase WS — US-regulated, frequently in Chainlink aggregations
- OKX WS — high volume on major pairs, leads Asian hours
- Bybit WS — leads during volatility, key for Asian hours

**Resolution source (implemented — Step 4):**
- Chainlink Data Streams via RTDS — the actual resolution price for Polymarket 15-min markets.
  Knowing Chainlink's price = knowing the answer before the market closes.
  Currently via Polymarket RTDS relay (~1-1.5s latency). Sponsored Chainlink API key
  application pending for direct access (lower latency).

**Pending:**
- Sponsored Chainlink API key — apply at https://pm-ds-request.streams.chain.link/
- Latency policy review — sources range from ~31ms (Binance) to ~1.5s (Chainlink RTDS); need tiered freshness or weighted validation

## Future Opportunities
- **5-minute markets**: Polymarket launched 5-min crypto markets (Feb 2026), same Chainlink
  resolution. Same strategy applies — higher frequency = more windows per day (288/asset vs 96).
  Requires faster execution and tighter latency budget.

## Key Decisions
| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust | Performance, safety, reliability |
| Async runtime | tokio | Industry standard for Rust async |
| SDK | rs-clob-client (official) | Only trusted Polymarket Rust client |
| Assets | BTC, ETH, SOL, XRP | All 4 live 15-min markets on Polymarket |
| Discovery | Slug-based (`{asset}-updown-15m-{epoch}`) | Ephemeral markets not in Gamma pagination |
| Fee model | 2% dynamic taker fee | Symmetric around 0.5, validated with unit tests |
| Strategy | Pure arb (buy both sides < $1.00) | Risk-free, guaranteed profit regardless of outcome |
| Price feeds | Binance + Coinbase + OKX (direct WS) | Multi-exchange cross-validation for phantom arb rejection |
| Resolution source | Chainlink Data Streams | Actual resolution price for 15-min markets |

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
