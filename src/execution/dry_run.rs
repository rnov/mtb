use async_trait::async_trait;
use tracing::info;

use crate::error::Result;
use crate::strategy::Opportunity;

use super::{ExecutionResult, Executor};

/// Dry-run executor: simulates execution without placing real orders.
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
        info!(
            market_id = %opportunity.market_id,
            asset = %opportunity.asset,
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
