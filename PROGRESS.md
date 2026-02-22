# PROGRESS.md - Project Status

## Current Phase
**Phase 3: External Feeds + Arb Validation** — IN PROGRESS (Step 7b complete, Steps 8-9 remaining)

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
- 61 unit tests passing (+37 from Phase 2: feeds, PriceState with exchange-only tier + snapshots, freshness guard, trade logger, leg price filter)
- 21 live integration tests (+9: exchange gap/latency tests for 4 exchanges + Chainlink RTDS; +1 CLOB min-size probe across all 4 assets)
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
- [x] **Step 2: Price State Manager** — Multi-exchange `PriceState` keyed by `exchange:symbol`. Tracks latest price, staleness, freshness per exchange+asset. Exchange agreement via `prices_agree()` for leg-risk assessment — if exchanges diverge, book is moving and fills are unlikely. 7 unit tests.
- [x] **Step 3: Multi-Exchange Feeds** — Added Coinbase (`advanced-trade-ws.coinbase.com` ticker) and OKX (`ws.okx.com:8443` tickers) direct WebSocket feeds. Same auto-reconnect pattern as Binance. `FeedEvent::CryptoPrice` now includes `exchange` field. Gap + network latency tests for all 3 exchanges. Measured latency: Binance ~31ms p50, OKX ~119ms p50, Coinbase ~30ms (clock offset). 4 unit tests + 4 integration tests.
- [x] **Step 4: Chainlink Data Streams (via RTDS)** — Resolution source feed using SDK's `subscribe_chainlink_prices()` via `wss://ws-live-data.polymarket.com`. Streams real-time Chainlink oracle prices for BTC/ETH/SOL/XRP. Added as `"chainlink"` source in `PriceState` (resolution tier, separate from exchange tier). Not essential for pure arb, but useful for: detecting danger-zone moments near strike, diagnosing abnormal oracle behavior, and future directional strategies. Measured latency: ~1-1.5s via RTDS relay. Sponsored Chainlink API key application pending (https://pm-ds-request.streams.chain.link/) for lower latency direct access. 2 unit tests + 1 integration test.
- [x] **Step 5: Arb Freshness Guard (leg-risk filter)** — `validate_freshness()` between arb detection and risk check. Assesses whether both sides can be filled before the book moves. 3 checks: sufficient fresh exchange sources (default 2), price staleness (<2s = book likely moving), exchange price agreement (<0.5% divergence = stable book, safe to fill). Config-driven thresholds in `StrategyConfig`. Rejected arbs logged to trade log. 8 unit tests.
- [x] **Step 6: Bybit Direct WS Feed** — Direct WebSocket to `wss://stream.bybit.com/v5/public/spot` for spot tickers. Uses `lastPrice` (spot tickers are snapshot+delta, update on price change only). Added as `"bybit"` source in `PriceState` (5 sources per asset now). Latency test via `publicTrade` stream `T` field (clock offset ~-11ms, similar to Coinbase). Gap avg ~762ms (quiet periods), faster during volatility. 2 unit tests + 2 integration tests.
- [x] **Step 6b: Tiered Source Architecture** — Separated price sources into two tiers: **Exchange tier** (Binance, Coinbase, OKX, Bybit — fast, ~20-120ms) used for freshness guard validation, and **Resolution tier** (Chainlink RTDS — ~1-1.5s) used separately for resolution confidence. Freshness guard now uses exchange-only methods (`count_fresh_exchange_sources`, `exchange_staleness`, `exchanges_agree`) so Chainlink's relay latency doesn't cause false divergence rejections or weaken validation by counting as a "fresh source." 4 new PriceState unit tests for exchange-only tier behavior.
- [x] **Step 7: Dry-Run Validation Logging** — Enriched `TradeRecord` with full pipeline context for data collection. Every detected arb now logs: `PipelineStage` (freshness_rejected, risk_rejected, executed, execution_failed), `ExchangeSnapshot` (each exchange's price + age at decision time, fresh count, divergence, Chainlink price/age), and timing breakdown (arb_us, fresh_us, risk_us, exec_ms, total_us). `PriceState::snapshot_asset()` captures exchange context at arb detection time. Run in `dry_run` mode to collect data for threshold tuning without placing orders. 5 new unit tests (2 snapshot, 3 logger serialization). 59 unit + 17 integration tests total.
- [x] **Step 7b: Orderbook Fix + Dry-Run Verified** — Fixed critical orderbook parsing bug: CLOB WS sorts bids ascending and asks descending, so `.first()` returned worst prices ($0.01/$0.99) instead of best. Changed to `.last()` — one-line fix per side. The WS book is already complement-merged (includes both native and CTF-derived orders). Added complement safety net in main.rs (`min(native_ask, 1 - complement_bid)`). Fixed discovery to map token IDs to outcomes via `market.outcomes` array. Reordered `Opportunity` fields for readability (up_ask/down_ask right after combined_ask). Enriched Telegram trade alerts with full breakdown (buy up/down, fees, combined before/after). Added `scripts/dry-run-test.sh` for timed dry-run testing. **Live dry-run verified: 25 arbs detected in 3 minutes across BTC/ETH/XRP, tick-to-trade ~115μs, all 4 exchanges fresh.**
- [x] **Step 7c: Pre-Live Safety Guards** — Max leg price filter (`max_leg_price = 0.90` in config) rejects arbs where either side exceeds threshold, preventing accidental directional exposure from lopsided arbs like up=$0.06/down=$0.93. Partial fill kill switch in main loop: if one FOK fills and the other is killed, immediately halts all trading via `risk_manager.kill()` + Telegram alert — operator must review stranded position and `/resume` manually. 2 new unit tests (leg price filter). 61 unit + 17 integration tests total.
- [x] **Step 7d: Live Execution Fix** — Fixed critical order response handling: `success: true` only means the CLOB accepted the order, not that it filled. FOK orders get accepted then immediately canceled if no liquidity at exact price. Now checks `status == Matched` for true fills. Distinguishes expected (Canceled/Unmatched = book moved, FOK killed, normal) from unexpected (Live/Delayed = order resting or queued, shouldn't happen for FOK). Full response logging with status, making_amount, taking_amount. DryRunExecutor now mirrors LiveExecutor share math for verification. Documented execution semantics in README (order flow, status table, outcome matrix, safety guards).
- [ ] **Step 8: Executable Arb Strategy** — Depth + notional checks to turn theoretical arbs into actually executable ones. Four sub-steps:
  - [x] **8a: Min-notional filter** — `ceil(1/up_ask)` and `ceil(1/down_ask)` gives minimum shares needed to clear CLOB's $1/order minimum. Skip arb if `floor(budget/combined_cost) < max(min_shares_up, min_shares_down)`. Pure math, no new data. Fixes the "$1 minimum" CLOB rejections.
  - [x] **8b: Orderbook depth propagation** — `BestBidAsk` currently only carries price, no size. Extend to pass ask-side levels `Vec<(price, size)>` from CLOB WS through `FeedEvent` → `MarketSnapshot`. Infrastructure change that unlocks 8c and 8d.
  - [x] **8c: Executable size check** — `executable_up/down` = available shares at best ask. `size_cap = min(executable_up, executable_down)`. `size_min = max(min_shares_up, min_shares_down)`. Skip if `size_cap < size_min`. Prevents sending FOK into thin books.
  - [ ] **8d: VWAP arb preservation** — At `size_cap`, compute VWAP per leg across depth levels. Verify `vwap_up(size_cap) + vwap_down(size_cap) < 1 − fees − buffer`. Ensures scaling into depth doesn't erode the edge.
- [ ] Step 9: EOA implementation
- [ ] Step 10: Deployment prep + production

**Deferred (data-driven, revisit after dry-run):**
- Volatility tracker — rolling window, dynamic thresholds. Premature without real rejection data.
- Multi-market prioritization — only 4 assets, all processed every tick. No bottleneck to solve yet.

### CLOB Constraints (discovered in live testing)
- **$1 minimum order size (FOK takers)**: Each individual FOK taker order must cost ≥ $1 (`price × shares ≥ $1`). Orders below this are rejected with "invalid amount for a marketable BUY order, min size: $1". Empirically validated by `live_probe_min_order_size` integration test across all 4 assets (BTC, ETH, SOL, XRP): 1, 3, and 10 shares at $0.01 → rejected; 100 shares at $0.01 → FOK-killed (sub-$1 reached engine). Docs: https://docs.polymarket.com/developers/CLOB/orders/create-order (INVALID_ORDER_MIN_SIZE error).
- **`min_order_size` in orderbook (maker orders only)**: The `min_order_size` field returned by the CLOB orderbook API (value: `5` for all 15-min crypto markets) is the minimum for **maker/resting limit orders** (GTC/GTD). It does **not** apply to FOK taker orders. Our bot only places FOK orders — this field is irrelevant to us.
- **Startup validation**: In live mode, `verify_clob_minimum()` posts a guaranteed-sub-$1 probe order at startup and parses the "min size: $N" rejection. Aborts with a clear error if the exchange minimum doesn't match `config.min_order_usdc` — catches exchange rule changes before the first real order.
- **Maker amount precision**: Market buy orders require maker_amount (USDC = price × shares) to have ≤ 2 decimal places. Integer shares with 2dp prices guarantee this. Fractional shares like 3.03 at $0.45 = $1.3635 (4dp) → rejected.
- **Batch endpoint (`POST /orders`) doesn't work for FOK**: Returns empty status/order_id. Individual `POST /order` returns proper responses. Use `tokio::join!` for concurrent individual posts.
- **FOK rejection is an error, not a status**: `POST /order` with FOK returns `Err("order couldn't be fully filled. FOK orders are fully filled or killed.")`, not `Ok(response)` with status Canceled.

### Defensive Sizing Strategy (future consideration)
Current approach sizes by budget: `units = floor(budget / combined_cost)`. This maximizes capital deployed per arb but also maximizes stranded exposure on partial fill.

**Alternative: minimum-exposure sizing.** Instead of "how much can I buy?", ask "what's the fewest units where the cheap leg clears $1?":
- `units = ceil($1 / min(up_ask, down_ask))`
- Then verify `max(up_ask, down_ask) × units` doesn't exceed acceptable stranded exposure

Example: up=$0.37, down=$0.60
- Budget sizing (current): units = floor($3 / $0.9854) = 3, cheap leg = $1.11, expensive leg = $1.80
- Minimum-exposure sizing: units = ceil($1 / $0.37) = 3, same result — can't go lower

For lopsided arbs the expensive leg grows fast:
- up=$0.10, down=$0.85: units = ceil($1/$0.10) = 10, expensive leg = $8.50 stranded exposure
- This is already filtered by max_leg_price=0.90, but even within range, more lopsided = more risk

Full defensive approach: `units = ceil($1 / min_leg)`, then reject if `max_leg × units > max_stranded_exposure`. Needs real partial fill data to calibrate the threshold.

### Feed Strategy
For pure arb (buy both Up + Down < $1.00), the arb is real if both orders fill — exchange
prices don't determine arb validity. However, exchanges predict **leg risk**: if prices are
moving rapidly, resting asks on Polymarket are likely to be canceled before we can fill both
sides. Exchange agreement = stable book = safe to attempt both fills.

**Exchange tier (leg-risk assessment):**
- Binance WS (direct bookTicker) — highest volume, primary price leader
- Coinbase WS — US-regulated, frequently in Chainlink aggregations
- OKX WS — high volume on major pairs, leads Asian hours
- Bybit WS — leads during volatility, key for Asian hours

If exchanges agree (divergence <0.5%), the underlying price is stable and Polymarket
asks are unlikely to be pulled. If they diverge, the book is moving — skip or require
larger edge / smaller size.

**Resolution tier (oracle tracking):**
- Chainlink Data Streams via RTDS — the actual resolution oracle for Polymarket 15-min markets.
  Not essential for pure arb (we profit regardless of outcome), but useful for:
  detecting danger-zone moments near strike, diagnosing abnormal oracle behavior, and
  future directional strategies. Currently via Polymarket RTDS relay (~1-1.5s latency).
  Sponsored Chainlink API key application pending for direct access (lower latency).

**Pending:**
- Sponsored Chainlink API key — apply at https://pm-ds-request.streams.chain.link/

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
| Price feeds | Binance + Coinbase + OKX + Bybit (direct WS) | Leg-risk assessment: exchange agreement = stable book = safe to fill both sides |
| Resolution source | Chainlink Data Streams (via RTDS) | Oracle tracking — danger-zone detection, abnormal behavior diagnosis |
| Source tiering | Exchange tier (fast) vs Resolution tier (slow) | Exchanges assess leg risk; Chainlink tracks resolution separately |

## Deployment
- **CLOB location**: AWS **eu-west-2 (London)** primary, eu-west-1 (Ireland) backup.
  Confirmed in Polymarket's own docs: https://docs.polymarket.com/developers/CLOB/geoblock.
  clob.polymarket.com (104.18.x.x) is Cloudflare anycast — it terminates TLS at the nearest
  edge PoP but proxies all requests to the London backend. Do NOT deploy in US East — a
  transatlantic hop to London adds ~80-100ms on top of local edge latency.
  Note: Polymarket's own servers run in the UK (eu-west-2), which is itself a geoblocked
  jurisdiction for end-users. The geoblock is applied at the application layer (user IP),
  not to the infrastructure — the servers have no nationality.
- **VPS location — options ranked (measured RTT to CLOB):**
  UK is geoblocked so a London VPS IP will have orders rejected despite being the same DC.
  AWS eu-west-1→eu-west-2 RTT is **~11ms** on the private AWS backbone (never public internet).
  Amsterdam via AMS-IX peering is **~8-12ms** on the public internet — marginally faster
  in absolute RTT but more jitter. Practical difference is 1-3ms; negligible vs execution overhead.
  | Location              | RTT to CLOB   | Path          | Geoblock | Notes                                          |
  |-----------------------|---------------|---------------|----------|------------------------------------------------|
  | London (eu-west-2)    | ~1-5ms        | —             | **YES**  | UK IP = orders rejected                        |
  | Amsterdam, NL         | ~8-12ms       | Public/AMS-IX | **YES (auth)** | Blocked for authenticated trading — confirmed empirically. Public endpoints (REST reads, latency bench) work fine, but live order placement returns "geo-blocked: trading not available from NL". Not listed in official docs — docs are outdated. |
  | Paris / Roubaix, FR   | ~10-14ms      | Public        | No       | OVH HQ, excellent EU peering                  |
  | Dublin (eu-west-1)    | **~11ms**     | AWS backbone  | No       | Measured; private fiber, low jitter; Polymarket's endorsed region |
  | Frankfurt (eu-central-1) | ~15-20ms  | Public        | No       | Hetzner closest; also Germany has regulatory risk for prediction markets |
- **AWS eu-west-1 vs Amsterdam VPS — key findings:**
  - AWS private backbone Dublin→London: **~11ms avg, ~11-13ms range** (measured). Stays off
    public internet entirely — lower jitter than any VPS routing.
  - Amsterdam AMS-IX→London: **~8-12ms** (measured). Can be 1-3ms faster in absolute RTT
    but subject to internet routing variability.
  - "Sub-1ms" claims from trading VPS providers measure RTT to the nearest **Cloudflare
    edge PoP** (TLS termination), not the CLOB backend in London. Treat as marketing.
  - For a parity arb bot where end-to-end execution is ~10-50ms, a 2ms RTT difference
    does not change outcomes. **Consistency (AWS) vs marginally lower floor (Amsterdam).**
- **VPS providers researched:**
  | Provider | Location | Price | Notes |
  |---|---|---|---|
  | [TradingVPS](https://tradingvps.io/) | Amsterdam | ~$19/mo | AMD Ryzen 9950X + NVMe; trading-focused peering; best price for a specialized option |
  | [QuantVPS](https://www.quantvps.com/polymarket-vps) | Amsterdam | ~$60/mo | Polymarket-specific colocation; colocated near AMS-IX |
  | [Vultr](https://www.vultr.com/) | Amsterdam | $6–24/mo | Good general cloud; best for initial benchmarking before committing |
  | [DigitalOcean](https://www.digitalocean.com/) | Amsterdam | $12–24/mo | Reliable, simple ops |
  | [Linode/Akamai](https://www.linode.com/) | Amsterdam | $12–20/mo | Strong EU network |
  | AWS EC2 t3.small | eu-west-1 Dublin | ~$15/mo | Private backbone; Polymarket's endorsed region; t3.micro ($7.60) works too |
  | [OVH](https://www.ovhcloud.com/) | Roubaix, FR | $6–50/mo | Near NL/BE border; excellent peering; bare metal options |
  | [Contabo](https://contabo.com/) | Amsterdam | $6–15/mo | Cheapest but network quality lower — avoid for latency work |
  Hetzner does NOT have an Amsterdam DC — Frankfurt/Nuremberg only (~15-20ms to CLOB).
- **Benchmark results (Vultr HF Amsterdam, x86_64-unknown-linux-musl binary, 3 runs):**
  | Run | CLOB REST p50 | CLOB REST p95 | CLOB WS first-msg | Notes |
  |---|---|---|---|---|
  | Run 1 | 26ms | 34ms | 118ms | Quiet market (6512ms WS gap p50) |
  | Run 2 | 24ms | 34ms | 106ms | Active market (35ms WS gap p50) |
  | Run 3 (same-time as local) | 24ms | 27ms | 113ms | Side-by-side comparison |
  | **Local macOS (same-time)** | **58ms** | **66ms** | **362ms** | 2.4× slower than VPS |

  Same-time run confirmed CLOB WS inter-message gaps are identical from both locations
  (Vultr: 7437ms, local: 7435ms) — gaps are market activity, not network latency.
  Exchange connect times all faster from Vultr (closer to exchange endpoints).

- **Amsterdam geo-block finding:** Netherlands is blocked for **authenticated trading** despite
  not appearing on the official Polymarket geoblock docs. Public endpoints (orderbook reads,
  latency bench) work fine from NL. Live mode immediately returns:
  `geo-blocked: trading not available from NL (x.x.x.x)`.
  This rules out: Vultr Amsterdam, TradingVPS Amsterdam, DigitalOcean Amsterdam, any NL VPS.

- **Recommended path (updated):**
  1. AWS eu-west-1 (Dublin, Ireland) — Polymarket's own endorsed region, private backbone
     to London (~11ms), Ireland confirmed not geo-blocked. Resolve AWS new-account region
     restrictions to launch t3.small in eu-west-1.
  2. Alternative: find a Dublin/Ireland VPS provider (Linode/Akamai has Dublin DC).
  3. Full research and reasoning: see `VPS_RESEARCH.md` (local, untracked).
- **Measured baselines (local macOS, for comparison)**: CLOB REST p50=63ms, p95=102ms.
  CLOB WS first-msg ~400ms (one-time lazy connect + snapshot), inter-message gaps p50=107ms.
  Exchange clock deltas: Binance no-ts (bookTicker), Coinbase -38ms skew, OKX +133ms, Bybit -8ms.
- **Industry latency benchmarks vs our bot — competitive analysis:**
  Parity arb opportunities (both sides < $1) are visible to all bots subscribed to the same
  CLOB WS stream. First to submit both FOK orders wins; others hit a moved book. End-to-end
  tick-to-trade latency determines the outcome of that race.

  *Opportunity window (researched):*
  | Arb type | Avg window (2025) | Down from (2024) |
  |---|---|---|
  | All arb types combined | ~2.7 seconds | ~12.3 seconds |
  | Simple parity / complement arb | ~4 seconds | — |
  | 73% of all arb profits captured by | sub-100ms bots | — |

  *Our end-to-end tick-to-trade breakdown:*
  | Component | Local macOS (measured) | Vultr Amsterdam (measured) |
  |---|---|---|
  | CLOB REST p50 (order submission) | ~58-63ms | **~24-26ms** |
  | CLOB REST p95 | ~66-102ms | **~27-34ms** |
  | CLOB WS first-msg (connect + snapshot) | ~362ms | **~106-118ms** |
  | CLOB WS inter-msg gap | market-driven | identical to local (confirmed same-time) |
  | Strategy processing (arb detect + risk) | ~0.1ms | ~0.1ms (Rust, ~115μs) |
  | **Total tick→trade (estimated)** | **~170ms** | **~30-40ms** |

  *Geographic latency floor — the key constraint for all competitors:*
  The UK geoblock means NO bot can legally be closer than ~8-11ms to the CLOB. This
  creates a hard floor that applies equally to all competitors. Bots are effectively
  clustered into two tiers:
  | Tier | Location | RTT floor | Notes |
  |---|---|---|---|
  | Best possible | Amsterdam / Dublin | ~8-11ms | The competitive cluster; where serious bots run |
  | Mid-tier | Frankfurt | ~15-20ms | Structural disadvantage vs Amsterdam cluster |
  | Uncompetitive | Home / local | ~63ms+ | Too slow; other bots at 20-40ms will consistently win |

  *Our position:*
  - **Local macOS (~170ms)**: well inside the 4s opportunity window in absolute terms, but
    other bots at Amsterdam (~20-40ms) will consistently beat us to the fill. Not viable.
  - **VPS Amsterdam/Dublin (~20-40ms)**: inside the competitive cluster. We're racing bots
    at the same geographic tier. The differentiator at that point is **processing speed**:
    our Rust bot at ~115μs strategy processing is faster than Python/JS bots (~1-5ms),
    giving a ~1-5ms edge when every millisecond at the same network distance counts.
  - **The geoblock is a leveler**: nobody in the competitive cluster has more than ~3-4ms
    network advantage over anyone else at Amsterdam. Rust processing speed and low-jitter
    VPS network become the actual differentiators.
- **Private RPC**: Required for Phase 2 (Alchemy/QuickNode EU, ~$50/month). Non-negotiable
  for both speed and reliability of order submission.
- **Geo-compliance**: Ireland (AWS eu-west-1) confirmed not geo-blocked. Netherlands is **blocked for authenticated trading** — empirically confirmed (live mode returns geo-block error from NL IP despite not being on the official docs list). France is on the official geoblock list. Germany is on the official geoblock list. Geoblock check runs at bot startup.
- **Full VPS research**: see `VPS_RESEARCH.md` (local, untracked).

## Risk Register
| Risk | Mitigation |
|------|------------|
| Fee modeling errors | Precise fee calc with unit tests against known values |
| Arb edges too competitive | Validate with dry-run before risking capital |
| API rate limits | Respect CLOB rate limits, implement backoff |
| Chainlink latency | Monitor Binance as leading indicator; get sponsored Chainlink key |
| MEV on Polygon | Use private relay / Flashbots-equivalent for order submission (research needed) |
| Edge window is ms not s | Optimize every hop; private RPC; benchmark real pings before going live |
