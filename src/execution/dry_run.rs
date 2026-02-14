use async_trait::async_trait;
use tracing::info;

use crate::error::Result;
use crate::monitor::TradeLogger;
use crate::strategy::Opportunity;

use super::{ExecutionResult, Executor};

/// Dry-run executor: logs opportunities to JSON lines without placing real orders.
pub struct DryRunExecutor {
    logger: TradeLogger,
}

impl DryRunExecutor {
    pub fn new(logger: TradeLogger) -> Self {
        Self { logger }
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

        self.logger.log_opportunity(opportunity)?;

        Ok(ExecutionResult {
            market_id: opportunity.market_id.clone(),
            success: true,
            message: "dry run logged".into(),
        })
    }
}
