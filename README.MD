## MTB Alpha


### Plan to implement                                                                                                                                              
                                                                                                                                                               
Architecture Plan: mtb — Polymarket 15-Minute Crypto Arb Trading Bot                                                                                           
                                                                                                                                                               
All code, config, and artifacts live in /Users/rsm/rdev/mtb.                                                                                                   
                                                                                                                                                               
Context                                                                                                                                                        
                                                                                                                                                               
We're building a Polymarket trading bot targeting 15-minute crypto up/down markets (BTC/ETH/SOL). The bot detects arb opportunities when YES_ask + NO_ask <    
1.0 after fees, and optionally trades directionally using external price signals. Built on top of the official Polymarket/rs-clob-client SDK (v0.4.2).         
                                                                                                                                                               
Module Layout                                                                                                                                                  
                                                                                                                                                               
src/                                                                                                                                                           
  main.rs -- Entry point: load config, wire modules, run event loop                                                                           
  lib.rs -- Re-exports for testing                                                                                                           
                                                                                                                                                               
  config/                                                                                                                                                      
    mod.rs                 -- Config loading (.env + config.toml)                                                                                              
    types.rs               -- AppConfig, RunMode, StrategyConfig, RiskConfig                                                                                   
                                                                                                                                                               
  market/                                                                                                                                                      
    mod.rs -- Market discovery and lifecycle                                                                                                   
    discovery.rs -- Gamma API: find active 15-min crypto markets                                                                                     
    types.rs -- MarketPair (yes/no token IDs, asset, expiry)                                                                                     
                                                                                                                                                               
  feed/                                                                                                                                                        
    mod.rs -- Unified feed manager (spawns + multiplexes WS sources)                                                                           
    orderbook.rs -- CLOB WebSocket: BestBidAsk per token                                                                                             
    crypto_price.rs -- RTDS: Binance BTC/ETH/SOL prices                                                                                                 
    types.rs -- FeedEvent enum                                                                                                                   
                                                                                                                                                               
  strategy/                                                                                                                                                    
    mod.rs -- Strategy dispatcher                                                                                                              
    arb.rs -- Pure arb: YES_ask + NO_ask < 1.0 after fees                                                                                      
    directional.rs -- Directional: external price signal + mispricing                                                                                  
    fees.rs -- Precise dynamic taker fee model                                                                                                  
    types.rs -- MarketSnapshot, Opportunity, OrderIntent                                                                                         
                                                                                                                                                               
  execution/                                                                                                                                                   
    mod.rs -- Executor trait                                                                                                                   
    executor.rs -- LiveExecutor: SDK order building + submission                                                                                    
    dry_run.rs -- DryRunExecutor: logs what it would do to JSON lines                                                                              
                                                                                                                                                               
  risk/                                                                                                                                                        
    mod.rs -- RiskManager: gates every execution                                                                                               
    limits.rs -- Position limits, exposure caps, drawdown tracking                                                                                
    kill_switch.rs -- Global halt (loss-based, feed disconnect, manual /kill)                                                                          
                                                                                                                                                               
  monitor/                                                                                                                                                     
    mod.rs -- Monitoring hub                                                                                                                   
    telegram.rs -- Telegram alerts + commands (/status, /kill, /resume)                                                                             
    metrics.rs -- Prometheus counters/gauges (optional feature)                                                                                    
    logger.rs -- Append-only JSON lines trade log                                                                                                 
                                                                                                                                                               
  error.rs -- Unified error types (thiserror)                                                                                                  
                                                                                                                                                               
Project root files:                                                                                                                                            
config.toml                -- Runtime configuration (committed, sane defaults)                                                                                 
.env                       -- Secrets (never committed, in .gitignore)                                                                                         
                                                                                                                                                               
Data Flow                                                                                                                                                      
                                                                                                                                                               
Gamma (HTTP 60s poll) → Market Registry (active MarketPairs)                                                                                                   
                                                                                                                                                             
              ┌───────────────┼───────────────┐                                                                                                                
              ▼                               ▼                                                                                                                
     CLOB WebSocket                    RTDS WebSocket                                                                                                          
     (orderbook data)                  (Binance prices)                                                                                                        
                                                                                                                                                           
              └───────────┬───────────────────┘                                                                                                                
                          ▼                                                                                                                                    
                  broadcast channel                                                                                                                            
                          ▼                                                                                                                                    
                  Main Event Loop (tokio::select!)                                                                                                             
                          ▼                                                                                                                                    
                  Strategy Engine (pure computation)                                                                                                           
                  - build MarketSnapshot                                                                                                                       
                  - detect arb / directional signal                                                                                                            
                  - calculate fees precisely                                                                                                                   
                          ▼                                                                                                                                    
                  Risk Manager (sync gate)                                                                                                                     
                  - position limit check                                                                                                                       
                  - drawdown check                                                                                                                             
                  - kill switch check                                                                                                                          
                  - feed connected check                                                                                                                       
                          ▼                                                                                                                                    
                  Executor (trait-based)                                                                                                                       
                  - Live: SDK order placement                                                                                                                  
                  - DryRun: log to JSON lines                                                                                                                  
                          ▼                                                                                                                                    
                  Monitor                                                                                                                                      
                  - Telegram alert                                                                                                                             
                  - Prometheus metric                                                                                                                          
                  - JSON log line                                                                                                                              
                                                                                                                                                               
Key Design Decisions                                                                                                                                           
                                                                                                                                                               
1. Executor trait — LiveExecutor and DryRunExecutor implement the same Executor trait. Switched via config.toml mode = "dry_run" | "live".                     
2. Strategy is pure — No IO, no async. Takes MarketSnapshot, returns Option<Opportunity>. Fully unit-testable.                                                 
3. Risk manager is synchronous — In-memory state behind RwLock. Hot path (read lock) never blocks on IO.                                                       
4. Kill switch auto-triggers on: drawdown threshold, daily loss limit, feed disconnect >5s, N consecutive failed orders. Resumable only via Telegram /resume.  
5. Fee model — Polymarket dynamic taker fees vary by price. Must be precise — wrong fees = false signals. Validated with unit tests against known values.      
                                                                                                                                                               
Cargo.toml Dependencies                                                                                                                                        
                                                                                                                                                               
[dependencies]                                                                                                                                                 
polymarket-client-sdk = { git = "https://github.com/Polymarket/rs-clob-client", features = ["clob", "ws", "rtds", "gamma", "data", "heartbeats"] }             
tokio = { version = "1", features = ["full"] }                                                                                                                 
serde = { version = "1", features = ["derive"] }                                                                                                               
serde_json = "1"                                                                                                                                               
toml = "0.8"                                                                                                                                                   
dotenvy = "0.15"                                                                                                                                               
thiserror = "2"                                                                                                                                                
anyhow = "1"                                                                                                                                                   
rust_decimal = "1.40"                                                                                                                                          
tracing = "0.1"                                                                                                                                                
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }                                                                                    
chrono = { version = "0.4", features = ["serde"] }                                                                                                             
alloy = { version = "1.6", features = ["signers", "signer-local"] }                                                                                            
teloxide = { version = "0.13", features = ["macros"], optional = true }                                                                                        
prometheus = { version = "0.13", optional = true }                                                                                                             
axum = { version = "0.8", optional = true }                                                                                                                    
                                                                                                                                                               
[features]                                                                                                                                                     
default = ["telegram"]                                                                                                                                         
telegram = ["dep:teloxide"]                                                                                                                                    
metrics = ["dep:prometheus", "dep:axum"]                                                                                                                       
                                                                                                                                                               
Build Phases                                                                                                                                                   
                                                                                                                                                               
Phase 1: Foundation (dry-run arb detection)                                                                                                                    
                                                                                                                                                               
1. config/ — load .env + config.toml, validate                                                                                                                 
2. error.rs — unified error type                                                                                                                               
3. market/discovery.rs — Gamma client, find active 15-min markets                                                                                              
4. feed/orderbook.rs — CLOB WS subscription for BestBidAsk                                                                                                     
5. strategy/fees.rs — fee model with unit tests                                                                                                                
6. strategy/arb.rs — pure arb detection                                                                                                                        
7. execution/dry_run.rs — log opportunities to JSON lines                                                                                                      
8. Wire in main.rs — end-to-end dry-run                                                                                                                        
                                                                                                                                                               
Deliverable: Running binary that connects to real Polymarket WS, discovers markets, logs every arb opportunity. Zero capital risk.

Phase 2: Live execution + safety                                                                                                                               
                                                                                                                                                               
1. execution/executor.rs — SDK auth, order building, submission                                                                                                
2. risk/ — position limits, drawdown kill switch, feed disconnect halt                                                                                         
3. monitor/logger.rs — structured trade log                                                                                                                    
4. monitor/telegram.rs — alerts + commands                                                                                                                     
                                                                                                                                                               
Deliverable: Bot places real orders with safety rails. Telegram alerts on every trade.                                                                         
                                                                                                                                                               
Phase 3: Enhancement                                                                                                                                           
                                                                                                                                                               
1. feed/crypto_price.rs — RTDS Binance price feed                                                                                                              
2. strategy/directional.rs — directional signal logic                                                                                                          
3. monitor/metrics.rs — Prometheus endpoint                                                                                                                    
4. Performance profiling and optimization                                                                                                                      
5. Deployment: Dockerfile, VPS setup                                                                                                                           
                                                                                                                                                               
Phase 4: Hardening                                                                                                                                             
                                                                                                                                                               
1. Backtesting with recorded dry-run data                                                                                                                      
2. Rate limit tracking and adaptive backoff                                                                                                                    
3. Testnet deployment on Amoy (chain_id 80002)                                                                                                                 
                                                                                                                                                               
Testing Strategy                                                                                                                                               
                                                                                                                                                               
- strategy/ and risk/ — >90% unit test coverage. Pure logic, no excuses.                                                                                       
- fees.rs — Every test validated against real Polymarket fee values.                                                                                           
- Integration tests (behind #[ignore]) — real Gamma API discovery, real CLOB WS connection, full dry-run pipeline.                                             
- Executor trait — allows mock executor for testing main event loop without network.                                                                           
                                                                                                                                                               
Verification                                                                                                                                                   
                                                                                                                                                               
After Phase 1:                                                                                                                                                 
1. cargo build — compiles cleanly                                                                                                                              
2. cargo clippy — no warnings                                                                                                                                  
3. cargo test — all unit tests pass (fee model, arb detection, risk checks)                                                                                    
4. cargo run with mode = "dry_run" — connects to WS, discovers markets, logs opportunities to stdout and JSON file                                             
5. Inspect dry-run JSON log — verify opportunity records have correct fee calculations