use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    pub mode: RunMode,
    pub clob_url: String,
    pub gamma_url: String,
    pub chain_id: u64,
    pub strategy: StrategyConfig,
    pub risk: RiskConfig,
    pub discovery: DiscoveryConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    DryRun,
    Live,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StrategyConfig {
    /// Minimum profit (after fees) to trigger an arb, e.g. 0.005 = 0.5%
    pub min_arb_profit: Decimal,
    /// Assets to track: "BTC", "ETH", "SOL"
    pub assets: Vec<String>,
    /// Maximum order size in USDC
    pub max_order_size_usdc: Decimal,
    /// Minimum number of fresh exchange price sources required to validate an arb
    #[serde(default = "default_min_fresh_sources")]
    pub min_fresh_sources: usize,
    /// Maximum age (ms) of exchange prices before considered stale
    #[serde(default = "default_max_price_age_ms")]
    pub max_price_age_ms: u64,
    /// Maximum price divergence (%) between exchanges — above this, leg risk is too high
    #[serde(default = "default_max_divergence_pct")]
    pub max_divergence_pct: Decimal,
    /// Maximum price for either leg — caps directional exposure on partial fill
    #[serde(default = "default_max_leg_price")]
    pub max_leg_price: Decimal,
    /// Minimum USDC value per order — CLOB rejects orders below this threshold
    #[serde(default = "default_min_order_usdc")]
    pub min_order_usdc: Decimal,
}

fn default_min_fresh_sources() -> usize {
    2
}

fn default_max_price_age_ms() -> u64 {
    2000
}

fn default_max_divergence_pct() -> Decimal {
    Decimal::new(5, 1) // 0.5%
}

fn default_max_leg_price() -> Decimal {
    Decimal::new(90, 2) // 0.90
}

fn default_min_order_usdc() -> Decimal {
    Decimal::ONE // $1.00 — CLOB minimum per order
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RiskConfig {
    /// Maximum total exposure in USDC across all positions
    pub max_exposure_usdc: Decimal,
    /// Maximum loss in USDC before kill switch triggers
    pub max_daily_loss_usdc: Decimal,
    /// Drawdown percentage (0.0-1.0) before kill switch triggers
    pub max_drawdown_pct: Decimal,
    /// Maximum consecutive failed orders before kill switch
    pub max_consecutive_failures: u32,
    /// Seconds of feed disconnect before halting
    pub feed_disconnect_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryConfig {
    /// How often to poll Gamma for new markets (seconds)
    pub poll_interval_secs: u64,
    /// Assets to discover: "BTC", "ETH", "SOL", "XRP"
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Path for JSON lines trade log
    pub trade_log_path: String,
    /// tracing filter directive, e.g. "mtb=debug,info"
    pub filter: String,
}

/// Secrets loaded from environment variables (never serialized)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Secrets {
    pub private_key: String,
    /// Telegram bot token (from @BotFather)
    pub telegram_bot_token: Option<String>,
    /// Telegram chat ID to send alerts to / accept commands from
    pub telegram_chat_id: Option<i64>,
}
