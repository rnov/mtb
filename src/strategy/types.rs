use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::market::MarketPair;

/// A snapshot of the current state of a market, used as input to strategies.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MarketSnapshot {
    pub market: MarketPair,
    pub up_best_bid: Option<Decimal>,
    pub up_best_ask: Option<Decimal>,
    pub down_best_bid: Option<Decimal>,
    pub down_best_ask: Option<Decimal>,
    /// Ask-side depth for the UP token: (price, size) sorted ascending (cheapest first).
    pub up_ask_levels: Vec<(Decimal, Decimal)>,
    /// Ask-side depth for the DOWN token: (price, size) sorted ascending (cheapest first).
    pub down_ask_levels: Vec<(Decimal, Decimal)>,
    pub timestamp: DateTime<Utc>,
}

/// An arb or directional opportunity detected by a strategy.
#[derive(Debug, Clone, Serialize)]
pub struct Opportunity {
    pub market_id: String,
    pub asset: String,
    pub question: String,
    pub kind: OpportunityKind,
    /// Raw combined ask price (up_ask + down_ask) before fees
    pub combined_ask: Decimal,
    /// Individual leg prices
    pub up_ask: Decimal,
    pub down_ask: Decimal,
    /// Per-leg fees
    pub up_fee: Decimal,
    pub down_fee: Decimal,
    /// Combined ask price after fees
    pub combined_ask_after_fees: Decimal,
    /// Expected profit per unit ($1.00 payout - combined_ask_after_fees)
    pub profit_per_unit: Decimal,
    /// Number of units (shares) to buy on each leg
    pub units: Decimal,
    /// Total USDC cost for all units on both legs including fees
    pub total_cost_usdc: Decimal,
    /// Expected total profit in USDC
    pub expected_profit_usdc: Decimal,
    pub up_token_id: String,
    pub down_token_id: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpportunityKind {
    Arb,
    #[allow(dead_code)]
    Directional {
        direction: Direction,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum Direction {
    Up,
    Down,
}

/// An intent to place an order, produced by the strategy layer.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct OrderIntent {
    pub token_id: String,
    pub side: OrderSide,
    pub size_usdc: Decimal,
    pub price: Decimal,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum OrderSide {
    Buy,
    Sell,
}
