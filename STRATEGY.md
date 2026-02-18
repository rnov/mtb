# MTB Strategy Reference

Operational reference for the parity arbitrage strategy. Read this before changing config values, adding filters, or modifying execution logic.

---

## What It Does

Buys the UP token and DOWN token of a Polymarket 15-minute crypto market simultaneously when their combined ask price (after fees) is below $1.00. Since one side always settles at $1 and the other at $0, a pair bought for $0.96 guarantees $0.04 profit per unit regardless of market direction.

---

## Full Strategy Logic — Step by Step

### 1. Market Discovery (Gamma API, every 60s)

Poll Gamma for active 15-minute parity markets for configured assets (BTC, ETH, SOL, XRP). Each market has an UP token and a DOWN token. Subscribes to live orderbook data for every token found.

Config: `discovery.poll_interval_secs`, `discovery.assets`

---

### 2. Price Feed (CLOB WebSocket, real-time)

For every subscribed token, the CLOB WS streams live orderbook updates:
- `best_ask` — cheapest ask price
- `ask_levels` — up to 10 price levels (price, size), sorted cheapest first

These flow into a per-token cache (`best_ask_map`, `ask_levels_map`) in the main event loop.

---

### 3. Freshness Check (per orderbook tick)

Before running arb logic, the underlying asset price must be confirmed by multiple exchanges:

| Check | Config | Purpose |
|-------|--------|---------|
| `min_fresh_sources = 2` | `strategy.min_fresh_sources` | At least 2 exchanges must have a recent price. Guards against relying on a single feed. |
| `max_price_age_ms = 2000` | `strategy.max_price_age_ms` | Exchange prices older than 2s are stale — book is likely moving. |
| `max_divergence_pct = 0.5%` | `strategy.max_divergence_pct` | If exchanges disagree by >0.5%, the underlying is moving fast — high leg risk. |

Exchanges used: Binance, Coinbase, OKX, Bybit (direct WebSocket, exchange tier).
Chainlink (RTDS) is tracked separately as a resolution source — not used for freshness decisions due to ~1.2s latency.

---

### 4. Arb Detection — 5 Filters

All in `src/strategy/arb.rs`, executed in order on every orderbook tick.

#### Filter 1 — Price sanity
Both `up_ask` and `down_ask` must be in `(0, 1)`. Zero or one is nonsensical for a parity token.

#### Filter 2 — Max leg price
```
if up_ask > max_leg_price || down_ask > max_leg_price → skip
```
Caps directional exposure on partial fill. If one side fails to fill and you're holding a $0.90 token, you have a near-certain directional bet. Keeping both legs below the cap limits the worst-case stranded position.

Config: `strategy.max_leg_price`
Use `scripts/exposure_calculator.py` to calibrate (see Config Calibration below).

#### Filter 3 — Minimum profit
```
profit_per_unit = 1.0 − (up_ask + down_ask + fees)
skip if profit_per_unit < min_arb_profit
```
Fee model: `fee = min(price, 1 − price) × 2%` per leg (Polymarket taker rate, symmetric around 0.5).

Config: `strategy.min_arb_profit` (default 0.5%)

#### Filter 4 — Min notional (Step 8a)
The CLOB rejects any individual order below `min_order_usdc` ($1.00). Minimum shares per leg:
```
min_shares_up   = ceil(min_order_usdc / up_ask)
min_shares_down = ceil(min_order_usdc / down_ask)
size_min        = max(min_shares_up, min_shares_down)
budget_units    = floor(max_order_size_usdc / combined_after_fees)

skip if budget_units < size_min
```
Example: `up=$0.30` → need ≥ 4 units (4 × $0.30 = $1.20). With `budget=$3` and combined ~$0.96 → `budget_units=3` < `size_min=4` → skip.

Config: `strategy.min_order_usdc`, `strategy.max_order_size_usdc`

#### Filter 5 — Executable size (Step 8c)
The ask price might exist with only 1 share available. Check real book depth:
```
executable_up   = shares at first ask level (UP)
executable_down = shares at first ask level (DOWN)
size_cap        = floor(min(executable_up, executable_down))

skip if size_cap < size_min
units = min(budget_units, size_cap)
```
Prevents FOK orders into books too thin to fill.

---

### 5. Order Sizing

```
budget_units        = floor(max_order_size_usdc / combined_after_fees)
units               = min(budget_units, size_cap)
suggested_size_usdc = units × combined_after_fees
expected_profit     = units × profit_per_unit
```

Both sides always use the **same unit count** — every UP token is paired with a DOWN token.

---

### 6. Execution — Concurrent FOK Orders

Two `POST /order` calls fired simultaneously via `tokio::join!`:
- Buy `units` UP @ `up_ask` (FOK)
- Buy `units` DOWN @ `down_ask` (FOK)

FOK = fill-or-kill. No partial fills at the CLOB level per order. The two orders are **not atomic** — one can fill while the other is killed.

| Up | Down | Outcome |
|----|------|---------|
| Matched | Matched | Arb filled — guaranteed profit at resolution |
| Matched | Not matched | **Partial fill** — kill switch triggers, halt trading |
| Not matched | Matched | **Partial fill** — kill switch triggers, halt trading |
| Not matched | Not matched | No fill — normal, no cost, continue |

FOK kills (both sides) do **not** count toward the consecutive failure kill switch — this is normal market behavior with zero cost.

---

### 7. Risk Controls

| Guard | Config | Triggers when |
|-------|--------|--------------|
| Partial fill | — | One side fills, other killed — immediate halt |
| Consecutive failures | `risk.max_consecutive_failures` | N real errors (not FOK kills) in a row |
| Daily loss | `risk.max_daily_loss_usdc` | Cumulative loss exceeds limit |
| Exposure cap | `risk.max_exposure_usdc` | Total open exposure hits ceiling |
| Feed disconnect | `risk.feed_disconnect_timeout_secs` | No orderbook WS data for N seconds |

Kill switch halts trading but not the process. Resume via Telegram `/resume` or restart.

---

## Worked Example

`up_ask = 0.47`, `down_ask = 0.49`, book depth 100 shares each, `max_order_size_usdc = $3`, `max_leg_price = 0.50`:

```
1. Sanity: 0.47 and 0.49 both in (0, 1) ✓
2. Leg cap: 0.47 ≤ 0.50 and 0.49 ≤ 0.50 ✓
3. Profit: 1 − (0.47 + 0.49 + 0.0094 + 0.0098) = 1 − 0.9792 = 0.0208 → 2.08% > 0.5% ✓
4. Min notional:
     min_shares_up   = ceil(1/0.47) = 3  →  3 × $0.47 = $1.41 ✓
     min_shares_down = ceil(1/0.49) = 3  →  3 × $0.49 = $1.47 ✓
     size_min = 3
     budget_units = floor($3 / 0.9792) = 3  →  3 ≥ 3 ✓
5. Depth: size_cap = min(100, 100) = 100 ≥ 3 ✓
6. Units: min(3, 100) = 3
7. Fire: buy 3 UP @ 0.47 + buy 3 DOWN @ 0.49 via tokio::join!
8. Both FOK fill → profit = 3 × 0.0208 = $0.0624
```

---

## Config Calibration

### Exposure Calculator

Before changing `max_leg_price` or `max_order_size_usdc`, run:

```bash
python3 scripts/exposure_calculator.py [max_order_size_usdc] [max_leg_price]

# Examples:
python3 scripts/exposure_calculator.py 3.0 0.50   # current recommended
python3 scripts/exposure_calculator.py 5.0 0.60   # higher budget/cap
```

The table shows for every cheap-leg price (with expensive leg fixed at `max_leg_price`):
- `MinUnits` — minimum shares to clear CLOB $1/order floor
- `BudgetUnits` — max shares the budget allows
- `MinExposure` / `MaxExposure` — worst-case directional cost on partial fill
- `FILTERED` — 8a would reject (budget too small for this price combination)

### Key relationships

| `max_leg_price` | OK range starts at | Max partial fill exposure |
|---|---|---|
| 0.50 | cheap ≥ $0.34 | $1.50 (3 units × $0.50) |
| 0.60 | cheap ≥ $0.34 | $1.80 (3 units × $0.60) |
| 0.90 | cheap ≥ $0.34 | $2.70 (3 units × $0.90) |

Rule of thumb: **`max_order_size_usdc` = 3 is well-matched for `max_leg_price` ≤ 0.50**. For higher leg caps, recalculate with the script.

---

## Decision Log

Key choices made during development, with rationale. Update this when strategy decisions change.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Order type | FOK (fill-or-kill) | No open orders to manage, no partial fills per order |
| Concurrency | `tokio::join!` on two `POST /order` | Batch `POST /orders` returns empty status for FOK — unusable |
| Share count | Integer only (`floor()`) | CLOB rejects if `price × shares` has >2 decimal places |
| VWAP check (Step 8d) | **Intentionally skipped** | With strict best-ask FOK, VWAP = best_ask always. 8d only adds value if we walk up the book (multi-level fills). We don't. |
| Max leg price | 0.50 (recommended) | Caps partial fill exposure at 1.5× the per-leg cost. 0.90 allows lopsided pairs (e.g. $0.06/$0.93) with unacceptable directional risk. |
| Exchange tier vs resolution tier | Separate | Binance/Coinbase/OKX/Bybit assess leg risk (fast, <32ms). Chainlink tracks resolution separately (~1.2s via RTDS). Different use cases, different latency profiles. |
| FOK kills and failure counter | FOK kills excluded | Both-sided FOK kill = zero cost, normal behavior. Counting it toward `max_consecutive_failures` would trigger spurious kill switch on active markets. |
| Partial fill response | Immediate kill switch | One side filled = stranded directional position. Operator must review manually before resuming. |
