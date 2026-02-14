# PROGRESS.md - Project Status

## Current Phase
**Phase 1: Foundation (Dry-Run Arb Detection)** — COMPLETE

## Completed
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

## Architecture Notes
- `.deps/rs-clob-client/` is a local clone for reading source only; not used by build
- SDK dependency is pulled from GitHub via git in `Cargo.toml`
- `.gitignore` excludes `/target`, `.deps/`, `.env`, `.claude`
- Market discovery uses slug-based lookup: `GET /events/slug/{asset}-updown-15m-{epoch}`
- Outcomes are "Up"/"Down" (not "Yes"/"No")
- Markets are ephemeral — new slug every 15 minutes, aligned to ET

## Next: Phase 2 (Live Execution + Safety)
- [ ] `execution/executor.rs` — LiveExecutor: SDK auth, order building, submission
- [ ] `risk/` enhancements — feed disconnect halt, adaptive backoff
- [ ] `monitor/telegram.rs` — Telegram alerts + /status, /kill, /resume commands

## Next: Phase 3 (External Feeds + Directional Strategy)

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
