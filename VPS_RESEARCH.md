# VPS Research — Polymarket CLOB Deployment

> Research date: 2026-02-19
> Context: Deploying mtb (parity arb bot) close to Polymarket's CLOB backend.

---

## Key Constraint

**The UK is on Polymarket's geoblock list.** The CLOB backend runs on AWS eu-west-2
(London), but the VPS IP must NOT be a UK IP — orders from blocked IPs are rejected at
the application layer. So "closest to London" means closest non-UK location.

Reference: https://docs.polymarket.com/developers/CLOB/geoblock

---

## Competitive Context — Why Latency Matters for Parity Arb

Parity arb (up_ask + down_ask < $1.00) is visible to every bot subscribed to the same
CLOB WS stream. When a mispricing appears, it's a race: the first bot to submit both
FOK orders wins the fill; every subsequent bot hits a moved book.

The arb signal comes entirely from the Polymarket orderbook — exchange prices (Binance,
Coinbase, OKX, Bybit) are used only to assess whether the book is stable enough to
attempt both fills (freshness guard). They do not determine the arb itself.

### Opportunity window (researched)

| Arb type | Avg window 2025 | Down from 2024 | Notes |
|---|---|---|---|
| All arb types combined | ~2.7 seconds | ~12.3 seconds | Compressed as more bots entered |
| Simple parity / complement arb | ~4 seconds | — | Window before book self-corrects |
| Sub-100ms bots | capture 73% of all arb profits | — | Speed is the primary differentiator |

A 4-second window sounds wide but the race is decided in the first ~50ms — bots at
Amsterdam/Dublin executing in 20-40ms consistently beat bots at 100ms+.

### Geographic latency floor — the geoblock as leveler

The UK geoblock means **no bot can legally be closer than ~8-11ms to the CLOB**. This
creates a hard latency floor that applies equally to all competitors. The competitive
landscape clusters into tiers:

| Tier | Location | RTT floor | Competitive status |
|---|---|---|---|
| Best possible | Amsterdam, NL | ~8-12ms | Primary competitive cluster |
| Near-equal | Dublin, IE (AWS eu-west-1) | ~11ms | Primary competitive cluster |
| Mid-tier | Frankfurt, DE | ~15-20ms | Structural disadvantage |
| Uncompetitive | Home / local connection | ~63ms+ | Loses consistently to VPS bots |

**The geoblock is a leveler**: nobody in the Amsterdam/Dublin cluster has more than
~3-4ms network advantage over anyone else. Within that cluster, processing speed and
low-jitter network become the actual differentiators.

### Our tick-to-trade breakdown

| Component | Local macOS (measured) | Amsterdam/Dublin VPS (estimated) |
|---|---|---|
| CLOB WS tick reception (inter-msg p50) | ~107ms | ~10-20ms |
| Strategy processing (arb + risk check) | ~0.1ms | ~0.1ms (Rust, ~115μs measured) |
| 2× POST /order concurrent (tokio::join!) | ~63ms (CLOB REST p50) | ~10-18ms |
| **Total tick → trade** | **~170ms** | **~20-40ms** |

- **Local macOS (~170ms)**: Inside the 4s opportunity window in absolute terms, but other
  bots at Amsterdam (~20-40ms) win the race consistently. Not viable for production.
- **Amsterdam/Dublin VPS (~20-40ms)**: Inside the competitive cluster. Racing bots at
  the same geographic tier. Differentiator becomes processing speed.
- **Rust processing advantage**: Our ~115μs strategy processing vs Python/JS bots at
  ~1-5ms gives a 1-5ms edge when all bots are at the same network distance. Meaningful
  when total execution is ~20-40ms.

---

## AWS eu-west-1 (Dublin) vs Amsterdam VPS — Direct Comparison

### Measured latency: AWS eu-west-1 → eu-west-2 (private backbone)

AWS inter-region traffic never touches the public internet — it stays on the AWS private
fiber backbone. Measured RTT between eu-west-1 (Ireland) and eu-west-2 (London):

| Metric | ms |
|---|---|
| Average | **~11ms** |
| Minimum | ~11ms |
| Maximum | ~13ms |

Source: [AWS cross-region ping measurements](https://blog-en.richardimaoka.net/network-latency-analysis-with-ping-aws-cross-region)

Your bot on EC2 in Dublin sees ~11ms RTT to the CLOB backend in London, over a private,
consistent, low-jitter path.

### Amsterdam VPS → London (public internet via AMS-IX)

Amsterdam hosts **AMS-IX**, one of the world's largest internet exchange points. Direct
fiber links to London are standard at AMS-IX. Measured RTT from Amsterdam to London over
well-peered providers: **~8-12ms**.

The best-placed Amsterdam VPS (colocated at AMS-IX) can be **marginally faster** than
AWS eu-west-1, but it's on the public internet and therefore has more jitter.

### Side-by-side

| Option | RTT to CLOB | Path | Jitter | Cost |
|---|---|---|---|---|
| AWS eu-west-1 (Dublin) | ~11ms | Private AWS backbone | Very low | $7.60–$30/mo |
| Amsterdam VPS (trading-focused) | ~8-10ms | Public internet via AMS-IX | Low-medium | $19–$60/mo |
| Amsterdam VPS (generic cloud) | ~8-12ms | Public internet via AMS-IX | Low-medium | $6–$24/mo |
| Frankfurt VPS (Hetzner) | ~15-20ms | Public internet | Medium | $6–$20/mo |

**Verdict: Amsterdam is marginally faster in absolute RTT (~1-3ms); AWS is more
consistent (private backbone). For a bot executing in ~20-40ms total, this difference
is small. Benchmark both — the real-world p50 from `latency_bench` decides.**

### On "sub-1ms" claims

Trading VPS providers (QuantVPS, TradingVPS, NYC Servers) advertise "0-1ms" or "sub-1ms"
latency to Polymarket. This measures RTT to the nearest **Cloudflare edge PoP** (which
terminates TLS), not to the CLOB matching engine in London. Backend RTT will always be
~8-12ms from Amsterdam. Treat as marketing.

---

## Best Locations, Ranked by Measured RTT to CLOB

| Location | RTT to CLOB | Geoblock risk | Notes |
|---|---|---|---|
| London, UK | ~1-5ms | **YES — blocked** | Same DC, but UK IP = orders rejected |
| Amsterdam, NL | ~8-12ms | **YES (auth)** | Public endpoints work; authenticated trading blocked. Confirmed empirically — live mode returns "geo-blocked: trading not available from NL". Not on official docs list — docs outdated. |
| Paris / Roubaix, FR | ~10-14ms | **YES — blocked** | France on official geoblock list |
| Dublin, IE (AWS eu-west-1) | ~11ms | No | Private AWS backbone; Polymarket's endorsed region; confirmed viable |
| Frankfurt, DE | ~15-20ms | **YES — blocked** | Germany on official geoblock list |

---

## AWS eu-west-1 — Pros and Cons

### Pros
- **Private backbone**: Dublin → London stays on AWS fiber, never touches public internet.
  Lower jitter than any VPS routing over the public internet.
- **Polymarket's own recommendation**: Explicitly documented as "Closest Non-Georestricted
  Region." Endorsed path for bot operators.
- **Predictable pricing**: t3.micro ($7.60/mo), t3.small ($15.18/mo) in eu-west-1.
- **Managed networking**: No ISP routing, peering agreements, or transit outages to think
  about.

### Cons
- **More complex to set up** than a VPS: IAM, security groups, VPC config vs `apt install`.
- **Not the absolute fastest**: Amsterdam with AMS-IX peering can be ~1-3ms faster.
- **Burstable CPUs**: t3 instances use CPU credits — can throttle under sustained load.
  Use c7g.medium (~$25/mo) for dedicated vCPU if needed.
- **Data transfer costs**: After 100 GB free, egress is ~$0.09/GB — negligible for a bot.

---

## VPS/Cloud Options

### Tier 1 — Purpose-built for trading (Amsterdam)

| Provider | Price | Notes |
|---|---|---|
| [TradingVPS](https://tradingvps.io/) | from ~$19/mo | Amsterdam, AMD Ryzen 9950X + NVMe, trading-focused peering, community-reported good latency |
| [QuantVPS](https://www.quantvps.com/polymarket-vps) | from ~$60/mo | Amsterdam, Polymarket-specific landing page, colocated near AMS-IX |

Most likely to have genuine network peering advantages. TradingVPS at $19/mo is the best
price/performance option in this tier if benchmarks confirm the latency.

### Tier 2 — General cloud, Amsterdam location

| Provider | Price | Notes |
|---|---|---|
| [Vultr Amsterdam](https://www.vultr.com/products/cloud-compute/) | $6–24/mo | Good general-purpose cloud, fast provisioning — best to benchmark first |
| [DigitalOcean Amsterdam](https://www.digitalocean.com/) | $12–24/mo | Reliable, simple ops |
| [Linode/Akamai Amsterdam](https://www.linode.com/) | $12–20/mo | Strong EU network |
| [Contabo Amsterdam](https://contabo.com/) | $6–15/mo | Cheapest, but network quality lower — avoid |

### Tier 3 — AWS (eu-west-1, Ireland)

| Instance | vCPU | RAM | Network | CPU | Price/mo | Burstable |
|---|---|---|---|---|---|---|
| t3.micro | 2 | 1 GB | Up to 5 Gbps | Intel x86 | ~$8.32 | Yes |
| t3.small | 2 | 2 GB | Up to 5 Gbps | Intel x86 | ~$16.64 | Yes |
| t4g.small | 2 | 2 GB | Up to 5 Gbps | Graviton2 ARM | ~$13.43 | Yes |
| **c7g.medium** | **1** | **2 GB** | **Up to 12.5 Gbps** | **Graviton3 ARM** | **~$28.32** | **No — dedicated** |

**On burstable (t3/t4g) vs dedicated (c7g):**
t3/t4g earn CPU credits at idle and spend them bursting. In unlimited mode (t3 default),
throttling doesn't happen but sustained high CPU is billed at ~1.5× the instance rate.
For this bot the risk is effectively zero — the hot path is ~115μs of Rust then waiting
on network; average CPU utilisation is ~1-3% and credits never drain. The real argument
for c7g.medium in production is the **dedicated vCPU** (no noisy-neighbour contention,
fully predictable latency) and the **12.5 Gbps network cap** vs 5 Gbps on t-series.

**Recommended instances:**
- **Benchmarking → t3.small (~$16.64/mo)**: x86, same arch as dev machine — `cargo build`
  and `scp`, done. 2 GB RAM gives comfortable headroom for 7 tokio tasks + TLS stack.
  Spin up on-demand, run `latency_bench`, check CLOB REST p50, terminate if Amsterdam wins.
  Avoid t3.micro (1 GB) — too tight once tokio runtime + reqwest TLS + tracing buffers load.
- **Production → c7g.medium (~$28.32/mo)**: dedicated Graviton3 vCPU, 12.5 Gbps network,
  no credit system, predictable latency under any load. Requires cross-compiling the Rust
  binary for ARM (`cargo build --target aarch64-unknown-linux-musl`) — one-time setup.
  The extra ~$12/mo over t3.small is worth it once trading live with real capital.

---

## Recommendation

**Amsterdam is out.** Netherlands blocks authenticated trading despite not appearing on the
official docs geoblock list. Confirmed empirically on Vultr Amsterdam.

**The only viable nearby option is Dublin, Ireland (AWS eu-west-1).**

### Option A — AWS eu-west-1 t3.small — ~$16/mo ← recommended
- Polymarket's own endorsed region
- Private AWS backbone Dublin→London (~11ms, never touches public internet)
- Ireland confirmed not geo-blocked
- New AWS accounts may have region restrictions — resolve via AWS support or account verification

### Option B — Linode/Akamai Dublin — ~$12/mo
- Dublin datacenter available
- Not geo-blocked (Ireland)
- Alternative if AWS account restrictions can't be resolved quickly
- Public internet path vs AWS private backbone

### Ruled out
| Provider | Location | Why out |
|---|---|---|
| TradingVPS | Amsterdam, NL | NL geo-blocked for authenticated trading |
| Vultr HF | Amsterdam, NL | NL geo-blocked for authenticated trading |
| DigitalOcean | Amsterdam, NL | NL geo-blocked for authenticated trading |
| Hetzner | Frankfurt, DE | Germany on official geoblock list |
| Any FR provider | France | France on official geoblock list |

---

## Vultr Instance Recommendation

### Instance types compared

| Type | vCPU | Clock | Storage | Price | Single-core benchmark |
|---|---|---|---|---|---|
| Cloud Compute | Shared, old gen | ~2.x GHz | SSD | $5–6/mo | 844 events/s |
| **High Frequency** | Shared, Intel | **3.8 GHz** | **NVMe** | **$6/mo** | **1,339 events/s** |
| High Performance AMD | Shared, EPYC | Lower per-core | NVMe | $6/mo | 2,620 events/s (multi-core) |
| Optimized Cloud Compute | Dedicated vCPU | — | NVMe | $30+/mo | — |
| VX1 | Dedicated | — | Block storage | ~$43+/mo | — |

Source: [Vultr VPS benchmark comparison Jan 2025](https://joshtronic.com/2025/01/12/vps-showdown-january-2025-vultr-cpu-comparison/)

### Why High Frequency

The bot's hot path is single-threaded and I/O-bound: receive WS tick → arb detect
(~115μs) → two concurrent POSTs. **Single-core clock speed matters more than multi-core
throughput.** High Frequency at 3.8 GHz Intel + NVMe beats everything at the $6/mo
price point on single-core latency.

- **High Performance AMD**: better multi-core throughput but lower per-core clock — wrong
  profile for this workload.
- **Optimized/Dedicated ($30+)**: no CPU contention, but the bot is network-bound not
  CPU-bound. Overkill for a benchmark test.

### Recommended instance for benchmarking

**Vultr High Frequency — 1 vCPU / 2GB RAM / 64GB NVMe — ~$12/mo ($0.018/hr) — Amsterdam**

- **Why 2GB not 1GB ($6)**: the bot runs ~7 concurrent tokio tasks (CLOB WS, 4 exchange
  feeds, Chainlink, main loop, Telegram) plus `tokio`, `tracing`, `reqwest` TLS stack.
  Estimated RSS in dry-run ~200-400MB. 1GB is too tight — any spike OOMs the process.
- **Why 1 vCPU not 2**: the bot is I/O-bound. A second vCPU adds cost without benefit.
- **Cost to benchmark**: spin up hourly (~$0.018/hr), run `latency_bench`, destroy.
  Total benchmark cost < $0.01.

---

## Benchmark Results (Measured)

Binary: `x86_64-unknown-linux-musl` (static-pie, 24MB). Three runs on Vultr HF Amsterdam.

### CLOB REST (order submission latency — the critical metric)

| Location | p50 | p95 | min | max |
|---|---|---|---|---|
| Local macOS | 58ms | 66ms | 55ms | 144ms |
| Vultr Amsterdam Run 1 | 26ms | 34ms | 22ms | 35ms |
| Vultr Amsterdam Run 2 | 24ms | 34ms | 20ms | 35ms |
| Vultr Amsterdam Run 3 | 24ms | 27ms | 21ms | 31ms |

**Amsterdam = 2.4× faster than local for order submission.**

### Same-time comparison (Run 3 — both machines running simultaneously)

| Source | Vultr Amsterdam | Local macOS |
|---|---|---|
| CLOB REST p50 | **24ms** | 58ms |
| CLOB REST p95 | **27ms** | 66ms |
| CLOB WS first-msg | **113ms** | 362ms |
| CLOB WS gap p50 | 7437ms | 7435ms ← identical |
| Binance gap p50 | 1ms | 0ms |
| Coinbase gap p50 | 247ms | 249ms |
| OKX gap p50 | 137ms | 142ms |
| Bybit gap p50 | 716ms | 604ms |

**Key finding**: CLOB WS inter-message gaps are identical from both locations — gaps are
driven by market activity (order events on the token), not by network distance. The gap
p50 varies run-to-run depending on how active the 15-min market is at that moment.

### Geo-block finding

Live run on Vultr Amsterdam immediately returned:
```
geo-blocked: trading not available from NL
```

Netherlands blocks **authenticated trading** even though it is not listed in the official
Polymarket geoblock docs. Public endpoints (REST orderbook reads, WS subscription) work
fine from NL — the block applies only to authenticated order placement. The official docs
list is outdated. This rules out all Amsterdam/NL VPS options for live trading.

## How to Benchmark

```bash
# scp the static binary to any candidate VPS (no dependencies required)
scp target/x86_64-unknown-linux-musl/release/latency_bench user@<vps-ip>:~/
ssh user@<vps-ip> "chmod +x ~/latency_bench && ~/latency_bench"

# Key output line:
# CLOB REST  p50=XXms  p95=XXms
```

Target: CLOB REST p50 < 30ms. Dublin should land at ~15-25ms.

| Baseline | CLOB REST p50 | Status |
|---|---|---|
| Local macOS (measured) | ~58-63ms | Not viable for production |
| Vultr Amsterdam (measured, 3 runs) | ~24-26ms | ~~Geo-blocked for live trading~~ |
| AWS eu-west-1 Dublin (estimated) | ~15-22ms | Target deployment location |

---

## Cross-Compilation from macOS

The bot needs to be compiled on macOS and shipped as a binary to each test machine.
Two target architectures are required:

| Machine | Arch | Target triple |
|---|---|---|
| Vultr High Frequency + AWS t3.small | x86_64 | `x86_64-unknown-linux-musl` |
| AWS c7g.medium | ARM64 | `aarch64-unknown-linux-musl` |

**Good news**: Vultr HF and t3.small are the same arch — one binary runs on both.
Only c7g.medium needs a separate ARM build.

**musl vs gnu**: We use the `musl` targets (not `gnu`). The gnu cross-container's GCC
has a known memcmp optimization bug ([gcc#95189](https://gcc.gnu.org/bugzilla/show_bug.cgi?id=95189))
that `aws-lc-sys` explicitly rejects at build time. The musl container uses a different
GCC that compiles cleanly. musl also produces **fully static binaries** (no glibc
dependency) — drop-and-run on any x86_64 Linux without worrying about glibc version.

### The complication: `aws-lc-sys` + `openssl-sys`

`rustls` (pulled in by `tokio-tungstenite`, `reqwest`, and `rs-clob-client`) uses
`aws-lc-rs` → `aws-lc-sys` as its default crypto backend. This is a C/C++ library
that requires CMake to build, which rules out `cargo-zigbuild` (known broken with
`aws-lc-sys` from macOS as of early 2025).

Additionally, `native-tls` (pulled in by `rs-clob-client` → `reqwest`) depends on
`openssl-sys`, which needs a system OpenSSL for musl targets — not available in the
cross container by default. Fixed by adding the vendored OpenSSL feature to `Cargo.toml`:

```toml
# Cargo.toml — already added
openssl = { version = "0.10", features = ["vendored"] }
```

This builds OpenSSL from source and bundles it statically in the binary. No system
OpenSSL required, works with musl cross-compilation.

### Recommended tool: `cross`

[`cross`](https://github.com/cross-rs/cross) builds inside Docker containers that
already have the correct C toolchain for each target. It handles `aws-lc-sys`,
`native-tls`, and all C/C++ transitive deps automatically — no manual linker setup.

**Prerequisites**: Docker Desktop must be running.

```bash
# Install cross
cargo install cross --git https://github.com/cross-rs/cross

# Register targets with rustup
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl

# Build for Vultr HF + AWS t3.small (x86_64) — one binary for both machines
cross build --release --target x86_64-unknown-linux-musl

# Build for AWS c7g.medium (ARM64)
cross build --release --target aarch64-unknown-linux-musl

# Also build the benchmark binary for each target
cross build --release --bin latency_bench --target x86_64-unknown-linux-musl
cross build --release --bin latency_bench --target aarch64-unknown-linux-musl
```

Binaries land in:
```
target/x86_64-unknown-linux-musl/release/mtb            (24 MB, static-pie ELF)
target/x86_64-unknown-linux-musl/release/latency_bench  (11 MB, static-pie ELF)
target/aarch64-unknown-linux-musl/release/mtb
target/aarch64-unknown-linux-musl/release/latency_bench
```

### Deploy and benchmark

Files required on every machine:

| File | Why | How |
|---|---|---|
| `latency_bench` | Network benchmark binary | scp |
| `mtb` | Bot binary (for dry-run testing beyond the bench) | scp |
| `config.toml` | Assets, URLs, strategy params | scp |
| Secrets | `POLYMARKET_PRIVATE_KEY`, `TELEGRAM_BOT_TOKEN` | **env vars only — never scp `.env`** |

```bash
# ── Vultr High Frequency (x86_64) ──────────────────────────────────────────
scp target/x86_64-unknown-linux-musl/release/latency_bench user@<vultr-ip>:~/
scp target/x86_64-unknown-linux-musl/release/mtb            user@<vultr-ip>:~/
scp config.toml                                              user@<vultr-ip>:~/

# ── AWS t3.small (x86_64 — same binary as Vultr) ───────────────────────────
scp target/x86_64-unknown-linux-musl/release/latency_bench user@<aws-t3-ip>:~/
scp target/x86_64-unknown-linux-musl/release/mtb            user@<aws-t3-ip>:~/
scp config.toml                                              user@<aws-t3-ip>:~/

# ── AWS c7g.medium (ARM64 — separate binary) ────────────────────────────────
scp target/aarch64-unknown-linux-musl/release/latency_bench user@<aws-c7g-ip>:~/
scp target/aarch64-unknown-linux-musl/release/mtb            user@<aws-c7g-ip>:~/
scp config.toml                                               user@<aws-c7g-ip>:~/

# ── Run benchmark — no secrets needed for latency_bench ────────────────────
ssh user@<vultr-ip>   "chmod +x ~/latency_bench && ~/latency_bench"
ssh user@<aws-t3-ip>  "chmod +x ~/latency_bench && ~/latency_bench"
ssh user@<aws-c7g-ip> "chmod +x ~/latency_bench && ~/latency_bench"

# ── Run bot dry-run — pass secrets as env vars, nothing written to disk ─────
ssh user@<ip> "POLYMARKET_PRIVATE_KEY=xxx TELEGRAM_BOT_TOKEN=yyy ./mtb"
```

### Secrets management — do not copy `.env` to the VPS

The code already handles a missing `.env` gracefully — `dotenvy::dotenv()` is
best-effort and `std::env::var()` reads from the shell environment as fallback.
No code changes are needed. Never copy a plaintext `.env` to a remote machine.

**Option 1 — Shell env vars (benchmark / one-off runs)**

```bash
# Inline — nothing persisted to disk, gone after the process exits
POLYMARKET_PRIVATE_KEY=xxx TELEGRAM_BOT_TOKEN=yyy ./latency_bench
POLYMARKET_PRIVATE_KEY=xxx TELEGRAM_BOT_TOKEN=yyy ./mtb

# Or export for the session — gone when you log out
export POLYMARKET_PRIVATE_KEY=xxx
export TELEGRAM_BOT_TOKEN=yyy
./latency_bench
```

**Option 2 — systemd EnvironmentFile chmod 600 (production VPS)**

```bash
sudo mkdir -p /etc/mtb
sudo nano /etc/mtb/secrets      # paste keys, save
sudo chmod 600 /etc/mtb/secrets
```

```ini
# /etc/mtb/secrets
POLYMARKET_PRIVATE_KEY=xxx
TELEGRAM_BOT_TOKEN=yyy
```

```ini
# /etc/systemd/system/mtb.service
[Service]
EnvironmentFile=/etc/mtb/secrets
ExecStart=/home/ubuntu/mtb
Restart=on-failure
```

Only the service user can read the file. Keys never appear in `ps aux` and
are not in the home directory or repo.

**Option 3 — AWS SSM Parameter Store (production on AWS, zero files)**

```bash
# Store once — encrypted at rest via KMS
aws ssm put-parameter --name "/mtb/private_key"    --value "xxx" --type SecureString
aws ssm put-parameter --name "/mtb/telegram_token" --value "yyy" --type SecureString
```

The EC2 instance fetches secrets at startup via its IAM role — no files, no env
vars, no `.env`. Requires attaching an IAM role to the instance and a small code
change to call SSM at boot. Best choice for a permanent AWS deployment.

**Option 4 — Compile-time embedding (`option_env!()`) — avoid for the private key**

Baking secrets into the binary with `option_env!("POLYMARKET_PRIVATE_KEY")` is
convenient but the private key controls real funds and can be extracted trivially:

```bash
strings ./mtb | grep -E '[0-9a-f]{64}'
```

If the binary is ever shared, pushed anywhere, or the VPS is compromised, the
key is exposed. Use Options 1-3 instead.

**Note on `latency_bench`**: the benchmark binary measures network latency only
and does not place orders. It likely needs no secrets at all — just scp the
binary and run it directly.

---

### Why not `cargo-zigbuild`?

`cargo-zigbuild` is the cleaner option (no Docker required) but `aws-lc-sys` has known
build failures with zigbuild from macOS. Fixing it requires swapping the `rustls` crypto
backend from `aws-lc-rs` to `ring` in `Cargo.toml` — a code change just to work around
a tooling limitation. `cross` avoids all of that with no code changes required.

### Why not `x86_64-unknown-linux-gnu`?

The `gnu` cross-container ships an older GCC with a known memcmp optimization bug
([gcc#95189](https://gcc.gnu.org/bugzilla/show_bug.cgi?id=95189)). `aws-lc-sys`
explicitly detects this at build time and aborts with `### COMPILER BUG DETECTED ###`.
The musl container uses a different GCC version that doesn't trigger the check.
Verified: `x86_64-unknown-linux-gnu` fails; `x86_64-unknown-linux-musl` builds cleanly.

---

## Germany / Hetzner Note

Hetzner has no Amsterdam DC — closest is Frankfurt/Nuremberg (~15-20ms to CLOB).
Additionally, Germany's gambling regulator has flagged prediction markets like Polymarket
as unlicensed — potential compliance risk for operators on German IPs.

---

## Sources

- [Polymarket CLOB Geoblock Docs](https://docs.polymarket.com/developers/CLOB/geoblock)
- [AWS cross-region latency measurements (eu-west-1 → eu-west-2: ~11ms)](https://blog-en.richardimaoka.net/network-latency-analysis-with-ping-aws-cross-region)
- [AWS EC2 t3.small pricing eu-west-1](https://instances.vantage.sh/aws/ec2/t3.small)
- [QuantVPS — Polymarket server location explained](https://www.quantvps.com/blog/polymarket-servers-location)
- [QuantVPS — Best VPS for Polymarket](https://www.quantvps.com/blog/best-vps-polymarket-low-latency-servers-faster-execution)
- [QuantVPS — Amsterdam VPS for Polymarket](https://www.quantvps.com/locations/amsterdam-vps)
- [TradingVPS — Best VPS for Polymarket Trading](https://tradingvps.io/best-vps-for-polymarket-trading/)
- [LowEndTalk — lowest latency VPS to Polymarket (community)](https://lowendtalk.com/discussion/214229/lowest-latency-vps-to-polymarket)
- [QuantVPS — How latency impacts Polymarket bot performance](https://www.quantvps.com/blog/how-latency-impacts-polymarket-trading-performance)
- [Polymarket dynamic fees — latency arbitrage context (Finance Magnates)](https://www.financemagnates.com/cryptocurrency/polymarket-introduces-dynamic-fees-to-curb-latency-arbitrage-in-short-term-crypto-markets/)
- [Beyond Simple Arbitrage — opportunity windows 2025/2026 (Medium)](https://medium.com/illumination/beyond-simple-arbitrage-4-polymarket-strategies-bots-actually-profit-from-in-2026-ddacc92c5b4f)
- [AWS Monitoring Global Network Performance](https://aws.amazon.com/blogs/networking-and-content-delivery/monitoring-aws-global-network-performance/)
- [Vultr VPS benchmark comparison Jan 2025 — High Frequency vs Cloud Compute vs High Performance](https://joshtronic.com/2025/01/12/vps-showdown-january-2025-vultr-cpu-comparison/)
- [Vultr High Frequency Compute product page](https://www.vultr.com/products/high-frequency-compute/)
- [Vultr pricing](https://www.vultr.com/pricing/)
- [AWS EC2 t3.small pricing eu-west-1](https://aws-pricing.com/t3.small.html)
- [AWS EC2 t4g.small pricing eu-west-1](https://aws-pricing.com/t4g.small.html)
- [AWS EC2 c7g.medium pricing eu-west-1](https://aws-pricing.com/c7g.medium.html)
- [AWS burstable unlimited mode docs](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-performance-instances-unlimited-mode.html)
- [AWS T4g instances](https://aws.amazon.com/ec2/instance-types/t4/)
- [cross — Docker-based Rust cross-compilation tool](https://github.com/cross-rs/cross)
- [cargo-zigbuild — aws-lc-rs known macOS issue](https://github.com/aws/aws-lc-rs/issues/680)
- [rustls aws-lc-rs vs ring feature flags](https://users.rust-lang.org/t/completely-disable-downstream-feature-aws-lc-sys/115366)
