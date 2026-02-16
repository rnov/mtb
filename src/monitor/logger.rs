use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::info;

use crate::error::{Error, Result};
use crate::feed::price_state::ExchangeSnapshot;
use crate::strategy::Opportunity;

/// Which stage of the pipeline the arb reached before logging.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    /// Freshness guard rejected (high leg risk)
    FreshnessRejected,
    /// Risk manager rejected (position/exposure/kill switch)
    RiskRejected,
    /// Execution attempted and succeeded
    Executed,
    /// Execution attempted but failed
    ExecutionFailed,
}

/// A complete trade record: opportunity + pipeline outcome + exchange context.
#[derive(Debug, Serialize)]
pub struct TradeRecord<'a> {
    /// When this record was written
    pub recorded_at: DateTime<Utc>,
    /// The detected opportunity
    #[serde(flatten)]
    pub opportunity: &'a Opportunity,
    /// Which pipeline stage this arb reached
    pub stage: PipelineStage,
    /// Rejection or error reason (None if executed successfully)
    pub reason: Option<String>,
    /// Exchange prices at decision time — full context for analysis
    pub exchange_snapshot: ExchangeSnapshot,
    /// Timing: arb detection (microseconds)
    pub arb_us: u128,
    /// Timing: freshness check (microseconds, None if not reached)
    pub fresh_us: Option<u128>,
    /// Timing: risk check (microseconds, None if not reached)
    pub risk_us: Option<u128>,
    /// Timing: execution (milliseconds, None if not reached)
    pub exec_ms: Option<u128>,
    /// Timing: total tick-to-log (microseconds)
    pub total_us: u128,
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

    /// Log a full trade record with pipeline context.
    pub fn log_record(&self, record: &TradeRecord) -> Result<()> {
        let json = serde_json::to_string(record)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::price_state::ExchangePrice;
    use crate::strategy::types::OpportunityKind;
    use rust_decimal_macros::dec;

    fn test_opportunity() -> Opportunity {
        Opportunity {
            market_id: "test-market".into(),
            asset: "BTC".into(),
            question: "test".into(),
            kind: OpportunityKind::Arb,
            combined_ask: dec!(0.96),
            combined_ask_after_fees: dec!(0.98),
            profit_per_unit: dec!(0.02),
            suggested_size_usdc: dec!(50),
            expected_profit_usdc: dec!(1.0),
            up_token_id: "up-123".into(),
            down_token_id: "down-456".into(),
            up_ask: dec!(0.48),
            down_ask: dec!(0.48),
            up_fee: dec!(0.0096),
            down_fee: dec!(0.0096),
            timestamp: Utc::now(),
        }
    }

    fn test_snapshot() -> ExchangeSnapshot {
        ExchangeSnapshot {
            asset: "BTC".into(),
            exchanges: vec![
                ExchangePrice {
                    exchange: "binance".into(),
                    price: dec!(69000),
                    age_ms: 15,
                },
                ExchangePrice {
                    exchange: "coinbase".into(),
                    price: dec!(69010),
                    age_ms: 22,
                },
            ],
            fresh_count: 2,
            divergence_pct: Some(dec!(0.014)),
            chainlink_price: Some(dec!(69005)),
            chainlink_age_ms: Some(1200),
        }
    }

    #[test]
    fn trade_record_serializes_executed() {
        let opp = test_opportunity();
        let record = TradeRecord {
            recorded_at: Utc::now(),
            opportunity: &opp,
            stage: PipelineStage::Executed,
            reason: None,
            exchange_snapshot: test_snapshot(),
            arb_us: 1,
            fresh_us: Some(2),
            risk_us: Some(3),
            exec_ms: Some(45),
            total_us: 45003,
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"stage\":\"executed\""));
        assert!(json.contains("\"exchange_snapshot\""));
        assert!(json.contains("\"binance\""));
        assert!(json.contains("\"fresh_count\":2"));
        assert!(json.contains("\"arb_us\":1"));
    }

    #[test]
    fn trade_record_serializes_rejected() {
        let opp = test_opportunity();
        let record = TradeRecord {
            recorded_at: Utc::now(),
            opportunity: &opp,
            stage: PipelineStage::FreshnessRejected,
            reason: Some("price divergence for BTC: 1.2% (max 0.5%)".into()),
            exchange_snapshot: test_snapshot(),
            arb_us: 1,
            fresh_us: Some(2),
            risk_us: None,
            exec_ms: None,
            total_us: 3,
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"stage\":\"freshness_rejected\""));
        assert!(json.contains("\"reason\":\"price divergence"));
    }

    #[test]
    fn trade_record_round_trips() {
        let opp = test_opportunity();
        let record = TradeRecord {
            recorded_at: Utc::now(),
            opportunity: &opp,
            stage: PipelineStage::RiskRejected,
            reason: Some("exposure limit".into()),
            exchange_snapshot: test_snapshot(),
            arb_us: 5,
            fresh_us: Some(2),
            risk_us: Some(8),
            exec_ms: None,
            total_us: 15,
        };

        let json = serde_json::to_string(&record).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["stage"], "risk_rejected");
        assert_eq!(value["reason"], "exposure limit");
        assert_eq!(value["exchange_snapshot"]["asset"], "BTC");
        assert_eq!(
            value["exchange_snapshot"]["exchanges"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(value["arb_us"], 5);
        assert!(value["exec_ms"].is_null());
    }
}
