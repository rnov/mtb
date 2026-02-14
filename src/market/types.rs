use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

/// Represents a binary 15-minute crypto up/down market on Polymarket.
/// Each market has an Up and Down token.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MarketPair {
    /// Gamma market ID
    pub market_id: String,
    /// Human-readable question, e.g. "Bitcoin Up or Down - February 14, 4:45PM-5:00PM ET"
    pub question: String,
    /// Underlying crypto asset: "BTC", "ETH", "SOL", "XRP"
    pub asset: String,
    /// Slug used to fetch this market, e.g. "btc-updown-15m-1771105500"
    pub slug: String,
    /// Up token ID (CLOB asset ID) — first outcome
    pub up_token_id: String,
    /// Down token ID (CLOB asset ID) — second outcome
    pub down_token_id: String,
    /// When this market expires/resolves
    pub end_date: Option<DateTime<Utc>>,
    /// Current liquidity in USDC
    pub liquidity: Option<Decimal>,
    /// Whether the market is neg-risk
    pub neg_risk: bool,
}
