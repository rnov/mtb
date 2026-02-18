use rust_decimal::Decimal;

/// Best bid/ask for a single token, plus ask-side depth levels.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BestBidAsk {
    /// CLOB token ID (as string, since these are U256)
    pub token_id: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    /// Ask-side depth levels: (price, size), sorted ascending (cheapest first).
    /// Capped at MAX_ASK_DEPTH levels. Used for executable size and VWAP checks.
    pub ask_levels: Vec<(Decimal, Decimal)>,
    pub timestamp: i64,
}

/// Maximum number of ask-side depth levels to carry through the pipeline.
pub const MAX_ASK_DEPTH: usize = 10;

/// Events emitted by the unified feed manager.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum FeedEvent {
    /// Orderbook update for a token
    OrderbookUpdate(BestBidAsk),
    /// Crypto price update from an external exchange
    CryptoPrice {
        exchange: String,
        symbol: String,
        price: Decimal,
        timestamp: i64,
    },
    /// Chainlink resolution price from RTDS
    ChainlinkPrice {
        symbol: String,
        price: Decimal,
        timestamp: i64,
    },
    /// Feed disconnected
    Disconnected { source: String },
    /// Feed reconnected
    Reconnected { source: String },
}
