use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::info;

use crate::error::{Error, Result};
use crate::execution::ExecutionResult;
use crate::strategy::Opportunity;

/// A complete trade record: opportunity + execution outcome.
#[derive(Debug, Serialize)]
pub struct TradeRecord<'a> {
    /// When this record was written
    pub recorded_at: DateTime<Utc>,
    /// The detected opportunity
    #[serde(flatten)]
    pub opportunity: &'a Opportunity,
    /// Whether execution was attempted (false if risk-rejected)
    pub executed: bool,
    /// Execution success (None if not executed)
    pub success: Option<bool>,
    /// Execution message (order IDs, error details, etc.)
    pub execution_message: Option<String>,
    /// Why execution was skipped (risk rejection reason)
    pub reject_reason: Option<String>,
}

/// Append-only JSON lines trade/opportunity log.
pub struct TradeLogger {
    #[allow(dead_code)]
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl TradeLogger {
    pub fn new(path: &str) -> Result<Self> {
        let path = PathBuf::from(path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        info!(path = %path.display(), "trade logger initialized");

        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Log an opportunity as a JSON line (simple format without execution details).
    #[allow(dead_code)]
    pub fn log_opportunity(&self, opportunity: &Opportunity) -> Result<()> {
        let json = serde_json::to_string(opportunity)?;
        self.write_line(&json)
    }

    /// Log a full trade record: opportunity + execution result.
    pub fn log_trade(&self, opportunity: &Opportunity, result: &ExecutionResult) -> Result<()> {
        let record = TradeRecord {
            recorded_at: Utc::now(),
            opportunity,
            executed: true,
            success: Some(result.success),
            execution_message: Some(result.message.clone()),
            reject_reason: None,
        };
        let json = serde_json::to_string(&record)?;
        self.write_line(&json)
    }

    /// Log a risk-rejected opportunity (not executed).
    pub fn log_rejected(&self, opportunity: &Opportunity, reason: &str) -> Result<()> {
        let record = TradeRecord {
            recorded_at: Utc::now(),
            opportunity,
            executed: false,
            success: None,
            execution_message: None,
            reject_reason: Some(reason.to_string()),
        };
        let json = serde_json::to_string(&record)?;
        self.write_line(&json)
    }

    fn write_line(&self, json: &str) -> Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| Error::Monitor("logger lock poisoned".into()))?;
        writeln!(file, "{json}")?;
        Ok(())
    }
}
