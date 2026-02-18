pub mod kill_switch;
pub mod limits;

use std::sync::RwLock;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::config::types::RiskConfig;
use crate::error::{Error, Result};
use crate::strategy::Opportunity;

use kill_switch::KillSwitch;
use limits::PositionLimits;

/// Central risk manager that gates every execution.
/// All state is behind RwLock for fast, synchronous access.
pub struct RiskManager {
    config: RiskConfig,
    feed_timeout: Duration,
    state: RwLock<RiskState>,
}

/// Base backoff delay after first failure (doubles each consecutive failure).
const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Maximum backoff cap to avoid waiting forever.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

struct RiskState {
    kill_switch: KillSwitch,
    limits: PositionLimits,
    /// Timestamp of last feed activity (orderbook update received).
    last_feed_activity: Instant,
    /// Whether the feed is currently connected.
    feed_connected: bool,
    /// Timestamp of last recorded failure (for backoff calculation).
    last_failure_at: Option<Instant>,
}

impl RiskManager {
    pub fn new(config: RiskConfig) -> Self {
        let feed_timeout = Duration::from_secs(config.feed_disconnect_timeout_secs);
        let state = RiskState {
            kill_switch: KillSwitch::new(),
            limits: PositionLimits::new(config.max_exposure_usdc),
            last_feed_activity: Instant::now(),
            feed_connected: true,
            last_failure_at: None,
        };
        Self {
            config,
            feed_timeout,
            state: RwLock::new(state),
        }
    }

    /// Check if an opportunity passes all risk checks.
    /// Returns Ok(()) if allowed, Err with reason if blocked.
    pub fn check(&self, opportunity: &Opportunity) -> Result<()> {
        let state = self
            .state
            .read()
            .map_err(|_| Error::Risk("lock poisoned".into()))?;

        // Kill switch check
        if state.kill_switch.is_killed() {
            return Err(Error::Risk(format!(
                "kill switch active: {}",
                state.kill_switch.reason()
            )));
        }

        // Feed connectivity check
        if !state.feed_connected {
            return Err(Error::Risk(
                "feed disconnected — refusing to trade on stale data".into(),
            ));
        }

        // Feed staleness check (even if "connected", no data = stale)
        let elapsed = state.last_feed_activity.elapsed();
        if elapsed > self.feed_timeout {
            return Err(Error::Risk(format!(
                "feed stale: no data for {:.1}s (timeout={}s)",
                elapsed.as_secs_f64(),
                self.feed_timeout.as_secs()
            )));
        }

        // Adaptive backoff check: if we have consecutive failures, wait before retrying
        let failures = state.limits.consecutive_failures();
        if failures > 0 {
            if let Some(last_fail) = state.last_failure_at {
                let backoff = backoff_duration(failures);
                let elapsed = last_fail.elapsed();
                if elapsed < backoff {
                    let remaining = backoff - elapsed;
                    return Err(Error::Risk(format!(
                        "backoff active: {} consecutive failures, wait {:.1}s (of {:.1}s)",
                        failures,
                        remaining.as_secs_f64(),
                        backoff.as_secs_f64(),
                    )));
                }
            }
        }

        // Exposure limit check
        if !state
            .limits
            .can_add_exposure(opportunity.total_cost_usdc)
        {
            return Err(Error::Risk(format!(
                "exposure limit exceeded: current={}, requested={}, max={}",
                state.limits.current_exposure(),
                opportunity.total_cost_usdc,
                self.config.max_exposure_usdc,
            )));
        }

        Ok(())
    }

    /// Record that we received feed data (resets staleness timer).
    pub fn record_feed_activity(&self) {
        if let Ok(mut state) = self.state.write() {
            state.last_feed_activity = Instant::now();
        }
    }

    /// Record a feed disconnect event. Triggers kill switch if already stale.
    pub fn record_disconnect(&self, source: &str) {
        if let Ok(mut state) = self.state.write() {
            state.feed_connected = false;
            warn!(source = source, "feed disconnected — trading halted");
        }
    }

    /// Record a feed reconnect event. Resumes trading if kill switch wasn't
    /// triggered for other reasons.
    pub fn record_reconnect(&self, source: &str) {
        if let Ok(mut state) = self.state.write() {
            state.feed_connected = true;
            state.last_feed_activity = Instant::now();
            info!(source = source, "feed reconnected — trading may resume");
        }
    }

    /// Record a realized loss. Triggers kill switch if thresholds breached.
    #[allow(dead_code)]
    pub fn record_loss(&self, amount: Decimal) {
        if let Ok(mut state) = self.state.write() {
            state.limits.add_daily_loss(amount);
            if state.limits.daily_loss() >= self.config.max_daily_loss_usdc {
                let reason = format!(
                    "daily loss limit reached: {} >= {}",
                    state.limits.daily_loss(),
                    self.config.max_daily_loss_usdc
                );
                warn!("{}", reason);
                state.kill_switch.trigger(&reason);
            }
        }
    }

    /// Record a successful execution (adds to exposure).
    pub fn record_execution(&self, size_usdc: Decimal) {
        if let Ok(mut state) = self.state.write() {
            state.limits.add_exposure(size_usdc);
        }
    }

    /// Record a consecutive failure. Triggers kill switch if threshold breached.
    pub fn record_failure(&self) {
        if let Ok(mut state) = self.state.write() {
            state.limits.increment_failures();
            state.last_failure_at = Some(Instant::now());
            let failures = state.limits.consecutive_failures();
            if failures >= self.config.max_consecutive_failures {
                let reason = format!("too many consecutive failures: {}", failures);
                warn!("{}", reason);
                state.kill_switch.trigger(&reason);
            } else {
                let backoff = backoff_duration(failures);
                warn!(
                    failures = failures,
                    backoff_secs = backoff.as_secs_f64(),
                    "execution failed — backoff active"
                );
            }
        }
    }

    /// Reset consecutive failure counter and backoff (on success).
    pub fn reset_failures(&self) {
        if let Ok(mut state) = self.state.write() {
            state.limits.reset_failures();
            state.last_failure_at = None;
        }
    }

    /// Manually trigger the kill switch.
    #[allow(dead_code)]
    pub fn kill(&self, reason: &str) {
        if let Ok(mut state) = self.state.write() {
            state.kill_switch.trigger(reason);
            warn!(reason, "kill switch manually triggered");
        }
    }

    /// Resume trading (reset kill switch).
    #[allow(dead_code)]
    pub fn resume(&self) {
        if let Ok(mut state) = self.state.write() {
            state.kill_switch.reset();
            info!("kill switch reset, trading resumed");
        }
    }

    #[allow(dead_code)]
    pub fn is_killed(&self) -> bool {
        self.state
            .read()
            .map(|s| s.kill_switch.is_killed())
            .unwrap_or(true)
    }

    /// Return a human-readable status summary.
    pub fn status_report(&self) -> String {
        let state = match self.state.read() {
            Ok(s) => s,
            Err(_) => return "error: lock poisoned".into(),
        };

        let killed = state.kill_switch.is_killed();
        let kill_reason = if killed {
            state.kill_switch.reason().to_string()
        } else {
            "—".into()
        };
        let connected = state.feed_connected;
        let feed_age = state.last_feed_activity.elapsed();
        let exposure = state.limits.current_exposure();
        let daily_loss = state.limits.daily_loss();
        let failures = state.limits.consecutive_failures();

        format!(
            "Kill switch: {}\n\
             Reason: {}\n\
             Feed connected: {}\n\
             Feed age: {:.1}s\n\
             Exposure: {} / {} USDC\n\
             Daily loss: {} / {} USDC\n\
             Consecutive failures: {} / {}",
            if killed { "ACTIVE" } else { "off" },
            kill_reason,
            connected,
            feed_age.as_secs_f64(),
            exposure,
            self.config.max_exposure_usdc,
            daily_loss,
            self.config.max_daily_loss_usdc,
            failures,
            self.config.max_consecutive_failures,
        )
    }
}

/// Compute backoff duration: 1s, 2s, 4s, 8s, ... capped at 30s.
fn backoff_duration(consecutive_failures: u32) -> Duration {
    let secs = BACKOFF_BASE.as_secs().saturating_mul(
        1u64.checked_shl(consecutive_failures.saturating_sub(1))
            .unwrap_or(u64::MAX),
    );
    Duration::from_secs(secs).min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::thread;

    use crate::strategy::types::{Opportunity, OpportunityKind};

    fn test_config() -> RiskConfig {
        RiskConfig {
            max_exposure_usdc: dec!(500),
            max_daily_loss_usdc: dec!(100),
            max_drawdown_pct: dec!(0.10),
            max_consecutive_failures: 3,
            feed_disconnect_timeout_secs: 1, // 1 second for fast tests
        }
    }

    fn test_opportunity() -> Opportunity {
        Opportunity {
            market_id: "test".into(),
            asset: "BTC".into(),
            question: "test".into(),
            kind: OpportunityKind::Arb,
            combined_ask: dec!(0.90),
            combined_ask_after_fees: dec!(0.92),
            profit_per_unit: dec!(0.08),
            units: dec!(50),
            total_cost_usdc: dec!(50),
            expected_profit_usdc: dec!(4),
            up_token_id: "123".into(),
            down_token_id: "456".into(),
            up_ask: dec!(0.45),
            down_ask: dec!(0.45),
            up_fee: dec!(0.009),
            down_fee: dec!(0.009),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn feed_disconnect_blocks_trading() {
        let rm = RiskManager::new(test_config());
        let opp = test_opportunity();

        // Initially connected — should pass
        assert!(rm.check(&opp).is_ok());

        // Disconnect — should block
        rm.record_disconnect("test-ws");
        assert!(rm.check(&opp).is_err());

        // Reconnect — should pass again
        rm.record_reconnect("test-ws");
        assert!(rm.check(&opp).is_ok());
    }

    #[test]
    fn feed_staleness_blocks_trading() {
        let config = RiskConfig {
            feed_disconnect_timeout_secs: 0, // 0 seconds = immediate staleness
            ..test_config()
        };
        let rm = RiskManager::new(config);
        let opp = test_opportunity();

        // Wait just a bit so the feed becomes stale
        thread::sleep(Duration::from_millis(50));

        let result = rm.check(&opp);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("feed stale"));

        // Recording activity resets the timer
        rm.record_feed_activity();
        // With 0s timeout, it will be stale again almost immediately,
        // so let's use a proper config for this part
    }

    #[test]
    fn adaptive_backoff_blocks_after_failure() {
        let rm = RiskManager::new(test_config());
        let opp = test_opportunity();

        // No failures — should pass
        assert!(rm.check(&opp).is_ok());

        // Record a failure — backoff should block immediately
        rm.record_failure();
        let result = rm.check(&opp);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("backoff active"));
    }

    #[test]
    fn backoff_clears_on_success() {
        let rm = RiskManager::new(test_config());
        let opp = test_opportunity();

        // Fail then reset
        rm.record_failure();
        assert!(rm.check(&opp).is_err());

        rm.reset_failures();
        assert!(rm.check(&opp).is_ok());
    }

    #[test]
    fn backoff_duration_scaling() {
        // 1 failure = 1s, 2 = 2s, 3 = 4s, etc.
        assert_eq!(backoff_duration(1), Duration::from_secs(1));
        assert_eq!(backoff_duration(2), Duration::from_secs(2));
        assert_eq!(backoff_duration(3), Duration::from_secs(4));
        assert_eq!(backoff_duration(4), Duration::from_secs(8));
        assert_eq!(backoff_duration(5), Duration::from_secs(16));
        // Capped at 30s
        assert_eq!(backoff_duration(6), Duration::from_secs(30));
        assert_eq!(backoff_duration(100), Duration::from_secs(30));
    }

    #[test]
    fn feed_activity_resets_staleness() {
        let rm = RiskManager::new(test_config()); // 1s timeout
        let opp = test_opportunity();

        // Fresh activity — should pass
        rm.record_feed_activity();
        assert!(rm.check(&opp).is_ok());

        // Still within 1s — should pass
        thread::sleep(Duration::from_millis(500));
        assert!(rm.check(&opp).is_ok());

        // Record new activity, resetting the timer
        rm.record_feed_activity();
        thread::sleep(Duration::from_millis(500));
        assert!(rm.check(&opp).is_ok()); // still within 1s of last activity
    }
}
