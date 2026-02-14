# Research - Polymarket Trading Bot

mtb stands for my trading bot. This project is about building infrastructure to automate
trading on Polymarket, specifically targeting 15-minute crypto up/down markets (BTC/ETH/SOL).

Starting point: https://x.com/marlowxbt/status/2021995081771667915?s=12

## Verified Resources

### Official SDK (Use This)
- **Polymarket/rs-clob-client** — https://github.com/Polymarket/rs-clob-client
  - Official Rust CLOB client, 500 stars, MIT license, actively maintained
  - Features: typed CLOB requests, dual auth flows, WebSocket streaming, order builders, market discovery, position tracking, async-first (reqwest), alloy::signers support (including AWS KMS)
  - Last pushed: 2026-02-10 (stable, mostly dependency bumps)
  - **This is our foundation — no need to fork community repos**

### Community Reference (Read-Only, Do Not Fork)
- **0xalberto/polymarket-arbitrage-bot** — https://github.com/0xalberto/polymarket-arbitrage-bot
  - 64 stars, 20 forks. Single-market and multi-market arb detection
  - Useful for understanding arb logic patterns, but not Rust-based
  - Use as reference only

### Repos to Avoid
- `franko74/polymarket-arbitrage-bot-btc-eth-15m` — SEO-loaded tags, 1 fork, possible scam/honeypot
- `crellOS/polymarket-arbitrage-bot-btc-15m` — Suspicious fork-to-star ratio (83 stars, 42 forks), likely scam
- `pio-ne-er/RUST-Polymarket-Trading-Bot-Set` — Does not exist (LLM fabrication)
- `cakaroni` — GitHub user does not exist (LLM fabrication)

## Strategy: 15-Minute Crypto Arb

### Core Concept
- Polymarket offers 15-min prediction markets: "Will BTC go up in the next 15 minutes?" YES/NO
- Each market resolves to $1 (YES) or $0 (NO)
- Arb opportunity: when `best_yes_ask + best_no_ask < 1.0` (after fees), buying both locks profit
- Can also trade directionally with external signal confirmation (Binance/Bybit price feeds)

### Fee Structure
- Dynamic taker fees, ~3.15% near 50/50 pricing
- Fees eat most small edges — must model precisely, not approximately
- Need to account for: taker fees on both sides, slippage, gas costs on Polygon

### Competition Reality
- These edges are extremely competitive in Feb 2026
- Sub-millisecond windows dominated by co-located bots
- Latency is critical: aim <150ms end-to-end, deploy on low-latency VPS (Hetzner/Vultr Frankfurt)
- Claims of "$700+ days" are unverified Twitter noise — validate with dry-run data first

## Technical Stack

### Core Dependencies
- **tokio** — async runtime
- **rs-clob-client** — Polymarket CLOB interaction
- **tokio-tungstenite** — WebSocket connections (CLOB + external feeds)
- **alloy** — Polygon transaction signing
- **reqwest** — HTTP client
- **serde/serde_json** — serialization

### Production Dependencies
- **tracing** — structured logging
- **teloxide** — Telegram bot for alerts
- **prometheus** — metrics export
- **toml** — configuration

## Configuration Requirements
- **Wallet**: Private key via env var (never committed)
- **RPC**: Private Polygon RPC (Alchemy/QuickNode)
- **Markets**: Token IDs / condition IDs for target 15-min markets
- **Thresholds**: Min edge after fees, max position size, fee buffer
- **Rate limits**: Polymarket CLOB API has rate limits — must respect them

## Open Research Items
- [ ] Exact Polymarket CLOB rate limits
- [ ] MEV protection options on Polygon (Flashbots Protect does NOT work same as on Ethereum)
- [ ] Optimal VPS location for Polygon latency
- [ ] Historical 15-min market data for backtesting edge validation
- [ ] rs-clob-client API surface deep dive
