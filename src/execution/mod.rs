pub mod dry_run;

use async_trait::async_trait;

use crate::error::Result;
use crate::strategy::Opportunity;

/// Execution result returned after processing an opportunity.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExecutionResult {
    pub market_id: String,
    pub success: bool,
    pub message: String,
}

/// Trait for executing trading opportunities.
/// Implementations: DryRunExecutor (logs), LiveExecutor (Phase 2).
#[async_trait]
pub trait Executor: Send + Sync {
    async fn execute(&self, opportunity: &Opportunity) -> Result<ExecutionResult>;
}

pub use dry_run::DryRunExecutor;
