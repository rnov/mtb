use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::info;

use crate::error::Result;
use crate::strategy::Opportunity;

use super::{ExecutionResult, Executor};

/// Dry-run executor: simulates execution without placing real orders.
/// Computes and logs the same share counts as LiveExecutor for verification.
#[derive(Default)]
pub struct DryRunExecutor;

impl DryRunExecutor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Executor for DryRunExecutor {
    async fn execute(&self, opportunity: &Opportunity) -> Result<ExecutionResult> {
        // Mirror LiveExecutor share calculation for verification
        let units =
            round_shares(opportunity.suggested_size_usdc / opportunity.combined_ask_after_fees);
        let up_cost = units * opportunity.up_ask;
        let down_cost = units * opportunity.down_ask;
        let total_cost = up_cost + down_cost;

        info!(
            market_id = %opportunity.market_id,
            asset = %opportunity.asset,
            units = %units,
            up_price = %opportunity.up_ask,
            down_price = %opportunity.down_ask,
            up_cost = %up_cost.round_dp(4),
            down_cost = %down_cost.round_dp(4),
            total_cost = %total_cost.round_dp(4),
            profit = %opportunity.expected_profit_usdc,
            "[DRY RUN] would execute arb"
        );

        Ok(ExecutionResult {
            market_id: opportunity.market_id.clone(),
            success: true,
            message: "dry run".into(),
        })
    }
}

/// Round share size to 2 decimal places (CLOB lot size constraint).
fn round_shares(shares: Decimal) -> Decimal {
    shares.round_dp(2)
}
